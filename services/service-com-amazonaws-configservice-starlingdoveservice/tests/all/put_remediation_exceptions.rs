// SPDX-License-Identifier: CC0-1.0

//! `PutRemediationExceptions` and `DescribeRemediationExceptions`: the resources one
//! Config rule does not auto-remediate, written and read back.
//!
//! The rules pinned here: a put adds one exception per resource key it names and a
//! second put for the same rule and resource REVISES that one in place ("This API adds a
//! new exception or updates an existing exception for a specified resource with a
//! specified Config rule"), so every member a read reports is the one the LAST write
//! carried — an explanation the request omitted is absent rather than empty; a rule the
//! account holds no remediation configuration for takes no exception at all and is
//! refused 400 `InvalidParameterValueException`; a key that resolves to no resource the
//! rule has EVALUATED is refused 400 `InvalidParameterValueException` naming the keys it
//! could not find; a resource the rule's own verdict does not place as NON_COMPLIANT
//! takes no exception at all and comes back in `FailedBatches`; a key naming no resource
//! is refused 400 `InvalidParameterValueException`; and a read of every exception a rule
//! holds is paged, while a read that names its resources in batch is not.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of inputs
//! rather than one recorded call: the rows differ along how many keys one request
//! carries, along the resource type and id each key names, along whether the request
//! carries an explanation and an expiration time at all, along the published length
//! bounds of the explanation, and along which verdict the rule already recorded for the
//! resource.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{
    managed_rule, with_recorder_and_channel, with_verdicts, Emulator,
};
use crate::put_evaluations::{custom_lambda_rule, result_token};

/// Resource types a key names, from `smithy_query(query="shape", shape="ResourceType")`;
/// `AWS::::Account` is the account-level type Config uses for the account itself
/// (`ExternalEvaluation$ComplianceResourceType`), which that value set does not list.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const ACCOUNT: &str = "AWS::::Account";

/// The rule the exceptions of every case belong to.
const RULE: &str = "encrypted-volumes";

/// Another rule, whose exceptions no read of [`RULE`] may report.
const OTHER_RULE: &str = "versioned-buckets";

/// A rule the account holds with NO remediation configuration, which is what makes it a
/// rule no exception can be placed under.
const PLAIN_RULE: &str = "unremediated-volumes";

/// Every `ComplianceType` a delivered verdict may carry, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`, `NON_COMPLIANT`,
/// `NOT_APPLICABLE` (the value set also lists `INSUFFICIENT_DATA`, which Config does not
/// accept from a delivery).
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The published length bounds of the explanation an exception carries
/// (`PutRemediationExceptionsRequest$Message`, target `StringWithCharLimit1024`,
/// length 1..1024).
const MESSAGE_MIN_LENGTH: usize = 1;
/// See [`MESSAGE_MIN_LENGTH`].
const MESSAGE_MAX_LENGTH: usize = 1024;

/// An emulator holding a recorder, a delivery channel, the rules the cases write
/// exceptions for and the remediation configuration each of those rules needs: an
/// exception exempts a resource from the remediation a rule's configuration runs, so a
/// rule holding none takes no exception at all.
async fn emulator_with_rules() -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule(RULE)).await;
    emulator.ok("PutConfigRule", &managed_rule(OTHER_RULE)).await;
    with_remediation_configuration(&emulator, RULE).await;
    with_remediation_configuration(&emulator, OTHER_RULE).await;
    emulator
}

/// Give `rule` the remediation configuration a remediation exception exempts a resource
/// from: the SSM document a `RemediationConfiguration` names as its target, which is all
/// the model requires beside the rule (`RemediationConfiguration`: ConfigRuleName,
/// TargetType and TargetId are its required members).
async fn with_remediation_configuration(emulator: &Emulator, rule: &str) {
    emulator
        .ok(
            "PutRemediationConfigurations",
            &json!({
                "RemediationConfigurations": [{
                    "ConfigRuleName": rule,
                    "TargetType": "SSM_DOCUMENT",
                    "TargetId": "AWS-PublishSNSNotification",
                }],
            }),
        )
        .await;
}

