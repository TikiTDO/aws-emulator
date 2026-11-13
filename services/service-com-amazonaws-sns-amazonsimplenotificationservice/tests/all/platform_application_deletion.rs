// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeletePlatformApplication`, the call that removes a mobile-push
//! platform application object of the calling account.
//!
//! The rule under test: a delete REMOVES the platform application its
//! `PlatformApplicationArn` names from persisted state, together with the device
//! endpoints registered against that application, and answers success whether or not a
//! record was there — the action models no lookup miss, so a repeated delete, and a
//! delete naming an application this account holds no record of, both answer 200 and
//! leave every other record alone — while a value that is not an ARN of the request's own
//! region names no application this endpoint serves and is refused as bad input (HTTP 400
//! / `InvalidParameter`). The application read observes it: HTTP 404 / `NotFound` once the
//! record is gone. Subscriptions to a deleted application's endpoints stay where they
//! are, as they do when an endpoint is deleted on its own.
//!
//! The rows differ along what the rule generalises over: the push platform (every entry
//! of the documented catalog), the application name, how many device endpoints the
//! application holds (none, one, several), the region the application lives in, the kind
//! of resource the ARN names, the shape of the ARN presented, and how many times the same
//! delete arrives.
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls: the applications and
//! their endpoints are registered through `CreatePlatformApplication` and
//! `CreatePlatformEndpoint` and observed through `GetPlatformApplicationAttributes`,
//! `ListPlatformApplications`, `ListEndpointsByPlatformApplication` and
//! `GetEndpointAttributes`.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    platform_application_arn, ENDPOINT_DOES_NOT_EXIST, PLATFORM_APPLICATION_DOES_NOT_EXIST,
    PLATFORM_NAMES,
};

use super::create_platform_application::registered_application;
use super::subscription_attributes::subscribe;
use super::topics_subscriptions_publish::{Answer, Sns};

/// The region the harness signs its calls for unless a row names another one.
const REGION: &str = "us-east-1";

/// One row of the deletion table: an application of one push platform holding
/// `endpoints` device endpoints.
struct Row {
    case: &'static str,
    platform: &'static str,
    name: &'static str,
    endpoints: usize,
}

/// One application per entry of the documented platform catalog
/// (`CreatePlatformApplicationInput$Platform` and the per-platform credential clauses of
/// the `CreatePlatformApplication` doc, as [`PLATFORM_NAMES`] holds them), differing also
/// in the application name's form and the number of device endpoints registered against
/// it — none, one and several, so the cascade is driven on each.
fn rows() -> Vec<Row> {
    let endpoint_counts = [0usize, 1, 3, 2];
    let names = [
        "delete-bare",
        "delete_one.endpoint",
        "DeleteThree-9",
        "delete.two_devices",
    ];
    PLATFORM_NAMES
        .iter()
        .enumerate()
        .map(|(index, platform)| Row {
            case: platform,
            platform,
            name: names[index % names.len()],
            endpoints: endpoint_counts[index % endpoint_counts.len()],
        })
        .collect()
}

/// One `DeletePlatformApplication` call signed for `region`.
async fn delete_in_region(sns: &Sns, region: &str, arn: &str) -> Answer {
    sns.call_in_region(
        region,
        "DeletePlatformApplication",
        &[("PlatformApplicationArn", arn)],
    )
    .await
}

/// One `DeletePlatformApplication` call in the suite's own region.
async fn delete(sns: &Sns, arn: &str) -> Answer {
    delete_in_region(sns, REGION, arn).await
}

