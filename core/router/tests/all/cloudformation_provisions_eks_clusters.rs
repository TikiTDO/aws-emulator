// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::EKS::Cluster` really creates that cluster
//! in Amazon EKS — and a stack that declares an `AWS::EKS::PodIdentityAssociation` really
//! associates that role with the declared service account.
//!
//! The rule under test: **a resource type CloudFormation provisions is created through the
//! owning service's own API, with every call that resource's declaration asks for, and what
//! that service answers is what the stack reports** — so the resource is independently
//! readable through that service's own describe, and a declaration the owning service (or
//! this emulator) cannot satisfy fails the resource and the stack rather than completing
//! it. For a cluster the stack additionally HOLDS the resource's creation open until Amazon
//! EKS reports the cluster `ACTIVE` ("you must wait for the cluster status to become ACTIVE
//! before you can … add nodes" — `CreateCluster`), so a stack that reports
//! `CREATE_COMPLETE` names a cluster that is usable. Four crates have to agree for that to
//! hold (CloudFormation makes the calls, Amazon EKS serves them, EC2 owns the VPC and the
//! subnets the cluster is placed in, and the stack's `Fn::GetAtt` reports what EKS minted),
//! so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: whether the template declares the
//! cluster's `Name` or leaves it for CloudFormation to generate, whether it declares a
//! Kubernetes version, how many subnets and security groups the VPC configuration names,
//! whether the endpoint switches and tags are declared, and — for the association — whether
//! a target role and session tags are. Every expected value is computed from what the row
//! declares.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for, and so the Region the stack, the VPC and
/// the cluster belong to.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// How long one poll waits before reading the stack again, and how many times: a cluster's
/// activation and the stack's own completion are both owed from an instant no fixed sleep
/// can name, so the suite polls rather than sleeping once.
const POLL_INTERVAL: Duration = Duration::from_millis(150);
const POLLS: usize = 40;

/// The `StackStatus` a completed create reports.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `StackStatus` a create that could not be completed reports once CloudFormation has
/// rolled it back: "if the stack creation fails, the stack is rolled back" — the failed
/// resource's own status is `CREATE_FAILED`, and the STACK settles at `ROLLBACK_COMPLETE`.
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The `Cluster$status` a cluster is usable in.
const CLUSTER_ACTIVE: &str = "ACTIVE";

/// The emulator with the three services this suite exercises, mounted on ONE engine
/// exactly as `core/router/src/lib.rs` mounts them. Request authentication is off, as in
/// every other suite here; the in-process call CloudFormation makes signs its request
/// either way (the credential scope is what routes it).
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
        service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One answer, as this suite reads it: the status, the body as text and as JSON.
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
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, body }
}

/// Percent-encode one value: the test crate's single encoder.
fn encoded(value: &str) -> String {
    crate::query_wire::form_encode(value)
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
        body.push_str(&encoded(key));
        body.push('=');
        body.push_str(&encoded(value));
    }
    send(
        router,
        "cloudformation",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        body,
    )
    .await
}

/// Call one EC2 action over its ec2Query wire.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> Answer {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push_str(&format!("&{name}={}", encoded(value)));
    }
    send(
        router,
        "ec2",
        "POST",
        "/",
        "application/x-www-form-urlencoded",
        body,
    )
    .await
}

/// Call one Amazon EKS operation over its restJson1 wire.
async fn eks(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|json| json.to_string()).unwrap_or_default();
    send(router, "eks", method, uri, "application/json", payload).await
}

/// The text of the first `<name>…</name>` element of an XML body.
fn element(xml: &str, name: &str) -> Option<String> {
    crate::query_wire::elements(xml, name).into_iter().next()
}

/// Every `<name>…</name>` element of an XML body.
fn elements(xml: &str, name: &str) -> Vec<String> {
    crate::query_wire::elements(xml, name)
}

/// A role ARN of the account this emulator serves, which is what a cluster's `RoleArn` and
/// an association's are.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// Create a stack from `template`, failing the case with the response body when the create
/// itself is refused.
async fn create_stack(router: &MultiServiceRouter, case: &str, stack_name: &str, template: &str) {
    let answer = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {} with body {}",
        answer.status, answer.text
    );
}

