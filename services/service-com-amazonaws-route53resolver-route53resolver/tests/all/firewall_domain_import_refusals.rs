// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `ImportFirewallDomains` refuses before it imports
//! anything, for the sentences DNS Firewall states on those refusals, and for
//! how many domains one imported file may list.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`] and the import harness of
//! [`super::firewall_domain_imports`], so what is asserted is the HTTP
//! behaviour and not a handler called directly.
//!
//! Three rules are pinned here.
//!
//! * `DomainFileUrl` is "the fully qualified URL or URI of the file stored in
//!   Amazon Simple Storage Service (Amazon S3)", so a value that is not an
//!   Amazon S3 address is refused with `ValidationException` and NO import is
//!   started: the list keeps its domains and the settled step it stood in.
//!   Being an S3 address is a question about the FORM of the value alone — an
//!   S3 address nothing is stored at is accepted, and that import then
//!   terminates in the step a failed download leaves on the list.
//! * A call that would change a domain list whose last change is still running
//!   is refused with `ConflictException`, and the sentence states the step the
//!   list stands in and quotes the list the request named, so a caller learns
//!   which list is busy and what it is busy with.
//! * An import is how a list comes to hold more domains than one
//!   `UpdateFirewallDomains` request may carry ("there is a limit of 1000
//!   domains per request"), up to the domains one list holds; a file listing
//!   more than that is a file the import cannot take, and the list keeps what it
//!   held.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    service_state, MAX_DOMAINS_PER_LIST, MAX_DOMAINS_PER_UPDATE, PROVISIONING_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::firewall_domain_imports::{
    emulator_with_object_storage, hold_object, read_list, seed_list, update, BUCKET,
};
use super::harness::{call, error_type, ok, string_member, REGION};

/// The sentence DNS Firewall states when `DomainFileUrl` is not an Amazon S3
/// address: fixed text that quotes nothing from the request, because the fault
/// is the form of the value and not which object it names.
const INVALID_S3_URL_MESSAGE: &str = "[RSLVR-02038] A valid S3 URL or S3 URI is required.";

/// The sentence a list nobody has written since its create states.
const CREATED_MESSAGE: &str = "Created Firewall Domain List";

/// The sentence a list states once a write of its domains has landed.
const CHANGED_MESSAGE: &str = "Finished domain list update";

/// The `FirewallDomainListStatus` values of the model's enum
/// (`smithy_query(query="shape",shape="FirewallDomainListStatus")`) that
/// describe work on the list still being carried out, each beside the work its
/// step is carrying out — which is what the conflict sentence names.
const IN_FLIGHT_STEPS: &[(&str, &str)] = &[
    ("IMPORTING", "import"),
    ("UPDATING", "update"),
    ("DELETING", "delete"),
];

/// The two `FirewallDomainListStatus` values a list has REACHED rather than is
/// moving through: an import is over once the list reports either of them.
const SETTLED_STATUSES: &[&str] = &["COMPLETE", "COMPLETE_IMPORT_FAILED"];

/// How long a case keeps looking for the outcome the clock owes an accepted
/// import before it gives up. Deliberately generous: the span itself is the
/// service's, and this only bounds a loop that stops the moment the list
/// reports a settled step.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long the poll loop waits before it looks at the list again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The sentence a list states once an import into it could not download the
/// file the request named: fixed text whose only variable part is the Region the
/// request was signed for.
fn download_failed(region: &str) -> String {
    format!(
        "Failed to download the file from provided url, please try with a smaller file or copy \
         the file into a {region} S3 bucket."
    )
}

/// The sentence DNS Firewall states when a call that would change `id` arrives
/// while the list stands in the in-flight `status`, whose work is `work`.
fn in_progress_message(id: &str, status: &str, work: &str) -> String {
    format!(
        "[RSLVR-02305] The DNS Firewall domain list is in {step} status: \"{id}\". Try again \
         after the {work} completes.",
        step = status.to_lowercase()
    )
}

/// Every domain the list holds, in the order it reports them.
async fn list_domains(router: &MultiServiceRouter, id: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": id, "MaxResults": 5_000 }),
    )
    .await;
    listed
        .get("Domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the response carries a Domains array, got {listed}"))
        .iter()
        .map(|domain| {
            domain
                .as_str()
                .unwrap_or_else(|| panic!("a domain is a string, got {domain}"))
                .to_string()
        })
        .collect()
}

