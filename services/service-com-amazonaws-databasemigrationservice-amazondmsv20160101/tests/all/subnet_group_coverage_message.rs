// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `ReplicationSubnetGroupDoesNotCoverEnoughAZs` refusal
//! `CreateReplicationSubnetGroup` answers a subnet list that does not span the minimum
//! number of Availability Zones with.
//!
//! The rule under test: that refusal carries DMS's own message for the error class — its
//! fixed text verbatim, with only the documented zone minimum, the count of DISTINCT
//! zones the subnets THIS request names sit in, and the Region's zones (as the service
//! that owns them reports them) interpolated into it. `services/.../src/ops/` is free to
//! choose neither the clauses nor the remediation the service names.
//!
//! The rows span what the rule generalises over: how many subnets a request names (none,
//! one, two, three), which zone those subnets sit in (the Region's first and its last), and
//! therefore the coverage the message reports (0 and 1) — plus the control that a list
//! which does cover the minimum is not refused at all. The zone list each case expects is
//! read from EC2 rather than written down, because it is a fact of the Region's network.

use serde_json::json;
use service_com_amazonaws_databasemigrationservice_amazondmsv20160101::state::MIN_SUBNET_GROUP_AVAILABILITY_ZONES;

use crate::migration_task_slice::{error_code, text, Emulator, REGION};
use crate::subnet_group_placement::{create_subnet, create_vpc, Subnet};

/// The error code the model declares for a group that does not span enough zones.
const NOT_ENOUGH_AZS: &str = "ReplicationSubnetGroupDoesNotCoverEnoughAZs";

/// The CIDR of the VPC every subnet below is cut out of.
const VPC_CIDR: &str = "10.40.0.0/16";

/// The message DMS serves for this error class.
///
/// The FIXED text is the service's, reproduced here as the requirement; the VARIABLE parts
/// are `min` (the minimum the operation's doc publishes), `covered` (the distinct zones the
/// request's own subnets sit in) and `zones` (the Region's zones, comma-and-space separated
/// inside square brackets).
fn expected_message(min: usize, covered: usize, zones: &[String]) -> String {
    format!(
        "Replication Subnet Group doesn't meet availability zone coverage requirement. \
         Please create a subnet group containing subnets to cover at least {min} availability \
         zones using the DMS console or CLI commands create-replication-subnet-group or \
         modify-replication-subnet-group. Current AZ coverage: {covered}. Valid availability \
         zones are: [{}]",
        zones.join(", ")
    )
}

/// One subnet list the rule is asserted over.
struct Row {
    /// What this row varies.
    case: &'static str,
    /// The subnet ids the create names.
    subnet_ids: Vec<String>,
    /// The distinct zones those subnets sit in, which the message reports as the coverage.
    covered: usize,
}

#[tokio::test]
async fn the_az_coverage_refusal_carries_the_message_dms_serves_for_it() {
    let (emulator, ec2, _iam) = Emulator::with_network();

    // The Region's zones, read from the service that owns them: both what the message has
    // to list and where the fixture's subnets are placed.
    let zones: Vec<String> = ec2
        .describe_availability_zones()
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must report {REGION}'s zones: {error:?}"))
        .availability_zones()
        .iter()
        .filter_map(|zone| zone.zone_name().map(str::to_string))
        .collect();
    assert!(
        zones.len() >= MIN_SUBNET_GROUP_AVAILABILITY_ZONES,
        "setup: expected {REGION} to have at least \
         {MIN_SUBNET_GROUP_AVAILABILITY_ZONES} zones, got {zones:?}"
    );

    let vpc = create_vpc(&ec2, VPC_CIDR).await;
    let first_zone = zones.first().expect("setup: a first zone").clone();
    let last_zone = zones.last().expect("setup: a last zone").clone();
    let in_first: Vec<Subnet> = {
        let mut subnets = Vec::new();
        for index in 0..3u8 {
            subnets.push(
                create_subnet(&ec2, &vpc, &format!("10.40.{index}.0/24"), &first_zone).await,
            );
        }
        subnets
    };
    let in_last = create_subnet(&ec2, &vpc, "10.40.200.0/24", &last_zone).await;

    let id = |subnet: &Subnet| subnet.id.clone();
    let rows = vec![
        Row {
            case: "no subnets at all, so no zone covered",
            subnet_ids: Vec::new(),
            covered: 0,
        },
        Row {
            case: "one subnet, in the Region's first zone",
            subnet_ids: vec![id(&in_first[0])],
            covered: 1,
        },
        Row {
            case: "one subnet, in the Region's last zone",
            subnet_ids: vec![id(&in_last)],
            covered: 1,
        },
        Row {
            case: "two subnets, both in the same zone",
            subnet_ids: vec![id(&in_first[0]), id(&in_first[1])],
            covered: 1,
        },
        Row {
            case: "three subnets, all in the same zone",
            subnet_ids: vec![id(&in_first[0]), id(&in_first[1]), id(&in_first[2])],
            covered: 1,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let (status, response) = emulator
            .call(
                "CreateReplicationSubnetGroup",
                &json!({
                    "ReplicationSubnetGroupIdentifier": format!("coverage-{index}"),
                    "ReplicationSubnetGroupDescription": row.case,
                    "SubnetIds": row.subnet_ids,
                }),
            )
            .await;
        let code = error_code(&response);
        assert_eq!(
            (status, code.as_str()),
            (400, NOT_ENOUGH_AZS),
            "case {:?}: expected the create to be refused with {NOT_ENOUGH_AZS}, got \
             {status} {response}",
            row.case
        );
        let message = text(&response, "/message");
        let expected = expected_message(MIN_SUBNET_GROUP_AVAILABILITY_ZONES, row.covered, &zones);
        assert_eq!(
            message, expected,
            "case {:?}: expected the message DMS serves for this error class, got \
             {message:?}",
            row.case
        );
    }

    // The control: a list that DOES cover the minimum is not refused, so the message above
    // belongs to the coverage rule rather than to every create.
    let covering: Vec<String> = {
        let mut ids = vec![id(&in_first[0]), id(&in_last)];
        ids.truncate(MIN_SUBNET_GROUP_AVAILABILITY_ZONES.max(2));
        ids
    };
    let (status, response) = emulator
        .call(
            "CreateReplicationSubnetGroup",
            &json!({
                "ReplicationSubnetGroupIdentifier": "coverage-control",
                "ReplicationSubnetGroupDescription": "subnets in two zones",
                "SubnetIds": covering,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected a list covering the minimum to be accepted, got \
         {status} {response}",
        "subnets in two zones"
    );
    // The accepted group is PERSISTED: read it back through the instance that resolves
    // it, which reports the very record the create stored.
    emulator
        .ok(
            "subnets in two zones",
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "coverage-control-instance",
                "ReplicationInstanceClass": "dms.t3.medium",
                "ReplicationSubnetGroupIdentifier": "coverage-control",
            }),
        )
        .await;
    let described = emulator
        .ok(
            "subnets in two zones",
            "DescribeReplicationInstances",
            &json!({}),
        )
        .await;
    assert_eq!(
        described.pointer("/ReplicationInstances/0/ReplicationSubnetGroup"),
        response.pointer("/ReplicationSubnetGroup"),
        "expected the accepted group to be readable back, got {described}"
    );
}
