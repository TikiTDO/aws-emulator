// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::ResourceGroups::Group` really creates the
//! group in Resource Groups, and the handles the stack hands back are the ones that service
//! answers to.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! OWNS it created it, through that service's own public API, under the identifiers the
//! type's published return values define — so `Ref` and `Fn::GetAtt` resolve to values the
//! owning service itself resolves to the resource the template declared, and deleting the
//! stack removes it there.** For this type the published return values are the group's NAME
//! (`Ref`) and the group ARN (`Fn::GetAtt "Arn"`), and a group ARN carries the owning
//! service's own `arnNamespace` — the hyphenated `resource-groups`, the only namespace token
//! the modelled group-ARN form admits (`Group$GroupArn` targets `GroupArnV2`, whose pattern is
//! `^arn:aws(-[a-z]+)*:resource-groups:…:group/…$`), not the lower-cased spelling of the
//! type's `ResourceGroups` namespace segment. Two crates have to agree for that to hold
//! (CloudFormation makes the calls, Resource Groups serves them), so the suite lives here
//! rather than in either crate.
//!
//! The rows differ along what the rule generalises over: the Region deployed in, whether the
//! template NAMES the group or leaves the name to CloudFormation, the FORM of the declared
//! name (plain, and one carrying the dots, underscores and hyphens the modelled
//! `CreateGroupName` pattern admits), which optional properties the declaration carries
//! (description, tags, a resource query or none), which `QueryType` the query is
//! (`TAG_FILTERS_1_0`, `CLOUDFORMATION_STACK_1_0`), whether the query's `Query` is declared as
//! the DOCUMENT a template writes or as the JSON TEXT the wire carries, and both template
//! spellings (JSON and YAML).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none, which
//! is CloudFormation's to compute: what is checked about it is that it is inside the form
//! Resource Groups publishes for a group name and that Resource Groups resolves it.
//!
//! The awsQuery form encoder and the XML readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element, outputs_of, physical_ids_of};

/// How long a wait for a stack to reach a TERMINAL status may run before the suite calls it a
/// failure. The stack's own clock-owed span is well under a second; this is a safety net so a
/// stack that never settles fails instead of hanging, and is generous enough that a loaded
/// host cannot decide the outcome. Nothing here is asserted about elapsed time.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(60);

/// How long to wait between two looks at a condition that is not satisfied yet.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The ARN namespace Resource Groups mints a group under: its model's
/// `awsService.arnNamespace`, which is what the modelled group-ARN pattern fixes.
const RESOURCE_GROUPS_ARN_NAMESPACE: &str = "resource-groups";

/// The namespace token a group ARN must NOT carry: the type's CloudFormation namespace
/// segment lower-cased, which is not any service's `arnNamespace`.
const UNHYPHENATED_NAMESPACE: &str = "resourcegroups";

/// The longest group name `CreateGroupName` admits (`@length(min: 1, max: 300)`), which a
/// GENERATED name is held to.
const MAX_GROUP_NAME: usize = 300;

/// How many `ListGroups` pages the reader below will take before calling a never-clearing
/// `NextToken` a failure. One page per group this suite creates, with room to spare.
const MAX_LIST_PAGES: usize = 64;

/// The refusal Resource Groups answers for a group it does not hold: the modelled
/// `NotFoundException` (HTTP 404) and that service's own fixed sentence.
const NOT_FOUND_CODE: &str = "NotFoundException";
const NOT_FOUND_MESSAGE: &str = "The specified group does not exist";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_resourcegroups_ardi::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which is
/// what routes the request and what each handler reads its Region from.
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

/// What one Resource Groups call answered: its HTTP status, the parsed body and the error code
/// the envelope named, wherever it put it.
struct Answer {
    status: u16,
    body: Value,
    code: String,
}

