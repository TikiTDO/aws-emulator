// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of WHICH ACCOUNT an ARN a request supplies names, over every operation
//! of this service that addresses an SLO by one: the ARN form of a `ServiceLevelObjectiveId`
//! (`GetServiceLevelObjective`, `UpdateServiceLevelObjective`, `DeleteServiceLevelObjective`,
//! `ListServiceLevelObjectiveExclusionWindows`, `BatchUpdateExclusionWindows` and
//! `BatchGetServiceLevelObjectiveBudgetReport`), an audit target's `SloArn`, and the
//! `ResourceArn` of the tagging plane.
//!
//! The rule under test: an ARN a request supplies names a resource of this service only when
//! its ACCOUNT field is the account this store holds — this process holds ONE account's
//! records, so an ARN naming any other account names a resource this emulator holds none of,
//! and every operation answers it as it answers an SLO that is not stored. The modelled
//! `ServiceLevelObjectiveId` pattern does not settle this before a handler runs: it fixes the
//! namespace but spells the account field `[^:]*`
//! (`smithy_query(query="shape", shape="ServiceLevelObjectiveId")`), which admits any value
//! without a `:`, the empty one included.
//!
//! The REGION and PARTITION fields are a different matter and are no part of what an ARN
//! identifies here; `slo_id_spelling_identity` and `tagged_arn_identity` are the suites that
//! own them.
//!
//! Each rule is asserted over a TABLE of account fields crossed with every operation above:
//! the rows differ along whether the field is this store's account, how far from it a
//! 12-digit id is, and the forms the pattern admits that are no account id at all (the empty
//! field, a non-digit one). The request helpers are the ones `slo_lifecycle`,
//! `exclusion_windows`, `budget_reports`, `resource_tags` and `audit_findings` already drive
//! this service's `restJson1` surface with, so every suite speaks to one harness.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::audit_findings::{audit, target};
use crate::budget_reports::{budget_report, now_seconds, CALLER_ERROR_CODE};
use crate::exclusion_windows::{batch, window};
use crate::refusal_wording::assert_work_item_wording;
use crate::resource_tags::{list_tags_raw, SLO_RESOURCE_TYPE};
use crate::slo_lifecycle::{
    arn_uri_segment, call, create, name_path_segment, router, sli_config, ACCOUNT, REGION,
};

/// The SLO every row addresses, by an ARN differing only in its account field.
const SLO_NAME: &str = "account-scoped-slo";

/// One row of the account-field table: the account an ARN carries, and whether that ARN names
/// the stored SLO.
struct Case {
    label: &'static str,
    account: &'static str,
    /// Whether an ARN carrying this account field names the resource this store holds.
    names_the_resource: bool,
}

/// The account fields the rule is asserted over.
///
/// Every value here is one the modelled `ServiceLevelObjectiveId` pattern admits in that
/// field (`[^:]*`), so each one reaches the handler instead of being refused at the boundary:
/// this store's own account, 12-digit ids at increasing distance from it, and the two forms
/// the pattern admits that are no account id at all — the EMPTY field (an SLO lives in an
/// account, so an account-less ARN names none of this store's) and a field of non-digits.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "this store's own account",
            account: ACCOUNT,
            names_the_resource: true,
        },
        Case {
            label: "an account one digit from this store's",
            account: "000000000001",
            names_the_resource: false,
        },
        Case {
            label: "the documented example account",
            account: "123456789012",
            names_the_resource: false,
        },
        Case {
            label: "an account sharing no digit with this store's",
            account: "999999999999",
            names_the_resource: false,
        },
        Case {
            label: "an empty account field",
            account: "",
            names_the_resource: false,
        },
        Case {
            label: "an account field that is no account id",
            account: "not-an-account",
            names_the_resource: false,
        },
    ]
}

/// The ARN of [`SLO_NAME`] carrying `account`, in the Region every request of this suite is
/// signed for.
fn arn_in(account: &str) -> String {
    format!("arn:aws:application-signals:{REGION}:{account}:slo/{SLO_NAME}")
}

