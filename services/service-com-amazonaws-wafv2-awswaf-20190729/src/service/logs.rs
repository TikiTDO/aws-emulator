// SPDX-License-Identifier: CC0-1.0

//! The LOG RECORD a web ACL delivers about one web request it decided, and the write of
//! that record into the destination its logging configuration names.
//!
//! A `LoggingConfiguration` is not a document that only gets read back: it "defines an
//! association between logging destinations and a web ACL resource, for logging from
//! WAF" (`LoggingConfiguration`), and `PutLoggingConfiguration` "enables the specified
//! LoggingConfiguration, to start logging from a web ACL". So while a web ACL carries
//! one, every request it evaluates leaves ONE record at the destination that
//! configuration names — a request it blocked as much as one it let through, because the
//! record is what the configuration exists to produce.
//!
//! This module is that record, computed in the ONE place a web ACL decides a request
//! ([`super::edge`]) once that evaluation has settled, so no protected resource's service
//! can serve traffic that is enforced but never logged. The protected resource's own
//! service hands the record over to its destination ([`deliver`]) when it answers the
//! request, because the in-process call that write IS belongs to the operation serving it.
//!
//! # What the configuration decides about a record
//!
//! * `LoggingFilter` decides WHETHER the record is kept at all — "filtering that
//!   specifies which web requests are kept in the logs and which are dropped" — by the
//!   action WAF applied and by the labels the evaluation added ([`kept`]).
//! * `RedactedFields` decides what the kept record SHOWS: "the parts of the request that
//!   you want to keep out of the logs. For example, if you redact the `SingleHeader`
//!   field, the `HEADER` field in the logs will be `REDACTED`" — so the component each
//!   entry names carries [`REDACTED`] in place of what the client sent, and only the
//!   four components the member admits for redaction can be named
//!   (`UriPath`, `QueryString`, `SingleHeader` and `Method`).
//! * `LogDestinationConfigs` decides WHERE it goes. A destination naming a CloudWatch
//!   Logs log group has the record written into that group through the one vended-logs
//!   write this tree holds ([`::service_common::cloudwatch_logs::delivered`]), so a
//!   delivered record and the readers of that group cannot disagree. A destination of
//!   another kind (an Amazon S3 bucket, a Firehose delivery stream) resolves no group, so
//!   the record is reported as one this path does not deliver rather than written
//!   somewhere the configuration did not name.
//!
//! A record therefore reports the request as WAF inspected it, with the components the
//! configuration named kept out of it: a caller that wants a component out of its logs —
//! the header a bearer token travels in, say — names that component in `RedactedFields`,
//! which is the one instruction WAF takes about what a record may show. Nothing is
//! written outside the log group the configuration names.

use super::edge::{EdgeDecision, InspectedRequest};
use crate::ops::types::{FieldToMatch, LoggingConfiguration, LoggingFilter, Rule};
use crate::state::WebAclEntry;

/// What a redacted component shows instead of what the client sent, from
/// `LoggingConfiguration$RedactedFields`: "the `HEADER` field in the logs will be
/// `REDACTED`".
pub const REDACTED: &str = "REDACTED";

/// The `formatVersion` of the record this module writes: WAF's first, and so far only,
/// published log format.
const FORMAT_VERSION: i64 = 1;

/// The `terminatingRuleId` of a request no rule terminated, which the web ACL's
/// `DefaultAction` decided — "the action to perform if none of the Rules contained in the
/// WebACL match" (`WebACL$DefaultAction`).
const DEFAULT_ACTION_RULE_ID: &str = "Default_Action";

/// The `terminatingRuleType` of a rule that inspects the request itself: a web ACL rule
/// with no rate limit and no rule group behind it, and the web ACL's own
/// `DefaultAction`.
const REGULAR_RULE_TYPE: &str = "REGULAR";

/// The `terminatingRuleType` of a `RateBasedStatement` rule.
const RATE_BASED_RULE_TYPE: &str = "RATE_BASED";

