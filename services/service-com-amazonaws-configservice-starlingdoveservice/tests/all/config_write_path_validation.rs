// SPDX-License-Identifier: CC0-1.0

//! What Config's write paths ACCEPT and what they REFUSE, end to end over the wire.
//!
//! Five rules are pinned here, each over a TABLE of inputs rather than one recorded
//! call:
//!
//! 1. `PutResourceConfig` records a configuration item only for a resource type the
//!    CloudFormation registry holds a registration for — "The custom resource type must
//!    be registered with CloudFormation" — refusing one it holds none for 400
//!    `ValidationException`, the class of a value the request carried that fails
//!    validation, and recording nothing;
//! 2. `PutStoredQuery` saves a query only when its `Expression` IS a Config query (the
//!    SQL `SELECT` command a select operation runs), refusing anything else 400
//!    `ValidationException` and creating no query;
//! 3. `PutRemediationExceptions` places an exception only under a rule the account
//!    holds a remediation configuration for, because the exception exempts a resource
//!    from the remediation that configuration runs, and refuses the whole request 400
//!    `InvalidParameterValueException` otherwise;
//! 4. `PutRemediationConfigurations` validates the MODELLED requirements of a
//!    `RemediationConfiguration` (`ConfigRuleName`, `TargetType`, `TargetId`, and a
//!    parameter that selects exactly one kind of value) and requires a role parameter
//!    only of an AUTOMATIC remediation, so a manual one carrying no role parameter is
//!    accepted;
//! 5. `ListTagsForResource` always reports `Tags` — an empty list for a resource that
//!    carries none — so a caller reads the member unconditionally.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` defines, and every write is read
//! back through the operation that reports it.

use serde_json::{json, Value};

use crate::config_connectors::refusal;
use crate::config_discovered_resource_counts::counts_of;
use crate::config_setup_and_rule_loop::{
    managed_rule, with_recorder_and_channel, with_verdicts, Emulator,
};

/// The name Config assigns a configuration recorder created without one
/// (`ConfigurationRecorder$name`: "Config automatically assigns the name of
/// \"default\" ... if you do not specify a name at creation time").
const RECORDER: &str = "default";

/// The only published `RemediationTargetType` value, from `smithy_query(query="shape",
/// shape="RemediationTargetType")`: `SSM_DOCUMENT`.
const SSM_DOCUMENT: &str = "SSM_DOCUMENT";

/// Resource types the CloudFormation registry publishes, from
/// `smithy_query(query="shape", shape="ResourceType")` — the enum of registry types this
/// service names — picked across the catalogue rather than from the types the issue
/// cited.
const REGISTERED_TYPES: [&str; 6] = [
    "AWS::EC2::Volume",
    "AWS::DynamoDB::Table",
    "AWS::SQS::Queue",
    "AWS::KMS::Key",
    "AWS::Lambda::Function",
    "AWS::ApiGateway::Method",
];

/// An emulator with a configuration recorder that is recording, which Config needs
/// before it records any configuration item. Shared with the suites that pin what the
/// other reads report for a resource this one's writes recorded or refused.
pub(crate) async fn recording_emulator() -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
    emulator
}

/// The `PutResourceConfig` body recording `configuration` for one resource. Shared with
/// the suites that record a resource through this write path.
pub(crate) fn resource_config(resource_type: &str, resource_id: &str, configuration: &str) -> Value {
    json!({
        "ResourceType": resource_type,
        "SchemaVersionId": "00000000-0000-0000-0000-000000000001",
        "ResourceId": resource_id,
        "ResourceName": resource_id,
        "Configuration": configuration,
    })
}

/// The configuration documents `GetResourceConfigHistory` reports for one resource,
/// newest state first.
async fn recorded_configurations(
    case: &str,
    emulator: &Emulator,
    resource_type: &str,
    resource_id: &str,
) -> Vec<String> {
    let body = emulator
        .ok(
            "GetResourceConfigHistory",
            &json!({ "resourceType": resource_type, "resourceId": resource_id }),
        )
        .await;
    body["configurationItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected configurationItems, got {body}"))
        .iter()
        .map(|item| {
            item["configuration"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a configuration, got {item}"))
                .to_string()
        })
        .collect()
}

/// The `(resourceType, count)` pairs Config reports as discovered, read over the wire
/// through the operation whose `resourceTypes` are plain strings.
async fn discovered(case: &str, emulator: &Emulator) -> Vec<(String, i64)> {
    let body = emulator.ok("GetDiscoveredResourceCounts", &json!({})).await;
    counts_of(case, &body)
}