/// Create the SLO every row addresses on a fresh router, and hand both back.
async fn stored() -> MultiServiceRouter {
    let router = router();
    let (status, created) = create(
        &router,
        json!({
            "Name": SLO_NAME,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"seed the SLO every row addresses\": expected HTTP 200, got {status} with {created}"
    );
    router
}

/// The message every refusal about an SLO that is not stored carries, naming the ARN the
/// request supplied ([`crate::error_identity`] owns that wording over more inputs).
fn not_found_message(arn: &str) -> String {
    format!("{SLO_RESOURCE_TYPE} with arn '{arn}' does not exist.")
}

/// Assert `error` is the modelled `ResourceNotFoundException` about `arn`.
fn assert_not_found(case: &str, status: u16, error: &Value, arn: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 for an ARN naming another account, got {status} with \
         {error:?}"
    );
    assert_eq!(
        error["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected a ResourceNotFoundException, got {error:?}"
    );
    assert_eq!(
        error["ResourceType"].as_str(),
        Some(SLO_RESOURCE_TYPE),
        "case {case:?}: expected ResourceType {SLO_RESOURCE_TYPE:?}, got {error:?}"
    );
    assert_eq!(
        error["ResourceId"].as_str(),
        Some(arn),
        "case {case:?}: expected the refusal to name the ARN the request carried ({arn:?}), got \
         {error:?}"
    );
}

/// One exclusion window, the mutation the window operations are driven with.
fn one_window() -> Vec<Value> {
    vec![window("HOUR", 1, Some(1_600_000_000), None, Some("deploy"))]
}

/// Every read of an SLO by an ARN answers the stored record for this store's account and the
/// modelled `ResourceNotFoundException` for any other: `GetServiceLevelObjective`,
/// `ListServiceLevelObjectiveExclusionWindows` and the tagging plane's
/// `ListTagsForResource`, which already held the rule.
#[tokio::test]
async fn every_read_by_arn_answers_this_account_or_not_found() {
    for row in cases() {
        let router = stored().await;
        let arn = arn_in(row.account);
        let segment = arn_uri_segment(&arn);

        let (status, answer) = call(&router, "GET", &format!("/slo/{segment}"), None).await;
        let case = format!("GetServiceLevelObjective by {}", row.label);
        if row.names_the_resource {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for this store's own account, got {status} \
                 with {answer:?}"
            );
            assert_eq!(
                answer["Slo"]["Name"].as_str(),
                Some(SLO_NAME),
                "case {case:?}: expected the stored SLO, got {answer:?}"
            );
        } else {
            assert_not_found(&case, status, &answer, &arn);
            assert_eq!(
                answer["Message"].as_str(),
                Some(not_found_message(&arn).as_str()),
                "case {case:?}: expected the message to name the ARN supplied, got {answer:?}"
            );
        }

        let (status, answer) = call(
            &router,
            "GET",
            &format!("/slo/{segment}/exclusion-windows"),
            None,
        )
        .await;
        let case = format!("ListServiceLevelObjectiveExclusionWindows by {}", row.label);
        if row.names_the_resource {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for this store's own account, got {status} \
                 with {answer:?}"
            );
        } else {
            assert_not_found(&case, status, &answer, &arn);
        }

        let (status, answer) = list_tags_raw(&router, &segment).await;
        let case = format!("ListTagsForResource by {}", row.label);
        if row.names_the_resource {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for this store's own account, got {status} \
                 with {answer:?}"
            );
        } else {
            assert_not_found(&case, status, &answer, &arn);
        }
    }
}

/// The batches that take a LIST of ids resolve each entry by the same rule: an entry naming
/// this store's account is answered for the stored SLO, and one naming any other account
/// earns that entry's own not-found failure.
#[tokio::test]
async fn every_batch_entry_is_resolved_against_this_account() {
    for row in cases() {
        let router = stored().await;
        let arn = arn_in(row.account);

        let (status, answer) =
            budget_report(&router, std::slice::from_ref(&arn), now_seconds()).await;
        let case = format!("BatchGetServiceLevelObjectiveBudgetReport by {}", row.label);
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 whichever account the id names, got {status} with \
             {answer:?}"
        );
        let reports = answer["Reports"].as_array().cloned().unwrap_or_default();
        let errors = answer["Errors"].as_array().cloned().unwrap_or_default();
        if row.names_the_resource {
            assert_eq!(
                reports.len(),
                1,
                "case {case:?}: expected ONE report for the stored SLO, got {answer:?}"
            );
            assert!(
                errors.is_empty(),
                "case {case:?}: expected no Errors entry for the stored SLO, got {answer:?}"
            );
        } else {
            assert!(
                reports.is_empty(),
                "case {case:?}: expected no report for another account's ARN, got {answer:?}"
            );
            assert_eq!(
                errors.len(),
                1,
                "case {case:?}: expected ONE Errors entry for another account's ARN, got \
                 {answer:?}"
            );
            // The failure identifies the SLO by the ARN the request supplied, and by nothing
            // else: an id that resolves to no record names no name to report.
            assert!(
                errors[0]["Name"].is_null(),
                "case {case:?}: expected no Name member on the entry, got {errors:?}"
            );
            assert_eq!(
                errors[0]["Arn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the entry to carry the ARN supplied, got {errors:?}"
            );
            assert_eq!(
                errors[0]["ErrorCode"].as_str(),
                Some(CALLER_ERROR_CODE),
                "case {case:?}: expected ErrorCode {CALLER_ERROR_CODE:?}, got {errors:?}"
            );
            assert_eq!(
                errors[0]["ErrorMessage"].as_str(),
                Some(not_found_message(&arn).as_str()),
                "case {case:?}: expected the message to name the ARN supplied, got {errors:?}"
            );
        }

        let (status, answer) =
            batch(&router, std::slice::from_ref(&arn), &one_window(), &[]).await;
        let case = format!("BatchUpdateExclusionWindows by {}", row.label);
        if row.names_the_resource {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for this store's own account, got {status} \
                 with {answer:?}"
            );
            assert_eq!(
                answer["SloIds"].as_array().map(Vec::len),
                Some(1),
                "case {case:?}: expected the window applied to the stored SLO, got {answer:?}"
            );
        } else {
            assert_not_found(&case, status, &answer, &arn);
        }

        // The window set of the stored SLO is what the refusal left alone: a request naming
        // another account adds nothing to this account's record.
        let (status, answer) = call(
            &router,
            "GET",
            &format!(
                "/slo/{}/exclusion-windows",
                name_path_segment(SLO_NAME)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the stored SLO's windows to be readable, got {status} with \
             {answer:?}",
            row.label
        );
        let expected = usize::from(row.names_the_resource);
        assert_eq!(
            answer["ExclusionWindows"]
                .as_array()
                .map_or(0, Vec::len),
            expected,
            "case {:?}: expected {expected} stored window(s) after the batch, got {answer:?}",
            row.label
        );
    }
}