/// A resource key naming both members, which is what identifies a resource in Config.
fn key(resource_type: &str, resource_id: &str) -> Value {
    json!({ "ResourceType": resource_type, "ResourceId": resource_id })
}

/// Record `rule`'s own NON_COMPLIANT verdict about every resource `keys` names.
///
/// A put resolves each key against the resources the rule has EVALUATED, and generates
/// the exception only for one the rule finds NON_COMPLIANT, so a case about an accepted
/// put has to say that the rule found the resources it names.
fn found_non_compliant(emulator: &Emulator, rule: &str, keys: &[Value]) {
    let resources: Vec<(String, String)> = keys
        .iter()
        .map(|named| {
            (
                named["ResourceType"].as_str().unwrap_or_default().to_string(),
                named["ResourceId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .filter(|(resource_type, resource_id)| {
            !resource_type.trim().is_empty() && !resource_id.trim().is_empty()
        })
        .collect();
    with_verdicts(emulator, rule, NON_COMPLIANT, &resources);
}

/// The `PutRemediationExceptions` body for `rule` over `keys`, with the explanation and
/// expiration time the case carries (each omitted when the case carries none).
fn put(rule: &str, keys: &[Value], message: Option<&str>, expires_at: Option<u64>) -> Value {
    let mut body = json!({ "ConfigRuleName": rule, "ResourceKeys": keys });
    if let Some(message) = message {
        body["Message"] = json!(message);
    }
    if let Some(expires_at) = expires_at {
        body["ExpirationTime"] = json!(expires_at);
    }
    body
}

/// What one reported exception carries: the resource it exempts, its explanation and the
/// instant it falls due for deletion, as epoch seconds.
type Reported = (String, String, Option<String>, Option<i64>);

/// One reported `RemediationException`, as [`Reported`], with its rule checked to be the
/// one the read asked about.
fn reported(case: &str, rule: &str, exception: &Value) -> Reported {
    assert_eq!(
        exception["ConfigRuleName"].as_str(),
        Some(rule),
        "case {case:?}: every reported exception belongs to {rule:?}, got {exception}"
    );
    (
        exception["ResourceType"].as_str().unwrap_or_default().to_string(),
        exception["ResourceId"].as_str().unwrap_or_default().to_string(),
        exception["Message"].as_str().map(str::to_string),
        exception["ExpirationTime"].as_f64().map(|at| at as i64),
    )
}

/// Every exception `rule` holds, read over the wire page by page with the `Limit` the
/// case asks for (`None`: whatever the operation serves by default).
async fn described(case: &str, emulator: &Emulator, rule: &str, limit: Option<i64>) -> Vec<Reported> {
    let mut all = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "ConfigRuleName": rule });
        if let Some(limit) = limit {
            request["Limit"] = json!(limit);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let body = emulator
            .ok("DescribeRemediationExceptions", &request)
            .await;
        let page = body["RemediationExceptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected RemediationExceptions, got {body}"))
            .clone();
        let page_size = page.len();
        all.extend(page.iter().map(|exception| reported(case, rule, exception)));
        // A page never carries more than the size the request asked for.
        if let Some(limit) = limit.filter(|limit| *limit > 0) {
            let allowed = usize::try_from(limit).unwrap_or(usize::MAX).min(25);
            assert!(
                page_size <= allowed,
                "case {case:?}: expected at most {allowed} exceptions on a page, got {page_size}"
            );
        }
        next_token = body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            page_size > 0,
            "case {case:?}: a page that hands out a NextToken reported something, got {body}"
        );
    }
    all
}

