// SPDX-License-Identifier: CC0-1.0

//! `UpdateEnabledBaseline` and `GetEnabledBaseline`, end to end over HTTP.
//!
//! Drives the calls through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * `UpdateEnabledBaseline` starts an asynchronous `UPDATE_ENABLED_BASELINE` operation
//!   against the `EnabledBaseline` its identifier names, and once the clock has made that
//!   operation due the enablement's applied `baselineVersion` is the requested one and its
//!   applied parameters are the requested list when the request carried a `parameters`
//!   member and the ones already applied when it did not — for any version the enablement's
//!   baseline offers and any list of the parameters that baseline declares;
//! * while that operation is in flight the enablement reads as `UNDER_CHANGE` with the new
//!   operation as its `lastOperationIdentifier` and the values it had still applied, so
//!   the update really is asynchronous;
//! * an identifier no `EnabledBaseline` matches is `ResourceNotFoundException` (404) from
//!   both calls, and an update that overlaps an operation still changing the enablement is
//!   `ConflictException` (409);
//! * `GetEnabledBaseline` is reporting-only: repeating it changes nothing a later caller
//!   sees.
//!
//! The enum values come from
//! `smithy_query(query="shape", shape="BaselineOperationType,EnablementStatus")` on this
//! crate's `smithy-model/`: `ENABLE_BASELINE` | `DISABLE_BASELINE` |
//! `UPDATE_ENABLED_BASELINE` | `RESET_ENABLED_BASELINE`, and `SUCCEEDED` | `FAILED` |
//! `UNDER_CHANGE`. `BaselineVersion`'s `@pattern ^\d+(?:\.\d+){0,2}$` and `@length` 1..10
//! come from `smithy_query(query="operation", operation="UpdateEnabledBaseline")`.

use crate::landing_zone_lifecycle::{
    call, is_operation_identifier, past_the_settle_span, set_up_emulator, Answer,
};

use smithy_server_runtime::MultiServiceRouter;

/// One row of the update table.
#[derive(Debug)]
struct UpdateCase {
    case: &'static str,
    /// The version `EnableBaseline` applies first.
    initial_version: &'static str,
    /// The `parameters` member `EnableBaseline` is given; `None` omits it.
    initial_parameters: Option<serde_json::Value>,
    /// The version the update moves the enablement to.
    new_version: &'static str,
    /// The `parameters` member the update is given; `None` omits it, which must leave the
    /// applied parameters as they are.
    new_parameters: Option<serde_json::Value>,
}

impl UpdateCase {
    /// The parameters the enablement must report once the update is due, as the rule
    /// states it: the update's list when the request carried one, else the ones already
    /// applied, else none at all.
    fn expected_parameters(&self) -> serde_json::Value {
        self.new_parameters
            .clone()
            .or_else(|| self.initial_parameters.clone())
            .unwrap_or_else(|| serde_json::json!([]))
    }
}

