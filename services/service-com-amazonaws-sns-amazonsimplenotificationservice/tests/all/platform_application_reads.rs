// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two mobile-push reads scoped by a PLATFORM APPLICATION:
//! `GetPlatformApplicationAttributes` and `ListEndpointsByPlatformApplication`.
//!
//! The rule under test: a read scoped by a platform-application ARN reports from the
//! application record that ARN names — the attributes the record holds with the
//! write-only push credential material withheld, and one entry per device endpoint of
//! that application, oldest first and paged at up to [`PAGE_SIZE`] endpoints with a
//! `NextToken` only while endpoints remain — and refuses an ARN this account holds no
//! application record for with the modelled lookup miss (HTTP 404 / `NotFound`), while a
//! value that is not an ARN at all is bad input (HTTP 400 / `InvalidParameter`). Neither
//! read changes what a later caller sees.
//!
//! The rows differ along what the rule generalises over: the push platform, the
//! application name, the attribute set the application holds (none, one, several,
//! credential-bearing), the endpoint count walked (none, one, several, past a page
//! boundary), which application an endpoint belongs to, and the shape of the ARN
//! presented.
//!
//! The application and endpoint records are seeded straight into persisted state because
//! the calls that register them, `CreatePlatformApplication` and
//! `CreatePlatformEndpoint`, are scaffold stubs with issues of their own — the same way
//! the sibling mobile-push suites seed theirs. Every read goes over the wire through the
//! router the emulator serves in production, so nothing but persisted state carries
//! information between calls.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    platform_endpoint_arn, PlatformEndpointRecord, ENDPOINT_ENABLED_ATTRIBUTE,
    ENDPOINT_TOKEN_ATTRIBUTE, MOBILE_PUSH_INVALID_NEXT_TOKEN_REFUSAL, PAGE_SIZE,
    PLATFORM_APPLICATION_DOES_NOT_EXIST, PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES,
};

use super::platform_applications::{entries_of, seed_in_region};
use super::topics_subscriptions_publish::{attribute_map, text_of, Answer, Sns};

/// The region the test harness signs its calls for, and so the region the seeded ARNs
/// carry.
const REGION: &str = "us-east-1";

/// One seeded platform application: the push platform, the application name and the
/// attributes its record holds.
///
/// The platforms are the ones `CreatePlatformApplicationInput$Platform` documents (ADM,
/// APNS, APNS_SANDBOX, GCM, Baidu, MPNS, WNS); the attribute names are from the
/// `SetPlatformApplicationAttributesInput$Attributes` doc
/// (`smithy_query(query="doc", shape="SetPlatformApplicationAttributesInput",
/// member="Attributes")`), which lists `PlatformCredential`, `PlatformPrincipal`, the
/// four `Event*` topics, the two feedback role ARNs, `SuccessFeedbackSampleRate` and the
/// two APNs token-based `ApplePlatform*` names.
type ApplicationCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// The applications the attribute-read table covers: a platform not cited by any other
/// suite row, every attribute set shape, and a value the envelope has to escape.
const APPLICATIONS: &[ApplicationCase] = &[
    ("no attributes at all", "WNS", "read-wns-bare", &[]),
    (
        "one event topic",
        "MPNS",
        "read.mpns_app-7",
        &[(
            "EventEndpointUpdated",
            "arn:aws:sns:us-east-1:000000000000:endpoint-updated",
        )],
    ),
    (
        "credential material only",
        "Baidu",
        "read-baidu-credentials",
        &[
            ("PlatformCredential", "baidu-secret-key"),
            ("PlatformPrincipal", "baidu-api-key"),
        ],
    ),
    (
        "credential material beside reportable attributes",
        "APNS",
        "read-apns-mixed",
        &[
            ("PlatformCredential", "-----BEGIN PRIVATE KEY-----"),
            ("PlatformPrincipal", "signing-key-id"),
            ("ApplePlatformTeamID", "TEAM987654"),
            ("ApplePlatformBundleID", "com.example.reader"),
            ("SuccessFeedbackSampleRate", "25"),
        ],
    ),
    (
        "a value the envelope must escape",
        "GCM",
        "read-gcm-escaped",
        &[(
            "EventDeliveryFailure",
            "arn:aws:sns:us-east-1:000000000000:a&b",
        )],
    ),
];

