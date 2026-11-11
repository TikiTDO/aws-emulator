// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Pipes::Pipe` really creates the pipe in
//! Amazon EventBridge Pipes, and the values its outputs carry are the ones that service
//! reports for the same pipe.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, and every output of the
//! stack carries a resolved value — `Ref` the resource's primary identifier, `Fn::GetAtt`
//! the value the owning service publishes for the attribute named — so a client reading the
//! stack's outputs holds the identifiers that service answers to. A declaration that does
//! not carry a property its type's schema requires is refused BEFORE any of that: the stack
//! rolls back and the owning service never hears of the resource.** Four crates have to
//! agree for either half to hold (CloudFormation makes the calls; EventBridge Pipes, IAM and
//! Amazon SQS serve them), so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the pipe
//! or leaves the name to CloudFormation, which state the pipe is asked to rest in
//! (`RUNNING`, the default, and `STOPPED`), whether the declaration carries `Description`,
//! `SourceParameters` and `Tags`, whether the template is JSON or YAML, which intrinsic each
//! output reads the resource with (`Ref`, `Fn::GetAtt` over a service-reported attribute,
//! `Fn::Sub`), and which Region the stack is deployed in. The refusal rows differ along
//! WHICH required property the declaration leaves out.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none, which
//! is CloudFormation's to compute: what is checked about it is that the pipe EventBridge
//! Pipes holds under that name is the stack's.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use super::query_wire::{
    block_of, elements, form_encode, members_of, opt_element, outputs_of, physical_ids_of,
};

/// How long a stack operation is given to reach a terminal status. The stack's own span and
/// the pipe's are each half a second, and the pipe's creation holds the stack open until
/// EventBridge Pipes reports it resting, so a settled stack takes a few of those spans.
const PATIENCE: Duration = Duration::from_secs(20);

/// How often the stack is read while it settles.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
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

/// `DescribePipe` over EventBridge Pipes' own `restJson1` wire, on the URI that operation's
/// `@http` binding declares (`GET /v1/pipes/{Name}`), signed for `region`.
async fn describe_pipe(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/v1/pipes/{name}"))
        .header("host", format!("pipes.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("pipes", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The stack as it reads once its operation has reached a TERMINAL status, polled rather
/// than slept for: the pipe's own creation holds the operation open, so how many spans a
/// stack takes is the deployment's to decide and not this suite's to assume.
async fn settled_stack(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    stack: &str,
) -> String {
    let began = Instant::now();
    loop {
        let (status, described) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks answered {status} {described}"
        );
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        let settled = !reported.ends_with("_IN_PROGRESS");
        if settled || began.elapsed() >= PATIENCE {
            return described;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The ARN a pipe of `name` carries in `region`: the same total function of the Region and
/// the name both services compute it from.
fn pipe_arn(region: &str, name: &str) -> String {
    format!("arn:aws:pipes:{region}:{ACCOUNT}:pipe/{name}")
}

/// The ARN a queue of `name` carries in `region`.
fn queue_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sqs:{region}:{ACCOUNT}:{name}")
}

/// The trust policy every row's execution role carries: EventBridge Pipes acts as the
/// pipe's `RoleArn`, so a role the stack creates has to admit that service principal or the
/// pipe's creation cannot be completed.
fn pipes_trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "pipes.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The identity policy that lets the pipe read its source and write its target, which is
/// what a pipe's execution role carries in practice.
fn pipes_identity_policy() -> Value {
    json!({
        "PolicyName": "move-records",
        "PolicyDocument": {
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": [
                    "sqs:ReceiveMessage",
                    "sqs:DeleteMessage",
                    "sqs:GetQueueAttributes",
                    "sqs:SendMessage",
                ],
                "Resource": "*",
            }],
        },
    })
}

/// The two queues and the role every row's template declares beside its pipe.
fn supporting_resources(source: &str, target: &str) -> Value {
    json!({
        "SourceQueue": { "Type": "AWS::SQS::Queue", "Properties": { "QueueName": source } },
        "TargetQueue": { "Type": "AWS::SQS::Queue", "Properties": { "QueueName": target } },
        "PipeRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": pipes_trust_policy(),
                "Policies": [pipes_identity_policy()],
            },
        },
    })
}

