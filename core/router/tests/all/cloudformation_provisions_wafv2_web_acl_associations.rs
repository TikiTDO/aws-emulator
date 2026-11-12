// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares a BINDING between resources really makes that
//! binding in the owning service, and the identifier it publishes for the binding is the
//! pair it bound.
//!
//! The rule under test: **where a resource type's primary identifier is COMPOSITE — the type
//! declares no resource of its own but a binding between resources — the physical resource id
//! the stack publishes for one, which is what `Ref` returns, is those property values joined
//! with `|` in the order the type publishes them; so a declaration that re-points either side
//! names a different resource, which the deployment replaces, and the record the stack's own
//! deletion acts on carries exactly the pair to unbind.** An
//! `AWS::WAFv2::WebACLAssociation` is `<ResourceArn>|<WebACLArn>`, the pair `AssociateWebACL`
//! takes and `DisassociateWebACL` removes; an `AWS::EKS::AccessEntry` is
//! `<ClusterName>|<PrincipalArn>`, the pair every Amazon EKS operation on an access entry
//! addresses one by. Several crates have to agree for that to hold (CloudFormation makes the
//! calls, AWS WAF and Amazon EKS serve them), so the suite lives here: a crate-local test can
//! only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: which kind of protected resource the
//! binding names (an Amazon Cognito user pool the same stack declares, an API Gateway stage,
//! an Application Load Balancer, an Amplify app, an Amazon Web Services Verified Access
//! instance — each in the ARN format `AssociateWebACLRequest$ResourceArn` documents for it),
//! whether the ARN is written literally or comes from an `Fn::GetAtt` to another resource of
//! the same stack, how many bindings one stack declares, which spelling the template is in
//! (JSON and YAML), and — for the other type the rule covers — which pair identifies the
//! binding at all. Every expected identifier is computed from what the row declares plus the
//! ARN AWS WAF minted for the web ACL, never read back out of the answer being checked.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire helpers this test crate publishes ONCE, in `query_wire`: a suite that
// writes its own percent-encoder or element reader drifts from the others.
use crate::query_wire::{elements, form_encode, opt_element};

/// The region every request here is signed for, and so the region the stack's resources
/// belong to.
const REGION: &str = "us-west-2";

/// The separator CloudFormation joins the parts of a composite physical resource id with.
const COMPOSITE_SEPARATOR: &str = "|";

/// Long enough for the clock-owed spans these services choose to come due: the stack's
/// (500 ms) and an Amazon EKS cluster's.
const PAST_THE_SPANS: Duration = Duration::from_millis(1_200);

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_waf_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(
        Arc::clone(&engine),
    ));
    // Amazon Cognito user pools, for the `AWS::Cognito::UserPool` one row declares beside the
    // web ACL it binds: that pool is really created there
    // (`state::provisioning::cognito_user_pool`), so its owning service has to be mounted
    // beside CloudFormation or the create is refused and the stack fails.
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::
            EmulatedService::new(engine),
    );
    router
}

/// The same emulator for the OTHER type the rule covers: an Amazon EKS access entry needs
/// that service and the EC2 one its host cluster's subnets live in.
fn build_eks_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One answer, as this suite reads it: the status and the body as text and as JSON.
struct Answer {
    status: u16,
    text: String,
    body: Value,
}

/// Drive one request at `signing_name`'s wire.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    body: String,
    target: Option<String>,
) -> Answer {
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, body }
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    send(
        router,
        "cloudformation",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        body,
        None,
    )
    .await
}

/// Call one WAFv2 operation over its awsJson1_1 wire.
async fn wafv2(router: &MultiServiceRouter, operation: &str, request: &Value) -> Answer {
    send(
        router,
        "wafv2",
        "POST",
        "/",
        "application/x-amz-json-1.1",
        serde_json::to_string(request).expect("the request serializes"),
        Some(format!("AWSWAF_20190729.{operation}")),
    )
    .await
}

