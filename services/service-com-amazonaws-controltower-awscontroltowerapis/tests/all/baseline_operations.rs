// SPDX-License-Identifier: CC0-1.0

//! The baseline operation log, end to end over HTTP.
//!
//! Drives `EnableBaseline`, `DisableBaseline` and `GetBaselineOperation` through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * an `operationIdentifier` a baseline call returned resolves through
//!   `GetBaselineOperation` to the record that call wrote — the call's own
//!   `BaselineOperationType`, `IN_PROGRESS` until the clock makes it due and `SUCCEEDED`
//!   with a stable `endTime` after — for any baseline identifier, version, parameter list
//!   and tag map;
//! * an identifier no baseline call returned is `ResourceNotFoundException` (404),
//!   including one a landing zone call minted: the two logs are separate stores;
//! * what `EnableBaseline` persists is what a later call reads: the baseline/target pair
//!   is enabled exactly once at a time, and enables again only once its `DISABLE` is due;
//! * the enablement store is BOUNDED at `MAX_ENABLED_BASELINES`, and a request past that
//!   cap is refused with the modelled `ServiceQuotaExceededException` rather than growing
//!   the store.
//!
//! The enum values come from
//! `smithy_query(query="shape", shape="BaselineOperationType,BaselineOperationStatus")`
//! on this crate's `smithy-model/`: `ENABLE_BASELINE` | `DISABLE_BASELINE` |
//! `UPDATE_ENABLED_BASELINE` | `RESET_ENABLED_BASELINE`, and `SUCCEEDED` | `FAILED` |
//! `IN_PROGRESS`. The two types this slice can produce are asserted here; the other two
//! belong to the `enabled_baseline_update` / `enabled_baseline_reset` suites.

use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, past_the_settle_span, set_up_emulator,
    simple_manifest, Answer,
};

use smithy_server_runtime::MultiServiceRouter;

/// One row of the enablement table: the case name and the `EnableBaseline` members it
/// varies. `parameters` and `tags` are `None` when the row omits the member entirely.
pub(crate) struct Enablement {
    pub(crate) case: &'static str,
    pub(crate) baseline_identifier: &'static str,
    pub(crate) baseline_version: &'static str,
    pub(crate) target_identifier: &'static str,
    pub(crate) parameters: Option<serde_json::Value>,
    pub(crate) tags: Option<serde_json::Value>,
}

/// The enablement table: rows differ along what the rule generalises over — the baseline
/// (several entries of the catalog `ListBaselines`/`GetBaseline` publish, in several
/// regions and partitions-worth of ARN forms), the `baselineVersion` (each baseline's
/// OLDEST and NEWEST offered version, and one in between), the target, the parameter list
/// (absent, empty, and the declared key with two `Document` kinds — the value is "a
/// document of any type") and the tag map (absent, empty, several). The baselines, their
/// versions and the parameter keys they declare come from
/// `service_..._awscontroltowerapis::state::BASELINE_CATALOG`, which is what
/// `ListBaselines` reports. Every row names a DIFFERENT baseline/target pair, because one
/// pair is enabled at most once at a time.
pub(crate) fn enablements() -> Vec<Enablement> {
    vec![
        Enablement {
            case: "the oldest offered version, no parameters and no tags",
            baseline_identifier: "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
            baseline_version: "1.0",
            target_identifier: "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a",
            parameters: None,
            tags: None,
        },
        Enablement {
            case: "a baseline that declares no parameters, with an empty parameter list",
            baseline_identifier: "arn:aws:controltower:us-east-1::baseline/4T4HA1KMO10S6311",
            baseline_version: "1.0",
            target_identifier: "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-bb22-2b",
            parameters: Some(serde_json::json!([])),
            tags: Some(serde_json::json!({})),
        },
        Enablement {
            case: "a baseline whose newest offered version is not its predecessor's",
            baseline_identifier: "arn:aws:controltower:eu-west-1::baseline/LN25R72TTG6IGPTQ",
            baseline_version: "4.0",
            target_identifier: "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-cc33-3c",
            parameters: None,
            tags: Some(serde_json::json!({ "Team": "platform" })),
        },
        Enablement {
            case: "the newest offered version, with the declared parameter as a boolean",
            baseline_identifier: "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
            baseline_version: "4.0",
            target_identifier: "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-dd44-4d",
            parameters: Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": true }
            ])),
            tags: Some(serde_json::json!({ "Team": "platform", "Cost-Centre": "4711" })),
        },
        Enablement {
            case: "a baseline of one of the shared accounts, in another region",
            baseline_identifier: "arn:aws:controltower:ap-south-1::baseline/3WPD0NA6TJ9AOMU2",
            baseline_version: "1.0",
            target_identifier: "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-ee55-5e",
            parameters: Some(serde_json::json!([])),
            tags: None,
        },
        Enablement {
            case: "a version between the oldest and the newest, and the declared parameter \
                   as a string",
            baseline_identifier: "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
            baseline_version: "3.0",
            target_identifier: "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-ff77-7g",
            parameters: Some(serde_json::json!([
                { "key": "IdentityCenterEnabledForBaseline", "value": "true" }
            ])),
            tags: None,
        },
    ]
}

