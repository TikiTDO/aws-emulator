// SPDX-License-Identifier: CC0-1.0

//! The `State` view an enumerated IAM Identity Center instance carries around the READ
//! edge cases a client makes on the instance surface, over the `awsJson1_1` wire.
//!
//! THE RULE: the instance projection an enumeration serves is a function of the stored
//! instance record alone, so every entry carries a `State` structure whose `Name` is that
//! entry's own `Status` — and because an enumeration and a single-instance read change
//! nothing a later caller sees, whatever read a client made before it (served or refused,
//! on the instance it holds or on one it does not, on whichever page size) leaves the next
//! enumeration serving exactly the projection it would have served without it.
//!
//! `instance_state_view.rs` pins that the member is served and mirrors the status through
//! the lifecycle, `listed_instance_state_source.rs` the RECORD it is read from,
//! `listed_instance_state_under_any_name.rs` the `Name` value space and
//! `listed_instance_state_after_instance_writes.rs` the WRITE paths of the record. This
//! suite spans the READ paths and their edge cases, the sequence a probing client actually
//! makes: a single-instance read of the instance, the same read on an `InstanceArn` the
//! account holds no instance for (refused), an enumeration continued through its
//! `NextToken`, a page size outside the published range of `MaxResults` (refused) and a
//! continuation token this enumeration never issued (refused). Each row is driven in both
//! phases an instance a client just created passes through, and each row's projection is
//! compared with the projection served BEFORE the read, so a view that is correct only on
//! a store nobody has read yet fails here.
//!
//! The rows' bounds are the model's own: `MaxResults` is `range(min: 1, max: 100)`
//! (`smithy_query(query="operation",operation="ListInstances")`), so 1 and 100 are inside
//! it and 0 and 101 one step outside.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, the enumeration and the rule assertion the ones
//! `instance_state_view.rs` owns, and the empty account and the held activation instant the
//! ones `instance_create.rs` owns. Where a phase lives on the far side of a due instant the
//! test moves the STORED instant rather than sleeping — the transition is still applied by
//! the service's own `materialize_due`, exactly as in production.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    STATUS_ACTIVE, STATUS_CREATE_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::{empty_account_router, hold_activation};
use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{call_in_region, error_code, UNKNOWN_INSTANCE_ARN};

/// The Region the instance is enabled in and every call below is signed for: a `RegionName`
/// (`^([a-z]+-){2,3}\d$`) none of the sibling suites uses.
const REGION: &str = "me-south-1";

/// One read a client makes before the enumeration whose entry is asserted.
#[derive(Debug, Clone, Copy)]
enum ReadEdgeCase {
    /// No read at all: the projection as the account's own create left it.
    NoPriorRead,
    /// `DescribeInstance` on the instance the account holds — a served single-instance read.
    DescribeTheInstance,
    /// `DescribeInstance` on an `InstanceArn` the account holds no instance for.
    DescribeAnUnknownInstance,
    /// An enumeration driven at the smallest page the model admits and continued through
    /// every `NextToken` it hands out, to the empty page that ends it.
    EnumerateAtTheSmallestPage,
    /// The same at the largest page the model admits.
    EnumerateAtTheLargestPage,
    /// `ListInstances` with a page size one step below the published range.
    PageSizeBelowTheRange,
    /// `ListInstances` with a page size one step above it.
    PageSizeAboveTheRange,
    /// `ListInstances` continued from a token this enumeration never issued.
    TokenThisEnumerationNeverIssued,
}

/// Every read edge case the rows span.
const READS: [ReadEdgeCase; 8] = [
    ReadEdgeCase::NoPriorRead,
    ReadEdgeCase::DescribeTheInstance,
    ReadEdgeCase::DescribeAnUnknownInstance,
    ReadEdgeCase::EnumerateAtTheSmallestPage,
    ReadEdgeCase::EnumerateAtTheLargestPage,
    ReadEdgeCase::PageSizeBelowTheRange,
    ReadEdgeCase::PageSizeAboveTheRange,
    ReadEdgeCase::TokenThisEnumerationNeverIssued,
];

/// `CreateInstance` on an account that holds none, as the created instance's ARN.
///
/// The `Name` is the shape of a probing client's generated name — a prefix and a hex
/// suffix the caller minted, admissible under `NameType`'s `^[\w+=,.@-]+$` — so nothing
/// about the view can be keyed on a name the service chose.
async fn create(router: &MultiServiceRouter) -> String {
    let name = format!("read-edge-idc-{:08x}", 0x2b71_f4e6_u32);
    let (status, body) =
        call_in_region(router, REGION, "CreateInstance", &json!({"Name": name})).await;
    assert_eq!(
        status, 200,
        "CreateInstance expected 200, got {status} {body}"
    );
    body["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no InstanceArn in {body}"))
        .to_string()
}