/// One WAFv2 call that must succeed.
async fn waf_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let answer = wafv2(router, operation, &request).await;
    assert!(
        answer.status == 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {}",
        answer.status,
        answer.text
    );
    answer.body
}

/// Call one EC2 action over its ec2Query wire.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> Answer {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={}", form_encode(value)));
    }
    send(
        router,
        "ec2",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        body,
        None,
    )
    .await
}

/// Call one Amazon EKS operation over its restJson1 wire.
async fn eks(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|json| json.to_string()).unwrap_or_default();
    send(
        router,
        "eks",
        method,
        uri,
        "application/json",
        payload,
        None,
    )
    .await
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> std::collections::BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Deploy `template` as a new stack, wait past the clock-owed spans and answer the
/// `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let created = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert!(
        created.status == 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {} with body {}",
        created.status,
        created.text
    );
    tokio::time::sleep(PAST_THE_SPANS).await;
    describe_stack(router, case, stack_name).await
}

/// The `DescribeStacks` body for one stack.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let described = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert!(
        described.status == 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {} with body {}",
        described.status,
        described.text
    );
    described.text
}

/// The ARN of a protected resource of each kind the catalogue in
/// `AssociateWebACLRequest$ResourceArn` publishes ("the ARN must be in one of the following
/// formats"), written out from that format rather than read off any answer.
fn stage_arn(api_id: &str, stage: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::/restapis/{api_id}/stages/{stage}")
}

fn load_balancer_arn(name: &str, id: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:loadbalancer/app/{name}/{id}",
        engine::emulator_account_id()
    )
}

fn amplify_app_arn(app_id: &str) -> String {
    format!(
        "arn:aws:amplify:{REGION}:{}:apps/{app_id}",
        engine::emulator_account_id()
    )
}

fn verified_access_instance_arn(instance_id: &str) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:verified-access-instance/{instance_id}",
        engine::emulator_account_id()
    )
}

/// An Amazon SQS queue's ARN: a resource kind that catalogue does NOT publish, which is
/// what the refusal half declares.
fn queue_arn(name: &str) -> String {
    format!(
        "arn:aws:sqs:{REGION}:{}:{name}",
        engine::emulator_account_id()
    )
}

/// An IAM role ARN of the account this emulator serves, which is what an access entry's
/// principal is.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// The declaration of one REGIONAL web ACL, with the two properties that type requires.
fn web_acl(name: &str) -> Value {
    json!({
        "Type": "AWS::WAFv2::WebACL",
        "Properties": {
            "Name": name,
            "Scope": "REGIONAL",
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": {
                "SampledRequestsEnabled": false,
                "CloudWatchMetricsEnabled": false,
                "MetricName": name
            }
        }
    })
}

/// What one binding a row declares must hold afterwards.
struct Binding {
    /// The output carrying `{"Ref": <the association's logical id>}`.
    ref_output: &'static str,
    /// The output carrying the ARN of the web ACL this binding puts in front of the
    /// resource.
    web_acl_arn_output: &'static str,
    /// The protected resource's ARN as the row declares it, or `None` where the stack
    /// itself declares the resource and the row reads its ARN from `resource_arn_output`.
    resource_arn: Option<fn() -> String>,
    resource_arn_output: &'static str,
    /// The `ResourceType` `ListResourcesForWebACL` lists that kind of resource under
    /// (`smithy_query(query="shape",shape="ResourceType")`).
    resource_type: &'static str,
}

/// One row: a template, and the bindings it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: fn() -> String,
    bindings: &'static [Binding],
}