/// One delete asserted to have been SERVED (not refused).
async fn delete_served(sns: &Sns, case: &str, arn: &str) -> Answer {
    let answer = delete(sns, arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeletePlatformApplication to answer 200, got {} / {}",
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

/// One `GetPlatformApplicationAttributes` call: the read that observes what a delete did.
async fn read_application(sns: &Sns, arn: &str) -> Answer {
    sns.call(
        "GetPlatformApplicationAttributes",
        &[("PlatformApplicationArn", arn)],
    )
    .await
}

/// One `GetEndpointAttributes` call: the read that observes the cascade.
async fn read_endpoint(sns: &Sns, arn: &str) -> Answer {
    sns.call("GetEndpointAttributes", &[("EndpointArn", arn)])
        .await
}

/// Register `token` as a device endpoint of `application` and answer the endpoint ARN.
async fn register_endpoint(sns: &Sns, case: &str, application: &str, token: &str) -> String {
    let answer = sns
        .call(
            "CreatePlatformEndpoint",
            &[
                ("PlatformApplicationArn", application),
                ("Token", token),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: CreatePlatformEndpoint expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("EndpointArn")
        .unwrap_or_else(|| panic!("case {case:?}: no EndpointArn in {}", answer.body))
}

/// Every application ARN this account holds a record of, sorted.
fn held_applications(sns: &Sns) -> Vec<String> {
    let state = sns.state();
    let mut arns: Vec<String> = state.lock().platform_applications.keys().cloned().collect();
    arns.sort();
    arns
}

/// Every endpoint ARN this account holds a record of, sorted.
fn held_endpoints(sns: &Sns) -> Vec<String> {
    let state = sns.state();
    let mut arns: Vec<String> = state.lock().platform_endpoints.keys().cloned().collect();
    arns.sort();
    arns
}

/// The ARNs `ListPlatformApplications` reports, in document order.
async fn listed_applications(sns: &Sns) -> Vec<String> {
    let answer = sns.call("ListPlatformApplications", &[]).await;
    assert_eq!(
        answer.status, 200,
        "the application listing expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer.elements("PlatformApplicationArn")
}

/// The application read answers the lookup miss SNS states for an application the account
/// does not hold.
async fn assert_application_read_is_a_miss(sns: &Sns, case: &str, arn: &str) {
    let answer = read_application(sns, arn).await;
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected the application read to answer 404, got {} / {}",
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
        Some(PLATFORM_APPLICATION_DOES_NOT_EXIST),
        "case {case:?}: expected the fixed missing-application sentence, got {}",
        answer.body
    );
}

/// The endpoint read answers the lookup miss SNS states for an endpoint the account does
/// not hold.
async fn assert_endpoint_read_is_a_miss(sns: &Sns, case: &str, arn: &str) {
    let answer = read_endpoint(sns, arn).await;
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected the endpoint read to answer 404, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(ENDPOINT_DOES_NOT_EXIST),
        "case {case:?}: expected the fixed missing-endpoint sentence, got {}",
        answer.body
    );
}

/// A delete takes exactly the application it names — and the device endpoints registered
/// against it — out of persisted state, and the same delete arriving again answers the
/// same way and changes nothing more.
#[tokio::test]
async fn an_application_and_its_endpoints_are_deleted_and_the_delete_is_idempotent() {
    let sns = Sns::new();
    let rows = rows();

    // Every row registered up front, so each delete has siblings to leave alone.
    let mut applications: Vec<String> = Vec::new();
    let mut endpoints: Vec<Vec<String>> = Vec::new();
    for row in &rows {
        let application = registered_application(&sns, REGION, row.name, row.platform).await;
        let mut registered = Vec::new();
        for device in 0..row.endpoints {
            let token = format!("device-token-{}-{device}", row.platform);
            registered.push(register_endpoint(&sns, row.case, &application, &token).await);
        }
        applications.push(application);
        endpoints.push(registered);
    }

    let mut expected_applications = applications.clone();
    expected_applications.sort();
    assert_eq!(
        held_applications(&sns),
        expected_applications,
        "the registered applications must be the ones the store holds"
    );

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let application = applications[index].as_str();

        // What the application looks like from outside BEFORE the delete.
        let before = read_application(&sns, application).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected the application read to answer 200 before the delete, \
             got {} / {}",
            before.status, before.body
        );
        for endpoint in &endpoints[index] {
            let answer = read_endpoint(&sns, endpoint).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the endpoint {endpoint} to read back before the \
                 delete, got {} / {}",
                answer.status, answer.body
            );
        }

        // The same delete, twice: the second arrival is the idempotent one.
        for attempt in 1..=2 {
            delete_served(&sns, case, application).await;

            assert_application_read_is_a_miss(&sns, case, application).await;
            let listing = listed_applications(&sns).await;
            assert!(
                !listing.contains(&application.to_string()),
                "case {case:?} attempt {attempt}: expected the deleted application to be \
                 gone from the listing, got {listing:?}"
            );

            // The endpoints registered against it went with it: the listing scoped by
            // that application is a miss now, and each endpoint reads back as one.
            let endpoints_listing = sns
                .call(
                    "ListEndpointsByPlatformApplication",
                    &[("PlatformApplicationArn", application)],
                )
                .await;
            assert_eq!(
                endpoints_listing.status, 404,
                "case {case:?} attempt {attempt}: expected the endpoint listing of a \
                 deleted application to answer 404, got {} / {}",
                endpoints_listing.status, endpoints_listing.body
            );
            for endpoint in &endpoints[index] {
                assert_endpoint_read_is_a_miss(&sns, case, endpoint).await;
            }

            // Every other application, and every endpoint of it, is untouched.
            let mut surviving: Vec<String> = applications[index + 1..].to_vec();
            surviving.sort();
            assert_eq!(
                held_applications(&sns),
                surviving,
                "case {case:?} attempt {attempt}: expected only the applications after \
                 this one to survive, got {:?}",
                held_applications(&sns)
            );
            let mut surviving_endpoints: Vec<String> = endpoints[index + 1..]
                .iter()
                .flat_map(|held| held.iter().cloned())
                .collect();
            surviving_endpoints.sort();
            assert_eq!(
                held_endpoints(&sns),
                surviving_endpoints,
                "case {case:?} attempt {attempt}: expected only the endpoints of the \
                 surviving applications to be held, got {:?}",
                held_endpoints(&sns)
            );
            for endpoint in surviving_endpoints.iter() {
                let answer = read_endpoint(&sns, endpoint).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} attempt {attempt}: the delete must leave {endpoint} \
                     readable, got {} / {}",
                    answer.status, answer.body
                );
            }
        }
    }

    assert!(
        held_applications(&sns).is_empty(),
        "every application was deleted: expected an empty store, got {:?}",
        held_applications(&sns)
    );
    assert!(
        held_endpoints(&sns).is_empty(),
        "every application was deleted: expected no endpoint to survive, got {:?}",
        held_endpoints(&sns)
    );
}

