// SPDX-License-Identifier: CC0-1.0

//! Amazon EMR resolves an EMR Studio's network and service role before creating it.
//!
//! THE RULE: a Studio's Workspaces run in the VPC and subnets the create names and carry the
//! security groups it names, and the Studio itself ASSUMES the `ServiceRole` it names — so
//! `CreateStudio` resolves each of those in the service that owns it (Amazon EC2 for the
//! network objects, IAM for the role) BEFORE allocating a Studio, and refuses the request
//! with the modelled HTTP 400 `InvalidRequestException` when the owning service reports none
//! of it — or, for a subnet, reports it under another VPC than the one the Studio names —
//! persisting nothing: no Studio id is handed back and `ListStudios` never reports the
//! name. The role is resolved first, so a request that gets both wrong is answered about the
//! role. Every refusal is stated in the wrapped form the EMR Studio backend states every
//! refusal it raises in, carrying its own service name, the wire fields of that refusal and
//! this response's own request id; the refusals about a network member name the input MEMBER
//! and echo no identifier.
//!
//! The rows span what the rule generalises over: which member names something that does not
//! resolve (the VPC, one subnet of a list, either security group, the service role), where in
//! a list the unresolvable entry sits, a subnet Amazon EC2 holds under ANOTHER VPC, whether
//! the role is one IAM holds nothing under or one
//! whose trust policy admits another principal, and a request that gets the role and the
//! network both wrong. Every id a row expects to be ACCEPTED was minted by Amazon EC2 itself,
//! and every role by IAM, so nothing here asserts against a value the test made up. The rule
//! covers the other operation that names subnets too, so `UpdateStudio` is driven after the
//! table: a subnet EC2 holds nothing for is refused and the Studio keeps the subnets it had.
//!
//! The suite needs three services on one engine — EMR reaches EC2 and IAM through the
//! in-process callback, so a single-service router has nothing to ask — which is why it lives
//! here rather than in the EMR crate. Assembling the whole router costs stack (see
//! `boot_serve.rs`), so it is built — and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The IAM API version an awsQuery IAM request carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// The role EMR Studio assumes, which IAM holds with a trust policy admitting Amazon EMR.
const STUDIO_ROLE: &str = "EMR_Studio_ServiceRole";

/// A role IAM holds whose trust policy admits ANOTHER service principal, so EMR Studio may
/// not assume it.
const FOREIGN_TRUST_ROLE: &str = "EMR_Studio_LambdaTrustedRole";

/// A role name IAM holds nothing under at all.
const ABSENT_ROLE: &str = "EMR_Studio_NeverCreatedRole";

/// Send one request to the service `signing_name` addresses, and answer `(status, body)`.
fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    target: Option<&str>,
    content_type: &str,
    body: String,
) -> (u16, String) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
                 /aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One form body: the action, the version and the parameters.
