// SPDX-License-Identifier: CC0-1.0

//! The `State` view an enumerated IAM Identity Center instance carries once the instance
//! RECORD has been written, over the `awsJson1_1` wire.
//!
//! THE RULE: the instance projection an enumeration serves is built from the stored
//! instance record as it stands at the instant of the read, so every entry carries a
//! `State` structure whose `Name` is that entry's own `Status` — whatever call last wrote
//! the record, and whichever Region the enumerating request is signed for, since neither
//! the writer nor the reader's Region is part of the projection.
//!
//! `instance_state_view.rs` pins that the member is SERVED and mirrors the status through
//! the lifecycle, `listed_instance_state_source.rs` the RECORD it is read from, and
//! `listed_instance_state_under_any_name.rs` the `Name` value space. This suite spans the
//! WRITE PATHS of the instance record — every operation of this service that writes one:
//! `UpdateInstance` (renaming it, and enabling permission sets), `TagResource`,
//! `UntagResource`, `AddRegion`, `RemoveRegion` and `DeleteInstance` — crossed with the
//! Region the enumeration is made from and the page size it asks for, because a projection
//! rebuilt after a write is where a member that is not in the on-disk model can go missing
//! again. The second test adds the write that is REFUSED: the record a refused call leaves
//! untouched is still served with its view.
//!
//! Replicating an instance across Regions is a feature of the organization instance, so
//! `AddRegion` and `RemoveRegion` are driven against one (`organization_engine`) — the same
//! instance every other row writes to, so the rows differ only in the write that was made.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, the enumeration and the rule assertion the ones
//! `instance_state_view.rs` owns, and the `CREATE_IN_PROGRESS` fixture the one
//! `instance_create_in_progress_writes.rs` owns. Where a phase lives on the far side of a
//! due instant the test moves the STORED instant rather than sleeping — the transition is
//! still applied by the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    STATUS_ACTIVE, STATUS_CREATE_IN_PROGRESS, STATUS_DELETE_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::hold_activation;
use crate::instance_create_in_progress_writes::creating_instance;
use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{
    call_in_region, error_code, hold_deletion, hold_region_activation, organization_engine,
    served_in_region,
    router_over, TEST_REGION,
};

/// The Region the organization instance is enabled in, and the Region every WRITE below is
/// made from: `organization_engine` enables the instance in the harness Region, which is
/// therefore its primary Region — the Region `RemoveRegion` documents a remove must be made
/// from.
const PRIMARY_REGION: &str = TEST_REGION;

/// The ADDITIONAL Region the Region writes add and remove: a `RegionName`
/// (`^([a-z]+-){2,3}\d$`) other than the primary one.
const ADDITIONAL_REGION: &str = "eu-west-3";

/// The Regions the ENUMERATION is made from: the instance's own primary Region, and a
/// Region it is not enabled in — the Region a read is signed for decides nothing about the
/// projection it is served.
///
/// WHICH instances an enumeration reports is a separate rule, and not this suite's subject:
/// the service enumerates the account's instances and reads the request's Region only to
/// settle the account's enablement and apply what the clock owes
/// (`state::ComAmazonawsSsoadminSwbexternalserviceState::on_request`). The rows below assert
/// only that an instance an enumeration DOES report carries its `State` view either way.
const READ_REGIONS: [&str; 2] = [PRIMARY_REGION, "ap-southeast-2"];

/// The tag key the tag rows write and the untag row strips.
const TAG_KEY: &str = "Owner";

/// One write of the instance record, made over the wire before the enumeration is read.
#[derive(Debug, Clone, Copy)]
enum InstanceWrite {
    /// No write at all: the instance as the account holds it.
    NoWrite,
    /// `UpdateInstance` renaming the instance.
    Rename,
    /// `UpdateInstance` turning the permission-set surface on.
    EnablePermissionSets,
    /// `TagResource` on the instance itself.
    Tag,
    /// `UntagResource` stripping the key a `TagResource` just wrote.
    Untag,
    /// `AddRegion` enabling the instance in one more Region.
    AddRegion,
    /// `RemoveRegion` taking that additional Region away again.
    RemoveRegion,
    /// `DeleteInstance`, with the removal the clock owes held open so the record is still
    /// enumerated in the status the delete left it in.
    Delete,
}