const ROWS: &[Row] = &[
    Row {
        // The reported case: a web ACL put in front of a user pool the same stack declares,
        // with `!Ref` of the association in the outputs.
        case: "an Amazon Cognito user pool of the same stack, bound by Fn::GetAtt",
        stack_name: "wafv2-assoc-pool",
        template: || {
            json!({
                "Resources": {
                    "Pool": { "Type": "AWS::Cognito::UserPool", "Properties": {} },
                    "Acl": web_acl("assoc-pool-acl"),
                    "Assoc": {
                        "Type": "AWS::WAFv2::WebACLAssociation",
                        "Properties": {
                            "ResourceArn": { "Fn::GetAtt": ["Pool", "Arn"] },
                            "WebACLArn": { "Fn::GetAtt": ["Acl", "Arn"] }
                        }
                    }
                },
                "Outputs": {
                    "PoolArn": { "Value": { "Fn::GetAtt": ["Pool", "Arn"] } },
                    "AclArn": { "Value": { "Fn::GetAtt": ["Acl", "Arn"] } },
                    "AssocRef": { "Value": { "Ref": "Assoc" } }
                }
            })
            .to_string()
        },
        bindings: &[Binding {
            ref_output: "AssocRef",
            web_acl_arn_output: "AclArn",
            resource_arn: None,
            resource_arn_output: "PoolArn",
            resource_type: "COGNITO_USER_POOL",
        }],
    },
    Row {
        case: "an API Gateway stage ARN written literally",
        stack_name: "wafv2-assoc-stage",
        template: || {
            json!({
                "Resources": {
                    "Acl": web_acl("assoc-stage-acl"),
                    "StageAssoc": {
                        "Type": "AWS::WAFv2::WebACLAssociation",
                        "Properties": {
                            "ResourceArn": stage_arn("a1b2c3d4e5", "prod"),
                            "WebACLArn": { "Fn::GetAtt": ["Acl", "Arn"] }
                        }
                    }
                },
                "Outputs": {
                    "AclArn": { "Value": { "Fn::GetAtt": ["Acl", "Arn"] } },
                    "StageRef": { "Value": { "Ref": "StageAssoc" } }
                }
            })
            .to_string()
        },
        bindings: &[Binding {
            ref_output: "StageRef",
            web_acl_arn_output: "AclArn",
            resource_arn: Some(|| stage_arn("a1b2c3d4e5", "prod")),
            resource_arn_output: "",
            resource_type: "API_GATEWAY",
        }],
    },
    Row {
        case: "an Application Load Balancer, in the YAML spelling",
        stack_name: "wafv2-assoc-alb",
        template: || {
            format!(
                concat!(
                    "Resources:\n",
                    "  Acl:\n",
                    "    Type: AWS::WAFv2::WebACL\n",
                    "    Properties:\n",
                    "      Name: assoc-alb-acl\n",
                    "      Scope: REGIONAL\n",
                    "      DefaultAction:\n",
                    "        Block: {{}}\n",
                    "      VisibilityConfig:\n",
                    "        SampledRequestsEnabled: false\n",
                    "        CloudWatchMetricsEnabled: false\n",
                    "        MetricName: assocAlbAcl\n",
                    "  BalancerAssoc:\n",
                    "    Type: AWS::WAFv2::WebACLAssociation\n",
                    "    Properties:\n",
                    "      ResourceArn: {balancer}\n",
                    "      WebACLArn: !GetAtt Acl.Arn\n",
                    "Outputs:\n",
                    "  AclArn:\n",
                    "    Value: !GetAtt Acl.Arn\n",
                    "  BalancerRef:\n",
                    "    Value: !Ref BalancerAssoc\n",
                ),
                balancer = load_balancer_arn("assoc-balancer", "0123456789abcdef"),
            )
        },
        bindings: &[Binding {
            ref_output: "BalancerRef",
            web_acl_arn_output: "AclArn",
            resource_arn: Some(|| load_balancer_arn("assoc-balancer", "0123456789abcdef")),
            resource_arn_output: "",
            resource_type: "APPLICATION_LOAD_BALANCER",
        }],
    },
    Row {
        case: "two bindings in one stack: an Amplify app and a Verified Access instance \
               behind two web ACLs",
        stack_name: "wafv2-assoc-pair",
        template: || {
            json!({
                "Resources": {
                    "FrontAcl": web_acl("assoc-front-acl"),
                    "EdgeAcl": web_acl("assoc-edge-acl"),
                    "AmplifyAssoc": {
                        "Type": "AWS::WAFv2::WebACLAssociation",
                        "Properties": {
                            "ResourceArn": amplify_app_arn("d1a2b3c4d5e6f7"),
                            "WebACLArn": { "Fn::GetAtt": ["FrontAcl", "Arn"] }
                        }
                    },
                    "AccessAssoc": {
                        "Type": "AWS::WAFv2::WebACLAssociation",
                        "Properties": {
                            "ResourceArn": verified_access_instance_arn("vai-0123456789abcdef0"),
                            "WebACLArn": { "Fn::GetAtt": ["EdgeAcl", "Arn"] }
                        }
                    }
                },
                "Outputs": {
                    "FrontAclArn": { "Value": { "Fn::GetAtt": ["FrontAcl", "Arn"] } },
                    "EdgeAclArn": { "Value": { "Fn::GetAtt": ["EdgeAcl", "Arn"] } },
                    "AmplifyRef": { "Value": { "Ref": "AmplifyAssoc" } },
                    "AccessRef": { "Value": { "Ref": "AccessAssoc" } }
                }
            })
            .to_string()
        },
        bindings: &[
            Binding {
                ref_output: "AmplifyRef",
                web_acl_arn_output: "FrontAclArn",
                resource_arn: Some(|| amplify_app_arn("d1a2b3c4d5e6f7")),
                resource_arn_output: "",
                resource_type: "AMPLIFY",
            },
            Binding {
                ref_output: "AccessRef",
                web_acl_arn_output: "EdgeAclArn",
                resource_arn: Some(|| verified_access_instance_arn("vai-0123456789abcdef0")),
                resource_arn_output: "",
                resource_type: "VERIFIED_ACCESS_INSTANCE",
            },
        ],
    },
];

