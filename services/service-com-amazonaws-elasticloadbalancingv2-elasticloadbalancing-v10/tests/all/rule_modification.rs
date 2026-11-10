// SPDX-License-Identifier: CC0-1.0

//! Replacing the properties of a listener RULE, driven over the wire exactly as a client
//! drives it: added with `CreateRule`, changed with `ModifyRule` and read back with
//! `DescribeRules`.
//!
//! Three rules are asserted here, each over a SET of inputs rather than over the one call
//! a recording happens to carry:
//!
//! * `ModifyRule` replaces exactly the properties the request names on the rule its
//!   `RuleArn` names and leaves every property it does not name holding the value it had —
//!   the rule it answers with is the rule a later read reports, and no other rule of the
//!   listener moves.
//! * A refused modification changes NOTHING: the rule reads back byte for byte as it was,
//!   whether the refusal was the rule not existing, the rule being the one its listener
//!   mints with itself, an action forwarding to a target group that does not exist, or a
//!   request whose members cannot hold together.
//! * Which load balancers a target group reports follows the rule's actions, so a
//!   modification that stops forwarding to one drops that association and one that starts
//!   forwarding adds it.
//!
//! The rows differ along what those rules generalise over: which member the request names
//! (actions, conditions, transforms, the reset, several at once, none at all), the kind of
//! condition and action replacing the stored one, how many target groups an action
//! forwards to, and which clause a refused request broke.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, forward_to, host_header, host_header_rewrite,
    rules_of, url_rewrite, Form,
};
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p, Reply,
};
use crate::rule_deletion_and_condition_value_spellings::{default_rule_arn, describe_rule};
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `RuleNotFoundException`, which `ModifyRule` models
/// (`smithy_query(query="operation",operation="ModifyRule")`: `awsQueryError` code
/// `RuleNotFound`).
const RULE_NOT_FOUND: &str = "RuleNotFound";

/// The awsQuery error code of `OperationNotPermittedException`, another error `ModifyRule`
/// models.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The awsQuery error code of `TargetGroupNotFoundException`, another error `ModifyRule`
/// models.
const TARGET_GROUP_NOT_FOUND: &str = "TargetGroupNotFound";

/// The error code this service answers an input its own documented rules refuse with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The host header the rule every row starts from matches on.
const BASE_HOST: &str = "base.example.com";

/// One row of the modification table: the request fields `ModifyRule` carries, and what
/// the stored rule must then report and no longer report.
struct ModifyRow {
    case: &'static str,
    /// The priority the rule is created at, which no modification ever changes:
    /// `ModifyRuleInput` has no priority member at all, so the rule keeps the one
    /// `CreateRule` placed it at.
    priority: i32,
    fields: Form,
    /// Values the rule must report after the modification.
    reports: Vec<String>,
    /// Values the rule must NOT report after it.
    omits: Vec<String>,
    /// Whether the rule still carries any transform. `Rule$Transforms` is reported on
    /// every rule, so what a modification changes is whether that list has anything in
    /// it, never whether the member is there.
    transforms: bool,
}

/// One row of the refusal table: the request fields, the expected status and error code.
struct RefusalRow {
    case: &'static str,
    /// Built from the rule's own ARN, so a row can address the rule under test, the rule
    /// its listener mints with itself, or an ARN naming no rule at all.
    fields: Form,
    status: u16,
    code: &'static str,
}

/// Send one `ModifyRule` request: the rule, and `fields`.
async fn modify_rule(router: &MultiServiceRouter, rule_arn: &str, fields: &[(String, String)]) -> Reply {
    let mut form = action("ModifyRule");
    form.push(p("RuleArn", rule_arn));
    form.extend(fields.iter().cloned());
    call(router, &form).await
}

/// The one rule a reply reports, as XML.
fn single_rule(case: &str, reply: &Reply) -> String {
    let rules = members(&reply.body, "Rules");
    assert_eq!(
        rules.len(),
        1,
        "case {case}: expected the one rule the call addressed, got {}",
        reply.body
    );
    rules[0].clone()
}