/// An audit target names an objective of this account only: a `SloArn` carrying any other
/// account resolves to no objective, so the read reports no finding for it, while the same
/// ARN in this store's account is assessed.
#[tokio::test]
async fn an_audit_target_resolves_an_arn_of_this_account_only() {
    for row in cases() {
        let router = stored().await;
        let arn = arn_in(row.account);
        let now = now_seconds();
        let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
        let case = format!("ListAuditFindings by {}", row.label);

        let (status, answer) = audit(
            &router,
            &window,
            vec![target("slo", json!({ "Slo": { "SloArn": arn } }))],
            json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 whichever account the target names, got {status} \
             with {answer:?}"
        );
        let expected = usize::from(row.names_the_resource);
        assert_eq!(
            answer["AuditFindings"].as_array().map_or(0, Vec::len),
            expected,
            "case {case:?}: expected {expected} finding(s), got {answer:?}"
        );
    }
}

/// The writes answer the same way, and — the consequence that matters — a write naming
/// another account changes NOTHING this store holds: the update leaves the stored record
/// exactly as it was and the delete leaves it stored.
#[tokio::test]
async fn no_write_by_another_accounts_arn_touches_this_accounts_record() {
    for row in cases() {
        let router = stored().await;
        let arn = arn_in(row.account);
        let segment = arn_uri_segment(&arn);
        let described = "a description only a write may set";

        let (status, answer) = call(
            &router,
            "PATCH",
            &format!("/slo/{segment}"),
            Some(json!({ "Description": described })),
        )
        .await;
        let case = format!("UpdateServiceLevelObjective by {}", row.label);
        if row.names_the_resource {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for this store's own account, got {status} \
                 with {answer:?}"
            );
        } else {
            assert_not_found(&case, status, &answer, &arn);
            // The update resolves the SLO through the work item that holds it, so its
            // refusal is worded about that item, by the name the id spells.
            assert_work_item_wording(
                &case,
                SLO_NAME,
                answer["Message"].as_str().unwrap_or_default(),
            );
        }

        // What the stored record carries after the update: the description only a write that
        // named THIS account can have set.
        let (status, fetched) = call(
            &router,
            "GET",
            &format!("/slo/{}", name_path_segment(SLO_NAME)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the stored SLO to still be readable, got {status} with \
             {fetched:?}",
            row.label
        );
        let expected = row.names_the_resource.then_some(described);
        assert_eq!(
            fetched["Slo"]["Description"].as_str(),
            expected,
            "case {:?}: expected Description {expected:?}, got {fetched:?}",
            row.label
        );

        let (status, answer) = call(&router, "DELETE", &format!("/slo/{segment}"), None).await;
        let case = format!("DeleteServiceLevelObjective by {}", row.label);
        if row.names_the_resource {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for this store's own account, got {status} \
                 with {answer:?}"
            );
        } else {
            assert_not_found(&case, status, &answer, &arn);
            assert_eq!(
                answer["Message"].as_str(),
                Some(not_found_message(&arn).as_str()),
                "case {case:?}: expected the message to name the ARN supplied, got {answer:?}"
            );
        }

        // The record itself: gone only when the delete named this store's account.
        let (status, after) = call(
            &router,
            "GET",
            &format!("/slo/{}", name_path_segment(SLO_NAME)),
            None,
        )
        .await;
        let expected = if row.names_the_resource { 404 } else { 200 };
        assert_eq!(
            status, expected,
            "case {:?}: expected HTTP {expected} reading the SLO back after the delete, got \
             {status} with {after:?}",
            row.label
        );
    }
}
