// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares `AWS::Route53::CidrCollection`,
//! `AWS::Route53::DelegationSet` and `AWS::Route53::TrafficPolicy` really creates them in
//! Amazon Route 53, and the value the stack hands back for each is the identifier that
//! service minted.
//!
//! The rule under test: **a resource type the CloudFormation resource reference publishes is
//! a type `CreateStack` accepts and provisions against the service that owns it, and the
//! physical resource id it reports — what `Ref` resolves to and what an output publishes — is
//! the identifier that owning service minted for the resource.** For these three types that
//! identifier is Route 53's: the collection's UUID, the ARN of the reusable delegation set
//! (nesting the `N…` id that service minted) and the traffic policy's id, each the handle
//! every operation on the resource takes. Two crates have to agree for that to hold
//! (CloudFormation makes the calls; Route 53 serves them and mints the ids), so the suite
//! lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: whether the template is JSON or
//! YAML, whether the CIDR collection declares locations at all and how many blocks each
//! carries, whether the delegation set states its own caller reference or leaves it to
//! CloudFormation, which Region the stack deploys in, and whether the traffic policy's
//! document is declared as a JSON document or as the text of one.
//!
//! The other half of the same rule is checked too: a declaration the OWNING service refuses —
//! a traffic policy whose document Route 53 will not accept — fails the resource with that
//! service's own refusal and rolls the stack back, rather than reporting a policy nothing
//! created.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element and list readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{
    element, elements, form_encode, members_of, opt_element, outputs_of, physical_ids_of,
};

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due, so the stack reports the template it deployed.
const PAST_PROVISIONING_SPANS: Duration = Duration::from_millis(900);

/// The version `CreateTrafficPolicy` assigns the policy a stack creates: "for a new traffic
/// policy, the value of `Version` is always 1" (`TrafficPolicy$Version`).
const FIRST_TRAFFIC_POLICY_VERSION: u32 = 1;