/// The rule `arn` names, as a read reports it.
async fn stored_rule(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let reply = describe_rule(router, arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: reading the rule back expected 200, got {} {}",
        reply.status, reply.body
    );
    single_rule(case, &reply)
}

/// The fields every rule under test is created with: one host header condition, one
/// forwarding action and one transform — so a modification naming any one of the three
/// can be checked against the two it left alone.
fn base_rule(target_group: &str) -> Form {
    [
        host_header(BASE_HOST),
        forward_to(target_group),
        host_header_rewrite(1, "^(.*)\\.example\\.com$", "$1.internal"),
    ]
    .concat()
}

/// The modification table: one row per member (or combination of members) a request can
/// name, each asserted against the members it leaves alone.
fn modify_table(group_a: &str, group_b: &str) -> Vec<ModifyRow> {
    vec![
        ModifyRow {
            case: "a request naming no property at all leaves the rule as it was",
            priority: 1,
            fields: Vec::new(),
            reports: vec![
                BASE_HOST.to_string(),
                group_a.to_string(),
                "host-header-rewrite".to_string(),
            ],
            omits: vec![group_b.to_string()],
            transforms: true,
        },
        ModifyRow {
            case: "the actions alone are replaced, forwarding to another target group",
            priority: 2,
            fields: forward_to(group_b),
            reports: vec![
                BASE_HOST.to_string(),
                group_b.to_string(),
                "host-header-rewrite".to_string(),
            ],
            omits: vec![group_a.to_string()],
            transforms: true,
        },
        ModifyRow {
            case: "the conditions alone are replaced, by a condition of another field",
            priority: 3,
            fields: vec![
                p("Conditions.member.1.Field", "path-pattern"),
                p("Conditions.member.1.PathPatternConfig.Values.member.1", "/v2/*"),
            ],
            reports: vec![
                "path-pattern".to_string(),
                "/v2/*".to_string(),
                group_a.to_string(),
                "host-header-rewrite".to_string(),
            ],
            omits: vec![BASE_HOST.to_string(), group_b.to_string()],
            transforms: true,
        },
        ModifyRow {
            case: "both lists at once: a weighted forward across two target groups",
            priority: 4,
            fields: vec![
                p("Conditions.member.1.Field", "http-header"),
                p("Conditions.member.1.HttpHeaderConfig.HttpHeaderName", "X-Cohort"),
                p("Conditions.member.1.HttpHeaderConfig.Values.member.1", "canary"),
                p("Actions.member.1.Type", "forward"),
                p(
                    "Actions.member.1.ForwardConfig.TargetGroups.member.1.TargetGroupArn",
                    group_a,
                ),
                p("Actions.member.1.ForwardConfig.TargetGroups.member.1.Weight", "90"),
                p(
                    "Actions.member.1.ForwardConfig.TargetGroups.member.2.TargetGroupArn",
                    group_b,
                ),
                p("Actions.member.1.ForwardConfig.TargetGroups.member.2.Weight", "10"),
            ],
            reports: vec![
                "X-Cohort".to_string(),
                "canary".to_string(),
                group_a.to_string(),
                group_b.to_string(),
                "90".to_string(),
                "10".to_string(),
            ],
            omits: vec![BASE_HOST.to_string()],
            transforms: true,
        },
        ModifyRow {
            case: "the transforms alone are replaced, by a transform of the other type",
            priority: 5,
            fields: url_rewrite(1, "^/old/(.*)$", "/new/$1"),
            reports: vec![
                BASE_HOST.to_string(),
                group_a.to_string(),
                "url-rewrite".to_string(),
                "/new/$1".to_string(),
            ],
            omits: vec!["host-header-rewrite".to_string()],
            transforms: true,
        },
        ModifyRow {
            case: "ResetTransforms removes every transform and nothing else",
            priority: 6,
            fields: vec![p("ResetTransforms", "true")],
            reports: vec![BASE_HOST.to_string(), group_a.to_string()],
            omits: vec!["host-header-rewrite".to_string()],
            transforms: false,
        },
        ModifyRow {
            case: "an explicit ResetTransforms of false keeps the transforms",
            priority: 7,
            fields: vec![p("ResetTransforms", "false")],
            reports: vec![
                BASE_HOST.to_string(),
                group_a.to_string(),
                "host-header-rewrite".to_string(),
            ],
            omits: Vec::new(),
            transforms: true,
        },
        ModifyRow {
            case: "a fixed response replacing a forward stops forwarding at all",
            priority: 8,
            fields: vec![
                p("Actions.member.1.Type", "fixed-response"),
                p("Actions.member.1.FixedResponseConfig.StatusCode", "503"),
                p("Actions.member.1.FixedResponseConfig.ContentType", "text/plain"),
            ],
            reports: vec![
                BASE_HOST.to_string(),
                "fixed-response".to_string(),
                "503".to_string(),
            ],
            omits: vec![group_a.to_string(), group_b.to_string()],
            transforms: true,
        },
    ]
}

