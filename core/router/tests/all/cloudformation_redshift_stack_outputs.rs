// SPDX-License-Identifier: CC0-1.0

//! What a completed CloudFormation stack publishes for the Amazon Redshift resources it
//! declared: every `Output` it reports carries a RESOLVED `OutputValue`, and each value is the
//! one the service that owns the resource reports for it.
//!
//! The rule under test: **a stack that reaches a completed status reports every output its
//! template declares with a value, and a value it publishes for a resource (`Ref`, and each
//! `Fn::GetAtt` attribute the type publishes) is read from the service that owns that
//! resource** — so a client reading `Stacks[0].Outputs` finds an `OutputValue` on every entry,
//! and the endpoint, port and parameter-group name it finds are what Amazon Redshift answers
//! for the cluster and the group the stack created. Two crates have to agree for that to hold
//! (CloudFormation publishes the outputs, Amazon Redshift owns the resources), so the suite
//! lives here.
//!
//! The rows differ along what the rule generalises over: whether the template states the
//! cluster's port or leaves the documented default (`CreateCluster$Port`: "Default: 5439"),
//! whether it declares a cluster parameter group beside the cluster and whether it NAMES that
//! group or leaves CloudFormation to generate the name, which published family the group
//! belongs to, and how the outputs read the published values — a `Ref`, an `Fn::GetAtt` of
//! each endpoint member, and an `Fn::Sub` composing both.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// How long the suite is willing to wait for a stack operation to settle, and how often it
/// looks again. Generous: the stack's own clock-owed transition and the created cluster's
/// provisioning span both have to come due, and the suite waits for the CONDITION (the stack's
/// status) rather than for a fixed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);

/// The admin password every row declares. It must never be echoed by either service.
const PASSWORD: &str = "Cfn-Redsh1ft-Out";

/// The port Amazon Redshift gives a cluster whose create states none
/// (`smithy_query(query="doc", shape="CreateClusterMessage", member="Port")`: "Default:
/// 5439").
const DEFAULT_PORT: &str = "5439";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_redshift_redshiftserviceversion20121201::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one query-protocol operation of `service`, at `version`, with these form members.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon Redshift operation over its awsQuery wire.
async fn redshift(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "redshift", "2012-12-01", action, members).await
}