#[tokio::test]
async fn put_resource_config_records_a_registered_resource_type_and_reads_it_back() {
    // Rows differ along what the rule generalises over: the registered resource type
    // recorded, the resource id, and the configuration document — an empty object, a
    // nested one, a list, a non-ASCII value, a null member, and a document of many
    // members.
    let cases: [(&str, &str, &str); 6] = [
        ("a volume with no members at all", "vol-0001", "{}"),
        (
            "a table with a nested document",
            "orders",
            r#"{"capacity":{"bytes":1024,"tier":"cold"}}"#,
        ),
        ("a queue with a list", "queue/7", r#"{"subscribers":[1,2,3]}"#),
        (
            "a key with a non-ASCII value",
            "key-é-1",
            r#"{"alias":"füße"}"#,
        ),
        ("a function with a null member", "fn-1", r#"{"vpc":null}"#),
        (
            "a method with many members",
            "method-1",
            r#"{"a":1,"b":"two","c":[3],"d":{"e":true}}"#,
        ),
    ];

    for (index, (case, resource_id, configuration)) in cases.into_iter().enumerate() {
        let resource_type = REGISTERED_TYPES[index];
        let emulator = recording_emulator().await;

        let answered = emulator
            .ok(
                "PutResourceConfig",
                &resource_config(resource_type, resource_id, configuration),
            )
            .await;
        assert_eq!(
            answered,
            json!({}),
            "case {case:?}: PutResourceConfig answers an empty body, got {answered}"
        );

        // Read back through the history of the resource: what was recorded is a
        // configuration item of that resource, carrying the document byte for byte.
        let recorded = recorded_configurations(case, &emulator, resource_type, resource_id).await;
        assert_eq!(
            recorded,
            vec![configuration.to_string()],
            "case {case:?}: expected the recorded configuration read back unchanged, got \
             {recorded:?}"
        );
        let counted = discovered(case, &emulator).await;
        assert_eq!(
            counted,
            vec![(resource_type.to_string(), 1)],
            "case {case:?}: the recorded resource is one discovered resource of its type, \
             got {counted:?}"
        );
    }
}

#[tokio::test]
async fn put_resource_config_refuses_a_resource_type_with_no_registration() {
    // Rows differ along WHY the CloudFormation registry holds no registration for the
    // type: an organization nothing registered a type under (several, so the rule is not
    // read off one name), an organization reserved against custom resource types in each
    // spelling the reservation covers, a name that is not a registry type name at all, a
    // registry MODULE name, a misspelled registered type, a registered type spelled in
    // another case, and the account-level type the registry publishes nothing for.
    let cases: [(&str, &str); 13] = [
        ("an organization nothing registered", "Acme::Network::Switch"),
        ("another unregistered organization", "MyOrg::MyService::MyResource"),
        ("a third unregistered organization", "Contoso::Billing::Invoice"),
        ("the reserved organization Amazon", "Amazon::Thing::Widget"),
        ("the reserved organization AMZN", "AMZN::Thing::Widget"),
        ("the reserved organization Alexa", "Alexa::Smart::Home"),
        ("the reserved organization custom", "custom::Thing::Widget"),
        ("a type name of one segment", "Switch"),
        ("a type name of two segments", "Acme::Switch"),
        ("a registry module name", "Acme::Network::Switch::MODULE"),
        ("a misspelled registered type", "AWS::DynamoDB::Tables"),
        ("a registered type in another case", "aws::dynamodb::table"),
        ("the account-level type", "AWS::::Account"),
    ];

    for (case, resource_type) in cases {
        let emulator = recording_emulator().await;
        let (status, code, message) = refusal(
            &emulator,
            "PutResourceConfig",
            &resource_config(resource_type, "r-1", r#"{"state":"new"}"#),
        )
        .await;
        let expected = format!(
            "Cannot find registered Resource Type with name {resource_type}. Please register \
             the Resource Type with Cloudformation and try again."
        );
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationException", expected.as_str()),
            "case {case:?}: expected the registration precondition for {resource_type:?}, got \
             {status} with {code} and {message:?}"
        );

        // Nothing is recorded: the refused resource is no discovered resource at all.
        let counted = discovered(case, &emulator).await;
        assert!(
            counted.is_empty(),
            "case {case:?}: a refused PutResourceConfig records nothing, got {counted:?}"
        );
    }
}

/// The `PutStoredQuery` body saving `expression` under `name`.
fn stored_query(name: &str, expression: &str) -> Value {
    json!({ "StoredQuery": { "QueryName": name, "Expression": expression } })
}

#[tokio::test]
async fn put_stored_query_saves_an_expression_that_is_a_config_query() {
    // Rows differ along the shape of the query: the clauses it carries (WHERE, GROUP BY
    // with an aggregate, ORDER BY, LIMIT), a nested property path, a comment, and the
    // whitespace it is written with.
    let accepted: [(&str, &str); 7] = [
        ("one selected property", "SELECT resourceId"),
        (
            "a condition on the resource type",
            "SELECT resourceId, resourceType WHERE resourceType = 'AWS::EC2::Volume'",
        ),
        (
            "a nested property path",
            "SELECT configuration.state WHERE resourceType = 'AWS::EC2::Volume'",
        ),
        (
            "an aggregate over a group",
            "SELECT resourceType, COUNT(*) GROUP BY resourceType",
        ),
        (
            "an ordering and a limit",
            "SELECT resourceId ORDER BY resourceId DESC LIMIT 10",
        ),
        (
            "newlines and tabs between the clauses",
            "SELECT\n\tresourceId\nWHERE\n\tresourceType = 'AWS::SQS::Queue'",
        ),
        (
            "a trailing comment",
            "SELECT resourceId /* every recorded resource */",
        ),
    ];

    for (index, (case, expression)) in accepted.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("query-{index}");
        let saved = emulator
            .ok("PutStoredQuery", &stored_query(&name, expression))
            .await;
        assert!(
            saved["QueryArn"]
                .as_str()
                .is_some_and(|arn| arn.contains(&format!(":stored-query/{name}/query-"))),
            "case {case:?}: expected the ARN of the saved query, got {saved}"
        );

        let read_back = emulator
            .ok("GetStoredQuery", &json!({ "QueryName": name }))
            .await;
        assert_eq!(
            read_back["StoredQuery"]["Expression"].as_str(),
            Some(expression),
            "case {case:?}: expected the expression read back unchanged, got {read_back}"
        );
    }
}

#[tokio::test]
async fn put_stored_query_refuses_an_expression_that_is_no_config_query() {
    // Rows differ along WHY the text is not a Config query: it is no SQL command at all,
    // it is another SQL command, or it is a SELECT carrying a clause the Config query
    // language has no place for (a FROM, a join, a subquery, a set operation, a HAVING).
    let refused: [(&str, &str); 10] = [
        ("prose", "this is not a query"),
        ("a bare word", "resources"),
        ("one character", "*"),
        ("a SELECT with nothing selected", "SELECT"),
        ("an unbalanced quote", "SELECT resourceId WHERE resourceId = 'x"),
        ("a data-modifying command", "DELETE resourceId"),
        ("an INSERT command", "INSERT INTO resources VALUES (1)"),
        (
            "a FROM clause the language has no place for",
            "SELECT resourceId FROM configuration_items",
        ),
        (
            "a set operation",
            "SELECT resourceId UNION SELECT resourceType",
        ),
        (
            "a subquery",
            "SELECT resourceId WHERE resourceId IN (SELECT resourceId)",
        ),
    ];

    for (index, (case, expression)) in refused.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("refused-{index}");
        let (status, code, message) =
            refusal(&emulator, "PutStoredQuery", &stored_query(&name, expression)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException for {expression:?}, got \
             {status} with {code} and {message:?}"
        );
        assert!(
            message.starts_with("syntax error at line "),
            "case {case:?}: the refusal must report where the reading of the expression \
             stopped, got {message:?}"
        );

        // No query was created: the name is not one the account holds.
        let (status, code) = emulator
            .refused("GetStoredQuery", &json!({ "QueryName": name }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused save must create no query, got {status} with {code}"
        );
    }
}

/// The `PutRemediationConfigurations` body for one configuration of `rule`, carrying
/// `parameters` only when the case names any and `Automatic` as the case asks.
fn remediation_configuration(rule: &str, automatic: bool, parameters: Option<Value>) -> Value {
    let mut configuration = json!({
        "ConfigRuleName": rule,
        "TargetType": SSM_DOCUMENT,
        "TargetId": "AWS-PublishSNSNotification",
        "TargetVersion": "1",
        "Automatic": automatic,
    });
    if let Some(parameters) = parameters {
        configuration["Parameters"] = parameters;
    }
    json!({ "RemediationConfigurations": [configuration] })
}

/// A static-valued remediation parameter carrying `value`.
fn static_value(value: &str) -> Value {
    json!({ "StaticValue": { "Values": [value] } })
}

#[tokio::test]
async fn a_manual_remediation_configuration_needs_no_role_parameter() {
    // Rows differ along the `Parameters` the manual configuration carries: none at all,
    // the two an SSM notification document takes, a dynamic (resource) value, a
    // parameter map at the published maximum size (`RemediationParameters`, 0..25), and
    // an empty map.
    let many: Value = Value::Object(
        (0..25)
            .map(|index| (format!("Parameter{index}"), static_value("value")))
            .collect(),
    );
    let cases: [(&str, Option<Value>); 5] = [
        ("no Parameters at all", None),
        (
            "the parameters of a notification document",
            Some(json!({
                "Message": static_value("the resource is not compliant"),
                "TopicArn": static_value("arn:aws:sns:us-east-1:000000000000:config-alerts"),
            })),
        ),
        (
            "a dynamic resource value",
            Some(json!({ "BucketName": { "ResourceValue": { "Value": "RESOURCE_ID" } } })),
        ),
        ("a parameter map at the published maximum", Some(many)),
        ("an empty parameter map", Some(json!({}))),
    ];

    for (case, parameters) in cases {
        let emulator = Emulator::new();
        let rule = "manual-remediation";
        with_recorder_and_channel(&emulator).await;
        emulator.ok("PutConfigRule", &managed_rule(rule)).await;

        let answered = emulator
            .ok(
                "PutRemediationConfigurations",
                &remediation_configuration(rule, false, parameters.clone()),
            )
            .await;
        assert_eq!(
            answered["FailedBatches"],
            json!([]),
            "case {case:?}: a manual remediation needs no role parameter, got {answered}"
        );

        // Read back: the configuration the rule now holds is the one that was written.
        let described = emulator
            .ok(
                "DescribeRemediationConfigurations",
                &json!({ "ConfigRuleNames": [rule] }),
            )
            .await;
        let configurations = described["RemediationConfigurations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected RemediationConfigurations"));
        assert_eq!(
            configurations.len(),
            1,
            "case {case:?}: the rule must hold the configuration, got {described}"
        );
        assert_eq!(
            (
                configurations[0]["TargetType"].as_str(),
                configurations[0]["TargetId"].as_str(),
                configurations[0]["Automatic"].as_bool(),
            ),
            (Some(SSM_DOCUMENT), Some("AWS-PublishSNSNotification"), Some(false)),
            "case {case:?}: expected the configuration as written, got {described}"
        );
    }

    // An AUTOMATIC remediation still needs the role it runs as, which is the one
    // requirement the documentation states for this API.
    let emulator = Emulator::new();
    let rule = "automatic-remediation";
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule(rule)).await;
    let (status, code) = emulator
        .refused(
            "PutRemediationConfigurations",
            &remediation_configuration(rule, true, None),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidParameterValueException"),
        "an automatic remediation with no AutomationAssumeRole must be refused, got {status} \
         with {code}"
    );
}