/// Every write path of the instance record this service serves.
const WRITES: [InstanceWrite; 8] = [
    InstanceWrite::NoWrite,
    InstanceWrite::Rename,
    InstanceWrite::EnablePermissionSets,
    InstanceWrite::Tag,
    InstanceWrite::Untag,
    InstanceWrite::AddRegion,
    InstanceWrite::RemoveRegion,
    InstanceWrite::Delete,
];

/// One row of the REFUSED-write table: a write of the instance record, made while the
/// instance's creation has not finished.
struct RefusedRow {
    label: &'static str,
    operation: &'static str,
    /// The request body, built from the created instance's ARN.
    body: fn(&str) -> Value,
}

/// One call made from the primary Region, asserted served — the harness's own
/// [`served_in_region`] bound to this suite's Region.
async fn served(case: &str, router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    served_in_region(case, router, PRIMARY_REGION, operation, body).await
}

/// The `TagResource` request one tag row makes: the instance tagged under itself.
fn tag_request(instance_arn: &str) -> Value {
    json!({
        "InstanceArn": instance_arn,
        "ResourceArn": instance_arn,
        "Tags": [{"Key": TAG_KEY, "Value": "identity"}],
    })
}

/// Make the write over the wire and answer the status the record then holds.
async fn apply_write(
    write: InstanceWrite,
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
) -> &'static str {
    let case = format!("{write:?}");
    let add_region = json!({"InstanceArn": instance_arn, "RegionName": ADDITIONAL_REGION});
    match write {
        InstanceWrite::NoWrite => {}
        InstanceWrite::Rename => {
            served(
                &case,
                router,
                "UpdateInstance",
                &json!({"InstanceArn": instance_arn, "Name": "WrittenInstance"}),
            )
            .await;
        }
        InstanceWrite::EnablePermissionSets => {
            served(
                &case,
                router,
                "UpdateInstance",
                &json!({"InstanceArn": instance_arn, "PermissionSetsEnabled": true}),
            )
            .await;
        }
        InstanceWrite::Tag => {
            served(&case, router, "TagResource", &tag_request(instance_arn)).await;
        }
        InstanceWrite::Untag => {
            served(&case, router, "TagResource", &tag_request(instance_arn)).await;
            served(
                &case,
                router,
                "UntagResource",
                &json!({
                    "InstanceArn": instance_arn,
                    "ResourceArn": instance_arn,
                    "TagKeys": [TAG_KEY],
                }),
            )
            .await;
        }
        InstanceWrite::AddRegion => {
            served(&case, router, "AddRegion", &add_region).await;
        }
        InstanceWrite::RemoveRegion => {
            served(&case, router, "AddRegion", &add_region).await;
            // `RemoveRegion` documents that "no other add or remove Region workflows can be
            // in progress", so the activation the add owes is made due; the service's own
            // transition applies it on the next request, leaving the Region with nothing
            // owing for the remove to collide with.
            hold_region_activation(
                engine,
                instance_arn,
                ADDITIONAL_REGION,
                SystemTime::now() - Duration::from_secs(1),
            );
            served(
                &case,
                router,
                "RemoveRegion",
                &json!({"InstanceArn": instance_arn, "RegionName": ADDITIONAL_REGION}),
            )
            .await;
        }
        InstanceWrite::Delete => {
            served(
                &case,
                router,
                "DeleteInstance",
                &json!({"InstanceArn": instance_arn}),
            )
            .await;
            // The removal the clock owes is held open, so the record is read in the status
            // the delete left it in rather than after it has gone.
            hold_deletion(engine, SystemTime::now() + Duration::from_secs(3_600));
            return STATUS_DELETE_IN_PROGRESS;
        }
    }
    STATUS_ACTIVE
}

/// THE RULE on the one instance the account holds, asserted over every Region the
/// enumeration can be made from and every page size the rows need: the entry's `State` is a
/// structure whose `Name` is that entry's own `Status`.
async fn assert_listed_view(
    write: InstanceWrite,
    router: &MultiServiceRouter,
    instance_arn: &str,
    expected_status: &str,
) {
    for read_region in READ_REGIONS {
        // `MaxResults` is `range 1..100`: the smallest page the model admits, and the
        // request that names no page size at all.
        for max_results in [None, Some(1)] {
            let case = format!("{write:?} / read in {read_region} / MaxResults {max_results:?}");
            let entries = listed(&case, router, read_region, max_results).await;
            assert_eq!(
                entries.len(),
                1,
                "case {case:?}: the account's one instance is enumerated, got {entries:?}"
            );
            let entry = &entries[0];
            assert_eq!(
                entry["InstanceArn"].as_str(),
                Some(instance_arn),
                "case {case:?}: expected the written instance, got {entry}"
            );
            assert_state_mirrors_status(&case, entry);
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
        }
    }
}