/// Why a stack's resources failed, as CloudFormation records it on the stack's own events:
/// quoted in an assertion message, so a stack that did not complete says what the owning
/// service refused rather than only that it rolled back.
async fn failure_reasons(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (_, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    elements(&body, "ResourceStatusReason")
}

/// The `DescribeStacks` body once the stack rests in a status that is not in progress, or the
/// last one read when the deadline passes. Waits for the CONDITION, looking again until the
/// stack has settled.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        let settled = opt_element(&body, "StackStatus")
            .is_some_and(|stack_status| !stack_status.ends_with("_IN_PROGRESS"));
        if settled || std::time::Instant::now() >= deadline {
            return body;
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Create the stack `stack_name` from `template` and answer its settled `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    settled_stack(router, case, stack_name).await
}

/// The logical ids every row declares its resources under.
const CLUSTER_LOGICAL_ID: &str = "Warehouse";
const GROUP_LOGICAL_ID: &str = "Settings";

/// The parameter every row that declares a group sets on it, with the value it sets: one of
/// the settings a `redshift-*` family publishes (`DescribeDefaultClusterParameters` lists
/// `enable_user_activity_logging` among them, whose engine default is `false`).
const GROUP_PARAMETER: (&str, &str) = ("enable_user_activity_logging", "true");

/// A second setting of that same catalogue, whose engine default is `false` too
/// (`require_ssl`): the one the update case DROPS, so its reset is observable.
const DROPPED_PARAMETER: &str = "require_ssl";

/// The `Parameters` list a row that declares a group states by default: the one setting above.
const DECLARED_PARAMETERS: &str =
    r#"[{"ParameterName":"enable_user_activity_logging","ParameterValue":"true"}]"#;

/// One row of the table: the stack a case deploys, and what the outputs it publishes must
/// carry.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The `ClusterIdentifier` the template states.
    cluster_identifier: &'static str,
    node_type: &'static str,
    /// The `Port` the template states, or `""` for a template that states none — the cluster
    /// then serves on the documented default.
    port: &'static str,
    /// The `ParameterGroupFamily` the row's group declares, or `""` for a row that declares no
    /// parameter group at all.
    group_family: &'static str,
    /// The `ParameterGroupName` the row's group states, or `""` where CloudFormation generates
    /// it.
    group_name: &'static str,
    /// The `Parameters` list the row's group declares, as template JSON.
    group_parameters: &'static str,
}

/// The rows: the axis is what the template states and how its outputs read the published
/// values, so the rule is not keyed to one declaration shape. The families are the ones a
/// group may belong to (`CreateClusterParameterGroup$ParameterGroupFamily`: "The parameter
/// group family names associated with the default parameter groups provide you the valid
/// values"), and the ports are the default and two inside the ranges that member publishes for
/// RA3 nodes (`5431-5455` and `8191-8215`).
const ROWS: &[Row] = &[
    Row {
        case: "a cluster alone, on the default port",
        stack_name: "rs-outputs-default-port",
        cluster_identifier: "cfn-outputs-plain",
        node_type: "ra3.xlplus",
        port: "",
        group_family: "",
        group_name: "",
        group_parameters: "",
    },
    Row {
        case: "a cluster on a stated port, configured from a named parameter group",
        stack_name: "rs-outputs-named-group",
        cluster_identifier: "cfn-outputs-named",
        node_type: "ra3.xlplus",
        port: "5441",
        group_family: "redshift-1.0",
        group_name: "cfn-outputs-named-pg",
        group_parameters: DECLARED_PARAMETERS,
    },
    Row {
        case: "a cluster configured from a parameter group CloudFormation names",
        stack_name: "rs-outputs-generated-group",
        cluster_identifier: "cfn-outputs-generated",
        node_type: "ra3.large",
        port: "8215",
        group_family: "redshift-2.0",
        group_name: "",
        group_parameters: DECLARED_PARAMETERS,
    },
];

/// Every output key the template of `row` declares, which is the set its `Outputs` must report
/// a value for.
fn declared_output_keys(row: &Row) -> Vec<&'static str> {
    let mut keys = vec!["ClusterRef", "EndpointAddress", "EndpointPort", "Connection"];
    if !row.group_family.is_empty() {
        keys.push("GroupRef");
    }
    keys
}

/// The template one row deploys: the cluster, the parameter group it is configured from where
/// the row declares one, and the outputs that publish what the stack knows about them.
fn template_of(row: &Row) -> String {
    let port = if row.port.is_empty() {
        String::new()
    } else {
        format!(r#","Port":{}"#, row.port)
    };
    let (group_resource, group_property, group_output) = if row.group_family.is_empty() {
        (String::new(), String::new(), String::new())
    } else {
        let named = if row.group_name.is_empty() {
            String::new()
        } else {
            format!(r#","ParameterGroupName":"{}""#, row.group_name)
        };
        (
            format!(
                r#","{GROUP_LOGICAL_ID}":{{"Type":"AWS::Redshift::ClusterParameterGroup","Properties":{{"Description":"settings for {}","ParameterGroupFamily":"{}"{named},"Parameters":{}}}}}"#,
                row.cluster_identifier, row.group_family, row.group_parameters
            ),
            format!(r#","ClusterParameterGroupName":{{"Ref":"{GROUP_LOGICAL_ID}"}}"#),
            format!(r#","GroupRef":{{"Value":{{"Ref":"{GROUP_LOGICAL_ID}"}}}}"#),
        )
    };
    format!(
        r#"{{"Resources":{{"{CLUSTER_LOGICAL_ID}":{{"Type":"AWS::Redshift::Cluster","Properties":{{"ClusterIdentifier":"{}","ClusterType":"single-node","NodeType":"{}","MasterUsername":"admin","MasterUserPassword":"{PASSWORD}"{port}{group_property}}}}}{group_resource}}},
             "Outputs":{{"ClusterRef":{{"Value":{{"Ref":"{CLUSTER_LOGICAL_ID}"}}}},
                         "EndpointAddress":{{"Value":{{"Fn::GetAtt":["{CLUSTER_LOGICAL_ID}","Endpoint.Address"]}}}},
                         "EndpointPort":{{"Value":{{"Fn::GetAtt":["{CLUSTER_LOGICAL_ID}","Endpoint.Port"]}}}},
                         "Connection":{{"Value":{{"Fn::Sub":"${{{CLUSTER_LOGICAL_ID}.Endpoint.Address}}:${{{CLUSTER_LOGICAL_ID}.Endpoint.Port}}"}}}}{group_output}}}}}"#,
        row.cluster_identifier, row.node_type
    )
}

/// The inner XML of the `<Parameter>` element of a `DescribeClusterParameters` body that
/// carries `name`, or `None` when the body reports no such parameter.
///
/// Matched on the element NAME rather than on a fixed `<Parameter>` open tag, because the
/// serializer repeats the service's XML namespace as an attribute on the first member of a
/// list (`<Parameter xmlns="…">`) and because `<ParameterName>` must not be mistaken for a
/// list member.
fn reported_parameter<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    const OPEN: &str = "<Parameter";
    const CLOSE: &str = "</Parameter>";
    let mut rest = body;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        if !(after.starts_with('>') || after.starts_with(' ')) {
            rest = after;
            continue;
        }
        let Some(head_end) = after.find('>') else {
            break;
        };
        let inner = &after[head_end + 1..];
        let Some(end) = inner.find(CLOSE) else { break };
        let block = &inner[..end];
        if opt_element(block, "ParameterName").as_deref() == Some(name) {
            return Some(block);
        }
        rest = &inner[end + CLOSE.len()..];
    }
    None
}

#[tokio::test]
async fn every_output_of_a_completed_redshift_stack_carries_the_value_redshift_reports() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let body = create_and_settle(&router, case, row.stack_name, &template_of(row)).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body} with the failure \
             reasons {:?}",
            failure_reasons(&router, row.stack_name).await
        );

        // THE RULE's first half: every entry of `Outputs` carries an `OutputValue`, so a
        // client indexing each entry by it finds a value rather than a missing member.
        let keys = elements(&body, "OutputKey");
        let values = elements(&body, "OutputValue");
        let expected_keys = declared_output_keys(row);
        assert_eq!(
            (keys.len(), values.len()),
            (expected_keys.len(), expected_keys.len()),
            "case {case:?}: expected one OutputKey and one OutputValue per declared output \
             ({expected_keys:?}), got keys {keys:?} and values {values:?} in {body}"
        );
        let outputs = outputs_of(&body);
        for key in &expected_keys {
            let value = outputs.get(*key);
            assert!(
                value.is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected the output {key:?} to carry a resolved value, got \
                 {value:?} in {body}"
            );
        }

        // THE RULE's second half: each published value is the one Amazon Redshift reports for
        // the resource the stack created.
        let identifier = outputs["ClusterRef"].clone();
        assert_eq!(
            identifier, row.cluster_identifier,
            "case {case:?}: expected the declared identifier as the cluster's Ref, got \
             {identifier:?}"
        );
        let (status, reported) = redshift(
            &router,
            "DescribeClusters",
            &[("ClusterIdentifier", identifier.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Redshift to report the cluster the stack created, got \
             {status} with body {reported}"
        );
        let cluster = block_of(&reported, "Cluster")
            .unwrap_or_else(|| panic!("case {case:?}: no Cluster in {reported}"));
        assert_eq!(
            opt_element(cluster, "ClusterStatus").as_deref(),
            Some("available"),
            "case {case:?}: expected a completed stack to leave the cluster available, got \
             {cluster}"
        );
        let endpoint = block_of(cluster, "Endpoint")
            .unwrap_or_else(|| panic!("case {case:?}: no Endpoint on the cluster in {reported}"));
        assert_eq!(
            Some(outputs["EndpointAddress"].as_str()),
            opt_element(endpoint, "Address").as_deref(),
            "case {case:?}: expected the published address to be Redshift's own, got {endpoint}"
        );
        let expected_port = if row.port.is_empty() {
            DEFAULT_PORT
        } else {
            row.port
        };
        assert_eq!(
            (
                outputs["EndpointPort"].as_str(),
                opt_element(endpoint, "Port").as_deref()
            ),
            (expected_port, Some(expected_port)),
            "case {case:?}: expected the published port to be the one the cluster serves on, \
             got {endpoint}"
        );
        // The output COMPOSED from both members is the pair, which is what a connection
        // string a consumer builds out of a stack's outputs is.
        assert_eq!(
            outputs["Connection"],
            format!("{}:{expected_port}", outputs["EndpointAddress"]),
            "case {case:?}: expected the composed output to carry both members, got {outputs:?}"
        );
        assert!(
            !reported.contains(PASSWORD),
            "case {case:?}: the admin password must never reach the wire, got {reported}"
        );

        if row.group_family.is_empty() {
            continue;
        }

        // The group the stack published is one Amazon Redshift holds under exactly that name,
        // with the family and the setting the template declared — and it is the group the
        // cluster is configured from.
        let group = outputs["GroupRef"].clone();
        if !row.group_name.is_empty() {
            assert_eq!(
                group, row.group_name,
                "case {case:?}: expected the declared ParameterGroupName as the group's Ref, \
                 got {group:?}"
            );
        }
        let memberships = block_of(cluster, "ClusterParameterGroups").unwrap_or_else(|| {
            panic!("case {case:?}: no ClusterParameterGroups on the cluster in {reported}")
        });
        assert_eq!(
            opt_element(memberships, "ParameterGroupName").as_deref(),
            Some(group.as_str()),
            "case {case:?}: expected the cluster to be configured from the published group, \
             got {memberships}"
        );
        let (status, groups) = redshift(
            &router,
            "DescribeClusterParameterGroups",
            &[("ParameterGroupName", group.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Redshift to report the group the stack created, got \
             {status} with body {groups}"
        );
        assert_eq!(
            (
                opt_element(&groups, "ParameterGroupName").as_deref(),
                opt_element(&groups, "ParameterGroupFamily").as_deref()
            ),
            (Some(group.as_str()), Some(row.group_family)),
            "case {case:?}: expected the group to be held under the published name and family, \
             got {groups}"
        );
        let (status, parameters) = redshift(
            &router,
            "DescribeClusterParameters",
            &[("ParameterGroupName", group.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Redshift to report the group's parameters, got {status} \
             with body {parameters}"
        );
        let setting = reported_parameter(&parameters, GROUP_PARAMETER.0).unwrap_or_else(|| {
            panic!(
                "case {case:?}: no {:?} parameter in {parameters}",
                GROUP_PARAMETER.0
            )
        });
        assert_eq!(
            opt_element(setting, "ParameterValue").as_deref(),
            Some(GROUP_PARAMETER.1),
            "case {case:?}: expected the declared parameter value on the group, got {setting}"
        );
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_cluster_and_its_parameter_group() {
    let case = "delete the stack that created the cluster and its group";
    let row = Row {
        case,
        stack_name: "rs-outputs-deleted",
        cluster_identifier: "cfn-outputs-deleted",
        node_type: "ra3.xlplus",
        port: "5439",
        group_family: "redshift-1.0",
        group_name: "cfn-outputs-deleted-pg",
        group_parameters: DECLARED_PARAMETERS,
    };
    let router = build_router();

    let body = create_and_settle(&router, case, row.stack_name, &template_of(&row)).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body} with the failure reasons \
         {:?}",
        failure_reasons(&router, row.stack_name).await
    );
    // "Deleted stacks: You must specify the unique stack ID": the deletion is polled by the
    // id, which stays addressable once the stack's name no longer is.
    let stack_id = opt_element(&body, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: no StackId in {body}"));

    let (status, body) =
        cloudformation(&router, "DeleteStack", &[("StackName", row.stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    let body = settled_stack(&router, case, &stack_id).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("DELETE_COMPLETE"),
        "case {case:?}: expected the stack's deletion to complete, got {body} with the \
         failure reasons {:?}",
        failure_reasons(&router, &stack_id).await
    );

    // Both resources went with the stack: the group is deletable only once the cluster that
    // was configured from it is gone, which is what the stack's own order carries out.
    for (action, member, identifier, code) in [
        (
            "DescribeClusters",
            "ClusterIdentifier",
            row.cluster_identifier,
            "ClusterNotFound",
        ),
        (
            "DescribeClusterParameterGroups",
            "ParameterGroupName",
            row.group_name,
            "ClusterParameterGroupNotFound",
        ),
    ] {
        let (status, reported) = redshift(&router, action, &[(member, identifier)]).await;
        assert_eq!(
            (status, opt_element(&reported, "Code").as_deref()),
            (404, Some(code)),
            "case {case:?}: expected {identifier:?} to be gone with the stack, got {status} \
             with body {reported}"
        );
    }
}

#[tokio::test]
async fn updating_the_stack_applies_the_parameter_settings_it_now_declares() {
    let case = "an update that changes one setting of the group and drops another";
    let before = Row {
        case,
        stack_name: "rs-outputs-updated",
        cluster_identifier: "cfn-outputs-updated",
        node_type: "ra3.xlplus",
        port: "5441",
        group_family: "redshift-1.0",
        group_name: "cfn-outputs-updated-pg",
        group_parameters: concat!(
            r#"[{"ParameterName":"enable_user_activity_logging","ParameterValue":"true"},"#,
            r#"{"ParameterName":"require_ssl","ParameterValue":"true"}]"#
        ),
    };
    // The new revision states the first setting differently and no longer states the second.
    let after = Row {
        group_parameters:
            r#"[{"ParameterName":"enable_user_activity_logging","ParameterValue":"false"}]"#,
        ..before
    };
    let router = build_router();

    let body = create_and_settle(&router, case, before.stack_name, &template_of(&before)).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body} with the failure reasons \
         {:?}",
        failure_reasons(&router, before.stack_name).await
    );

    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", after.stack_name),
            ("TemplateBody", &template_of(&after)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    let body = settled_stack(&router, case, after.stack_name).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {body} with the failure reasons \
         {:?}",
        failure_reasons(&router, after.stack_name).await
    );

    // The group the stack already had carries what the NEW revision declares: the setting it
    // states differently is that value and was set by this account, and the one it no longer
    // states is back to the engine's own default.
    let (status, parameters) = redshift(
        &router,
        "DescribeClusterParameters",
        &[("ParameterGroupName", after.group_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Redshift to report the group's parameters, got {status} with \
         body {parameters}"
    );
    for (parameter, value, source) in [
        (GROUP_PARAMETER.0, "false", "user"),
        (DROPPED_PARAMETER, "false", "engine-default"),
    ] {
        let setting = reported_parameter(&parameters, parameter)
            .unwrap_or_else(|| panic!("case {case:?}: no {parameter:?} in {parameters}"));
        assert_eq!(
            (
                opt_element(setting, "ParameterValue").as_deref(),
                opt_element(setting, "Source").as_deref()
            ),
            (Some(value), Some(source)),
            "case {case:?}: expected {parameter:?} to be {value:?} from {source:?}, got \
             {setting}"
        );
    }
}
