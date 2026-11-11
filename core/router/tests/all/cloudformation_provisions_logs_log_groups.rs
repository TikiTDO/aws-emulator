// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::Logs::LogGroup` its template declares — in Amazon CloudWatch Logs, with the
//! retention the template asked for.
//!
//! The rule under test: **a stack creates every resource its `Resources` section declares
//! through the service that OWNS the type, so after the create completes that service's own
//! read answers for it with the configuration the template declared and the tags
//! CloudFormation gives it, a later deployment that changes either is applied through the
//! same service, and deleting the stack deletes it there.** For this type the owning calls
//! are CloudWatch Logs' `CreateLogGroup` and — because a group's retention is a setting of
//! its own, not a member of the create — `PutRetentionPolicy` and `DeleteRetentionPolicy`,
//! and the read is `DescribeLogGroups`. Two crates have to agree for that to hold
//! (CloudFormation makes the calls, CloudWatch Logs serves them), so the suite lives here: a
//! crate-local test can only ever see one side of the call.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the log
//! group or leaves the name to CloudFormation, which name form it states (a path-like name, a
//! Lambda function's own `/aws/lambda/<function>` group, a name with the dot, hyphen,
//! underscore and `#` characters the member admits), which retention it declares (each
//! published bound and values in between, and none at all, which is "always retained and
//! never expire"), which class the group is created in, and how many tags the stack and the
//! declaration carry. Every expected value is computed here from what the row declares, never
//! read back out of the answer being checked.
//!
//! The last suite adds the other half of the same rule over a deployment: a changed
//! retention reaches the group the stack already has, and a declaration that drops the
//! retention takes the group's policy away rather than leaving the old one in force.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use crate::query_wire::{elements, form_encode, opt_element};
// Polling a stack until it first reports a status, and reading the status it reports now:
// the helpers this crate publishes once for every suite that deploys a stack whose
// resources their owning services report as still coming up
// (`tests/all/cloudformation_firehose_stream_stabilisation.rs`).
use crate::cloudformation_firehose_stream_stabilisation::{stack_status, wait_for_stack_status};

/// The Region every request here is signed for — the Region `wait_for_stack_status` polls
/// in — and so the Region the stack's resources belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// CloudWatch Logs' `X-Amz-Target` prefix, from its model's service shape
/// (`Logs_20140328`), and the signing name its credential scope carries
/// (`sigv4Name: logs`).
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const LOGS_SIGNING_NAME: &str = "logs";

/// The longest a log group's name may be, and the characters one may carry: `logGroupName`
/// is `@length(min: 1, max: 512)` with pattern `^[\.\-_/#A-Za-z0-9]+$`.
const MAX_LOG_GROUP_NAME_LEN: usize = 512;

/// The retentions a log group can be set to, from the model's `Days` shape: "Possible values
/// are: 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192,
/// 2557, 2922, 3288, and 3653."
const RETENTION_VALUES: [i64; 22] = [
    1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922,
    3288, 3653,
];

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
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
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudWatch Logs operation over its awsJson1_1 wire.
async fn logs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{LOGS_SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{LOGS_TARGET_PREFIX}.{operation}"),
        )
        .header("authorization", authorization(LOGS_SIGNING_NAME))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// How often, and how many times, a stack is asked for its status. A stack completes once