/// The update table: rows differ along what the rule generalises over — the direction the
/// version moves (to a newer offered version, to an older one, and to the one already
/// applied), the `parameters` member of the update (absent, empty, and the declared key
/// with several `Document` kinds — the value is "a document of any type") and the
/// parameters already applied (absent, empty, present). The versions are the ones
/// `AWSControlTowerBaseline` offers and the key is the one it declares, both read off
/// `service_..._awscontroltowerapis::state::BASELINE_CATALOG`.
/// `enabledBaselineIdentifier` is always the full ARN: the model constrains it to `Arn`
/// (pattern `^arn:aws[0-9a-zA-Z_\-:\/]+$`, length 20..2048) and documents it "in ARN
/// format", so a bare resource id is refused at the boundary before any handler runs.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a version-only update, with no parameters applied before or asked for",
            initial_version: "1.0",
            initial_parameters: None,
            new_version: "4.0",
            new_parameters: None,
        },
        UpdateCase {
            case: "a version-only update must not clear the parameters already applied",
            initial_version: "2.0",
            initial_parameters: Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": "true" }
            ])),
            new_version: "3.0",
            new_parameters: None,
        },
        UpdateCase {
            case: "an empty parameter list clears the parameters already applied",
            initial_version: "3.0",
            initial_parameters: Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": true }
            ])),
            new_version: "4.0",
            new_parameters: Some(serde_json::json!([])),
        },
        UpdateCase {
            case: "a parameter of another Document kind replaces the applied one",
            initial_version: "1.0",
            initial_parameters: Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": false }
            ])),
            new_version: "2.0",
            new_parameters: Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline",
                  "value": { "enabled": true, "regions": ["us-west-2"] } }
            ])),
        },
        UpdateCase {
            case: "an update back to the oldest version the baseline offers",
            initial_version: "4.0",
            initial_parameters: Some(serde_json::json!([])),
            new_version: "1.0",
            new_parameters: Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": "false" }
            ])),
        },
        UpdateCase {
            case: "an update to the version already applied",
            initial_version: "2.0",
            initial_parameters: None,
            new_version: "2.0",
            new_parameters: None,
        },
    ]
}

/// POST `/enable-baseline`, omitting `parameters` when the row does.
async fn enable(
    router: &MultiServiceRouter,
    baseline: &str,
    target: &str,
    version: &str,
    parameters: Option<serde_json::Value>,
) -> Answer {
    let mut body = serde_json::json!({
        "baselineIdentifier": baseline,
        "baselineVersion": version,
        "targetIdentifier": target,
    });
    if let Some(parameters) = parameters {
        body["parameters"] = parameters;
    }
    call(router, "/enable-baseline", body).await
}

/// POST `/update-enabled-baseline`, omitting `parameters` when the caller does.
async fn update(
    router: &MultiServiceRouter,
    identifier: &str,
    version: &str,
    parameters: Option<serde_json::Value>,
) -> Answer {
    let mut body = serde_json::json!({
        "enabledBaselineIdentifier": identifier,
        "baselineVersion": version,
    });
    if let Some(parameters) = parameters {
        body["parameters"] = parameters;
    }
    call(router, "/update-enabled-baseline", body).await
}

/// POST `/get-enabled-baseline` and hand back the answer plus its
/// `enabledBaselineDetails` member, which is what every assertion below reads.
async fn get_enabled_baseline(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (Answer, serde_json::Value) {
    let answer = call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await;
    let details = answer.get("enabledBaselineDetails").clone();
    (answer, details)
}

/// POST `/get-baseline-operation` and hand back its `baselineOperation` member.
///
/// Shared with `crate::enabled_baseline_update_validation`, which reads back the operation
/// an accepted update mints, so the two suites cannot disagree about that call.
pub(crate) async fn get_operation(
    router: &MultiServiceRouter,
    operation_identifier: &str,
) -> (Answer, serde_json::Value) {
    let answer = call(
        router,
        "/get-baseline-operation",
        serde_json::json!({ "operationIdentifier": operation_identifier }),
    )
    .await;
    let operation = answer.get("baselineOperation").clone();
    (answer, operation)
}

/// A member of a JSON object, as a `&str`.
fn member<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// `statusSummary.status` of an `EnabledBaselineDetails`.
fn status(details: &serde_json::Value) -> Option<&str> {
    details
        .get("statusSummary")
        .and_then(|summary| member(summary, "status"))
}

/// `statusSummary.lastOperationIdentifier` of an `EnabledBaselineDetails`.
fn last_operation(details: &serde_json::Value) -> Option<&str> {
    details
        .get("statusSummary")
        .and_then(|summary| member(summary, "lastOperationIdentifier"))
}

/// The applied `parameters` of an `EnabledBaselineDetails`, as a list — an absent member
/// reads as the empty list, which is what "no parameters are applied" means.
fn parameters(details: &serde_json::Value) -> serde_json::Value {
    details
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]))
}

