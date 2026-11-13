// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `SetEndpointAttributes`, the call that writes the attributes of a
//! device endpoint of a mobile-push platform application.
//!
//! The rule under test: a write carrying an attribute MAP applies the entries that map
//! holds over the ones the addressed endpoint record already holds and leaves every
//! attribute the request does not name as it was, the delivery flag included — "Users can
//! set it back to true, typically after updating Token" — with a written `Token` moving the
//! record's own device-token field, the one every report of an endpoint takes that value
//! from; the record then reads back through `GetEndpointAttributes` and through the
//! application's endpoint listing. The write is refused with the operation's modelled
//! client error (HTTP 400 / `InvalidParameter`) when the ARN is not an ARN of the region
//! this endpoint serves, when the user data crosses its published size, or when the map
//! carries a name SNS defines for no endpoint of that application's platform, and with the
//! modelled lookup miss (HTTP 404 / `NotFound`, "Endpoint does not exist") when the
//! account holds no endpoint record under that ARN — and a refused write persists nothing.
//!
//! The rows differ along what the rule generalises over: the push platform (every entry of
//! the documented catalog), the attribute written (every entry of the defined catalog, the
//! two a Baidu endpoint carries, names outside it, one entry and several at once), the
//! number of writes addressed at one record, the device token's form, the user data's size
//! (both sides of the published bound), which endpoint an ARN names, and the form and
//! region of the ARN presented.
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls.

use std::collections::HashMap;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    attribute_flag, custom_user_data_refusal, mobile_push_unknown_attribute_refusal,
    platform_endpoint_arn, ATTRIBUTES_MEMBER, CHANNEL_ENDPOINT_ATTRIBUTE_NAMES,
    ENDPOINT_ATTRIBUTE_NAMES, ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, ENDPOINT_DOES_NOT_EXIST,
    ENDPOINT_ENABLED_ATTRIBUTE, ENDPOINT_TOKEN_ATTRIBUTE, MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES,
    PLATFORM_NAMES, PLATFORM_WITH_CHANNEL_ENDPOINTS,
};

use super::create_platform_application::{attribute_params, registered_application};
use super::platform_applications::entries_of;
use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The region the harness signs its calls for unless a row names another one.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, as the sibling suites spell it.
const ACCOUNT: &str = "000000000000";

/// The attribute names SNS defines for a device endpoint, spelled out here rather than
/// read from the handler's own catalog, so a set that grew by mistake fails this suite.
///
/// From the `SetEndpointAttributesInput$Attributes` doc
/// (`smithy_query(query="doc",shape="SetEndpointAttributesInput",member="Attributes")`):
/// "CustomUserData – arbitrary user data to associate with the endpoint … Enabled – flag
/// that enables/disables delivery to the endpoint … Token – device token, also referred to
/// as a registration id".
const DOCUMENTED_ENDPOINT_ATTRIBUTE_NAMES: &[&str] = &["CustomUserData", "Enabled", "Token"];

/// The device token a row registers its endpoint under, one form per push platform.
fn token_for(platform: &str) -> String {
    match platform {
        "ADM" => "amzn1.adm-registration.v3.shortish-token".to_string(),
        "APNS" => "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90".to_string(),
        "APNS_SANDBOX" => "0f0e0d0c0b0a09080706050403020100f".to_string(),
        "GCM" => "cZ:APA91bF_device-token_0123456789-abcdefghijklmnopqrstuvwxyz".to_string(),
        "MPNS" => "http://sn1.notify.live.net/throttledthirdparty/01.00/AQE".to_string(),
        "WNS" => "https://db5.notify.windows.com/?token=AgYAAAD".to_string(),
        channel => format!("channel-{channel}-9876543210"),
    }
}