impl Answer {
    /// The error message the refusal carries, whichever casing the envelope spells it with.
    fn message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Call one Resource Groups operation over its restJson1 wire, signed for `region`: the method
/// and URI its own `@http` binding declares.
async fn resource_groups(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match &body {
        Some(document) => {
            Bytes::from(serde_json::to_vec(document).expect("the request body serializes"))
        }
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "host",
            format!("{RESOURCE_GROUPS_ARN_NAMESPACE}.{region}.amazonaws.com"),
        )
        .header("content-type", "application/json")
        .header(
            "authorization",
            authorization(RESOURCE_GROUPS_ARN_NAMESPACE, region),
        )
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
        .unwrap_or_default();
    let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let code = if header_code.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    // The envelope may qualify the code with a namespace or a shape id; a client branches on
    // the bare name.
    let code = code
        .rsplit(['#', '/', ':'])
        .next()
        .unwrap_or_default()
        .to_string();
    Answer { status, body, code }
}

/// `POST /get-group` — Resource Groups' own read of one group, by name or by ARN.
async fn get_group(router: &MultiServiceRouter, region: &str, group: &str) -> Answer {
    resource_groups(
        router,
        region,
        "POST",
        "/get-group",
        Some(json!({ "Group": group })),
    )
    .await
}

/// Every `GroupIdentifiers` entry `ListGroups` reports in `region`, as `(GroupName, GroupArn)`
/// pairs, read across EVERY page: the operation is paginated on `NextToken`, so a suite that
/// read one page could miss the group it is looking for. The page count is bounded so a token
/// that never clears fails the suite instead of looping for ever.
async fn listed_groups(
    router: &MultiServiceRouter,
    region: &str,
) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_LIST_PAGES {
        let mut request = serde_json::Map::new();
        if let Some(next) = &token {
            request.insert("NextToken".to_string(), Value::String(next.clone()));
        }
        let answer = resource_groups(
            router,
            region,
            "POST",
            // `MaxResults` is an `@httpQuery` member; a small page is what makes the loop
            // above read more than one of them.
            "/groups-list?maxResults=1",
            Some(Value::Object(request)),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "ListGroups in {region} must answer 200, got {} / {:?}",
            answer.status, answer.body
        );
        for entry in answer
            .body
            .get("GroupIdentifiers")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            found.push((
                entry
                    .get("GroupName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry
                    .get("GroupArn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
        token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .filter(|next| !next.is_empty())
            .map(ToString::to_string);
        if token.is_none() {
            return found;
        }
    }
    panic!("ListGroups in {region} paged past {MAX_LIST_PAGES} pages, having read {found:?}")
}

/// The tags Resource Groups holds for the group of `arn`, as the map `GetTags` reports.
async fn group_tags(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    // The ARN is a non-greedy `@httpLabel`, so it travels as ONE path segment with
    // everything outside RFC 3986's unreserved set escaped — the encoding this test crate
    // already writes once ([`crate::query_wire::form_encode`]).
    let encoded = form_encode(arn);
    let answer = resource_groups(
        router,
        region,
        "GET",
        &format!("/resources/{encoded}/tags"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "GetTags on {arn} must answer 200, got {} / {:?}",
        answer.status, answer.body
    );
    answer
        .body
        .get("Tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The stack's status, once it is no longer `*_IN_PROGRESS`: looked at again until it settles
/// or [`TERMINAL_DEADLINE`] passes, so what the suite waits for is the CONDITION rather than a
/// span of the host's clock.
async fn settled_stack(
    router: &MultiServiceRouter,
    region: &str,
    stack_name: &str,
) -> (String, String) {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let (status, described) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "DescribeStacks({stack_name}) must answer 200, got {status} with body {described}"
        );
        let reached = opt_element(&described, "StackStatus").unwrap_or_default();
        if !reached.ends_with("_IN_PROGRESS") || Instant::now() >= deadline {
            return (reached, described);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The resource query a row declares, and what Resource Groups must then report for it.
struct DeclaredQuery {
    /// The `ResourceQuery` the template writes, in the row's own form.
    declared: fn() -> Value,
    /// The `Type` the stored query must carry.
    query_type: &'static str,
    /// The `Query` TEXT the stored query must carry, which is a string on the wire however the
    /// template declared it.
    query_text: fn() -> String,
}

/// One row: a template, and what the rule says the group it declares must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the group is created in and the one
    /// its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The group's logical id in the template.
    logical_id: &'static str,
    /// The name the template declares, or `None` for a template that names nothing — whose
    /// group carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The description the declaration carries, where it carries one.
    declared_description: Option<&'static str>,
    /// The resource query the declaration carries, where it carries one.
    declared_query: Option<DeclaredQuery>,
    /// The tags the DECLARATION asks for, each of which the group must end up carrying.
    declared_tags: &'static [(&'static str, &'static str)],
    /// The stack-level tags the `CreateStack` call carries, which CloudFormation propagates to
    /// the resources it creates.
    stack_tags: &'static [(&'static str, &'static str)],
}

/// The tag-filter query most rows declare, as the DOCUMENT a template writes it.
fn tag_filter_query() -> Value {
    json!({
        "Type": "TAG_FILTERS_1_0",
        "Query": {
            "ResourceTypeFilters": ["AWS::AllSupported"],
            "TagFilters": [{ "Key": "sim2real", "Values": ["probe"] }],
        },
    })
}

/// That same query's `Query` as the JSON TEXT the wire carries.
fn tag_filter_query_text() -> String {
    serde_json::to_string(&json!({
        "ResourceTypeFilters": ["AWS::AllSupported"],
        "TagFilters": [{ "Key": "sim2real", "Values": ["probe"] }],
    }))
    .expect("the query document serializes")
}

/// A `CLOUDFORMATION_STACK_1_0` query, which names a stack rather than tags.
fn stack_query() -> Value {
    json!({
        "Type": "CLOUDFORMATION_STACK_1_0",
        "Query": {
            "ResourceTypeFilters": ["AWS::AllSupported"],
            "StackIdentifier": format!("arn:aws:cloudformation:eu-west-1:{ACCOUNT}:stack/rg-probe/0"),
        },
    })
}

fn stack_query_text() -> String {
    serde_json::to_string(&json!({
        "ResourceTypeFilters": ["AWS::AllSupported"],
        "StackIdentifier": format!("arn:aws:cloudformation:eu-west-1:{ACCOUNT}:stack/rg-probe/0"),
    }))
    .expect("the query document serializes")
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a named, described, tagged tag-filter group in JSON",
            region: "us-east-1",
            template: || {
                json!({
                    "AWSTemplateFormatVersion": "2010-09-09",
                    "Resources": {
                        "Group": {
                            "Type": "AWS::ResourceGroups::Group",
                            "Properties": {
                                "Name": "rg-probe-tagged",
                                "Description": "a tag-filter group a stack declared",
                                "ResourceQuery": tag_filter_query(),
                                "Tags": [
                                    { "Key": "Owner", "Value": "search" },
                                    { "Key": "Tier", "Value": "prod" }
                                ],
                            },
                        }
                    },
                    "Outputs": {
                        "GroupName": { "Value": { "Ref": "Group" } },
                        "GroupArn": { "Value": { "Fn::GetAtt": ["Group", "Arn"] } },
                    }
                })
                .to_string()
            },
            logical_id: "Group",
            declared_name: Some("rg-probe-tagged"),
            declared_description: Some("a tag-filter group a stack declared"),
            declared_query: Some(DeclaredQuery {
                declared: tag_filter_query,
                query_type: "TAG_FILTERS_1_0",
                query_text: tag_filter_query_text,
            }),
            declared_tags: &[("Owner", "search"), ("Tier", "prod")],
            stack_tags: &[("Fleet", "probe")],
        },
        Row {
            case: "a CloudFormation-stack query group in another Region, name with . _ -",
            region: "eu-west-1",
            template: || {
                json!({
                    "Resources": {
                        "TheGroup": {
                            "Type": "AWS::ResourceGroups::Group",
                            "Properties": {
                                "Name": "rg.probe_stack-2",
                                "Description": "a stack query group",
                                "ResourceQuery": stack_query(),
                            },
                        }
                    },
                    "Outputs": {
                        "GroupName": { "Value": { "Ref": "TheGroup" } },
                        "GroupArn": { "Value": { "Fn::GetAtt": ["TheGroup", "Arn"] } },
                    }
                })
                .to_string()
            },
            logical_id: "TheGroup",
            declared_name: Some("rg.probe_stack-2"),
            declared_description: Some("a stack query group"),
            declared_query: Some(DeclaredQuery {
                declared: stack_query,
                query_type: "CLOUDFORMATION_STACK_1_0",
                query_text: stack_query_text,
            }),
            declared_tags: &[],
            stack_tags: &[],
        },
        Row {
            case: "a group whose Query is declared as the JSON text the wire carries",
            region: "us-west-2",
            template: || {
                json!({
                    "Resources": {
                        "Group": {
                            "Type": "AWS::ResourceGroups::Group",
                            "Properties": {
                                "Name": "rg-probe-text-query",
                                "ResourceQuery": {
                                    "Type": "TAG_FILTERS_1_0",
                                    "Query": tag_filter_query_text(),
                                },
                            },
                        }
                    },
                    "Outputs": {
                        "GroupName": { "Value": { "Ref": "Group" } },
                        "GroupArn": { "Value": { "Fn::GetAtt": ["Group", "Arn"] } },
                    }
                })
                .to_string()
            },
            logical_id: "Group",
            declared_name: Some("rg-probe-text-query"),
            declared_description: None,
            declared_query: Some(DeclaredQuery {
                declared: || {
                    json!({
                        "Type": "TAG_FILTERS_1_0",
                        "Query": tag_filter_query_text(),
                    })
                },
                query_type: "TAG_FILTERS_1_0",
                query_text: tag_filter_query_text,
            }),
            declared_tags: &[],
            stack_tags: &[],
        },
        Row {
            // Neither a `ResourceQuery` nor a `Configuration` is required of `CreateGroup`, so
            // a group declaring no membership at all is a group all the same.
            case: "a group in YAML with a description and no resource query",
            region: "ap-south-1",
            template: || {
                [
                    "Resources:",
                    "  Group:",
                    "    Type: AWS::ResourceGroups::Group",
                    "    Properties:",
                    "      Name: rg-probe-yaml",
                    "      Description: a group declared in YAML",
                    "Outputs:",
                    "  GroupName:",
                    "    Value: !Ref Group",
                    "  GroupArn:",
                    "    Value: !GetAtt Group.Arn",
                    "",
                ]
                .join("\n")
            },
            logical_id: "Group",
            declared_name: Some("rg-probe-yaml"),
            declared_description: Some("a group declared in YAML"),
            declared_query: None,
            declared_tags: &[],
            stack_tags: &[],
        },
        Row {
            case: "a group the template does not name, so CloudFormation generates one",
            region: "us-east-2",
            template: || {
                json!({
                    "Resources": {
                        "Generated": {
                            "Type": "AWS::ResourceGroups::Group",
                            "Properties": { "ResourceQuery": tag_filter_query() },
                        }
                    },
                    "Outputs": {
                        "GroupName": { "Value": { "Ref": "Generated" } },
                        "GroupArn": { "Value": { "Fn::GetAtt": ["Generated", "Arn"] } },
                    }
                })
                .to_string()
            },
            logical_id: "Generated",
            declared_name: None,
            declared_description: None,
            declared_query: Some(DeclaredQuery {
                declared: tag_filter_query,
                query_type: "TAG_FILTERS_1_0",
                query_text: tag_filter_query_text,
            }),
            declared_tags: &[],
            stack_tags: &[],
        },
    ]
}

/// The group ARN of `name` in `region`: the one total function of the partition, the Region,
/// the account and the name the modelled group-ARN form fixes.
fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:{RESOURCE_GROUPS_ARN_NAMESPACE}:{region}:{ACCOUNT}:group/{name}")
}

/// Whether `name` is inside the form the model publishes for a group name
/// (`CreateGroupName`: `^[a-zA-Z0-9_\.-]+$`, 1 to 300 characters).
fn is_group_name(name: &str) -> bool {
    (1..=MAX_GROUP_NAME).contains(&name.chars().count())
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character == '_'
                || character == '.'
                || character == '-'
        })
}

/// Every row: the group a stack declares exists in Resource Groups under the identifiers the
/// stack published, carries what the declaration asked for, and is gone once the stack is.
#[tokio::test]
async fn a_stack_provisions_the_resource_group_its_template_declares() {
    let router = build_router();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("rg-stack-{index}");
        let template = (row.template)();

        let mut members: Vec<(String, String)> = vec![
            ("StackName".to_string(), stack_name.clone()),
            ("TemplateBody".to_string(), template),
        ];
        for (position, (key, value)) in row.stack_tags.iter().enumerate() {
            let ordinal = position + 1;
            members.push((format!("Tags.member.{ordinal}.Key"), (*key).to_string()));
            members.push((format!("Tags.member.{ordinal}.Value"), (*value).to_string()));
        }
        let borrowed: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();

        let (status, body) = cloudformation(&router, region, "CreateStack", &borrowed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (reached, described) = settled_stack(&router, region, &stack_name).await;
        assert_eq!(
            reached, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {reached:?} in \
             {described}"
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
        let physical_id = physical_ids.get(row.logical_id).unwrap_or_else(|| {
            panic!(
                "case {case:?}: no physical id for {:?} in {resources}",
                row.logical_id
            )
        });

        // "Ref returns the group name": the physical id is the name the template asked for, or
        // the one CloudFormation generated for a template that named nothing.
        let name = match row.declared_name {
            Some(declared) => {
                assert_eq!(
                    physical_id, declared,
                    "case {case:?}: expected the physical id to be the declared name \
                     {declared:?}, got {physical_id:?}"
                );
                declared.to_string()
            }
            None => physical_id.clone(),
        };
        assert!(
            is_group_name(&name),
            "case {case:?}: expected a group name inside the published name form, got {name:?}"
        );

        let arn = group_arn(region, &name);
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("GroupName"),
            Some(&name),
            "case {case:?}: expected the Ref output to be the group name {name:?}, got {:?}",
            outputs.get("GroupName")
        );
        assert_eq!(
            outputs.get("GroupArn"),
            Some(&arn),
            "case {case:?}: expected the Fn::GetAtt Arn output to be {arn:?}, got {:?}",
            outputs.get("GroupArn")
        );
        // The namespace token is the requirement the Region, the account and the name are not:
        // the published form is the hyphenated one, and the unhyphenated spelling matches no
        // group ARN at all.
        let published = outputs.get("GroupArn").cloned().unwrap_or_default();
        assert!(
            published.contains(&format!(":{RESOURCE_GROUPS_ARN_NAMESPACE}:")),
            "case {case:?}: expected the group ARN to carry the \
             {RESOURCE_GROUPS_ARN_NAMESPACE:?} namespace, got {published:?}"
        );
        assert!(
            !published.contains(&format!(":{UNHYPHENATED_NAMESPACE}:")),
            "case {case:?}: expected no {UNHYPHENATED_NAMESPACE:?} namespace in the group ARN, \
             got {published:?}"
        );

        // Resource Groups lists exactly one group under that name, and the pair it reports is
        // the pair the stack published.
        let listed: Vec<(String, String)> = listed_groups(&router, region)
            .await
            .into_iter()
            .filter(|(listed_name, _)| *listed_name == name)
            .collect();
        assert_eq!(
            listed,
            vec![(name.clone(), arn.clone())],
            "case {case:?}: expected ListGroups to report exactly one group ({name:?}, \
             {arn:?}), got {listed:?}"
        );

        // And that service's own read resolves the group, by the NAME the stack published and
        // by the ARN alike — which is what the ARN namespace has to be right for.
        for handle in [name.clone(), arn.clone()] {
            let read = get_group(&router, region, &handle).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 from GetGroup({handle:?}), got {} / {:?}",
                read.status, read.body
            );
            let group = read.body.get("Group").cloned().unwrap_or(Value::Null);
            assert_eq!(
                group.get("Name").and_then(Value::as_str),
                Some(name.as_str()),
                "case {case:?}: expected GetGroup({handle:?}) to report the name {name:?}, got \
                 {:?}",
                read.body
            );
            assert_eq!(
                group.get("GroupArn").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?}: expected GetGroup({handle:?}) to report the ARN {arn:?}, got \
                 {:?}",
                read.body
            );
            assert_eq!(
                group.get("Description").and_then(Value::as_str),
                row.declared_description,
                "case {case:?}: expected GetGroup({handle:?}) to report the declared \
                 description {:?}, got {:?}",
                row.declared_description,
                read.body
            );
        }

        // The query the declaration asked for is the query the group holds, as the `Type` and
        // the `Query` TEXT that service stores — whichever of the two forms the template wrote.
        if let Some(query) = &row.declared_query {
            let read = resource_groups(
                &router,
                region,
                "POST",
                "/get-group-query",
                Some(json!({ "Group": name })),
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 from GetGroupQuery, got {} / {:?}",
                read.status, read.body
            );
            let stored = read
                .body
                .get("GroupQuery")
                .and_then(|group| group.get("ResourceQuery"))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                stored.get("Type").and_then(Value::as_str),
                Some(query.query_type),
                "case {case:?}: expected the stored query type {:?}, got {:?}",
                query.query_type,
                read.body
            );
            let expected_text = (query.query_text)();
            let reported_text = stored
                .get("Query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                serde_json::from_str::<Value>(reported_text).unwrap_or(Value::Null),
                serde_json::from_str::<Value>(&expected_text).unwrap_or(Value::Null),
                "case {case:?}: expected the stored query text to be the declared document \
                 {expected_text}, got {reported_text:?}; the declaration was {}",
                (query.declared)()
            );
        }