/// RULE: `UpdateEnabledBaseline` starts an asynchronous `UPDATE_ENABLED_BASELINE`
/// operation against the enablement its identifier names; while that operation is in
/// flight the enablement is `UNDER_CHANGE` with the values it had still applied, and once
/// the clock has made the operation due the applied version is the requested one and the
/// applied parameters are the requested list when the request carried one and the ones
/// already applied when it did not.
#[tokio::test]
async fn an_update_applies_the_requested_version_and_parameters() {
    let baseline = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";
    let router = set_up_emulator().await;

    // Every row is its own enablement on its own target, so the rows do not interact.
    let mut rows: Vec<(UpdateCase, String, String)> = Vec::new();
    for (index, row) in update_cases().into_iter().enumerate() {
        let target =
            format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-up11-{index:04}");
        let enabled = enable(
            &router,
            baseline,
            &target,
            row.initial_version,
            row.initial_parameters.clone(),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: enable expected 200, got {} body {:?}",
            row.case, enabled.status, enabled.body
        );
        let arn = enabled.string("arn");
        rows.push((row, arn, target));
    }

    // The enables are asynchronous too, so wait for them before updating: an enablement
    // still being applied refuses an update as a conflict (asserted separately below).
    tokio::time::sleep(past_the_settle_span()).await;

    let mut started: Vec<(UpdateCase, String, String)> = Vec::new();
    for (row, arn, _target) in rows {
        let identifier = arn.clone();

        let updated = update(
            &router,
            &identifier,
            row.new_version,
            row.new_parameters.clone(),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: update expected 200, got {} body {:?}",
            row.case, updated.status, updated.body
        );
        let operation_identifier = updated.string("operationIdentifier");
        assert!(
            is_operation_identifier(&operation_identifier),
            "case {:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation_identifier:?}",
            row.case
        );

        // The update "starts an asynchronous operation", so a caller acting straight
        // after the call sees the transient state: the operation in flight, the
        // enablement changing, and the OLD values still applied.
        let (resolved, operation) = get_operation(&router, &operation_identifier).await;
        assert_eq!(
            resolved.status, 200,
            "case {:?}: the returned identifier must resolve, got {} body {:?}",
            row.case, resolved.status, resolved.body
        );
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some("UPDATE_ENABLED_BASELINE"), Some("IN_PROGRESS")),
            "case {:?}: expected UPDATE_ENABLED_BASELINE IN_PROGRESS, got {operation:?}",
            row.case
        );

        let (in_flight, details) = get_enabled_baseline(&router, &identifier).await;
        assert_eq!(
            in_flight.status, 200,
            "case {:?}: get-enabled-baseline expected 200, got {} body {:?}",
            row.case, in_flight.status, in_flight.body
        );
        assert_eq!(
            status(&details),
            Some("UNDER_CHANGE"),
            "case {:?}: expected UNDER_CHANGE while the update is in flight, got \
             {details:?}",
            row.case
        );
        assert_eq!(
            last_operation(&details),
            Some(operation_identifier.as_str()),
            "case {:?}: expected the update to be the lastOperationIdentifier, got \
             {details:?}",
            row.case
        );
        assert_eq!(
            member(&details, "baselineVersion"),
            Some(row.initial_version),
            "case {:?}: expected the version still applied while the update is in \
             flight, got {details:?}",
            row.case
        );
        assert_eq!(
            parameters(&details),
            row.initial_parameters
                .clone()
                .unwrap_or_else(|| serde_json::json!([])),
            "case {:?}: expected the parameters still applied while the update is in \
             flight, got {details:?}",
            row.case
        );

        started.push((row, arn, operation_identifier));
    }

    for (index, (row, _, operation_identifier)) in started.iter().enumerate() {
        for (other, _, other_identifier) in &started[index + 1..] {
            assert_ne!(
                operation_identifier, other_identifier,
                "cases {:?} and {:?}: two updates must mint different \
                 operationIdentifiers, got {operation_identifier:?} twice",
                row.case, other.case
            );
        }
    }

    // Nothing sleeps server-side; the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn, operation_identifier) in &started {
        let (_, operation) = get_operation(&router, operation_identifier).await;
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some("UPDATE_ENABLED_BASELINE"), Some("SUCCEEDED")),
            "case {:?}: expected UPDATE_ENABLED_BASELINE SUCCEEDED once due, got \
             {operation:?}",
            row.case
        );
        assert!(
            operation.get("endTime").is_some(),
            "case {:?}: expected an endTime once the update is due, got {operation:?}",
            row.case
        );

        let (applied, details) = get_enabled_baseline(&router, arn).await;
        assert_eq!(
            applied.status, 200,
            "case {:?}: get-enabled-baseline expected 200, got {} body {:?}",
            row.case, applied.status, applied.body
        );
        assert_eq!(
            member(&details, "baselineVersion"),
            Some(row.new_version),
            "case {:?}: expected the requested version applied once the update is due, \
             got {details:?}",
            row.case
        );
        assert_eq!(
            parameters(&details),
            row.expected_parameters(),
            "case {:?}: expected the parameters the rule leaves applied, got {details:?}",
            row.case
        );
        assert_eq!(
            status(&details),
            Some("SUCCEEDED"),
            "case {:?}: expected SUCCEEDED once the update is due, got {details:?}",
            row.case
        );
        assert_eq!(
            last_operation(&details),
            Some(operation_identifier.as_str()),
            "case {:?}: the update stays the lastOperationIdentifier, got {details:?}",
            row.case
        );
        // An update changes the applied version and parameters and NOTHING else: the
        // resource keeps its ARN, its baseline and its target.
        assert_eq!(
            (member(&details, "arn"), member(&details, "baselineIdentifier")),
            (Some(arn.as_str()), Some(baseline)),
            "case {:?}: an update must not move the resource or its baseline, got \
             {details:?}",
            row.case
        );
    }
}

