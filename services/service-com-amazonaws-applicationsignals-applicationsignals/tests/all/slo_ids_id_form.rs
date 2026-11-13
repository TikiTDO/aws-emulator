// SPDX-License-Identifier: CC0-1.0

//! The id FORM every operation that takes `ServiceLevelObjectiveIds` holds its entries to,
//! over HTTP: `BatchUpdateExclusionWindows` (PATCH `/exclusion-windows`) and
//! `BatchGetServiceLevelObjectiveBudgetReport` (POST `/budget-report`), the two operations
//! the model binds that list shape to.
//!
//! The rule under test: an entry of a request member whose list element the model types as a
//! plain `String` while the resource it names is modelled with a `@pattern` is held to that
//! pattern by the handler, and a request carrying an entry that violates it is refused WHOLE
//! with the modelled HTTP 400 `ValidationException` — never reported in a per-item `Errors`
//! entry, whose own members the model pattern-constrains, and never applied in part.
//!
//! The rows span what the rule generalises over: which OPERATION the list is sent to, WHERE
//! in the list the offending entry sits, and HOW an id fails the pattern — a length under
//! and over the published one, a character the name class excludes, a non-alphanumeric first
//! or last character, a multi-byte character, and an ARN of another partition, another
//! service, another resource type or no resource name at all. The served rows span the forms
//! the pattern DOES admit, including each published length bound.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! through the one harness (`router`, `call`, `create`, …) `slo_lifecycle` already drives
//! this service's surface with.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::budget_reports::{budget_report, now_seconds, CALLER_ERROR_CODE};
use crate::exclusion_windows::{batch, create_slo, keys_of, list, window, DURATION_UNITS};
use crate::slo_lifecycle::{router, ACCOUNT, REGION};

/// The regular expression a refusal reports for an `SloIds` entry: the modelled
/// `ServiceLevelObjectiveId` `@pattern`
/// (`smithy_query(query="shape",shape="ServiceLevelObjectiveId")`) without the OUTER anchors
/// the model spells it with, and with the anchors that separate the alternation's two
/// branches kept, since those are part of the expression rather than a wrapper around it.
const REPORTED_PATTERN: &str = "[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$|\
     ^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/\
     [0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]";

/// The shortest and longest `ServiceLevelObjectiveName` the pattern above admits: two
/// anchored alphanumeric characters, plus up to 126 between them.
const NAME_LENGTH_MIN: usize = 2;
const NAME_LENGTH_MAX: usize = 128;

/// The exception a request whose every id names no stored SLO is refused with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A name of exactly `length` characters, alphanumeric at both ends, which the pattern
/// admits for every `length` in [`NAME_LENGTH_MIN`]..=[`NAME_LENGTH_MAX`].
fn name_of_length(length: usize) -> String {
    match length {
        0 => String::new(),
        1 => "a".to_string(),
        _ => format!("a{}z", "b".repeat(length - 2)),
    }
}

/// The SLO ARN of `name` in `partition` and `region`, for this account.
fn slo_arn_in(partition: &str, region: &str, name: &str) -> String {
    format!("arn:{partition}:application-signals:{region}:{ACCOUNT}:slo/{name}")
}

/// The SLO ARN of `name` in the partition and region every request of this suite is signed
/// for.
fn slo_arn(name: &str) -> String {
    slo_arn_in("aws", REGION, name)
}

/// The two operations the model binds `ServiceLevelObjectiveIds` to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Batch {
    UpdateExclusionWindows,
    GetBudgetReport,
}

impl Batch {
    const ALL: [Self; 2] = [Self::UpdateExclusionWindows, Self::GetBudgetReport];
}

/// Send `ids` to `operation` as its `SloIds`, with every other member of the request valid,
/// so the only thing a row can be refused for is the id form it sets out to probe.
async fn send(router: &MultiServiceRouter, operation: Batch, ids: &[String]) -> (u16, Value) {
    match operation {
        Batch::UpdateExclusionWindows => {
            let add = window(DURATION_UNITS[1], 1, None, None, None);
            batch(router, ids, std::slice::from_ref(&add), &[]).await
        }
        Batch::GetBudgetReport => budget_report(router, ids, now_seconds()).await,
    }
}