/// The ARN `GetWebACLForResource` reports for the web ACL in front of `resource_arn`, or
/// `None` for a resource nothing is in front of.
async fn web_acl_in_front_of(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> Option<String> {
    let answer = waf_ok(
        router,
        case,
        "GetWebACLForResource",
        json!({ "ResourceArn": resource_arn }),
    )
    .await;
    answer
        .get("WebACL")
        .and_then(|web_acl| web_acl.get("ARN"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The resources `ListResourcesForWebACL` reports behind `web_acl_arn`.
async fn resources_behind(
    router: &MultiServiceRouter,
    case: &str,
    web_acl_arn: &str,
    resource_type: &str,
) -> BTreeSet<String> {
    let answer = waf_ok(
        router,
        case,
        "ListResourcesForWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceType": resource_type }),
    )
    .await;
    answer
        .get("ResourceArns")
        .and_then(Value::as_array)
        .map(|arns| {
            arns.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every row: the stack completes, `Ref` of each association is the `<ResourceArn>|<WebACLArn>`
/// pair it bound, and AWS WAF itself really has that web ACL in front of that resource.
#[tokio::test]
async fn a_web_acl_association_is_published_as_the_pair_it_binds() {
    let router = build_waf_router();

    for row in ROWS {
        let body = create_and_settle(&router, row.case, row.stack_name, &(row.template)()).await;
        assert!(
            opt_element(&body, "StackStatus").as_deref() == Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);

        for binding in row.bindings {
            let case = format!("{} [{}]", row.case, binding.ref_output);
            let output = |key: &str| {
                outputs
                    .get(key)
                    .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                    .clone()
            };
            let web_acl_arn = output(binding.web_acl_arn_output);
            let resource_arn = match binding.resource_arn {
                Some(declared) => declared(),
                None => output(binding.resource_arn_output),
            };

            // `Ref` is the pair, in the order the type publishes it — not a name
            // CloudFormation generated, which identifies no binding at all.
            let expected = format!("{resource_arn}{COMPOSITE_SEPARATOR}{web_acl_arn}");
            let reported = output(binding.ref_output);
            assert!(
                reported == expected,
                "case {case:?}: expected Ref of the association to be the pair it bound \
                 ({expected:?}), got {reported:?}"
            );

            // ...and the binding is real in AWS WAF, readable through that service's own
            // API from either end.
            let in_front = web_acl_in_front_of(&router, &case, &resource_arn).await;
            assert!(
                in_front.as_deref() == Some(web_acl_arn.as_str()),
                "case {case:?}: expected AWS WAF to have the web ACL {web_acl_arn:?} in \
                 front of {resource_arn:?}, got {in_front:?}"
            );
            let behind =
                resources_behind(&router, &case, &web_acl_arn, binding.resource_type).await;
            assert!(
                behind == BTreeSet::from([resource_arn.clone()]),
                "case {case:?}: expected exactly {resource_arn:?} behind the web ACL, got \
                 {behind:?}"
            );
        }
    }
}

/// The template a re-pointing stack deploys: a user pool, two web ACLs, and an association
/// that puts the one `acl` names in front of the pool.
fn repointed_template(acl: &str) -> String {
    json!({
        "Resources": {
            "Pool": { "Type": "AWS::Cognito::UserPool", "Properties": {} },
            "AclA": web_acl("repointed-acl-a"),
            "AclB": web_acl("repointed-acl-b"),
            "Assoc": {
                "Type": "AWS::WAFv2::WebACLAssociation",
                "Properties": {
                    "ResourceArn": { "Fn::GetAtt": ["Pool", "Arn"] },
                    "WebACLArn": { "Fn::GetAtt": [acl, "Arn"] }
                }
            }
        },
        "Outputs": {
            "PoolArn": { "Value": { "Fn::GetAtt": ["Pool", "Arn"] } },
            "AclAArn": { "Value": { "Fn::GetAtt": ["AclA", "Arn"] } },
            "AclBArn": { "Value": { "Fn::GetAtt": ["AclB", "Arn"] } },
            "AssocRef": { "Value": { "Ref": "Assoc" } }
        }
    })
    .to_string()
}

/// A declaration that re-points one side of the binding names a DIFFERENT resource: the
/// deployment replaces it, the new `Ref` names the new pair, and the stack's own deletion
/// unbinds exactly that pair.
#[tokio::test]
async fn re_pointing_a_binding_replaces_it_and_the_new_ref_names_the_new_pair() {
    let router = build_waf_router();
    let case = "a web ACL association re-pointed from one web ACL to another";
    let stack_name = "wafv2-assoc-repointed";

    let body = create_and_settle(&router, case, stack_name, &repointed_template("AclA")).await;
    assert!(
        opt_element(&body, "StackStatus").as_deref() == Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    let pool_arn = outputs
        .get("PoolArn")
        .unwrap_or_else(|| panic!("case {case:?}: no PoolArn output in {body}"))
        .clone();
    let acl_a = outputs
        .get("AclAArn")
        .unwrap_or_else(|| panic!("case {case:?}: no AclAArn output in {body}"))
        .clone();
    let acl_b = outputs
        .get("AclBArn")
        .unwrap_or_else(|| panic!("case {case:?}: no AclBArn output in {body}"))
        .clone();
    let before = outputs.get("AssocRef").cloned().unwrap_or_default();
    assert!(
        before == format!("{pool_arn}{COMPOSITE_SEPARATOR}{acl_a}"),
        "case {case:?}: expected the first Ref to name the first pair, got {before:?}"
    );

    // The same template with the association's `WebACLArn` pointing at the other web ACL.
    let updated = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &repointed_template("AclB")),
        ],
    )
    .await;
    assert!(
        updated.status == 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {} with body {}",
        updated.status,
        updated.text
    );
    tokio::time::sleep(PAST_THE_SPANS).await;
    let body = describe_stack(&router, case, stack_name).await;
    assert!(
        opt_element(&body, "StackStatus").as_deref() == Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the re-pointing update to complete, got {body}"
    );
    let after = outputs_of(&body).get("AssocRef").cloned().unwrap_or_default();
    assert!(
        after == format!("{pool_arn}{COMPOSITE_SEPARATOR}{acl_b}"),
        "case {case:?}: expected the re-pointed Ref to name the new pair \
         ({pool_arn}{COMPOSITE_SEPARATOR}{acl_b}), got {after:?}"
    );

    // AWS WAF has the NEW web ACL in front of the pool, and the old one is in front of
    // nothing: the replacement's cleanup removed the pair the old record named, not the
    // binding the new one made.
    let in_front = web_acl_in_front_of(&router, case, &pool_arn).await;
    assert!(
        in_front.as_deref() == Some(acl_b.as_str()),
        "case {case:?}: expected the pool to be behind the re-pointed web ACL {acl_b:?}, \
         got {in_front:?}"
    );
    let behind_old = resources_behind(&router, case, &acl_a, "COGNITO_USER_POOL").await;
    assert!(
        behind_old.is_empty(),
        "case {case:?}: expected nothing behind the web ACL the template no longer names, \
         got {behind_old:?}"
    );

    // Deleting the stack unbinds the pair the record carries, so nothing is in front of the
    // pool any more.
    let deleted = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert!(
        deleted.status == 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {} with body {}",
        deleted.status,
        deleted.text
    );
    tokio::time::sleep(PAST_THE_SPANS).await;
    let in_front = web_acl_in_front_of(&router, case, &pool_arn).await;
    assert!(
        in_front.is_none(),
        "case {case:?}: expected the stack's deletion to unbind the pool, got {in_front:?}"
    );
}

/// One refusal row: the protected-resource ARN a template declares, and why AWS WAF will
/// not put a web ACL in front of it.
struct RefusalRow {
    case: &'static str,
    stack_name: &'static str,
    resource_arn: fn() -> String,
}

const REFUSAL_ROWS: &[RefusalRow] = &[
    RefusalRow {
        // The reported case: a resource kind WAF can never front.
        case: "an Amazon SQS queue, a resource kind the ResourceArn catalogue does not list",
        stack_name: "wafv2-assoc-queue",
        resource_arn: || queue_arn("assoc-refused-queue"),
    },
    RefusalRow {
        case: "a well-formed ARN of a listed service in a resource form it does not publish",
        stack_name: "wafv2-assoc-wrong-form",
        resource_arn: || {
            format!(
                "arn:aws:cognito-idp:{REGION}:{}:identitypool/{REGION}_abcdef",
                engine::emulator_account_id()
            )
        },
    },
    RefusalRow {
        case: "a value that is no ARN at all",
        stack_name: "wafv2-assoc-not-an-arn",
        resource_arn: || "a-protected-resource".to_string(),
    },
];

/// Every refusal row: AWS WAF refuses the pair, so the resource fails and the stack rolls
/// back — reporting that service's own refusal on the resource's event — rather than
/// completing with a protection nothing could apply.
#[tokio::test]
async fn a_protected_resource_waf_refuses_rolls_the_stack_back() {
    let router = build_waf_router();

    for row in REFUSAL_ROWS {
        let resource_arn = (row.resource_arn)();
        let template = json!({
            "Resources": {
                "Acl": web_acl(&format!("{}-acl", row.stack_name)),
                "Assoc": {
                    "Type": "AWS::WAFv2::WebACLAssociation",
                    "Properties": {
                        "ResourceArn": resource_arn,
                        "WebACLArn": { "Fn::GetAtt": ["Acl", "Arn"] }
                    }
                }
            }
        })
        .to_string();
        let body = create_and_settle(&router, row.case, row.stack_name, &template).await;
        assert!(
            opt_element(&body, "StackStatus").as_deref() == Some("ROLLBACK_COMPLETE"),
            "case {:?}: expected the failed create to roll back rather than complete, got \
             {body}",
            row.case
        );
        // The resource's own event carries AWS WAF's refusal, naming the parameter it
        // rejected and the value it rejected it for.
        let events = cloudformation(
            &router,
            "DescribeStackEvents",
            &[("StackName", row.stack_name)],
        )
        .await
        .text;
        let reasons = elements(&events, "ResourceStatusReason");
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("RESOURCE_ARN") && reason.contains(&resource_arn)),
            "case {:?}: expected the failing resource's event to carry WAF's refusal of \
             {resource_arn:?}, got {reasons:?}",
            row.case
        );
    }
}

/// Create the VPC and the two subnets an Amazon EKS cluster is placed in, through EC2
/// itself, and answer the subnet ids: Amazon EKS resolves every subnet a cluster names
/// against EC2, so they have to exist before any cluster can be created.
async fn given_subnets(router: &MultiServiceRouter) -> Vec<String> {
    let created = ec2(router, "CreateVpc", &[("CidrBlock", "10.9.0.0/16")]).await;
    assert!(
        created.status == 200,
        "setup: CreateVpc must succeed, got {} with body {}",
        created.status,
        created.text
    );
    let vpc_id = elements(&created.text, "vpcId")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("setup: no <vpcId> in {}", created.text));
    let mut subnets = Vec::new();
    for cidr_block in ["10.9.1.0/24", "10.9.2.0/24"] {
        let answer = ec2(
            router,
            "CreateSubnet",
            &[("VpcId", &vpc_id), ("CidrBlock", cidr_block)],
        )
        .await;
        assert!(
            answer.status == 200,
            "setup: CreateSubnet {cidr_block} must succeed, got {} with body {}",
            answer.status,
            answer.text
        );
        subnets.push(
            elements(&answer.text, "subnetId")
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("setup: no <subnetId> in {}", answer.text)),
        );
    }
    subnets
}