/// The `StackStatus` the stack reports right now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let answer = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {} with body {}",
        answer.status, answer.text
    );
    (
        element(&answer.text, "StackStatus").unwrap_or_default(),
        answer.text,
    )
}

/// Poll `stack` until it first reports `wanted`, and answer the last status it reported
/// with the answer that status was read from: a cluster's activation is owed from an
/// instant no fixed sleep can name.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let mut last = stack_status(router, case, stack).await;
    for _ in 0..POLLS {
        if last.0 == wanted {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, case, stack).await;
    }
    last
}

/// The outputs of a stack as `{OutputKey: OutputValue}` — the hand-off every
/// outputs-driven client reads, which is why each entry has to carry BOTH members.
fn stack_outputs(case: &str, body: &str) -> Vec<(String, String)> {
    let keys = elements(body, "OutputKey");
    let values = elements(body, "OutputValue");
    assert_eq!(
        keys.len(),
        values.len(),
        "case {case:?}: expected every output to carry an OutputValue beside its \
         OutputKey, got {keys:?} against {values:?} in {body}"
    );
    keys.into_iter().zip(values).collect()
}

/// The value of one output, or a panic naming the case.
fn output(case: &str, body: &str, key: &str) -> String {
    stack_outputs(case, body)
        .into_iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
}

/// `DescribeCluster` for one cluster.
async fn describe_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    eks(
        router,
        "GET",
        &format!("/clusters/{}", encoded(name)),
        None,
    )
    .await
}

/// The cluster names `ListClusters` reports.
async fn list_clusters(router: &MultiServiceRouter) -> Vec<String> {
    let answer = eks(router, "GET", "/clusters", None).await;
    answer.body["clusters"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The logical ids every template below declares its resources under.
const VPC_ID: &str = "Net";
const SUBNET_IDS: [&str; 3] = ["SubnetA", "SubnetB", "SubnetC"];
const CLUSTER_ID: &str = "Cluster";
const ASSOCIATION_ID: &str = "Association";

/// The Availability Zones the subnets of a row are created in, in declaration order: a
/// cluster's control plane is placed across the zones of the subnets it names, so the rows
/// that declare more than two subnets spread them.
const ZONES: [&str; 3] = ["us-west-2a", "us-west-2b", "us-west-2c"];

/// The network half of every template: the VPC and the subnets the cluster is placed in,
/// each created in EC2 by the stack itself, so the cluster's `SubnetIds` are ids EC2
/// assigned rather than values the template invented.
fn network_resources(subnet_count: usize) -> serde_json::Map<String, Value> {
    let mut resources = serde_json::Map::new();
    resources.insert(
        VPC_ID.to_string(),
        json!({
            "Type": "AWS::EC2::VPC",
            "Properties": { "CidrBlock": "10.90.0.0/16" }
        }),
    );
    for index in 0..subnet_count {
        resources.insert(
            SUBNET_IDS[index].to_string(),
            json!({
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": VPC_ID },
                    "CidrBlock": format!("10.90.{}.0/24", index + 1),
                    "AvailabilityZone": ZONES[index]
                }
            }),
        );
    }
    resources
}

/// The subnet references a row's cluster declares.
fn subnet_refs(subnet_count: usize) -> Value {
    Value::Array(
        (0..subnet_count)
            .map(|index| json!({ "Ref": SUBNET_IDS[index] }))
            .collect(),
    )
}

/// One row of the cluster table: what the template declares, and what Amazon EKS must then
/// hold for it.
struct Row {
    case: &'static str,
    /// The cluster's `Name`, or `None` where the template declares none and CloudFormation
    /// generates it.
    declared_name: Option<&'static str>,
    /// The Kubernetes version the declaration asks for, if any.
    version: Option<&'static str>,
    /// How many subnets the VPC configuration names.
    subnet_count: usize,
    /// Whether the declaration states the two endpoint switches, and with what values.
    endpoint_access: Option<(bool, bool)>,
    /// The tags the declaration asks for.
    tags: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named cluster, two subnets, nothing else declared",
        declared_name: Some("stack-cluster-alpha"),
        version: None,
        subnet_count: 2,
        endpoint_access: None,
        tags: &[],
    },
    Row {
        case: "no name declared, so CloudFormation generates one",
        declared_name: None,
        version: Some("1.31"),
        subnet_count: 2,
        endpoint_access: None,
        tags: &[("team", "platform")],
    },
    Row {
        case: "three subnets, both endpoint switches and several tags",
        declared_name: Some("stack-cluster-gamma"),
        // A Kubernetes version Amazon EKS still offers a control plane for: a create on
        // a version whose extended support has ended is refused.
        version: Some("1.35"),
        subnet_count: 3,
        endpoint_access: Some((false, true)),
        tags: &[("team", "platform"), ("env", "test")],
    },
];

