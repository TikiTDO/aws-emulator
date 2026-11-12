// SPDX-License-Identifier: CC0-1.0

//! The physical name CloudFormation GENERATES for an Amazon RDS resource a template does
//! not name, read back in Amazon RDS.
//!
//! RULE: a physical name CloudFormation generates for a resource whose template names none
//! is fitted to the rules of the member the owning service creates the resource under —
//! its length AND the case that service folds the name to — so the physical id the stack
//! publishes (what `Ref` resolves to) IS the name that service holds, and a client that
//! reads the id and then asks the owning service about the resource finds it under exactly
//! that name.
//!
//! Every Amazon RDS resource an `AWS::RDS::*` type creates is named under a member whose
//! own documentation says "This value is stored as a lowercase string" — so for these types
//! a generated name comes out all lowercase:
//!
//! * `CreateDBInstance.DBInstanceIdentifier` (also "must contain from 1 to 63 letters,
//!   numbers, or hyphens");
//! * `CreateDBSubnetGroup.DBSubnetGroupName` ("Must contain no more than 255 letters,
//!   numbers, periods, underscores, spaces, or hyphens");
//! * `CreateDBParameterGroup.DBParameterGroupName` ("Must be 1 to 255 letters, numbers, or
//!   hyphens");
//! * `CreateDBClusterParameterGroup.DBClusterParameterGroupName`, whose name a modify
//!   answers with (`DBClusterParameterGroupNameMessage$DBClusterParameterGroupName`) states
//!   "from 1 to 255 letters or numbers".
//!
//! Each quotation is that member's own doc in Amazon RDS's model
//! (`smithy_query(query="doc", shape="CreateDBParameterGroupMessage",
//! member="DBParameterGroupName", model_path="services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json")`
//! and its siblings).
//!
//! The rows differ along what the rule generalises over: the resource TYPE (every Amazon
//! RDS type a stack creates in that service, each read back through its own describe), the
//! CASE of the logical id the name is built from (mixed, all upper, all lower) and of the
//! stack name, and the LENGTH of the stack name — one that needs no fitting beside one past
//! the 63 characters a DB instance identifier admits, so the case rule and the length rule
//! are exercised together.
//!
//! Both services are driven through one router over their own wires, so what is asserted is
//! what a client that deploys the template, reads the stack's physical ids and then calls
//! `DescribeDBInstances`, `DescribeDBSubnetGroups`, `DescribeDBParameterGroups`,
//! `DescribeDBClusterParameterGroups` or `DescribeDBSnapshots` observes.

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::rds_stack_resources::group_settings;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, rds, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts,
};

/// How many characters the unique segment of a generated name carries
/// (`<stack name>-<logical id>-<unique id>`, 12 hexadecimal characters).
const UNIQUE_SEGMENT_CHARACTERS: usize = 12;

/// The two hyphens that join the three segments of a generated name.
const SEPARATORS: usize = 2;

/// One Amazon RDS type a template declares WITHOUT naming the resource, with the call that
/// reports the name the service holds it under.
struct RdsNamed {
    /// The CloudFormation resource type.
    resource_type: &'static str,
    /// The logical id it is declared under, whose case the generated name carries.
    logical_id: &'static str,
    /// The Amazon RDS read that reports the resource.
    describe: &'static str,
    /// The member that read addresses the resource by, and reports its name as — the same
    /// member the create takes, and so what this type's `Ref` returns.
    name_member: &'static str,
    /// The longest name that member admits (see this module's own documentation).
    limit: usize,
}

/// Every Amazon RDS type a stack really creates in Amazon RDS, once each, under a logical id
/// of its own case.
const RDS_NAMED: &[RdsNamed] = &[
    RdsNamed {
        resource_type: "AWS::RDS::DBInstance",
        logical_id: "db",
        describe: "DescribeDBInstances",
        name_member: "DBInstanceIdentifier",
        limit: 63,
    },
    RdsNamed {
        resource_type: "AWS::RDS::DBSubnetGroup",
        logical_id: "SubnetGroup",
        describe: "DescribeDBSubnetGroups",
        name_member: "DBSubnetGroupName",
        limit: 255,
    },
    RdsNamed {
        resource_type: "AWS::RDS::DBParameterGroup",
        logical_id: "Pg",
        describe: "DescribeDBParameterGroups",
        name_member: "DBParameterGroupName",
        limit: 255,
    },
    RdsNamed {
        resource_type: "AWS::RDS::DBClusterParameterGroup",
        logical_id: "CLUSTERPG",
        describe: "DescribeDBClusterParameterGroups",
        name_member: "DBClusterParameterGroupName",
        limit: 255,
    },
];