/// The `terminatingRuleType` of a rule that runs a customer-owned rule group
/// (`RuleGroupReferenceStatement`).
const RULE_GROUP_RULE_TYPE: &str = "GROUP";

/// The `terminatingRuleType` of a rule that runs a managed rule group
/// (`ManagedRuleGroupStatement`).
const MANAGED_RULE_GROUP_RULE_TYPE: &str = "MANAGED_RULE_GROUP";

/// The `FilterBehavior` that keeps a record (`smithy_query(query="shape",
/// shape="FilterBehavior")`: `KEEP` or `DROP`).
const KEEP_BEHAVIOR: &str = "KEEP";

/// The `FilterRequirement` that needs every condition of a filter met; the other,
/// `MEETS_ANY`, needs one.
const MEETS_ALL_REQUIREMENT: &str = "MEETS_ALL";

/// What a record names as its `httpSourceName` when nothing says which kind of resource
/// took the request: the value AWS writes a log field it has no value for as.
const UNKNOWN_SOURCE: &str = "-";

/// The `httpSourceName` of each kind of protected resource, keyed by the service
/// namespace (the ARN's own third field) of the resource the association names.
///
/// The ARN forms are the ones `AssociateWebACLRequest$ResourceArn` publishes — an
/// Application Load Balancer, an API Gateway REST API stage, an AppSync GraphQL API, an
/// Amazon Cognito user pool, an App Runner service, a Verified Access instance and an
/// Amplify application — so WHICH kind of resource a request arrived at is read off the
/// association rather than told to this module by the resource's own service.
const HTTP_SOURCE_NAMES: &[(&str, &str)] = &[
    ("elasticloadbalancing", "ALB"),
    ("apigateway", "APIGW"),
    ("appsync", "APPSYNC"),
    ("cognito-idp", "COGNITOIDP"),
    ("apprunner", "APPRUNNER"),
    ("ec2", "VERIFIED_ACCESS"),
    ("amplify", "AMPLIFY"),
];

/// The `httpSourceName` of a request a CLOUDFRONT-scoped web ACL decided: such a web ACL
/// is not associated through `AssociateWebACL` ("for Amazon CloudFront, don't use
/// AssociateWebACL") — the distribution names it in its own configuration — so the scope
/// is what says the request arrived at a distribution.
const CLOUDFRONT_SOURCE: &str = "CF";

/// The field of an ARN that names the service namespace, in
/// `arn:<partition>:<service>:<region>:<account>:<resource>`.
const ARN_SERVICE_FIELD: usize = 2;

/// The field of an ARN that names the Region.
const ARN_REGION_FIELD: usize = 3;

/// The record one decided request logs, and where it goes.
///
/// Computed by the evaluation that decided the request and carried out on
/// [`EdgeDecision::log_record`], so the service that serves the request delivers exactly
/// what WAF composed for it ([`deliver`]) rather than composing a second record of its
/// own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedRecord {
    /// The Region the destination's own ARN names, which is where the group lives.
    pub region: String,
    /// The log group the destination ARN names.
    pub log_group: String,
    /// The stream within it ([`log_stream_name`]).
    pub log_stream: String,
    /// The record itself: one line of JSON, as WAF writes a log record.
    pub record: String,
}