/// The template a cluster row deploys: the network, the cluster, and the stack outputs that
/// hand the cluster's identity and its endpoints on — every one of them an `Fn::GetAtt` of
/// a value only Amazon EKS can produce.
fn cluster_template(row: &Row) -> String {
    let mut properties = serde_json::Map::new();
    if let Some(name) = row.declared_name {
        properties.insert("Name".to_string(), json!(name));
    }
    properties.insert("RoleArn".to_string(), json!(role_arn("eksClusterRole")));
    if let Some(version) = row.version {
        properties.insert("Version".to_string(), json!(version));
    }
    let mut vpc_config = serde_json::Map::new();
    vpc_config.insert("SubnetIds".to_string(), subnet_refs(row.subnet_count));
    if let Some((public, private)) = row.endpoint_access {
        vpc_config.insert("EndpointPublicAccess".to_string(), json!(public));
        vpc_config.insert("EndpointPrivateAccess".to_string(), json!(private));
    }
    properties.insert(
        "ResourcesVpcConfig".to_string(),
        Value::Object(vpc_config),
    );
    if !row.tags.is_empty() {
        properties.insert(
            "Tags".to_string(),
            Value::Array(
                row.tags
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect(),
            ),
        );
    }
    let mut resources = network_resources(row.subnet_count);
    resources.insert(
        CLUSTER_ID.to_string(),
        json!({ "Type": "AWS::EKS::Cluster", "Properties": Value::Object(properties) }),
    );
    json!({
        "Resources": Value::Object(resources),
        "Outputs": {
            "ClusterName": { "Value": { "Ref": CLUSTER_ID } },
            "ClusterArn": { "Value": { "Fn::GetAtt": [CLUSTER_ID, "Arn"] } },
            "Endpoint": { "Value": { "Fn::GetAtt": [CLUSTER_ID, "Endpoint"] } },
            "CaData": {
                "Value": { "Fn::GetAtt": [CLUSTER_ID, "CertificateAuthorityData"] }
            },
            "OidcIssuer": {
                "Value": { "Fn::GetAtt": [CLUSTER_ID, "OpenIdConnectIssuerUrl"] }
            }
        }
    })
    .to_string()
}