/// The `PutRemediationExceptions` body exempting `resource_id` of `resource_type` from
/// `rule`.
fn remediation_exception(rule: &str, resource_type: &str, resource_id: &str) -> Value {
    json!({
        "ConfigRuleName": rule,
        "ResourceKeys": [{ "ResourceType": resource_type, "ResourceId": resource_id }],
        "Message": "waiting on the owning team",
    })
}

#[tokio::test]
async fn a_remediation_exception_needs_the_rules_remediation_configuration() {
    // Rows differ along what the account holds for the rule the request names: nothing
    // at all, the rule with no remediation configuration, the rule whose remediation
    // configuration was deleted, and a rule whose name differs from the one that holds a
    // configuration (in case, and by surrounding space).
    let cases: [(&str, bool, bool, &str); 5] = [
        ("a rule nothing created", false, false, "no-such-rule"),
        ("a rule with no remediation configuration", true, false, "unremediated"),
        (
            "a rule whose remediation configuration was deleted",
            true,
            true,
            "unremediated",
        ),
        ("another case of a remediated rule's name", true, false, "Unremediated"),
        ("a remediated rule's name with trailing space", true, false, "unremediated "),
    ];

    for (case, create_rule, delete_configuration, named) in cases {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        if create_rule {
            emulator
                .ok("PutConfigRule", &managed_rule("unremediated"))
                .await;
        }
        if delete_configuration {
            emulator
                .ok(
                    "PutRemediationConfigurations",
                    &remediation_configuration("unremediated", false, None),
                )
                .await;
            emulator
                .ok(
                    "DeleteRemediationConfiguration",
                    &json!({ "ConfigRuleName": "unremediated" }),
                )
                .await;
        }

        let (status, code, message) = refusal(
            &emulator,
            "PutRemediationExceptions",
            &remediation_exception(named, "AWS::EC2::Volume", "vol-0001"),
        )
        .await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "InvalidParameterValueException",
                format!("No RemediationConfiguration found for {named}").as_str(),
            ),
            "case {case:?}: expected the remediation-configuration precondition, got {status} \
             with {code} and {message:?}"
        );
    }

    // Once the rule HOLDS a remediation configuration, the same request is accepted and
    // the exception is read back — for a resource the rule FOUND non-compliant, which is
    // what a put resolves its keys against.
    let emulator = Emulator::new();
    let rule = "remediated";
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule(rule)).await;
    with_verdicts(
        &emulator,
        rule,
        "NON_COMPLIANT",
        &[("AWS::EC2::Volume".to_string(), "vol-0001".to_string())],
    );
    emulator
        .ok(
            "PutRemediationConfigurations",
            &remediation_configuration(rule, false, None),
        )
        .await;
    let answered = emulator
        .ok(
            "PutRemediationExceptions",
            &remediation_exception(rule, "AWS::EC2::Volume", "vol-0001"),
        )
        .await;
    assert_eq!(
        answered["FailedBatches"],
        json!([]),
        "a rule holding a remediation configuration takes the exception, got {answered}"
    );
    let described = emulator
        .ok(
            "DescribeRemediationExceptions",
            &json!({ "ConfigRuleName": rule }),
        )
        .await;
    assert_eq!(
        described["RemediationExceptions"]
            .as_array()
            .map(|exceptions| exceptions.len()),
        Some(1),
        "the exception the put accepted must be read back, got {described}"
    );
}