/// The most characters a reusable delegation set id may have: `ResourceId`'s own
/// `@length(min: 0, max: 32)`, the bound every member that takes one is declared with.
const MAX_RESOURCE_ID_CHARS: usize = 32;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Route 53 operation over its restXml wire, signed for `region`.
async fn route53(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("route53.{region}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization("route53", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// The traffic policy document the rows declare, as the JSON document a template states.
fn traffic_policy_document(record_type: &str, value: &str) -> serde_json::Value {
    json!({
        "AWSPolicyFormatVersion": "2015-10-01",
        "RecordType": record_type,
        "StartEndpoint": "main",
        "Endpoints": {
            "main": { "Type": "value", "Value": value }
        }
    })
}

/// One row of the table: a template declaring the three types, and what the rule says the
/// Route 53 resources it declares must then be.
struct Row {
    case: &'static str,
    region: &'static str,
    template: fn() -> String,
    /// The CIDR collection's declared name, and the locations it must hold afterwards as
    /// `(location, blocks)` pairs in the order `ListCidrBlocks` reports them.
    collection_name: &'static str,
    collection_blocks: &'static [(&'static str, &'static [&'static str])],
    /// The traffic policy's declared name, the DNS type Route 53 reads off its document and
    /// the comment it must carry.
    policy_name: &'static str,
    policy_record_type: &'static str,
    policy_comment: Option<&'static str>,
    /// The delegation set's declared caller reference, where the row states one.
    declared_caller_reference: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "one location of two blocks, a declared caller reference, a document, JSON",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Collection": {
                        "Type": "AWS::Route53::CidrCollection",
                        "Properties": {
                            "Name": "cfn-collection-one",
                            "Locations": [{
                                "LocationName": "office",
                                "CidrList": ["10.0.0.0/24", "10.0.1.0/24"]
                            }]
                        }
                    },
                    "Set": {
                        "Type": "AWS::Route53::DelegationSet",
                        "Properties": { "CallerReference": "cfn-set-one" }
                    },
                    "Policy": {
                        "Type": "AWS::Route53::TrafficPolicy",
                        "Properties": {
                            "Name": "cfn-policy-one",
                            "Comment": "deployed by a stack",
                            "Document": traffic_policy_document("A", "192.0.2.10")
                        }
                    }
                },
                "Outputs": {
                    "CollectionId": { "Value": { "Ref": "Collection" } },
                    "CollectionIdAttribute": { "Value": { "Fn::GetAtt": ["Collection", "Id"] } },
                    "CollectionArn": { "Value": { "Fn::GetAtt": ["Collection", "Arn"] } },
                    "SetId": { "Value": { "Ref": "Set" } },
                    "SetIdAttribute": { "Value": { "Fn::GetAtt": ["Set", "Id"] } },
                    "SetNameServers": {
                        "Value": {
                            "Fn::Join": [",", { "Fn::GetAtt": ["Set", "NameServers"] }]
                        }
                    },
                    "FirstNameServer": {
                        "Value": {
                            "Fn::Select": [0, { "Fn::GetAtt": ["Set", "NameServers"] }]
                        }
                    },
                    "PolicyId": { "Value": { "Ref": "Policy" } },
                    "PolicyIdAttribute": { "Value": { "Fn::GetAtt": ["Policy", "Id"] } },
                    "PolicyVersion": { "Value": { "Fn::GetAtt": ["Policy", "Version"] } },
                    "PolicyType": { "Value": { "Fn::GetAtt": ["Policy", "Type"] } }
                }
            })
            .to_string()
        },
        collection_name: "cfn-collection-one",
        collection_blocks: &[("office", &["10.0.0.0/24", "10.0.1.0/24"])],
        policy_name: "cfn-policy-one",
        policy_record_type: "A",
        policy_comment: Some("deployed by a stack"),
        declared_caller_reference: Some("cfn-set-one"),
    },
    Row {
        case: "two locations of one block, no caller reference, a document as text, YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Collection:",
                "    Type: AWS::Route53::CidrCollection",
                "    Properties:",
                "      Name: cfn-collection-two",
                "      Locations:",
                "        - LocationName: east",
                "          CidrList:",
                "            - 198.51.100.0/24",
                "        - LocationName: west",
                "          CidrList:",
                "            - 203.0.113.0/24",
                "  Set:",
                "    Type: AWS::Route53::DelegationSet",
                "  Policy:",
                "    Type: AWS::Route53::TrafficPolicy",
                "    Properties:",
                "      Name: cfn-policy-two",
                "      Document: >-",
                "        {\"AWSPolicyFormatVersion\":\"2015-10-01\",",
                "        \"RecordType\":\"CNAME\",\"StartEndpoint\":\"main\",",
                "        \"Endpoints\":{\"main\":{\"Type\":\"value\",",
                "        \"Value\":\"target.example.com\"}}}",
                "Outputs:",
                "  CollectionId:",
                "    Value: !Ref Collection",
                "  CollectionIdAttribute:",
                "    Value: !GetAtt Collection.Id",
                "  CollectionArn:",
                "    Value: !GetAtt Collection.Arn",
                "  SetId:",
                "    Value: !Ref Set",
                "  SetIdAttribute:",
                "    Value: !GetAtt Set.Id",
                "  SetNameServers:",
                "    Value: !Join [\",\", !GetAtt Set.NameServers]",
                "  FirstNameServer:",
                "    Value: !Select [0, !GetAtt Set.NameServers]",
                "  PolicyId:",
                "    Value: !Ref Policy",
                "  PolicyIdAttribute:",
                "    Value: !GetAtt Policy.Id",
                "  PolicyVersion:",
                "    Value: !GetAtt Policy.Version",
                "  PolicyType:",
                "    Value: !GetAtt Policy.Type",
            ]
            .join("\n")
        },
        collection_name: "cfn-collection-two",
        collection_blocks: &[("east", &["198.51.100.0/24"]), ("west", &["203.0.113.0/24"])],
        policy_name: "cfn-policy-two",
        policy_record_type: "CNAME",
        policy_comment: None,
        declared_caller_reference: None,
    },
    Row {
        case: "a collection with no locations at all, JSON",
        region: "us-west-2",
        template: || {
            json!({
                "Resources": {
                    "Collection": {
                        "Type": "AWS::Route53::CidrCollection",
                        "Properties": { "Name": "cfn-collection-three" }
                    },
                    "Set": { "Type": "AWS::Route53::DelegationSet", "Properties": {} },
                    "Policy": {
                        "Type": "AWS::Route53::TrafficPolicy",
                        "Properties": {
                            "Name": "cfn-policy-three",
                            "Document": traffic_policy_document("AAAA", "2001:db8::1")
                        }
                    }
                },
                "Outputs": {
                    "CollectionId": { "Value": { "Ref": "Collection" } },
                    "CollectionIdAttribute": { "Value": { "Fn::GetAtt": ["Collection", "Id"] } },
                    "CollectionArn": { "Value": { "Fn::GetAtt": ["Collection", "Arn"] } },
                    "SetId": { "Value": { "Ref": "Set" } },
                    "SetIdAttribute": { "Value": { "Fn::GetAtt": ["Set", "Id"] } },
                    "SetNameServers": {
                        "Value": {
                            "Fn::Join": [",", { "Fn::GetAtt": ["Set", "NameServers"] }]
                        }
                    },
                    "FirstNameServer": {
                        "Value": {
                            "Fn::Select": [0, { "Fn::GetAtt": ["Set", "NameServers"] }]
                        }
                    },
                    "PolicyId": { "Value": { "Ref": "Policy" } },
                    "PolicyIdAttribute": { "Value": { "Fn::GetAtt": ["Policy", "Id"] } },
                    "PolicyVersion": { "Value": { "Fn::GetAtt": ["Policy", "Version"] } },
                    "PolicyType": { "Value": { "Fn::GetAtt": ["Policy", "Type"] } }
                }
            })
            .to_string()
        },
        collection_name: "cfn-collection-three",
        collection_blocks: &[],
        policy_name: "cfn-policy-three",
        policy_record_type: "AAAA",
        policy_comment: None,
        declared_caller_reference: None,
    },
];