/// The template declaring every row of [`RDS_NAMED`], none of them named: each declaration
/// carries exactly what its owning create requires of a caller, and the DB instance is
/// placed in the subnet group and the parameter group the same stack declares — so the
/// names those two `Ref`s resolve to have to be names Amazon RDS accepts.
///
/// The subnet ids are the ones the stack's own `AWS::EC2::Subnet` resources resolve to, since
/// Amazon RDS resolves every entry of `SubnetIds` against the subnets the account holds: what
/// is under test here is the NAME of the group, not where its subnets came from.
const TEMPLATE: &str = concat!(
    "Resources:\n",
    "  Vpc:\n",
    "    Type: AWS::EC2::VPC\n",
    "    Properties:\n",
    "      CidrBlock: 10.6.0.0/16\n",
    "  SubnetA:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.6.1.0/24\n",
    "      AvailabilityZone: !Select [0, !GetAZs \"\"]\n",
    "  SubnetB:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.6.2.0/24\n",
    "      AvailabilityZone: !Select [1, !GetAZs \"\"]\n",
    "  SubnetGroup:\n",
    "    Type: AWS::RDS::DBSubnetGroup\n",
    "    Properties:\n",
    "      DBSubnetGroupDescription: the generated-name stack's subnets\n",
    "      SubnetIds:\n",
    "        - !Ref SubnetA\n",
    "        - !Ref SubnetB\n",
    "  Pg:\n",
    "    Type: AWS::RDS::DBParameterGroup\n",
    "    Properties:\n",
    "      Description: the generated-name stack's parameters\n",
    "      Family: mysql8.0\n",
    "      Parameters:\n",
    "        max_connections: '77'\n",
    "  CLUSTERPG:\n",
    "    Type: AWS::RDS::DBClusterParameterGroup\n",
    "    Properties:\n",
    "      Description: the generated-name stack's cluster parameters\n",
    "      Family: aurora-mysql8.0\n",
    "  db:\n",
    "    Type: AWS::RDS::DBInstance\n",
    "    Properties:\n",
    "      Engine: mysql\n",
    "      DBInstanceClass: db.t3.micro\n",
    "      AllocatedStorage: 20\n",
    "      DBSubnetGroupName: !Ref SubnetGroup\n",
    "      DBParameterGroupName: !Ref Pg\n",
);

/// The `PhysicalResourceId` the stack publishes for one of its resources — the value this
/// type's `Ref` resolves to, and what a client addresses the resource by.
///
/// Shared with `rds_published_name_identity.rs`, which holds that same published value to
/// the identity the OWNING service reports, so both suites read it the one way.
pub(crate) async fn published_name(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackResources({stack}, {logical_id}) must answer 200, got {status_code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// Whether `name` is of the form Amazon RDS admits for one of these names: lowercase
/// letters, digits and single hyphens, beginning and ending with an alphanumeric.
fn is_an_rds_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric())
        && !name.contains("--")
}