/// The outputs every row's template declares: the pipe read with `Ref`, with `Fn::GetAtt`
/// over each attribute EventBridge Pipes reports for it, and with `Fn::Sub` over the same
/// `Ref` — beside the ARNs of the resources the pipe connects.
fn pipe_outputs() -> Value {
    json!({
        "PipeRef": { "Value": { "Ref": "ThePipe" } },
        "PipeArn": { "Value": { "Fn::GetAtt": ["ThePipe", "Arn"] } },
        "PipeCurrentState": { "Value": { "Fn::GetAtt": ["ThePipe", "CurrentState"] } },
        "PipeSub": { "Value": { "Fn::Sub": ["${Name}", { "Name": { "Ref": "ThePipe" } }] } },
        "PipeRoleArn": { "Value": { "Fn::GetAtt": ["PipeRole", "Arn"] } },
        "SourceQueueArn": { "Value": { "Fn::GetAtt": ["SourceQueue", "Arn"] } },
        "TargetQueueArn": { "Value": { "Fn::GetAtt": ["TargetQueue", "Arn"] } },
    })
}

/// One JSON template: the two queues and the role beside the pipe `pipe` declares, and the
/// outputs every row reads the deployment through.
fn json_template(source: &str, target: &str, pipe: Value) -> String {
    let mut resources = supporting_resources(source, target);
    resources
        .as_object_mut()
        .expect("the resources are an object")
        .insert("ThePipe".to_string(), pipe);
    json!({ "Resources": resources, "Outputs": pipe_outputs() }).to_string()
}