/// the services that own its resources report them ready, which no fixed sleep can name.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The log group CloudWatch Logs holds under `name`, as `DescribeLogGroups` reports it, or
/// `None` when it holds none.
async fn described_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Option<Value> {
    let (status, parsed) = logs(
        router,
        "DescribeLogGroups",
        &json!({"logGroupNamePrefix": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeLogGroups for {name:?}, got {status} \
         with body {parsed}"
    );
    parsed
        .get("logGroups")
        .and_then(Value::as_array)
        .and_then(|groups| {
            groups
                .iter()
                .find(|group| group.get("logGroupName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
}

/// The tags CloudWatch Logs holds for the resource `arn` names.
async fn resource_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, parsed) = logs(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource for {arn:?}, got {status} \
         with body {parsed}"
    );
    parsed
        .get("tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| {
                    Some((key.clone(), value.as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ARN a log group of `name` carries, as the resource reference publishes it: "such as
/// arn:aws:logs:us-east-1:123456789012:log-group:/mystack-testgroup-12ABC1AB12A1:*".
fn log_group_attribute_arn(name: &str) -> String {
    format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{name}:*")
}

/// The ARN a tagging call addresses a log group by: "The ARN format of a log group is
/// arn:aws:logs:Region:account-id:log-group:log-group-name".
fn log_group_tagging_arn(name: &str) -> String {
    format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{name}")
}

/// Whether a log group of `name` can be named in the ARN a tagging call takes: every
/// character of `TagResource$resourceArn`'s own pattern, `^[\w+=/:,.@-]*$`.
fn addressable_by_tagging_arn(name: &str) -> bool {
    name.chars().all(|character| {
        character.is_alphanumeric()
            || ['_', '+', '=', '/', ':', ',', '.', '@', '-'].contains(&character)
    })
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The tags a resource of this stack must end up carrying: the ones its own declaration asks
/// for, then the stack-level ones under every key that declaration does not use.
///
/// The three `aws:cloudformation:*` tags CloudFormation records on a resource are NOT among
/// them: CloudWatch Logs refuses a tag key in the namespace Amazon Web Services reserves for
/// its own tags ("Tags starting with 'AWS:' are reserved for AWS use."), so a provider leaves
/// them off rather than sending them to be refused
/// (`state::provisioning::is_reserved_tag_key`).
fn expected_tags(
    stack_tags: &[(&str, &str)],
    declared_tags: &[(&str, &str)],
) -> BTreeMap<String, String> {
    stack_tags
        .iter()
        .chain(declared_tags)
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The stack-level `Tags` of a stack request, as the awsQuery form members that carry them.
fn tag_members(stack_tags: &[(&str, &str)]) -> Vec<(String, String)> {
    stack_tags
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let ordinal = index + 1;
            [
                (format!("Tags.member.{ordinal}.Key"), (*key).to_string()),
                (format!("Tags.member.{ordinal}.Value"), (*value).to_string()),
            ]
        })
        .collect()
}

/// One stack request's members: the stack name, the template and the stack's tags.
fn stack_request<'a>(
    stack_name: &'a str,
    template: &'a str,
    tags: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    let mut members: Vec<(&str, &str)> = vec![("StackName", stack_name), ("TemplateBody", template)];
    for (key, value) in tags {
        members.push((key.as_str(), value.as_str()));
    }
    members
}

/// A template declaring ONE log group under `logical_id`, with outputs resolving that
/// resource's `Ref` — "Ref returns the log group name" — and its `Arn` attribute.
fn template(logical_id: &str, properties: &Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "one CloudWatch Logs log group",
        "Resources": {
            logical_id: {
                "Type": "AWS::Logs::LogGroup",
                "Properties": properties,
            },
        },
        "Outputs": {
            "GroupName": {"Value": {"Ref": logical_id}},
            "GroupArn": {"Value": {"Fn::GetAtt": [logical_id, "Arn"]}},
        },
    })
    .to_string()
}

/// Poll until the stack's deletion has finished: a deleted stack reports `DELETE_COMPLETE`
/// and is then no longer addressable by its NAME, so either answer ends the wait. `false` is
/// a stack still being deleted when the budget ran out, with the last answer read.
///
/// The shared `wait_for_stack_status` cannot serve this one: it requires every read to
/// answer 200, and a stack whose deletion finished no longer answers at all under its name.
///
/// `pub(crate)` because more than one suite deletes a stack and then asks what its owning
/// services still hold: the wait is written once, beside the `wait_for_stack_status` it
/// complements, so two suites cannot disagree about when a deletion has finished.
pub(crate) async fn wait_for_stack_gone(
    router: &MultiServiceRouter,
    stack_name: &str,
) -> (bool, String) {
    let mut last = String::new();
    for _ in 0..POLLS {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        last = body;
        if status != 200 || opt_element(&last, "StackStatus").as_deref() == Some(DELETE_COMPLETE) {
            return (true, last);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    (false, last)
}

/// Create a stack, poll until it reports `CREATE_COMPLETE`, and answer the `DescribeStacks`
/// body that status was read from.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    stack_tags: &[(&str, &str)],
) -> String {
    let tags = tag_members(stack_tags);
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &stack_request(stack_name, template, &tags),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let (reported, described) = wait_for_stack_status(router, stack_name, CREATE_COMPLETE).await;
    assert_eq!(
        reported, CREATE_COMPLETE,
        "case {case:?}: expected the stack to complete; CreateStack answered {body} and the \
         stack last read as {described}"
    );
    described
}

/// One row of the table: a log-group declaration, and what the rule says CloudWatch Logs must
/// then hold.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    logical_id: &'static str,
    /// The `Properties` the template declares for the log group.
    properties: fn() -> Value,
    /// The name the template declares, or `None` where CloudFormation generates it.
    declared_name: Option<String>,
    /// The retention `DescribeLogGroups` must report, or `None` for a declaration that states
    /// none — whose group's events "are always retained and never expire".
    expected_retention: Option<i64>,
    /// The class `DescribeLogGroups` must report: the declared one, or the documented default
    /// for a declaration that states none.
    expected_class: &'static str,
    /// The stack-level tags the stack operations carry.
    stack_tags: &'static [(&'static str, &'static str)],
    /// The tags the group's OWN declaration asks for.
    declared_tags: &'static [(&'static str, &'static str)],
}

/// Every form of log-group declaration round-trips: the stack creates it in CloudWatch Logs
/// under the name its `Ref` resolves to, carrying the retention, the class and the tags the
/// template gives it, and deleting the stack deletes it there.
#[tokio::test]
async fn every_declared_log_group_is_created_in_cloudwatch_logs_with_its_retention() {
    let longest_name = format!("/longest/{}", "g".repeat(MAX_LOG_GROUP_NAME_LEN - 9));
    let rows: Vec<Row> = vec![
        Row {
            case: "a named group at the lowest published retention",
            stack_name: "logs-lowest-retention",
            logical_id: "AppLogs",
            properties: || {
                json!({
                    "LogGroupName": "/app/lowest",
                    "RetentionInDays": RETENTION_VALUES[0],
                })
            },
            declared_name: Some("/app/lowest".to_string()),
            expected_retention: Some(RETENTION_VALUES[0]),
            expected_class: "STANDARD",
            stack_tags: &[("team", "platform")],
            declared_tags: &[],
        },
        Row {
            case: "a Lambda function's own log group with a week's retention",
            stack_name: "logs-lambda-group",
            logical_id: "FunctionLogs",
            properties: || {
                json!({
                    "LogGroupName": "/aws/lambda/orders-handler",
                    "RetentionInDays": 7,
                })
            },
            declared_name: Some("/aws/lambda/orders-handler".to_string()),
            expected_retention: Some(7),
            expected_class: "STANDARD",
            stack_tags: &[("team", "orders"), ("env", "staging")],
            declared_tags: &[],
        },
        Row {
            case: "a name using every documented character class, at a mid retention",
            stack_name: "logs-every-character",
            logical_id: "Mixed_Logs42",
            properties: || {
                json!({
                    "LogGroupName": "/svc.one-two_three/#4",
                    "RetentionInDays": 90,
                })
            },
            declared_name: Some("/svc.one-two_three/#4".to_string()),
            expected_retention: Some(90),
            expected_class: "STANDARD",
            stack_tags: &[],
            declared_tags: &[],
        },
        Row {
            case: "a group at the highest published retention",
            stack_name: "logs-highest-retention",
            logical_id: "ArchiveLogs",
            properties: || {
                json!({
                    "LogGroupName": "/app/highest",
                    "RetentionInDays": RETENTION_VALUES[RETENTION_VALUES.len() - 1],
                })
            },
            declared_name: Some("/app/highest".to_string()),
            expected_retention: Some(RETENTION_VALUES[RETENTION_VALUES.len() - 1]),
            expected_class: "STANDARD",
            stack_tags: &[("team", "platform")],
            declared_tags: &[],
        },
        Row {
            case: "a group declaring no retention, whose events never expire",
            stack_name: "logs-no-retention",
            logical_id: "ForeverLogs",
            properties: || json!({"LogGroupName": "/app/forever"}),
            declared_name: Some("/app/forever".to_string()),
            expected_retention: None,
            expected_class: "STANDARD",
            stack_tags: &[("team", "platform")],
            declared_tags: &[],
        },
        Row {
            case: "a group in the infrequent-access class with a retention",
            stack_name: "logs-infrequent-access",
            logical_id: "CheapLogs",
            properties: || {
                json!({
                    "LogGroupName": "/app/infrequent",
                    "LogGroupClass": "INFREQUENT_ACCESS",
                    "RetentionInDays": 30,
                })
            },
            declared_name: Some("/app/infrequent".to_string()),
            expected_retention: Some(30),
            expected_class: "INFREQUENT_ACCESS",
            stack_tags: &[("team", "platform")],
            declared_tags: &[],
        },
        Row {
            case: "a declaration carrying tags of its own, one of them the stack's key",
            stack_name: "logs-declared-tags",
            logical_id: "TaggedLogs",
            properties: || {
                json!({
                    "LogGroupName": "/app/tagged",
                    "RetentionInDays": 14,
                    "Tags": [
                        {"Key": "team", "Value": "payments"},
                        {"Key": "tier", "Value": "gold"},
                    ],
                })
            },
            declared_name: Some("/app/tagged".to_string()),
            expected_retention: Some(14),
            expected_class: "STANDARD",
            stack_tags: &[("team", "platform"), ("env", "prod")],
            declared_tags: &[("team", "payments"), ("tier", "gold")],
        },
        Row {
            case: "a 512-character name, the documented maximum",
            stack_name: "logs-longest-name",
            logical_id: "LongestLogs",
            properties: move || {
                json!({
                    "LogGroupName": format!("/longest/{}", "g".repeat(MAX_LOG_GROUP_NAME_LEN - 9)),
                    "RetentionInDays": 5,
                })
            },
            declared_name: Some(longest_name),
            expected_retention: Some(5),
            expected_class: "STANDARD",
            stack_tags: &[],
            declared_tags: &[],
        },
        Row {
            case: "a group CloudFormation names, the template declaring none",
            stack_name: "logs-generated-name",
            logical_id: "Unnamed",
            properties: || json!({"RetentionInDays": 180}),
            declared_name: None,
            expected_retention: Some(180),
            expected_class: "STANDARD",
            stack_tags: &[("team", "platform")],
            declared_tags: &[],
        },
    ];

    for row in &rows {
        let router = build_router();
        let case = row.case;
        let properties = (row.properties)();
        let body = create_and_settle(
            &router,
            case,
            row.stack_name,
            &template(row.logical_id, &properties),
            row.stack_tags,
        )
        .await;

        // `Ref` for this type is the group's name, so the output is the handle a client reads
        // the deployment through — and the name CloudWatch Logs must answer for.
        let outputs = outputs_of(&body);
        let reported_name = outputs.get("GroupName").cloned().unwrap_or_default();
        match &row.declared_name {
            Some(declared) => assert_eq!(
                &reported_name, declared,
                "case {case:?}: expected Ref to resolve to the declared name, got \
                 {reported_name:?}"
            ),
            None => assert!(
                reported_name.contains(row.logical_id)
                    && reported_name.chars().count() <= MAX_LOG_GROUP_NAME_LEN
                    && reported_name.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || ['.', '-', '_', '/', '#'].contains(&character)
                    }),
                "case {case:?}: expected a generated name naming the resource and inside \
                 CloudWatch Logs' own rule, got {reported_name:?}"
            ),
        }
        assert_eq!(
            outputs.get("GroupArn").cloned(),
            Some(log_group_attribute_arn(&reported_name)),
            "case {case:?}: expected the type's published ARN form, got {outputs:?}"
        );

        let group = described_group(&router, case, &reported_name)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected CloudWatch Logs to hold {reported_name:?}")
            });
        assert_eq!(
            group.get("retentionInDays").and_then(Value::as_i64),
            row.expected_retention,
            "case {case:?}: expected the declared retention, got {group}"
        );
        assert_eq!(
            group.get("logGroupClass").and_then(Value::as_str),
            Some(row.expected_class),
            "case {case:?}: expected the declared class, got {group}"
        );
        assert_eq!(
            group.get("arn").and_then(Value::as_str),
            Some(log_group_attribute_arn(&reported_name).as_str()),
            "case {case:?}: expected the group's own ARN to agree with the stack's \
             attribute, got {group}"
        );

        // A tagging call addresses the group by an ARN, whose member admits fewer characters
        // than a log group's name does (`TagResource$resourceArn`: `^[\w+=/:,.@-]*$`, against
        // `logGroupName`'s `^[\.\-_/#A-Za-z0-9]+$`), so a group whose name carries one of the
        // others is a group no tagging call can read the tags of.
        if addressable_by_tagging_arn(&reported_name) {
            let tags = resource_tags(&router, case, &log_group_tagging_arn(&reported_name)).await;
            assert_eq!(
                tags,
                expected_tags(row.stack_tags, row.declared_tags),
                "case {case:?}: expected the group to carry the tags CloudFormation gives it, \
                 got {tags:?}"
            );
        }

        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}"
        );
        let (reported, after) = wait_for_stack_gone(&router, row.stack_name).await;
        assert!(
            reported,
            "case {case:?}: expected the stack's deletion to complete, got {after}"
        );
        assert_eq!(
            described_group(&router, case, &reported_name).await,
            None,
            "case {case:?}: expected the stack's deletion to delete the group in CloudWatch \
             Logs"
        );
    }
}

