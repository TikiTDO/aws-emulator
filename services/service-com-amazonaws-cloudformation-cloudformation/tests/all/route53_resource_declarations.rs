// SPDX-License-Identifier: CC0-1.0

//! The two things CloudFormation decides about an Amazon Route 53 declaration BEFORE any
//! call reaches that service: whether the resource TYPE is one a template may declare, and
//! whether the declaration satisfies the type's own schema.
//!
//! The rules under test:
//!
//! 1. **A resource type the "AWS resource and property types reference" publishes is a type a
//!    submitted template may declare**, so an operation that takes a template admits it — the
//!    three Route 53 types this suite is about are published there, each with its own page, so
//!    a template declaring them is accepted rather than refused as an unrecognised type. Their
//!    published RETURN VALUES come with them: the registry reports an extension for each, and
//!    the schema it publishes names the attributes `Fn::GetAtt` reads off one.
//! 2. **A resource whose declaration satisfies more than one subschema of a `oneOf` its
//!    type's schema states is refused by the pre-deployment validation**: the stack operation
//!    fails before anything is provisioned, the stack ends `ROLLBACK_COMPLETE`, and
//!    `DescribeEvents` records the validation that failed with the schema validator's own
//!    sentence and the path into the submitted template. The two Route 53 record types each
//!    publish `HostedZoneId` and `HostedZoneName` with "specify either `HostedZoneName` or
//!    `HostedZoneId`, but not both".
//!
//! Both are CLOUDFORMATION's own decisions — no owning service is asked anything — so they
//! are driven here, through the awsQuery wire, over the harness the other suites of this
//! crate use. What the resources then become in Route 53 is the router suite's subject
//! (`core/router/tests/all/cloudformation_provisions_route53_policies.rs`).

use serde_json::json;

use service_com_amazonaws_cloudformation_cloudformation::state::property_validation::
    matched_subschemas_reason;
use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::is_published;
use service_com_amazonaws_cloudformation_cloudformation::state::template::
    {published_attribute_names, IMPLEMENTED_RESOURCE_TYPES};

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts,
};

/// The three Route 53 types this suite is about, each published by the resource reference
/// with a page of its own, with the attributes that page's "Return values" section names.
const PUBLISHED_TYPES: &[(&str, &[&str])] = &[
    ("AWS::Route53::CidrCollection", &["Arn", "Id"]),
    ("AWS::Route53::DelegationSet", &["Id"]),
    ("AWS::Route53::TrafficPolicy", &["Id", "Type", "Version"]),
];