/// One seeded device endpoint of an application.
struct EndpointCase {
    case: &'static str,
    id: &'static str,
    token: &'static str,
    attributes: &'static [(&'static str, &'static str)],
}

/// The endpoints the listing table covers: the attribute sets an endpoint record takes
/// (none, the delivery flag on and off, user data) and device tokens of different forms.
const ENDPOINTS: &[EndpointCase] = &[
    EndpointCase {
        case: "no attributes at all",
        id: "11111111-1111-4111-8111-111111111111",
        token: "amzn1.adm-registration.v3.short",
        attributes: &[],
    },
    EndpointCase {
        case: "user data and the delivery flag",
        id: "22222222-2222-4222-8222-222222222222",
        token: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
        attributes: &[("Enabled", "true"), ("CustomUserData", "cart-watcher")],
    },
    EndpointCase {
        case: "a disabled endpoint",
        id: "33333333-3333-4333-8333-333333333333",
        token: "0f0e0d0c0b0a09080706050403020100f0e0d0c0b0a0908070605040302010",
        attributes: &[("Enabled", "false")],
    },
    EndpointCase {
        case: "empty user data",
        id: "44444444-4444-4444-8444-444444444444",
        token: "cZ:APA91bF_longer-device-token_0123456789-abcdefghijklmnopqrstuvwxyz",
        attributes: &[("CustomUserData", "")],
    },
];

/// One device endpoint to write into persisted state: the application it belongs to, as
/// its ARN spells it, and what its record holds. `at` orders the records, so two seeded
/// endpoints never share a creation instant.
struct Device<'a> {
    platform: &'a str,
    application: &'a str,
    id: &'a str,
    token: &'a str,
    attributes: &'a [(&'a str, &'a str)],
    at: u64,
}

/// Write one device endpoint of `application_arn` into persisted state and answer its
/// ARN.
fn seed_endpoint(sns: &Sns, application_arn: &str, device: Device<'_>) -> String {
    let arn = platform_endpoint_arn(REGION, device.platform, device.application, device.id);
    let record = PlatformEndpointRecord {
        arn: arn.clone(),
        platform_application_arn: application_arn.to_string(),
        token: device.token.to_string(),
        attributes: device
            .attributes
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
        created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(device.at + 1),
    };
    let state = sns.state();
    state.lock().platform_endpoints.insert(arn.clone(), record);
    arn
}

/// The body of the response's `<Endpoints>` element, or `None` when the response carries
/// no such member at all — a refused call serves no listing.
fn endpoints_block(body: &str) -> Option<&str> {
    if let Some(start) = body.find("<Endpoints>") {
        let rest = &body[start + "<Endpoints>".len()..];
        let end = rest.find("</Endpoints>")?;
        return Some(&rest[..end]);
    }
    body.contains("<Endpoints/>").then_some("")
}

/// The `(EndpointArn, Attributes)` pairs the listing carries, in document order.
///
/// A list member's open tag carries the envelope's default namespace, so the opening is
/// matched up to its first `>` rather than as a bare `<member>`.
fn reported(answer: &Answer) -> Vec<(String, HashMap<String, String>)> {
    let block = endpoints_block(&answer.body)
        .unwrap_or_else(|| panic!("no Endpoints member in {}", answer.body));
    let mut found = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find("<member") {
        rest = &rest[start..];
        let Some(open) = rest.find('>') else { break };
        rest = &rest[open + 1..];
        let Some(end) = rest.find("</member>") else {
            break;
        };
        let member = &rest[..end];
        found.push((text_of(member, "EndpointArn"), entries_of(member)));
        rest = &rest[end + "</member>".len()..];
    }
    found
}

