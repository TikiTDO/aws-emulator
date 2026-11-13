// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreatePlatformEndpoint`, the call that registers a device as an
//! endpoint of a mobile-push platform application.
//!
//! The rule under test: a registration RESOLVES its `PlatformApplicationArn` against the
//! applications the account holds in the request's own region — an ARN that resolves to
//! none is the lookup miss (HTTP 404 / `NotFound`, "PlatformApplication does not exist"),
//! whatever it names — and against an application that is there it is idempotent on the
//! device token: the same token under the same attributes answers the ARN of the endpoint
//! that token already has, the same token under other attributes is refused as bad input
//! naming that endpoint, and any other token mints, persists and answers a new endpoint
//! ARN under that application. The request's own values are a separate refusal class
//! judged before the application is looked up (HTTP 400 / `InvalidParameter`): a value
//! that is not an ARN of this region, user data past its documented size, and an
//! attribute name SNS does not define for an endpoint of that platform.
//!
//! The rows differ along what the rule generalises over: the push platform (every entry
//! of the documented catalog), the device token's form and length, the attribute set
//! (none, each defined name, the two a Baidu endpoint carries, an undefined name), the
//! user data's size (both sides of the published bound), the region and account the ARN
//! names, the kind of resource the ARN names, and how many registrations are addressed
//! at one device token.
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls: a registered endpoint
//! is observed through `GetEndpointAttributes` and `ListEndpointsByPlatformApplication`.

use std::collections::HashMap;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    attribute_flag, custom_user_data_refusal, endpoint_token_conflict_refusal,
    mobile_push_unknown_attribute_refusal, platform_application_arn, platform_endpoint_arn,
    ATTRIBUTES_MEMBER, CHANNEL_ENDPOINT_ATTRIBUTE_NAMES, ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE,
    ENDPOINT_ENABLED_ATTRIBUTE, ENDPOINT_TOKEN_ATTRIBUTE, MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES,
    PLATFORM_APPLICATION_DOES_NOT_EXIST, PLATFORM_NAMES, PLATFORM_WITH_CHANNEL_ENDPOINTS,
};

use super::create_platform_application::{attribute_params, registered_application};
use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The region the harness signs its calls for unless a row names another one.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, as the sibling suites spell it.
const ACCOUNT: &str = "000000000000";

/// One `CreatePlatformEndpoint` call signed for `region`.
async fn register_in_region(
    sns: &Sns,
    region: &str,
    application_arn: &str,
    token: &str,
    custom_user_data: Option<&str>,
    attributes: &[(String, String)],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        (
            "PlatformApplicationArn".to_string(),
            application_arn.to_string(),
        ),
        ("Token".to_string(), token.to_string()),
    ];
    if let Some(data) = custom_user_data {
        params.push(("CustomUserData".to_string(), data.to_string()));
    }
    params.extend(attribute_params(attributes));
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call_in_region(region, "CreatePlatformEndpoint", &borrowed)
        .await
}

/// One `CreatePlatformEndpoint` call in the suite's own region.
async fn register(
    sns: &Sns,
    application_arn: &str,
    token: &str,
    custom_user_data: Option<&str>,
    attributes: &[(String, String)],
) -> Answer {
    register_in_region(
        sns,
        REGION,
        application_arn,
        token,
        custom_user_data,
        attributes,
    )
    .await
}

/// One registration asserted to have been SERVED, with the endpoint ARN it answered.
async fn registered(
    sns: &Sns,
    case: &str,
    application_arn: &str,
    token: &str,
    custom_user_data: Option<&str>,
    attributes: &[(String, String)],
) -> String {
    let answer = register(sns, application_arn, token, custom_user_data, attributes).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("EndpointArn")
        .unwrap_or_else(|| panic!("case {case:?}: no EndpointArn in {}", answer.body))
}

/// The attributes `GetEndpointAttributes` reports for `arn`.
async fn read(sns: &Sns, arn: &str) -> Answer {
    sns.call("GetEndpointAttributes", &[("EndpointArn", arn)])
        .await
}

