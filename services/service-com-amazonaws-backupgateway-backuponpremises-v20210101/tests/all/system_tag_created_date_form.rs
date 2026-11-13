// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FORM of the value the AWS-assigned creation-date tag
//! (`aws:createdDate`) carries on a tag read.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the resource helpers of
//! `aws_assigned_system_tags`, so what is asserted is the wire behaviour and the only way a
//! later request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: the AWS-assigned creation-date tag of a resource this API created
//! reports that record's own creation instant as a WHOLE NUMBER OF MILLISECONDS SINCE THE
//! UNIX EPOCH, written in decimal digits and nothing else — no date-time punctuation, no
//! zone designator, no fraction. The layout is the fixed part of the value; the instant is
//! the variable part and is read off the record, so a later read of one resource reports the
//! same digits whatever the client does to its own tags. The `Tags` list carries a string,
//! so the digits are the value's whole text.
//!
//! The table below spans that class rather than one recorded request: both resource types a
//! request of this model creates (a gateway through `CreateGateway`, a hypervisor through
//! `ImportHypervisorConfiguration`), each with no client tags, with client tags carried BY
//! the create, with client tags written afterwards and with a client key removed afterwards
//! — the whole set of client-tag histories a resource can have when its creation date is
//! read.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, CREATED_DATE_TAG_KEY};

use crate::aws_assigned_system_tags::{create_resource, reported_tags, tag_body};
use crate::gateway_lifecycle::{build_engine, build_router, call};

/// The band every epoch-millisecond value of an instant between 2001-09-09T01:46:40Z and
/// 2286-11-20T17:46:40Z falls in — the bound that tells the UNIT of a decimal creation date
/// apart from its neighbours: the same instant counted in epoch SECONDS is ten digits and
/// below this floor, counted in MICROSECONDS sixteen digits and above this ceiling. Derived
/// from the Unix epoch itself, not from any recorded value.
const EPOCH_MILLIS_FLOOR: i64 = 1_000_000_000_000;
const EPOCH_MILLIS_CEILING: i64 = 9_999_999_999_999;

/// One row: which resource type is created, the tags the create itself carries, the tags a
/// later `TagResource` writes and the keys a later `UntagResource` removes.
struct Row {
    case: &'static str,
    kind: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    tagged_with: &'static [(&'static str, &'static str)],
    untagged: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a gateway with no client tags",
            kind: "gateway",
            created_with: &[],
            tagged_with: &[],
            untagged: &[],
        },
        Row {
            case: "a gateway whose create carried two client tags",
            kind: "gateway",
            created_with: &[("created-k1", "v1"), ("created-k2", "")],
            tagged_with: &[],
            untagged: &[],
        },
        Row {
            case: "a gateway tagged and then untagged afterwards",
            kind: "gateway",
            created_with: &[("env", "test")],
            tagged_with: &[("team", "core")],
            untagged: &["env"],
        },
        Row {
            case: "a hypervisor with no client tags",
            kind: "hypervisor",
            created_with: &[],
            tagged_with: &[],
            untagged: &[],
        },
        Row {
            case: "a hypervisor whose import carried a client tag, tagged again afterwards",
            kind: "hypervisor",
            created_with: &[("env", "test")],
            tagged_with: &[("Alpha", "first"), ("zulu", "last")],
            untagged: &[],
        },
        Row {
            case: "a hypervisor untagged with a key it never carried",
            kind: "hypervisor",
            created_with: &[],
            tagged_with: &[],
            untagged: &["never-set"],
        },
    ]
}

/// The value the reported tag list carries for [`CREATED_DATE_TAG_KEY`], asserting the entry
/// is there at all.
async fn reported_created_date(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let reported = reported_tags(router, case, arn).await;
    reported
        .iter()
        .find(|(key, _)| key == CREATED_DATE_TAG_KEY)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: no {CREATED_DATE_TAG_KEY} entry among {reported:?}")
        })
}

