// SPDX-License-Identifier: CC0-1.0

//! The identity a stack PUBLISHES for an Amazon RDS resource whose name CloudFormation
//! generated, read back through every handle a client holds it by.
//!
//! RULE: the identifier a stack publishes for a resource it created — its
//! `PhysicalResourceId`, which is what the resource's `Ref` resolves to — is the identifier
//! the service that OWNS the resource holds it under, so every handle a client has for one
//! resource is the same byte string: `Ref`, the `PhysicalResourceId` of both stack-resource
//! reads, the name member the owning service's own read reports, and the resource part of
//! the ARN that read reports. A client that compares, indexes, caches or reaps by the
//! identifier the stack published — the normal pattern for a resource a `DeletionPolicy`
//! outlives its stack by, and for cross-stack wiring — therefore finds the resource rather
//! than a near-miss spelling of it.
//!
//! For the Amazon RDS types here the owning service FOLDS the name it is created under:
//! every one of these members documents "This value is stored as a lowercase string"
//! (`CreateDBParameterGroup.DBParameterGroupName`,
//! `CreateDBClusterParameterGroup.DBClusterParameterGroupName`,
//! `CreateDBSubnetGroup.DBSubnetGroupName` and `CreateDBInstance.DBInstanceIdentifier` —
//! `smithy_query(query="doc", shape="CreateDBParameterGroupMessage",
//! member="DBParameterGroupName",
//! model_path="services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json")`
//! and its siblings). A generated name carrying upper case would therefore name a resource
//! Amazon RDS does not hold under that spelling, and the published identifier would not
//! round-trip.
//!
//! Addressing one of these resources is itself case-insensitive — Amazon RDS folds the name
//! a read asks under — so the second half of each row asks for the resource under the
//! UPPERCASED identifier and holds the answer to reporting the STORED spelling: what the
//! owning service reports is its own record, never the spelling the request carried.
//!
//! The rows differ along what the rule generalises over: the resource TYPE (each read back
//! through its own describe and its own name and ARN members), the CASE of the logical id
//! the generated name is built from (mixed, all upper, all lower) and of the stack name, and
//! the LENGTH of the stack name — one that needs no fitting beside one past the 63
//! characters a `DBInstanceIdentifier` admits, so a name the fitting SHORTENED is held to
//! the same identity.
//!
//! `rds_generated_resource_names.rs` is the sibling suite that asserts the FORM of such a
//! name (its segments, its bound, its character set); this one asserts that the published
//! identifier and the owning service's own identity for the resource are one value. Both
//! services are driven through one router over their own wires, so what is asserted is what
//! a client that deploys the template and then reads the stack and Amazon RDS observes.

use smithy_server_runtime::MultiServiceRouter;

use crate::rds_generated_resource_names::{deploy, published_name};
use crate::stack_lifecycle::{
    call, describe_stack, member, output_value, rds, router, xml_blocks, xml_text,
};

/// One Amazon RDS resource the template declares WITHOUT naming, with every handle its
/// identity is published and read through.
struct Published {
    /// The CloudFormation resource type.
    resource_type: &'static str,
    /// The logical id it is declared under, whose case the generated name is built from.
    logical_id: &'static str,
    /// The `OutputKey` whose `Value` is `!Ref <logical id>`.
    output_key: &'static str,
    /// The Amazon RDS read that reports the resource.
    describe: &'static str,
    /// The element that read reports one resource as.
    record: &'static str,
    /// The member that read addresses the resource by and reports its name as — the same
    /// member the create takes, and so what this type's `Ref` returns.
    name_member: &'static str,
    /// The member that read reports the resource's ARN as.
    arn_member: &'static str,
    /// The resource-type segment of that ARN, whose following field is the resource's name
    /// (`arn:aws:rds:<region>:<account>:<segment>:<name>`).
    arn_segment: &'static str,
}