/// Every row: the stack completes once Amazon EKS reports the cluster `ACTIVE`, the cluster
/// is independently readable through `DescribeCluster` and listed by `ListClusters` with the
/// configuration the template gave it, and every stack output carries the value that service
/// minted.
#[tokio::test]
async fn a_stack_provisions_the_eks_cluster_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("eks-cluster-{index}");
        create_stack(&router, case, &stack_name, &cluster_template(row)).await;
        let (status, body) =
            wait_for_stack_status(&router, case, &stack_name, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to complete, got {body}"
        );

        // Every output carries a value: the hand-off a client reads out of the stack.
        let outputs = stack_outputs(case, &body);
        assert_eq!(
            outputs.len(),
            5,
            "case {case:?}: expected every declared output reported, got {outputs:?}"
        );
        let cluster_name = output(case, &body, "ClusterName");
        if let Some(declared) = row.declared_name {
            assert_eq!(
                cluster_name, declared,
                "case {case:?}: expected Ref to report the declared name, got {body}"
            );
        } else {
            assert!(
                cluster_name.contains(&stack_name),
                "case {case:?}: expected a generated name naming its stack, got \
                 {cluster_name:?}"
            );
        }

        // The cluster is readable through the EKS API itself, independently of the stack.
        let read = describe_cluster(&router, &cluster_name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the declared cluster to be readable, got {} with body {}",
            read.status, read.text
        );
        let cluster = &read.body["cluster"];
        assert_eq!(
            cluster["status"],
            json!(CLUSTER_ACTIVE),
            "case {case:?}: expected the cluster {CLUSTER_ACTIVE} once the stack completed, \
             got {}",
            read.text
        );
        assert_eq!(
            cluster["roleArn"],
            json!(role_arn("eksClusterRole")),
            "case {case:?}: expected the declared role on the cluster, got {}",
            read.text
        );
        if let Some(version) = row.version {
            assert_eq!(
                cluster["version"],
                json!(version),
                "case {case:?}: expected the declared version, got {}",
                read.text
            );
        }
        let reported_subnets = cluster["resourcesVpcConfig"]["subnetIds"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            reported_subnets, row.subnet_count,
            "case {case:?}: expected the cluster placed in {} subnets, got {}",
            row.subnet_count, read.text
        );
        for subnet in cluster["resourcesVpcConfig"]["subnetIds"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let subnet = subnet.as_str().unwrap_or_default();
            assert!(
                subnet.starts_with("subnet-"),
                "case {case:?}: expected the cluster to name subnet ids EC2 assigned, got \
                 {subnet:?}"
            );
            let described = ec2(&router, "DescribeSubnets", &[("SubnetId.1", subnet)]).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: expected {subnet:?} to exist in EC2, got {} with body {}",
                described.status, described.text
            );
        }
        if let Some((public, private)) = row.endpoint_access {
            assert_eq!(
                cluster["resourcesVpcConfig"]["endpointPublicAccess"],
                json!(public),
                "case {case:?}: expected the declared public access switch, got {}",
                read.text
            );
            assert_eq!(
                cluster["resourcesVpcConfig"]["endpointPrivateAccess"],
                json!(private),
                "case {case:?}: expected the declared private access switch, got {}",
                read.text
            );
        }
        for (key, value) in row.tags {
            assert_eq!(
                cluster["tags"][*key],
                json!(value),
                "case {case:?}: expected the declared tag {key:?}={value:?} on the cluster, \
                 got {}",
                read.text
            );
        }
        assert_eq!(
            cluster["tags"]["aws:cloudformation:stack-name"],
            json!(stack_name),
            "case {case:?}: expected the propagated stack-name tag, got {}",
            read.text
        );

        // The cluster the stack named is one that service lists.
        assert!(
            list_clusters(&router).await.contains(&cluster_name),
            "case {case:?}: expected {cluster_name:?} among the listed clusters"
        );

        // Every attribute the stack reports is the value Amazon EKS answered with, never a
        // handle CloudFormation invented.
        assert_eq!(
            json!(output(case, &body, "ClusterArn")),
            cluster["arn"],
            "case {case:?}: expected the stack to report the cluster's own ARN, got {body}"
        );
        assert_eq!(
            json!(output(case, &body, "Endpoint")),
            cluster["endpoint"],
            "case {case:?}: expected the stack to report the cluster's own endpoint, got \
             {body}"
        );
        assert_eq!(
            json!(output(case, &body, "CaData")),
            cluster["certificateAuthority"]["data"],
            "case {case:?}: expected the stack to report the cluster's own CA data, got \
             {body}"
        );
        assert_eq!(
            json!(output(case, &body, "OidcIssuer")),
            cluster["identity"]["oidc"]["issuer"],
            "case {case:?}: expected the stack to report the cluster's own OIDC issuer, got \
             {body}"
        );
    }
}

