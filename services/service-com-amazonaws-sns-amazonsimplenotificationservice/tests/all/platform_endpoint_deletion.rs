// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteEndpoint` and the read that observes it,
//! `GetEndpointAttributes`: the device endpoints of a platform application.
//!
//! The rule under test: `DeleteEndpoint` removes the endpoint record its `EndpointArn`
//! names from persisted state and answers success whether or not a record was there —
//! the action is idempotent, so a repeated delete, and a delete naming an endpoint this
//! account holds no record of, both answer 200 and leave every other record alone —
//! while a value that is not an ARN at all names no endpoint anywhere and is refused as
//! bad input (HTTP 400 / `InvalidParameter`). The attribute read reports what the
//! record holds and answers HTTP 404 / `NotFound` for an endpoint the account does not
//! hold, which is how a delete is observed from outside. A deleted endpoint's
//! subscriptions stay where they are: unsubscribing them is a separate call the caller
//! makes.
//!
//! The rows differ along what the rule generalises over: the push platform, the
//! application name, the endpoint id, the attributes the record carries, the region and
//! account its ARN names, the kind of resource the ARN names, and how many times the
//! same delete arrives.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls. The endpoint
//! records themselves are seeded straight into that state — the way the SMS sandbox
//! suite seeds its store — so a row fixes the record's id, attributes, region and
//! account directly; the registration call that mints one is covered by
//! `platform_endpoint_registration_lifecycle`.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    platform_application_arn, platform_endpoint_arn, PlatformEndpointRecord, DEFAULT_REGION,
    ENDPOINT_ATTRIBUTE_NAMES, ENDPOINT_DOES_NOT_EXIST, ENDPOINT_ENABLED_ATTRIBUTE,
    ENDPOINT_TOKEN_ATTRIBUTE,
};

use super::subscription_attributes::subscribe;
use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The push platforms a platform application is created for, from the
/// `CreatePlatformApplicationInput$Platform` doc
/// (`smithy_query(query="doc",shape="CreatePlatformApplicationInput",member="Platform")`):
/// "ADM (Amazon Device Messaging), APNS (Apple Push Notification Service),
/// APNS_SANDBOX, and GCM (Firebase Cloud Messaging)".
const PLATFORMS: &[&str] = &["ADM", "APNS", "APNS_SANDBOX", "GCM"];

/// One seeded device endpoint: the record the delete has to act on.
struct Endpoint {
    case: &'static str,
    platform: &'static str,
    application: &'static str,
    id: &'static str,
    token: &'static str,
    attributes: &'static [(&'static str, &'static str)],
}

/// One endpoint per push platform, differing also in the application name, the
/// endpoint id, the device token's form and the attributes the record carries.
fn held_endpoints() -> Vec<Endpoint> {
    vec![
        Endpoint {
            case: "an Amazon Device Messaging endpoint with no attributes",
            platform: PLATFORMS[0],
            application: "store-app",
            id: "11111111-1111-1111-1111-111111111111",
            token: "amzn1.adm-registration.v3.short",
            attributes: &[],
        },
        Endpoint {
            case: "an Apple production endpoint with user data",
            platform: PLATFORMS[1],
            application: "retail_app-ios",
            id: "22222222-2222-2222-2222-222222222222",
            token: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
            attributes: &[("Enabled", "true"), ("CustomUserData", "cart-watcher")],
        },
        Endpoint {
            case: "an Apple sandbox endpoint that is disabled",
            platform: PLATFORMS[2],
            application: "retail_app-ios",
            id: "33333333-3333-3333-3333-333333333333",
            token: "0f0e0d0c0b0a09080706050403020100f0e0d0c0b0a0908070605040302010",
            attributes: &[("Enabled", "false")],
        },
        Endpoint {
            case: "a Firebase endpoint with a long token",
            platform: PLATFORMS[3],
            application: "one-more-app",
            id: "44444444-4444-4444-4444-444444444444",
            token: "cZ:APA91bF_longer-device-token_0123456789-abcdefghijklmnopqrstuvwxyz",
            attributes: &[("Enabled", "true"), ("CustomUserData", "")],
        },
    ]
}

