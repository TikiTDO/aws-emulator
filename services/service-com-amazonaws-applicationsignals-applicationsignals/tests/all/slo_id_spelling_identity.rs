// SPDX-License-Identifier: CC0-1.0

//! The identity a `ServiceLevelObjectiveId` denotes, across the batches that take a list of
//! them: `BatchGetServiceLevelObjectiveBudgetReport` (POST `/budget-report`) and
//! `BatchUpdateExclusionWindows` (PATCH `/exclusion-windows`).
//!
//! The rule under test: an SLO of this account is identified by the NAME its id denotes —
//! the partition and Region fields of an SLO ARN are no part of that identity — so every
//! entry of one request that denotes the same SLO is answered ONCE, whichever id form and
//! whichever of those scope fields each entry is spelled with. The ACCOUNT field is part of
//! what an ARN identifies, so it is held out of these rows; `slo_id_account_scope` is the
//! suite that owns it.
//!
//! Each rule is asserted over a TABLE spanning the class: the rows differ along the id forms
//! paired (a bare name with an ARN, two ARNs), the ARN field they disagree on (Region,
//! partition, both), the number of spellings one SLO is named by, and the characters the
//! SLO's own name carries.

use serde_json::{json, Value};

use crate::budget_reports::{budget_report, now_seconds, CALLER_ERROR_CODE};
use crate::exclusion_windows::{batch, window};
use crate::resource_tags::SLO_RESOURCE_TYPE;
use crate::slo_lifecycle::{create, router, sli_config, ACCOUNT, REGION};

/// An SLO ARN with each of its scope fields spelled out.
///
/// The modelled `ServiceLevelObjectiveArn` pattern
/// (`smithy_query(shape="ServiceLevelObjectiveArn")` ->
/// `^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`)
/// names the two partitions and leaves the Region and account fields unconstrained — `[^:]*`
/// admits ANY value for either, the empty one included.
fn slo_arn_in(partition: &str, region: &str, account: &str, name: &str) -> String {
    format!("arn:{partition}:application-signals:{region}:{account}:slo/{name}")
}

/// The ARN of `name` as the caller's own credential scope renders it: the partition, Region
/// and account every request of this suite is signed for.
fn signed_arn(name: &str) -> String {
    slo_arn_in("aws", REGION, ACCOUNT, name)
}

/// The ARN an id DENOTES, which is how a failure identifies an SLO no record is stored for:
/// an id already in ARN form is that ARN, and a bare name is the ARN it would carry in the
/// Region the request is signed for.
fn denoted_arn(id: &str) -> String {
    if id.starts_with("arn:") {
        id.to_string()
    } else {
        signed_arn(id)
    }
}

/// One row: the SLO name, and every id spelling of it the row's single request sends.
struct Case {
    label: &'static str,
    name: &'static str,
    ids: Vec<String>,
}

/// The table: one row per way a single request can name ONE SLO more than once.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a bare name and the ARN of the signed-for scope",
            name: "spelled-by-name-and-arn",
            ids: vec![
                "spelled-by-name-and-arn".to_string(),
                signed_arn("spelled-by-name-and-arn"),
            ],
        },
        Case {
            label: "a bare name and an ARN of another Region",
            name: "spelled-across-regions",
            ids: vec![
                "spelled-across-regions".to_string(),
                slo_arn_in("aws", "us-west-2", ACCOUNT, "spelled-across-regions"),
            ],
        },
        Case {
            label: "two ARNs differing only in their Region field",
            name: "two-regions",
            ids: vec![
                slo_arn_in("aws", "ap-south-1", ACCOUNT, "two-regions"),
                slo_arn_in("aws", "us-east-1", ACCOUNT, "two-regions"),
            ],
        },
        Case {
            label: "two ARNs differing in their Region AND their partition",
            name: "two-scopes",
            ids: vec![
                slo_arn_in("aws", "ap-south-1", ACCOUNT, "two-scopes"),
                slo_arn_in("aws-us-gov", "us-gov-west-1", ACCOUNT, "two-scopes"),
            ],
        },
        Case {
            label: "two ARNs differing only in their partition field",
            name: "two-partitions",
            ids: vec![
                signed_arn("two-partitions"),
                slo_arn_in("aws-us-gov", REGION, ACCOUNT, "two-partitions"),
            ],
        },
        Case {
            // The pattern's `[^:]*` admits an EMPTY Region field, one step past the values a
            // real credential scope renders.
            label: "a bare name and an ARN whose Region field is empty",
            name: "empty-region-field",
            ids: vec![
                "empty-region-field".to_string(),
                slo_arn_in("aws", "", ACCOUNT, "empty-region-field"),
            ],
        },
        Case {
            label: "four spellings of one SLO in one request",
            name: "four-spellings",
            ids: vec![
                slo_arn_in("aws-us-gov", "eu-central-1", ACCOUNT, "four-spellings"),
                "four-spellings".to_string(),
                signed_arn("four-spellings"),
                slo_arn_in("aws-us-gov", "eu-central-1", ACCOUNT, "four-spellings"),
            ],
        },
        Case {
            // A name carrying every separator the `ServiceLevelObjectiveName` class admits
            // (`-`, `.`, `_` and a space), which the ARN form carries in its resource field.
            label: "a name carrying the separators the name class admits",
            name: "a.b-c_d e",
            ids: vec![
                "a.b-c_d e".to_string(),
                slo_arn_in("aws", "us-west-2", ACCOUNT, "a.b-c_d e"),
            ],
        },
    ]
}

