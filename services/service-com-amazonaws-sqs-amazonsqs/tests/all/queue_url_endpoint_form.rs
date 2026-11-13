// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FORM of every queue url this service reports, driven over the
//! wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared
//! harness in [`super::support`].
//!
//! The rule under test: each address uses the emulator's published endpoint, not an AWS
//! hostname or the request's Host header. Region remains in the resource path so the
//! same name in different regions has a distinct canonical Cloud Control identifier.
//!
//! The tables span what the rule generalises over: the REGION the call is made in (four of
//! them), the queue NAME (every character class the member admits, its shortest value, the
//! published length bound and one past it, and a FIFO name), the OPERATION the url is
//! reported by (`CreateQueue`, the repeat of a create, `GetQueueUrl`, `ListQueues` and
//! `ListDeadLetterSourceQueues`), the COUNT of urls in a listing (none, one, several) and
//! the ADDRESS the request arrived on (the service's own endpoint, another region's
//! endpoint, loopback with an ephemeral port, a host name with a port and an IPv6
//! literal).

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::MAX_QUEUE_NAME_LENGTH;

use super::support::{
    account, call_in, create_in, create_ok_in, emulator, expected_arn_in, expected_url_in,
    fifo_attributes, host_in, ok_in, ok_on_host, REGION,
};

/// The regions every table walks: the harness's own and three further ones, so no row can
/// be satisfied by an authority keyed to one region label.
const REGIONS: &[&str] = &[REGION, "us-west-2", "eu-central-1", "ap-southeast-2"];

/// The endpoint prefix Amazon SQS publishes its regional endpoints under, which is the
/// first label of the authority a queue url carries. Spelled out here rather than read from
/// the crate, so a row fails if the service starts rendering urls on some other host.
const PUBLISHED_AUTHORITY: &str = "emulator.invalid";

/// One queue name the url form is asserted over.
struct NameCase {
    /// What the row is about, for the assertion messages.
    case: &'static str,
    /// The `QueueName` the create carries.
    name: String,
    /// The `Attributes` it carries: a FIFO name needs the attribute that makes the queue
    /// a FIFO queue, and nothing else here needs any attribute at all.
    attributes: Value,
    /// Whether a queue is created at all. A name past the published bound is refused, and
    /// a refused create mints no url — the one row where no url may appear.
    accepted: bool,
}

/// The name catalog, from `CreateQueueRequest$QueueName`'s own rules ("A queue name can
/// have up to 80 characters", "Valid values: alphanumeric characters, hyphens (-), and
/// underscores (_)", "A FIFO queue name must end with the .fifo suffix", "Queue URLs and
/// names are case-sensitive"): the shortest name, each character class, mixed case, the
/// published length bound, one character past it, and a FIFO name — none of them the name
/// any recorded call used.
fn name_cases() -> Vec<NameCase> {
    let at_the_bound = format!(
        "url-form-{}",
        "a".repeat(MAX_QUEUE_NAME_LENGTH - "url-form-".len())
    );
    let past_the_bound = format!("{at_the_bound}b");
    assert_eq!(
        (at_the_bound.chars().count(), past_the_bound.chars().count()),
        (MAX_QUEUE_NAME_LENGTH, MAX_QUEUE_NAME_LENGTH + 1),
        "case the length rows: expected the bound and one past it, got {} and {}",
        at_the_bound.chars().count(),
        past_the_bound.chars().count()
    );
    vec![
        NameCase {
            case: "the shortest name",
            name: "q".to_string(),
            attributes: json!({}),
            accepted: true,
        },
        NameCase {
            case: "hyphens, underscores, digits and mixed case",
            name: "Url-Form_Queue-07".to_string(),
            attributes: json!({}),
            accepted: true,
        },
        NameCase {
            case: "underscores only",
            name: "url_form_underscored".to_string(),
            attributes: json!({}),
            accepted: true,
        },
        NameCase {
            case: "a FIFO name",
            name: "url-form-ordered.fifo".to_string(),
            attributes: fifo_attributes(&[]),
            accepted: true,
        },
        NameCase {
            case: "the published length bound",
            name: at_the_bound,
            attributes: json!({}),
            accepted: true,
        },
        NameCase {
            case: "one character past the published length bound",
            name: past_the_bound,
            attributes: json!({}),
            accepted: false,
        },
    ]
}