/// POST `/enable-baseline` with the row's members, omitting the optional ones the row
/// leaves out.
pub(crate) async fn enable(router: &MultiServiceRouter, row: &Enablement) -> Answer {
    let mut body = serde_json::json!({
        "baselineIdentifier": row.baseline_identifier,
        "baselineVersion": row.baseline_version,
        "targetIdentifier": row.target_identifier,
    });
    if let Some(parameters) = row.parameters.clone() {
        body["parameters"] = parameters;
    }
    if let Some(tags) = row.tags.clone() {
        body["tags"] = tags;
    }
    call(router, "/enable-baseline", body).await
}

/// POST `/enable-baseline` for a bare baseline/target pair, for the cases whose subject
/// is the store rather than the input shape.
async fn enable_pair(router: &MultiServiceRouter, baseline: &str, target: &str) -> Answer {
    call(
        router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": baseline,
            "baselineVersion": "4.0",
            "targetIdentifier": target,
        }),
    )
    .await
}

/// POST `/disable-baseline` for an `EnabledBaseline` identifier.
pub(crate) async fn disable(router: &MultiServiceRouter, enabled_baseline: &str) -> Answer {
    call(
        router,
        "/disable-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": enabled_baseline }),
    )
    .await
}

/// POST `/get-baseline-operation` and hand back the answer plus its `baselineOperation`
/// member, which is what every assertion below reads.
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

/// A member of a `baselineOperation`, as a `&str`.
pub(crate) fn member<'a>(operation: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    operation.get(name).and_then(serde_json::Value::as_str)
}