/// THE RULE over the write paths of the instance record: after each of them the enumeration
/// still serves the instance's `State` view, mirroring the status the record then holds, in
/// every Region a client can read from.
#[tokio::test]
async fn every_instance_write_leaves_the_listed_state_view_in_place() {
    for write in WRITES {
        let (engine, instance_arn) = organization_engine();
        let router = router_over(Arc::clone(&engine));
        let expected_status = apply_write(write, &router, &engine, &instance_arn).await;

        assert_listed_view(write, &router, &instance_arn, expected_status).await;

        // And the single-instance read of the same record agrees about the lifecycle, so a
        // client that waits on `State` through either call sees one answer.
        let case = format!("{write:?} / DescribeInstance");
        let described = served(
            &case,
            &router,
            "DescribeInstance",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        assert_eq!(
            described["Status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected Status {expected_status:?}, got {described}"
        );
        assert_eq!(
            described["Instance"]["State"]["Name"].as_str(),
            Some(expected_status),
            "case {case:?}: expected the envelope's State.Name {expected_status:?}, got \
             {described}"
        );
    }
}

/// The same rule where the write is REFUSED: an instance whose creation has not finished
/// serves only its own describe and delete, so each of these calls is refused and writes
/// nothing — and the record it left alone is still enumerated with its `State` view, first
/// reporting the creation that is in flight and then the activation the clock made due.
#[tokio::test]
async fn a_refused_write_leaves_the_listed_state_view_on_the_record_it_found() {
    // The writes of the instance record that are available on an ACCOUNT instance — the
    // kind `CreateInstance` mints — so each row is refused for the window rather than for
    // the kind of instance it names. Each row builds its request from the created
    // instance's ARN.
    let rows: [RefusedRow; 3] = [
        RefusedRow {
            label: "UpdateInstance renaming the instance",
            operation: "UpdateInstance",
            body: |arn| json!({"InstanceArn": arn, "Name": "RefusedRename"}),
        },
        RefusedRow {
            label: "UpdateInstance enabling permission sets",
            operation: "UpdateInstance",
            body: |arn| json!({"InstanceArn": arn, "PermissionSetsEnabled": true}),
        },
        RefusedRow {
            label: "TagResource on the instance itself",
            operation: "TagResource",
            body: tag_request,
        },
    ];

    for RefusedRow {
        label: row,
        operation,
        body,
    } in rows
    {
        let (router, engine, instance_arn) = creating_instance().await;
        let request = body(&instance_arn);

        let (status, refused) = call_in_region(&router, PRIMARY_REGION, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {row:?}: the write must be refused while the instance is being created, \
             got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {row:?}: expected ConflictException, got {refused}"
        );

        // The record the refused call left alone: still enumerated, still carrying the view,
        // reporting the creation that is in flight.
        let case = format!("{row} / creation in flight");
        let entries = listed(&case, &router, PRIMARY_REGION, None).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the account's one instance is enumerated, got {entries:?}"
        );
        for entry in &entries {
            assert_state_mirrors_status(&case, entry);
            assert_eq!(
                entry["State"]["Name"].as_str(),
                Some(STATUS_CREATE_IN_PROGRESS),
                "case {case:?}: expected State.Name {STATUS_CREATE_IN_PROGRESS:?}, got {entry}"
            );
        }

        // And once the activation the clock owes has fallen due, the same enumeration
        // reports the status the record then holds, in `State.Name` as in `Status`.
        hold_activation(&engine, SystemTime::now() - Duration::from_secs(1));
        let case = format!("{row} / activation due");
        let entries = listed(&case, &router, PRIMARY_REGION, None).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the account's one instance is enumerated, got {entries:?}"
        );
        for entry in &entries {
            assert_state_mirrors_status(&case, entry);
            assert_eq!(
                entry["State"]["Name"].as_str(),
                Some(STATUS_ACTIVE),
                "case {case:?}: expected State.Name {STATUS_ACTIVE:?}, got {entry}"
            );
        }
    }
}