/// The attribute names an endpoint of `platform` takes: the documented catalog, and for the
/// one push service whose endpoints carry a channel and a user of their own the two names
/// its `CreatePlatformEndpoint` doc adds ("When using CreatePlatformEndpoint with Baidu,
/// two attributes must be provided: ChannelId and UserId").
fn catalog(platform: &str) -> Vec<String> {
    let mut names: Vec<String> = DOCUMENTED_ENDPOINT_ATTRIBUTE_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    if platform == PLATFORM_WITH_CHANNEL_ENDPOINTS {
        names.extend(
            CHANNEL_ENDPOINT_ATTRIBUTE_NAMES
                .iter()
                .map(|name| (*name).to_string()),
        );
    }
    names
}

/// A value of the kind the attribute `name` of an endpoint takes.
fn value_for(name: &str) -> String {
    match name {
        ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE => "{\"tier\":\"gold\"}".to_string(),
        ENDPOINT_ENABLED_ATTRIBUTE => attribute_flag(false),
        ENDPOINT_TOKEN_ATTRIBUTE => "re-registered-device-token-0001".to_string(),
        "ChannelId" => "1234567890".to_string(),
        other => format!("{other}-value"),
    }
}

/// One `SetEndpointAttributes` call signed for `region`.
async fn write_in_region(
    sns: &Sns,
    region: &str,
    endpoint_arn: &str,
    attributes: &[(String, String)],
) -> Answer {
    let mut params: Vec<(String, String)> =
        vec![("EndpointArn".to_string(), endpoint_arn.to_string())];
    params.extend(attribute_params(attributes));
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call_in_region(region, "SetEndpointAttributes", &borrowed)
        .await
}

/// One `SetEndpointAttributes` call in the suite's own region.
async fn write(sns: &Sns, endpoint_arn: &str, attributes: &[(String, String)]) -> Answer {
    write_in_region(sns, REGION, endpoint_arn, attributes).await
}