/// The endpoint ARNs the listing reports, in document order.
fn reported_arns(answer: &Answer) -> Vec<String> {
    reported(answer).into_iter().map(|(arn, _)| arn).collect()
}

/// One `GetPlatformApplicationAttributes` call.
async fn read_application(sns: &Sns, arn: &str) -> Answer {
    sns.call(
        "GetPlatformApplicationAttributes",
        &[("PlatformApplicationArn", arn)],
    )
    .await
}

/// One `ListEndpointsByPlatformApplication` call, with the optional paging parameter.
async fn list_endpoints(sns: &Sns, arn: &str, token: Option<&str>) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![("PlatformApplicationArn", arn)];
    if let Some(token) = token {
        params.push(("NextToken", token));
    }
    sns.call("ListEndpointsByPlatformApplication", &params).await
}

/// The attributes an application's record holds that a report carries: all of them
/// except the write-only credential material.
fn expected_attributes(attributes: &[(&str, &str)]) -> HashMap<String, String> {
    attributes
        .iter()
        .filter(|(name, _)| !PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES.contains(name))
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// The application read reports the attributes the record holds, withholds the
/// credential material, agrees with the account-wide listing about every application,
/// and changes nothing.
#[tokio::test]
async fn the_application_read_reports_the_attributes_the_record_holds() {
    let sns = Sns::new();

    let seeded: Vec<String> = APPLICATIONS
        .iter()
        .enumerate()
        .map(|(at, case)| seed_in_region(&sns, REGION, at as u64, case.1, case.2, case.3))
        .collect();

    for (at, case) in APPLICATIONS.iter().enumerate() {
        let (label, _, _, attributes) = *case;
        let arn = seeded[at].as_str();
        let answer = read_application(&sns, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let reported = attribute_map(&answer);
        let expected = expected_attributes(attributes);
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the attributes {expected:?}, got {reported:?}"
        );
        for (name, value) in attributes.iter() {
            if !PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES.contains(name) {
                continue;
            }
            assert!(
                !answer.body.contains(value),
                "case {label:?}: the read must withhold the {name} value, got {}",
                answer.body
            );
        }

        // A read must not change what the next read sees.
        let again = read_application(&sns, arn).await;
        assert_eq!(
            attribute_map(&again),
            reported,
            "case {label:?}: a second read must report the same attributes, got {}",
            again.body
        );
    }

    // One application reads the same whichever call asked for it: the account-wide
    // listing holds every application the single read served.
    let listing = sns.call("ListPlatformApplications", &[]).await;
    assert_eq!(
        listing.status, 200,
        "the listing must answer 200, got {} / {}",
        listing.status, listing.body
    );
    for arn in &seeded {
        assert!(
            listing.body.contains(arn),
            "expected the listing to hold {arn}, got {}",
            listing.body
        );
    }
    assert_eq!(
        sns.state().lock().platform_applications.len(),
        seeded.len(),
        "the reads must not add or drop an application record"
    );
}

/// The endpoint listing reports one entry per device endpoint OF THE APPLICATION NAMED —
/// oldest first, each carrying the attributes that endpoint's own read reports — and
/// reports none of another application's endpoints.
#[tokio::test]
async fn the_endpoint_listing_reports_every_endpoint_of_the_application() {
    let sns = Sns::new();
    let application = seed_in_region(&sns, REGION, 0, "GCM", "listed-app", &[]);
    let other = seed_in_region(&sns, REGION, 1, "APNS", "other-app", &[]);

    // An application holding no endpoint yet: a served, empty listing — not a miss.
    let empty = list_endpoints(&sns, &application, None).await;
    assert_eq!(
        empty.status, 200,
        "an application holding no endpoint: expected 200, got {} / {}",
        empty.status, empty.body
    );
    assert!(
        reported(&empty).is_empty(),
        "an application holding no endpoint: expected no entries, got {:?}",
        reported_arns(&empty)
    );
    assert!(
        empty.element("NextToken").is_none(),
        "an application holding no endpoint: expected no NextToken, got {}",
        empty.body
    );

    // One endpoint of the OTHER application, which must never be reported here.
    let foreign = seed_endpoint(
        &sns,
        &other,
        Device {
            platform: "APNS",
            application: "other-app",
            id: "99999999-9999-4999-8999-999999999999",
            token: "foreign-device-token",
            attributes: &[],
            at: 99,
        },
    );

    let mut seeded: Vec<String> = Vec::new();
    for (at, endpoint) in ENDPOINTS.iter().enumerate() {
        let arn = seed_endpoint(
            &sns,
            &application,
            Device {
                platform: "GCM",
                application: "listed-app",
                id: endpoint.id,
                token: endpoint.token,
                attributes: endpoint.attributes,
                at: at as u64,
            },
        );
        seeded.push(arn.clone());

        let answer = list_endpoints(&sns, &application, None).await;
        let case = endpoint.case;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let entries = reported(&answer);
        assert_eq!(
            entries.iter().map(|(arn, _)| arn.clone()).collect::<Vec<_>>(),
            seeded,
            "case {case:?}: expected every endpoint of the application once, oldest \
             first, got {}",
            answer.body
        );
        assert!(
            !answer.body.contains(&foreign),
            "case {case:?}: another application's endpoint must not be listed, got {}",
            answer.body
        );

        // The listed attributes are the ones the endpoint's own read reports: one
        // endpoint does not describe itself differently per call.
        let single = sns
            .call("GetEndpointAttributes", &[("EndpointArn", &arn)])
            .await;
        assert_eq!(
            single.status, 200,
            "case {case:?}: expected the endpoint read to answer 200, got {} / {}",
            single.status, single.body
        );
        let listed = entries
            .iter()
            .find(|(reported_arn, _)| *reported_arn == arn)
            .map(|(_, attributes)| attributes.clone())
            .unwrap_or_else(|| panic!("case {case:?}: no entry for {arn} in {}", answer.body));
        assert_eq!(
            listed,
            attribute_map(&single),
            "case {case:?}: the listing and the endpoint read must agree, got {listed:?}"
        );
        assert_eq!(
            listed.get(ENDPOINT_TOKEN_ATTRIBUTE).map(String::as_str),
            Some(endpoint.token),
            "case {case:?}: expected the device token the record holds, got {listed:?}"
        );
        let expected_enabled = endpoint
            .attributes
            .iter()
            .find(|(name, _)| *name == ENDPOINT_ENABLED_ATTRIBUTE)
            .map_or("true", |(_, value)| value);
        assert_eq!(
            listed.get(ENDPOINT_ENABLED_ATTRIBUTE).map(String::as_str),
            Some(expected_enabled),
            "case {case:?}: expected the delivery flag the record holds, defaulting to \
             enabled, got {listed:?}"
        );
    }

    // A read must not change what the next read sees.
    let first = list_endpoints(&sns, &application, None).await;
    let second = list_endpoints(&sns, &application, None).await;
    assert_eq!(
        reported(&second),
        reported(&first),
        "a second listing must report the same endpoints and attributes, got {}",
        second.body
    );
    assert_eq!(
        sns.state().lock().platform_endpoints.len(),
        seeded.len() + 1,
        "a listing must not add or drop an endpoint record"
    );
}

/// The listing pages at up to [`PAGE_SIZE`] endpoints and the token walks the pages: the
/// pages concatenate to every endpoint of the application, oldest first, each once, and
/// only the last page carries no token. A token the listing did not mint is refused.
#[tokio::test]
async fn the_endpoint_listing_pages_at_the_documented_page_size() {
    let sns = Sns::new();
    let application = seed_in_region(&sns, REGION, 0, "ADM", "paged-app", &[]);

    // Two full pages and a partial third, so a page boundary falls both on and off the
    // end of the set.
    let count = PAGE_SIZE * 2 + PAGE_SIZE / 2;
    let seeded: Vec<String> = (0..count)
        .map(|at| {
            let id = format!("00000000-0000-4000-8000-{at:012}");
            let token = format!("device-token-{at:04}");
            seed_endpoint(
                &sns,
                &application,
                Device {
                    platform: "ADM",
                    application: "paged-app",
                    id: &id,
                    token: &token,
                    attributes: &[],
                    at: at as u64,
                },
            )
        })
        .collect();

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let answer = list_endpoints(&sns, &application, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "page {pages}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let page = reported_arns(&answer);
        assert!(
            page.len() <= PAGE_SIZE,
            "page {pages}: expected at most {PAGE_SIZE} endpoints, got {}",
            page.len()
        );
        assert!(
            !page.is_empty(),
            "page {pages}: a page a token led to must not be empty"
        );
        walked.extend(page);
        pages += 1;
        token = answer.element("NextToken");
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() < seeded.len(),
            "a NextToken was returned with every endpoint already reported, got {} of {}",
            walked.len(),
            seeded.len()
        );
    }

    assert_eq!(
        walked, seeded,
        "expected every endpoint once, oldest first, got {} of {}",
        walked.len(),
        seeded.len()
    );
    let expected_pages = seeded.len().div_ceil(PAGE_SIZE);
    assert_eq!(
        pages, expected_pages,
        "expected {expected_pages} pages, got {pages}"
    );

    // A token this listing never minted names no page to serve.
    let tokens: &[(&str, &str)] = &[
        ("not a token at all", "bogus"),
        ("past the end of the set", "99999"),
        ("negative", "-1"),
        ("fractional", "1.5"),
        ("whitespace", " "),
        ("an ARN", "arn:aws:sns:us-east-1:000000000000:app/ADM/paged-app"),
    ];
    for (case, bad) in tokens {
        let answer = list_endpoints(&sns, &application, Some(bad)).await;
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
            Some(MOBILE_PUSH_INVALID_NEXT_TOKEN_REFUSAL),
            "case {case:?}: expected the member's refusal sentence, got {}",
            answer.body
        );
        assert!(
            endpoints_block(&answer.body).is_none(),
            "case {case:?}: a refused request must serve no listing, got {}",
            answer.body
        );
    }
}