/// Seed `endpoint` into this service's persisted state and answer its ARN.
///
/// `at` orders the records, so two seeded endpoints never share a creation instant.
fn seed(sns: &Sns, endpoint: &Endpoint, at: u64) -> String {
    let arn = platform_endpoint_arn(
        DEFAULT_REGION,
        endpoint.platform,
        endpoint.application,
        endpoint.id,
    );
    let record = PlatformEndpointRecord {
        arn: arn.clone(),
        platform_application_arn: platform_application_arn(
            DEFAULT_REGION,
            endpoint.platform,
            endpoint.application,
        ),
        token: endpoint.token.to_string(),
        attributes: endpoint
            .attributes
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
        created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(at + 1),
    };
    let state = sns.state();
    state
        .lock()
        .platform_endpoints
        .insert(arn.clone(), record);
    arn
}

/// Every endpoint ARN this account currently holds a record of.
fn held(sns: &Sns) -> Vec<String> {
    let state = sns.state();
    let mut arns: Vec<String> = state.lock().platform_endpoints.keys().cloned().collect();
    arns.sort();
    arns
}

/// The device token the record under `arn` carries, or `None` when no record is held.
fn token_of(sns: &Sns, arn: &str) -> Option<String> {
    let state = sns.state();
    // `guard` is declared after `state`, so it is dropped first.
    let guard = state.lock();
    guard
        .platform_endpoints
        .get(arn)
        .map(|record| record.token.clone())
}

/// One `DeleteEndpoint` call, asserted to have been SERVED (not refused).
async fn delete_served(sns: &Sns, case: &str, arn: &str) -> Answer {
    let answer = sns.call("DeleteEndpoint", &[("EndpointArn", arn)]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeleteEndpoint to answer 200, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "",
        "case {case:?}: expected a success envelope, got {}",
        answer.body
    );
    answer
}

/// One `GetEndpointAttributes` call: the read that observes what a delete did.
async fn read(sns: &Sns, arn: &str) -> Answer {
    sns.call("GetEndpointAttributes", &[("EndpointArn", arn)])
        .await
}

/// The attribute read answers the lookup miss SNS states for an endpoint the account
/// does not hold.
async fn assert_read_is_a_miss(sns: &Sns, case: &str, arn: &str) {
    let answer = read(sns, arn).await;
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected the read to answer 404, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "NotFound",
        "case {case:?}: expected the NotFound code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(ENDPOINT_DOES_NOT_EXIST),
        "case {case:?}: expected the fixed missing-endpoint sentence, got {}",
        answer.body
    );
}

/// A delete takes exactly the endpoint it names out of persisted state, and the same
/// delete arriving again answers the same way and changes nothing more.
#[tokio::test]
async fn a_held_endpoint_is_deleted_and_the_delete_is_idempotent() {
    let sns = Sns::new();
    let endpoints = held_endpoints();
    let arns: Vec<String> = endpoints
        .iter()
        .enumerate()
        .map(|(at, endpoint)| seed(&sns, endpoint, at as u64))
        .collect();
    let mut expected = arns.clone();
    expected.sort();
    assert_eq!(
        held(&sns),
        expected,
        "the seeded records must be the ones the store holds"
    );

    for (at, endpoint) in endpoints.iter().enumerate() {
        let case = endpoint.case;
        let arn = arns[at].as_str();
        assert_eq!(
            token_of(&sns, arn).as_deref(),
            Some(endpoint.token),
            "case {case:?}: the endpoint must be held before the delete"
        );

        // What the endpoint looks like from outside BEFORE the delete: the attributes
        // its record holds, every one of them a name SNS defines for an endpoint.
        let before = read(&sns, arn).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected the read to answer 200 before the delete, got {} / {}",
            before.status, before.body
        );
        let reported = attribute_map(&before);
        // A report changes nothing, so the same read answers the same thing again.
        let again = attribute_map(&read(&sns, arn).await);
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report the same attributes, got {again:?}"
        );
        assert_eq!(
            reported.get(ENDPOINT_TOKEN_ATTRIBUTE).map(String::as_str),
            Some(endpoint.token),
            "case {case:?}: expected the device token the record holds, got {reported:?}"
        );
        let expected_enabled = endpoint
            .attributes
            .iter()
            .find(|(name, _)| *name == ENDPOINT_ENABLED_ATTRIBUTE)
            .map_or("true", |(_, value)| value);
        assert_eq!(
            reported.get(ENDPOINT_ENABLED_ATTRIBUTE).map(String::as_str),
            Some(expected_enabled),
            "case {case:?}: expected the delivery flag the record holds, defaulting to \
             enabled, got {reported:?}"
        );
        for (name, value) in endpoint.attributes {
            assert_eq!(
                reported.get(*name).map(String::as_str),
                Some(*value),
                "case {case:?}: expected the attribute {name:?} the record holds, got \
                 {reported:?}"
            );
        }
        for name in reported.keys() {
            assert!(
                ENDPOINT_ATTRIBUTE_NAMES.contains(&name.as_str()),
                "case {case:?}: {name:?} is not an attribute SNS defines for an endpoint, \
                 got {reported:?}"
            );
        }

        // The same delete, twice: the second arrival is the idempotent one.
        for attempt in 1..=2 {
            delete_served(&sns, case, arn).await;
            assert!(
                token_of(&sns, arn).is_none(),
                "case {case:?} attempt {attempt}: expected the endpoint to be gone, got {:?}",
                token_of(&sns, arn)
            );
            assert_read_is_a_miss(&sns, case, arn).await;
            let surviving: Vec<String> = arns[at + 1..].to_vec();
            let mut sorted = surviving.clone();
            sorted.sort();
            assert_eq!(
                held(&sns),
                sorted,
                "case {case:?} attempt {attempt}: expected only the endpoints after this \
                 one to survive, got {:?}",
                held(&sns)
            );
            for (other_at, other) in endpoints.iter().enumerate().skip(at + 1) {
                assert_eq!(
                    token_of(&sns, arns[other_at].as_str()).as_deref(),
                    Some(other.token),
                    "case {case:?} attempt {attempt}: the delete must leave {:?} untouched",
                    other.case
                );
            }
        }
    }

    assert!(
        held(&sns).is_empty(),
        "every endpoint was deleted: expected an empty store, got {:?}",
        held(&sns)
    );
}