/// RULE (the version's published bounds, and one past them): `BaselineVersion` is
/// `@length` 1..10 with `@pattern ^\d+(?:\.\d+){0,2}$`
/// (`smithy_query(query="operation", operation="UpdateEnabledBaseline")`), so a value
/// inside both is accepted — the table above covers both length bounds, 1 and 10 — and one
/// outside either is refused as a bad request before the handler runs, leaving the applied
/// version alone.
#[tokio::test]
async fn a_version_outside_the_modelled_bounds_is_refused() {
    /// The error code the REST front door names an input-validation refusal with, as the
    /// `front_door_validation` suite pins it.
    const FRONT_DOOR_CODE: &str = "BadRequestException";

    let baseline = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";
    let target = "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-bd11-1a";

    let router = set_up_emulator().await;
    let enabled = enable(&router, baseline, target, "4.0", None).await;
    assert_eq!(
        enabled.status, 200,
        "enable: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let arn = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, version) in [
        ("one character past the length max of 10", "11111.11.11"),
        ("one character below the length min of 1", ""),
        ("one component past the pattern's three", "4.0.2.1"),
        ("a component that is not a number", "4.x"),
    ] {
        let refused = update(&router, &arn, version, None).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(FRONT_DOOR_CODE)),
            "case {case:?} ({version:?}): expected 400 {FRONT_DOOR_CODE}, got {:?} body \
             {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;
    let (_, details) = get_enabled_baseline(&router, &arn).await;
    assert_eq!(
        (member(&details, "baselineVersion"), status(&details)),
        (Some("4.0"), Some("SUCCEEDED")),
        "a refused update must apply nothing and start no operation, got {details:?}"
    );
}