/// Every endpoint ARN `ListEndpointsByPlatformApplication` reports for `application`.
async fn listed(sns: &Sns, case: &str, application: &str) -> Vec<String> {
    let answer = sns
        .call(
            "ListEndpointsByPlatformApplication",
            &[("PlatformApplicationArn", application)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the listing to answer 200, got {} / {}",
        answer.status, answer.body
    );
    answer.elements("EndpointArn")
}

/// How many endpoint records this account currently holds.
fn held(sns: &Sns) -> usize {
    sns.state().lock().platform_endpoints.len()
}

/// Whether `id` is in the 8-4-4-4-12 hexadecimal form the last segment of an SNS
/// endpoint ARN carries.
fn is_uuid_shaped(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8usize, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Owned attribute entries from a table row's borrowed pairs.
fn pairs(attributes: &[(&str, &str)]) -> Vec<(String, String)> {
    attributes
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One registration of the catalog table: a device of one push platform.
struct Row {
    case: &'static str,
    platform: &'static str,
    token: &'static str,
    custom_user_data: Option<&'static str>,
    attributes: Vec<(String, String)>,
}

/// One row per entry of the documented platform catalog, differing also in the device
/// token's form and length, the user data and the attribute set.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an Amazon Device Messaging registration with no attributes",
            platform: "ADM",
            token: "amzn1.adm-registration.v3.shortish-token",
            custom_user_data: None,
            attributes: Vec::new(),
        },
        Row {
            case: "an Apple production registration with user data",
            platform: "APNS",
            token: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
            custom_user_data: Some("cart-watcher"),
            attributes: Vec::new(),
        },
        Row {
            case: "an Apple sandbox registration that arrives disabled",
            platform: "APNS_SANDBOX",
            token: "0f0e0d0c0b0a09080706050403020100f",
            custom_user_data: None,
            attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "false")]),
        },
        Row {
            case: "a Baidu registration carrying the channel and user of its device",
            platform: PLATFORM_WITH_CHANNEL_ENDPOINTS,
            token: "channel-9876543210",
            custom_user_data: None,
            attributes: pairs(&[
                (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[0], "9876543210"),
                (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[1], "user-42"),
            ]),
        },
        Row {
            case: "a Firebase registration with a long token, user data and the flag",
            platform: "GCM",
            token: "cZ:APA91bF_longer-device-token_0123456789-abcdefghijklmnopqrstuvwxyz",
            custom_user_data: Some("{\"tier\":\"gold\",\"locale\":\"en-GB\"}"),
            attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]),
        },
        Row {
            case: "an MPNS registration with empty user data",
            platform: "MPNS",
            token: "http://sn1.notify.live.net/throttledthirdparty/01.00/AQE",
            custom_user_data: Some(""),
            attributes: Vec::new(),
        },
        Row {
            case: "a WNS registration with one attribute",
            platform: "WNS",
            token: "https://db5.notify.windows.com/?token=AgYAAAD",
            custom_user_data: None,
            attributes: pairs(&[(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE, "store-shelf")]),
        },
    ]
}

/// The attributes the endpoint a row registered must report: what the row carried, the
/// device token off the record, and the delivery flag an endpoint nobody disabled has.
fn expected_attributes(row: &Row) -> HashMap<String, String> {
    let mut expected: HashMap<String, String> = row.attributes.iter().cloned().collect();
    if let Some(data) = row.custom_user_data {
        expected.insert(
            ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE.to_string(),
            data.to_string(),
        );
    }
    expected.insert(ENDPOINT_TOKEN_ATTRIBUTE.to_string(), row.token.to_string());
    expected
        .entry(ENDPOINT_ENABLED_ATTRIBUTE.to_string())
        .or_insert_with(|| attribute_flag(true));
    expected
}