/// A delete naming an endpoint this account holds no record of is a success that
/// removes nothing — whatever the ARN names.
#[tokio::test]
async fn deleting_an_endpoint_the_account_does_not_hold_changes_nothing() {
    let sns = Sns::new();
    let endpoints = held_endpoints();
    let kept = seed(&sns, &endpoints[0], 0);
    let topic = sns.topic("endpoint-delete-topic").await;

    let mut cases: Vec<(String, String)> = PLATFORMS
        .iter()
        .map(|platform| {
            (
                format!("an endpoint of a {platform} application nobody registered"),
                platform_endpoint_arn(
                    DEFAULT_REGION,
                    platform,
                    "never-created-app",
                    "55555555-5555-5555-5555-555555555555",
                ),
            )
        })
        .collect();
    cases.push((
        "an ARN that names a topic rather than an endpoint".to_string(),
        topic.clone(),
    ));
    cases.push((
        "an ARN that names the platform application itself".to_string(),
        platform_application_arn(
            DEFAULT_REGION,
            endpoints[0].platform,
            endpoints[0].application,
        ),
    ));
    cases.push((
        "an ARN of another service entirely".to_string(),
        "arn:aws:sqs:us-east-1:000000000000:endpoint-delete-queue".to_string(),
    ));

    for (case, arn) in &cases {
        // The account holds no endpoint under this ARN before the delete, and the
        // delete is still served.
        assert_read_is_a_miss(&sns, case, arn).await;
        delete_served(&sns, case, arn).await;
        assert_read_is_a_miss(&sns, case, arn).await;
        assert_eq!(
            held(&sns),
            vec![kept.clone()],
            "case {case:?}: the held endpoint must survive a delete that names another \
             resource, got {:?}",
            held(&sns)
        );
    }

    // An `EndpointArn` of ANOTHER REGION is not an endpoint this endpoint of the service
    // can reach at all: both the delete and the read are refused as bad input rather than
    // served against this region's state, and the held endpoint is untouched.
    let elsewhere = platform_endpoint_arn(
        "eu-west-1",
        endpoints[0].platform,
        endpoints[0].application,
        endpoints[0].id,
    );
    for operation in ["DeleteEndpoint", "GetEndpointAttributes"] {
        let answer = sns.call(operation, &[("EndpointArn", &elsewhere)]).await;
        assert_eq!(
            answer.status, 400,
            "case another region on {operation}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case another region on {operation}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some("Invalid parameter: EndpointArn"),
            "case another region on {operation}: expected the member's bare refusal \
             sentence, got {}",
            answer.body
        );
    }
    assert_eq!(
        held(&sns),
        vec![kept.clone()],
        "case another region: the held endpoint must survive a refused delete, got {:?}",
        held(&sns)
    );

    // The endpoint this account does hold is still readable, so none of the deletes
    // above touched it.
    let survivor = read(&sns, &kept).await;
    assert_eq!(
        survivor.status, 200,
        "the held endpoint must still be readable, got {} / {}",
        survivor.status, survivor.body
    );
    assert_eq!(
        attribute_map(&survivor)
            .get(ENDPOINT_TOKEN_ATTRIBUTE)
            .map(String::as_str),
        Some(endpoints[0].token),
        "the held endpoint must still carry its device token, got {}",
        survivor.body
    );

    // The resource the misdirected deletes named is still there too.
    let still_there = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        still_there.status, 200,
        "the topic a misdirected DeleteEndpoint named must survive, got {} / {}",
        still_there.status, still_there.body
    );
}