/// Every row: the stack is accepted, reaches `CREATE_COMPLETE`, and the three resources it
/// declares are the ones Route 53 holds under the identifiers the stack reports — then
/// deleting the stack takes all three away there.
#[tokio::test]
async fn a_stack_provisions_the_three_route53_policy_resources() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("route53-policy-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_ids = physical_ids_of(&resources);
        let outputs = outputs_of(&described);

        // --- the CIDR collection -------------------------------------------------------
        let collection_id = physical_ids.get("Collection").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack reports no resource \"Collection\": {resources}")
        });
        assert!(
            is_uuid(&collection_id),
            "case {case:?}: expected the collection's physical id to be the UUID Route 53 \
             mints (8-4-4-4-12 lowercase hex), got {collection_id:?}"
        );
        for key in ["CollectionId", "CollectionIdAttribute"] {
            assert_eq!(
                outputs.get(key),
                Some(&collection_id),
                "case {case:?}: expected output {key:?} to be the minted collection id \
                 {collection_id:?}, got {:?}",
                outputs.get(key)
            );
        }
        let expected_arn = format!("arn:aws:route53::000000000000:cidrcollection/{collection_id}");
        assert_eq!(
            outputs.get("CollectionArn"),
            Some(&expected_arn),
            "case {case:?}: expected output \"CollectionArn\" to be {expected_arn:?}, got {:?}",
            outputs.get("CollectionArn")
        );
        // Route 53 itself holds a collection under that id, carrying the declared name.
        let (status, listed) = route53(&router, region, "GET", "/2013-04-01/cidrcollection").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListCidrCollections, got {status} with body \
             {listed}"
        );
        let summary = members_of(&listed)
            .into_iter()
            .find(|member| elements(member, "Id") == vec![collection_id.clone()])
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected Route 53 to list the collection {collection_id:?}, \
                     got {listed}"
                )
            })
            .to_string();
        assert_eq!(
            opt_element(&summary, "Name").as_deref(),
            Some(row.collection_name),
            "case {case:?}: expected the declared collection name, got {summary}"
        );
        // ...and the blocks the declaration asked for, in the locations it named.
        let (status, blocks) = route53(
            &router,
            region,
            "GET",
            &format!("/2013-04-01/cidrcollection/{collection_id}/cidrblocks"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListCidrBlocks, got {status} with body \
             {blocks}"
        );
        let reported: Vec<(String, String)> = members_of(&blocks)
            .into_iter()
            .filter_map(|member| {
                let location = elements(member, "LocationName").into_iter().next()?;
                let block = elements(member, "CidrBlock").into_iter().next()?;
                Some((location, block))
            })
            .collect();
        let expected: Vec<(String, String)> = row
            .collection_blocks
            .iter()
            .flat_map(|(location, blocks)| {
                blocks
                    .iter()
                    .map(move |block| ((*location).to_string(), (*block).to_string()))
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the collection to hold the declared blocks {expected:?}, \
             got {blocks}"
        );

        // --- the reusable delegation set ----------------------------------------------
        // The resource's identity is the set's ARN, which is what this type's `Ref` returns;
        // the bare id Route 53 minted — the value every call on the set takes — is its `Id`
        // attribute, and the ARN nests exactly that id.
        let set_arn = physical_ids.get("Set").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack reports no resource \"Set\": {resources}")
        });
        let set_id = outputs.get("SetIdAttribute").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack reports no SetIdAttribute output: {described}")
        });
        assert!(
            is_delegation_set_id(&set_id),
            "case {case:?}: expected the delegation set's id to be a Route 53 delegation set \
             id (`N` and uppercase alphanumerics, at most {MAX_RESOURCE_ID_CHARS} \
             characters), got {set_id:?}"
        );
        let expected_set_arn = format!("arn:aws:route53:::delegationset/{set_id}");
        assert_eq!(
            set_arn, expected_set_arn,
            "case {case:?}: expected the set's physical id to be its ARN \
             {expected_set_arn:?}, got {set_arn:?}"
        );
        assert_eq!(
            outputs.get("SetId"),
            Some(&expected_set_arn),
            "case {case:?}: expected output \"SetId\" to be the set's ARN \
             {expected_set_arn:?}, got {:?}",
            outputs.get("SetId")
        );
        let (status, set) = route53(
            &router,
            region,
            "GET",
            &format!("/2013-04-01/delegationset/{set_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetReusableDelegationSet on the id the \
             stack reported, got {status} with body {set}"
        );
        assert_eq!(
            opt_element(&set, "Id").as_deref(),
            Some(format!("/delegationset/{set_id}").as_str()),
            "case {case:?}: expected the set to report the id the stack published, got {set}"
        );
        if let Some(reference) = row.declared_caller_reference {
            assert_eq!(
                opt_element(&set, "CallerReference").as_deref(),
                Some(reference),
                "case {case:?}: expected the declared caller reference, got {set}"
            );
        }
        // The set's `NameServers` are a LIST attribute, so the outputs that read it are the
        // name servers Route 53 delegated to — joined, and selected one by one.
        let name_servers = elements(&set, "NameServer");
        assert!(
            !name_servers.is_empty(),
            "case {case:?}: expected Route 53 to report the set's name servers, got {set}"
        );
        let joined = name_servers.join(",");
        assert_eq!(
            outputs.get("SetNameServers"),
            Some(&joined),
            "case {case:?}: expected output \"SetNameServers\" to be the set's name servers \
             joined ({joined:?}), got {:?}",
            outputs.get("SetNameServers")
        );
        assert_eq!(
            outputs.get("FirstNameServer"),
            name_servers.first(),
            "case {case:?}: expected output \"FirstNameServer\" to be the set's first name \
             server {:?}, got {:?}",
            name_servers.first(),
            outputs.get("FirstNameServer")
        );

        // --- the traffic policy --------------------------------------------------------
        // As for the delegation set, the resource's identity is the policy's ARN, and the id
        // Route 53 minted — the value `GetTrafficPolicy` takes — is its `Id` attribute.
        let policy_arn = physical_ids.get("Policy").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack reports no resource \"Policy\": {resources}")
        });
        let policy_id = outputs.get("PolicyIdAttribute").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack reports no PolicyIdAttribute output: {described}")
        });
        assert!(
            is_uuid(&policy_id),
            "case {case:?}: expected the policy's id to be the id Route 53 mints \
             (8-4-4-4-12 lowercase hex), got {policy_id:?}"
        );
        let expected_policy_arn = format!("arn:aws:route53:::trafficpolicy/{policy_id}");
        assert_eq!(
            policy_arn, expected_policy_arn,
            "case {case:?}: expected the policy's physical id to be its ARN \
             {expected_policy_arn:?}, got {policy_arn:?}"
        );
        assert_eq!(
            outputs.get("PolicyId"),
            Some(&expected_policy_arn),
            "case {case:?}: expected output \"PolicyId\" to be the policy's ARN \
             {expected_policy_arn:?}, got {:?}",
            outputs.get("PolicyId")
        );
        // The version and the DNS type the policy publishes are its other two return values:
        // a new policy is version 1, and its type is the one its document states.
        assert_eq!(
            outputs.get("PolicyVersion").map(String::as_str),
            Some(FIRST_TRAFFIC_POLICY_VERSION.to_string().as_str()),
            "case {case:?}: expected output \"PolicyVersion\" to be \
             {FIRST_TRAFFIC_POLICY_VERSION}, got {:?}",
            outputs.get("PolicyVersion")
        );
        assert_eq!(
            outputs.get("PolicyType").map(String::as_str),
            Some(row.policy_record_type),
            "case {case:?}: expected output \"PolicyType\" to be the record type the declared \
             document states, got {:?}",
            outputs.get("PolicyType")
        );
        let (status, policy) = route53(
            &router,
            region,
            "GET",
            &format!("/2013-04-01/trafficpolicy/{policy_id}/{FIRST_TRAFFIC_POLICY_VERSION}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetTrafficPolicy on the id the stack \
             reported, got {status} with body {policy}"
        );
        assert_eq!(
            opt_element(&policy, "Name").as_deref(),
            Some(row.policy_name),
            "case {case:?}: expected the declared policy name, got {policy}"
        );
        // The DNS type is no property of the type: Route 53 reads it off the document the
        // declaration carried, which is how the policy's own document reached that service.
        assert_eq!(
            opt_element(&policy, "Type").as_deref(),
            Some(row.policy_record_type),
            "case {case:?}: expected the record type the declared document states, got {policy}"
        );
        assert_eq!(
            opt_element(&policy, "Comment").as_deref(),
            row.policy_comment,
            "case {case:?}: expected the declared comment {:?}, got {policy}",
            row.policy_comment
        );

        // --- the delete ----------------------------------------------------------------
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, listed) = route53(&router, region, "GET", "/2013-04-01/cidrcollection").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListCidrCollections, got {status} with body \
             {listed}"
        );
        assert!(
            !elements(&listed, "Id").contains(&collection_id),
            "case {case:?}: expected the collection to be gone once the stack deleted it, got \
             {listed}"
        );
        let (status, gone) = route53(
            &router,
            region,
            "GET",
            &format!("/2013-04-01/delegationset/{set_id}"),
        )
        .await;
        assert_eq!(
            element(&gone, "Code"),
            "NoSuchDelegationSet",
            "case {case:?}: expected the delegation set to be gone once the stack deleted it, \
             got {status} with body {gone}"
        );
        let (status, listed) = route53(&router, region, "GET", "/2013-04-01/trafficpolicies").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTrafficPolicies, got {status} with body \
             {listed}"
        );
        assert!(
            !elements(&listed, "TrafficPolicyId").contains(&policy_id),
            "case {case:?}: expected the traffic policy to be gone once the stack deleted it, \
             got {listed}"
        );
    }
}