/// One row of the provisioning table: a template, and what the rule says the pipe it
/// declares must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the pipe is created in and the one
    /// its ARN carries.
    region: &'static str,
    stack: &'static str,
    /// The queue names the row's two queues are declared under.
    source_queue: &'static str,
    target_queue: &'static str,
    /// The template body, built from the row's own names.
    template: fn(&Row) -> String,
    /// The pipe name the template declares, or `None` for a template that names none —
    /// whose pipe carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The `PipeState` the pipe must come to rest in, which is the state its `DesiredState`
    /// asked for.
    resting_state: &'static str,
    /// The `Description` the declaration carries, if any.
    description: Option<&'static str>,
    /// The tags the declaration asks for, which the pipe must carry beside the three
    /// CloudFormation records on every resource it provisions.
    declared_tags: &'static [(&'static str, &'static str)],
    /// Whether the row's outputs include the `Fn::Sub` over the pipe's `Ref`, which the
    /// JSON rows declare.
    reads_with_sub: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a named, described, tagged pipe in JSON, left to its default desired state",
        region: "us-east-1",
        stack: "pipes-default",
        source_queue: "pipes-default-source",
        target_queue: "pipes-default-target",
        template: |row| {
            json_template(
                row.source_queue,
                row.target_queue,
                json!({
                    "Type": "AWS::Pipes::Pipe",
                    "Properties": {
                        "Name": "pipes-default-pipe",
                        "Description": "orders to fulfilment",
                        "Source": { "Fn::GetAtt": ["SourceQueue", "Arn"] },
                        "Target": { "Fn::GetAtt": ["TargetQueue", "Arn"] },
                        "RoleArn": { "Fn::GetAtt": ["PipeRole", "Arn"] },
                        "Tags": { "Owner": "orders", "Tier": "prod" },
                    },
                }),
            )
        },
        declared_name: Some("pipes-default-pipe"),
        resting_state: "RUNNING",
        description: Some("orders to fulfilment"),
        declared_tags: &[("Owner", "orders"), ("Tier", "prod")],
        reads_with_sub: true,
    },
    Row {
        case: "a stopped pipe in YAML, untagged, with source parameters",
        region: "eu-west-1",
        stack: "pipes-stopped",
        source_queue: "pipes-stopped-source",
        target_queue: "pipes-stopped-target",
        template: |_| {
            [
                "Resources:",
                "  SourceQueue:",
                "    Type: AWS::SQS::Queue",
                "    Properties:",
                "      QueueName: pipes-stopped-source",
                "  TargetQueue:",
                "    Type: AWS::SQS::Queue",
                "    Properties:",
                "      QueueName: pipes-stopped-target",
                "  PipeRole:",
                "    Type: AWS::IAM::Role",
                "    Properties:",
                "      AssumeRolePolicyDocument:",
                "        Version: '2012-10-17'",
                "        Statement:",
                "          - Effect: Allow",
                "            Principal:",
                "              Service: pipes.amazonaws.com",
                "            Action: sts:AssumeRole",
                // The pipe is created only if its role may take the source's records, so
                // this role carries the same grant the JSON rows' role does
                // ([`pipes_identity_policy`]) \u2014 a pipe whose role cannot read its source
                // fails its creation and rolls the stack back, which is what the second
                // test below asserts.
                "      Policies:",
                "        - PolicyName: move-records",
                "          PolicyDocument:",
                "            Version: '2012-10-17'",
                "            Statement:",
                "              - Effect: Allow",
                "                Action:",
                "                  - sqs:ReceiveMessage",
                "                  - sqs:DeleteMessage",
                "                  - sqs:GetQueueAttributes",
                "                  - sqs:SendMessage",
                "                Resource: '*'",
                "  ThePipe:",
                "    Type: AWS::Pipes::Pipe",
                "    Properties:",
                "      Name: pipes-stopped-pipe",
                "      DesiredState: STOPPED",
                "      Source: !GetAtt SourceQueue.Arn",
                "      Target: !GetAtt TargetQueue.Arn",
                "      RoleArn: !GetAtt PipeRole.Arn",
                "      SourceParameters:",
                "        SqsQueueParameters:",
                "          BatchSize: 5",
                "Outputs:",
                "  PipeRef:",
                "    Value: !Ref ThePipe",
                "  PipeArn:",
                "    Value: !GetAtt ThePipe.Arn",
                "  PipeCurrentState:",
                "    Value: !GetAtt ThePipe.CurrentState",
                "  PipeRoleArn:",
                "    Value: !GetAtt PipeRole.Arn",
                "  SourceQueueArn:",
                "    Value: !GetAtt SourceQueue.Arn",
                "  TargetQueueArn:",
                "    Value: !GetAtt TargetQueue.Arn",
                "",
            ]
            .join("\n")
        },
        declared_name: Some("pipes-stopped-pipe"),
        resting_state: "STOPPED",
        description: None,
        declared_tags: &[],
        reads_with_sub: false,
    },
    Row {
        case: "a pipe the template does not name, so CloudFormation generates one",
        region: "us-west-2",
        stack: "pipes-generated",
        source_queue: "pipes-generated-source",
        target_queue: "pipes-generated-target",
        template: |row| {
            json_template(
                row.source_queue,
                row.target_queue,
                json!({
                    "Type": "AWS::Pipes::Pipe",
                    "Properties": {
                        "DesiredState": "RUNNING",
                        "Source": { "Fn::GetAtt": ["SourceQueue", "Arn"] },
                        "Target": { "Fn::GetAtt": ["TargetQueue", "Arn"] },
                        "RoleArn": { "Fn::GetAtt": ["PipeRole", "Arn"] },
                    },
                }),
            )
        },
        declared_name: None,
        resting_state: "RUNNING",
        description: None,
        declared_tags: &[],
        reads_with_sub: true,
    },
];