/// What the web ACL `entry` logs about the request it has just decided, or `None` when it
/// logs nothing about it.
///
/// `configuration` is the web ACL's customer-owned logging configuration, `None` when it
/// carries none — in which case the request is decided and nothing is logged, which is the
/// difference between a web ACL that has been enabled for logging and one that has not.
/// Nothing is logged either when the configuration's own `LoggingFilter` drops the record,
/// or when its destination names no log group. `protected_resource` is the ARN the
/// association was made by, which says what kind of resource took the request; a
/// CLOUDFRONT-scoped web ACL is reached through no association and so names none.
pub(super) fn logged(
    entry: &WebAclEntry,
    configuration: Option<&LoggingConfiguration>,
    protected_resource: Option<&str>,
    request: &InspectedRequest,
    decision: &EdgeDecision,
    at_millis: i64,
) -> Option<LoggedRecord> {
    let configuration = configuration?;
    let action = decision.action.action_value();
    if !kept(configuration.logging_filter.as_ref(), action, &decision.labels) {
        return None;
    }
    let Some(destination) = configuration
        .log_destination_configs
        .iter()
        .find_map(|configured| destination(configured, entry))
    else {
        ::tracing::info!(
            web_acl = %entry.web_acl.arn,
            "this web acl's logging destination is not a CloudWatch Logs log group, so the \
             record of this request is not delivered"
        );
        return None;
    };
    Some(LoggedRecord {
        region: destination.region,
        log_group: destination.log_group,
        log_stream: destination.log_stream,
        record: record(
            entry,
            configuration,
            protected_resource,
            request,
            decision,
            at_millis,
        ),
    })
}

/// Deliver `logged` to the destination it names.
///
/// Called by the protected resource's own service once it has answered the request the
/// record is about: the write is an in-process call into the CloudWatch Logs this emulator
/// mounts, so it belongs to the operation that is serving the request rather than to the
/// evaluation, which holds no such context. Call it with NO state guard held — it awaits.
///
/// Nothing here can fail the request: it has already been answered, so a record that could
/// not be delivered is traced by the write and nothing else.
pub async fn deliver(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    logged: &LoggedRecord,
) -> bool {
    ::service_common::cloudwatch_logs::delivered(
        ctx,
        &logged.region,
        &logged.log_group,
        &logged.log_stream,
        ::std::slice::from_ref(&logged.record),
    )
    .await
}

/// Where one record is delivered: the log group its destination names and the stream
/// within it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Destination {
    /// The Region the destination's own ARN names, which is where the group lives.
    region: String,
    /// The log group the destination ARN names.
    log_group: String,
    /// The stream within it ([`log_stream_name`]).
    log_stream: String,
}

/// Where `configured` delivers, or `None` when it names no CloudWatch Logs log group.
///
/// A log group is named by its ARN, in either of the two forms a client reads one in —
/// the group on its own and the group with everything under it — which is the ONE reading
/// of such an ARN this tree holds
/// ([`::service_common::cloudwatch_logs::log_group_of_arn`]).
fn destination(configured: &str, entry: &WebAclEntry) -> Option<Destination> {
    let log_group = ::service_common::cloudwatch_logs::log_group_of_arn(configured)?;
    Some(Destination {
        region: configured
            .split(':')
            .nth(ARN_REGION_FIELD)
            .unwrap_or_default()
            .to_string(),
        log_group: log_group.to_string(),
        log_stream: log_stream_name(entry),
    })
}

/// The stream a web ACL's records are written into.
///
/// WAF publishes no name for it, so the name is composed of what identifies the
/// DELIVERING web ACL — the Region its series are reported for and its own name and id,
/// the values the record's `webaclId` names it by — so two web ACLs pointed at one group
/// accumulate their records in two streams while a reader of the group (which reads
/// across its streams) finds both.
fn log_stream_name(entry: &WebAclEntry) -> String {
    format!(
        "{}_{}_{}",
        super::metrics::reported_region(&entry.scope, &entry.web_acl.arn),
        entry.web_acl.name,
        entry.web_acl.id
    )
}