/// The message a refusal of an `SloIds` entry must carry: the service's one
/// constraint-violation envelope over one clause naming the member and the requirement.
fn expected_message() -> String {
    format!(
        "1 validation error detected: Value at 'sloIds' failed to satisfy constraint: Member \
         must satisfy regular expression pattern: {REPORTED_PATTERN}"
    )
}

/// The error code a refusal reports, with any namespace prefix the envelope carries dropped.
fn error_code(answer: &Value) -> Option<String> {
    answer["__type"]
        .as_str()
        .map(|kind| kind.rsplit('#').next().unwrap_or(kind).to_string())
}

/// The ids no `ServiceLevelObjectiveId` admits, each labelled by the way it fails the
/// pattern.
fn malformed_ids() -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        (
            "one character, under the length minimum",
            name_of_length(NAME_LENGTH_MIN - 1),
        ),
        (
            "one character over the length maximum",
            name_of_length(NAME_LENGTH_MAX + 1),
        ),
        ("a character the name class excludes", "bad!name".to_string()),
        ("a slash, which no name carries", "has/slash".to_string()),
        ("a colon, which no name carries", "has:colon".to_string()),
        ("a leading separator", "-leading".to_string()),
        ("a trailing separator", "trailing.".to_string()),
        ("a leading space", " leading".to_string()),
        ("a trailing space", "trailing ".to_string()),
        ("a multi-byte character", "caf\u{e9}-slo".to_string()),
        (
            "an ARN of another partition",
            slo_arn_in("aws-cn", "cn-north-1", "elsewhere"),
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:::slo/not-ours".to_string(),
        ),
        (
            "an ARN of another resource type",
            format!(
                "arn:aws:application-signals:{REGION}:{ACCOUNT}:instrumentationConfig/elsewhere"
            ),
        ),
        ("an SLO ARN with no name", slo_arn("")),
        (
            "an SLO ARN whose name is under the length minimum",
            slo_arn(&name_of_length(NAME_LENGTH_MIN - 1)),
        ),
        (
            "an SLO ARN whose name carries a character the class excludes",
            slo_arn("bad!name"),
        ),
    ]
}

/// Every operation that takes `ServiceLevelObjectiveIds` refuses an entry that is no
/// `ServiceLevelObjectiveId` with one HTTP 400 `ValidationException`, wherever in the list
/// the entry sits and whichever way it fails the pattern.
#[tokio::test]
async fn both_batches_refuse_an_slo_id_entry_the_pattern_does_not_admit() {
    let router = router();
    let stored = "id-form-stored";
    create_slo(&router, stored).await;
    let expected = expected_message();

    for (label, id) in malformed_ids() {
        // The positions differ along where the offending entry sits: alone, after an id that
        // names a stored SLO (so the request would otherwise have applied), and before it.
        let lists = vec![
            ("alone", vec![id.clone()]),
            ("after a stored id", vec![stored.to_string(), id.clone()]),
            ("before a stored id", vec![id.clone(), stored.to_string()]),
        ];
        for (position, ids) in lists {
            for operation in Batch::ALL {
                let case = format!("{label} / {position} / {operation:?}");
                let (status, answer) = send(&router, operation, &ids).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for the id {id:?}, got {status} with \
                     {answer}"
                );
                assert_eq!(
                    error_code(&answer),
                    Some("ValidationException".to_string()),
                    "case {case:?}: expected a ValidationException, got {answer}"
                );
                let message = answer["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the one constraint-violation wording, got \
                     {message:?}"
                );
                // The refusal states the member and the requirement and nothing of the
                // rejected value — asserted on the rows whose value the reported expression
                // does not itself contain.
                if !id.is_empty() && !REPORTED_PATTERN.contains(id.as_str()) {
                    assert!(
                        !message.contains(id.as_str()),
                        "case {case:?}: the message must not echo the rejected value {id:?}, \
                         got {message:?}"
                    );
                }
            }
        }
    }
}