/// A device registers against an application of every platform of the documented
/// catalog: the endpoint ARN sits under that application, the record persists, and it
/// reads back through both reads of the endpoint plane.
#[tokio::test]
async fn a_device_registers_against_an_application_of_every_platform() {
    let sns = Sns::new();
    let table = rows();
    assert_eq!(
        table.len(),
        PLATFORM_NAMES.len(),
        "the table must cover every platform of the documented catalog, got {} rows \
         for {} platforms",
        table.len(),
        PLATFORM_NAMES.len()
    );

    for (index, row) in table.iter().enumerate() {
        let case = row.case;
        assert_eq!(
            row.platform, PLATFORM_NAMES[index],
            "case {case:?}: expected the catalog entry {:?}, got {:?}",
            PLATFORM_NAMES[index], row.platform
        );

        let name = format!("endpoints-{}", row.platform);
        let application = registered_application(&sns, REGION, &name, row.platform).await;
        let arn = registered(
            &sns,
            case,
            &application,
            row.token,
            row.custom_user_data,
            &row.attributes,
        )
        .await;

        // The ARN names the endpoint resource of the application that registered it,
        // with a minted id the device token is not readable off.
        let prefix = platform_endpoint_arn(REGION, row.platform, &name, "");
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case:?}: expected an ARN under {prefix:?}, got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case:?}: expected a minted endpoint id, got {id:?}"
        );
        assert!(
            !arn.contains(row.token),
            "case {case:?}: the ARN must not carry the device token, got {arn:?}"
        );
        assert_eq!(
            held(&sns),
            index + 1,
            "case {case:?}: expected the registration to persist one endpoint"
        );

        // The endpoint reads back: the attributes the registration carried, the device
        // token, and the delivery flag.
        let reported = attribute_map(&read(&sns, &arn).await);
        let expected = expected_attributes(row);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert_eq!(
            listed(&sns, case, &application).await,
            vec![arn.clone()],
            "case {case:?}: expected the application's listing to report the endpoint"
        );
    }

    // A regional service registers the device against the application of the region the
    // request addresses, and the ARN it answers carries that region.
    let elsewhere = "us-west-2";
    let application = registered_application(&sns, elsewhere, "endpoints-elsewhere", "GCM").await;
    let answer = register_in_region(
        &sns,
        elsewhere,
        &application,
        "token-in-another-region",
        None,
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"another region\": expected 200, got {} / {}",
        answer.status, answer.body
    );
    let arn = answer.element("EndpointArn").unwrap_or_default();
    assert!(
        arn.starts_with(&platform_endpoint_arn(
            elsewhere,
            "GCM",
            "endpoints-elsewhere",
            ""
        )),
        "case \"another region\": expected an ARN of that region's application, got {arn:?}"
    );
}