/// Whether a record about a request WAF applied `action` to, carrying `labels`, is KEPT.
///
/// A configuration with no `LoggingFilter` keeps every record: filtering is what the
/// member adds. With one, the filters apply in the order they were given and the first
/// one the record satisfies decides — "how to handle logs that satisfy the filter's
/// conditions and requirement" (`Filter$Behavior`) — and a record satisfying none is
/// handled by the `DefaultBehavior`, "default handling for logs that don't match any of
/// the specified filtering conditions".
fn kept(filter: Option<&LoggingFilter>, action: &str, labels: &[String]) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    for one in &filter.filters {
        let met: Vec<bool> = one
            .conditions
            .iter()
            .map(|condition| {
                let by_action = condition
                    .action_condition
                    .as_ref()
                    .is_some_and(|wanted| wanted.action == action);
                let by_label = condition
                    .label_name_condition
                    .as_ref()
                    .is_some_and(|wanted| labels.contains(&wanted.label_name));
                by_action || by_label
            })
            .collect();
        let satisfied = if one.requirement == MEETS_ALL_REQUIREMENT {
            !met.is_empty() && met.iter().all(|one| *one)
        } else {
            met.iter().any(|one| *one)
        };
        if satisfied {
            return one.behavior == KEEP_BEHAVIOR;
        }
    }
    filter.default_behavior == KEEP_BEHAVIOR
}

/// The record one decided request is logged as.
///
/// The fields WAF publishes for a log record, each computed from the evaluation that has
/// just happened: when it happened, the web ACL that decided, the rule that terminated
/// the evaluation and its kind, the action applied, where the request arrived, the labels
/// the evaluation added, and the request itself as the configuration's `RedactedFields`
/// leaves it. A field whose value this evaluation holds none of — a request's country,
/// which needs a geolocation database, or a TLS fingerprint, which never reaches a server
/// — is left OUT of the document rather than written as a value nothing produced.
fn record(
    entry: &WebAclEntry,
    configuration: &LoggingConfiguration,
    protected_resource: Option<&str>,
    request: &InspectedRequest,
    decision: &EdgeDecision,
    at_millis: i64,
) -> String {
    let redacted = configuration.redacted_fields.as_deref().unwrap_or_default();
    let mut document = ::serde_json::json!({
        "timestamp": at_millis,
        "formatVersion": FORMAT_VERSION,
        "webaclId": entry.web_acl.arn,
        "terminatingRuleId": decision
            .rule_name
            .clone()
            .unwrap_or_else(|| DEFAULT_ACTION_RULE_ID.to_string()),
        "terminatingRuleType": terminating_rule_type(entry, decision),
        "action": decision.action.action_value(),
        "httpSourceName": http_source_name(entry, protected_resource),
        "httpSourceId": http_source_id(protected_resource),
        "labels": decision
            .labels
            .iter()
            .map(|name| ::serde_json::json!({ "name": name }))
            .collect::<Vec<::serde_json::Value>>(),
        "httpRequest": http_request(request, redacted),
    });
    // The headers a `CustomRequestHandling` on the way inserted into the request, which
    // WAF reports for the request it let through carrying them.
    if !decision.inserted_headers.is_empty() {
        document["requestHeadersInserted"] = ::serde_json::json!(decision
            .inserted_headers
            .iter()
            .map(|(name, value)| ::serde_json::json!({ "name": name, "value": value }))
            .collect::<Vec<::serde_json::Value>>());
    }
    // The status a terminating action's own `CustomResponse` answered with: WAF served
    // that response, so WAF is what knows the code the client was sent.
    if let Some(custom) = decision.custom_response.as_ref() {
        document["responseCodeSent"] = ::serde_json::json!(custom.status);
    }
    document.to_string()
}

