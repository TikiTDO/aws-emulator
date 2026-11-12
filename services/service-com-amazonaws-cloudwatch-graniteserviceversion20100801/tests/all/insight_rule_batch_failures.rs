// SPDX-License-Identifier: CC0-1.0

//! Cover for HOW the batch Contributor Insights rule operations CLASSIFY a per-entry failure:
//! which `ExceptionType`, `FailureCode` and `FailureDescription` one entry of `Failures`
//! carries.
//!
//! Two rules are under test, one per kind of batch:
//!
//! 1. **A batch rule operation that MOVES a rule's state reports a rule name it holds no rule
//!    for as one `PartialFailure` of the resource-not-found class, coded by the member at fault
//!    — the rule name — and described as the sentence "Rule " + the name the request carried +
//!    " does not exist."** "An array listing the rules that could not be disabled."
//!    (`DisableInsightRulesOutput$Failures`), "If the operation could not process one of the
//!    rules, the following data is returned for each of those rules." (`PartialFailure`) — a
//!    name that is well formed but addresses no rule is a resource that is not there, not a
//!    value the request had no business sending, and the two are what a batch caller branches
//!    on. The DELETE of such a name reports NOTHING: the state it asks the rule to reach is
//!    being gone, which a name no rule answers to has already reached.
//!
//! 2. **A `ManagedRule` entry whose `ResourceARN` addresses no resource is reported about that
//!    ARN — the parameter-fault class, still keyed by the entry's template name, described
//!    "Invalid ResourceArn." — before the entry's template is resolved at all, and enables
//!    nothing.** "The ARN of an Amazon Web Services resource that has managed Contributor
//!    Insights rules." (`ManagedRule$ResourceARN`) — an ARN whose resource type its own
//!    service namespace does not address names nothing a managed rule could collect data for,
//!    whatever its template name would have resolved to.
//!
//! Rows differ along what each rule generalises over: which batch operation is called, the
//! length, case and punctuation of the rule name, how many names one request carries and how
//! held, unknown and repeated names are mixed; and for the managed rules, the form of the ARN
//! (which resource type, the `type/id` and `type:id` forms, which vendor, an absent Region or
//! account), whether its template is published, and how many entries one request carries.

use crate::harness::{build_router, build_router_with_engine, ok, publish_managed_rule_template};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The batch Contributor Insights rule operations that MOVE a rule's state, and so report back
/// a name they could not write the asked-for state on: "The rules can be enabled, disabled, and
/// deleted using EnableInsightRules, DisableInsightRules, and DeleteInsightRules."
/// (`PutManagedInsightRules`)
///
/// The DELETE is not one of them: the state it asks for is the rule being gone, which a name no
/// rule answers to already is, so it reports no failure for one — asserted below.
const STATE_BATCH_OPERATIONS: [&str; 2] = ["DisableInsightRules", "EnableInsightRules"];

/// A rule definition in the Contributor Insights rule syntax, as `PutInsightRule` takes it.
fn definition() -> String {
    json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": ["API-Gateway-Access-Logs"],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    })
    .to_string()
}