/// The rule: `ModifyRule` replaces exactly the properties the request names, leaves every
/// other property of the rule as it was, and persists what it answered.
#[tokio::test]
async fn modifying_a_rule_replaces_only_the_properties_the_request_names() {
    let (router, _engine) = emulator();
    let load_balancer = create_load_balancer(&router, "rule-modify-alb").await;
    let group_a = create_http_target_group(&router, "rule-modify-tg-a").await;
    let group_b = create_http_target_group(&router, "rule-modify-tg-b").await;
    let listener = create_listener(&router, &load_balancer, &group_a, 80).await;

    let table = modify_table(&group_a, &group_b);
    let mut arns: Vec<String> = Vec::new();
    for row in &table {
        arns.push(
            add_rule(
                &router,
                row.case,
                &listener,
                row.priority,
                &base_rule(&group_a),
            )
            .await,
        );
    }

    // Every rule of the listener, as it reads before any modification: a modification of
    // one rule must leave each of the others exactly here.
    let mut before: Vec<String> = Vec::new();
    for (row, arn) in table.iter().zip(&arns) {
        before.push(stored_rule(&router, row.case, arn).await);
    }

    for (index, row) in table.iter().enumerate() {
        let arn = &arns[index];
        let reply = modify_rule(&router, arn, &row.fields).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected 200, got {} {}",
            row.case, reply.status, reply.body
        );
        let answered = single_rule(row.case, &reply);
        assert_eq!(
            (
                first_text(&answered, "RuleArn").as_deref(),
                first_text(&answered, "Priority").as_deref(),
                first_text(&answered, "IsDefault").as_deref(),
            ),
            (
                Some(arn.as_str()),
                Some(row.priority.to_string().as_str()),
                Some("false"),
            ),
            "case {}: the modified rule keeps its own ARN and priority and is not the \
             default, got {answered}",
            row.case
        );

        // What the write answered is what the store holds: the read is the assertion, the
        // write's own reply is only checked to agree with it.
        let stored = stored_rule(&router, row.case, arn).await;
        for value in &row.reports {
            assert!(
                stored.contains(value.as_str()) && answered.contains(value.as_str()),
                "case {}: the modified rule must report {value}, got {stored} (the write \
                 answered {answered})",
                row.case
            );
        }
        for value in &row.omits {
            assert!(
                !stored.contains(value.as_str()) && !answered.contains(value.as_str()),
                "case {}: the modified rule must no longer report {value}, got {stored} \
                 (the write answered {answered})",
                row.case
            );
        }
        assert!(
            stored.contains("<Transforms") && answered.contains("<Transforms"),
            "case {}: every reported rule carries the Transforms member, got {stored} (the \
             write answered {answered})",
            row.case
        );
        assert_eq!(
            first_text(&stored, "Transforms").is_some(),
            row.transforms,
            "case {}: expected transforms reported: {}, got {stored}",
            row.case,
            row.transforms
        );

        // Only the rule the request addressed moved: every other rule of the listener
        // still reads exactly as it did before this modification.
        for (other, (other_row, other_arn)) in table.iter().zip(&arns).enumerate() {
            if other == index {
                continue;
            }
            let now = stored_rule(&router, other_row.case, other_arn).await;
            assert_eq!(
                now, before[other],
                "case {}: modifying another rule must leave this one untouched",
                other_row.case
            );
        }
        before[index] = stored;
    }

    // The listener still carries exactly the rules it was given, at the priorities it was
    // given them at: no modification adds, removes or re-orders a rule.
    let expected: Vec<(String, String)> = table
        .iter()
        .map(|row| (row.priority.to_string(), "false".to_string()))
        .chain(std::iter::once(("default".to_string(), "true".to_string())))
        .collect();
    assert_eq!(
        rules_of(&router, "after every modification", &listener).await,
        expected,
        "case after every modification: the listener reports its rules in priority order \
         with the default last"
    );
}