/// The one entry the account's enumeration serves, with THE RULE asserted on it: its
/// `State` is a structure whose `Name` is its own `Status`, and that status is the one the
/// phase owes.
async fn sole_entry(
    case: &str,
    router: &MultiServiceRouter,
    instance_arn: &str,
    expected_status: &str,
) -> Value {
    let entries = listed(case, router, REGION, None).await;
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: the account's one instance is enumerated, got {entries:?}"
    );
    let entry = entries.into_iter().next().expect("one entry");
    assert_eq!(
        entry["InstanceArn"].as_str(),
        Some(instance_arn),
        "case {case:?}: expected the created instance, got {entry}"
    );
    assert_state_mirrors_status(case, &entry);
    assert_eq!(
        entry["Status"].as_str(),
        Some(expected_status),
        "case {case:?}: expected Status {expected_status:?}, got {entry}"
    );
    assert_eq!(
        entry["State"]["Name"].as_str(),
        Some(expected_status),
        "case {case:?}: expected State.Name {expected_status:?}, got {entry}"
    );
    entry
}

/// One `ListInstances` refusal: HTTP 400 carrying the modelled `ValidationException`, the
/// channel this awsJson1_1 service answers a malformed request on.
async fn assert_refused_list(case: &str, router: &MultiServiceRouter, request: &Value) {
    let (status, body) = call_in_region(router, REGION, "ListInstances", request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for {request}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException for {request}, got {body}"
    );
}

/// Make the row's read over the wire, asserting it is served or refused as the surface
/// models it — every one of them a READ, so none of them may leave a mark.
async fn make_read(
    case: &str,
    read: ReadEdgeCase,
    router: &MultiServiceRouter,
    instance_arn: &str,
    expected_status: &str,
) {
    match read {
        ReadEdgeCase::NoPriorRead => {}
        ReadEdgeCase::DescribeTheInstance => {
            let (status, body) = call_in_region(
                router,
                REGION,
                "DescribeInstance",
                &json!({"InstanceArn": instance_arn}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeInstance expected 200, got {status} {body}"
            );
            // The single-instance read and the listed view report ONE lifecycle.
            assert_eq!(
                body["Status"].as_str(),
                Some(expected_status),
                "case {case:?}: expected Status {expected_status:?}, got {body}"
            );
            assert_eq!(
                body["Instance"]["State"]["Name"].as_str(),
                Some(expected_status),
                "case {case:?}: expected the envelope's State.Name {expected_status:?}, got \
                 {body}"
            );
        }
        ReadEdgeCase::DescribeAnUnknownInstance => {
            // `DescribeInstance` declares no `ResourceNotFoundException`, so the channel for
            // an instance this account does not hold is the `AccessDeniedException` every
            // operation of this service declares (`unknown_instance_channel.rs`).
            let (status, body) = call_in_region(
                router,
                REGION,
                "DescribeInstance",
                &json!({"InstanceArn": UNKNOWN_INSTANCE_ARN}),
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected HTTP 403 for an instance the account does not hold, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {body}"
            );
        }
        ReadEdgeCase::EnumerateAtTheSmallestPage => {
            // The page size's published minimum, continued to the page that ends the
            // enumeration: every entry on every page carries the view.
            for entry in listed(case, router, REGION, Some(1)).await {
                assert_state_mirrors_status(case, &entry);
            }
        }
        ReadEdgeCase::EnumerateAtTheLargestPage => {
            for entry in listed(case, router, REGION, Some(100)).await {
                assert_state_mirrors_status(case, &entry);
            }
        }
        ReadEdgeCase::PageSizeBelowTheRange => {
            assert_refused_list(case, router, &json!({"MaxResults": 0})).await;
        }
        ReadEdgeCase::PageSizeAboveTheRange => {
            assert_refused_list(case, router, &json!({"MaxResults": 101})).await;
        }
        ReadEdgeCase::TokenThisEnumerationNeverIssued => {
            assert_refused_list(case, router, &json!({"NextToken": "not-a-token"})).await;
        }
    }
}

/// THE RULE over the read edge cases: in both phases a just-created instance passes
/// through, the enumeration serves its `State` view mirroring the status the record holds,
/// and the projection is the same one it served before the read.
#[tokio::test]
async fn every_read_edge_case_leaves_the_listed_state_view_in_place() {
    for read in READS {
        let (router, engine) = empty_account_router();
        let instance_arn = create(&router).await;

        // The two phases the clock takes the record through: the creation still in flight,
        // and the activation it owes fallen due.
        for in_flight in [true, false] {
            let (phase, due_at, expected_status) = if in_flight {
                (
                    "creation in flight",
                    SystemTime::now() + Duration::from_secs(3_600),
                    STATUS_CREATE_IN_PROGRESS,
                )
            } else {
                (
                    "activation due",
                    SystemTime::now() - Duration::from_secs(1),
                    STATUS_ACTIVE,
                )
            };
            hold_activation(&engine, due_at);
            let case = format!("{read:?} / {phase}");

            let before = sole_entry(&case, &router, &instance_arn, expected_status).await;
            make_read(&case, read, &router, &instance_arn, expected_status).await;
            let after = sole_entry(&case, &router, &instance_arn, expected_status).await;
            assert_eq!(
                before, after,
                "case {case:?}: a read changes nothing a later caller sees, got {after} after \
                 {before}"
            );
        }
    }
}