/// A `PlatformApplicationArn` that resolves to no application of this account is the
/// lookup miss, whatever the ARN names — and the miss persists nothing.
#[tokio::test]
async fn an_application_this_account_does_not_hold_is_a_not_found() {
    let sns = Sns::new();
    let application = registered_application(&sns, REGION, "held-app", "GCM").await;
    let endpoint = registered(&sns, "the held endpoint", &application, "token-held", None, &[]).await;
    let topic = sns.topic("endpoint-registration-topic").await;
    let before = held(&sns);
    assert_eq!(before, 1, "the suite starts from one held endpoint");

    // Every platform of the catalog, under a name nobody registered: the ARN is
    // well-formed and of this region, so nothing about it is bad input.
    let mut cases: Vec<(String, String)> = PLATFORM_NAMES
        .iter()
        .map(|platform| {
            (
                format!("a well-formed {platform} application ARN nobody registered"),
                platform_application_arn(REGION, platform, "never-registered"),
            )
        })
        .collect();
    cases.push((
        "the held application's name under another platform".to_string(),
        platform_application_arn(REGION, "ADM", "held-app"),
    ));
    cases.push((
        "an ARN that names a topic rather than an application".to_string(),
        topic.clone(),
    ));
    cases.push((
        "an ARN that names an endpoint rather than its application".to_string(),
        endpoint.clone(),
    ));
    cases.push((
        "an ARN of another service entirely".to_string(),
        format!("arn:aws:sqs:{REGION}:{ACCOUNT}:push-queue"),
    ));

    for (case, arn) in &cases {
        let answer = register(&sns, arn, "token-for-a-ghost", None, &[]).await;
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
            Some(PLATFORM_APPLICATION_DOES_NOT_EXIST),
            "case {case:?}: expected the fixed missing-application sentence, got {}",
            answer.body
        );
        assert!(
            answer.element("EndpointArn").is_none(),
            "case {case:?}: a refused registration must answer no ARN, got {}",
            answer.body
        );
        assert_eq!(
            held(&sns),
            before,
            "case {case:?}: a refused registration must persist nothing"
        );
    }

    // An application of ANOTHER region is not an application this endpoint of the
    // service can resolve at all: that is a fault in the value, not an absent record.
    let elsewhere = registered_application(&sns, "eu-west-1", "held-app", "GCM").await;
    let answer = register(&sns, &elsewhere, "token-for-another-region", None, &[]).await;
    assert_eq!(
        answer.status, 400,
        "case \"an application of another region\": expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "InvalidParameter",
        "case \"an application of another region\": expected the InvalidParameter code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some("Invalid parameter: PlatformApplicationArn"),
        "case \"an application of another region\": expected the member's bare refusal \
         sentence, got {}",
        answer.body
    );
    assert_eq!(
        held(&sns),
        before,
        "case \"an application of another region\": a refused registration must persist nothing"
    );

    // The endpoint the account does hold is untouched by every miss above.
    let reported = attribute_map(&read(&sns, &endpoint).await);
    assert_eq!(
        reported.get(ENDPOINT_TOKEN_ATTRIBUTE).map(String::as_str),
        Some("token-held"),
        "the held endpoint must still carry its device token, got {reported:?}"
    );
}