/// A delete naming an application this account holds no record of in the request's own
/// region is a success that removes nothing — whatever the ARN names — and an application
/// of ANOTHER region is reached only by that region's own endpoint.
#[tokio::test]
async fn deleting_an_application_the_account_does_not_hold_changes_nothing() {
    let sns = Sns::new();
    let kept = registered_application(&sns, REGION, "kept-app", "GCM").await;
    let kept_endpoint = register_endpoint(&sns, "the held application", &kept, "kept-device").await;
    let topic = sns.topic("application-delete-topic").await;

    let mut cases: Vec<(String, String)> = PLATFORM_NAMES
        .iter()
        .map(|platform| {
            (
                format!("a {platform} application nobody registered"),
                platform_application_arn(REGION, platform, "never-created-app"),
            )
        })
        .collect();
    cases.push((
        "an ARN that names a topic rather than an application".to_string(),
        topic.clone(),
    ));
    cases.push((
        "an ARN that names a device endpoint of the held application".to_string(),
        kept_endpoint.clone(),
    ));
    cases.push((
        "an ARN of another service entirely".to_string(),
        "arn:aws:sqs:us-east-1:000000000000:application-delete-queue".to_string(),
    ));

    for (case, arn) in &cases {
        // The account holds no application under this ARN before the delete, and the
        // delete is still served.
        assert_application_read_is_a_miss(&sns, case, arn).await;
        delete_served(&sns, case, arn).await;
        assert_application_read_is_a_miss(&sns, case, arn).await;
        assert_eq!(
            held_applications(&sns),
            vec![kept.clone()],
            "case {case:?}: the held application must survive a delete that names another \
             resource, got {:?}",
            held_applications(&sns)
        );
        assert_eq!(
            held_endpoints(&sns),
            vec![kept_endpoint.clone()],
            "case {case:?}: the held application's endpoint must survive, got {:?}",
            held_endpoints(&sns)
        );
    }

    // An application of ANOTHER REGION: this region's endpoint refuses the ARN as bad
    // input rather than serving it against another region's state, and the delete signed
    // for that region is the one that removes it.
    let elsewhere_region = "eu-west-1";
    let elsewhere = registered_application(&sns, elsewhere_region, "elsewhere-app", "APNS").await;
    for operation in ["DeletePlatformApplication", "GetPlatformApplicationAttributes"] {
        let answer = sns
            .call(operation, &[("PlatformApplicationArn", &elsewhere)])
            .await;
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
            Some("Invalid parameter: PlatformApplicationArn"),
            "case another region on {operation}: expected the member's bare refusal \
             sentence, got {}",
            answer.body
        );
    }
    assert!(
        held_applications(&sns).contains(&elsewhere),
        "case another region: the refused delete must leave the application alone, got {:?}",
        held_applications(&sns)
    );
    let answer = delete_in_region(&sns, elsewhere_region, &elsewhere).await;
    assert_eq!(
        answer.status, 200,
        "case another region: that region's own delete must be served, got {} / {}",
        answer.status, answer.body
    );
    assert!(
        !held_applications(&sns).contains(&elsewhere),
        "case another region: that region's own delete must remove the application, got {:?}",
        held_applications(&sns)
    );

    // The application this account does hold in this region is still readable, so none of
    // the deletes above touched it.
    let survivor = read_application(&sns, &kept).await;
    assert_eq!(
        survivor.status, 200,
        "the held application must still be readable, got {} / {}",
        survivor.status, survivor.body
    );
    let still_there = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        still_there.status, 200,
        "the topic a misdirected delete named must survive, got {} / {}",
        still_there.status, still_there.body
    );
}