/// The exceptions `rule` holds for exactly `keys`, read in batch — the form the
/// operation documents `Limit` and `NextToken` as not applying to.
async fn described_in_batch(
    case: &str,
    emulator: &Emulator,
    rule: &str,
    keys: &[Value],
    limit: Option<i64>,
) -> (Vec<Reported>, Option<String>) {
    let mut request = json!({ "ConfigRuleName": rule, "ResourceKeys": keys });
    if let Some(limit) = limit {
        request["Limit"] = json!(limit);
    }
    let body = emulator
        .ok("DescribeRemediationExceptions", &request)
        .await;
    let page = body["RemediationExceptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected RemediationExceptions, got {body}"))
        .iter()
        .map(|exception| reported(case, rule, exception))
        .collect();
    (page, body["NextToken"].as_str().map(str::to_string))
}

/// The `(resourceType, resourceId, FailureMessage)` triples one `FailedBatches` reports.
fn failed_items(case: &str, body: &Value) -> Vec<(String, String, String)> {
    body["FailedBatches"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected FailedBatches, got {body}"))
        .iter()
        .flat_map(|batch| {
            let message = batch["FailureMessage"].as_str().unwrap_or_default().to_string();
            batch["FailedItems"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |item| {
                    (
                        item["ResourceType"].as_str().unwrap_or_default().to_string(),
                        item["ResourceId"].as_str().unwrap_or_default().to_string(),
                        message.clone(),
                    )
                })
        })
        .collect()
}

/// An instant far enough ahead that no case's exception falls due while it runs.
const FAR_FUTURE: u64 = 4_000_000_000;

/// One row of the writing table: what it is called, the keys the request names, the
/// explanation it carries and the instant its exceptions fall due for deletion.
type PutCase = (&'static str, Vec<Value>, Option<String>, Option<u64>);

#[tokio::test]
async fn a_put_writes_an_exception_per_resource_key_and_a_read_reports_it() {
    // Rows differ along what the rule generalises over: how many keys one request names,
    // the resource type and id each one carries, whether the request carries an
    // explanation at all, how long that explanation is (both published bounds), and
    // whether the exceptions fall due for deletion.
    let cases: [PutCase; 7] = [
        (
            "one key with an explanation and no expiration",
            vec![key(EC2_VOLUME, "vol-0001")],
            Some("waiting on the owning team".to_string()),
            None,
        ),
        (
            "one key with no explanation at all",
            vec![key(EC2_VOLUME, "vol-0001")],
            None,
            None,
        ),
        (
            "one key with an explanation and an expiration",
            vec![key(S3_BUCKET, "reports")],
            Some("migrating the bucket".to_string()),
            Some(FAR_FUTURE),
        ),
        (
            "an expiration and no explanation",
            vec![key(S3_BUCKET, "reports")],
            None,
            Some(FAR_FUTURE),
        ),
        (
            "three keys across two resource types and the account itself",
            vec![
                key(EC2_VOLUME, "vol-0001"),
                key(S3_BUCKET, "reports"),
                key(ACCOUNT, "000000000000"),
            ],
            Some("tracked in the migration plan".to_string()),
            Some(FAR_FUTURE),
        ),
        (
            "an explanation of the published minimum length",
            vec![key(EC2_VOLUME, "vol-0002")],
            Some("x".repeat(MESSAGE_MIN_LENGTH)),
            None,
        ),
        (
            "an explanation of the published maximum length",
            vec![key(EC2_VOLUME, "vol-0002")],
            Some("y".repeat(MESSAGE_MAX_LENGTH)),
            None,
        ),
    ];

    for (case, keys, message, expires_at) in cases {
        let emulator = emulator_with_rules().await;
        found_non_compliant(&emulator, RULE, &keys);
        assert!(
            described(case, &emulator, RULE, None).await.is_empty(),
            "case {case:?}: a rule nothing has excepted holds no exception"
        );

        let answered = emulator
            .ok(
                "PutRemediationExceptions",
                &put(RULE, &keys, message.as_deref(), expires_at),
            )
            .await;
        assert_eq!(
            failed_items(case, &answered),
            Vec::new(),
            "case {case:?}: every key the request named took an exception, got {answered}"
        );

        // What the read must report: one exception per key, carrying the explanation and
        // the expiration the request carried — and NOTHING where the request carried
        // nothing, which is what tells an unexplained exception from an explained one.
        let mut expected: Vec<Reported> = keys
            .iter()
            .map(|named| {
                (
                    named["ResourceType"].as_str().unwrap_or_default().to_string(),
                    named["ResourceId"].as_str().unwrap_or_default().to_string(),
                    message.clone(),
                    expires_at.map(|at| at as i64),
                )
            })
            .collect();
        expected.sort();

        let mut seen = described(case, &emulator, RULE, None).await;
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the exceptions {expected:?} read back, got {seen:?}"
        );

        // A read in batch reports the same exceptions for the keys it names.
        let (batched, batch_token) =
            described_in_batch(case, &emulator, RULE, &keys, Some(1)).await;
        let mut batched_sorted = batched.clone();
        batched_sorted.sort();
        assert_eq!(
            (batched_sorted, batch_token),
            (expected.clone(), None),
            "case {case:?}: a batch read reports the exceptions of the keys it names, \
             whatever Limit it carries"
        );

        // The exceptions belong to ONE rule: another rule holds none of them.
        assert_eq!(
            described(case, &emulator, OTHER_RULE, None).await,
            Vec::new(),
            "case {case:?}: a put under {RULE:?} writes nothing under {OTHER_RULE:?}"
        );
    }
}

/// One row of the revising table: what it is called, and the explanation and expiration
/// of the first and of the second write.
type ReviseCase = (
    &'static str,
    (Option<&'static str>, Option<u64>),
    (Option<&'static str>, Option<u64>),
);

#[tokio::test]
async fn a_second_put_for_the_same_resource_revises_the_exception_it_holds() {
    // Rows differ along what the second write CHANGES: the explanation, the expiration,
    // both, or the explanation away again — the member the second request omits.
    let cases: [ReviseCase; 5] = [
        (
            "the explanation is replaced",
            (Some("waiting on the owning team"), None),
            (Some("the owning team answered"), None),
        ),
        (
            "an explanation is added where there was none",
            (None, None),
            (Some("now explained"), None),
        ),
        (
            "the explanation is dropped again",
            (Some("waiting on the owning team"), None),
            (None, None),
        ),
        (
            "the expiration is replaced",
            (Some("waiting"), Some(FAR_FUTURE)),
            (Some("waiting"), Some(FAR_FUTURE + 86_400)),
        ),
        (
            "the expiration is dropped again",
            (Some("waiting"), Some(FAR_FUTURE)),
            (None, None),
        ),
    ];

    for (case, first, second) in cases {
        let emulator = emulator_with_rules().await;
        let keys = [key(EC2_VOLUME, "vol-0001")];
        found_non_compliant(&emulator, RULE, &keys);

        emulator
            .ok("PutRemediationExceptions", &put(RULE, &keys, first.0, first.1))
            .await;
        let answered = emulator
            .ok(
                "PutRemediationExceptions",
                &put(RULE, &keys, second.0, second.1),
            )
            .await;
        assert_eq!(
            failed_items(case, &answered),
            Vec::new(),
            "case {case:?}: revising an exception the rule holds is accepted, got {answered}"
        );

        let seen = described(case, &emulator, RULE, None).await;
        assert_eq!(
            seen,
            vec![(
                EC2_VOLUME.to_string(),
                "vol-0001".to_string(),
                second.0.map(str::to_string),
                second.1.map(|at| at as i64),
            )],
            "case {case:?}: the rule holds ONE exception for the resource, carrying what \
             the second write said, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_key_that_names_no_resource_is_refused_and_writes_nothing() {
    // Rows differ along WHICH member of the key names no resource: neither, one of the
    // two, or one that is present but blank — and along whether the request carries a
    // well-formed key beside it.
    let cases: [(&str, Vec<Value>); 6] = [
        ("a key naming neither member", vec![json!({})]),
        (
            "a key naming only the resource type",
            vec![json!({ "ResourceType": EC2_VOLUME })],
        ),
        (
            "a key naming only the resource id",
            vec![json!({ "ResourceId": "vol-0001" })],
        ),
        (
            "a resource id that is blank",
            vec![key(EC2_VOLUME, "   ")],
        ),
        (
            "a resource type that is blank",
            vec![key("  ", "vol-0001")],
        ),
        (
            "a well-formed key beside one that names nothing",
            vec![key(EC2_VOLUME, "vol-0001"), json!({})],
        ),
    ];

    for (case, keys) in cases {
        let emulator = emulator_with_rules().await;

        let (status, parsed) = emulator
            .call(
                "PutRemediationExceptions",
                &put(RULE, &keys, Some("waiting"), None),
            )
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        assert_eq!(
            (status, code),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected 400 InvalidParameterValueException, got {status} {parsed}"
        );

        // A refused put writes NOTHING — not even the keys that did name a resource.
        assert_eq!(
            described(case, &emulator, RULE, None).await,
            Vec::new(),
            "case {case:?}: a refused put leaves the rule holding no exception"
        );
    }
}

/// What a put answers for the verdict the rule holds about the resource: the exception is
/// generated, the key comes back in `FailedBatches` because the rule does not place the
/// resource as NON_COMPLIANT, or the whole request is refused because the key resolves to
/// no resource the rule has evaluated at all.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Generated,
    FailedBatch,
    Refused,
}

/// One row of the verdict table: what it is called, the verdict the rule already
/// recorded — the resource it is about and the compliance it reports, if any — and what
/// the put then answers.
type VerdictCase = (&'static str, Option<(&'static str, &'static str)>, Outcome);

#[tokio::test]
async fn an_exception_is_generated_only_where_the_rule_does_not_place_the_resource_as_compliant() {
    // Rows differ along the verdict the rule already recorded, and the resource it is
    // about: each `ComplianceType` a delivered verdict may carry (`PutEvaluations`
    // accepts COMPLIANT, NON_COMPLIANT and NOT_APPLICABLE), no verdict at all, and a
    // verdict about ANOTHER resource. The exception is always placed on `vol-0001`.
    let cases: [VerdictCase; 5] = [
        ("no verdict recorded at all", None, Outcome::Refused),
        (
            "the rule evaluated it NON_COMPLIANT",
            Some(("vol-0001", NON_COMPLIANT)),
            Outcome::Generated,
        ),
        (
            "the rule evaluated it COMPLIANT",
            Some(("vol-0001", COMPLIANT)),
            Outcome::FailedBatch,
        ),
        (
            "the rule does not apply to it",
            Some(("vol-0001", NOT_APPLICABLE)),
            Outcome::FailedBatch,
        ),
        (
            "the rule placed another resource as compliant",
            Some(("vol-9999", COMPLIANT)),
            Outcome::Refused,
        ),
    ];

    for (case, verdict, expected) in cases {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        emulator
            .ok("PutConfigRule", &custom_lambda_rule(RULE))
            .await;
        with_remediation_configuration(&emulator, RULE).await;
        if let Some((evaluated, compliance_type)) = verdict {
            emulator
                .ok(
                    "PutEvaluations",
                    &json!({
                        "ResultToken": result_token(&emulator, RULE),
                        "Evaluations": [{
                            "ComplianceResourceType": EC2_VOLUME,
                            "ComplianceResourceId": evaluated,
                            "ComplianceType": compliance_type,
                            "OrderingTimestamp": 1_700_000_000u64,
                        }],
                    }),
                )
                .await;
        }

        let keys = [key(EC2_VOLUME, "vol-0001")];
        let request = put(RULE, &keys, Some("waiting"), None);

        if expected == Outcome::Refused {
            // A resource the rule has not evaluated resolves to nothing, so the request
            // is refused and the rule holds no exception.
            let (status, parsed) = emulator.call("PutRemediationExceptions", &request).await;
            let code = parsed["__type"]
                .as_str()
                .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
                .unwrap_or("<none>");
            assert_eq!(
                (status, code, parsed["message"].as_str()),
                (
                    400,
                    "InvalidParameterValueException",
                    Some(format!("Could not find resources: {RULE}: ({EC2_VOLUME}, vol-0001)").as_str()),
                ),
                "case {case:?}: expected the key to resolve to no evaluated resource, got \
                 {status} {parsed}"
            );
            assert_eq!(
                described(case, &emulator, RULE, None).await,
                Vec::new(),
                "case {case:?}: a refused put leaves the rule holding no exception"
            );
            continue;
        }

        let answered = emulator.ok("PutRemediationExceptions", &request).await;
        let failed = failed_items(case, &answered);
        let held = described(case, &emulator, RULE, None).await;

        if expected == Outcome::Generated {
            assert_eq!(
                failed,
                Vec::new(),
                "case {case:?}: the exception was generated, so nothing failed, got {answered}"
            );
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected the rule to hold the exception, got {held:?}"
            );
        } else {
            assert_eq!(
                held,
                Vec::new(),
                "case {case:?}: a resource the rule does not place as non-compliant takes no \
                 exception, got {held:?}"
            );
            let reported_keys: Vec<(String, String)> = failed
                .iter()
                .map(|(resource_type, resource_id, _)| {
                    (resource_type.clone(), resource_id.clone())
                })
                .collect();
            assert_eq!(
                reported_keys,
                vec![(EC2_VOLUME.to_string(), "vol-0001".to_string())],
                "case {case:?}: the key comes back in FailedBatches, got {answered}"
            );
            assert!(
                failed
                    .iter()
                    .all(|(_, _, message)| message.contains(RULE) && !message.is_empty()),
                "case {case:?}: the failed batch explains why, got {answered}"
            );
        }
    }
}

#[tokio::test]
async fn a_rule_with_no_remediation_configuration_takes_no_exception() {
    // An exception exempts a resource from the remediation ONE rule's remediation
    // configuration would run, so a rule holding none has no remediation to exempt it
    // from and the whole request is refused. Rows differ along WHY the request names no
    // rule holding a remediation configuration: a rule the account holds but gave no
    // remediation configuration, a name nothing created, a name that differs from a held
    // rule only in case, a name that differs by surrounding space, and how many keys the
    // request names under it.
    let cases: [(&str, &str, Vec<Value>); 5] = [
        (
            "a held rule with no remediation configuration",
            PLAIN_RULE,
            vec![key(EC2_VOLUME, "vol-0001")],
        ),
        (
            "a rule nothing created",
            "no-such-rule",
            vec![key(EC2_VOLUME, "vol-0001")],
        ),
        (
            "a held rule's name in another case",
            "Encrypted-Volumes",
            vec![key(EC2_VOLUME, "vol-0001")],
        ),
        (
            "a held rule's name with trailing space",
            "encrypted-volumes ",
            vec![key(EC2_VOLUME, "vol-0001")],
        ),
        (
            "three keys under a rule nothing created",
            "no-such-rule",
            vec![
                key(EC2_VOLUME, "vol-0001"),
                key(S3_BUCKET, "reports"),
                key(ACCOUNT, "000000000000"),
            ],
        ),
    ];

    for (case, rule, keys) in cases {
        let emulator = emulator_with_rules().await;
        // The rule of the first row exists and holds no remediation configuration.
        emulator.ok("PutConfigRule", &managed_rule(PLAIN_RULE)).await;

        let (status, parsed) = emulator
            .call(
                "PutRemediationExceptions",
                &put(rule, &keys, Some("waiting"), None),
            )
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        let message = parsed["message"].as_str().unwrap_or_default();
        assert_eq!(
            (status, code, message),
            (
                400,
                "InvalidParameterValueException",
                format!("No RemediationConfiguration found for {rule}").as_str(),
            ),
            "case {case:?}: expected the remediation-configuration precondition to be \
             reported, got {status} {code} {message:?}"
        );

        assert_eq!(
            described(case, &emulator, rule, None).await,
            Vec::new(),
            "case {case:?}: a rule with no remediation configuration holds no exception"
        );
        assert_eq!(
            described(case, &emulator, RULE, None).await,
            Vec::new(),
            "case {case:?}: nothing was written under the rule that does hold one"
        );
    }
}

#[tokio::test]
async fn a_read_of_every_exception_a_rule_holds_is_paged() {
    // One put naming many keys, then the same report read at several page sizes: the
    // rows differ along the `Limit` the read asks for — none at all, the smallest, one
    // in between, the page the operation defaults to, and the published maximum of
    // `Limit`, which is larger than a page of this report can carry.
    let written = 30usize;
    let keys: Vec<Value> = (0..written)
        .map(|index| key(EC2_VOLUME, &format!("vol-{index:04}")))
        .collect();

    for limit in [None, Some(0), Some(1), Some(7), Some(25), Some(100)] {
        let case = format!("Limit {limit:?}");
        let case = case.as_str();
        let emulator = emulator_with_rules().await;
        found_non_compliant(&emulator, RULE, &keys);
        let answered = emulator
            .ok(
                "PutRemediationExceptions",
                &put(RULE, &keys, Some("bulk exception"), None),
            )
            .await;
        assert_eq!(
            failed_items(case, &answered),
            Vec::new(),
            "case {case:?}: every key of the bulk put took an exception, got {answered}"
        );

        let seen = described(case, &emulator, RULE, limit).await;
        let ids: Vec<String> = seen
            .iter()
            .map(|(_, resource_id, _, _)| resource_id.clone())
            .collect();
        let expected: Vec<String> = (0..written).map(|index| format!("vol-{index:04}")).collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: every exception the rule holds is reported exactly once, \
             across pages, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_report_did_not_issue_is_refused() {
    // Rows differ along HOW the token is malformed: not base64 at all, base64 of
    // something this report never encoded, and the empty string.
    let cases: [(&str, &str); 4] = [
        ("not base64 at all", "!!!not-a-token!!!"),
        ("base64 of a value this report never encoded", "dm9sLTAwMDE="),
        ("base64 of a length prefix that overruns", "OTk6dm9s"),
        ("the empty string", ""),
    ];

    for (case, token) in cases {
        let emulator = emulator_with_rules().await;
        found_non_compliant(&emulator, RULE, &[key(EC2_VOLUME, "vol-0001")]);
        emulator
            .ok(
                "PutRemediationExceptions",
                &put(
                    RULE,
                    &[key(EC2_VOLUME, "vol-0001")],
                    Some("waiting"),
                    None,
                ),
            )
            .await;

        let (status, parsed) = emulator
            .call(
                "DescribeRemediationExceptions",
                &json!({ "ConfigRuleName": RULE, "NextToken": token }),
            )
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        assert_eq!(
            (status, code),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {status} {parsed}"
        );
    }
}

#[tokio::test]
async fn an_exception_the_clock_has_taken_past_its_expiration_is_reported_by_no_read() {
    // Rows differ along how far past its expiration the clock already is, and along
    // whether the exception carries an explanation.
    let cases: [(&str, u64, Option<&str>); 3] = [
        ("an expiration one second after the epoch", 1, None),
        ("an expiration in 2020", 1_600_000_000, Some("waiting")),
        ("an expiration in 2023", 1_700_000_000, Some("waiting")),
    ];

    for (case, expires_at, message) in cases {
        let emulator = emulator_with_rules().await;
        let keys = [key(EC2_VOLUME, "vol-0001"), key(S3_BUCKET, "reports")];
        found_non_compliant(&emulator, RULE, &keys);
        emulator
            .ok(
                "PutRemediationExceptions",
                &put(RULE, &keys[..1], message, Some(expires_at)),
            )
            .await;
        // One exception with no expiration at all, so the read is answered by a rule
        // that does hold something: the expired one is the only one missing.
        emulator
            .ok("PutRemediationExceptions", &put(RULE, &keys[1..], message, None))
            .await;

        let seen = described(case, &emulator, RULE, None).await;
        assert_eq!(
            seen,
            vec![(
                S3_BUCKET.to_string(),
                "reports".to_string(),
                message.map(str::to_string),
                None,
            )],
            "case {case:?}: the exception whose expiration the clock has passed is gone, \
             got {seen:?}"
        );
    }
}