/// One row of the mutual-exclusion table: the type that publishes the pair, and the
/// properties a declaration of it carries beside that pair.
type ExclusiveType = (&'static str, fn() -> serde_json::Value);

/// The two record types whose schema makes `HostedZoneId` and `HostedZoneName` mutually
/// exclusive, with the properties a declaration of each carries beside that pair.
const EXCLUSIVE_TYPES: &[ExclusiveType] = &[
    ("AWS::Route53::RecordSet", || {
        json!({ "Name": "both.cfn-schema.example.", "Type": "A", "TTL": "300" })
    }),
    ("AWS::Route53::RecordSetGroup", || {
        json!({
            "RecordSets": [
                { "Name": "g.cfn-schema.example.", "Type": "A", "TTL": "300" }
            ]
        })
    }),
];

/// Rule 1, the catalogue side: each type is published, carries the return values its page
/// names, and is reported as an implemented extension of the registry.
#[test]
fn the_three_route53_types_are_published_with_their_return_values() {
    for (resource_type, attributes) in PUBLISHED_TYPES {
        assert!(
            is_published(resource_type),
            "case {resource_type:?}: expected the type to be one a template may declare"
        );
        assert!(
            IMPLEMENTED_RESOURCE_TYPES.contains(resource_type),
            "case {resource_type:?}: expected the type among the ones this emulator carries \
             the return values of"
        );
        let published = published_attribute_names(resource_type);
        assert_eq!(
            published, *attributes,
            "case {resource_type:?}: expected the attributes its Return values section names \
             ({attributes:?}), got {published:?}"
        );
    }
    // A record set group publishes NO attribute of its own: Route 53 holds record sets, not
    // groups, so the group's only return value is the physical id CloudFormation gave it.
    assert!(
        is_published("AWS::Route53::RecordSetGroup"),
        "case \"a record set group\": expected the type to be one a template may declare"
    );
    assert!(
        published_attribute_names("AWS::Route53::RecordSetGroup").is_empty(),
        "case \"a record set group\": expected no published attribute, got {:?}",
        published_attribute_names("AWS::Route53::RecordSetGroup")
    );
}

/// Rule 1, the wire side: a template declaring the three types is ADMITTED by the operations
/// that take one — `ValidateTemplate`, which is a verdict on the body, and `CreateStack`,
/// which answers a stack id for it.
#[tokio::test]
async fn a_template_declaring_the_three_types_is_admitted() {
    let router = router();
    let template = json!({
        "Resources": {
            "Collection": {
                "Type": "AWS::Route53::CidrCollection",
                "Properties": { "Name": "cfn-schema-collection" }
            },
            "Set": {
                "Type": "AWS::Route53::DelegationSet",
                "Properties": { "CallerReference": "cfn-schema-set" }
            },
            "Policy": {
                "Type": "AWS::Route53::TrafficPolicy",
                "Properties": {
                    "Name": "cfn-schema-policy",
                    "Document": {
                        "AWSPolicyFormatVersion": "2015-10-01",
                        "RecordType": "A",
                        "StartEndpoint": "main",
                        "Endpoints": { "main": { "Type": "value", "Value": "192.0.2.10" } }
                    }
                }
            }
        }
    })
    .to_string();
    let template = template.as_str();

    let (status, body) = call(
        &router,
        "ValidateTemplate",
        &[member("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ValidateTemplate\": expected the body to be admitted, got {status} with \
         {body}"
    );

    let (status, body) = create_stack(&router, "route53-schema-stack", template, &[]).await;
    assert_eq!(
        status, 200,
        "case \"CreateStack\": expected the template to be accepted, got {status} with {body}"
    );
    assert!(
        xml_text(&body, "StackId").is_some(),
        "case \"CreateStack\": expected a StackId for the accepted template, got {body}"
    );
}

/// Rule 2: a declaration carrying BOTH members of the mutually exclusive pair is refused
/// before provisioning — for either type that publishes the pair.
#[tokio::test]
async fn a_declaration_matching_two_subschemas_fails_the_operation() {
    for (index, (resource_type, properties)) in EXCLUSIVE_TYPES.iter().enumerate() {
        let case = *resource_type;
        let router = router();
        let stack_name = format!("route53-schema-exclusive-{index}");
        let mut properties = properties();
        let members = properties.as_object_mut().expect("an object");
        members.insert(
            "HostedZoneId".to_string(),
            json!("Z00000000000000EXAMPLE"),
        );
        members.insert(
            "HostedZoneName".to_string(),
            json!("cfn-schema.example."),
        );
        let template = json!({
            "Resources": { "R": { "Type": resource_type, "Properties": properties } }
        })
        .to_string();
        let (status, body) = create_stack(&router, &stack_name, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack — the template is well formed, \
             the declaration is what the schema refuses — got {status} with {body}"
        );
        wait_for_the_transition().await;

        let (status, described) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with \
             {described}"
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the create to roll back, got {described}"
        );

        // The validation that failed is reported as the validation result it is: one event,
        // naming the resource, the validator's own sentence and the path it ran on.
        let (status, events) = call(
            &router,
            "DescribeEvents",
            &[member("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeEvents, got {status} with {events}"
        );
        let validation: Vec<String> = xml_blocks(&events, "member")
            .into_iter()
            .filter(|event| {
                xml_text(event, "EventType").as_deref() == Some("VALIDATION_ERROR")
            })
            .collect();
        assert_eq!(
            validation.len(),
            1,
            "case {case:?}: expected exactly one VALIDATION_ERROR event, got {events}"
        );
        let reported = &validation[0];
        assert_eq!(
            xml_text(reported, "LogicalResourceId").as_deref(),
            Some("R"),
            "case {case:?}: expected the event to be about the resource that declared both \
             members, got {reported}"
        );
        // Two of the pair's subschemas matched, which is the one variable part of the
        // sentence.
        assert_eq!(
            xml_text(reported, "ValidationStatusReason").as_deref(),
            Some(matched_subschemas_reason(2).as_str()),
            "case {case:?}: expected the schema validator's own sentence, got {reported}"
        );
        assert_eq!(
            xml_text(reported, "ValidationPath").as_deref(),
            Some("/Resources/R/Properties"),
            "case {case:?}: expected the path into the submitted template, got {reported}"
        );

        // Nothing was provisioned: the stack lists no resource at all.
        let (status, listed) = call(
            &router,
            "ListStackResources",
            &[member("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListStackResources, got {status} with \
             {listed}"
        );
        assert!(
            xml_texts(&listed, "LogicalResourceId").is_empty(),
            "case {case:?}: expected a rolled-back stack to list no resources, got {listed}"
        );
    }
}

/// The control: a declaration carrying exactly ONE of the pair satisfies the `oneOf`, so the
/// pre-deployment validation passes it on — whichever of the two it states.
#[tokio::test]
async fn a_declaration_naming_its_zone_one_way_passes_the_validation() {
    // (case, the zone member the declaration carries)
    let rows: [(&str, &str, &str); 2] = [
        ("by id", "HostedZoneId", "Z00000000000000EXAMPLE"),
        ("by DNS name", "HostedZoneName", "cfn-schema.example."),
    ];
    for (index, (case, member_name, value)) in rows.into_iter().enumerate() {
        let router = router();
        let stack_name = format!("route53-schema-one-of-{index}");
        let template = json!({
            "Resources": {
                "R": {
                    "Type": "AWS::Route53::RecordSet",
                    "Properties": {
                        "Name": "one.cfn-schema.example.",
                        "Type": "A",
                        "TTL": "300",
                        member_name: value
                    }
                }
            }
        })
        .to_string();
        let (status, body) = create_stack(&router, &stack_name, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
        );
        wait_for_the_transition().await;
        let (status, events) = call(
            &router,
            "DescribeEvents",
            &[member("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeEvents, got {status} with {events}"
        );
        assert!(
            !xml_texts(&events, "EventType").contains(&"VALIDATION_ERROR".to_string()),
            "case {case:?}: expected no validation to fail for a declaration naming its zone \
             one way, got {events}"
        );
    }
}