        // Every tag the declaration asked for, and every stack-level tag CloudFormation
        // propagates, is a tag the group carries — and none of the reserved `aws:` keys, which
        // Resource Groups refuses on every call that takes tags.
        let held = group_tags(&router, region, &arn).await;
        for (key, value) in row.declared_tags.iter().chain(row.stack_tags.iter()) {
            assert_eq!(
                held.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected the group to carry the tag {key:?}={value:?}, got \
                 {held:?}"
            );
        }
        assert!(
            held.keys().all(|key| !key.starts_with("aws:")),
            "case {case:?}: expected no reserved tag key on the group, got {held:?}"
        );

        // Deleting the stack deletes what Resource Groups holds for it: the read is refused
        // with that service's own modelled not-found answer.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let deadline = Instant::now() + TERMINAL_DEADLINE;
        let mut gone = get_group(&router, region, &name).await;
        while gone.status == 200 && Instant::now() < deadline {
            tokio::time::sleep(POLL_INTERVAL).await;
            gone = get_group(&router, region, &name).await;
        }
        assert_eq!(
            (gone.status, gone.code.as_str()),
            (404, NOT_FOUND_CODE),
            "case {case:?}: expected HTTP 404 {NOT_FOUND_CODE} from GetGroup once the stack \
             deleted the group, got {} / {:?}",
            gone.status,
            gone.body
        );
        assert_eq!(
            gone.message(),
            NOT_FOUND_MESSAGE,
            "case {case:?}: expected the absent-group refusal, got {:?}",
            gone.body
        );
    }
}