/// RULE: the identifier a baseline call returns resolves through `GetBaselineOperation`
/// to that call's own record — its `BaselineOperationType`, `IN_PROGRESS` while the
/// operation is in flight, and `SUCCEEDED` with a stable `endTime` once the clock has
/// made it due — whatever the enablement's members were.
#[tokio::test]
async fn an_identifier_enable_baseline_returned_reports_that_enablement() {
    let router = set_up_emulator().await;
    let mut started: Vec<(&'static str, String, String)> = Vec::new();

    for row in enablements() {
        let enabled = enable(&router, &row).await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: expected 200, got {} body {:?}",
            row.case, enabled.status, enabled.body
        );

        let arn = enabled.string("arn");
        let operation_identifier = enabled.string("operationIdentifier");
        assert!(
            arn.starts_with("arn:aws:controltower:us-west-2:")
                && arn.contains(":enabledbaseline/")
                && arn.len() >= 20,
            "case {:?}: expected an EnabledBaseline ARN of at least 20 chars in the \
             request's region, got {arn:?}",
            row.case
        );
        assert!(
            is_operation_identifier(&operation_identifier),
            "case {:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation_identifier:?}",
            row.case
        );

        // The enable "starts an asynchronous operation" (model docs), so the transient
        // state is observable to a caller acting straight after the call.
        let (in_flight, operation) = get_operation(&router, &operation_identifier).await;
        assert_eq!(
            in_flight.status, 200,
            "case {:?}: expected the returned identifier to resolve, got {} body {:?}",
            row.case, in_flight.status, in_flight.body
        );
        assert_eq!(
            member(&operation, "operationIdentifier"),
            Some(operation_identifier.as_str()),
            "case {:?}: expected the identifier to be echoed from state, got {operation:?}",
            row.case
        );
        assert_eq!(
            member(&operation, "operationType"),
            Some("ENABLE_BASELINE"),
            "case {:?}: expected operationType ENABLE_BASELINE, got {operation:?}",
            row.case
        );
        assert_eq!(
            member(&operation, "status"),
            Some("IN_PROGRESS"),
            "case {:?}: expected IN_PROGRESS while the operation is in flight, got \
             {operation:?}",
            row.case
        );
        assert!(
            operation.get("startTime").is_some(),
            "case {:?}: expected a startTime, got {operation:?}",
            row.case
        );
        assert!(
            operation.get("endTime").is_none(),
            "case {:?}: expected no endTime while the operation is in flight, got \
             {operation:?}",
            row.case
        );
        assert!(
            operation.get("statusMessage").is_none(),
            "case {:?}: a statusMessage is reported in case of failure only, got \
             {operation:?}",
            row.case
        );

        started.push((row.case, arn, operation_identifier));
    }

    for (index, (case, arn, operation_identifier)) in started.iter().enumerate() {
        for (other_case, other_arn, other_identifier) in &started[index + 1..] {
            assert_ne!(
                operation_identifier, other_identifier,
                "cases {case:?} and {other_case:?}: two calls must mint different \
                 operationIdentifiers, got {operation_identifier:?} twice"
            );
            assert_ne!(
                arn, other_arn,
                "cases {case:?} and {other_case:?}: two enablements must mint different \
                 ARNs, got {arn:?} twice"
            );
        }
    }

    // The clock makes every operation due; nothing sleeps server-side, the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, _, operation_identifier) in &started {
        let (settled, operation) = get_operation(&router, operation_identifier).await;
        assert_eq!(
            settled.status, 200,
            "case {case:?}: expected the record to still resolve, got {} body {:?}",
            settled.status, settled.body
        );
        assert_eq!(
            member(&operation, "status"),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED once the operation is due, got {operation:?}"
        );
        assert_eq!(
            member(&operation, "operationType"),
            Some("ENABLE_BASELINE"),
            "case {case:?}: the recorded type must not change as the status does, got \
             {operation:?}"
        );
        let end_time = operation.get("endTime").cloned();
        assert!(
            end_time.is_some(),
            "case {case:?}: expected an endTime once the operation is due, got {operation:?}"
        );

        // A terminal record is stable: a second read reports the same endTime, it is not
        // re-stamped per read.
        let (_, reread) = get_operation(&router, operation_identifier).await;
        assert_eq!(
            reread.get("endTime").cloned(),
            end_time,
            "case {case:?}: re-reading a terminal operation must report the same endTime, \
             got {reread:?}"
        );
    }
}