fn form(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

/// Drive one ec2Query action that must succeed, and answer its body.
fn ec2_ok(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = send(
        router,
        runtime,
        "ec2",
        None,
        "application/x-www-form-urlencoded",
        form(action, EC2_API_VERSION, params),
    );
    assert_eq!(status, 200, "setup: EC2 {action} must succeed, got {body}");
    body
}

/// Drive one IAM action that must succeed.
fn iam_ok(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) {
    let (status, body) = send(
        router,
        runtime,
        "iam",
        None,
        "application/x-www-form-urlencoded",
        form(action, IAM_API_VERSION, params),
    );
    assert_eq!(status, 200, "setup: IAM {action} must succeed, got {body}");
}

/// Drive one EMR operation: `POST /` with `X-Amz-Target` and an awsJson1_1 body.
fn emr(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let (status, text) = send(
        router,
        runtime,
        "elasticmapreduce",
        Some(&format!("ElasticMapReduce.{operation}")),
        "application/x-amz-json-1.1",
        body.to_string(),
    );
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The awsJson1_1 error code, without the namespace the envelope may prefix.
fn error_code(payload: &serde_json::Value) -> String {
    payload
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(payload: &serde_json::Value) -> String {
    payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A trust policy admitting `principal` and nothing else, written compactly and with no
/// `&`, `=`, `+` or `%` so it travels in a form body as written.
fn trust_policy(principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{principal}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// This account's IAM role ARN for the role NAME `name`.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// The network Amazon EC2 holds for these cases, every id minted by EC2 itself.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    workspace_group: String,
    engine_group: String,
    /// A subnet of ANOTHER VPC the account holds, which the Studio's VPC does not contain.
    foreign_subnet: String,
}

/// An id of the same FORM Amazon EC2 mints, which this account holds no such resource
/// under: 17 lowercase hex characters after the type prefix, the way EC2 writes one.
fn unheld_id(prefix: &str) -> String {
    format!("{prefix}-0fedcba987654321f")
}

fn seed(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> Network {
    let vpc = element(
        &ec2_ok(router, runtime, "CreateVpc", &[("CidrBlock", "10.70.0.0/16")]),
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (cidr, zone) in [("10.70.1.0/24", "us-west-2a"), ("10.70.2.0/24", "us-west-2b")] {
        subnets.push(element(
            &ec2_ok(
                router,
                runtime,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", cidr),
                    ("AvailabilityZone", zone),
                ],
            ),
            "subnetId",
        ));
    }
    let group = |name: &str, description: &str| {
        element(
            &ec2_ok(
                router,
                runtime,
                "CreateSecurityGroup",
                &[
                    ("GroupName", name),
                    ("GroupDescription", description),
                    ("VpcId", vpc.as_str()),
                ],
            ),
            "groupId",
        )
    };
    let workspace_group = group("emr-studio-workspace", "the Studio Workspace group");
    let engine_group = group("emr-studio-engine", "the Studio Engine group");

    // A second VPC with a subnet of its own: a subnet the account HOLDS that the Studio's
    // VPC does not contain, which is what the membership half of the rule is about.
    let other_vpc = element(
        &ec2_ok(router, runtime, "CreateVpc", &[("CidrBlock", "10.80.0.0/16")]),
        "vpcId",
    );
    let foreign_subnet = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSubnet",
            &[
                ("VpcId", other_vpc.as_str()),
                ("CidrBlock", "10.80.1.0/24"),
                ("AvailabilityZone", "us-west-2a"),
            ],
        ),
        "subnetId",
    );

    for (role, principal) in [
        (STUDIO_ROLE, "elasticmapreduce.amazonaws.com"),
        (FOREIGN_TRUST_ROLE, "lambda.amazonaws.com"),
    ] {
        iam_ok(
            router,
            runtime,
            "CreateRole",
            &[
                ("RoleName", role),
                ("AssumeRolePolicyDocument", &trust_policy(principal)),
            ],
        );
    }

    Network {
        vpc,
        subnets,
        workspace_group,
        engine_group,
        foreign_subnet,
    }
}

/// The wrapped form the EMR Studio backend states `sentence` in, with `request_id` as the
/// request id that response reported.
fn wrapped(sentence: &str, request_id: &str) -> String {
    format!(
        "{sentence} (Service: AWSEditors; Status Code: 400; Error Code: \
         InvalidRequestException; Request ID: {request_id}; Proxy: null)"
    )
}

/// What a row expects: the Studio is created, or the create is refused with this sentence
/// inside the backend's wrapped form, whose request id the assertion reads out of the answer
/// because it is that response's own.
enum Expected {
    Created,
    Refused(String),
}

/// The request id a wrapped Studio refusal reports, asserted to be the 8-4-4-4-12
/// lowercase-hex form an AWS request id carries.
fn wrapped_request_id(case: &str, said: &str) -> String {
    let after = said.split_once("Request ID: ").unwrap_or_else(|| {
        panic!("case {case:?}: the wrapped form names a request id, got {said:?}")
    });
    let request_id = after
        .1
        .split_once(';')
        .unwrap_or_else(|| panic!("case {case:?}: the request id ends the field, got {said:?}"))
        .0
        .to_string();
    let groups: Vec<usize> = request_id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8usize, 4, 4, 4, 12],
        "case {case:?}: expected an 8-4-4-4-12 request id, got {request_id:?}"
    );
    assert!(
        request_id
            .chars()
            .all(|c| c == '-' || c.is_ascii_digit() || c.is_ascii_lowercase()),
        "case {case:?}: expected a lowercase-hex request id, got {request_id:?}"
    );
    request_id
}

/// A `CreateStudio` body for `name`, with `overrides` applied over the resolvable one.
fn studio_body(
    network: &Network,
    name: &str,
    overrides: &serde_json::Value,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "Name": name,
        "AuthMode": "IAM",
        "VpcId": network.vpc.clone(),
        "SubnetIds": [network.subnets[0].clone()],
        "ServiceRole": role_arn(STUDIO_ROLE),
        "WorkspaceSecurityGroupId": network.workspace_group.clone(),
        "EngineSecurityGroupId": network.engine_group.clone(),
        "DefaultS3Location": format!("s3://emr-studio-{name}/backup/"),
    });
    for (member, value) in overrides.as_object().expect("an override object") {
        body[member] = value.clone();
    }
    body
}