/// The url the service must report for the queue named `name` in `region`, composed here
/// from the parts the rule names rather than taken out of any answer.
///
/// Deliberately a second spelling of the form [`expected_url_in`] composes: the two are
/// cross-checked in [`assert_regional_form`], so this suite fails if the harness and the
/// rule ever drift apart.
fn regional_url(region: &str, name: &str) -> String {
    format!(
        "https://{PUBLISHED_AUTHORITY}/sqs/{region}/{}/{name}",
        account()
    )
}

/// Assert that `url` is the regional endpoint form for `region` and `name`, component by
/// component: the scheme, the authority, and the two path segments.
fn assert_regional_form(case: &str, region: &str, name: &str, url: &str) {
    let expected = regional_url(region, name);
    assert_eq!(
        url, expected,
        "case {case} in {region}: expected {expected:?}, got {url:?}"
    );
    let authority_and_path = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case} in {region}: expected an https url, got {url:?}"));
    let (authority, path) = authority_and_path
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case} in {region}: expected a path, got {url:?}"));
    assert_eq!(
        authority, PUBLISHED_AUTHORITY,
        "case {case} in {region}: the authority must be the regional endpoint, \
         got {authority:?}"
    );
    assert_eq!(
        path.split('/').collect::<Vec<&str>>(),
        vec!["sqs", region, account(), name],
        "case {case} in {region}: expected /<accountId>/<QueueName>, got {path:?}"
    );
    // The harness's own derivation of the same form, so the suite and every other suite
    // in this crate cannot drift on it.
    assert_eq!(
        url,
        expected_url_in(region, name),
        "case {case} in {region}: the harness form must be this form"
    );
}

/// The `QueueUrls` of a `ListQueues` reply, as a list — a listing that reports nothing
/// carries no member at all.
fn listed_urls(reply: &Value) -> Vec<String> {
    strings(reply, "QueueUrls")
}

/// The `queueUrls` of a `ListDeadLetterSourceQueues` reply, as a list.
fn source_urls(reply: &Value) -> Vec<String> {
    strings(reply, "queueUrls")
}