/// One exclusion window, the mutation the sibling batch is driven with.
fn one_window() -> Vec<Value> {
    vec![window("HOUR", 1, Some(1_600_000_000), None, Some("deploy"))]
}

/// A request naming ONE SLO that is not stored earns exactly ONE failure for it, however
/// many id spellings of it the request carries — the budget report states one `Errors` entry,
/// and the sibling batch over the same member is refused once.
#[tokio::test]
async fn one_missing_slo_named_by_many_spellings_fails_once() {
    for case in cases() {
        let router = router();
        // The failure identifies the SLO by the ARN the FIRST spelling denotes: there is no
        // stored record to read an ARN off.
        let expected_arn = denoted_arn(&case.ids[0]);

        let (status, response) = budget_report(&router, &case.ids, now_seconds()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the budget report, got {status} with {response}",
            case.label
        );
        assert_eq!(
            response["Reports"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: expected no reports for an SLO that is not stored, got {:?}",
            case.label,
            response["Reports"]
        );
        let errors = response["Errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            1,
            "case {:?}: expected ONE Errors entry for the one SLO named, got {:?}",
            case.label,
            response["Errors"]
        );
        assert!(
            errors[0]["Name"].is_null(),
            "case {:?}: expected no Name member on the entry, got {:?}",
            case.label,
            errors[0]
        );
        assert_eq!(
            errors[0]["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {:?}: expected Arn {expected_arn:?}, got {:?}",
            case.label,
            errors[0]
        );
        assert_eq!(
            errors[0]["ErrorCode"].as_str(),
            Some(CALLER_ERROR_CODE),
            "case {:?}: expected ErrorCode {CALLER_ERROR_CODE:?}, got {:?}",
            case.label,
            errors[0]
        );
        assert_eq!(
            errors[0]["ErrorMessage"].as_str(),
            Some(format!("{SLO_RESOURCE_TYPE} with arn '{expected_arn}' does not exist.").as_str()),
            "case {:?}: expected the message to identify the SLO by its ARN, got {:?}",
            case.label,
            errors[0]
        );

        // The sibling batch over the same member resolves those ids to the same one SLO, so
        // the request applies to nothing and is refused with the modelled exception once.
        let (status, error) = batch(&router, &case.ids, &one_window(), &[]).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected HTTP 404 when no id names a stored SLO, got {status} with {error}",
            case.label
        );
        assert_eq!(
            error["ResourceId"].as_str(),
            Some(expected_arn.as_str()),
            "case {:?}: expected ResourceId {expected_arn:?}, got {error}",
            case.label
        );
    }
}

/// A request naming ONE stored SLO is answered once for it, however many id spellings of it
/// the request carries — the budget report states one report, carrying the STORED record's
/// ARN rather than any spelling the request used, and the sibling batch applies it once.
#[tokio::test]
async fn one_stored_slo_named_by_many_spellings_is_answered_once() {
    for case in cases() {
        let router = router();
        let (status, created) = create(
            &router,
            json!({
                "Name": case.name,
                "SliConfig": sli_config("orders", "GET /orders", 250.0),
                "Goal": {
                    "Interval": { "RollingInterval": { "DurationUnit": "DAY", "Duration": 7 } },
                    "AttainmentGoal": 99.0
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 creating the SLO, got {status} with {created}",
            case.label
        );
        let stored_arn = created["Slo"]["Arn"].as_str().unwrap_or_default().to_string();

        let (status, response) = budget_report(&router, &case.ids, now_seconds() + 60).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the budget report, got {status} with {response}",
            case.label
        );
        assert_eq!(
            response["Errors"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: expected no Errors entry for a stored SLO, got {:?}",
            case.label,
            response["Errors"]
        );
        let reports = response["Reports"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            reports.len(),
            1,
            "case {:?}: expected ONE report for the one SLO named, got {:?}",
            case.label,
            response["Reports"]
        );
        assert_eq!(
            reports[0]["Name"].as_str(),
            Some(case.name),
            "case {:?}: expected the report to name the SLO {:?}, got {:?}",
            case.label,
            case.name,
            reports[0]
        );
        assert_eq!(
            reports[0]["Arn"].as_str(),
            Some(stored_arn.as_str()),
            "case {:?}: expected the STORED ARN {stored_arn:?}, got {:?}",
            case.label,
            reports[0]
        );

        // The sibling batch applies the one SLO once and reports it once.
        let (status, updated) = batch(&router, &case.ids, &one_window(), &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the sibling batch, got {status} with {updated}",
            case.label
        );
        assert_eq!(
            updated["SloIds"].as_array().map(Vec::len),
            Some(1),
            "case {:?}: expected the one SLO to be applied once, got {:?}",
            case.label,
            updated["SloIds"]
        );
        assert_eq!(
            updated["Errors"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: expected no failures applying a stored SLO, got {:?}",
            case.label,
            updated["Errors"]
        );
    }
}