/// A traffic policy whose document Route 53 will not accept fails the resource with that
/// service's own refusal: the stack rolls back and no policy is created.
#[tokio::test]
async fn an_invalid_traffic_policy_document_fails_the_resource() {
    let router = build_router();
    let region = "us-east-2";
    // (case, the declared document, the constraint of the document format Route 53 reports
    // it as violating — its own sentence for that constraint)
    let rows: [(&str, serde_json::Value, &str); 3] = [
        (
            "a document that is not a policy at all",
            json!({ "not": "a policy" }),
            "AWSPolicyFormatVersion must be declared.",
        ),
        (
            "a policy naming a start endpoint it does not declare",
            json!({
                "AWSPolicyFormatVersion": "2015-10-01",
                "RecordType": "A",
                "StartEndpoint": "missing",
                "Endpoints": { "main": { "Type": "value", "Value": "192.0.2.10" } }
            }),
            "StartEndpoint missing does not reference a declared endpoint.",
        ),
        (
            "a policy whose record type is not a DNS type",
            json!({
                "AWSPolicyFormatVersion": "2015-10-01",
                "RecordType": "NOTATYPE",
                "StartEndpoint": "main",
                "Endpoints": { "main": { "Type": "value", "Value": "192.0.2.10" } }
            }),
            "Policy RecordType is missing or not supported.",
        ),
    ];

    for (index, (case, document, expected_violation)) in rows.into_iter().enumerate() {
        let stack_name = format!("route53-policy-invalid-{index}");
        let template = json!({
            "Resources": {
                "Policy": {
                    "Type": "AWS::Route53::TrafficPolicy",
                    "Properties": { "Name": format!("cfn-invalid-{index}"), "Document": document }
                }
            }
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack — the template is valid, the \
             document is Route 53's to judge — got {status} with body {body}"
        );
        tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the create to roll back, got {described}"
        );
        // The resource's own event carries Route 53's refusal of the document.
        let (status, events) = cloudformation(
            &router,
            region,
            "DescribeStackEvents",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with body \
             {events}"
        );
        let refused = elements(&events, "ResourceStatusReason")
            .into_iter()
            .find(|reason| reason.contains(expected_violation))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an event carrying Route 53's own refusal of the \
                     document ({expected_violation:?}), got {events}"
                )
            });
        // The refusal reaches the event as the called service's own SDK raises it: that
        // service's message followed by the SDK's diagnostic of the call.
        for fragment in [
            "(Service: Route53, Status Code: 400, Request ID: ",
            ") (SDK Attempt Count: 1)",
        ] {
            assert!(
                refused.contains(fragment),
                "case {case:?}: expected the reason to carry the SDK clause {fragment:?}, got \
                 {refused:?}"
            );
        }
        // ...and nothing was created: the account lists no traffic policy.
        let (status, listed) = route53(&router, region, "GET", "/2013-04-01/trafficpolicies").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTrafficPolicies, got {status} with body \
             {listed}"
        );
        assert!(
            elements(&listed, "TrafficPolicyId").is_empty(),
            "case {case:?}: expected no traffic policy to have been created, got {listed}"
        );
    }
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form the model fixes for a CIDR collection's
/// `UUID`, which is also the form a traffic policy id takes.
fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(&groups).all(|(length, group)| {
            group.len() == *length
                && group
                    .chars()
                    .all(|character| character.is_ascii_hexdigit() && !character.is_uppercase())
        })
}