/// One row of the association table: what the association's declaration states beyond the
/// cluster it is made on.
struct AssociationRow {
    case: &'static str,
    namespace: &'static str,
    service_account: &'static str,
    /// Whether the declaration states `DisableSessionTags`, and with what value.
    disable_session_tags: Option<bool>,
    /// The tags the declaration asks for.
    tags: &'static [(&'static str, &'static str)],
}

const ASSOCIATION_ROWS: &[AssociationRow] = &[
    AssociationRow {
        case: "the default namespace, nothing else declared",
        namespace: "default",
        service_account: "app",
        disable_session_tags: None,
        tags: &[],
    },
    AssociationRow {
        case: "another namespace, session tags disabled and a tag declared",
        namespace: "team-a",
        service_account: "reader",
        disable_session_tags: Some(true),
        tags: &[("team", "platform")],
    },
];

/// The template an association row deploys: the network, a cluster to associate on, and the
/// association — with the two ids Amazon EKS mints for it as the stack's outputs, which is
/// the hand-off an outputs-driven client reads.
fn association_template(row: &AssociationRow, cluster_name: &str) -> String {
    let mut properties = serde_json::Map::new();
    properties.insert("ClusterName".to_string(), json!(cluster_name));
    properties.insert("Namespace".to_string(), json!(row.namespace));
    properties.insert("ServiceAccount".to_string(), json!(row.service_account));
    properties.insert("RoleArn".to_string(), json!(role_arn("podRole")));
    if let Some(disabled) = row.disable_session_tags {
        properties.insert("DisableSessionTags".to_string(), json!(disabled));
    }
    if !row.tags.is_empty() {
        properties.insert(
            "Tags".to_string(),
            Value::Array(
                row.tags
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect(),
            ),
        );
    }
    let mut resources = serde_json::Map::new();
    resources.insert(
        ASSOCIATION_ID.to_string(),
        json!({
            "Type": "AWS::EKS::PodIdentityAssociation",
            "Properties": Value::Object(properties)
        }),
    );
    json!({
        "Resources": Value::Object(resources),
        "Outputs": {
            "AssociationArn": {
                "Value": { "Fn::GetAtt": [ASSOCIATION_ID, "AssociationArn"] }
            },
            "AssociationId": {
                "Value": { "Fn::GetAtt": [ASSOCIATION_ID, "AssociationId"] }
            }
        }
    })
    .to_string()
}

/// Every association row: the stack completes, the association it declared exists in Amazon
/// EKS with the configuration the template gave it, and both of the stack's outputs carry
/// the value that service minted — the pair a client reads the association back by.
#[tokio::test]
async fn a_stack_provisions_the_eks_pod_identity_association_its_template_declares() {
    let router = build_router();
    // One cluster for every row, created by a stack of its own so the association rows
    // exercise nothing but their own declaration.
    let host_row = &ROWS[0];
    create_stack(
        &router,
        "the host cluster",
        "pod-identity-host",
        &cluster_template(host_row),
    )
    .await;
    let (status, body) = wait_for_stack_status(
        &router,
        "the host cluster",
        "pod-identity-host",
        CREATE_COMPLETE,
    )
    .await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "setup: expected the host cluster's stack to complete, got {body}"
    );
    let cluster_name = output("the host cluster", &body, "ClusterName");

    for (index, row) in ASSOCIATION_ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("pod-identity-{index}");
        create_stack(
            &router,
            case,
            &stack_name,
            &association_template(row, &cluster_name),
        )
        .await;
        let (status, body) =
            wait_for_stack_status(&router, case, &stack_name, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to complete, got {body}"
        );

        // Both outputs carry a value, and both are what Amazon EKS minted.
        let association_arn = output(case, &body, "AssociationArn");
        let association_id = output(case, &body, "AssociationId");
        assert!(
            association_arn.starts_with("arn:"),
            "case {case:?}: expected an ARN output, got {association_arn:?}"
        );
        assert!(
            !association_id.is_empty(),
            "case {case:?}: expected an association id output, got {body}"
        );

        // The association is readable through the EKS API itself, by the pair that service
        // keys one by.
        let read = eks(
            &router,
            "GET",
            &format!(
                "/clusters/{}/pod-identity-associations/{}",
                encoded(&cluster_name),
                encoded(&association_id)
            ),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the declared association to be readable, got {} with \
             body {}",
            read.status, read.text
        );
        let association = &read.body["association"];
        assert_eq!(
            association["namespace"],
            json!(row.namespace),
            "case {case:?}: expected the declared namespace, got {}",
            read.text
        );
        assert_eq!(
            association["serviceAccount"],
            json!(row.service_account),
            "case {case:?}: expected the declared service account, got {}",
            read.text
        );
        assert_eq!(
            association["roleArn"],
            json!(role_arn("podRole")),
            "case {case:?}: expected the declared role, got {}",
            read.text
        );
        assert_eq!(
            association["associationArn"],
            json!(association_arn),
            "case {case:?}: expected the stack to report the association's own ARN, got {}",
            read.text
        );
        if let Some(disabled) = row.disable_session_tags {
            assert_eq!(
                association["disableSessionTags"],
                json!(disabled),
                "case {case:?}: expected the declared session-tag switch, got {}",
                read.text
            );
        }
        for (key, value) in row.tags {
            assert_eq!(
                association["tags"][*key],
                json!(value),
                "case {case:?}: expected the declared tag {key:?}={value:?}, got {}",
                read.text
            );
        }
    }
}