/// A refused batch applies nothing: the window set of every SLO the request also named is
/// exactly what it was before the call, whichever way the offending entry failed the pattern.
#[tokio::test]
async fn a_refused_batch_leaves_every_window_set_as_it_was() {
    let router = router();
    let stored = "id-form-untouched";
    create_slo(&router, stored).await;

    let kept = window(DURATION_UNITS[2], 3, Some(1_700_000_000), None, Some("Kept"));
    let (status, applied) = batch(
        &router,
        &[stored.to_string()],
        std::slice::from_ref(&kept),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the setup add to succeed, got {status} with {applied}"
    );
    let (_, before) = list(&router, stored, "").await;
    let before = keys_of(&before);

    for (label, id) in malformed_ids() {
        let ids = vec![stored.to_string(), id.clone()];
        // The removal names the window the SLO does hold, so a request that was not refused
        // for the id form would take it away and be visible in the read below.
        let (status, answer) = batch(&router, &ids, &[], std::slice::from_ref(&kept)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected the batch to be refused, got {status} with {answer}"
        );
        let (status, page) = list(&router, stored, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the read to succeed, got {status} with {page}"
        );
        assert_eq!(
            keys_of(&page),
            before,
            "case {label:?}: expected the window set to be untouched by the refused batch, \
             got {page}"
        );
    }
}

/// Every form the modelled pattern DOES admit is resolved rather than refused: an id naming a
/// stored SLO is served, and a well-formed id naming none earns the resolution failure each
/// operation reports for an absent SLO — never the id-form refusal.
#[tokio::test]
async fn every_id_form_the_pattern_admits_is_resolved() {
    let router = router();
    // The names differ along what the name class generalises over: each published length
    // bound, and every separator the class admits between the anchored characters.
    let names = vec![
        ("the shortest name", name_of_length(NAME_LENGTH_MIN)),
        ("the longest name", name_of_length(NAME_LENGTH_MAX)),
        ("every separator the class admits", "a-b.c_d e9".to_string()),
    ];
    for (_, name) in &names {
        create_slo(&router, name).await;
    }

    // The id forms of a stored SLO: the bare name, its ARN, and its ARN in the other
    // partition the pattern admits — neither the partition nor the region is part of what an
    // ARN identifies here, so all three name the one record.
    let mut served: Vec<(String, String)> = Vec::new();
    for (label, name) in &names {
        served.push((format!("{label}, by name"), name.clone()));
        served.push((format!("{label}, by ARN"), slo_arn(name)));
        served.push((
            format!("{label}, by an ARN of the other partition"),
            slo_arn_in("aws-us-gov", "us-gov-west-1", name),
        ));
    }

    for (label, id) in &served {
        for operation in Batch::ALL {
            let case = format!("{label} / {operation:?}");
            let (status, answer) = send(&router, operation, std::slice::from_ref(id)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the id {id:?} to be served, got {status} with {answer}"
            );
        }
    }

    // A well-formed id that names no stored SLO passed the form check and failed to resolve:
    // the batch update has no applied id to report, so it answers the exception the absent
    // SLO earns, and the budget report states that failure per item.
    for (label, id) in [
        ("an absent SLO by name", "id-form-absent".to_string()),
        ("an absent SLO by ARN", slo_arn("id-form-absent-by-arn")),
    ] {
        let (status, answer) =
            send(&router, Batch::UpdateExclusionWindows, std::slice::from_ref(&id)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected the absent SLO to be reported as not found, got {status} \
             with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            Some(NOT_FOUND.to_string()),
            "case {label:?}: expected a {NOT_FOUND}, got {answer}"
        );

        let (status, answer) =
            send(&router, Batch::GetBudgetReport, std::slice::from_ref(&id)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the report to state the failure per item, got {status} \
             with {answer}"
        );
        assert_eq!(
            answer["Errors"][0]["ErrorCode"].as_str(),
            Some(CALLER_ERROR_CODE),
            "case {label:?}: expected the per-item error to report {CALLER_ERROR_CODE}, got {answer}"
        );
    }
}