/// The registration is idempotent on the device token: the same token under the same
/// attributes addresses the endpoint that token already has, the same token under other
/// attributes is a conflict naming that endpoint, and another token is another endpoint.
#[tokio::test]
async fn the_registration_is_idempotent_on_the_device_token() {
    let sns = Sns::new();
    let application = registered_application(&sns, REGION, "idempotent-app", "GCM").await;
    let other = registered_application(&sns, REGION, "other-app", "GCM").await;
    let token = "cZ:APA91bF_idempotent-device-token";
    let attributes = pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]);

    let first = registered(
        &sns,
        "the first registration",
        &application,
        token,
        Some("shelf-1"),
        &attributes,
    )
    .await;

    // The same token and the same attributes, three times over: the same ARN, and never
    // a second record for one device.
    for attempt in 2..=4 {
        let case = format!("registration {attempt} of one device");
        let again = registered(&sns, &case, &application, token, Some("shelf-1"), &attributes).await;
        assert_eq!(
            again, first,
            "case {case:?}: expected the ARN the device already has, got {again:?}"
        );
        assert_eq!(
            held(&sns),
            1,
            "case {case:?}: expected one endpoint for one device"
        );
    }

    // A `Token` entry inside the attribute map carries the value every report already
    // takes off the record's own field, so it is absorbed rather than stored a second
    // time: the registration is still the idempotent one.
    let mut with_token = pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]);
    with_token.push((ENDPOINT_TOKEN_ATTRIBUTE.to_string(), token.to_string()));
    let absorbed = registered(
        &sns,
        "the device token repeated inside the attribute map",
        &application,
        token,
        Some("shelf-1"),
        &with_token,
    )
    .await;
    assert_eq!(
        absorbed, first,
        "case \"the device token repeated inside the attribute map\": expected the ARN \
         the device already has, got {absorbed:?}"
    );
    assert_eq!(
        held(&sns),
        1,
        "case \"the device token repeated inside the attribute map\": expected one \
         endpoint for one device"
    );

    // The same token under OTHER attributes is a conflict, not a second endpoint: the
    // refusal names the endpoint that holds the token so the caller can read or update
    // it. The sentence is pinned as literal text here, with only the ARN of the endpoint
    // the EMULATOR holds taken from state.
    let conflict_sentence = format!(
        "Invalid parameter: Token Reason: Endpoint {first} already exists with the same \
         Token, but different attributes."
    );
    assert_eq!(
        endpoint_token_conflict_refusal(&first),
        conflict_sentence,
        "the conflict sentence must be the fixed text around the held endpoint's ARN"
    );
    /// One conflicting registration of the SAME device token: the user data and the
    /// attribute map it carries instead.
    struct Conflict {
        case: &'static str,
        custom_user_data: Option<&'static str>,
        attributes: Vec<(String, String)>,
    }
    let conflicting = vec![
        Conflict {
            case: "other user data",
            custom_user_data: Some("shelf-2"),
            attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]),
        },
        Conflict {
            case: "no user data at all",
            custom_user_data: None,
            attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]),
        },
        Conflict {
            case: "the delivery flag turned off",
            custom_user_data: Some("shelf-1"),
            attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "false")]),
        },
        Conflict {
            case: "no attribute map at all",
            custom_user_data: Some("shelf-1"),
            attributes: Vec::new(),
        },
    ];
    for conflict in &conflicting {
        let case = conflict.case;
        let answer = register(
            &sns,
            &application,
            token,
            conflict.custom_user_data,
            &conflict.attributes,
        )
        .await;
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
            Some(conflict_sentence.as_str()),
            "case {case:?}: expected the conflict sentence naming the held endpoint, got {}",
            answer.body
        );
        assert_eq!(
            held(&sns),
            1,
            "case {case:?}: a refused registration must persist nothing"
        );
    }

    // Another device token of the same application is another endpoint; and the SAME
    // token under a DIFFERENT application is another endpoint too, since a device token
    // identifies a device inside one application.
    let second = registered(
        &sns,
        "another device of the same application",
        &application,
        "cZ:APA91bF_second-device-token",
        None,
        &[],
    )
    .await;
    assert_ne!(
        second, first,
        "another device token must get an endpoint of its own, got {second:?}"
    );
    let elsewhere = registered(
        &sns,
        "the same device token under another application",
        &other,
        token,
        None,
        &[],
    )
    .await;
    assert_ne!(
        elsewhere, first,
        "the same token under another application must get an endpoint of its own, got \
         {elsewhere:?}"
    );
    assert_eq!(
        held(&sns),
        3,
        "expected one endpoint per (application, device token) pair"
    );

    let mut reported = listed(&sns, "the idempotent application", &application).await;
    reported.sort();
    let mut expected = vec![first.clone(), second.clone()];
    expected.sort();
    assert_eq!(
        reported, expected,
        "expected the application's own two endpoints, got {reported:?}"
    );
}