/// One row of the update table: the case and the retention this deployment declares.
struct Change {
    case: &'static str,
    retention: Option<i64>,
}

/// A deployment that declares a DIFFERENT retention for a log group the stack already has
/// reaches CloudWatch Logs as that change: the group keeps its name, its retention is
/// replaced, and a declaration that drops the retention takes the policy away rather than
/// leaving the old one in force.
#[tokio::test]
async fn a_changed_log_group_declaration_reaches_cloudwatch_logs() {
    let router = build_router();
    let stack_name = "logs-updates";
    let logical_id = "AppLogs";
    let group_name = "/app/updates";
    let declaration = |retention: Option<i64>, declared_tags: &[(&str, &str)]| {
        let mut properties = json!({"LogGroupName": group_name});
        if let Some(retention) = retention {
            properties["RetentionInDays"] = json!(retention);
        }
        if !declared_tags.is_empty() {
            properties["Tags"] = Value::Array(
                declared_tags
                    .iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            );
        }
        template(logical_id, &properties)
    };

    let created = create_and_settle(
        &router,
        "the first deployment",
        stack_name,
        &declaration(Some(7), &[("tier", "gold")]),
        &[("team", "platform")],
    )
    .await;
    assert!(
        !created.is_empty(),
        "the first deployment: expected a DescribeStacks answer to read the stack from"
    );
    // What the create gave the group: the tags the stack propagates and the ones the
    // declaration states, outside the namespace CloudWatch Logs reserves.
    let tags = resource_tags(&router, "the first deployment", &log_group_tagging_arn(group_name))
        .await;
    assert_eq!(
        tags,
        expected_tags(&[("team", "platform")], &[("tier", "gold")]),
        "the first deployment: expected the group to carry the tags CloudFormation gives it, \
         got {tags:?}"
    );

    // Each row is a deployment: what retention it declares, and what the group must then
    // report. The rows differ along how the retention changes — raised, lowered, to each
    // published bound, dropped altogether, and set again after being dropped. The
    // declaration's own tags are the create's throughout, since this table's subject is the
    // retention.
    let changes: &[Change] = &[
        Change {
            case: "the retention raised",
            retention: Some(30),
        },
        Change {
            case: "the retention lowered",
            retention: Some(3),
        },
        Change {
            case: "the retention at the highest published value",
            retention: Some(RETENTION_VALUES[RETENTION_VALUES.len() - 1]),
        },
        Change {
            case: "the retention at the lowest published value",
            retention: Some(RETENTION_VALUES[0]),
        },
        Change {
            case: "the retention dropped",
            retention: None,
        },
        Change {
            case: "a retention declared again after being dropped",
            retention: Some(14),
        },
    ];

    for Change { case, retention } in changes {
        let tags = tag_members(&[("team", "platform")]);
        let template = declaration(*retention, &[("tier", "gold")]);
        let (reported, after) = cloudformation(
            &router,
            "UpdateStack",
            &stack_request(stack_name, &template, &tags),
        )
        .await;
        assert_eq!(
            reported, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {reported} with {after}"
        );
        // The stack is IN PROGRESS before it completes, so the wait below cannot be satisfied
        // by the status the PREVIOUS row left behind.
        let (in_flight, described) = stack_status(&router, stack_name).await;
        assert_eq!(
            in_flight, UPDATE_IN_PROGRESS,
            "case {case:?}: expected the update to be in progress once UpdateStack answered, \
             got {in_flight} with {described}"
        );
        let (settled, described) = wait_for_stack_status(&router, stack_name, UPDATE_COMPLETE).await;
        assert_eq!(
            settled, UPDATE_COMPLETE,
            "case {case:?}: expected the update to complete; UpdateStack answered {after} and \
             the stack last read as {described}"
        );

        let group = described_group(&router, case, group_name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the group survives the update"));
        assert_eq!(
            group.get("retentionInDays").and_then(Value::as_i64),
            *retention,
            "case {case:?}: expected the retention this deployment declares, got {group}"
        );
        // The tags the create gave the group are still the ones it carries: a deployment
        // that changes only the retention leaves them alone.
        let tags = resource_tags(&router, case, &log_group_tagging_arn(group_name)).await;
        assert_eq!(
            tags,
            expected_tags(&[("team", "platform")], &[("tier", "gold")]),
            "case {case:?}: expected the group to keep the tags it was created with, got \
             {tags:?}"
        );
    }
}