/// A second stack declaring a group NAME Resource Groups already holds is refused by that
/// service, so the stack does not report a group it did not create — and the group the first
/// stack created is left exactly as it was.
///
/// The same rule as the suite above, read from the other side: the owning service's own
/// uniqueness rule ("a resource group name must be unique within each Amazon Web Services
/// Region in your Amazon Web Services account") is what decides a second create, and it can
/// only decide it because the first stack really created the group.
#[tokio::test]
async fn a_second_stack_declaring_the_same_group_name_leaves_the_original() {
    let router = build_router();
    let region = "eu-central-1";
    let name = "rg-probe-duplicate";

    let template = |description: &str| {
        json!({
            "Resources": {
                "Group": {
                    "Type": "AWS::ResourceGroups::Group",
                    "Properties": {
                        "Name": name,
                        "Description": description,
                        "ResourceQuery": tag_filter_query(),
                    },
                }
            },
            "Outputs": {
                "GroupArn": { "Value": { "Fn::GetAtt": ["Group", "Arn"] } },
            }
        })
        .to_string()
    };

    let first = template("the original group");
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "rg-dupe-first"), ("TemplateBody", &first)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the first CreateStack must answer 200, got {status} with body {body}"
    );
    let (reached, described) = settled_stack(&router, region, "rg-dupe-first").await;
    assert_eq!(
        reached, "CREATE_COMPLETE",
        "the first stack must reach CREATE_COMPLETE, got {reached:?} in {described}"
    );
    let arn = group_arn(region, name);
    assert_eq!(
        outputs_of(&described).get("GroupArn"),
        Some(&arn),
        "the first stack must publish the group ARN {arn:?}, got {described}"
    );

    let second = template("the group the second stack asked for");
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "rg-dupe-second"), ("TemplateBody", &second)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the second CreateStack must answer 200 — the template declares a type CloudFormation \
         publishes — got {status} with body {body}"
    );
    let (reached, described) = settled_stack(&router, region, "rg-dupe-second").await;
    assert_ne!(
        reached, "CREATE_COMPLETE",
        "the second stack must not report a group it did not create, got {reached:?} in \
         {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Group"),
        "the second stack's failure must name the resource, got {reason:?} in {described}"
    );

    // The original is untouched: still there, still the first stack's description.
    let read = get_group(&router, region, name).await;
    assert_eq!(
        read.status, 200,
        "the original group must still be readable, got {} / {:?}",
        read.status, read.body
    );
    let group = read.body.get("Group").cloned().unwrap_or(Value::Null);
    assert_eq!(
        group.get("Description").and_then(Value::as_str),
        Some("the original group"),
        "the original group must keep the first stack's description, got {:?}",
        read.body
    );
    assert_eq!(
        group.get("GroupArn").and_then(Value::as_str),
        Some(arn.as_str()),
        "the original group must keep its ARN {arn:?}, got {:?}",
        read.body
    );
}