/// Every Amazon RDS type this suite's template declares, once each, under a logical id of
/// its own case.
const PUBLISHED: &[Published] = &[
    Published {
        resource_type: "AWS::RDS::DBParameterGroup",
        logical_id: "PgOne",
        output_key: "ParameterGroupName",
        describe: "DescribeDBParameterGroups",
        record: "DBParameterGroup",
        name_member: "DBParameterGroupName",
        arn_member: "DBParameterGroupArn",
        arn_segment: "pg",
    },
    Published {
        resource_type: "AWS::RDS::DBClusterParameterGroup",
        logical_id: "CLUSTERPARAMS",
        output_key: "ClusterParameterGroupName",
        describe: "DescribeDBClusterParameterGroups",
        record: "DBClusterParameterGroup",
        name_member: "DBClusterParameterGroupName",
        arn_member: "DBClusterParameterGroupArn",
        arn_segment: "cluster-pg",
    },
    Published {
        resource_type: "AWS::RDS::DBSubnetGroup",
        logical_id: "subnets",
        output_key: "SubnetGroupName",
        describe: "DescribeDBSubnetGroups",
        record: "DBSubnetGroup",
        name_member: "DBSubnetGroupName",
        arn_member: "DBSubnetGroupArn",
        arn_segment: "subgrp",
    },
    Published {
        resource_type: "AWS::RDS::DBInstance",
        logical_id: "DbInstance",
        output_key: "InstanceIdentifier",
        describe: "DescribeDBInstances",
        record: "DBInstance",
        name_member: "DBInstanceIdentifier",
        arn_member: "DBInstanceArn",
        arn_segment: "db",
    },
];

/// The template declaring every row of [`PUBLISHED`], none of them named, with one `Ref`
/// output per resource — so the identifier the stack publishes is read both as a resource's
/// `PhysicalResourceId` and as the value its `Ref` resolved to.
///
/// The DB instance is placed in the subnet group and the parameter group the same stack
/// declares, so the names those `Ref`s resolve to have to be names Amazon RDS accepts; the
/// subnet ids are the stack's own subnets, since Amazon RDS resolves every entry of
/// `SubnetIds` against the subnets the account holds.
const TEMPLATE: &str = concat!(
    "Resources:\n",
    "  Vpc:\n",
    "    Type: AWS::EC2::VPC\n",
    "    Properties:\n",
    "      CidrBlock: 10.9.0.0/16\n",
    "  SubnetOne:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.9.1.0/24\n",
    "      AvailabilityZone: !Select [0, !GetAZs \"\"]\n",
    "  SubnetTwo:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.9.2.0/24\n",
    "      AvailabilityZone: !Select [1, !GetAZs \"\"]\n",
    "  subnets:\n",
    "    Type: AWS::RDS::DBSubnetGroup\n",
    "    Properties:\n",
    "      DBSubnetGroupDescription: the published-name stack's subnets\n",
    "      SubnetIds:\n",
    "        - !Ref SubnetOne\n",
    "        - !Ref SubnetTwo\n",
    "  PgOne:\n",
    "    Type: AWS::RDS::DBParameterGroup\n",
    "    Properties:\n",
    "      Description: the published-name stack's parameters\n",
    "      Family: mysql8.0\n",
    "      Parameters:\n",
    "        max_connections: '77'\n",
    "  CLUSTERPARAMS:\n",
    "    Type: AWS::RDS::DBClusterParameterGroup\n",
    "    Properties:\n",
    "      Description: the published-name stack's cluster parameters\n",
    "      Family: aurora-mysql8.0\n",
    "  DbInstance:\n",
    "    Type: AWS::RDS::DBInstance\n",
    "    Properties:\n",
    "      Engine: mysql\n",
    "      DBInstanceClass: db.t3.micro\n",
    "      AllocatedStorage: 20\n",
    "      DBSubnetGroupName: !Ref subnets\n",
    "      DBParameterGroupName: !Ref PgOne\n",
    "Outputs:\n",
    "  SubnetGroupName:\n",
    "    Value: !Ref subnets\n",
    "  ParameterGroupName:\n",
    "    Value: !Ref PgOne\n",
    "  ClusterParameterGroupName:\n",
    "    Value: !Ref CLUSTERPARAMS\n",
    "  InstanceIdentifier:\n",
    "    Value: !Ref DbInstance\n",
);