/// The Studio names `ListStudios` reports.
fn listed_studios(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
) -> Vec<String> {
    let (status, body) = emr(router, runtime, "ListStudios", &serde_json::json!({}));
    assert_eq!(status, 200, "ListStudios must answer 200, got {body}");
    body["Studios"]
        .as_array()
        .map(|studios| {
            studios
                .iter()
                .filter_map(|studio| studio["Name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_studio_is_created_only_for_a_network_and_role_the_account_holds() {
    std::thread::Builder::new()
        .name("emr-studio-network".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            let network = seed(&router, &runtime);

            let absent_vpc = unheld_id("vpc");
            let absent_subnet = unheld_id("subnet");
            let absent_group = unheld_id("sg");
            let rows: Vec<(&str, serde_json::Value, Expected)> = vec![
                (
                    "every member resolvable",
                    serde_json::json!({}),
                    Expected::Created,
                ),
                (
                    "two subnets, both of them EC2's",
                    serde_json::json!({ "SubnetIds": network.subnets.clone() }),
                    Expected::Created,
                ),
                (
                    "a VPC id the account holds no VPC under",
                    serde_json::json!({ "VpcId": absent_vpc.clone() }),
                    Expected::Refused("VpcId is not valid.".to_string()),
                ),
                (
                    "the only subnet named holds nothing",
                    serde_json::json!({ "SubnetIds": [absent_subnet.clone()] }),
                    Expected::Refused("SubnetIds is not valid.".to_string()),
                ),
                (
                    "one subnet EC2 holds and one it does not",
                    serde_json::json!({
                        "SubnetIds": [network.subnets[0].clone(), absent_subnet.clone()],
                    }),
                    Expected::Refused("SubnetIds is not valid.".to_string()),
                ),
                (
                    // "The subnets must belong to the VPC specified by VpcId"
                    // (`CreateStudio$SubnetIds` doc): a subnet the account holds, in
                    // another VPC, is no subnet this Studio may run in.
                    "a subnet the account holds, in another VPC",
                    serde_json::json!({ "SubnetIds": [network.foreign_subnet.clone()] }),
                    Expected::Refused("SubnetIds is not valid.".to_string()),
                ),
                (
                    "one subnet of this VPC and one of another",
                    serde_json::json!({
                        "SubnetIds": [
                            network.subnets[0].clone(),
                            network.foreign_subnet.clone(),
                        ],
                    }),
                    Expected::Refused("SubnetIds is not valid.".to_string()),
                ),
                (
                    "a Workspace security group the account does not hold",
                    serde_json::json!({ "WorkspaceSecurityGroupId": absent_group.clone() }),
                    Expected::Refused("WorkspaceSecurityGroupId is not valid.".to_string()),
                ),
                (
                    "an Engine security group the account does not hold",
                    serde_json::json!({ "EngineSecurityGroupId": absent_group.clone() }),
                    Expected::Refused("EngineSecurityGroupId is not valid.".to_string()),
                ),
                (
                    "a service role IAM holds nothing under",
                    serde_json::json!({ "ServiceRole": role_arn(ABSENT_ROLE) }),
                    Expected::Refused(format!(
                        "This entity does not have permissions to assume role '{}'",
                        role_arn(ABSENT_ROLE)
                    )),
                ),
                (
                    "a service role whose trust policy admits another principal",
                    serde_json::json!({ "ServiceRole": role_arn(FOREIGN_TRUST_ROLE) }),
                    Expected::Refused(format!(
                        "This entity does not have permissions to assume role '{}'",
                        role_arn(FOREIGN_TRUST_ROLE)
                    )),
                ),
                (
                    // The role is resolved FIRST, so a request that gets both wrong is
                    // answered about the role.
                    "a role the Studio may not assume and a VPC the account does not hold",
                    serde_json::json!({
                        "ServiceRole": role_arn(ABSENT_ROLE),
                        "VpcId": absent_vpc.clone(),
                    }),
                    Expected::Refused(format!(
                        "This entity does not have permissions to assume role '{}'",
                        role_arn(ABSENT_ROLE)
                    )),
                ),
            ];

            for (index, (case, overrides, expected)) in rows.iter().enumerate() {
                let name = format!("studio-{index}");
                let body = studio_body(&network, &name, overrides);
                let (status, payload) = emr(&router, &runtime, "CreateStudio", &body);
                let listed = listed_studios(&router, &runtime);
                match expected {
                    Expected::Created => {
                        assert_eq!(
                            status, 200,
                            "case {case:?}: CreateStudio must succeed, got {status} {payload}"
                        );
                        assert!(
                            payload["StudioId"].as_str().is_some(),
                            "case {case:?}: an accepted create answers a StudioId, got \
                             {payload}"
                        );
                        assert!(
                            listed.contains(&name),
                            "case {case:?}: the Studio must be listed, got {listed:?}"
                        );
                    }
                    Expected::Refused(sentence) => {
                        assert_eq!(
                            (status, error_code(&payload).as_str()),
                            (400, "InvalidRequestException"),
                            "case {case:?}: an unresolvable member must be refused 400 \
                             InvalidRequestException, got {status} {payload}"
                        );
                        let said = error_message(&payload);
                        let request_id = wrapped_request_id(case, &said);
                        assert_eq!(
                            said,
                            wrapped(sentence, &request_id),
                            "case {case:?}: expected {:?}, got {payload}",
                            wrapped(sentence, "<this response's request id>")
                        );
                        assert!(
                            payload.get("StudioId").is_none(),
                            "case {case:?}: a refused create hands back no StudioId, got \
                             {payload}"
                        );
                        assert!(
                            !listed.contains(&name),
                            "case {case:?}: a refused create leaves no Studio, got {listed:?}"
                        );
                    }
                }
            }

            // The same rule on the OTHER operation that names subnets: `UpdateStudio` may
            // change a Studio's subnet list, so an entry Amazon EC2 holds no subnet for is
            // refused there too, and the Studio keeps the subnets it had.
            let created = emr(
                &router,
                &runtime,
                "CreateStudio",
                &studio_body(&network, "studio-updated", &serde_json::json!({})),
            );
            assert_eq!(created.0, 200, "setup: CreateStudio: got {:?}", created);
            let studio_id = created
                .1["StudioId"]
                .as_str()
                .expect("setup: CreateStudio answers a StudioId")
                .to_string();

            let (status, payload) = emr(
                &router,
                &runtime,
                "UpdateStudio",
                &serde_json::json!({
                    "StudioId": studio_id.clone(),
                    "SubnetIds": [network.subnets[0].clone(), absent_subnet.clone()],
                }),
            );
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (400, "InvalidRequestException"),
                "case \"UpdateStudio with a subnet EC2 does not hold\": got {status} {payload}"
            );
            assert_eq!(
                error_message(&payload),
                wrapped(
                    "SubnetIds is not valid.",
                    &wrapped_request_id(
                        "UpdateStudio with a subnet EC2 does not hold",
                        &error_message(&payload)
                    )
                ),
                "case \"UpdateStudio with a subnet EC2 does not hold\": got {payload}"
            );
            let (status, described) = emr(
                &router,
                &runtime,
                "DescribeStudio",
                &serde_json::json!({ "StudioId": studio_id.clone() }),
            );
            assert_eq!(status, 200, "DescribeStudio must answer 200, got {described}");
            assert_eq!(
                described["Studio"]["SubnetIds"],
                serde_json::json!([network.subnets[0].clone()]),
                "case \"UpdateStudio with a subnet EC2 does not hold\": a refused update \
                 leaves the Studio's subnets, got {described}"
            );

            // The same update naming only subnets EC2 holds is applied and reads back.
            let (status, payload) = emr(
                &router,
                &runtime,
                "UpdateStudio",
                &serde_json::json!({
                    "StudioId": studio_id.clone(),
                    "SubnetIds": network.subnets.clone(),
                }),
            );
            assert!(
                status == 200 || status == 204,
                "case \"UpdateStudio with subnets EC2 holds\": got {status} {payload}"
            );
            let (status, described) = emr(
                &router,
                &runtime,
                "DescribeStudio",
                &serde_json::json!({ "StudioId": studio_id }),
            );
            assert_eq!(status, 200, "DescribeStudio must answer 200, got {described}");
            assert_eq!(
                described["Studio"]["SubnetIds"],
                serde_json::json!(network.subnets.clone()),
                "case \"UpdateStudio with subnets EC2 holds\": the update must be applied, \
                 got {described}"
            );
        })
        .expect("spawn the EMR Studio network thread")
        .join()
        .expect("every Studio's network and role must be resolved where they live");
}