/// RULE (the type side of it): the record reports the CALL that created it, so two calls
/// against one enablement are two records with two types, and neither read disturbs the
/// other.
#[tokio::test]
async fn the_record_reports_the_call_that_created_it() {
    let baseline = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";
    let target = "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-ff66-6f";

    let router = set_up_emulator().await;
    let enabled = enable_pair(&router, baseline, target).await;
    assert_eq!(
        enabled.status, 200,
        "enable: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let arn = enabled.string("arn");
    let enable_operation = enabled.string("operationIdentifier");

    // Past the settle span, so the enablement is no longer changing and the disable is
    // not refused as a conflict.
    tokio::time::sleep(past_the_settle_span()).await;

    let disabled = disable(&router, &arn).await;
    assert_eq!(
        disabled.status, 200,
        "disable: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    let disable_operation = disabled.string("operationIdentifier");
    assert!(
        is_operation_identifier(&disable_operation),
        "disable: expected a 36-char lowercase-hex operationIdentifier, got \
         {disable_operation:?}"
    );
    assert_ne!(
        disable_operation, enable_operation,
        "disable: expected a new operationIdentifier, got the enable's \
         {enable_operation:?}"
    );

    for (case, identifier, expected_type, expected_status) in [
        (
            "the disable record, in flight",
            &disable_operation,
            "DISABLE_BASELINE",
            "IN_PROGRESS",
        ),
        (
            "the enable record, already terminal",
            &enable_operation,
            "ENABLE_BASELINE",
            "SUCCEEDED",
        ),
    ] {
        let (answer, operation) = get_operation(&router, identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {identifier:?} to resolve, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some(expected_type), Some(expected_status)),
            "case {case:?}: expected {expected_type} {expected_status}, got {operation:?}"
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    let (_, settled_disable) = get_operation(&router, &disable_operation).await;
    assert_eq!(
        member(&settled_disable, "status"),
        Some("SUCCEEDED"),
        "the disable record once due: expected SUCCEEDED, got {settled_disable:?}"
    );
    assert!(
        settled_disable.get("endTime").is_some(),
        "the disable record once due: expected an endTime, got {settled_disable:?}"
    );
}

/// RULE: an identifier no baseline call returned does not resolve — it is 404
/// `ResourceNotFoundException`, whether or not the account holds baseline operations, and
/// including an identifier the LANDING ZONE log minted: the two logs are separate stores.
#[tokio::test]
async fn an_identifier_no_baseline_call_returned_is_not_found() {
    for populated in [false, true] {
        let router = emulator();

        // A landing zone operation identifier: minted by this service, logged elsewhere.
        let created = call(
            &router,
            "/create-landingzone",
            serde_json::json!({
                "version": "3.3",
                "manifest": simple_manifest()
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "create-landingzone: expected 200, got {} body {:?}",
            created.status, created.body
        );
        let landing_zone_operation = created.string("operationIdentifier");

        if populated {
            let row = &enablements()[0];
            let enabled = enable(&router, row).await;
            assert_eq!(
                enabled.status, 200,
                "populating enable: expected 200, got {} body {:?}",
                enabled.status, enabled.body
            );
        }

        // Every value satisfies `OperationIdentifier`'s pattern and length, so the front
        // door passes them and the handler is what refuses them.
        let cases: Vec<(&str, String)> = vec![
            (
                "an all-zero identifier",
                "00000000-0000-0000-0000-000000000000".to_string(),
            ),
            (
                "an identifier no call minted",
                "deadbeef-dead-beef-dead-beefdeadbeef".to_string(),
            ),
            (
                "an identifier the landing zone log minted",
                landing_zone_operation.clone(),
            ),
        ];

        for (case, identifier) in cases {
            let (answer, _) = get_operation(&router, &identifier).await;
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} (log populated: {populated}): expected 404 \
                 ResourceNotFoundException, got {:?} body {:?}",
                (answer.status, answer.error_code.as_deref()),
                answer.body
            );
        }
    }
}

/// RULE: what `EnableBaseline` persists is what the next call reads — a baseline/target
/// pair is enabled at most once at a time, an enablement still changing refuses a
/// teardown, a repeated teardown of one in flight is the SAME operation (`DisableBaseline`
/// is `@idempotent`), and the pair enables again only once that teardown is due.
#[tokio::test]
async fn an_enablement_round_trips_through_the_store() {
    let baseline = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";
    let target = "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-9988-8x";
    let other_target = "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-7766-6y";

    let router = set_up_emulator().await;
    let first = enable_pair(&router, baseline, target).await;
    assert_eq!(
        first.status, 200,
        "first enable: expected 200, got {} body {:?}",
        first.status, first.body
    );
    let arn = first.string("arn");

    // The pair is in the store, so enabling it again would describe one target twice.
    let again = enable_pair(&router, baseline, target).await;
    assert_eq!(
        (again.status, again.error_code.as_deref()),
        (409, Some("ConflictException")),
        "a second enable of the same pair: expected 409 ConflictException, got {:?} \
         body {:?}",
        (again.status, again.error_code.as_deref()),
        again.body
    );

    // The same baseline on a DIFFERENT target is a different enablement.
    let sibling = enable_pair(&router, baseline, other_target).await;
    assert_eq!(
        sibling.status, 200,
        "the same baseline on another target: expected 200, got {} body {:?}",
        sibling.status, sibling.body
    );
    assert_ne!(
        sibling.string("arn"),
        arn,
        "two enablements must mint different ARNs, got {arn:?} twice"
    );

    // While the enablement is still being applied, tearing it down would leave it half
    // applied.
    let too_soon = disable(&router, &arn).await;
    assert_eq!(
        (too_soon.status, too_soon.error_code.as_deref()),
        (409, Some("ConflictException")),
        "disable while the enable is in flight: expected 409 ConflictException, got {:?} \
         body {:?}",
        (too_soon.status, too_soon.error_code.as_deref()),
        too_soon.body
    );

    tokio::time::sleep(past_the_settle_span()).await;

    let disabled = disable(&router, &arn).await;
    assert_eq!(
        disabled.status, 200,
        "disable once the enable is due: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    let disable_operation = disabled.string("operationIdentifier");

    // `@idempotent`: repeating the call while that teardown is in flight reports the SAME
    // operation rather than starting a second one.
    let repeated = disable(&router, &arn).await;
    assert_eq!(
        (repeated.status, repeated.string("operationIdentifier")),
        (200, disable_operation.clone()),
        "a repeated disable while the first is in flight: expected 200 and \
         {disable_operation:?}, got {:?} body {:?}",
        repeated.status,
        repeated.body
    );

    tokio::time::sleep(past_the_settle_span()).await;

    // Once the teardown is due the resource is gone, so the identifier no longer names
    // one...
    let gone = disable(&router, &arn).await;
    assert_eq!(
        (gone.status, gone.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "disable after the teardown is due: expected 404 ResourceNotFoundException, got \
         {:?} body {:?}",
        (gone.status, gone.error_code.as_deref()),
        gone.body
    );

    // ...and the pair enables again, as a NEW resource.
    let reenabled = enable_pair(&router, baseline, target).await;
    assert_eq!(
        reenabled.status, 200,
        "enabling the pair again once the teardown is due: expected 200, got {} body {:?}",
        reenabled.status, reenabled.body
    );
    assert_ne!(
        reenabled.string("arn"),
        arn,
        "a re-enabled pair must mint a new ARN, got the torn-down {arn:?}"
    );

    // The teardown's record outlives the resource it tore down.
    let (resolved, operation) = get_operation(&router, &disable_operation).await;
    assert_eq!(
        resolved.status, 200,
        "the teardown's record: expected it to outlive the resource, got {} body {:?}",
        resolved.status, resolved.body
    );
    assert_eq!(
        (
            member(&operation, "operationType"),
            member(&operation, "status")
        ),
        (Some("DISABLE_BASELINE"), Some("SUCCEEDED")),
        "the teardown's record: expected DISABLE_BASELINE SUCCEEDED, got {operation:?}"
    );
}

/// RULE: the enablement store is BOUNDED. An account holds at most
/// `MAX_ENABLED_BASELINES` enablements; a request past that cap is refused with the
/// `ServiceQuotaExceededException` the model declares for `EnableBaseline` (HTTP 402) and
/// writes nothing, and the cap is a live bound — tearing an enablement down makes room
/// again.
#[tokio::test]
async fn the_enablement_store_is_bounded() {
    use service_com_amazonaws_controltower_awscontroltowerapis::state::MAX_ENABLED_BASELINES;

    let baseline = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";
    let overflow_target =
        "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-cap1-overflow";

    let router = set_up_emulator().await;
    let mut first_arn = String::new();
    for index in 0..MAX_ENABLED_BASELINES {
        let target =
            format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-cap1-{index:06}");
        let enabled = enable_pair(&router, baseline, &target).await;
        assert_eq!(
            enabled.status, 200,
            "enablement {index} of {MAX_ENABLED_BASELINES}: expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );
        if index == 0 {
            first_arn = enabled.string("arn");
        }
    }

    let over = enable_pair(&router, baseline, overflow_target).await;
    assert_eq!(
        (over.status, over.error_code.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "one enablement past the cap of {MAX_ENABLED_BASELINES}: expected 402 \
         ServiceQuotaExceededException, got {:?} body {:?}",
        (over.status, over.error_code.as_deref()),
        over.body
    );
    // The refusal wrote nothing: it minted neither a resource ARN nor an operation.
    assert!(
        over.body.get("arn").is_none() && over.body.get("operationIdentifier").is_none(),
        "a refused enablement must mint nothing, got {:?}",
        over.body
    );

    // The cap is a live bound, not a permanent wall: tearing one enablement down makes
    // room for the refused one. Past the settle span first, so the enablement being torn
    // down is no longer changing.
    tokio::time::sleep(past_the_settle_span()).await;
    let disabled = disable(&router, &first_arn).await;
    assert_eq!(
        disabled.status, 200,
        "disabling to make room: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    tokio::time::sleep(past_the_settle_span()).await;

    let accepted = enable_pair(&router, baseline, overflow_target).await;
    assert_eq!(
        accepted.status, 200,
        "the enablement that was over the cap, after one was torn down: expected 200, \
         got {} body {:?}",
        accepted.status, accepted.body
    );
}