/// The refusal table, built against the rule under test, the listener's default rule and
/// an ARN naming no rule.
fn refusal_table(rule_arn: &str, default_arn: &str, unknown_arn: &str, absent_group: &str) -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "an ARN naming no rule",
            fields: vec![p("RuleArn", unknown_arn), p("ResetTransforms", "true")],
            status: 400,
            code: RULE_NOT_FOUND,
        },
        RefusalRow {
            case: "a value that is no rule ARN at all",
            fields: vec![p("RuleArn", "not-an-arn"), p("ResetTransforms", "true")],
            status: 400,
            code: RULE_NOT_FOUND,
        },
        RefusalRow {
            case: "the rule a listener mints with itself",
            fields: [
                vec![p("RuleArn", default_arn)],
                vec![
                    p("Actions.member.1.Type", "fixed-response"),
                    p("Actions.member.1.FixedResponseConfig.StatusCode", "404"),
                ],
            ]
            .concat(),
            status: 400,
            code: OPERATION_NOT_PERMITTED,
        },
        RefusalRow {
            case: "an action forwarding to a target group that does not exist",
            fields: [vec![p("RuleArn", rule_arn)], forward_to(absent_group)].concat(),
            status: 400,
            code: TARGET_GROUP_NOT_FOUND,
        },
        RefusalRow {
            case: "the transforms to apply named together with the reset",
            fields: [
                vec![p("RuleArn", rule_arn), p("ResetTransforms", "true")],
                url_rewrite(1, "^/a$", "/b"),
            ]
            .concat(),
            status: 400,
            code: VALIDATION_ERROR,
        },
        RefusalRow {
            case: "two transforms of one type",
            fields: [
                vec![p("RuleArn", rule_arn)],
                url_rewrite(1, "^/a$", "/b"),
                url_rewrite(2, "^/c$", "/d"),
            ]
            .concat(),
            status: 400,
            code: VALIDATION_ERROR,
        },
        RefusalRow {
            // A list longer than the transform TYPES there are must repeat one of them, so a
            // third transform is refused inside the bounded walk
            // (`state::check_rule_transforms`) however long the list is. The sentence it
            // earns is asserted after this table.
            case: "one more transform than a rule carries",
            fields: [
                vec![p("RuleArn", rule_arn)],
                host_header_rewrite(1, "^a\\.example\\.com$", "b.example.com"),
                url_rewrite(2, "^/a$", "/b"),
                url_rewrite(3, "^/c$", "/d"),
            ]
            .concat(),
            status: 400,
            code: VALIDATION_ERROR,
        },
        RefusalRow {
            case: "a condition field this service does not match on",
            fields: vec![
                p("RuleArn", rule_arn),
                p("Conditions.member.1.Field", "http-method"),
                p("Conditions.member.1.Values.member.1", "POST"),
            ],
            status: 400,
            code: VALIDATION_ERROR,
        },
        RefusalRow {
            case: "two conditions of a field a rule carries at most one of",
            fields: vec![
                p("RuleArn", rule_arn),
                p("Conditions.member.1.Field", "host-header"),
                p("Conditions.member.1.Values.member.1", "one.example.com"),
                p("Conditions.member.2.Field", "host-header"),
                p("Conditions.member.2.Values.member.1", "two.example.com"),
            ],
            status: 400,
            code: VALIDATION_ERROR,
        },
        RefusalRow {
            case: "one more condition value than the published bound allows",
            fields: vec![
                p("RuleArn", rule_arn),
                p("Conditions.member.1.Field", "host-header"),
                // `condition-values-per-alb-rule` is 5, so six values is one past it.
                p("Conditions.member.1.Values.member.1", "a.example.com"),
                p("Conditions.member.1.Values.member.2", "b.example.com"),
                p("Conditions.member.1.Values.member.3", "c.example.com"),
                p("Conditions.member.1.Values.member.4", "d.example.com"),
                p("Conditions.member.1.Values.member.5", "e.example.com"),
                p("Conditions.member.1.Values.member.6", "f.example.com"),
            ],
            status: 400,
            code: VALIDATION_ERROR,
        },
    ]
}