/// The `httpRequest` of a record: the request as it arrived, with every component the
/// configuration's `RedactedFields` names showing [`REDACTED`] instead.
///
/// Redaction applies to the component the entry names and to nothing else, so a
/// `SingleHeader` entry redacts the value of that one header (compared
/// case-insensitively, as HTTP header names are) and leaves every other header as the
/// client sent it.
///
/// The headers a record carries are the ones the host service would have forwarded to WAF
/// — "a maximum of 200 headers and at most 8 KB of header contents"
/// (`Headers$OversizeHandling`), the same two budgets a stored sample of the request is
/// held under ([`super::edge::MAX_INSPECTED_HEADERS`],
/// [`super::edge::HEADER_INSPECTION_LIMIT`]) — so the walk stops at whichever of them it
/// reaches first and a request choosing its own header lengths does not decide how large
/// one record is.
fn http_request(request: &InspectedRequest, redacted: &[FieldToMatch]) -> ::serde_json::Value {
    let redacts_uri = redacted.iter().any(|field| field.uri_path.is_some());
    let redacts_query = redacted.iter().any(|field| field.query_string.is_some());
    let redacts_method = redacted.iter().any(|field| field.method.is_some());
    let mut held = 0_usize;
    let mut headers: Vec<::serde_json::Value> = Vec::new();
    for (name, value) in request
        .headers
        .iter()
        .take(super::edge::MAX_INSPECTED_HEADERS)
    {
        held = held.saturating_add(name.len()).saturating_add(value.len());
        if held > super::edge::HEADER_INSPECTION_LIMIT {
            break;
        }
        let redacts_header = redacted.iter().any(|field| {
            field
                .single_header
                .as_ref()
                .is_some_and(|header| header.name.eq_ignore_ascii_case(name))
        });
        let shown = if redacts_header { REDACTED } else { value };
        headers.push(::serde_json::json!({ "name": name, "value": shown }));
    }
    ::serde_json::json!({
        "clientIp": request.client_ip,
        "httpMethod": if redacts_method { REDACTED } else { &request.method },
        "uri": if redacts_uri { REDACTED } else { &request.uri_path },
        "args": if redacts_query { REDACTED } else { &request.query_string },
        "headers": headers,
    })
}

/// The `terminatingRuleType` of the rule that ended the evaluation.
///
/// The kind is a property of the rule's own `Statement`: a rate limit over the requests
/// it matched (`RateBasedStatement`), the rules of a managed rule group
/// (`ManagedRuleGroupStatement`) or of a customer-owned one
/// (`RuleGroupReferenceStatement`), and otherwise an inspection of the request itself.
/// A request no rule terminated was decided by the web ACL's `DefaultAction`, which
/// inspects nothing.
fn terminating_rule_type(entry: &WebAclEntry, decision: &EdgeDecision) -> &'static str {
    let Some(name) = decision.rule_name.as_deref() else {
        return REGULAR_RULE_TYPE;
    };
    let decided = entry
        .web_acl
        .rules
        .iter()
        .flatten()
        .find(|rule: &&Rule| rule.name == name);
    let Some(rule) = decided else {
        return REGULAR_RULE_TYPE;
    };
    if rule.statement.rate_based_statement.is_some() {
        return RATE_BASED_RULE_TYPE;
    }
    if rule.statement.managed_rule_group_statement.is_some() {
        return MANAGED_RULE_GROUP_RULE_TYPE;
    }
    if rule.statement.rule_group_reference_statement.is_some() {
        return RULE_GROUP_RULE_TYPE;
    }
    REGULAR_RULE_TYPE
}

/// The `httpSourceName` of a record: which kind of resource the request arrived at.
///
/// Read off the ARN the web ACL was associated with ([`HTTP_SOURCE_NAMES`]); a
/// CLOUDFRONT-scoped web ACL is associated by no such ARN and decides requests that
/// arrived at a distribution ([`CLOUDFRONT_SOURCE`]).
fn http_source_name(entry: &WebAclEntry, protected_resource: Option<&str>) -> &'static str {
    if entry.scope == crate::state::SCOPE_CLOUDFRONT {
        return CLOUDFRONT_SOURCE;
    }
    let namespace = protected_resource
        .and_then(|arn| arn.split(':').nth(ARN_SERVICE_FIELD))
        .unwrap_or_default();
    HTTP_SOURCE_NAMES
        .iter()
        .find(|(vendor, _)| *vendor == namespace)
        .map(|(_, source)| *source)
        .unwrap_or(UNKNOWN_SOURCE)
}