/// The rule's refusal half: a declaration this emulator cannot satisfy FAILS the resource
/// and the stack rather than completing it with a cluster configured differently from its
/// declaration — a property no arm maps onto `CreateCluster`, and a cluster Amazon EKS
/// itself refuses.
///
/// The rows differ in what cannot be satisfied: a nested configuration the emulator holds
/// no state for, a VPC configuration with too few subnets, and a role ARN that names no
/// role of this account.
#[tokio::test]
async fn a_stack_fails_when_the_declared_cluster_cannot_be_created() {
    let router = build_router();
    // (case, the cluster's extra properties, the subnets it names)
    let cases: [(&str, Value, usize); 3] = [
        (
            "a control-plane logging configuration the emulator holds no state for",
            json!({
                "Logging": {
                    "ClusterLogging": { "EnabledTypes": [{ "Type": "api" }] }
                }
            }),
            2,
        ),
        ("one subnet, where Amazon EKS requires two", json!({}), 1),
        (
            "a role ARN naming no role of this account",
            json!({ "RoleArn": "arn:aws:iam::999999999999:role/elsewhere" }),
            2,
        ),
    ];

    for (index, (case, extra, subnet_count)) in cases.into_iter().enumerate() {
        let stack_name = format!("eks-cluster-refused-{index}");
        let mut properties = serde_json::Map::new();
        properties.insert("Name".to_string(), json!(format!("refused-{index}")));
        properties.insert("RoleArn".to_string(), json!(role_arn("eksClusterRole")));
        properties.insert(
            "ResourcesVpcConfig".to_string(),
            json!({ "SubnetIds": subnet_refs(subnet_count) }),
        );
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            properties.insert(key, value);
        }
        let mut resources = network_resources(subnet_count);
        resources.insert(
            CLUSTER_ID.to_string(),
            json!({ "Type": "AWS::EKS::Cluster", "Properties": Value::Object(properties) }),
        );
        let template = json!({ "Resources": Value::Object(resources) }).to_string();
        create_stack(&router, case, &stack_name, &template).await;
        let (status, body) =
            wait_for_stack_status(&router, case, &stack_name, ROLLBACK_COMPLETE).await;
        assert_eq!(
            status, ROLLBACK_COMPLETE,
            "case {case:?}: expected the stack to fail rather than complete, got {body}"
        );
        assert!(
            element(&body, "StackStatusReason")
                .is_some_and(|reason| reason.contains(CLUSTER_ID)),
            "case {case:?}: expected the failure to name the cluster resource, got {body}"
        );

        // And nothing was left behind for a cluster the stack does not record.
        let read = describe_cluster(&router, &format!("refused-{index}")).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected no cluster for a failed resource, got {} with body {}",
            read.status, read.text
        );
    }
}