/// The rule: a refused modification persists nothing — the rule it addressed, and the rule
/// its listener mints with itself, read back exactly as they did before the refusal.
#[tokio::test]
async fn a_refused_modification_leaves_every_rule_as_it_was() {
    let (router, _engine) = emulator();
    let load_balancer = create_load_balancer(&router, "rule-modify-refuse-alb").await;
    let group = create_http_target_group(&router, "rule-modify-refuse-tg").await;
    let listener = create_listener(&router, &load_balancer, &group, 80).await;
    let rule = add_rule(
        &router,
        "the rule every refusal addresses",
        &listener,
        10,
        &base_rule(&group),
    )
    .await;

    let default_rule = default_rule_arn(&router, &listener).await;
    // Well formed, naming this listener, and holding an id this store never minted.
    let unknown = format!(
        "{}/0123456789abcdef",
        listener.replacen(":listener/", ":listener-rule/", 1)
    );
    // A target group ARN of the same account that no target group of this store answers to.
    let absent_group = format!("{group}-absent");

    let before = stored_rule(&router, "before every refusal", &rule).await;
    let default_before = stored_rule(&router, "the default rule before", &default_rule).await;

    for row in refusal_table(&rule, &default_rule, &unknown, &absent_group) {
        let mut form = action("ModifyRule");
        form.extend(row.fields.iter().cloned());
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (row.status, row.code),
            "case {}: expected {} {}, got {} {}",
            row.case,
            row.status,
            row.code,
            reply.status,
            reply.body
        );

        assert_eq!(
            stored_rule(&router, row.case, &rule).await,
            before,
            "case {}: a refused modification changes nothing about the rule",
            row.case
        );
        assert_eq!(
            stored_rule(&router, row.case, &default_rule).await,
            default_before,
            "case {}: a refused modification changes nothing about the default rule",
            row.case
        );
    }

    // A transform list past the two types `TransformTypeEnum` declares repeats one of them,
    // and the clause that refuses it names the type the caller has to remove — read here
    // because the table above asserts the refusal CLASS and this is the sentence inside it.
    let case = "the sentence a transform list past the bound earns";
    let over_the_bound = modify_rule(
        &router,
        &rule,
        &[
            host_header_rewrite(1, "^a\\.example\\.com$", "b.example.com"),
            url_rewrite(2, "^/a$", "/b"),
            url_rewrite(3, "^/c$", "/d"),
        ]
        .concat(),
    )
    .await;
    let message = over_the_bound.text("Message").unwrap_or_default();
    // The quoted type as the wire carries it: the awsQuery serializer escapes the
    // apostrophes of the clause's own `'<type>'` quoting.
    assert_eq!(
        message, "A rule carries at most one &apos;url-rewrite&apos; transform",
        "case {case:?}: expected the clause to name the repeated type, got {message:?} in {}",
        over_the_bound.body
    );
}