/// The `Failures` entries an answer carries, in the order it carried them.
fn failures(body: &Value) -> Vec<Value> {
    body.get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The entry a batch rule operation reports for a name it holds no rule for.
fn rule_not_found(rule_name: &str) -> Value {
    json!({
        "FailureResource": rule_name,
        "ExceptionType": "ResourceNotFoundException",
        "FailureCode": "RULE_NAME",
        "FailureDescription": format!("Rule {rule_name} does not exist."),
    })
}

/// The entry `PutManagedInsightRules` reports for an entry whose `ResourceARN` addresses no
/// resource.
fn invalid_resource_arn(template_name: &str) -> Value {
    json!({
        "FailureResource": template_name,
        "ExceptionType": "InvalidParameterException",
        "FailureCode": "TEMPLATE_NAME",
        "FailureDescription": "Invalid ResourceArn.",
    })
}

/// The entry it reports for an entry whose template the catalog does not publish.
fn template_not_found(template_name: &str) -> Value {
    json!({
        "FailureResource": template_name,
        "ExceptionType": "ResourceNotFoundException",
        "FailureCode": "TEMPLATE_NAME",
        "FailureDescription": "Could not find template with the specified templateName.",
    })
}

/// One `PutManagedInsightRules` entry.
fn managed_rule(template_name: &str, resource_arn: &str) -> Value {
    json!({"TemplateName": template_name, "ResourceARN": resource_arn})
}

/// The names of the Contributor Insights rules the account holds.
async fn rule_names(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "DescribeInsightRules", &json!({})).await;
    body.get("InsightRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| {
            rule.get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The managed-rule descriptions `ListManagedInsightRules` reports for `resource_arn`.
async fn listed(router: &MultiServiceRouter, resource_arn: &str) -> Vec<Value> {
    let body = ok(
        router,
        "ListManagedInsightRules",
        &json!({"ResourceARN": resource_arn}),
    )
    .await;
    body.get("ManagedRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One row of the unknown-rule-name table: the rules that exist before the call and the names
/// the request carries.
struct NameRow {
    /// What this row is, named in every assertion message.
    case: &'static str,
    /// The rule names created before the call.
    held: Vec<String>,
    /// The rule names the request carries, in request order.
    named: Vec<String>,
}

#[tokio::test]
async fn a_name_no_rule_answers_to_is_reported_as_the_rule_that_is_not_there() {
    // Rows span the name's length (one character and the modelled 128-character maximum), the
    // characters `^[\x20-\x7E]+$` allows, a name differing from a held rule only in case, a
    // request mixing held and unknown names, one repeating an unknown name, and one naming only
    // rules that are held.
    let long_name = "n".repeat(128);
    let rows: Vec<NameRow> = vec![
        NameRow {
            case: "one unknown name, nothing held",
            held: Vec::new(),
            named: vec!["cwprobe-managed-rule-missing".to_string()],
        },
        NameRow {
            case: "a one-character unknown name",
            held: Vec::new(),
            named: vec!["x".to_string()],
        },
        NameRow {
            case: "a 128-character unknown name",
            held: Vec::new(),
            named: vec![long_name.clone()],
        },
        NameRow {
            case: "an unknown name carrying the spaces and punctuation the pattern allows",
            held: Vec::new(),
            named: vec!["My rule (v2) / high-cardinality".to_string()],
        },
        NameRow {
            case: "an unknown name differing from a held rule only in case",
            held: vec!["Orders".to_string()],
            named: vec!["orders".to_string()],
        },
        NameRow {
            case: "a held name between two unknown ones",
            held: vec!["held".to_string()],
            named: vec![
                "ghost one".to_string(),
                "held".to_string(),
                "ghost two".to_string(),
            ],
        },
        NameRow {
            case: "the same unknown name twice",
            held: vec!["held".to_string()],
            named: vec!["ghost".to_string(), "ghost".to_string()],
        },
        NameRow {
            case: "every named rule held",
            held: vec!["alpha".to_string(), "bravo".to_string()],
            named: vec!["alpha".to_string(), "bravo".to_string()],
        },
    ];

    for operation in STATE_BATCH_OPERATIONS {
        for row in &rows {
            let case = format!("{operation} / {}", row.case);
            let router = build_router();
            for name in &row.held {
                ok(
                    &router,
                    "PutInsightRule",
                    &json!({"RuleName": name, "RuleDefinition": definition()}),
                )
                .await;
            }

            let answered = ok(&router, operation, &json!({"RuleNames": row.named})).await;

            // One entry per named rule the service holds none of, in the order the request
            // named them, each classified as the rule that is not there and described by the
            // name the request carried.
            let expected: Vec<Value> = row
                .named
                .iter()
                .filter(|name| !row.held.contains(name))
                .map(|name| rule_not_found(name))
                .collect();
            let reported = failures(&answered);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the not-found entry per unknown name, got {reported:?}"
            );
            // "This array is empty if the API operation was successful for all the rules
            // specified in the request." (`PartialFailure`) — empty, not absent.
            assert!(
                answered.get("Failures").is_some_and(Value::is_array),
                "case {case:?}: expected a Failures array on the answer, got {answered}"
            );

            // The DELETE of the very same names reports NO entry for one it holds no rule
            // for: the rule the request asks to be gone is already gone.
            let case = format!("DeleteInsightRules / {}", row.case);
            let deleted = ok(&router, "DeleteInsightRules", &json!({"RuleNames": row.named}))
                .await;
            assert_eq!(
                failures(&deleted),
                Vec::<Value>::new(),
                "case {case:?}: expected no failure entry for a name that names no rule, got \
                 {deleted}"
            );
            assert!(
                deleted.get("Failures").is_some_and(Value::is_array),
                "case {case:?}: expected an empty Failures array on the answer, got {deleted}"
            );
        }
    }
}

/// Resource ARNs that are well formed as ARNs and address no resource: every one is in this
/// service's own namespace, where the resource types it addresses are `alarm`,
/// `alarm-mute-rule`, `dashboard`, `dataset`, `insight-rule` and `metric-stream`
/// (`TagResourceInput$ResourceARN`, `GetDatasetInput$DatasetIdentifier`, `PutAlarmMuteRule`),
/// each carrying the id of a resource of that type. Rows differ along the form the resource
/// field takes and how it misses that: an unaddressed type in either ARN form, an addressed
/// type written differently, a bare type, and an addressed type with no id at all.
const UNADDRESSED_RESOURCES: [(&str, &str); 6] = [
    (
        "a slash-separated resource of a type this service does not address",
        "arn:aws:cloudwatch:us-east-1:123456789012:not-a-real-resource/does-not-exist",
    ),
    (
        "a colon-separated resource of a type this service does not address",
        "arn:aws:cloudwatch:us-east-1:123456789012:widget:orders-latency",
    ),
    (
        "an addressed type written without its hyphen",
        "arn:aws:cloudwatch:us-east-1:123456789012:insightrule/orders",
    ),
    (
        "an addressed type written in another case",
        "arn:aws:cloudwatch:us-east-1:123456789012:Alarm:orders-latency",
    ),
    (
        "a resource field that is a bare unaddressed type",
        "arn:aws:cloudwatch:us-east-1:123456789012:widget",
    ),
    (
        "an addressed type carrying no resource id",
        "arn:aws:cloudwatch:us-east-1:123456789012:alarm",
    ),
];

/// Resource ARNs that DO name a resource: one per resource type this service addresses, in
/// both ARN forms and with the Region or account a resource of that type omits, plus the
/// resources of other vendors, whose own namespaces say what they address.
const ADDRESSED_RESOURCES: [(&str, &str); 9] = [
    (
        "an alarm",
        "arn:aws:cloudwatch:us-east-1:123456789012:alarm:orders-latency",
    ),
    (
        "an alarm mute rule",
        "arn:aws:cloudwatch:us-east-1:123456789012:alarm-mute-rule:weekend",
    ),
    (
        "a dashboard, whose ARN carries no Region",
        "arn:aws:cloudwatch::123456789012:dashboard/orders",
    ),
    (
        "a dataset",
        "arn:aws:cloudwatch:us-east-1:123456789012:dataset/default",
    ),
    (
        "a Contributor Insights rule",
        "arn:aws:cloudwatch:us-east-1:123456789012:insight-rule/orders",
    ),
    (
        "a metric stream",
        "arn:aws:cloudwatch:us-east-1:123456789012:metric-stream/orders",
    ),
    (
        "a DynamoDB table, another vendor's resource",
        "arn:aws:dynamodb:us-east-1:123456789012:table/Orders",
    ),
    (
        "an S3 bucket, which carries neither Region nor account",
        "arn:aws:s3:::forged-access-logs",
    ),
    (
        "a Lambda function in another partition",
        "arn:aws-cn:lambda:cn-north-1:123456789012:function:checkout",
    ),
];

#[tokio::test]
async fn a_managed_rule_entry_whose_resource_arn_addresses_no_resource_is_reported_about_the_arn() {
    // The same template name is used for every row and resolves to no template, so the only
    // thing that differs between a row here and its partner in the test below is the ARN.
    let template = "no-such-template-xyz";

    for (case, resource_arn) in UNADDRESSED_RESOURCES {
        // Judged on the ARN whether or not a template answers for it: the first row of each
        // pair publishes nothing, the second publishes the very template the entry names for
        // the very resource it names, which is what shows the ARN is read FIRST.
        for published in [false, true] {
            let case = if published {
                format!("{case} / its template published")
            } else {
                format!("{case} / nothing published")
            };
            let (router, engine) = build_router_with_engine();
            if published {
                publish_managed_rule_template(&engine, template, resource_arn);
            }

            let put = ok(
                &router,
                "PutManagedInsightRules",
                &json!({"ManagedRules": [managed_rule(template, resource_arn)]}),
            )
            .await;
            let reported = failures(&put);
            assert_eq!(
                reported,
                vec![invalid_resource_arn(template)],
                "case {case:?}: expected exactly one entry reporting the ARN, got {reported:?}"
            );

            // Nothing is enabled: the account holds no Contributor Insights rule at all.
            let held = rule_names(&router).await;
            assert!(
                held.is_empty(),
                "case {case:?}: an entry reported back must enable no rule, got {held:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_entry_whose_resource_arn_addresses_a_resource_is_judged_by_its_template() {
    let template = "Orders-Insights";

    for (case, resource_arn) in ADDRESSED_RESOURCES {
        // With nothing published the entry is reported about its TEMPLATE, not its ARN.
        let router = build_router();
        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule(template, resource_arn)]}),
        )
        .await;
        let reported = failures(&put);
        assert_eq!(
            reported,
            vec![template_not_found(template)],
            "case {case:?}: expected the entry judged by its template, got {reported:?}"
        );

        // With the template published for that resource the entry is enabled and reads back
        // through the catalog reader and the rule reader.
        let (router, engine) = build_router_with_engine();
        publish_managed_rule_template(&engine, template, resource_arn);
        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule(template, resource_arn)]}),
        )
        .await;
        let reported = failures(&put);
        assert!(
            reported.is_empty(),
            "case {case:?}: a published template for a named resource enables with no failure, \
             got {reported:?}"
        );
        let described = listed(&router, resource_arn).await;
        let enabled: Vec<(&str, &str)> = described
            .iter()
            .map(|description| {
                (
                    description
                        .get("TemplateName")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    description
                        .pointer("/RuleState/State")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            enabled,
            vec![(template, "ENABLED")],
            "case {case:?}: expected the managed rule reported as enabled, got {described:?}"
        );
        let held = rule_names(&router).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected the one enabled managed rule to be described, got {held:?}"
        );
    }
}