/// The `httpSourceId` of a record: WHICH resource of that kind the request arrived at,
/// as the association's own ARN names it (its resource field).
fn http_source_id(protected_resource: Option<&str>) -> String {
    protected_resource
        .and_then(|arn| {
            let mut fields = arn.splitn(6, ':');
            fields.nth(5).filter(|resource| !resource.is_empty())
        })
        .unwrap_or(UNKNOWN_SOURCE)
        .to_string()
}

#[cfg(test)]
mod tests {
    //! Which records a `LoggingFilter` keeps, what a `RedactedFields` entry hides, and
    //! which source a protected resource's ARN names. The delivery itself is pinned end
    //! to end over the wire in `tests/all/web_acl_logging_delivery.rs`.

    use super::*;
    use crate::ops::types::{
        ActionCondition, Condition, Filter, LabelNameCondition, Method, QueryString, SingleHeader,
        UriPath,
    };

    /// One row of the filter table: the case, the configuration's filter, the action WAF
    /// applied, the labels the evaluation added, and whether the record is kept.
    type FilterRow = (&'static str, Option<LoggingFilter>, &'static str, Vec<String>, bool);

    /// One row of the redaction table: the case, the configuration's redacted fields, and
    /// the method, uri, args and authorization header the record shows.
    type RedactionRow = (
        &'static str,
        Vec<FieldToMatch>,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    );

    /// A filter of one action condition.
    fn on_action(behavior: &str, action: &str) -> Filter {
        Filter {
            behavior: behavior.to_string(),
            requirement: "MEETS_ANY".to_string(),
            conditions: vec![Condition {
                action_condition: Some(ActionCondition {
                    action: action.to_string(),
                }),
                label_name_condition: None,
            }],
        }
    }

    /// A filter needing both an action and a label.
    fn on_action_and_label(behavior: &str, action: &str, label: &str) -> Filter {
        Filter {
            behavior: behavior.to_string(),
            requirement: MEETS_ALL_REQUIREMENT.to_string(),
            conditions: vec![
                Condition {
                    action_condition: Some(ActionCondition {
                        action: action.to_string(),
                    }),
                    label_name_condition: None,
                },
                Condition {
                    action_condition: None,
                    label_name_condition: Some(LabelNameCondition {
                        label_name: label.to_string(),
                    }),
                },
            ],
        }
    }

    #[test]
    fn a_logging_filter_decides_which_records_are_kept() {
        let label = "awswaf:000000000000:webacl:guard:probed";
        // (case, the filter, the action applied, the labels carried, kept)
        let rows: Vec<FilterRow> = vec![
            ("no filter at all keeps every record", None, "BLOCK", vec![], true),
            (
                "a KEEP filter on the action that was applied",
                Some(LoggingFilter {
                    filters: vec![on_action(KEEP_BEHAVIOR, "BLOCK")],
                    default_behavior: "DROP".to_string(),
                }),
                "BLOCK",
                vec![],
                true,
            ),
            (
                "the same filter, for a request of another action",
                Some(LoggingFilter {
                    filters: vec![on_action(KEEP_BEHAVIOR, "BLOCK")],
                    default_behavior: "DROP".to_string(),
                }),
                "ALLOW",
                vec![],
                false,
            ),
            (
                "a DROP filter on the action that was applied",
                Some(LoggingFilter {
                    filters: vec![on_action("DROP", "ALLOW")],
                    default_behavior: KEEP_BEHAVIOR.to_string(),
                }),
                "ALLOW",
                vec![],
                false,
            ),
            (
                "a filter matching nothing, kept by the default behavior",
                Some(LoggingFilter {
                    filters: vec![on_action("DROP", "CAPTCHA")],
                    default_behavior: KEEP_BEHAVIOR.to_string(),
                }),
                "BLOCK",
                vec![],
                true,
            ),
            (
                "MEETS_ALL with both conditions met",
                Some(LoggingFilter {
                    filters: vec![on_action_and_label(KEEP_BEHAVIOR, "BLOCK", label)],
                    default_behavior: "DROP".to_string(),
                }),
                "BLOCK",
                vec![label.to_string()],
                true,
            ),
            (
                "MEETS_ALL with only the action met",
                Some(LoggingFilter {
                    filters: vec![on_action_and_label(KEEP_BEHAVIOR, "BLOCK", label)],
                    default_behavior: "DROP".to_string(),
                }),
                "BLOCK",
                vec![],
                false,
            ),
            (
                "the first filter that is satisfied decides",
                Some(LoggingFilter {
                    filters: vec![on_action("DROP", "BLOCK"), on_action(KEEP_BEHAVIOR, "BLOCK")],
                    default_behavior: KEEP_BEHAVIOR.to_string(),
                }),
                "BLOCK",
                vec![],
                false,
            ),
        ];
        for (case, filter, action, labels, expected) in rows {
            let got = kept(filter.as_ref(), action, &labels);
            assert_eq!(
                got, expected,
                "case {case:?}: expected kept={expected}, got {got}"
            );
        }
    }