/// A value that is not an ARN at all is bad input, not an idempotent delete: it is
/// refused with the sentence SNS states for the constraint it broke, and nothing is
/// removed.
#[tokio::test]
async fn a_value_that_is_not_an_arn_is_refused() {
    let sns = Sns::new();
    let endpoints = held_endpoints();
    let kept = seed(&sns, &endpoints[0], 0);

    // The two clauses of the ARN-shape rule: an ARN has at least six colon-separated
    // elements, and it begins with `arn:`.
    let cases: &[(&str, &str, &str)] = &[
        (
            "not an ARN at all",
            "device-endpoint-1",
            "Invalid parameter: EndpointArn Reason: An ARN must have at least 6 elements, not 1",
        ),
        (
            "an endpoint resource with no ARN around it",
            "endpoint/GCM/store-app/11111111-1111-1111-1111-111111111111",
            "Invalid parameter: EndpointArn Reason: An ARN must have at least 6 elements, not 1",
        ),
        (
            "a URL rather than an ARN",
            "https://push.example.com/device/1",
            "Invalid parameter: EndpointArn Reason: An ARN must have at least 6 elements, not 2",
        ),
        (
            "an ARN missing its resource field",
            "arn:aws:sns:us-east-1:000000000000",
            "Invalid parameter: EndpointArn Reason: An ARN must have at least 6 elements, not 5",
        ),
        (
            "six elements that do not begin with 'arn:'",
            "aws:aws:sns:us-east-1:000000000000:endpoint/GCM/store-app/1",
            "Invalid parameter: EndpointArn Reason: An ARN must begin with 'arn:'",
        ),
    ];

    for (case, value, message) in cases {
        // The same rule holds for both calls that take an `EndpointArn`: a value that
        // is not an ARN is bad input, whichever one carried it.
        for action in ["DeleteEndpoint", "GetEndpointAttributes"] {
            let answer = sns.call(action, &[("EndpointArn", value)]).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {action}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?} on {action}: expected the InvalidParameter code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(*message),
                "case {case:?} on {action}: expected the sentence for the constraint it \
                 broke, got {}",
                answer.body
            );
            assert!(
                !answer.body.contains(value),
                "case {case:?} on {action}: the refusal must name the member and the \
                 constraint, not echo the value, got {}",
                answer.body
            );
        }
    }

    assert_eq!(
        held(&sns),
        vec![kept],
        "a refused delete must leave the store alone, got {:?}",
        held(&sns)
    );
}

/// Deleting an endpoint leaves every subscription to it in place: "you must also
/// unsubscribe the endpoint from the topic" (`DeleteEndpoint` doc), which is a call the
/// caller makes, not one this delete makes for them.
#[tokio::test]
async fn deleting_an_endpoint_leaves_its_subscriptions_in_place() {
    let sns = Sns::new();
    let endpoints = held_endpoints();
    let arn = seed(&sns, &endpoints[1], 0);
    let topic = sns.topic("endpoint-subscription-topic").await;
    let subscription = subscribe(
        &sns,
        endpoints[1].case,
        &topic,
        "application",
        &arn,
        &[],
    )
    .await;

    delete_served(&sns, endpoints[1].case, &arn).await;
    assert!(
        held(&sns).is_empty(),
        "expected the endpoint record to be gone, got {:?}",
        held(&sns)
    );
    assert_read_is_a_miss(&sns, endpoints[1].case, &arn).await;

    let attributes = sns
        .call(
            "GetSubscriptionAttributes",
            &[("SubscriptionArn", &subscription)],
        )
        .await;
    assert_eq!(
        attributes.status, 200,
        "the subscription must survive the endpoint's delete, got {} / {}",
        attributes.status, attributes.body
    );
    let reported = attribute_map(&attributes);
    assert_eq!(
        reported.get("Endpoint").map(String::as_str),
        Some(arn.as_str()),
        "the surviving subscription must still address the deleted endpoint, got {reported:?}"
    );
    assert!(
        sns.state().lock().subscriptions.contains_key(&subscription),
        "the subscription record must still be persisted"
    );
}