/// The load balancers one target group reports.
async fn load_balancers_of(router: &MultiServiceRouter, case: &str, group: &str) -> Vec<String> {
    let mut form = action("DescribeTargetGroups");
    form.push(p("TargetGroupArns.member.1", group));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected 200, got {} {}",
        reply.status, reply.body
    );
    let groups = members(&reply.body, "TargetGroups");
    assert_eq!(
        groups.len(),
        1,
        "case {case}: expected the one target group asked for, got {}",
        reply.body
    );
    members(&groups[0], "LoadBalancerArns")
}

/// The rule: a load balancer routes to every target group an action of one of its rules
/// forwards to, so the association a target group reports follows a modification of those
/// actions in both directions.
#[tokio::test]
async fn a_modification_moves_the_target_group_association_with_the_actions() {
    let (router, _engine) = emulator();
    let load_balancer = create_load_balancer(&router, "rule-modify-assoc-alb").await;
    let listener_group = create_http_target_group(&router, "rule-modify-assoc-listener").await;
    let first = create_http_target_group(&router, "rule-modify-assoc-first").await;
    let second = create_http_target_group(&router, "rule-modify-assoc-second").await;
    let listener = create_listener(&router, &load_balancer, &listener_group, 80).await;
    let rule = add_rule(
        &router,
        "a rule forwarding to the first target group",
        &listener,
        10,
        &[host_header(BASE_HOST), forward_to(&first)].concat(),
    )
    .await;

    // Each step names the target groups the rule's actions forward to afterwards, so both
    // directions are walked: one group, the other group, both at once, and neither.
    let steps: Vec<(&str, Form, Vec<&String>)> = vec![
        (
            "forwarding to the second target group only",
            forward_to(&second),
            vec![&second],
        ),
        (
            "forwarding to both target groups at once",
            vec![
                p("Actions.member.1.Type", "forward"),
                p(
                    "Actions.member.1.ForwardConfig.TargetGroups.member.1.TargetGroupArn",
                    &first,
                ),
                p("Actions.member.1.ForwardConfig.TargetGroups.member.1.Weight", "50"),
                p(
                    "Actions.member.1.ForwardConfig.TargetGroups.member.2.TargetGroupArn",
                    &second,
                ),
                p("Actions.member.1.ForwardConfig.TargetGroups.member.2.Weight", "50"),
            ],
            vec![&first, &second],
        ),
        (
            "answering a fixed response, forwarding to neither",
            vec![
                p("Actions.member.1.Type", "fixed-response"),
                p("Actions.member.1.FixedResponseConfig.StatusCode", "503"),
                p("Actions.member.1.FixedResponseConfig.ContentType", "text/plain"),
            ],
            Vec::new(),
        ),
        (
            "forwarding to the first target group again",
            forward_to(&first),
            vec![&first],
        ),
    ];

    assert_eq!(
        load_balancers_of(&router, "before any modification", &first).await,
        vec![load_balancer.clone()],
        "case before any modification: the group the rule forwards to reports its load \
         balancer"
    );

    for (case, fields, forwarded) in steps {
        let reply = modify_rule(&router, &rule, &fields).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        for group in [&first, &second] {
            let reported = load_balancers_of(&router, case, group).await;
            let expected: Vec<String> = if forwarded.contains(&group) {
                vec![load_balancer.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                reported, expected,
                "case {case}: expected {expected:?} for {group}, got {reported:?}"
            );
        }
        // The target group the LISTENER's own default action forwards to is not a rule's
        // to release.
        assert_eq!(
            load_balancers_of(&router, case, &listener_group).await,
            vec![load_balancer.clone()],
            "case {case}: the listener's own target group keeps its association"
        );
    }
}
