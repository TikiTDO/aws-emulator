// SPDX-License-Identifier: CC0-1.0

//! End-to-end CloudFormation stack provisioning for Route 53 zone NS delegation
//! (`Fn::GetAtt: [ChildZone, NameServers]` -> `AWS::Route53::RecordSet`) and ACM DNS
//! validation with `DomainValidationOptions[].HostedZoneId` (including duplicate option
//! entries, wildcard SANs, CNAME UPSERT into Route 53, issuance, and clean stack deletion).

use std::sync::Arc;

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, create_stack, member, router_with_engine, wait_for_the_transition, xml_blocks, xml_text,
    xml_texts, REGION,
};
use crate::stack_lifecycle::describe_stack;

fn router_with_dns_and_acm() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = router_with_engine();
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

async fn route53_get(router: &MultiServiceRouter, path: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/2013-04-01{path}"))
        .header("host", "route53.amazonaws.com")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/route53/aws4_request, SignedHeaders=host, Signature=0123456789abcdef",
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed Route 53 request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

async fn acm_describe(router: &MultiServiceRouter, arn: &str) -> (u16, serde_json::Value) {
    let body = serde_json::json!({ "CertificateArn": arn }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("acm.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "CertificateManager.DescribeCertificate")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/acm/aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed ACM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: serde_json::Value =
        serde_json::from_slice(response.body()).expect("ACM answers JSON");
    (status, parsed)
}

#[tokio::test]
async fn hosted_zone_delegation_and_acm_hosted_zone_validation_create_issue_and_delete_cleanly() {
    let (router, _engine) = router_with_dns_and_acm();

    // Exercises both Build 7 stack blockers on a public domain (`example-estate.org`) so
    // issuance proves real Route 53 CNAME validation (not just sandbox auto-issue), plus
    // duplicate `DomainValidationOptions` entries and wildcard SAN coverage:
    // 1. `Fn::GetAtt: [ChildZone, NameServers]` feeding a parent `AWS::Route53::RecordSet`
    //    (`Type: NS`, `TTL: "1800"`).
    // 2. `AWS::CertificateManager::Certificate` with `DomainValidationOptions[].HostedZoneId`
    //    (and no `ValidationDomain`), duplicate domain entries, and a wildcard SAN.
    // 3. `DeleteStack` cleaning up published validation CNAMEs before deleting `ChildZone`.
    let template = serde_json::json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ParentZone": {
                "Type": "AWS::Route53::HostedZone",
                "Properties": {
                    "Name": "example-estate.org"
                }
            },
            "ChildZone": {
                "Type": "AWS::Route53::HostedZone",
                "Properties": {
                    "Name": "app.example-estate.org"
                }
            },
            "ChildDelegation": {
                "Type": "AWS::Route53::RecordSet",
                "Properties": {
                    "HostedZoneId": { "Ref": "ParentZone" },
                    "Name": "app.example-estate.org.",
                    "Type": "NS",
                    "TTL": "1800",
                    "ResourceRecords": {
                        "Fn::GetAtt": ["ChildZone", "NameServers"]
                    }
                }
            },
            "AppCertificate": {
                "Type": "AWS::CertificateManager::Certificate",
                "Properties": {
                    "DomainName": "app.example-estate.org",
                    "SubjectAlternativeNames": [
                        "app.example-estate.org",
                        "*.app.example-estate.org"
                    ],
                    "ValidationMethod": "DNS",
                    "DomainValidationOptions": [
                        {
                            "DomainName": "app.example-estate.org",
                            "HostedZoneId": { "Ref": "ChildZone" }
                        },
                        {
                            "DomainName": "app.example-estate.org",
                            "HostedZoneId": { "Ref": "ChildZone" }
                        }
                    ]
                }
            }
        },
        "Outputs": {
            "ParentZoneId": { "Value": { "Ref": "ParentZone" } },
            "ChildZoneId": { "Value": { "Ref": "ChildZone" } },
            "CertArn": { "Value": { "Ref": "AppCertificate" } }
        }
    })
    .to_string();

    let (create_status, create_body) = create_stack(&router, "dns-acm-stack", &template, &[]).await;
    assert_eq!(create_status, 200, "CreateStack refused: {create_body}");

    let mut stack_body = String::new();
    for _ in 0..8 {
        wait_for_the_transition().await;
        let (desc_status, body) = describe_stack(&router, "dns-acm-stack").await;
        assert_eq!(desc_status, 200, "DescribeStacks failed: {body}");
        stack_body = body;
        let current = xml_text(&stack_body, "StackStatus").unwrap_or_default();
        if current != status::CREATE_IN_PROGRESS {
            break;
        }
    }

    let final_status = xml_text(&stack_body, "StackStatus").unwrap_or_default();
    if final_status != status::CREATE_COMPLETE {
        let (_, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", "dns-acm-stack")],
        )
        .await;
        panic!("expected CREATE_COMPLETE, got {final_status}; events:\n{events}");
    }

    let mut outputs = std::collections::BTreeMap::new();
    for block in xml_blocks(&stack_body, "member") {
        if let (Some(k), Some(v)) = (xml_text(&block, "OutputKey"), xml_text(&block, "OutputValue"))
        {
            outputs.insert(k, v);
        }
    }
    let parent_zone_id = outputs.get("ParentZoneId").expect("ParentZoneId output");
    let child_zone_id = outputs.get("ChildZoneId").expect("ChildZoneId output");
    let cert_arn = outputs.get("CertArn").expect("CertArn output");

    // 1. Verify ChildZone's minted NameServers match the NS record written into ParentZone.
    let (gz_status, gz_xml) = route53_get(&router, &format!("/hostedzone/{child_zone_id}")).await;
    assert_eq!(gz_status, 200, "GetHostedZone failed: {gz_xml}");
    let mut expected_ns = xml_texts(&gz_xml, "NameServer");
    assert_eq!(expected_ns.len(), 4, "expected 4 minted name servers: {gz_xml}");
    expected_ns.sort();

    let (parent_rr_status, parent_rr_xml) =
        route53_get(&router, &format!("/hostedzone/{parent_zone_id}/rrset")).await;
    assert_eq!(parent_rr_status, 200, "ListResourceRecordSets failed: {parent_rr_xml}");
    let delegated_rrset = xml_blocks(&parent_rr_xml, "ResourceRecordSet")
        .into_iter()
        .find(|b| {
            xml_text(b, "Name").as_deref() == Some("app.example-estate.org.")
                && xml_text(b, "Type").as_deref() == Some("NS")
        })
        .expect("parent zone must contain delegated NS record set for app.example-estate.org.");
    assert_eq!(xml_text(&delegated_rrset, "TTL").as_deref(), Some("1800"));
    let mut actual_ns = xml_texts(&delegated_rrset, "Value");
    actual_ns.sort();
    assert_eq!(
        actual_ns, expected_ns,
        "delegated NS record values must match ChildZone's minted NameServers"
    );

    // 2. Verify ACM certificate reached ISSUED via real Route 53 validation CNAME records.
    let (acm_status, acm_json) = acm_describe(&router, cert_arn).await;
    assert_eq!(acm_status, 200, "DescribeCertificate failed: {acm_json}");
    assert_eq!(
        acm_json["Certificate"]["Status"].as_str(),
        Some("ISSUED"),
        "certificate must reach ISSUED once validation CNAMEs are published: {acm_json}"
    );

    // 3. DeleteStack must remove published validation CNAMEs first so ChildZone deletes cleanly.
    let stack_id = xml_text(&stack_body, "StackId").expect("StackId");
    let (del_status, del_body) =
        call(&router, "DeleteStack", &[member("StackName", &stack_id)]).await;
    assert_eq!(del_status, 200, "DeleteStack refused: {del_body}");
    wait_for_the_transition().await;
    let (after_del_status, after_del_body) = describe_stack(&router, &stack_id).await;
    assert_eq!(after_del_status, 200, "DescribeStacks by id failed: {after_del_body}");
    assert_eq!(
        xml_text(&after_del_body, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "DeleteStack must reach DELETE_COMPLETE without HostedZoneNotEmpty: {after_del_body}"
    );
}
