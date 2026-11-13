// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the lifecycle step a DNS Firewall domain list stands in
//! while an import of a domain file into it is under way, and for the sentence
//! that step states in `StatusMessage`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`] and the import harness of
//! [`super::firewall_domain_imports`], so what is asserted is the HTTP
//! behaviour and not a handler called directly.
//!
//! What these cases pin is the rule the slice implements: a domain list stands
//! in ONE lifecycle step at a time, and every projection of it — the write that
//! moved it and every later read — reports that step both as `Status` and as
//! the sentence that step states in `StatusMessage`. No step a caller can
//! observe a list in is left without its sentence, so a client that polls a
//! list always learns which operation the step it reports belongs to; and the
//! sentence belongs to the STEP, so it carries nothing the request chose and is
//! the same for every list standing in it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::firewall_domain_imports::{
    emulator_with_object_storage, hold_object, read_list, seed_list, update, BUCKET,
};
use super::harness::{ok, string_member, REGION};

/// The sentence DNS Firewall states on a list whose import of the file the
/// request named is still running — the `IMPORTING` step's own sentence, fixed
/// text with no part of the request in it.
const IMPORTING_MESSAGE: &str = "Importing domains from provided file.";

/// The sentence a list states once a write of its domains has landed, whether
/// the write came from an update or from an import that went through.
const CHANGED_MESSAGE: &str = "Finished domain list update";

/// The sentence a list nobody has written since its create states.
const CREATED_MESSAGE: &str = "Created Firewall Domain List";

/// The sentence a list states once an import into it could not download the
/// file the request named: fixed text whose only variable part is the Region
/// the request was signed for.
fn download_failed(region: &str) -> String {
    format!(
        "Failed to download the file from provided url, please try with a smaller file or copy \
         the file into a {region} S3 bucket."
    )
}

/// The `FirewallDomainListStatus` values the model declares
/// (`smithy_query(query="shape",shape="FirewallDomainListStatus")`), which any
/// status a response reports must be one of.
const DOMAIN_LIST_STATUSES: &[&str] = &[
    "COMPLETE",
    "COMPLETE_IMPORT_FAILED",
    "IMPORTING",
    "DELETING",
    "UPDATING",
];

/// The two of those a list has REACHED rather than is moving through: an
/// import is over once the list reports either of them.
const SETTLED_STATUSES: &[&str] = &["COMPLETE", "COMPLETE_IMPORT_FAILED"];

/// `StatusMessage`'s modelled maximum length, from the member's `length` trait
/// (0..255) on `FirewallDomainList$StatusMessage`.
const STATUS_MESSAGE_MAX: usize = 255;

/// How long a case keeps looking for the outcome the clock owes an accepted
/// import before it gives up. Deliberately generous: the span itself is the
/// service's, and this only bounds a loop that stops the moment the list
/// reports a settled step.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long the poll loop waits before it looks at the list again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The sentence the step `status` states on a list whose domains a caller has
/// written, for a request signed for `region` — the mapping every projection of
/// a list has to agree with.
fn sentence_for(status: &str, region: &str) -> String {
    match status {
        "COMPLETE" => CHANGED_MESSAGE.to_string(),
        "COMPLETE_IMPORT_FAILED" => download_failed(region),
        "IMPORTING" => IMPORTING_MESSAGE.to_string(),
        other => panic!("no import moves a list through {other:?}"),
    }
}

/// Assert `projection` reports the step it stands in — `expected_status` when
/// the case knows which step that is — and the sentence that step states,
/// within the member's modelled length.
fn assert_step(
    projection: &Value,
    expected_status: Option<&str>,
    expected_message: &str,
    case: &str,
) {
    let status = string_member(projection, "Status");
    assert!(
        DOMAIN_LIST_STATUSES.contains(&status.as_str()),
        "case {case:?}: Status must be one of the modelled {DOMAIN_LIST_STATUSES:?}, got {status:?}"
    );
    if let Some(expected) = expected_status {
        assert_eq!(
            status, expected,
            "case {case:?}: expected Status {expected:?}, got {projection}"
        );
    }
    let message = string_member(projection, "StatusMessage");
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected the {status:?} step's sentence {expected_message:?}, got {projection}"
    );
    assert!(
        !message.is_empty() && message.len() <= STATUS_MESSAGE_MAX,
        "case {case:?}: StatusMessage must satisfy its modelled length 0..{STATUS_MESSAGE_MAX}, got {message:?}"
    );
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

/// One row of the import-step table: the object the account holds for the
/// import to read, if any, the URL the request names it by, and the step the
/// clock owes that import.
struct ImportStepCase {
    case: &'static str,
    /// `(key, body)` of the object this row stores first, or `None` for a row
    /// whose URL addresses no object at all.
    object: Option<(&'static str, &'static str)>,
    url: String,
    settled_status: &'static str,
}