/// Create the stack of [`TEMPLATE`] under `stack_name`, wait past both clocks — the stack's
/// own terminal status, and the status Amazon RDS owes a database it is bringing up — and
/// answer its stack id, which is how a stack whose deletion COMPLETED is still readable.
///
/// Shared with `rds_published_name_identity.rs`, which deploys a template of these same
/// types through it, so neither suite spells the two clocks a database stack owes itself.
pub(crate) async fn deploy(
    case: &str,
    router: &MultiServiceRouter,
    stack_name: &str,
    template: &str,
) -> String {
    let (status_code, created) = create_stack(router, stack_name, template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (status_code, described) = describe_stack(router, stack_name).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must reach CREATE_COMPLETE, got {status_code} {described}"
    );
    xml_text(&created, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a StackId, got {created}"))
}

/// The whole rule, per row of the table, for one stack name.
async fn generated_names_are_the_names_rds_holds(case: &str, stack_name: &str) {
    let router = router();
    deploy(case, &router, stack_name, TEMPLATE).await;

    for row in RDS_NAMED {
        let row_case = format!("{case}/{}/{}", row.resource_type, row.logical_id);
        let name = published_name(&router, stack_name, row.logical_id).await;

        // The FORM the owning service's member admits, which is what makes the id usable at
        // all: lowercase, within the bound, and the three segments of a generated name with
        // the unique one whole.
        assert!(
            is_an_rds_name(&name),
            "case {row_case:?}: expected a lowercase name of alphanumerics and single \
             hyphens beginning with a letter, got {name:?}"
        );
        assert!(
            name.chars().count() <= row.limit,
            "case {row_case:?}: expected a name within the {} characters its owning service \
             admits, got {} ({name:?})",
            row.limit,
            name.chars().count()
        );
        let unique = name
            .rsplit_once('-')
            .map(|(_, unique)| unique.to_string())
            .unwrap_or_default();
        assert!(
            unique.chars().count() == UNIQUE_SEGMENT_CHARACTERS
                && unique
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "case {row_case:?}: expected a {UNIQUE_SEGMENT_CHARACTERS}-character lowercase \
             hexadecimal unique segment, got {unique:?} in {name:?}"
        );
        // Nothing is fitted away where the whole name is within the bound, so there the
        // name is exactly the lowercased `<stack name>-<logical id>-<unique id>`.
        let unfitted = stack_name.chars().count()
            + row.logical_id.chars().count()
            + UNIQUE_SEGMENT_CHARACTERS
            + SEPARATORS;
        if unfitted <= row.limit {
            assert_eq!(
                name,
                format!(
                    "{}-{}-{unique}",
                    stack_name.to_ascii_lowercase(),
                    row.logical_id.to_ascii_lowercase()
                ),
                "case {row_case:?}: a name inside its bound must be the lowercased \
                 <stack name>-<logical id>-<unique id>"
            );
        }

        // The rule itself: Amazon RDS holds the resource under exactly the name the stack
        // published, and reports it under that spelling — so the stack's view of the
        // resource's identity and the owning service's view cannot disagree.
        let (status_code, reported) =
            rds(&router, row.describe, &[(row.name_member, name.as_str())]).await;
        assert_eq!(
            status_code, 200,
            "case {row_case:?}: {}({name}) must answer 200, got {status_code} {reported}",
            row.describe
        );
        assert!(
            xml_texts(&reported, row.name_member)
                .iter()
                .any(|reported_name| reported_name == &name),
            "case {row_case:?}: expected Amazon RDS to report {} {name:?}, got {reported}",
            row.name_member
        );
    }
}

#[tokio::test]
async fn a_generated_rds_name_is_the_name_amazon_rds_holds_the_resource_under() {
    // A lowercase stack name: only the logical ids carry upper case, so this row isolates
    // the logical-id half of the folding.
    generated_names_are_the_names_rds_holds("a lowercase stack name", "rds-generated-names")
        .await;
    // A stack name carrying upper case ("A stack name can contain only alphanumeric
    // characters (case-sensitive) and hyphens"), which the folding has to carry too.
    generated_names_are_the_names_rds_holds(
        "a stack name carrying upper case",
        "RDS-Generated-Names",
    )
    .await;
    // A stack name past the 63 characters a `DBInstanceIdentifier` admits, so the instance's
    // name is SHORTENED as well as folded while the groups' 255 characters leave theirs
    // whole.
    let long = {
        let head = "platform-production-shared-services-delivery-rds-";
        let mut name = head.to_string();
        name.push_str(&"a".repeat(70 - head.len()));
        name
    };
    assert_eq!(
        long.chars().count(),
        70,
        "the long-stack-name row must use a name of exactly 70 characters"
    );
    generated_names_are_the_names_rds_holds("a stack name past the instance bound", &long).await;
}

/// The same rule where it is READ after the stack is gone: a `DeletionPolicy` of `Retain`
/// leaves the parameter group and one of `Snapshot` leaves the DB instance's final snapshot,
/// and both are addressed by the name the stack published for the resource — which is the
/// only name the client has once the stack is deleted.
#[tokio::test]
async fn a_deleted_stacks_retained_group_and_final_snapshot_answer_to_the_published_names() {
    let case = "an unnamed Retain parameter group beside an unnamed Snapshot DB instance";
    let stack = "rds-generated-dp";
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Pg:\n",
        "    Type: AWS::RDS::DBParameterGroup\n",
        "    DeletionPolicy: Retain\n",
        "    UpdateReplacePolicy: Retain\n",
        "    Properties:\n",
        "      Description: retained by its deletion policy\n",
        "      Family: mysql8.0\n",
        "      Parameters:\n",
        "        max_connections: '77'\n",
        "  Db:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    DeletionPolicy: Snapshot\n",
        "    UpdateReplacePolicy: Snapshot\n",
        "    Properties:\n",
        "      Engine: mysql\n",
        "      DBInstanceClass: db.t3.micro\n",
        "      AllocatedStorage: 20\n",
        "      BackupRetentionPeriod: 0\n",
        "      DBParameterGroupName: !Ref Pg\n",
    );
    let stack_id = deploy(case, &router, stack, template).await;

    // The names the client holds: the ids the stack published while it still existed.
    let group = published_name(&router, stack, "Pg").await;
    let instance = published_name(&router, stack, "Db").await;

    let (status_code, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DeleteStack must answer 200, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (status_code, described) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "case {case:?}: expected the deletion to complete, got {status_code} {described}"
    );
    let (status_code, body) = rds(
        &router,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", instance.as_str())],
    )
    .await;
    assert_eq!(
        status_code, 404,
        "case {case:?}: expected the deleted instance to be gone, got {status_code} {body}"
    );

    // The RETAINED group: Amazon RDS still lists it, under the name the stack published,
    // with the value the template set on it.
    let (status_code, listed) = rds(&router, "DescribeDBParameterGroups", &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeDBParameterGroups must answer 200, got {status_code} {listed}"
    );
    let names = xml_texts(&listed, "DBParameterGroupName");
    assert!(
        names.iter().any(|name| name == &group),
        "case {case:?}: expected the retained group {group:?} among the groups Amazon RDS \
         lists, got {names:?}"
    );
    let (status_code, body, reported) =
        group_settings(&router, "DescribeDBParameters", "DBParameterGroupName", &group).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeDBParameters({group}) must answer 200, got {status_code} \
         {body}"
    );
    assert!(
        reported
            .iter()
            .any(|(name, value)| name == "max_connections" && value == "77"),
        "case {case:?}: expected the retained group to keep its declared value, got \
         {reported:?}"
    );

    // The SNAPSHOT policy's artifact: one manual snapshot of the deleted instance, named
    // from the stack, carrying the instance's engine, and available.
    let (status_code, body) = rds(
        &router,
        "DescribeDBSnapshots",
        &[
            ("DBInstanceIdentifier", instance.as_str()),
            ("SnapshotType", "manual"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeDBSnapshots({instance}) must answer 200, got {status_code} \
         {body}"
    );
    let snapshots = xml_blocks(&body, "DBSnapshot");
    assert_eq!(
        snapshots.len(),
        1,
        "case {case:?}: expected exactly one final snapshot of {instance}, got {body}"
    );
    let snapshot = &snapshots[0];
    let identifier = xml_text(snapshot, "DBSnapshotIdentifier").unwrap_or_default();
    assert!(
        identifier.starts_with(stack) && is_an_rds_name(&identifier),
        "case {case:?}: expected a snapshot named from the stack {stack:?}, got \
         {identifier:?}"
    );
    for (member_name, expected) in [
        ("SnapshotType", "manual"),
        ("Engine", "mysql"),
        ("Status", "available"),
        ("DBInstanceIdentifier", instance.as_str()),
    ] {
        assert_eq!(
            xml_text(snapshot, member_name).as_deref(),
            Some(expected),
            "case {case:?}: expected the final snapshot's {member_name} to be \
             {expected:?}, got {snapshot}"
        );
    }
}
