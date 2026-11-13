// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `SetPlatformApplicationAttributes`, the call that writes the
//! attributes of a mobile-push platform application object.
//!
//! The rule under test: a write carrying an attribute MAP applies the entries that map
//! holds over the ones the addressed record already holds and leaves every attribute the
//! request does not name as it was — except an attribute the SERVICE maintains (the
//! delivery flag), whose value is not a caller's to write — so the record reads back
//! through `GetPlatformApplicationAttributes` with the write-only push credential
//! material withheld; the write is refused with the operation's modelled client error
//! (HTTP 400 / `InvalidParameter`) when the ARN is not an ARN of the region this endpoint
//! serves or the map carries a name SNS does not define for an application, with the
//! modelled lookup miss (HTTP 404 / `NotFound`) when the account holds no application
//! record under that ARN, and with the credential refusal when the material it would
//! store is not the artifact the application's platform authenticates with — and a
//! refused write persists nothing.
//!
//! The rows differ along what the rule generalises over: the push platform (every entry
//! of the documented catalog), the attribute written (every entry of the defined
//! catalog, names outside it, one entry and several at once), the number of writes
//! addressed at one record, which application an ARN names, the form of the ARN
//! presented, and the form of the credential material offered.
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls.

use std::collections::HashMap;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    attribute_flag, mobile_push_unknown_attribute_refusal, platform_application_arn,
    ATTRIBUTES_MEMBER, INVALID_PLATFORM_CREDENTIALS_REFUSAL,
    PLATFORM_APPLICATION_ATTRIBUTES_A_WRITE_DOES_NOT_TAKE, PLATFORM_APPLICATION_ATTRIBUTE_NAMES,
    PLATFORM_APPLICATION_DOES_NOT_EXIST, PLATFORM_APPLICATION_ENABLED_ATTRIBUTE,
    PLATFORM_CREDENTIAL_ATTRIBUTE, PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES, PLATFORM_NAMES,
    PLATFORM_PRINCIPAL_ATTRIBUTE,
};

use super::create_platform_application::{attribute_params, credentials, registered_application};
use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The region the harness signs its calls for unless a row names another one.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, as the sibling suites spell it.
const ACCOUNT: &str = "000000000000";

/// One `SetPlatformApplicationAttributes` call signed for `region`.
async fn write_in_region(
    sns: &Sns,
    region: &str,
    application_arn: &str,
    attributes: &[(String, String)],
) -> Answer {
    let mut params: Vec<(String, String)> = vec![(
        "PlatformApplicationArn".to_string(),
        application_arn.to_string(),
    )];
    params.extend(attribute_params(attributes));
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call_in_region(region, "SetPlatformApplicationAttributes", &borrowed)
        .await
}

/// One `SetPlatformApplicationAttributes` call in the suite's own region.
async fn write(sns: &Sns, application_arn: &str, attributes: &[(String, String)]) -> Answer {
    write_in_region(sns, REGION, application_arn, attributes).await
}