/// The string list `member` of a reply, empty when the member is absent.
fn strings(reply: &Value, member: &str) -> Vec<String> {
    reply
        .get(member)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every operation that carries a queue url reports the regional endpoint form, for every
/// region and every shape of name — and the url it reports addresses the queue it names.
#[tokio::test]
async fn every_reported_queue_url_is_the_regional_endpoint_form() {
    let router = emulator();
    for region in REGIONS {
        for NameCase {
            case,
            name,
            attributes,
            accepted,
        } in name_cases()
        {
            let (status, created) = create_in(&router, region, &name, &attributes).await;
            if !accepted {
                assert_eq!(
                    status, 400,
                    "case {case} in {region}: a name past the bound must be refused, \
                     got {status} with {created}"
                );
                assert!(
                    created.get("QueueUrl").is_none(),
                    "case {case} in {region}: a refused create must mint no url, got {created}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {case} in {region}: expected the queue to be created, \
                 got {status} with {created}"
            );
            let url = created["QueueUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case} in {region}: no QueueUrl in {created}"))
                .to_string();
            assert_regional_form(case, region, &name, &url);

            // The repeat of a create that asks for nothing different is answered with the
            // url the queue already carries.
            let again = ok_in(
                &router,
                region,
                "CreateQueue",
                match attributes.as_object().is_some_and(|map| !map.is_empty()) {
                    true => json!({"QueueName": &name, "Attributes": attributes}),
                    false => json!({"QueueName": &name}),
                },
            )
            .await;
            assert_eq!(
                again["QueueUrl"].as_str(),
                Some(url.as_str()),
                "case {case} in {region}: a repeated create must report the same url, \
                 got {again}"
            );

            // The name lookup reports the same string.
            let resolved = ok_in(&router, region, "GetQueueUrl", json!({"QueueName": &name})).await;
            assert_eq!(
                resolved["QueueUrl"].as_str(),
                Some(url.as_str()),
                "case {case} in {region}: GetQueueUrl must report the same url, got {resolved}"
            );

            // So does the listing of exactly this queue.
            let listed = ok_in(
                &router,
                region,
                "ListQueues",
                json!({"QueueNamePrefix": &name}),
            )
            .await;
            assert_eq!(
                listed_urls(&listed),
                vec![url.clone()],
                "case {case} in {region}: ListQueues must report the same url, got {listed}"
            );

            // And the url addresses the queue it names: the ARN read back through it is
            // that queue's.
            let reported = ok_in(
                &router,
                region,
                "GetQueueAttributes",
                json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
            )
            .await;
            assert_eq!(
                reported["Attributes"]["QueueArn"].as_str(),
                Some(expected_arn_in(region, &name).as_str()),
                "case {case} in {region}: the url must address the queue it names, \
                 got {reported}"
            );
        }
    }
}

/// The url reported for a queue does not depend on the address the request arrived on: the
/// same queue reports one url through every address, a create made through any of them
/// mints the regional form of the region its credential scope names, and a url minted for
/// the regional endpoint still addresses its queue when the call arrives on a loopback
/// address.
#[tokio::test]
async fn a_queue_url_is_the_same_whatever_address_the_call_arrived_on() {
    // The addresses span what a request can arrive on: the service's own regional
    // endpoint, the endpoint of a DIFFERENT region (the region is the credential scope's
    // to say, never the host's), loopback with an ephemeral port, a host name with the
    // emulator's own port, and an IPv6 literal.
    let addresses: Vec<String> = vec![
        host_in(REGION),
        host_in("eu-west-1"),
        "127.0.0.1:40085".to_string(),
        "localhost:9297".to_string(),
        "[::1]:9297".to_string(),
    ];

    let router = emulator();
    let shared = "url-form-shared";
    let shared_url = create_ok_in(&router, REGION, shared, &json!({})).await;
    assert_regional_form("the shared queue", REGION, shared, &shared_url);

    for (index, address) in addresses.iter().enumerate() {
        // 1. The stored queue reports its one url through this address.
        let resolved = ok_on_host(
            &router,
            address,
            REGION,
            "GetQueueUrl",
            json!({"QueueName": shared}),
        )
        .await;
        assert_eq!(
            resolved["QueueUrl"].as_str(),
            Some(shared_url.as_str()),
            "case GetQueueUrl on {address}: expected {shared_url:?}, got {resolved}"
        );

        // 2. A create made through this address mints the regional form all the same.
        let name = format!("url-form-address-{index}");
        let created = ok_on_host(
            &router,
            address,
            REGION,
            "CreateQueue",
            json!({"QueueName": &name}),
        )
        .await;
        let minted = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case CreateQueue on {address}: no QueueUrl in {created}"))
            .to_string();
        assert_regional_form("a create arriving on an address", REGION, &name, &minted);
        // An address that is not this region's own endpoint must appear nowhere in the
        // url; the region's own endpoint is the url's authority by the rule itself.
        if *address != host_in(REGION) {
            assert!(
                !minted.contains(address.as_str()),
                "case CreateQueue on {address}: the url must not carry the address the call \
                 arrived on, got {minted:?}"
            );
        }

        // 3. The listing read through this address reports every url in the same form.
        let listed = ok_on_host(&router, address, REGION, "ListQueues", json!({})).await;
        let urls = listed_urls(&listed);
        assert!(
            urls.contains(&shared_url) && urls.contains(&minted),
            "case ListQueues on {address}: expected the listing to hold {shared_url:?} and \
             {minted:?}, got {urls:?}"
        );
        let authority = format!("https://{PUBLISHED_AUTHORITY}/sqs/{REGION}/");
        for url in &urls {
            assert!(
                url.starts_with(&authority),
                "case ListQueues on {address}: every url must be on the regional endpoint, \
                 got {url:?}"
            );
        }

        // 4. And the url minted for the regional endpoint still addresses its queue when
        //    the call arrives here, so the form costs no reachability.
        let reported = ok_on_host(
            &router,
            address,
            REGION,
            "GetQueueAttributes",
            json!({"QueueUrl": &shared_url, "AttributeNames": ["QueueArn"]}),
        )
        .await;
        assert_eq!(
            reported["Attributes"]["QueueArn"].as_str(),
            Some(expected_arn_in(REGION, shared).as_str()),
            "case GetQueueAttributes on {address}: the url must still address its queue, \
             got {reported}"
        );
    }
}

/// The dead-letter source listing reports each source queue's own url, in the same form
/// the create minted it in — for a dead-letter queue with no source, one, and several, and
/// whatever address the listing is read through.
#[tokio::test]
async fn the_dead_letter_source_listing_reports_the_urls_the_creates_minted() {
    // The counts span what the listing generalises over: a dead-letter queue nothing
    // redrives to, one source, and several.
    for sources in [0_usize, 1, 3] {
        let router = emulator();
        let dead_letter = format!("url-form-dlq-{sources}");
        let dead_letter_url = create_ok_in(&router, REGION, &dead_letter, &json!({})).await;
        let target_arn = expected_arn_in(REGION, &dead_letter);

        let mut expected: Vec<String> = Vec::with_capacity(sources);
        for index in 0..sources {
            let name = format!("url-form-source-{sources}-{index}");
            let policy = json!({"deadLetterTargetArn": &target_arn, "maxReceiveCount": "3"});
            let url = create_ok_in(
                &router,
                REGION,
                &name,
                &json!({"RedrivePolicy": policy.to_string()}),
            )
            .await;
            assert_regional_form("a source queue", REGION, &name, &url);
            expected.push(url);
        }
        expected.sort();

        // Read through the regional endpoint and through a loopback address: the elements
        // are the same strings either way.
        for address in [host_in(REGION), "127.0.0.1:40225".to_string()] {
            let reply = ok_on_host(
                &router,
                &address,
                REGION,
                "ListDeadLetterSourceQueues",
                json!({"QueueUrl": &dead_letter_url}),
            )
            .await;
            let mut reported = source_urls(&reply);
            reported.sort();
            assert_eq!(
                reported, expected,
                "case {sources} sources listed on {address}: expected {expected:?}, \
                 got {reported:?}"
            );
        }
    }
}

/// A queue url reported by this service resolves back to its queue: a name whose url is
/// taken from one operation and handed to another addresses the same queue, and no other.
#[tokio::test]
async fn a_reported_url_round_trips_through_the_operations_that_take_one() {
    let router = emulator();
    let name = "url-form-round-trip";
    let created = create_ok_in(&router, REGION, name, &json!({})).await;
    let resolved = ok_in(&router, REGION, "GetQueueUrl", json!({"QueueName": name})).await;
    let from_lookup = resolved["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case the lookup: no QueueUrl in {resolved}"))
        .to_string();
    assert_eq!(
        from_lookup, created,
        "case the lookup: expected {created:?}, got {from_lookup:?}"
    );

    // Every url-addressed operation this suite needs to prove the url is a working
    // address, each one driven with the url the LOOKUP reported.
    let sent = ok_in(
        &router,
        REGION,
        "SendMessage",
        json!({"QueueUrl": &from_lookup, "MessageBody": "through a reported url"}),
    )
    .await;
    assert!(
        sent.get("MessageId").is_some(),
        "case SendMessage through a reported url: expected a message id, got {sent}"
    );
    let tagged = call_in(
        &router,
        REGION,
        "TagQueue",
        &json!({"QueueUrl": &from_lookup, "Tags": {"suite": "url-form"}}),
    )
    .await;
    assert_eq!(
        tagged.0, 200,
        "case TagQueue through a reported url: expected HTTP 200, got {tagged:?}"
    );
    let tags = ok_in(
        &router,
        REGION,
        "ListQueueTags",
        json!({"QueueUrl": &from_lookup}),
    )
    .await;
    assert_eq!(
        tags["Tags"]["suite"].as_str(),
        Some("url-form"),
        "case ListQueueTags through a reported url: expected the tag just set, got {tags}"
    );
}