/// The ARN `DescribeConfigRules` reports for the rule `name`.
async fn rule_arn(emulator: &Emulator, name: &str) -> String {
    let described = emulator
        .ok("DescribeConfigRules", &json!({ "ConfigRuleNames": [name] }))
        .await;
    described["ConfigRules"][0]["ConfigRuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the rule must carry a ConfigRuleArn, got {described}"))
        .to_string()
}

/// The `Tags` member `ListTagsForResource` reports for `arn`, as key/value pairs.
async fn listed_tags(case: &str, emulator: &Emulator, arn: &str) -> Vec<(String, String)> {
    let body = emulator
        .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    body["Tags"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListTagsForResource must always report Tags, got {body}")
        })
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn list_tags_for_resource_always_reports_the_tags_member() {
    // Rows differ along how many tags the resource carries when it is read: none at all
    // (never tagged), one, several, and none again after every key is untagged — which is
    // the read that confirms an untag took effect.
    let cases: [(&str, usize, bool); 6] = [
        ("never tagged", 0, false),
        ("one tag", 1, false),
        ("three tags", 3, false),
        ("one tag, then untagged", 1, true),
        ("three tags, then untagged", 3, true),
        ("fifty tags, the published maximum, then untagged", 50, true),
    ];

    for (case, written, untag) in cases {
        let emulator = Emulator::new();
        let rule = "tagged-rule";
        with_recorder_and_channel(&emulator).await;
        emulator.ok("PutConfigRule", &managed_rule(rule)).await;
        let arn = rule_arn(&emulator, rule).await;

        let tags: Vec<(String, String)> = (0..written)
            .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
            .collect();
        if !tags.is_empty() {
            emulator
                .ok(
                    "TagResource",
                    &json!({
                        "ResourceArn": arn,
                        "Tags": tags
                            .iter()
                            .map(|(key, value)| json!({ "Key": key, "Value": value }))
                            .collect::<Vec<Value>>(),
                    }),
                )
                .await;
        }
        if untag {
            emulator
                .ok(
                    "UntagResource",
                    &json!({
                        "ResourceArn": arn,
                        "TagKeys": tags.iter().map(|(key, _)| key.clone()).collect::<Vec<String>>(),
                    }),
                )
                .await;
        }

        let expected: Vec<(String, String)> = if untag { Vec::new() } else { tags };
        let reported = listed_tags(case, &emulator, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the tags {expected:?} reported, got {reported:?}"
        );
    }
}