/// RULE (the asynchrony of it, from the other side): a second update is accepted once the
/// first is due, and the applied values are then the second update's — so the pending
/// change is not lost or applied twice.
#[tokio::test]
async fn updates_apply_in_order_one_after_the_other() {
    let baseline = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";
    let target = "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-seq1-1a";

    let router = set_up_emulator().await;
    let enabled = enable(&router, baseline, target, "1.0", None).await;
    assert_eq!(
        enabled.status, 200,
        "enable: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let arn = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, version, params, expected) in [
        (
            "the first update",
            "2.0",
            Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": true }
            ])),
            serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": true }
            ]),
        ),
        (
            "a second update, version only",
            "3.0",
            None,
            serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": true }
            ]),
        ),
        (
            "a third update, replacing the parameters",
            "4.0",
            Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": "false" }
            ])),
            serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": "false" }
            ]),
        ),
    ] {
        let updated = update(&router, &arn, version, params).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            updated.status, updated.body
        );
        tokio::time::sleep(past_the_settle_span()).await;

        let (_, details) = get_enabled_baseline(&router, &arn).await;
        assert_eq!(
            (member(&details, "baselineVersion"), parameters(&details)),
            (Some(version), expected.clone()),
            "case {case:?}: expected version {version:?} with {expected:?} applied, got \
             {details:?}"
        );
    }
}

/// RULE: an update that overlaps an operation still changing the enablement is refused
/// with the modelled `ConflictException` (409) and writes nothing, whichever call started
/// that operation; once the clock has made it due the same request is accepted.
#[tokio::test]
async fn an_update_overlapping_an_operation_in_flight_is_a_conflict() {
    let baseline = "arn:aws:controltower:us-east-1::baseline/17BSJV3IGJ2QSGA2";
    let router = set_up_emulator().await;

    for (case, start_a_second_operation) in [
        ("while the enable is in flight", false),
        ("while another update is in flight", true),
    ] {
        let target = format!(
            "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-cf11-case{}",
            u8::from(start_a_second_operation)
        );
        let enabled = enable(&router, baseline, &target, "1.0", None).await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: enable expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );
        let arn = enabled.string("arn");

        // Whichever operation is in flight when the overlapping update arrives, the
        // version applied is still the enable's: an update in flight has not applied its
        // own version yet.
        if start_a_second_operation {
            tokio::time::sleep(past_the_settle_span()).await;
            let first = update(&router, &arn, "2.0", None).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: the first update expected 200, got {} body {:?}",
                first.status, first.body
            );
        }

        let refused = update(&router, &arn, "3.0", None).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected 409 ConflictException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused update must mint no operation, got {:?}",
            refused.body
        );

        let (_, details) = get_enabled_baseline(&router, &arn).await;
        assert_eq!(
            member(&details, "baselineVersion"),
            Some("1.0"),
            "case {case:?}: a refused update must apply nothing, got {details:?}"
        );

        // Once everything in flight is due, the SAME request is accepted.
        tokio::time::sleep(past_the_settle_span()).await;
        let accepted = update(&router, &arn, "3.0", None).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same update once nothing is in flight: expected 200, got \
             {} body {:?}",
            accepted.status, accepted.body
        );
        tokio::time::sleep(past_the_settle_span()).await;
        let (_, applied) = get_enabled_baseline(&router, &arn).await;
        assert_eq!(
            member(&applied, "baselineVersion"),
            Some("3.0"),
            "case {case:?}: expected the accepted update applied, got {applied:?}"
        );
    }
}