/// The request's own values are judged before the application is resolved: a value that
/// is not an ARN at all, user data past its documented size, and an attribute name SNS
/// does not define for an endpoint of that platform.
#[tokio::test]
async fn bad_arguments_are_refused_as_bad_input() {
    let sns = Sns::new();
    let application = registered_application(&sns, REGION, "argument-app", "GCM").await;
    let baidu =
        registered_application(&sns, REGION, "argument-app", PLATFORM_WITH_CHANNEL_ENDPOINTS).await;

    /// Assert `answer` is the modelled client refusal carrying `message`, with nothing
    /// persisted.
    fn refused(case: &str, answer: &Answer, message: &str, sns: &Sns, persisted: usize) {
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
            Some(message),
            "case {case:?}: expected the sentence for the constraint it broke, got {}",
            answer.body
        );
        assert!(
            answer.element("EndpointArn").is_none(),
            "case {case:?}: a refused registration must answer no ARN, got {}",
            answer.body
        );
        assert_eq!(
            held(sns),
            persisted,
            "case {case:?}: a refused registration must persist nothing"
        );
    }

    // The two clauses of the ARN-shape rule, which the value fails before any record is
    // looked up — so it is bad input and not the lookup miss above.
    let shapes: &[(&str, &str, &str)] = &[
        (
            "not an ARN at all",
            "argument-app",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 1",
        ),
        (
            "an application resource with no ARN around it",
            "app/GCM/argument-app",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 1",
        ),
        (
            "an ARN missing its resource field",
            "arn:aws:sns:us-east-1:000000000000",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 5",
        ),
        (
            "six elements that do not begin with 'arn:'",
            "aws:aws:sns:us-east-1:000000000000:app/GCM/argument-app",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must begin with 'arn:'",
        ),
    ];
    for (case, value, message) in shapes {
        let answer = register(&sns, value, "token-for-a-bad-arn", None, &[]).await;
        refused(case, &answer, message, &sns, 0);
    }

    // The attribute map is the closed set SNS defines for an endpoint of that platform:
    // the two names a Baidu endpoint carries are defined for Baidu and for nothing else.
    let unknown: &[(&str, &str)] = &[
        ("a name of another resource's set", "PlatformCredential"),
        ("a misspelled defined name", "CustomUserdata"),
        ("a defined name in another case", "enabled"),
        ("a name nothing defines", "Whatever"),
        (
            "the channel of a Baidu endpoint, on another platform",
            CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[0],
        ),
        (
            "the user of a Baidu endpoint, on another platform",
            CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[1],
        ),
    ];
    for (case, name) in unknown {
        let answer = register(
            &sns,
            &application,
            "token-for-an-unknown-attribute",
            None,
            &pairs(&[(*name, "x")]),
        )
        .await;
        refused(
            case,
            &answer,
            &mobile_push_unknown_attribute_refusal(ATTRIBUTES_MEMBER, name),
            &sns,
            0,
        );
    }

    // The same two names ARE defined for an endpoint of the platform whose doc sentence
    // names them, so the closed set is per platform rather than one list for all.
    let channel = registered(
        &sns,
        "the channel and user of a Baidu endpoint",
        &baidu,
        "channel-1234567890",
        None,
        &pairs(&[
            (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[0], "1234567890"),
            (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[1], "user-7"),
        ]),
    )
    .await;
    let reported = attribute_map(&read(&sns, &channel).await);
    for name in CHANNEL_ENDPOINT_ATTRIBUTE_NAMES {
        assert!(
            reported.contains_key(*name),
            "the Baidu endpoint must report {name:?}, got {reported:?}"
        );
    }

    // The user data's published size bound, on each side of it: "less than 2KB"
    // (`CreatePlatformEndpointInput$CustomUserData` doc), so the most an endpoint holds
    // is one byte under 2 KiB — the bound itself is accepted and one byte past it is not.
    assert_eq!(
        MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES,
        2 * 1024 - 1,
        "the published bound is one byte under 2 KiB, got {MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES}"
    );
    let at_bound = "u".repeat(MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES);
    let past_bound = "u".repeat(MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES + 1);
    let accepted = registered(
        &sns,
        "user data at the published bound",
        &application,
        "token-at-the-bound",
        Some(&at_bound),
        &[],
    )
    .await;
    let reported = attribute_map(&read(&sns, &accepted).await);
    assert_eq!(
        reported
            .get(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE)
            .map(String::len),
        Some(MAX_ENDPOINT_CUSTOM_USER_DATA_BYTES),
        "case \"user data at the published bound\": expected the data to read back whole, \
         got {:?}",
        reported.get(ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE).map(String::len)
    );

    let held_now = held(&sns);
    let answer = register(
        &sns,
        &application,
        "token-past-the-bound",
        Some(&past_bound),
        &[],
    )
    .await;
    // The size refusal, pinned as literal text: the member, the published ceiling and
    // the size the request offered.
    let message = "Invalid parameter: CustomUserData - at most 2047 bytes: 2048";
    assert_eq!(
        custom_user_data_refusal(&past_bound).as_deref(),
        Some(message),
        "the size refusal must state the member, the ceiling and the size offered"
    );
    refused(
        "user data one byte past the published bound",
        &answer,
        message,
        &sns,
        held_now,
    );
    assert!(
        !answer.body.contains(&past_bound),
        "the refusal must state the size, not echo the data, got {}",
        answer.body
    );
}