/// The `PhysicalResourceId` the SINGULAR read reports for one resource of a stack, read as
/// its own `StackResourceDetail` block.
async fn detail_physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackResource({stack}, {logical_id}) must answer 200, got {status_code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// The ONE record Amazon RDS reports for the resource asked for under `asked`, left exactly
/// as that read answered it — so the caller is what asserts which resource it is.
///
/// Each of these reads addresses the resource by its own name member, so the answer carries
/// exactly one record; a read that reported several would say that the name addresses more
/// than one resource, which is the opposite of what this suite asserts.
async fn reported_record(
    case: &str,
    router: &MultiServiceRouter,
    row: &Published,
    asked: &str,
) -> String {
    let (status_code, body) = rds(router, row.describe, &[(row.name_member, asked)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: {}({asked}) must answer 200, got {status_code} {body}",
        row.describe
    );
    let records = xml_blocks(&body, row.record);
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: expected {}({asked}) to report exactly one {}, got {body}",
        row.describe,
        row.record
    );
    records.into_iter().next().unwrap_or_default()
}

/// The whole rule, per row of the table, for one stack name.
async fn published_identity_is_the_owning_services_identity(case: &str, stack_name: &str) {
    let router = router();
    deploy(case, &router, stack_name, TEMPLATE).await;
    let (status_code, described) = describe_stack(&router, stack_name).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {status_code} {described}"
    );

    for row in PUBLISHED {
        let row_case = format!("{case}/{}/{}", row.resource_type, row.logical_id);
        let published = published_name(&router, stack_name, row.logical_id).await;
        assert!(
            !published.is_empty(),
            "case {row_case:?}: expected the stack to publish a PhysicalResourceId"
        );
        // The name Amazon RDS folds what it stores to, which is what makes the identity
        // below possible at all.
        assert_eq!(
            published,
            published.to_ascii_lowercase(),
            "case {row_case:?}: expected the published identifier to be the lowercase name \
             Amazon RDS stores, got {published:?}"
        );

        // The stack's OWN handles for the resource: the singular resource read and the
        // `Ref` the template resolved.
        assert_eq!(
            detail_physical_id(&router, stack_name, row.logical_id).await,
            published,
            "case {row_case:?}: DescribeStackResource must report the same \
             PhysicalResourceId as DescribeStackResources ({published:?})"
        );
        assert_eq!(
            output_value(&described, row.output_key).as_deref(),
            Some(published.as_str()),
            "case {row_case:?}: expected the Ref output {} to be {published:?}, got \
             {described}",
            row.output_key
        );

        // The owning service's handles: the name its own read reports, and the resource
        // part of the ARN it reports.
        let record = reported_record(&row_case, &router, row, &published).await;
        assert_eq!(
            xml_text(&record, row.name_member).as_deref(),
            Some(published.as_str()),
            "case {row_case:?}: expected Amazon RDS to report {} {published:?}, got {record}",
            row.name_member
        );
        let arn = xml_text(&record, row.arn_member).unwrap_or_default();
        let fields: Vec<&str> = arn.split(':').collect();
        assert_eq!(
            fields.len(),
            7,
            "case {row_case:?}: expected {} to be an ARN of seven fields, got {arn:?}",
            row.arn_member
        );
        assert_eq!(
            (fields[5], fields[6]),
            (row.arn_segment, published.as_str()),
            "case {row_case:?}: expected {} to address {}:{published} , got {arn:?}",
            row.arn_member,
            row.arn_segment
        );

        // Asked for under a DIFFERENT spelling of the same name, the owning service answers
        // with the spelling it STORES — so the identity above is its own record and not an
        // echo of the request.
        let asked = published.to_ascii_uppercase();
        assert_ne!(
            asked, published,
            "case {row_case:?}: the uppercased row must ask under a different spelling"
        );
        let folded = reported_record(&row_case, &router, row, &asked).await;
        assert_eq!(
            xml_text(&folded, row.name_member).as_deref(),
            Some(published.as_str()),
            "case {row_case:?}: expected {} to report the stored {} {published:?} for the \
             request under {asked:?}, got {folded}",
            row.describe,
            row.name_member
        );
    }
}

#[tokio::test]
async fn every_handle_a_stack_publishes_for_an_rds_resource_is_the_name_rds_holds_it_under() {
    // A stack name carrying upper case ("A stack name can contain only alphanumeric
    // characters (case-sensitive) and hyphens"), so both halves of the generated name — the
    // stack name and the logical id — carry case the owning service folds.
    published_identity_is_the_owning_services_identity(
        "a stack name carrying upper case",
        "RDS-Published-Identity",
    )
    .await;
    // A stack name past the 63 characters a `DBInstanceIdentifier` admits, so the instance's
    // generated name is SHORTENED as well as folded while the groups' 255 characters leave
    // theirs whole — the identity has to survive the fitting.
    let long = {
        let head = "platform-staging-shared-services-published-identity-";
        let mut name = head.to_string();
        name.push_str(&"b".repeat(72 - head.len()));
        name
    };
    assert_eq!(
        long.chars().count(),
        72,
        "the long-stack-name row must use a name of exactly 72 characters"
    );
    published_identity_is_the_owning_services_identity(
        "a stack name past the instance bound",
        &long,
    )
    .await;
}