/// One write asserted to have been SERVED.
async fn written(sns: &Sns, case: &str, application_arn: &str, attributes: &[(String, String)]) {
    let answer = write(sns, application_arn, attributes).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// The attributes `GetPlatformApplicationAttributes` reports for `arn`.
async fn reported(sns: &Sns, case: &str, arn: &str) -> HashMap<String, String> {
    let answer = sns
        .call(
            "GetPlatformApplicationAttributes",
            &[("PlatformApplicationArn", arn)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the attribute read expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// The attributes the application's PERSISTED record holds, credential material
/// included — what a report withholds is still what the record has to carry.
fn held(sns: &Sns, arn: &str) -> HashMap<String, String> {
    sns.state()
        .lock()
        .platform_applications
        .get(arn)
        .unwrap_or_else(|| panic!("no application record under {arn}"))
        .attributes
        .clone()
}

/// The entries of `attributes` a report of the application carries: every one except the
/// write-only push credential material.
fn reportable(attributes: &HashMap<String, String>) -> HashMap<String, String> {
    attributes
        .iter()
        .filter(|(name, _)| !PLATFORM_CREDENTIAL_ATTRIBUTE_NAMES.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// A value of the kind the attribute `name` of an application of `platform` takes.
///
/// The names are the catalog on `PLATFORM_APPLICATION_ATTRIBUTE_NAMES`, which comes from
/// the `SetPlatformApplicationAttributesInput$Attributes` doc
/// (`smithy_query(query="doc", shape="SetPlatformApplicationAttributesInput",
/// member="Attributes")`): the credential pair, the four `Event*` topic ARNs the endpoint
/// lifecycle events are published to, the two feedback role ARNs with their sample rate
/// percentage, the delivery flag, and the two APNs token-based `ApplePlatform*`
/// identifiers. The credential pair is the material that platform's own documented
/// credential type names, taken from the sibling create suite rather than re-derived.
fn value_for(platform: &str, name: &str) -> String {
    if let Some((_, value)) = credentials(platform)
        .into_iter()
        .find(|(offered, _)| offered == name)
    {
        return value;
    }
    match name {
        PLATFORM_PRINCIPAL_ATTRIBUTE => "signing-key-id".to_string(),
        "SuccessFeedbackSampleRate" => "25".to_string(),
        "ApplePlatformTeamID" => "TEAM123456".to_string(),
        "ApplePlatformBundleID" => "com.example.push".to_string(),
        flag if flag == PLATFORM_APPLICATION_ENABLED_ATTRIBUTE => attribute_flag(false),
        role if role.ends_with("RoleArn") => format!("arn:aws:iam::{ACCOUNT}:role/sns-{role}"),
        topic => format!("arn:aws:sns:{REGION}:{ACCOUNT}:{topic}-events"),
    }
}

/// The attribute names this write does NOT take from its caller, spelled out here
/// rather than read from the handler's own predicate, so a set that grew by mistake
/// fails this suite.
///
/// One name: the delivery flag. The `SetPlatformApplicationAttributesInput$Attributes`
/// doc enumerates what this call writes — the credential pair, the four `Event*` topics,
/// the two feedback roles with their sample rate and the two `ApplePlatform*`
/// identifiers — and the flag is in none of those clauses.
const SERVICE_SET_ATTRIBUTE_NAMES: &[&str] = &["Enabled"];

/// The entries of a written map the record KEEPS: every one except the attributes this
/// write does not take from its caller.
fn applied(attributes: &[(String, String)]) -> Vec<(String, String)> {
    attributes
        .iter()
        .filter(|(name, _)| !SERVICE_SET_ATTRIBUTE_NAMES.contains(&name.as_str()))
        .cloned()
        .collect()
}

/// Every defined attribute is written on every platform of the documented catalog, one
/// name at a time, and each write leaves the attributes it does not name alone: the
/// record reads back as the map the writes so far produced, credential material
/// withheld.
#[tokio::test]
async fn each_write_applies_its_entries_over_the_attributes_the_record_already_holds() {
    let sns = Sns::new();
    assert_eq!(
        PLATFORM_APPLICATION_ATTRIBUTES_A_WRITE_DOES_NOT_TAKE, SERVICE_SET_ATTRIBUTE_NAMES,
        "the attributes this write does not take must be the delivery flag alone, got {:?}",
        PLATFORM_APPLICATION_ATTRIBUTES_A_WRITE_DOES_NOT_TAKE
    );
    assert_eq!(
        PLATFORM_APPLICATION_ENABLED_ATTRIBUTE, SERVICE_SET_ATTRIBUTE_NAMES[0],
        "the delivery flag must be the attribute this suite spells out"
    );

    for platform in PLATFORM_NAMES {
        let arn = registered_application(&sns, REGION, &format!("set-{platform}"), platform).await;

        // What the registered application starts from: the delivery flag the create
        // writes, beside the credential pair it carried.
        let mut expected: HashMap<String, String> = HashMap::from([(
            PLATFORM_APPLICATION_ENABLED_ATTRIBUTE.to_string(),
            attribute_flag(true),
        )]);
        expected.extend(credentials(platform));
        let case = format!("{platform} / before any write");
        assert_eq!(
            reported(&sns, &case, &arn).await,
            reportable(&expected),
            "case {case:?}: expected the create's own attributes, got {:?}",
            held(&sns, &arn)
        );

        for name in PLATFORM_APPLICATION_ATTRIBUTE_NAMES {
            let case = format!("{platform} / {name}");
            let entry = vec![((*name).to_string(), value_for(platform, name))];
            written(&sns, &case, &arn, &entry).await;
            expected.extend(applied(&entry));

            let report = reported(&sns, &case, &arn).await;
            let want = reportable(&expected);
            assert_eq!(
                report, want,
                "case {case:?}: expected the report {want:?}, got {report:?}"
            );
            let record = held(&sns, &arn);
            assert_eq!(
                record, expected,
                "case {case:?}: expected the record to hold {expected:?}, got {record:?}"
            );
        }

        // Several entries in ONE map, and the same map a second time: the write is
        // idempotent, and still leaves the names it does not carry alone.
        let several: Vec<(String, String)> = vec![
            (
                "EventEndpointCreated".to_string(),
                format!("arn:aws:sns:{REGION}:{ACCOUNT}:created-again"),
            ),
            ("SuccessFeedbackSampleRate".to_string(), "100".to_string()),
            (
                PLATFORM_APPLICATION_ENABLED_ATTRIBUTE.to_string(),
                attribute_flag(true),
            ),
        ];
        for round in 1..=2 {
            let case = format!("{platform} / several entries at once, round {round}");
            written(&sns, &case, &arn, &several).await;
            expected.extend(applied(&several));
            let report = reported(&sns, &case, &arn).await;
            let want = reportable(&expected);
            assert_eq!(
                report, want,
                "case {case:?}: expected the report {want:?}, got {report:?}"
            );
        }
    }

    assert_eq!(
        sns.state().lock().platform_applications.len(),
        PLATFORM_NAMES.len(),
        "a write must not register or drop an application"
    );
}

/// An attribute SNS maintains ITSELF is accepted and not written: the call is served —
/// the name is one SNS defines for an application, so there is no typo to report — and
/// the application still reports the value the service holds, while the entries of the
/// same map that a caller DOES write are applied.
#[tokio::test]
async fn an_attribute_the_service_maintains_is_accepted_and_not_written() {
    let sns = Sns::new();
    let flag = PLATFORM_APPLICATION_ENABLED_ATTRIBUTE.to_string();

    for platform in PLATFORM_NAMES {
        let arn =
            registered_application(&sns, REGION, &format!("owned-{platform}"), platform).await;
        let mut expected = held(&sns, &arn);

        // Rows: the flag alone under each of its two values, a value the flag never
        // takes, and the flag beside an attribute the caller does write.
        let rows: Vec<(String, Vec<(String, String)>)> = vec![
            (
                format!("{platform} / the flag alone, asking for it to be cleared"),
                vec![(flag.clone(), attribute_flag(false))],
            ),
            (
                format!("{platform} / the flag alone, under the value it already has"),
                vec![(flag.clone(), attribute_flag(true))],
            ),
            (
                format!("{platform} / a value the flag never takes"),
                vec![(flag.clone(), "whenever".to_string())],
            ),
            (
                format!("{platform} / the flag beside an attribute the caller writes"),
                vec![
                    (flag.clone(), attribute_flag(false)),
                    ("SuccessFeedbackSampleRate".to_string(), "40".to_string()),
                ],
            ),
        ];

        for (case, attributes) in &rows {
            written(&sns, case, &arn, attributes).await;
            expected.extend(applied(attributes));
            let record = held(&sns, &arn);
            assert_eq!(
                record, expected,
                "case {case:?}: expected the record to hold {expected:?}, got {record:?}"
            );
            let report = reported(&sns, case, &arn).await;
            assert_eq!(
                report.get(&flag).map(String::as_str),
                Some(attribute_flag(true).as_str()),
                "case {case:?}: the service's own value for the flag must survive the \
                 write, got {report:?}"
            );
        }
    }
}

/// A write acts on the ONE application its ARN names: another application of the same
/// account keeps every attribute it held.
#[tokio::test]
async fn a_write_touches_only_the_application_its_arn_names() {
    let sns = Sns::new();
    let one = registered_application(&sns, REGION, "isolated-one", "GCM").await;
    let other = registered_application(&sns, REGION, "isolated-other", "APNS").await;
    let untouched = held(&sns, &other);

    written(
        &sns,
        "a write addressed at one of two applications",
        &one,
        &[(
            "EventDeliveryFailure".to_string(),
            format!("arn:aws:sns:{REGION}:{ACCOUNT}:failures"),
        )],
    )
    .await;

    let report = reported(&sns, "the application that was written", &one).await;
    assert_eq!(
        report.get("EventDeliveryFailure").map(String::as_str),
        Some(format!("arn:aws:sns:{REGION}:{ACCOUNT}:failures").as_str()),
        "expected the written attribute to read back, got {report:?}"
    );
    assert_eq!(
        held(&sns, &other),
        untouched,
        "expected the other application's attributes to be left alone"
    );
}

/// An attribute name SNS does not define for a platform application is refused with the
/// member the map arrived in and the key that was not recognised, and the write persists
/// nothing — not even the entries of the same map that ARE defined.
#[tokio::test]
async fn an_attribute_name_the_service_does_not_define_is_refused() {
    let sns = Sns::new();
    let arn = registered_application(&sns, REGION, "closed-set", "GCM").await;
    let before = held(&sns, &arn);

    // Rows: a name of another resource's set, a name the service DERIVES rather than
    // taking, a misspelling, a defined name in another case, a defined name carrying
    // whitespace, two unrecognised names at once (the first by name is the one reported,
    // so one request always names the same key), and a defined name beside an
    // unrecognised one.
    type Row = (&'static str, Vec<(String, String)>, &'static str);
    let rows: Vec<Row> = vec![
        (
            "a name defined for a device endpoint, not an application",
            vec![("Token".to_string(), "device-token".to_string())],
            "Token",
        ),
        (
            "a name defined for a topic, not an application",
            vec![("DisplayName".to_string(), "push".to_string())],
            "DisplayName",
        ),
        (
            "a name the service derives from the credential material",
            vec![(
                "AppleCertificateExpiryDate".to_string(),
                "2030-01-01T00:00:00Z".to_string(),
            )],
            "AppleCertificateExpiryDate",
        ),
        (
            "a misspelled defined name",
            vec![(
                "EventEndpointDelete".to_string(),
                format!("arn:aws:sns:{REGION}:{ACCOUNT}:deleted"),
            )],
            "EventEndpointDelete",
        ),
        (
            "a defined name in another case",
            vec![("enabled".to_string(), attribute_flag(false))],
            "enabled",
        ),
        (
            "a defined name carrying trailing whitespace",
            vec![("SuccessFeedbackSampleRate ".to_string(), "10".to_string())],
            "SuccessFeedbackSampleRate ",
        ),
        (
            "two unrecognised names at once",
            vec![
                ("ZetaAttribute".to_string(), "1".to_string()),
                ("AlphaAttribute".to_string(), "2".to_string()),
            ],
            "AlphaAttribute",
        ),
        (
            "a defined name beside an unrecognised one",
            vec![
                ("SuccessFeedbackSampleRate".to_string(), "75".to_string()),
                ("NotAnAttribute".to_string(), "x".to_string()),
            ],
            "NotAnAttribute",
        ),
    ];

    for (case, attributes, unknown) in &rows {
        let answer = write(&sns, &arn, attributes).await;
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
        let record = held(&sns, &arn);
        assert_eq!(
            record, before,
            "case {case:?}: a refused write must persist nothing, got {record:?}"
        );
    }
}

/// An ARN that is not an ARN of the region this endpoint serves is bad input; an
/// ARN-shaped value the account holds no application record for is the modelled lookup
/// miss. Neither writes anything.
#[tokio::test]
async fn a_bad_arn_is_bad_input_and_an_unheld_one_is_the_lookup_miss() {
    let sns = Sns::new();
    let arn = registered_application(&sns, REGION, "resolved", "GCM").await;
    let topic = sns.topic("set-application-attributes-topic").await;
    let before = held(&sns, &arn);
    let entry = vec![(
        "EventEndpointUpdated".to_string(),
        format!("arn:aws:sns:{REGION}:{ACCOUNT}:updated"),
    )];

    // Not an ARN at all: judged on the value's shape, before any record is looked up.
    let bad: &[(&str, &str, &str)] = &[
        (
            "not an ARN at all",
            "resolved",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 1",
        ),
        (
            "too few ARN elements",
            "arn:aws:sns:us-east-1:000000000000",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must have at least 6 \
             elements, not 5",
        ),
        (
            "six elements that do not begin with the ARN prefix",
            "urn:aws:sns:us-east-1:000000000000:app/GCM/resolved",
            "Invalid parameter: PlatformApplicationArn Reason: An ARN must begin with 'arn:'",
        ),
        (
            "an application of another region",
            "arn:aws:sns:eu-west-1:000000000000:app/GCM/resolved",
            "Invalid parameter: PlatformApplicationArn",
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

    // Well-formed ARNs of this region that name no application record of this account:
    // every platform of the catalog under a name nobody registered, the registered name
    // under another platform, and ARNs of other resources. An ARN naming another ACCOUNT
    // is refused before the lookup, by the account-resolution refusal every ARN-addressed
    // call of this service shares (see the `arn_reference_refusals` suite).
    let mut missing: Vec<(String, String)> = PLATFORM_NAMES
        .iter()
        .map(|platform| {
            (
                format!("a well-formed {platform} application ARN nobody registered"),
                platform_application_arn(REGION, platform, "never-registered"),
            )
        })
        .collect();
    missing.push((
        "the registered name under another platform".to_string(),
        platform_application_arn(REGION, "ADM", "resolved"),
    ));
    missing.push((
        "an ARN that names a topic rather than an application".to_string(),
        topic,
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
            Some(PLATFORM_APPLICATION_DOES_NOT_EXIST),
            "case {case:?}: expected the fixed missing-application sentence, got {}",
            answer.body
        );
    }

    let record = held(&sns, &arn);
    assert_eq!(
        record, before,
        "a refused write must persist nothing, got {record:?}"
    );
}

/// Credential material the application's platform could not authenticate with is refused
/// and never stored; the material that platform's documented credential type names is
/// stored and withheld from every report.
#[tokio::test]
async fn credential_material_the_platform_could_not_use_is_refused() {
    let sns = Sns::new();
    let credential = PLATFORM_CREDENTIAL_ATTRIBUTE.to_string();

    // One row per platform of the documented catalog: material of the form that
    // platform's own credential clause names, and material of a form no clause of it
    // names. The Apple services and MPNS document a private KEY (PEM-encoded) and GCM an
    // API key or a JSON key file; the platforms whose clause names an opaque secret (a
    // client secret, a secret key) have no form to read, so only an empty value offers
    // them nothing to authenticate with.
    for platform in PLATFORM_NAMES {
        let arn =
            registered_application(&sns, REGION, &format!("credential-{platform}"), platform).await;
        let before = held(&sns, &arn);

        let usable = value_for(platform, &credential);
        let unusable: &[(&str, &str)] = match *platform {
            "APNS" | "APNS_SANDBOX" | "MPNS" => &[
                ("material carrying no PEM block", "not-a-private-key"),
                ("an empty value", ""),
            ],
            "GCM" => &[
                ("neither an API key nor a key file", "not-a-google-api-key"),
                ("an API key one character short", "AIzaSy0123456789_abcdefghijklmnopq-rst"),
                ("an empty value", ""),
            ],
            _ => &[("an empty value", ""), ("whitespace only", " ")],
        };

        for (case, value) in unusable {
            let case = format!("{platform} / {case}");
            let answer = write(&sns, &arn, &[(credential.clone(), (*value).to_string())]).await;
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
                Some(INVALID_PLATFORM_CREDENTIALS_REFUSAL),
                "case {case:?}: expected the credential refusal sentence, got {}",
                answer.body
            );
            let record = held(&sns, &arn);
            assert_eq!(
                record, before,
                "case {case:?}: a refused write must persist nothing, got {record:?}"
            );
        }

        let case = format!("{platform} / the material its credential type names");
        written(&sns, &case, &arn, &[(credential.clone(), usable.clone())]).await;
        assert_eq!(
            held(&sns, &arn).get(&credential),
            Some(&usable),
            "case {case:?}: expected the record to hold the credential it was given"
        );
        let report = reported(&sns, &case, &arn).await;
        assert!(
            !report.contains_key(&credential),
            "case {case:?}: a report must withhold the credential material, got {report:?}"
        );
    }
}