/// Every step an import moves a domain list through states that step's own
/// sentence: the creation step before anything writes the list, the
/// finished-change step after a write, the import step while the import runs,
/// and the step the import settles in.
///
/// The rows span what the rule generalises over — the addressing form of the
/// file (the `s3://` URI and the virtual-hosted HTTPS URL), the number of
/// domains the file lists, and both outcomes an import has (a file that is read
/// and a URL that addresses no file the service can download).
#[tokio::test]
async fn every_step_an_import_moves_a_list_through_states_that_steps_sentence() {
    let (router, engine) = emulator_with_object_storage();

    let cases = vec![
        ImportStepCase {
            case: "the s3 URI of a readable file, one domain",
            object: Some(("steps/one.txt", "ads.example.com\n")),
            url: format!("s3://{BUCKET}/steps/one.txt"),
            settled_status: "COMPLETE",
        },
        ImportStepCase {
            case: "the virtual-hosted URL of a readable file, three domains",
            object: Some((
                "steps/three.txt",
                "a.example.com\nb.example.org\nc.example.net\n",
            )),
            url: format!("https://{BUCKET}.s3.{REGION}.amazonaws.com/steps/three.txt"),
            settled_status: "COMPLETE",
        },
        ImportStepCase {
            case: "a key of the account's bucket nothing was stored at",
            object: None,
            url: format!("s3://{BUCKET}/steps/never-written.txt"),
            settled_status: "COMPLETE_IMPORT_FAILED",
        },
        ImportStepCase {
            case: "a bucket the account does not hold",
            object: None,
            url: "s3://no-such-bucket-for-steps/domains.txt".to_string(),
            settled_status: "COMPLETE_IMPORT_FAILED",
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let ImportStepCase {
            case,
            object,
            url,
            settled_status,
        } = row;
        if let Some((key, body)) = object {
            hold_object(&engine, key, body.as_bytes().to_vec());
        }

        let name = format!("import-step-{index}");
        let id = seed_list(&router, &name).await;
        assert_step(
            &read_list(&router, &id).await,
            Some("COMPLETE"),
            CREATED_MESSAGE,
            &format!("{case}: the created list"),
        );

        // A write of the list's own domains first, so the step the import
        // settles in is told apart from the creation step as well.
        update(&router, &id, "ADD", &["kept.example.com"]).await;
        assert_step(
            &read_list(&router, &id).await,
            Some("COMPLETE"),
            CHANGED_MESSAGE,
            &format!("{case}: after a write of its domains"),
        );

        let answered = ok(
            &router,
            "ImportFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": "REPLACE",
                "DomainFileUrl": url.clone(),
            }),
        )
        .await;
        assert_eq!(
            string_member(&answered, "Id"),
            id,
            "case {case:?}: the import must report the list it is importing into, got {answered}"
        );
        assert_step(
            &answered,
            Some("IMPORTING"),
            IMPORTING_MESSAGE,
            &format!("{case}: the import acknowledgement"),
        );
        // The sentence is the step's, so it quotes nothing the caller chose.
        let acknowledged = string_member(&answered, "StatusMessage");
        for (member, value) in [
            ("DomainFileUrl", url.as_str()),
            ("Name", name.as_str()),
            ("Id", id.as_str()),
        ] {
            assert!(
                !acknowledged.contains(value),
                "case {case:?}: StatusMessage must not quote {member} ({value:?}), got {acknowledged:?}"
            );
        }

        // Whatever step a read reports, the sentence beside it is that step's:
        // a read taken while the import is still in flight states the import
        // step, and one taken after it states the step it settled in.
        let read = read_list(&router, &id).await;
        let reported = string_member(&read, "Status");
        assert_step(
            &read,
            None,
            &sentence_for(&reported, REGION),
            &format!("{case}: a read taken right after the import was accepted"),
        );

        let settled = poll_until_settled(&router, &id, case).await;
        assert_step(
            &settled,
            Some(settled_status),
            &sentence_for(settled_status, REGION),
            &format!("{case}: the settled list"),
        );
    }
}

/// The import step's sentence belongs to the STEP and not to one request: every
/// import in flight states the same sentence, whatever list it writes, whatever
/// form its URL takes and whatever the file holds.
#[tokio::test]
async fn the_import_sentence_belongs_to_the_step_and_not_to_one_request() {
    let (router, engine) = emulator_with_object_storage();

    // (what the message was read from, the message) for imports that differ in
    // every part of the request a caller chooses.
    let mut seen: Vec<(String, String)> = Vec::new();
    for (index, body) in [
        "one.example.com\n",
        "two.example.com\nthree.example.org\n",
        "\n",
    ]
    .into_iter()
    .enumerate()
    {
        let key = format!("sentence/{index}.txt");
        hold_object(&engine, &key, body.as_bytes().to_vec());
        for (form_index, (form, url)) in [
            ("the s3 URI", format!("s3://{BUCKET}/{key}")),
            (
                "the virtual-hosted URL",
                format!("https://{BUCKET}.s3.{REGION}.amazonaws.com/{key}"),
            ),
            (
                "the path-style URL",
                format!("https://s3.{REGION}.amazonaws.com/{BUCKET}/{key}"),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            // Its OWN list per row: each import must be the first one in flight
            // on the list it writes, so what is read back is that import's
            // acknowledgement and not a refusal of a second, overlapping one.
            let id = seed_list(&router, &format!("sentence-{index}-{form_index}")).await;
            let answered = ok(
                &router,
                "ImportFirewallDomains",
                json!({
                    "FirewallDomainListId": id,
                    "Operation": "REPLACE",
                    "DomainFileUrl": url,
                }),
            )
            .await;
            seen.push((
                format!("{form} of the file holding {} byte(s)", body.len()),
                string_member(&answered, "StatusMessage"),
            ));
        }
    }

    for (case, message) in &seen {
        assert_eq!(
            message, IMPORTING_MESSAGE,
            "case {case:?}: expected the import step's sentence {IMPORTING_MESSAGE:?}, got {message:?}"
        );
    }
    assert_eq!(
        seen.len(),
        9,
        "case \"every import form\": every acknowledgement was read, got {seen:?}"
    );
}