    /// The request every redaction row below is logged about.
    fn probed() -> InspectedRequest {
        InspectedRequest {
            method: "POST".to_string(),
            uri_path: "/prod/orders".to_string(),
            query_string: "id=7".to_string(),
            headers: vec![
                ("host".to_string(), "example.test".to_string()),
                ("Authorization".to_string(), "Bearer secret".to_string()),
            ],
            body: Vec::new(),
            client_ip: "198.51.100.5".to_string(),
        }
    }

    #[test]
    fn every_redactable_component_is_hidden_and_nothing_else_is() {
        let header = |name: &str| FieldToMatch {
            single_header: Some(SingleHeader {
                name: name.to_string(),
            }),
            ..FieldToMatch::default()
        };
        // One row per component `RedactedFields` admits for redaction, plus none at all
        // and every one at once. (case, the redacted fields, method, uri, args,
        // the authorization header's value)
        let rows: Vec<RedactionRow> = vec![
            (
                "nothing redacted",
                vec![],
                "POST",
                "/prod/orders",
                "id=7",
                "Bearer secret",
            ),
            (
                "the method alone",
                vec![FieldToMatch {
                    method: Some(Method {}),
                    ..FieldToMatch::default()
                }],
                REDACTED,
                "/prod/orders",
                "id=7",
                "Bearer secret",
            ),
            (
                "the uri path alone",
                vec![FieldToMatch {
                    uri_path: Some(UriPath {}),
                    ..FieldToMatch::default()
                }],
                "POST",
                REDACTED,
                "id=7",
                "Bearer secret",
            ),
            (
                "the query string alone",
                vec![FieldToMatch {
                    query_string: Some(QueryString {}),
                    ..FieldToMatch::default()
                }],
                "POST",
                "/prod/orders",
                REDACTED,
                "Bearer secret",
            ),
            (
                "one header, named in another case than it arrived in",
                vec![header("authorization")],
                "POST",
                "/prod/orders",
                "id=7",
                REDACTED,
            ),
            (
                "a header the request does not carry",
                vec![header("cookie")],
                "POST",
                "/prod/orders",
                "id=7",
                "Bearer secret",
            ),
            (
                "every redactable component at once",
                vec![
                    FieldToMatch {
                        method: Some(Method {}),
                        ..FieldToMatch::default()
                    },
                    FieldToMatch {
                        uri_path: Some(UriPath {}),
                        ..FieldToMatch::default()
                    },
                    FieldToMatch {
                        query_string: Some(QueryString {}),
                        ..FieldToMatch::default()
                    },
                    header("Authorization"),
                ],
                REDACTED,
                REDACTED,
                REDACTED,
                REDACTED,
            ),
        ];
        for (case, redacted, method, uri, args, authorization) in rows {
            let logged = http_request(&probed(), &redacted);
            assert_eq!(
                logged["httpMethod"],
                ::serde_json::json!(method),
                "case {case:?}: expected httpMethod {method:?}, got {logged}"
            );
            assert_eq!(
                logged["uri"],
                ::serde_json::json!(uri),
                "case {case:?}: expected uri {uri:?}, got {logged}"
            );
            assert_eq!(
                logged["args"],
                ::serde_json::json!(args),
                "case {case:?}: expected args {args:?}, got {logged}"
            );
            let headers = logged["headers"].as_array().expect("the headers").clone();
            let shown = headers
                .iter()
                .find(|entry| entry["name"] == ::serde_json::json!("Authorization"))
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                shown["value"],
                ::serde_json::json!(authorization),
                "case {case:?}: expected the authorization header {authorization:?}, got \
                 {logged}"
            );
            // Redaction applies only to the component named: the host header, which no
            // row redacts, always carries what the client sent.
            let host = headers
                .iter()
                .find(|entry| entry["name"] == ::serde_json::json!("host"))
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                host["value"],
                ::serde_json::json!("example.test"),
                "case {case:?}: a header no entry names must carry what the client sent, \
                 got {logged}"
            );
        }
    }

    #[test]
    fn the_source_of_a_record_is_the_kind_of_resource_the_association_names() {
        // One row per ARN form `AssociateWebACLRequest$ResourceArn` publishes, plus a
        // CLOUDFRONT-scoped web ACL (associated by no ARN) and a resource of a kind
        // nothing publishes a source name for.
        let rows: [(&str, &str, Option<&str>, &str); 7] = [
            (
                "an API Gateway REST API stage",
                crate::state::SCOPE_REGIONAL,
                Some("arn:aws:apigateway:us-west-2::/restapis/a1b2c3d4e5/stages/prod"),
                "APIGW",
            ),
            (
                "an Application Load Balancer",
                crate::state::SCOPE_REGIONAL,
                Some(
                    "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/web/50dc",
                ),
                "ALB",
            ),
            (
                "an Amazon Cognito user pool",
                crate::state::SCOPE_REGIONAL,
                Some("arn:aws:cognito-idp:us-west-2:000000000000:userpool/us-west-2_abc"),
                "COGNITOIDP",
            ),
            (
                "an AppSync GraphQL API",
                crate::state::SCOPE_REGIONAL,
                Some("arn:aws:appsync:us-west-2:000000000000:apis/abc123"),
                "APPSYNC",
            ),
            (
                "a Verified Access instance",
                crate::state::SCOPE_REGIONAL,
                Some("arn:aws:ec2:us-west-2:000000000000:verified-access-instance/vai-1"),
                "VERIFIED_ACCESS",
            ),
            (
                "a CLOUDFRONT-scoped web ACL, which no association names",
                crate::state::SCOPE_CLOUDFRONT,
                None,
                CLOUDFRONT_SOURCE,
            ),
            (
                "a resource of a kind no source name is published for",
                crate::state::SCOPE_REGIONAL,
                Some("arn:aws:bedrock-agentcore:us-west-2:000000000000:gateway/gw-1"),
                UNKNOWN_SOURCE,
            ),
        ];
        for (case, scope, resource, expected) in rows {
            let entry = WebAclEntry {
                scope: scope.to_string(),
                web_acl: crate::ops::types::WebACL {
                    name: "guard".to_string(),
                    id: "1111".to_string(),
                    arn: "arn:aws:wafv2:us-west-2:000000000000:regional/webacl/guard/1111"
                        .to_string(),
                    ..crate::ops::types::WebACL::default()
                },
                lock_token: String::new(),
                tags: Vec::new(),
            };
            let got = http_source_name(&entry, resource);
            assert_eq!(
                got, expected,
                "case {case:?}: expected the source {expected:?}, got {got:?}"
            );
        }
    }
}