/// The OTHER type the rule covers: an `AWS::EKS::AccessEntry`'s `Ref` is the
/// `<ClusterName>|<PrincipalArn>` pair it binds, and Amazon EKS really holds the entry that
/// pair addresses.
#[tokio::test]
async fn an_access_entry_is_published_as_the_pair_it_binds() {
    let router = build_eks_router();
    let case = "an Amazon EKS access entry for one principal on one cluster";
    let cluster = "composite-identity-host";
    let principal = role_arn("composite-identity-principal");
    let subnets = given_subnets(&router).await;
    let created = eks(
        &router,
        "POST",
        "/clusters",
        Some(json!({
            "name": cluster,
            "roleArn": role_arn("eksClusterRole"),
            "resourcesVpcConfig": { "subnetIds": subnets },
            "accessConfig": { "authenticationMode": "API_AND_CONFIG_MAP" },
        })),
    )
    .await;
    assert!(
        created.status == 200,
        "setup: CreateCluster must succeed, got {} with body {}",
        created.status,
        created.text
    );

    let template = json!({
        "Resources": {
            "Entry": {
                "Type": "AWS::EKS::AccessEntry",
                "Properties": { "ClusterName": cluster, "PrincipalArn": principal }
            }
        },
        "Outputs": { "EntryRef": { "Value": { "Ref": "Entry" } } }
    })
    .to_string();
    let body = create_and_settle(&router, case, "eks-access-entry-pair", &template).await;
    assert!(
        opt_element(&body, "StackStatus").as_deref() == Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let reported = outputs_of(&body).get("EntryRef").cloned().unwrap_or_default();
    let expected = format!("{cluster}{COMPOSITE_SEPARATOR}{principal}");
    assert!(
        reported == expected,
        "case {case:?}: expected Ref of the access entry to be the pair it bound \
         ({expected:?}), got {reported:?}"
    );

    // ...and that pair is what addresses the entry in Amazon EKS itself.
    let (declared_cluster, declared_principal) = reported
        .split_once(COMPOSITE_SEPARATOR)
        .unwrap_or_else(|| panic!("case {case:?}: {reported:?} is not a pair"));
    let read = eks(
        &router,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}",
            form_encode(declared_cluster),
            form_encode(declared_principal)
        ),
        None,
    )
    .await;
    assert!(
        read.status == 200,
        "case {case:?}: expected the pair the stack published to address the entry, got {} \
         with body {}",
        read.status,
        read.text
    );
}