/// The creation instant the STORE holds for `arn`, as whole milliseconds since the epoch:
/// what the reported value has to be the rendering of, read off the record rather than off
/// the test's clock.
fn persisted_created_millis(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    kind: &str,
    arn: &str,
) -> i64 {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    let created_time = match kind {
        "gateway" => store.gateway(arn).map(|gateway| gateway.created_time),
        "hypervisor" => store.hypervisor(arn).map(|hypervisor| hypervisor.created_time),
        other => panic!("case {case:?}: unknown resource kind {other:?}"),
    }
    .unwrap_or_else(|| panic!("case {case:?}: no {kind} stored under {arn}"));
    let since_epoch = created_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_else(|_| panic!("case {case:?}: the record's creation instant is past the epoch"));
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// Assert `value` is written the one way the rule allows: decimal digits, nothing else, and
/// a count of milliseconds rather than of seconds or of microseconds.
///
/// Shared with the sibling `aws_assigned_system_tags` suite, so the rule is asserted one way
/// wherever a creation date is read.
pub(crate) fn assert_epoch_millis_form(case: &str, value: &str) -> i64 {
    assert!(
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: expected decimal digits and nothing else, got {value:?}"
    );
    assert!(
        !value.starts_with('0'),
        "case {case:?}: expected a count with no leading zero, got {value:?}"
    );
    let millis: i64 = value
        .parse()
        .unwrap_or_else(|error| panic!("case {case:?}: expected a number, got {value:?} ({error})"));
    assert!(
        (EPOCH_MILLIS_FLOOR..=EPOCH_MILLIS_CEILING).contains(&millis),
        "case {case:?}: expected epoch milliseconds in \
         {EPOCH_MILLIS_FLOOR}..={EPOCH_MILLIS_CEILING}, got {millis}"
    );
    millis
}

/// The creation-date tag of every resource this API creates reports that record's own
/// creation instant as decimal epoch milliseconds, whatever the resource type and whatever
/// the client has done to its own tags.
#[tokio::test]
async fn the_creation_date_tag_reports_epoch_milliseconds() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut reported_in_order: Vec<(&str, i64)> = Vec::new();
    for (nonce, row) in rows().iter().enumerate() {
        let arn = create_resource(&router, row.kind, 100 + nonce, row.created_with).await;

        if !row.tagged_with.is_empty() {
            let written = call(
                &router,
                "TagResource",
                &json!({ "ResourceARN": arn, "Tags": tag_body(row.tagged_with) }),
            )
            .await;
            assert_eq!(
                written.status, 200,
                "case {:?}: TagResource must answer 200, got {} {:?}",
                row.case, written.status, written.body
            );
        }
        if !row.untagged.is_empty() {
            let removed = call(
                &router,
                "UntagResource",
                &json!({ "ResourceARN": arn, "TagKeys": row.untagged }),
            )
            .await;
            assert_eq!(
                removed.status, 200,
                "case {:?}: UntagResource must answer 200, got {} {:?}",
                row.case, removed.status, removed.body
            );
        }

        let reported = reported_created_date(&router, row.case, &arn).await;
        let millis = assert_epoch_millis_form(row.case, &reported);
        assert_eq!(
            millis,
            persisted_created_millis(&engine, row.case, row.kind, &arn),
            "case {:?}: expected the record's own creation instant in milliseconds, got \
             {reported:?}",
            row.case
        );

        // A read reports the same digits again: the value is a fact about the record, not
        // something minted per call.
        let read_again = reported_created_date(&router, row.case, &arn).await;
        assert_eq!(
            read_again, reported,
            "case {:?}: a second read must report the same value, got {read_again:?} after \
             {reported:?}",
            row.case
        );

        reported_in_order.push((row.case, millis));
    }

    // The rows were created one after another, so their reported counts never fall: a value
    // that is really the record's creation instant in one unit cannot go backwards across
    // creations.
    assert!(
        reported_in_order
            .windows(2)
            .all(|pair| pair[0].1 <= pair[1].1),
        "expected non-decreasing creation dates across resources created in order, got \
         {reported_in_order:?}"
    );
}

/// The rendering is ONE renderer: a gateway and a hypervisor report their creation dates in
/// the same form, and the form survives a client tag write — the client's own values are
/// reported verbatim beside it and never change it.
#[tokio::test]
async fn one_rendering_serves_every_resource_type() {
    let engine = build_engine();
    let router = build_router(&engine);

    for (nonce, kind) in ["gateway", "hypervisor"].into_iter().enumerate() {
        let case = kind;
        let arn = create_resource(&router, kind, 200 + nonce, &[]).await;
        let before = reported_created_date(&router, case, &arn).await;
        assert_epoch_millis_form(case, &before);

        // A client value that LOOKS like a date-time is reported exactly as it was written,
        // and the AWS-assigned entry beside it keeps its own form.
        let client_value = "2026-10-30T21:47:26Z";
        let written = call(
            &router,
            "TagResource",
            &json!({
                "ResourceARN": arn,
                "Tags": tag_body(&[("client-date", client_value)]),
            }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: TagResource must answer 200, got {} {:?}",
            written.status, written.body
        );

        let reported = reported_tags(&router, case, &arn).await;
        let client = reported
            .iter()
            .find(|(key, _)| key == "client-date")
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("case {case:?}: no client tag among {reported:?}"));
        assert_eq!(
            client, client_value,
            "case {case:?}: a client tag value must be reported verbatim, got {client:?}"
        );

        let after = reported_created_date(&router, case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a client tag write must not change the creation date, got \
             {after:?} after {before:?}"
        );
    }
}