/// Read the list until it reports a step it has REACHED, and answer that
/// projection. It waits for the CONDITION — the step the clock owes the import
/// — and never for a fixed span.
async fn poll_until_settled(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = read_list(router, id).await;
        let status = string_member(&read, "Status");
        if SETTLED_STATUSES.contains(&status.as_str()) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the list must reach one of the settled steps {SETTLED_STATUSES:?}, it still reports {status:?}: {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Issue one `ImportFirewallDomains` and answer `(http status, parsed body)`.
async fn import(router: &MultiServiceRouter, id: &str, url: &str) -> (u16, Value) {
    call(
        router,
        "ImportFirewallDomains",
        json!({
            "FirewallDomainListId": id,
            "Operation": "REPLACE",
            "DomainFileUrl": url,
        }),
    )
    .await
}

/// A `DomainFileUrl` that is not an Amazon S3 address is refused with
/// `ValidationException`, the sentence the service states for that fault, and no
/// import at all: the list keeps its domains, its `DomainCount` and the settled
/// step it stood in.
///
/// The rows span the ways a value fails to be an S3 address: no scheme at all, a
/// scheme no S3 address uses, a malformed `s3:` scheme, an S3 URI naming no
/// bucket, one naming no key, an HTTPS URL of a host that is not Amazon S3, a
/// path-style Amazon S3 URL naming no key, and the ARN form of the same object
/// (which is an identifier rather than a URL).
#[tokio::test]
async fn a_domain_file_url_that_is_not_an_s3_address_is_refused() {
    let (router, _engine) = emulator_with_object_storage();
    let held = ["kept.example.com", "also-kept.example.net"];

    let cases: Vec<(&str, String)> = vec![
        ("no scheme at all", "not-a-valid-s3-uri".to_string()),
        ("a bare file name", "domains.txt".to_string()),
        (
            "a malformed s3 scheme",
            format!("s3:/{BUCKET}/domains.txt"),
        ),
        ("an S3 URI naming no bucket", "s3:///domains.txt".to_string()),
        ("an S3 URI naming no key", format!("s3://{BUCKET}")),
        ("an S3 URI with an empty key", format!("s3://{BUCKET}/")),
        (
            "an HTTPS URL of another host",
            "https://example.com/domains.txt".to_string(),
        ),
        (
            "a scheme no S3 address uses",
            format!("ftp://{BUCKET}.s3.{REGION}.amazonaws.com/domains.txt"),
        ),
        (
            "a path-style Amazon S3 URL naming no key",
            format!("https://s3.{REGION}.amazonaws.com/{BUCKET}"),
        ),
        (
            "the ARN form of the object",
            format!("arn:aws:s3:::{BUCKET}/domains.txt"),
        ),
    ];

    for (index, (case, url)) in cases.into_iter().enumerate() {
        let id = seed_list(&router, &format!("invalid-url-{index}")).await;
        update(&router, &id, "ADD", &held).await;

        let (status, refusal) = import(&router, &id, &url).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {url:?} must be refused, got {status}: {refusal}"
        );
        assert!(
            error_type(&refusal).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {refusal}"
        );
        assert_eq!(
            string_member(&refusal, "Message"),
            INVALID_S3_URL_MESSAGE,
            "case {case:?}: the refusal must state the fault as AWS words it, got {refusal}"
        );

        // No import was started: the list stands in the settled step its last
        // write left it in and still holds exactly what that write wrote.
        let kept = read_list(&router, &id).await;
        assert_eq!(
            string_member(&kept, "Status"),
            "COMPLETE",
            "case {case:?}: the refused import must leave the list settled, got {kept}"
        );
        assert_eq!(
            string_member(&kept, "StatusMessage"),
            CHANGED_MESSAGE,
            "case {case:?}: the refused import must leave the step's sentence alone, got {kept}"
        );
        assert_eq!(
            kept.get("DomainCount").and_then(Value::as_i64),
            Some(2),
            "case {case:?}: DomainCount follows the domains the list still holds, got {kept}"
        );
        let domains = list_domains(&router, &id).await;
        assert_eq!(
            domains,
            held.iter()
                .map(|domain| format!("{domain}."))
                .collect::<Vec<_>>(),
            "case {case:?}: the refused import must write nothing, got {domains:?}"
        );
    }
}

/// The refusal above is about the FORM of the value: every addressing form of an
/// S3 object is ACCEPTED even when the account holds no object there, and that
/// import runs and terminates in the step a failed download leaves on the list.
///
/// The rows are the three addressing forms of one object — the `s3://` URI, the
/// virtual-hosted HTTPS URL and the path-style one — each naming a key nothing
/// was stored at.
#[tokio::test]
async fn an_s3_address_nothing_is_stored_at_starts_an_import_that_fails_to_download() {
    let (router, _engine) = emulator_with_object_storage();

    let cases: Vec<(&str, String)> = vec![
        ("the s3 URI", format!("s3://{BUCKET}/absent/domains.txt")),
        (
            "the virtual-hosted URL",
            format!("https://{BUCKET}.s3.{REGION}.amazonaws.com/absent/domains.txt"),
        ),
        (
            "the path-style URL",
            format!("https://s3.{REGION}.amazonaws.com/{BUCKET}/absent/domains.txt"),
        ),
    ];

    for (index, (case, url)) in cases.into_iter().enumerate() {
        let id = seed_list(&router, &format!("absent-object-{index}")).await;

        let (status, answered) = import(&router, &id, &url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an S3 address is accepted whatever is stored at it, got {status}: {answered}"
        );
        assert_eq!(
            string_member(&answered, "Status"),
            "IMPORTING",
            "case {case:?}: an accepted import is under way when it answers, got {answered}"
        );

        let settled = poll_until_settled(&router, &id, case).await;
        assert_eq!(
            string_member(&settled, "Status"),
            "COMPLETE_IMPORT_FAILED",
            "case {case:?}: an import that could not download its file terminates in the failed step, got {settled}"
        );
        assert_eq!(
            string_member(&settled, "StatusMessage"),
            download_failed(REGION),
            "case {case:?}: the list must state the download failure, got {settled}"
        );
    }
}

/// A list whose last change is still running takes no further change: the call
/// is refused with `ConflictException`, the sentence states the step the list
/// stands in and quotes the list the request named, and the record is left
/// exactly as it was.
///
/// The rows span what the rule generalises over: each in-flight
/// `FirewallDomainListStatus` the model declares ([`IN_FLIGHT_STEPS`]) against
/// each operation that changes a list — the import and the delete.
#[tokio::test]
async fn a_list_with_a_change_in_flight_refuses_the_next_one_and_names_the_step() {
    let (router, engine) = emulator_with_object_storage();
    let held = ["busy.example.com", "still-busy.example.org"];
    let url = format!("s3://{BUCKET}/in-flight.txt");
    hold_object(&engine, "in-flight.txt", b"imported.example.com\n".to_vec());

    for &(status, work) in IN_FLIGHT_STEPS {
        for operation in ["ImportFirewallDomains", "DeleteFirewallDomainList"] {
            let case = format!("{operation} on a list reported {status}");
            let id = seed_list(&router, &format!("in-flight-{status}-{operation}")).await;
            update(&router, &id, "ADD", &held).await;

            // Put the list in the step this row is about, through the store the
            // service reads: every write this service serves applies its change
            // inside the call, so only an import leaves a list in flight — and
            // the other two steps are reached the same way a longer-running
            // change would reach them.
            {
                let state = service_state(&*engine)
                    .expect("the store is reachable from the engine the service is mounted on");
                let mut store = state.lock();
                let record = store
                    .firewall_domain_lists
                    .get_mut(&id)
                    .expect("the create wrote a record for the list it answered with");
                record.status = status.to_string();
                record.settled_at = Some(Instant::now() + PROVISIONING_DELAY);
            }
            let reported = read_list(&router, &id).await;
            assert_eq!(
                string_member(&reported, "Status"),
                status,
                "case {case:?}: the change has not settled yet, got {reported}"
            );

            let request = if operation == "ImportFirewallDomains" {
                json!({
                    "FirewallDomainListId": id.clone(),
                    "Operation": "REPLACE",
                    "DomainFileUrl": url.clone(),
                })
            } else {
                json!({ "FirewallDomainListId": id.clone() })
            };
            let (http_status, refusal) = call(&router, operation, request).await;
            assert_eq!(
                http_status, 400,
                "case {case:?}: the call must be refused, got {http_status}: {refusal}"
            );
            assert!(
                error_type(&refusal).contains("ConflictException"),
                "case {case:?}: the refusal must name ConflictException, got {refusal}"
            );
            assert_eq!(
                string_member(&refusal, "Message"),
                in_progress_message(&id, status, work),
                "case {case:?}: the refusal must state the step as AWS words it, got {refusal}"
            );

            // The refusal wrote nothing: the list is still there, still holding
            // the domains the update wrote.
            let kept = read_list(&router, &id).await;
            assert_eq!(
                kept.get("DomainCount").and_then(Value::as_i64),
                Some(2),
                "case {case:?}: the refused call must leave the list as it was, got {kept}"
            );
            let domains = list_domains(&router, &id).await;
            assert_eq!(
                domains,
                held.iter()
                    .map(|domain| format!("{domain}."))
                    .collect::<Vec<_>>(),
                "case {case:?}: the refused call must write nothing, got {domains:?}"
            );
        }
    }
}

/// The recorded shape of the conflict: a second import issued while the first
/// one is still running is refused, naming the list and the import step, and the
/// list the first import wrote is left to finish it.
///
/// The rows are the addressing form the second import uses, so the refusal is
/// shown to follow from the list's step and not from how the second request
/// addressed its file.
#[tokio::test]
async fn a_second_import_while_the_first_is_running_is_refused() {
    let (router, engine) = emulator_with_object_storage();
    hold_object(&engine, "first.txt", b"first.example.com\n".to_vec());
    hold_object(&engine, "second.txt", b"second.example.com\n".to_vec());

    let forms: Vec<(&str, String)> = vec![
        ("the s3 URI", format!("s3://{BUCKET}/second.txt")),
        (
            "the virtual-hosted URL",
            format!("https://{BUCKET}.s3.{REGION}.amazonaws.com/second.txt"),
        ),
    ];

    for (index, (case, second_url)) in forms.into_iter().enumerate() {
        let id = seed_list(&router, &format!("second-import-{index}")).await;
        let created = read_list(&router, &id).await;
        assert_eq!(
            string_member(&created, "StatusMessage"),
            CREATED_MESSAGE,
            "case {case:?}: a list nobody has written states its creation step, got {created}"
        );

        let (status, accepted) = import(&router, &id, &format!("s3://{BUCKET}/first.txt")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first import must be accepted, got {status}: {accepted}"
        );

        let (refused_status, refusal) = import(&router, &id, &second_url).await;
        assert_eq!(
            refused_status, 400,
            "case {case:?}: the second import must be refused, got {refused_status}: {refusal}"
        );
        assert_eq!(
            string_member(&refusal, "Message"),
            in_progress_message(&id, "IMPORTING", "import"),
            "case {case:?}: the refusal must name the list and its step, got {refusal}"
        );

        // The first import is the one that runs: once the clock has made its
        // outcome due the list holds the file THAT import named.
        let settled = poll_until_settled(&router, &id, case).await;
        assert_eq!(
            string_member(&settled, "Status"),
            "COMPLETE",
            "case {case:?}: the first import must still land, got {settled}"
        );
        let domains = list_domains(&router, &id).await;
        assert_eq!(
            domains,
            vec!["first.example.com.".to_string()],
            "case {case:?}: the refused second import wrote nothing, got {domains:?}"
        );
    }
}

/// An imported file may list every domain one list holds — far more than the
/// 1000 domains one `UpdateFirewallDomains` request may carry — and the whole
/// file lands: the list reports exactly that many domains. A file listing one
/// more than a list holds is a file the import cannot take, so the list keeps
/// what it held and the import terminates in the failed step.
///
/// The rows are the number of domains the file lists, read off the two bounds
/// the slice carries: one past what a single update may carry
/// ([`MAX_DOMAINS_PER_UPDATE`]), the domains one list holds
/// ([`MAX_DOMAINS_PER_LIST`]), and one past that.
#[tokio::test]
async fn an_imported_file_may_list_every_domain_one_list_holds() {
    let (router, engine) = emulator_with_object_storage();

    for (case, count, lands) in [
        (
            "one domain more than a single update may carry",
            MAX_DOMAINS_PER_UPDATE + 1,
            true,
        ),
        (
            "as many domains as one list holds",
            MAX_DOMAINS_PER_LIST,
            true,
        ),
        (
            "one domain more than one list holds",
            MAX_DOMAINS_PER_LIST + 1,
            false,
        ),
    ] {
        let key = format!("counted/{count}.txt");
        let body: String = (0..count)
            .map(|index| format!("d{index}.example.com\n"))
            .collect();
        hold_object(&engine, &key, body.into_bytes());

        let id = seed_list(&router, &format!("counted-{count}")).await;
        let held = ["kept.example.com"];
        update(&router, &id, "ADD", &held).await;

        let (status, accepted) = import(&router, &id, &format!("s3://{BUCKET}/{key}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the import must be accepted, got {status}: {accepted}"
        );

        let settled = poll_until_settled(&router, &id, case).await;
        let expected_status = if lands {
            "COMPLETE"
        } else {
            "COMPLETE_IMPORT_FAILED"
        };
        assert_eq!(
            string_member(&settled, "Status"),
            expected_status,
            "case {case:?}: expected the import to settle in {expected_status}, got {settled}"
        );
        // `REPLACE` makes the list "exactly match the list in the file", so a
        // file that lands is the whole count; one that does not leaves the
        // domains the update wrote.
        let expected_count = if lands { count } else { held.len() };
        assert_eq!(
            settled.get("DomainCount").and_then(Value::as_i64),
            Some(i64::try_from(expected_count).expect("a count that fits")),
            "case {case:?}: expected DomainCount {expected_count}, got {settled}"
        );
    }
}