#[tokio::test]
async fn each_entry_of_one_request_is_judged_on_its_own_resource_and_template() {
    // One request carrying, in order: an entry whose ARN addresses nothing though its template
    // is published, an entry that is enabled, and an entry whose ARN is fine and whose template
    // is not published.
    let table = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";
    let unaddressed = "arn:aws:cloudwatch:us-east-1:123456789012:not-a-real-resource/does-not-exist";
    let (router, engine) = build_router_with_engine();
    publish_managed_rule_template(&engine, "Orders-Insights", table);
    publish_managed_rule_template(&engine, "Orders-Insights", unaddressed);

    let put = ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [
            managed_rule("Orders-Insights", unaddressed),
            managed_rule("Orders-Insights", table),
            managed_rule("missing-last", table),
        ]}),
    )
    .await;
    let reported = failures(&put);
    assert_eq!(
        reported,
        vec![
            invalid_resource_arn("Orders-Insights"),
            template_not_found("missing-last"),
        ],
        "expected one entry per unusable entry, each about its own fault and in request order, \
         got {reported:?}"
    );

    // Exactly the one entry that named a resource and a published template is enabled.
    let held = rule_names(&router).await;
    assert_eq!(
        held.len(),
        1,
        "expected only the usable entry enabled, got {held:?}"
    );
    let described = listed(&router, table).await;
    let enabled: Vec<&str> = described
        .iter()
        .filter(|description| {
            description.pointer("/RuleState/State").and_then(Value::as_str) == Some("ENABLED")
        })
        .map(|description| {
            description
                .get("TemplateName")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        enabled,
        vec!["Orders-Insights"],
        "expected the table's published template enabled, got {described:?}"
    );
}