/// One write asserted to have been SERVED.
async fn written(sns: &Sns, case: &str, endpoint_arn: &str, attributes: &[(String, String)]) {
    let answer = write(sns, endpoint_arn, attributes).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// The attributes `GetEndpointAttributes` reports for `arn`.
async fn reported(sns: &Sns, case: &str, arn: &str) -> HashMap<String, String> {
    let answer = sns
        .call("GetEndpointAttributes", &[("EndpointArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the attribute read expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// The attributes `ListEndpointsByPlatformApplication` reports for the ONE endpoint of
/// `application_arn` — the other read of the endpoint plane, which the same record feeds.
async fn listed(sns: &Sns, case: &str, application_arn: &str) -> HashMap<String, String> {
    let answer = sns
        .call(
            "ListEndpointsByPlatformApplication",
            &[("PlatformApplicationArn", application_arn)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the listing expected 200, got {} / {}",
        answer.status, answer.body
    );
    entries_of(&answer.body)
}

/// A device endpoint registered through the wire call that mints one, with its ARN.
async fn registered_endpoint(sns: &Sns, application_arn: &str, token: &str) -> String {
    let answer = sns
        .call(
            "CreatePlatformEndpoint",
            &[
                ("PlatformApplicationArn", application_arn),
                ("Token", token),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "CreatePlatformEndpoint: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("EndpointArn")
        .unwrap_or_else(|| panic!("no EndpointArn in {}", answer.body))
}

/// An application of `platform` and one endpoint of it, with both ARNs.
async fn given_endpoint(sns: &Sns, platform: &str, name: &str) -> (String, String) {
    let application = registered_application(sns, REGION, name, platform).await;
    let endpoint = registered_endpoint(sns, &application, &token_for(platform)).await;
    (application, endpoint)
}

/// What the endpoint record `arn` names holds: its attribute map and its device token.
fn held(sns: &Sns, arn: &str) -> (HashMap<String, String>, String) {
    let store = sns.state();
    let store = store.lock();
    let record = store
        .platform_endpoints
        .get(arn)
        .unwrap_or_else(|| panic!("no endpoint record under {arn}"));
    (record.attributes.clone(), record.token.clone())
}

/// The attributes a report of an endpoint holding `attributes` under `token` carries: the
/// device token off the record's own field, and the delivery flag an endpoint nobody has
/// disabled has.
fn expected_report(attributes: &HashMap<String, String>, token: &str) -> HashMap<String, String> {
    let mut expected = attributes.clone();
    expected.insert(ENDPOINT_TOKEN_ATTRIBUTE.to_string(), token.to_string());
    expected
        .entry(ENDPOINT_ENABLED_ATTRIBUTE.to_string())
        .or_insert_with(|| attribute_flag(true));
    expected
}

/// Owned attribute entries from a row's borrowed pairs.
fn pairs(attributes: &[(&str, &str)]) -> Vec<(String, String)> {
    attributes
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// Every defined attribute is written on an endpoint of every platform of the documented
/// catalog, one name at a time, and each write leaves the attributes it does not name
/// alone: the record reads back as the map the writes so far produced, through both reads
/// of the endpoint plane.
#[tokio::test]
async fn each_write_applies_its_entries_over_the_attributes_the_record_already_holds() {
    let sns = Sns::new();
    assert_eq!(
        ENDPOINT_ATTRIBUTE_NAMES, DOCUMENTED_ENDPOINT_ATTRIBUTE_NAMES,
        "the endpoint attribute catalog must be the documented one, got {ENDPOINT_ATTRIBUTE_NAMES:?}"
    );

    for platform in PLATFORM_NAMES {
        let (application, endpoint) =
            given_endpoint(&sns, platform, &format!("set-{platform}")).await;

        // What the registered endpoint starts from: no attributes of its own, so the
        // report carries the device token and the flag of an endpoint nobody disabled.
        let mut expected: HashMap<String, String> = HashMap::new();
        let mut token = token_for(platform);
        let case = format!("{platform} / before any write");
        let report = reported(&sns, &case, &endpoint).await;
        let want = expected_report(&expected, &token);
        assert_eq!(
            report, want,
            "case {case:?}: expected the registration's own report {want:?}, got {report:?}"
        );

        for name in catalog(platform) {
            let case = format!("{platform} / {name}");
            let value = value_for(&name);
            written(&sns, &case, &endpoint, &[(name.clone(), value.clone())]).await;
            if name == ENDPOINT_TOKEN_ATTRIBUTE {
                token = value;
            } else {
                expected.insert(name.clone(), value);
            }

            let report = reported(&sns, &case, &endpoint).await;
            let want = expected_report(&expected, &token);
            assert_eq!(
                report, want,
                "case {case:?}: expected the report {want:?}, got {report:?}"
            );
            let (attributes, record_token) = held(&sns, &endpoint);
            assert_eq!(
                attributes, expected,
                "case {case:?}: expected the record to hold {expected:?}, got {attributes:?}"
            );
            assert_eq!(
                record_token, token,
                "case {case:?}: expected the record's token field to be {token:?}, got \
                 {record_token:?}"
            );
            let listing = listed(&sns, &case, &application).await;
            assert_eq!(
                listing, want,
                "case {case:?}: expected the listing to report {want:?}, got {listing:?}"
            );
        }

        // Several entries in ONE map, and the same map a second time: the write is
        // idempotent, and still leaves the names it does not carry alone.
        let several = pairs(&[
            (ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, "cart-watcher"),
            (ENDPOINT_ENABLED_ATTRIBUTE, "true"),
        ]);
        for round in 1..=2 {
            let case = format!("{platform} / several entries at once, round {round}");
            written(&sns, &case, &endpoint, &several).await;
            expected.extend(several.iter().cloned());
            let report = reported(&sns, &case, &endpoint).await;
            let want = expected_report(&expected, &token);
            assert_eq!(
                report, want,
                "case {case:?}: expected the report {want:?}, got {report:?}"
            );
        }

        // A map naming an attribute under the value the record already holds changes
        // nothing: the write is a map of values, not a sequence of operations.
        let case = format!("{platform} / the value the record already holds");
        written(
            &sns,
            &case,
            &endpoint,
            &pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]),
        )
        .await;
        let (attributes, record_token) = held(&sns, &endpoint);
        assert_eq!(
            attributes, expected,
            "case {case:?}: expected the record to hold {expected:?}, got {attributes:?}"
        );
        assert_eq!(
            record_token, token,
            "case {case:?}: expected the record's token to stay {token:?}, got {record_token:?}"
        );
    }

    assert_eq!(
        sns.state().lock().platform_endpoints.len(),
        PLATFORM_NAMES.len(),
        "a write must not register or drop an endpoint"
    );
}

/// A written device token moves the record's OWN token field — the field every report of
/// an endpoint takes that value from — rather than being stored beside it, so the token
/// has one source whichever call wrote it, and the delivery flag the caller sets back to
/// true survives the same write.
#[tokio::test]
async fn a_written_token_moves_the_records_own_device_token_field() {
    let sns = Sns::new();
    let (application, endpoint) = given_endpoint(&sns, "GCM", "token-rotation").await;

    // The sequence the doc describes: the service disables an invalid endpoint, the owner
    // updates the token and sets the flag back to true. The rows differ in the token's
    // form and length.
    let rows: &[(&str, &str)] = &[
        ("a short replacement token", "short-token-1"),
        (
            "a long replacement token",
            "cZ:APA91bE_rotated-device-token_0123456789-abcdefghijklmnopqrstuvwxyz-0123456789",
        ),
        (
            "a token carrying characters a form must encode",
            "https://db5.notify.windows.com/?token=AgYAAAD+/=",
        ),
        ("the token it already holds", "short-token-1"),
    ];

    written(
        &sns,
        "the service disables the endpoint",
        &endpoint,
        &pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "false")]),
    )
    .await;

    for (case, token) in rows {
        written(
            &sns,
            case,
            &endpoint,
            &pairs(&[
                (ENDPOINT_TOKEN_ATTRIBUTE, token),
                (ENDPOINT_ENABLED_ATTRIBUTE, "true"),
            ]),
        )
        .await;

        let (attributes, record_token) = held(&sns, &endpoint);
        assert_eq!(
            record_token, *token,
            "case {case:?}: expected the record's token field to be {token:?}, got \
             {record_token:?}"
        );
        assert!(
            !attributes.contains_key(ENDPOINT_TOKEN_ATTRIBUTE),
            "case {case:?}: the token must not be stored beside its own field, got \
             {attributes:?}"
        );
        let report = reported(&sns, case, &endpoint).await;
        assert_eq!(
            report.get(ENDPOINT_TOKEN_ATTRIBUTE).map(String::as_str),
            Some(*token),
            "case {case:?}: expected the report to carry the written token, got {report:?}"
        );
        assert_eq!(
            report.get(ENDPOINT_ENABLED_ATTRIBUTE).map(String::as_str),
            Some(attribute_flag(true).as_str()),
            "case {case:?}: expected the flag the caller set back to true, got {report:?}"
        );
        let listing = listed(&sns, case, &application).await;
        assert_eq!(
            listing.get(ENDPOINT_TOKEN_ATTRIBUTE).map(String::as_str),
            Some(*token),
            "case {case:?}: expected the listing to carry the written token, got {listing:?}"
        );
    }
}

/// A write acts on the ONE endpoint its ARN names: another endpoint of the same
/// application, and one of another application, keep every attribute they held.
#[tokio::test]
async fn a_write_touches_only_the_endpoint_its_arn_names() {
    let sns = Sns::new();
    let (application, one) = given_endpoint(&sns, "APNS", "isolated-app").await;
    let sibling = registered_endpoint(&sns, &application, "sibling-device-token").await;
    let (_, elsewhere) = given_endpoint(&sns, "ADM", "other-app").await;
    let untouched_sibling = held(&sns, &sibling);
    let untouched_elsewhere = held(&sns, &elsewhere);

    written(
        &sns,
        "a write addressed at one of three endpoints",
        &one,
        &pairs(&[(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, "only-this-one")]),
    )
    .await;

    let report = reported(&sns, "the endpoint that was written", &one).await;
    assert_eq!(
        report
            .get(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE)
            .map(String::as_str),
        Some("only-this-one"),
        "expected the written attribute to read back, got {report:?}"
    );
    assert_eq!(
        held(&sns, &sibling),
        untouched_sibling,
        "expected the sibling endpoint of the same application to be left alone"
    );
    assert_eq!(
        held(&sns, &elsewhere),
        untouched_elsewhere,
        "expected the endpoint of another application to be left alone"
    );
}

/// An attribute name SNS defines for no endpoint of that application's platform is refused
/// with the member the map arrived in and the key that was not recognised, and the write
/// persists nothing — not even the entries of the same map that ARE defined.
#[tokio::test]
async fn an_attribute_name_the_service_does_not_define_is_refused() {
    let sns = Sns::new();
    let (_, endpoint) = given_endpoint(&sns, "GCM", "closed-set").await;
    let before = held(&sns, &endpoint);

    // Rows: a name of another resource's set, a misspelling, a defined name in another
    // case, a defined name carrying whitespace, a name only the endpoints of another push
    // service carry, two unrecognised names at once (the first by name is the one
    // reported, so one request always names the same key), and a defined name beside an
    // unrecognised one.
    type Row = (&'static str, Vec<(String, String)>, &'static str);
    let rows: Vec<Row> = vec![
        (
            "a name defined for a platform application, not an endpoint",
            pairs(&[("PlatformCredential", "AIzaSyB-secret")]),
            "PlatformCredential",
        ),
        (
            "a name defined for a topic, not an endpoint",
            pairs(&[("DisplayName", "push")]),
            "DisplayName",
        ),
        (
            "a misspelled defined name",
            pairs(&[("CustomUserdata", "cart-watcher")]),
            "CustomUserdata",
        ),
        (
            "a defined name in another case",
            pairs(&[("enabled", "false")]),
            "enabled",
        ),
        (
            "a defined name carrying trailing whitespace",
            pairs(&[("Token ", "device-token")]),
            "Token ",
        ),
        (
            "a name only another push service's endpoints carry",
            pairs(&[
                (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[0], "1234567890"),
                (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[1], "user-42"),
            ]),
            CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[0],
        ),
        (
            "two unrecognised names at once",
            pairs(&[("ZetaAttribute", "1"), ("AlphaAttribute", "2")]),
            "AlphaAttribute",
        ),
        (
            "a defined name beside an unrecognised one",
            pairs(&[
                (ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, "cart-watcher"),
                ("NotAnAttribute", "x"),
            ]),
            "NotAnAttribute",
        ),
    ];

    for (case, attributes, unknown) in &rows {
        let answer = write(&sns, &endpoint, attributes).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(mobile_push_unknown_attribute_refusal(ATTRIBUTES_MEMBER, unknown).as_str()),
            "case {case:?}: expected the member-and-key refusal sentence, got {}",
            answer.body
        );
        let record = held(&sns, &endpoint);
        assert_eq!(
            record, before,
            "case {case:?}: a refused write must persist nothing, got {record:?}"
        );
    }

    // The two names an endpoint of the push service whose devices carry a channel and a
    // user of their own DOES take: the same keys, written on an endpoint of that service.
    let (_, channelled) =
        given_endpoint(&sns, PLATFORM_WITH_CHANNEL_ENDPOINTS, "channelled").await;
    let entries = pairs(&[
        (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[0], "1234567890"),
        (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[1], "user-42"),
    ]);
    written(
        &sns,
        "the channel and user of a device of the service that carries them",
        &channelled,
        &entries,
    )
    .await;
    let report = reported(&sns, "the channelled endpoint", &channelled).await;
    for (name, value) in &entries {
        assert_eq!(
            report.get(name).map(String::as_str),
            Some(value.as_str()),
            "case {name:?}: expected the written value to read back, got {report:?}"
        );
    }
}

/// User data past the size its own doc publishes is refused and never stored; user data at
/// the bound is stored and reads back.
#[tokio::test]
async fn user_data_past_its_published_size_is_refused() {
    let sns = Sns::new();
    let (_, endpoint) = given_endpoint(&sns, "WNS", "user-data").await;

    // The published bound and one byte past it: "The data must be in UTF-8 format and less
    // than 2KB" (`SetEndpointAttributesInput$Attributes` doc), so the most an endpoint
    // holds is one byte under 2 KiB.
    let at_bound = "d".repeat(MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES);
    let past_bound = "d".repeat(MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES + 1);

    written(
        &sns,
        "user data at the published bound",
        &endpoint,
        &pairs(&[(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, &at_bound)]),
    )
    .await;
    let report = reported(&sns, "user data at the published bound", &endpoint).await;
    assert_eq!(
        report
            .get(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE)
            .map(String::as_str),
        Some(at_bound.as_str()),
        "expected the user data at the bound to read back, got {} bytes",
        report
            .get(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE)
            .map_or(0, String::len)
    );
    let before = held(&sns, &endpoint);

    // The size refusal, pinned as literal text: the member, the published ceiling and the
    // size the request offered.
    let message = "Invalid parameter: CustomUserData - at most 2047 bytes: 2048";
    assert_eq!(
        custom_user_data_refusal(&past_bound).as_deref(),
        Some(message),
        "the size refusal must state the member, the ceiling and the size offered"
    );
    let answer = write(
        &sns,
        &endpoint,
        &pairs(&[(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, &past_bound)]),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"user data one byte past the bound\": expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "InvalidParameter",
        "case \"user data one byte past the bound\": expected the InvalidParameter code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(message),
        "case \"user data one byte past the bound\": expected the size refusal sentence, got {}",
        answer.body
    );
    let record = held(&sns, &endpoint);
    assert_eq!(
        record, before,
        "a refused write must persist nothing, got {} bytes of user data",
        record
            .0
            .get(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE)
            .map_or(0, String::len)
    );

    // The size rule is a fault in the ARGUMENT, so it is judged before the endpoint is
    // looked up: the same oversized data against an ARN naming no endpoint is still the
    // size refusal, where a map the rule accepts against that ARN is the lookup miss.
    let unheld = platform_endpoint_arn(
        REGION,
        "WNS",
        "user-data",
        "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee",
    );
    let answer = write(
        &sns,
        &unheld,
        &pairs(&[(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, &past_bound)]),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"user data past the bound against an ARN naming no endpoint\": expected 400, \
         got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(message),
        "case \"user data past the bound against an ARN naming no endpoint\": expected the \
         size refusal sentence, got {}",
        answer.body
    );
    let answer = write(
        &sns,
        &unheld,
        &pairs(&[(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, &at_bound)]),
    )
    .await;
    assert_eq!(
        answer.status, 404,
        "case \"acceptable user data against an ARN naming no endpoint\": expected 404, got \
         {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(ENDPOINT_DOES_NOT_EXIST),
        "case \"acceptable user data against an ARN naming no endpoint\": expected the fixed \
         missing-endpoint sentence, got {}",
        answer.body
    );
}

/// An ARN that is not an ARN of the region this endpoint serves is bad input; an ARN-shaped
/// value the account holds no endpoint record for is the modelled lookup miss. Neither
/// writes anything.
#[tokio::test]
async fn a_bad_arn_is_bad_input_and_an_unheld_one_is_the_lookup_miss() {
    let sns = Sns::new();
    let (application, endpoint) = given_endpoint(&sns, "GCM", "resolved").await;
    let topic = sns.topic("set-endpoint-attributes-topic").await;
    let before = held(&sns, &endpoint);
    let entry = pairs(&[(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, "unwritten")]);

    // Not an ARN of this region: judged on the value itself, before any record is looked
    // up. The reason names the member and the constraint the value failed.
    let bad: &[(&str, &str, &str)] = &[
        (
            "not an ARN at all",
            "resolved",
            "Invalid parameter: EndpointArn Reason: An ARN must have at least 6 elements, not 1",
        ),
        (
            "too few ARN elements",
            "arn:aws:sns:us-east-1:000000000000",
            "Invalid parameter: EndpointArn Reason: An ARN must have at least 6 elements, not 5",
        ),
        (
            "six elements that do not begin with the ARN prefix",
            "urn:aws:sns:us-east-1:000000000000:endpoint/GCM/resolved/1111",
            "Invalid parameter: EndpointArn Reason: An ARN must begin with 'arn:'",
        ),
    ];
    for (case, value, message) in bad {
        let answer = write(&sns, value, &entry).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(*message),
            "case {case:?}: expected the member's refusal sentence, got {}",
            answer.body
        );
    }

    // The endpoint's own ARN, presented to the endpoint of ANOTHER region: an endpoint of
    // one region names nothing the other serves, so the value is refused rather than
    // reported missing.
    let answer = write_in_region(&sns, "eu-west-1", &endpoint, &entry).await;
    assert_eq!(
        answer.status, 400,
        "case \"an endpoint of another region\": expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some("Invalid parameter: EndpointArn"),
        "case \"an endpoint of another region\": expected the member's bare refusal sentence, \
         got {}",
        answer.body
    );

    // Well-formed ARNs of this region that name no endpoint record of this account: an id
    // nobody minted under every platform of the catalog, the held endpoint under another
    // account, an endpoint of an application nobody registered, ARNs of other resources,
    // and the held endpoint once it has been deleted.
    let mut missing: Vec<(String, String)> = PLATFORM_NAMES
        .iter()
        .map(|platform| {
            (
                format!("a well-formed {platform} endpoint ARN nobody registered"),
                platform_endpoint_arn(
                    REGION,
                    platform,
                    "resolved",
                    "ffffffff-ffff-ffff-ffff-ffffffffffff",
                ),
            )
        })
        .collect();
    missing.push((
        "an endpoint of an application nobody registered".to_string(),
        platform_endpoint_arn(
            REGION,
            "GCM",
            "never-registered",
            "11111111-1111-1111-1111-111111111111",
        ),
    ));
    missing.push((
        "an ARN that names a topic rather than an endpoint".to_string(),
        topic,
    ));
    missing.push((
        "an ARN that names the application rather than one of its endpoints".to_string(),
        application,
    ));
    missing.push((
        "an ARN of another service entirely".to_string(),
        format!("arn:aws:sqs:{REGION}:{ACCOUNT}:push-queue"),
    ));
    for (case, value) in &missing {
        let answer = write(&sns, value, &entry).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
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

    let record = held(&sns, &endpoint);
    assert_eq!(
        record, before,
        "a refused write must persist nothing, got {record:?}"
    );

    // Once the endpoint is deleted the write that resolved before is the lookup miss too.
    let deleted = sns
        .call("DeleteEndpoint", &[("EndpointArn", &endpoint)])
        .await;
    assert_eq!(
        deleted.status, 200,
        "the delete expected 200, got {} / {}",
        deleted.status, deleted.body
    );
    let answer = write(&sns, &endpoint, &entry).await;
    assert_eq!(
        answer.status, 404,
        "case \"the endpoint after it was deleted\": expected 404, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(ENDPOINT_DOES_NOT_EXIST),
        "case \"the endpoint after it was deleted\": expected the fixed missing-endpoint \
         sentence, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Type").unwrap_or_default(),
        "Sender",
        "the lookup miss is a client fault, got {}",
        answer.body
    );
}