/// Both reads refuse an ARN this account holds no platform application for with the
/// modelled lookup miss, and a value that is not an ARN at all as bad input — neither
/// serving a report and neither touching the store.
#[tokio::test]
async fn both_reads_refuse_an_application_the_account_does_not_hold() {
    let sns = Sns::new();
    // One held application with one endpoint, so a refusal cannot pass by the store
    // being empty.
    let held = seed_in_region(&sns, REGION, 0, "GCM", "held-app", &[("SuccessFeedbackSampleRate", "5")]);
    seed_endpoint(
        &sns,
        &held,
        Device {
            platform: "GCM",
            application: "held-app",
            id: "55555555-5555-4555-8555-555555555555",
            token: "held-device-token",
            attributes: &[],
            at: 0,
        },
    );

    // Well-formed ARNs this account holds no application for: a name nobody registered,
    // another platform of a held name, another service, and SNS ARNs naming another KIND
    // of resource. An ARN naming another REGION is not one this endpoint can read at all,
    // and is refused below; one naming another ACCOUNT is refused before the lookup, by
    // the account-resolution refusal every ARN-addressed call of this service shares (see
    // the `arn_reference_refusals` suite).
    let missing: &[(&str, &str)] = &[
        (
            "a name nobody registered",
            "arn:aws:sns:us-east-1:000000000000:app/GCM/absent-app",
        ),
        (
            "another platform of a held name",
            "arn:aws:sns:us-east-1:000000000000:app/APNS/held-app",
        ),
        (
            "another service",
            "arn:aws:sqs:us-east-1:000000000000:app/GCM/held-app",
        ),
        (
            "a topic ARN",
            "arn:aws:sns:us-east-1:000000000000:some-topic",
        ),
        (
            "an endpoint ARN",
            "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/held-app/\
             55555555-5555-4555-8555-555555555555",
        ),
    ];

    for (case, arn) in missing {
        for (operation, answer) in [
            ("GetPlatformApplicationAttributes", read_application(&sns, arn).await),
            (
                "ListEndpointsByPlatformApplication",
                list_endpoints(&sns, arn, None).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?} on {operation}: expected 404, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "NotFound",
                "case {case:?} on {operation}: expected the NotFound code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(PLATFORM_APPLICATION_DOES_NOT_EXIST),
                "case {case:?} on {operation}: expected the fixed missing-application \
                 sentence, got {}",
                answer.body
            );
            assert!(
                endpoints_block(&answer.body).is_none(),
                "case {case:?} on {operation}: a refused read must serve no listing, got {}",
                answer.body
            );
        }
    }

    // An ARN of ANOTHER REGION: a regional endpoint serves the applications of its own
    // region only, so such a value is refused as bad input rather than reported as an
    // application this account does not hold.
    let elsewhere: &[(&str, &str)] = &[
        (
            "another region",
            "arn:aws:sns:eu-west-1:000000000000:app/GCM/held-app",
        ),
        (
            "another partition's region",
            "arn:aws-us-gov:sns:us-gov-west-1:000000000000:app/GCM/held-app",
        ),
    ];
    for (case, arn) in elsewhere {
        for (operation, answer) in [
            ("GetPlatformApplicationAttributes", read_application(&sns, arn).await),
            (
                "ListEndpointsByPlatformApplication",
                list_endpoints(&sns, arn, None).await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {operation}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?} on {operation}: expected the InvalidParameter code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some("Invalid parameter: PlatformApplicationArn"),
                "case {case:?} on {operation}: expected the member's bare refusal \
                 sentence, got {}",
                answer.body
            );
            assert!(
                endpoints_block(&answer.body).is_none(),
                "case {case:?} on {operation}: a refused read must serve no listing, got {}",
                answer.body
            );
        }
    }

    // Values that are not ARNs at all: too few elements, no `arn:` prefix.
    let malformed = [
        "does-not-exist",
        "arn:aws:sns",
        "arn:aws:sns:us-east-1",
        "aws:sns:us-east-1:000000000000:app/GCM/held-app",
    ];
    for arn in malformed {
        for (operation, answer) in [
            ("GetPlatformApplicationAttributes", read_application(&sns, arn).await),
            (
                "ListEndpointsByPlatformApplication",
                list_endpoints(&sns, arn, None).await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {arn:?} on {operation}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {arn:?} on {operation}: expected the InvalidParameter code, got {}",
                answer.body
            );
            let message = answer.element("Message").unwrap_or_default();
            assert!(
                message.starts_with("Invalid parameter: PlatformApplicationArn Reason: "),
                "case {arn:?} on {operation}: expected the member's refusal sentence, \
                 got {message:?}"
            );
        }
    }

    // The held application still reads, and the refusals changed nothing.
    let served = read_application(&sns, &held).await;
    assert_eq!(
        served.status, 200,
        "the held application must still read, got {} / {}",
        served.status, served.body
    );
    assert_eq!(
        attribute_map(&served).get("SuccessFeedbackSampleRate").map(String::as_str),
        Some("5"),
        "the held application must still report the attribute it holds, got {}",
        served.body
    );
    let listed = list_endpoints(&sns, &held, None).await;
    assert_eq!(
        reported_arns(&listed).len(),
        1,
        "the held application must still list its one endpoint, got {}",
        listed.body
    );
    let store = sns.state();
    let store = store.lock();
    assert_eq!(
        (store.platform_applications.len(), store.platform_endpoints.len()),
        (1, 1),
        "the refused reads must leave the store exactly as it was"
    );
}