/// Every row: the stack reaches `CREATE_COMPLETE`, every output it declares carries a
/// resolved value, the pipe's own outputs are the values `pipes DescribePipe` reports for
/// the pipe the template declared, and deleting the stack deletes it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stack_provisions_the_pipe_its_template_declares() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let region = row.region;
        let template = (row.template)(row);

        let (status, created) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
        );

        let described = settled_stack(&router, region, case, row.stack).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        // The pipe's name: the one the template asked for, or — for the row that named
        // nothing — the physical id CloudFormation generated, which is what `Ref` returns.
        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_ids = physical_ids_of(&resources);
        let name = match row.declared_name {
            Some(declared) => declared.to_string(),
            None => physical_ids
                .get("ThePipe")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no pipe physical id in {resources}")),
        };
        assert_eq!(
            physical_ids.get("ThePipe").map(String::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the pipe's physical id to be its name {name:?}, got \
             {physical_ids:?}"
        );
        assert!(
            (1..=64).contains(&name.chars().count())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)),
            "case {case:?}: expected a name inside the modelled PipeName shape, got {name:?}"
        );

        // EVERY output the template declares carries a value: `outputs_of` pairs a key with
        // a value within one member, so an entry reported without an `OutputValue`
        // contributes nothing and the key set below is short by it.
        let outputs = outputs_of(&described);
        let arn = pipe_arn(region, &name);
        let mut expected: Vec<(&str, String)> = vec![
            ("PipeRef", name.clone()),
            ("PipeArn", arn.clone()),
            ("PipeCurrentState", row.resting_state.to_string()),
            (
                "SourceQueueArn",
                queue_arn(region, row.source_queue),
            ),
            (
                "TargetQueueArn",
                queue_arn(region, row.target_queue),
            ),
        ];
        if row.reads_with_sub {
            expected.push(("PipeSub", name.clone()));
        }
        for (key, value) in &expected {
            assert_eq!(
                outputs.get(*key),
                Some(value),
                "case {case:?}: expected output {key:?} to be {value:?}, got {:?} (every \
                 output: {outputs:?})",
                outputs.get(*key)
            );
        }
        let role_arn = outputs
            .get("PipeRoleArn")
            .unwrap_or_else(|| panic!("case {case:?}: no PipeRoleArn output in {outputs:?}"));
        assert!(
            role_arn.starts_with(&format!("arn:aws:iam::{ACCOUNT}:role/")),
            "case {case:?}: expected the role output to be an IAM role ARN, got {role_arn:?}"
        );

        // ...and EventBridge Pipes itself reports exactly those values for the pipe.
        let (status, pipe) = describe_pipe(&router, region, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribePipe on the name the stack \
             published, got {status} with body {pipe}"
        );
        let reported: Vec<(&str, Option<&str>)> = vec![
            ("Arn", pipe.get("Arn").and_then(Value::as_str)),
            ("Name", pipe.get("Name").and_then(Value::as_str)),
            (
                "CurrentState",
                pipe.get("CurrentState").and_then(Value::as_str),
            ),
            ("Source", pipe.get("Source").and_then(Value::as_str)),
            ("Target", pipe.get("Target").and_then(Value::as_str)),
            ("RoleArn", pipe.get("RoleArn").and_then(Value::as_str)),
        ];
        let source = queue_arn(region, row.source_queue);
        let target = queue_arn(region, row.target_queue);
        let expected_report: Vec<(&str, Option<&str>)> = vec![
            ("Arn", Some(arn.as_str())),
            ("Name", Some(name.as_str())),
            ("CurrentState", Some(row.resting_state)),
            ("Source", Some(source.as_str())),
            ("Target", Some(target.as_str())),
            ("RoleArn", Some(role_arn.as_str())),
        ];
        assert_eq!(
            reported, expected_report,
            "case {case:?}: expected the pipe the stack's outputs name, got {pipe}"
        );
        assert_eq!(
            pipe.get("Description").and_then(Value::as_str),
            row.description,
            "case {case:?}: expected the declared description, got {pipe}"
        );

        // The tags the declaration asked for, beside the three CloudFormation records on
        // every resource it provisions: that `aws:` namespace is one this API refuses a
        // CALLER, and the create a stack makes is not a caller's request, so the three
        // travel with it.
        let tags: BTreeMap<String, String> = pipe
            .get("Tags")
            .and_then(Value::as_object)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let stack_id = opt_element(&described, "StackId")
            .unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {described}"));
        let expected_tags: BTreeMap<String, String> = row
            .declared_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .chain([
                (
                    "aws:cloudformation:stack-name".to_string(),
                    row.stack.to_string(),
                ),
                ("aws:cloudformation:stack-id".to_string(), stack_id),
                (
                    "aws:cloudformation:logical-id".to_string(),
                    "ThePipe".to_string(),
                ),
            ])
            .collect();
        assert_eq!(
            tags, expected_tags,
            "case {case:?}: expected the declared tags beside the three CloudFormation \
             records, got {tags:?}"
        );

        // Deleting the stack deletes the pipe EventBridge Pipes holds for it.
        let (status, deleted) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
        );
        // Polled, not asserted at once: a stack deletion is an operation with its own span,
        // so when the pipe stops being there is the deployment's to decide.
        let (status, gone) = pipe_once_gone(&router, region, case, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from DescribePipe once the stack deleted the \
             pipe, got {status} with body {gone}"
        );
    }
}