/// The configured base, including its port, is the resource-address authority even
/// when the request arrives with another Host. Region stays a path identity, not a
/// public AWS hostname. This uses only the existing in-process transport.
#[tokio::test]
async fn published_endpoint_not_request_host_owns_queue_addresses() {
    let mut router = emulator();
    let base = "https://sandbox.test.invalid:18443";
    router.set_url_base(smithy_server_runtime::EndpointUrl::parse(base).unwrap());
    let mut urls = Vec::new();
    for region in ["us-east-1", "us-west-2"] {
        let name = "same-name-published-base";
        let expected = format!("{base}/sqs/{region}/{}/{name}", account());
        let created = ok_on_host(
            &router,
            "untrusted.test.invalid:9297",
            region,
            "CreateQueue",
            json!({"QueueName": name}),
        )
        .await;
        assert_eq!(created["QueueUrl"], expected);
        let repeated = ok_in(&router, region, "CreateQueue", json!({"QueueName": name})).await;
        assert_eq!(repeated["QueueUrl"], expected);
        let resolved = ok_in(&router, region, "GetQueueUrl", json!({"QueueName": name})).await;
        assert_eq!(resolved["QueueUrl"], expected);
        let listed = ok_in(&router, region, "ListQueues", json!({})).await;
        assert_eq!(listed["QueueUrls"], json!([expected]));
        let attributes = ok_in(
            &router,
            region,
            "GetQueueAttributes",
            json!({
                "QueueUrl": expected, "AttributeNames": ["QueueArn"]
            }),
        )
        .await;
        assert_eq!(
            attributes["Attributes"]["QueueArn"],
            expected_arn_in(region, name)
        );
        urls.push(expected);
    }
    assert_ne!(urls[0], urls[1]);
}