/// Whether `id` is a reusable delegation set id of the form Route 53 publishes: `N` followed
/// by uppercase alphanumerics, inside the 32-character bound `ResourceId` fixes.
fn is_delegation_set_id(id: &str) -> bool {
    let mut characters = id.chars();
    characters.next() == Some('N')
        && id.len() > 1
        && id.len() <= MAX_RESOURCE_ID_CHARS
        && characters.all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
}

/// The suite's own helpers, held to the forms the rule is about: an id each service mints
/// passes, and a name or another service's id does not.
#[test]
fn the_identifier_forms_admit_only_what_route53_mints() {
    // (case, the value, whether it is a UUID, whether it is a delegation set id)
    let rows: [(&str, &str, bool, bool); 5] = [
        ("a UUID Route 53 mints", "2c1d27ef-fe93-6b0e-baea-c82774654e58", true, false),
        ("the same digits in upper case", "2C1D27EF-FE93-6B0E-BAEA-C82774654E58", false, false),
        ("a delegation set id", "N1PA6795SAMPLE", false, true),
        ("a hosted zone id", "Z1D633PJN98FT9", false, false),
        ("a collection name", "cfn-collection-one", false, false),
    ];
    for (case, value, uuid, set) in rows {
        assert_eq!(
            is_uuid(value),
            uuid,
            "case {case:?}: expected is_uuid({value:?}) to be {uuid}, got {}",
            is_uuid(value)
        );
        assert_eq!(
            is_delegation_set_id(value),
            set,
            "case {case:?}: expected is_delegation_set_id({value:?}) to be {set}, got {}",
            is_delegation_set_id(value)
        );
    }
}