/// `DescribePipe` once EventBridge Pipes no longer holds the pipe, or whatever it answers
/// after [`PATIENCE`].
async fn pipe_once_gone(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    name: &str,
) -> (u16, Value) {
    let began = Instant::now();
    loop {
        let (status, body) = describe_pipe(router, region, name).await;
        if status == 404 || began.elapsed() >= PATIENCE {
            return (status, body);
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribePipe to answer the pipe or a 404 while the \
             stack's deletion runs, got {status} with body {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One refusal row: which required property the declaration leaves out.
struct Refused {
    case: &'static str,
    stack: &'static str,
    pipe: &'static str,
    /// The property the declaration omits, which the validation must name.
    missing: &'static str,
}

const REFUSED: &[Refused] = &[
    Refused {
        case: "a pipe declared with no target",
        stack: "pipes-no-target",
        pipe: "pipes-no-target-pipe",
        missing: "Target",
    },
    Refused {
        case: "a pipe declared with no role",
        stack: "pipes-no-role",
        pipe: "pipes-no-role-pipe",
        missing: "RoleArn",
    },
];

/// A stack whose `AWS::Pipes::Pipe` omits a property the type's schema requires is refused
/// before anything is provisioned: the stack ends `ROLLBACK_COMPLETE` carrying the
/// `PROPERTY_VALIDATION` result for that resource, and EventBridge Pipes does not hold the
/// pipe the template named.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pipe_missing_a_required_property_rolls_the_stack_back() {
    let router = build_router();
    let region = "us-east-2";

    for row in REFUSED {
        let case = row.case;
        let mut properties = json!({
            "Name": row.pipe,
            "Source": { "Fn::GetAtt": ["SourceQueue", "Arn"] },
            "Target": { "Fn::GetAtt": ["TargetQueue", "Arn"] },
            "RoleArn": { "Fn::GetAtt": ["PipeRole", "Arn"] },
        });
        properties
            .as_object_mut()
            .expect("the declaration is an object")
            .remove(row.missing);
        let mut resources = supporting_resources(
            &format!("{}-source", row.stack),
            &format!("{}-target", row.stack),
        );
        resources
            .as_object_mut()
            .expect("the resources are an object")
            .insert(
                "ThePipe".to_string(),
                json!({ "Type": "AWS::Pipes::Pipe", "Properties": properties }),
            );
        let template = json!({ "Resources": resources, "Outputs": pipe_outputs() }).to_string();
        let (status, created) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack — the request is valid and the \
             stack exists — got {status} with body {created}"
        );

        let described = settled_stack(&router, region, case, row.stack).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the stack to roll back, got {described}"
        );

        // The detail of the refusal: the validation that failed, on the resource that failed
        // it, naming the missing property at the resource's own properties path.
        let (status, events) = cloudformation(
            &router,
            region,
            "DescribeEvents",
            &[("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeEvents, got {status} with body \
             {events}"
        );
        let validations: Vec<Vec<String>> = members_of(&events)
            .into_iter()
            .filter(|event| block_of(event, "ValidationName").is_some())
            .map(|event| {
                [
                    "LogicalResourceId",
                    "ValidationName",
                    "ValidationStatus",
                    "ValidationFailureMode",
                    "ValidationStatusReason",
                    "ValidationPath",
                ]
                .iter()
                .map(|tag| elements(event, tag).into_iter().next().unwrap_or_default())
                .collect()
            })
            .collect();
        let expected: Vec<Vec<String>> = vec![vec![
            "ThePipe".to_string(),
            "PROPERTY_VALIDATION".to_string(),
            "FAILED".to_string(),
            "FAIL".to_string(),
            format!("Required property [{}] not found", row.missing),
            "/Resources/ThePipe/Properties".to_string(),
        ]];
        assert_eq!(
            validations, expected,
            "case {case:?}: expected the validation result {expected:?}, got {validations:?}"
        );

        // ...and nothing of the kind exists in the owning service: the pipe the template
        // named was never asked for.
        let (status, gone) = describe_pipe(&router, region, row.pipe).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from DescribePipe for a pipe the refused stack \
             never created, got {status} with body {gone}"
        );
        assert_eq!(
            gone.get("message").and_then(Value::as_str),
            Some(format!("Pipe {} does not exist.", row.pipe).as_str()),
            "case {case:?}: expected the owning service's own not-found wording, got {gone}"
        );
    }
}