/// A value that is not an ARN at all is bad input, not an idempotent delete: it is
/// refused with the sentence SNS states for the constraint it broke, and nothing is
/// removed.
#[tokio::test]
async fn a_value_that_is_not_an_arn_is_refused() {
    let sns = Sns::new();
    let kept = registered_application(&sns, REGION, "refusal-app", "ADM").await;

    // The two clauses of the ARN-shape rule: an ARN has at least six colon-separated
    // elements, and it begins with `arn:`.
    let cases: &[(&str, &str, &str)] = &[
        (
            "not an ARN at all",
            "platform-application-1",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 1",
        ),
        (
            "an application resource with no ARN around it",
            "app/GCM/refusal-app",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 1",
        ),
        (
            "a URL rather than an ARN",
            "https://push.example.com/app/1",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 2",
        ),
        (
            "an ARN missing its resource field",
            "arn:aws:sns:us-east-1:000000000000",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 5",
        ),
        (
            "six elements that do not begin with 'arn:'",
            "aws:aws:sns:us-east-1:000000000000:app/GCM/refusal-app",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must begin with 'arn:'",
        ),
    ];

    for (case, value, message) in cases {
        // The same rule holds for both calls that take a `PlatformApplicationArn` of an
        // existing application: a value that is not an ARN is bad input, whichever one
        // carried it.
        for action in ["DeletePlatformApplication", "GetPlatformApplicationAttributes"] {
            let answer = sns.call(action, &[("PlatformApplicationArn", value)]).await;
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
        held_applications(&sns),
        vec![kept],
        "a refused delete must leave the store alone, got {:?}",
        held_applications(&sns)
    );
}

/// Deleting an application leaves every subscription to its endpoints in place: the
/// unsubscribe is a call the caller makes, not one this delete makes for them.
#[tokio::test]
async fn deleting_an_application_leaves_its_endpoint_subscriptions_in_place() {
    let sns = Sns::new();
    let case = "an application whose endpoint is subscribed";
    let application = registered_application(&sns, REGION, "subscribed-app", "APNS_SANDBOX").await;
    let endpoint = register_endpoint(&sns, case, &application, "subscribed-device").await;
    let topic = sns.topic("application-subscription-topic").await;
    let subscription = subscribe(&sns, case, &topic, "application", &endpoint, &[]).await;

    delete_served(&sns, case, &application).await;
    assert!(
        held_applications(&sns).is_empty(),
        "expected the application record to be gone, got {:?}",
        held_applications(&sns)
    );
    assert!(
        held_endpoints(&sns).is_empty(),
        "expected the application's endpoint record to be gone, got {:?}",
        held_endpoints(&sns)
    );
    assert_application_read_is_a_miss(&sns, case, &application).await;
    assert_endpoint_read_is_a_miss(&sns, case, &endpoint).await;

    let attributes = sns
        .call(
            "GetSubscriptionAttributes",
            &[("SubscriptionArn", &subscription)],
        )
        .await;
    assert_eq!(
        attributes.status, 200,
        "case {case:?}: the subscription must survive the application's delete, got {} / {}",
        attributes.status, attributes.body
    );
    assert!(
        sns.state().lock().subscriptions.contains_key(&subscription),
        "case {case:?}: the subscription record must still be persisted"
    );
}