/// RULE: an identifier no `EnabledBaseline` matches is `ResourceNotFoundException` (404)
/// from `UpdateEnabledBaseline` and from `GetEnabledBaseline` alike — including a
/// well-formed ARN of another resource kind and the ARN of an enablement whose teardown
/// the clock has made due.
#[tokio::test]
async fn an_identifier_no_enablement_matches_is_not_found() {
    let baseline = "arn:aws:controltower:ap-south-1::baseline/YX7VMZML5IG8EJUD";
    let target = "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-nf11-1a";

    let router = set_up_emulator().await;
    let enabled = enable(&router, baseline, target, "1.0", None).await;
    assert_eq!(
        enabled.status, 200,
        "enable: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let torn_down_arn = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    let disabled = call(
        &router,
        "/disable-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": torn_down_arn }),
    )
    .await;
    assert_eq!(
        disabled.status, 200,
        "disable: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    tokio::time::sleep(past_the_settle_span()).await;

    // Every value satisfies `Arn`'s pattern and length, so the front door passes them and
    // the handlers are what refuse them.
    let cases: Vec<(&str, String)> = vec![
        (
            "an EnabledBaseline ARN no call minted",
            "arn:aws:controltower:us-west-2:123456789012:enabledbaseline/absent".to_string(),
        ),
        (
            "a well-formed ARN of another resource kind",
            "arn:aws:controltower:us-west-2:123456789012:landingzone/absent".to_string(),
        ),
        (
            "the baseline ARN rather than the enablement's",
            baseline.to_string(),
        ),
        ("a torn-down enablement's ARN", torn_down_arn.clone()),
    ];

    for (case, identifier) in cases {
        let refused = update(&router, &identifier, "4.0", None).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: update expected 404 ResourceNotFoundException, got {:?} body \
             {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );

        let (read, _) = get_enabled_baseline(&router, &identifier).await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: get expected 404 ResourceNotFoundException, got {:?} body \
             {:?}",
            (read.status, read.error_code.as_deref()),
            read.body
        );
    }
}

/// RULE: `GetEnabledBaseline` is reporting-only — reading an enablement repeatedly, while
/// an update is in flight and after it is due, reports the same thing and starts no
/// operation of its own, so a read never changes what a later caller sees.
#[tokio::test]
async fn reading_an_enablement_changes_nothing() {
    let baseline = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";
    let target = "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-ro11-1a";

    let router = set_up_emulator().await;
    let enabled = enable(
        &router,
        baseline,
        target,
        "1.0",
        Some(serde_json::json!([
            { "key": "IdentityCenterEnabledForBaseline", "value": "keep me" }
        ])),
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "enable: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let arn = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    let updated = update(&router, &arn, "2.0", None).await;
    assert_eq!(
        updated.status, 200,
        "update: expected 200, got {} body {:?}",
        updated.status, updated.body
    );
    let operation_identifier = updated.string("operationIdentifier");

    // Repeated reads while the update is in flight: identical, and none of them applies
    // the pending change early or moves the operation on.
    let (_, first) = get_enabled_baseline(&router, &arn).await;
    for read in 0..3u8 {
        let (_, again) = get_enabled_baseline(&router, &arn).await;
        assert_eq!(
            again, first,
            "read {read} while the update is in flight: expected the same details as the \
             first read {first:?}, got {again:?}"
        );
        let (_, operation) = get_operation(&router, &operation_identifier).await;
        assert_eq!(
            member(&operation, "status"),
            Some("IN_PROGRESS"),
            "read {read}: a read must not move the operation on, got {operation:?}"
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    let (_, settled) = get_enabled_baseline(&router, &arn).await;
    assert_eq!(
        (member(&settled, "baselineVersion"), status(&settled)),
        (Some("2.0"), Some("SUCCEEDED")),
        "once the update is due: expected 2.0 SUCCEEDED, got {settled:?}"
    );
    for read in 0..3u8 {
        let (_, again) = get_enabled_baseline(&router, &arn).await;
        assert_eq!(
            again, settled,
            "read {read} once the update is due: expected the same details as the first \
             read {settled:?}, got {again:?}"
        );
    }
    // The parameters the enable applied survived a version-only update, and no read
    // changed them.
    assert_eq!(
        parameters(&settled),
        serde_json::json!([
            { "key": "IdentityCenterEnabledForBaseline", "value": "keep me" }
        ]),
        "once the update is due: expected the enable's parameters still applied, got \
         {settled:?}"
    );
}
