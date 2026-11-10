// SPDX-License-Identifier: CC0-1.0

//! What a proactive resource evaluation's SUMMARY reports: the status its execution
//! has reached, and the compliance a finished execution places the resource at.
//!
//! `StartResourceEvaluation` persists the execution ([`super::ResourceEvaluationEntry`])
//! and `ListResourceEvaluations` reports it; this module holds the part
//! `GetResourceEvaluationSummary` derives on top of it, so the read computes
//! everything from stored state and the clock and writes nothing.
//!
//! What this module deliberately does NOT model: the retention window after which
//! Config stops reporting a finished resource evaluation. No sentence of the model
//! or of the operation's documentation states that duration, and an uncited one
//! would be invented, so an execution is only ever dropped by the store's own bound
//! ([`super::RESOURCE_EVALUATIONS_KEPT`]). Expiring a summary is left to an issue
//! that can cite the window.

use std::time::{Duration, SystemTime};

use super::{
    ConfigRuleEntry, ResourceEvaluationEntry, COMPLIANCE_TYPE_COMPLIANT,
    COMPLIANCE_TYPE_INSUFFICIENT_DATA, COMPLIANCE_TYPE_NON_COMPLIANT,
    COMPLIANCE_TYPE_NOT_APPLICABLE,
};
use crate::ops::types::{EvaluationStatus, ResourceDetails, Scope};

/// `ResourceEvaluationStatus` values
/// (`com.amazonaws.configservice#ResourceEvaluationStatus`, whose values are
/// `IN_PROGRESS`, `FAILED` and `SUCCEEDED`).
pub const RESOURCE_EVALUATION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`RESOURCE_EVALUATION_STATUS_IN_PROGRESS`].
pub const RESOURCE_EVALUATION_STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// See [`RESOURCE_EVALUATION_STATUS_IN_PROGRESS`].
pub const RESOURCE_EVALUATION_STATUS_FAILED: &str = "FAILED";

/// How long after `StartResourceEvaluation` an execution's status leaves
/// `IN_PROGRESS` for its terminal one.
///
/// The model documents no duration for a resource evaluation's execution, so this
/// span is CHOSEN: it sits inside the 250–750 ms window this project uses for a
/// transition the clock owes, which keeps the transient `IN_PROGRESS` status
/// observable to a caller that reads the summary straight after starting the
/// evaluation. Only the clock moves the status, so the read that reports it never
/// has to write.
pub const EXECUTION_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// The `EvaluationStatus` and `ComplianceType` the summary of `evaluation` reports as
/// of `now`, given the Config `rules` the account holds.
///
/// Until the clock owes the execution its terminal status it is `IN_PROGRESS` and no
/// compliance is reported: no rule has placed the resource yet. Afterwards both come
/// from [`execution_outcome`].
pub fn summary_of<'a>(
    evaluation: &ResourceEvaluationEntry,
    rules: impl Iterator<Item = &'a ConfigRuleEntry>,
    now: SystemTime,
) -> (EvaluationStatus, Option<String>) {
    if now < evaluation.started_at + EXECUTION_SETTLES_AFTER {
        return (
            EvaluationStatus {
                status: RESOURCE_EVALUATION_STATUS_IN_PROGRESS.to_string(),
                failure_reason: None,
            },
            None,
        );
    }
    // An execution runs the account's rules that evaluate in the evaluation's own
    // mode; of those, the ones whose scope covers the resource are the ones that can
    // place it. Both are needed: whether the account has ANY rule configured for the
    // mode decides whether there was an execution to run at all.
    let in_mode: Vec<&ConfigRuleEntry> = rules_evaluating_in(rules, &evaluation.evaluation_mode);
    let covering: Vec<&ConfigRuleEntry> = covering_rules(&in_mode, &evaluation.resource_details);
    let (status, failure_reason, compliance) = execution_outcome(
        !in_mode.is_empty(),
        &covering,
        &evaluation.evaluation_mode,
        &evaluation.resource_details,
    );
    (
        EvaluationStatus {
            status,
            failure_reason,
        },
        Some(compliance),
    )
}

/// The rules an execution runs: the account's Config rules that evaluate in
/// `evaluation_mode` and whose scope covers the resource being evaluated — the
/// composition of [`rules_evaluating_in`] and [`covering_rules`].
pub fn matching_rules<'a>(
    rules: impl Iterator<Item = &'a ConfigRuleEntry>,
    evaluation_mode: &str,
    details: &ResourceDetails,
) -> Vec<&'a ConfigRuleEntry> {
    covering_rules(&rules_evaluating_in(rules, evaluation_mode), details)
}

/// The account's Config rules that evaluate in `evaluation_mode` — the rule
/// CONFIGURATION an execution in that mode runs, whatever resource it is handed.
/// Empty means the account has nothing configured to evaluate in that mode.
pub fn rules_evaluating_in<'a>(
    rules: impl Iterator<Item = &'a ConfigRuleEntry>,
    evaluation_mode: &str,
) -> Vec<&'a ConfigRuleEntry> {
    rules
        .filter(|rule| {
            rule.evaluation_modes
                .iter()
                .any(|mode| mode.mode.as_deref() == Some(evaluation_mode))
        })
        .collect()
}

/// The subset of `rules` whose scope covers the resource `details` describes: the
/// rules that can place THIS resource at a verdict.
pub fn covering_rules<'a>(
    rules: &[&'a ConfigRuleEntry],
    details: &ResourceDetails,
) -> Vec<&'a ConfigRuleEntry> {
    rules
        .iter()
        .copied()
        .filter(|rule| scope_covers(rule.scope.as_ref(), details))
        .collect()
}

/// Whether a Config rule with `scope` covers the resource `details` describes.
///
/// A rule with no scope covers every resource; a scope narrows by resource type and
/// by resource id. `Scope.TagKey`/`Scope.TagValue` narrow by the TAGS of a RECORDED
/// resource, and a proactive evaluation is handed the details of a resource that need
/// not exist and carries no tags Config has recorded — so a scope that narrows by tag
/// covers no proactive execution at all, rather than being widened to every one.
fn scope_covers(scope: Option<&Scope>, details: &ResourceDetails) -> bool {
    let Some(scope) = scope else {
        return true;
    };
    let narrows_by_tag = [scope.tag_key.as_deref(), scope.tag_value.as_deref()]
        .iter()
        .any(|member| member.is_some_and(|value| !value.is_empty()));
    if narrows_by_tag {
        return false;
    }
    let covers_type = scope
        .compliance_resource_types
        .as_deref()
        .filter(|types| !types.is_empty())
        .is_none_or(|types| types.iter().any(|scoped| scoped == &details.resource_type));
    let covers_id = scope
        .compliance_resource_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .is_none_or(|id| id == details.resource_id);
    covers_type && covers_id
}

/// The `ComplianceType` a set of recorded evaluation results rolls up to:
/// noncompliant if any result is, compliant if at least one is and none is not, and
/// `INSUFFICIENT_DATA` when no result places the resource either way (which includes
/// an empty set).
///
/// The ONE place this precedence is written, so a rule's own roll-up
/// ([`ConfigRuleEntry::compliance`]) and an execution's ([`execution_outcome`])
/// cannot drift apart.
pub fn roll_up_compliance<'a>(results: impl Iterator<Item = &'a str>) -> &'static str {
    let mut compliant = false;
    for result in results {
        if result == COMPLIANCE_TYPE_NON_COMPLIANT {
            return COMPLIANCE_TYPE_NON_COMPLIANT;
        }
        compliant |= result == COMPLIANCE_TYPE_COMPLIANT;
    }
    if compliant {
        COMPLIANCE_TYPE_COMPLIANT
    } else {
        COMPLIANCE_TYPE_INSUFFICIENT_DATA
    }
}

/// The `ComplianceType` an EXECUTION's summary reports, given the verdicts the rules
/// covering the resource placed it at (`results`) and whether any covering rule left
/// it unplaced (`a_covering_rule_is_undecided`).
///
/// The three terminal verdicts mean different things to a client, so they are not
/// interchangeable: `NON_COMPLIANT`/`COMPLIANT` when a verdict places the resource
/// ([`roll_up_compliance`] holds that precedence), `INSUFFICIENT_DATA` when a rule
/// APPLIES to the resource but no verdict decided it, and `NOT_APPLICABLE` when no
/// rule verdict applies at all — which includes an execution no rule covers. This
/// differs from a CONFIG RULE's own roll-up on purpose: a rule with no evaluation
/// results of its own reports `INSUFFICIENT_DATA` ("If Config has no evaluation
/// results for the rule, it returns `INSUFFICIENT_DATA`",
/// `DescribeComplianceByConfigRule`), whereas a resource evaluation no rule covers has
/// nothing the resource could be judged against.
pub fn execution_compliance(results: &[&str], a_covering_rule_is_undecided: bool) -> &'static str {
    let placed = roll_up_compliance(results.iter().copied());
    if placed != COMPLIANCE_TYPE_INSUFFICIENT_DATA {
        return placed;
    }
    let undecided = a_covering_rule_is_undecided
        || results.contains(&COMPLIANCE_TYPE_INSUFFICIENT_DATA);
    if undecided {
        COMPLIANCE_TYPE_INSUFFICIENT_DATA
    } else {
        COMPLIANCE_TYPE_NOT_APPLICABLE
    }
}

/// The terminal outcome of an execution, as the status, the reason it failed and the
/// `ComplianceType` it places the resource at.
///
/// An execution runs "all of the Config rules in your account that match with the
/// specified proactive mode and resource type" against the resource details, so it
/// SUCCEEDS only when that rule configuration existed and every rule covering the
/// resource had already placed it:
///
/// * `account_evaluates_in_mode` is false — the account has no rule configured to
///   evaluate in this mode, so there was no execution to run and the evaluation
///   terminates in the modelled `FAILED` status (recorded behaviour of a bare account);
/// * a covering rule has no recorded verdict for the resource — its verdict comes from
///   EXECUTING its implementation (a managed rule's, a custom Lambda's, or a Guard
///   policy's), which this emulator has no rule engine for, so the execution ends
///   `FAILED` naming those rules instead of synthesising a verdict;
/// * otherwise it SUCCEEDS.
///
/// The compliance it reports is [`execution_compliance`] over the verdicts the
/// covering rules recorded for the resource, in every case: a failed execution still
/// says where the resource stands (nowhere, `NOT_APPLICABLE`, when no rule's verdict
/// applies to it).
pub fn execution_outcome(
    account_evaluates_in_mode: bool,
    covering: &[&ConfigRuleEntry],
    evaluation_mode: &str,
    details: &ResourceDetails,
) -> (String, Option<String>, String) {
    // A rule holds one result per (resource type, resource id, evaluation mode), so
    // the verdict this execution would have produced is the one recorded for the
    // resource it evaluates IN ITS OWN MODE.
    let recorded_for = (
        details.resource_type.clone(),
        details.resource_id.clone(),
        evaluation_mode.to_string(),
    );
    let mut results = Vec::new();
    let mut unrunnable = Vec::new();
    for rule in covering {
        match rule.evaluation_results.get(&recorded_for) {
            Some(recorded) => results.push(recorded.compliance_type.as_str()),
            None => unrunnable.push(rule.name.as_str()),
        }
    }
    let compliance = execution_compliance(&results, !unrunnable.is_empty()).to_string();
    if !unrunnable.is_empty() {
        return (
            RESOURCE_EVALUATION_STATUS_FAILED.to_string(),
            Some(unrunnable_rules_failure_reason(&unrunnable)),
            compliance,
        );
    }
    if !account_evaluates_in_mode {
        return (
            RESOURCE_EVALUATION_STATUS_FAILED.to_string(),
            Some(NO_RULE_CONFIGURATION_FAILURE_REASON.to_string()),
            compliance,
        );
    }
    (
        RESOURCE_EVALUATION_STATUS_SUCCEEDED.to_string(),
        None,
        compliance,
    )
}

/// Why an execution failed when the account had no rule configured to evaluate in its
/// mode: there was no rule configuration for Config to run the resource details
/// against, so no evaluation ran.
///
/// The sentence Config answers with, verbatim and carrying no part of the request. It
/// names the PROACTIVE mode because that is the only mode an execution runs in
/// (`StartResourceEvaluation` refuses any other), so this reason is reached only for
/// an account with no proactive rule.
pub const NO_RULE_CONFIGURATION_FAILURE_REASON: &str =
    "No Config rule is enabled in proactive mode for the given resource type.";

/// Why an execution failed when it had rule implementations to run: this emulator
/// runs no rule logic, so it says so and names the rules whose verdicts are missing
/// rather than inventing them.
fn unrunnable_rules_failure_reason(rules: &[&str]) -> String {
    format!(
        "the proactive evaluation could not be completed: this emulator does not execute the \
         evaluation logic of the Config rule(s) {}, so no verdict was produced for the resource",
        rules.join(", "),
    )
}

/// How a request that named a resource evaluation this account does not hold is
/// refused: a not-found, worded as the recorded refusals of this operation are, with
/// the `ResourceEvaluationId` the caller sent as its only request-derived part.
pub fn no_such_resource_evaluation_message(id: &str) -> String {
    format!("Provided ResourceEvaluationId {id} not found.")
}

/// How a `StartResourceEvaluation` retry that reused a `ClientToken` with different
/// parameters is refused.
///
/// One template, whose only request-derived part is the token: "If you retry a
/// successful request using the same client token, but one or more of the parameters
/// are different ... the retry fails with an IdempotentParameterMismatch error"
/// (`StartResourceEvaluationRequest$ClientToken`).
pub fn idempotent_parameter_mismatch_message(client_token: &str) -> String {
    format!(
        "The request inputs are not associated with the supplied client token \
         {client_token}. You are using the same client token with one or more different \
         parameters. Specify a new client token with the parameter changes and try again."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{EvaluationModeConfiguration, Source};
    use crate::state::{
        EvaluationResultEntry, RuleEvaluationEntry, CONFIG_RULE_STATE_ACTIVE,
        EVALUATION_MODE_DETECTIVE, EVALUATION_MODE_PROACTIVE, RULE_EVALUATION_VISIBILITY_EXTERNAL,
    };

    /// The details of the resource every case below evaluates.
    fn details(resource_type: &str, resource_id: &str) -> ResourceDetails {
        ResourceDetails {
            resource_id: resource_id.to_string(),
            resource_type: resource_type.to_string(),
            resource_configuration: "{}".to_string(),
            resource_configuration_schema_type: None,
        }
    }

    /// A rule named `name` evaluating in `modes`, scoped by `scope`, whose recorded
    /// results are `recorded` keyed by the resource each is about.
    fn rule(
        name: &str,
        modes: &[&str],
        scope: Option<Scope>,
        recorded: &[(&str, &str, &str)],
    ) -> ConfigRuleEntry {
        ConfigRuleEntry {
            name: name.to_string(),
            arn: format!("arn:aws:config:us-east-1:000000000000:config-rule/{name}"),
            id: name.to_string(),
            description: None,
            scope,
            source: Source::default(),
            input_parameters: None,
            maximum_execution_frequency: None,
            state: CONFIG_RULE_STATE_ACTIVE.to_string(),
            evaluation_modes: modes
                .iter()
                .map(|mode| EvaluationModeConfiguration {
                    mode: Some((*mode).to_string()),
                })
                .collect(),
            rule_evaluation_visibility: Some(RULE_EVALUATION_VISIBILITY_EXTERNAL.to_string()),
            created_by: None,
            created_at: SystemTime::UNIX_EPOCH,
            tags: Default::default(),
            evaluation_results: recorded
                .iter()
                .map(|(resource_type, resource_id, compliance)| {
                    (
                        (
                            (*resource_type).to_string(),
                            (*resource_id).to_string(),
                            EVALUATION_MODE_PROACTIVE.to_string(),
                        ),
                        EvaluationResultEntry::new(
                            resource_type,
                            resource_id,
                            EVALUATION_MODE_PROACTIVE,
                            compliance,
                            SystemTime::UNIX_EPOCH,
                        ),
                    )
                })
                .collect(),
            evaluation: RuleEvaluationEntry::default(),
        }
    }

    /// An execution of `details` that started at the Unix epoch.
    fn evaluation(details: ResourceDetails) -> ResourceEvaluationEntry {
        ResourceEvaluationEntry {
            id: "0123456789abcdef".to_string(),
            evaluation_mode: EVALUATION_MODE_PROACTIVE.to_string(),
            started_at: SystemTime::UNIX_EPOCH,
            evaluation_context_identifier: None,
            resource_details: details,
            evaluation_timeout_seconds: 900,
            client_token: None,
        }
    }

    #[test]
    fn an_execution_matches_the_rules_whose_mode_and_scope_cover_the_resource() {
        let evaluated = details("AWS::S3::Bucket", "bucket-1");
        let scoped_to = |resource_types: &[&str]| {
            Some(Scope {
                compliance_resource_types: Some(
                    resource_types.iter().map(|t| (*t).to_string()).collect(),
                ),
                ..Scope::default()
            })
        };

        // Rows differ along the mode and the scope of the one rule in the account.
        let cases: [(&str, &[&str], Option<Scope>, bool); 7] = [
            ("detective only", &[EVALUATION_MODE_DETECTIVE], None, false),
            ("proactive, unscoped", &[EVALUATION_MODE_PROACTIVE], None, true),
            (
                "proactive, scoped to this type",
                &[EVALUATION_MODE_PROACTIVE],
                scoped_to(&["AWS::S3::Bucket"]),
                true,
            ),
            (
                "proactive, scoped to another type",
                &[EVALUATION_MODE_PROACTIVE],
                scoped_to(&["AWS::DynamoDB::Table"]),
                false,
            ),
            (
                "proactive, scoped to this type and id",
                &[EVALUATION_MODE_PROACTIVE],
                Some(Scope {
                    compliance_resource_types: Some(vec!["AWS::S3::Bucket".to_string()]),
                    compliance_resource_id: Some("bucket-1".to_string()),
                    ..Scope::default()
                }),
                true,
            ),
            (
                "proactive, scoped to another id",
                &[EVALUATION_MODE_PROACTIVE],
                Some(Scope {
                    compliance_resource_types: Some(vec!["AWS::S3::Bucket".to_string()]),
                    compliance_resource_id: Some("bucket-2".to_string()),
                    ..Scope::default()
                }),
                false,
            ),
            (
                "proactive, scoped by tag",
                &[EVALUATION_MODE_PROACTIVE],
                Some(Scope {
                    tag_key: Some("owner".to_string()),
                    tag_value: Some("platform".to_string()),
                    ..Scope::default()
                }),
                false,
            ),
        ];

        for (case, modes, scope, expected) in cases {
            let held = rule("rule", modes, scope, &[]);
            let in_mode = rules_evaluating_in([&held].into_iter(), EVALUATION_MODE_PROACTIVE);
            let matched = matching_rules(
                [&held].into_iter(),
                EVALUATION_MODE_PROACTIVE,
                &evaluated,
            );
            assert_eq!(
                !matched.is_empty(),
                expected,
                "case {case:?}: expected the rule to be matched: {expected}, got {} matches",
                matched.len()
            );

            let (status, reason, compliance) = execution_outcome(
                !in_mode.is_empty(),
                &matched,
                EVALUATION_MODE_PROACTIVE,
                &evaluated,
            );
            if expected {
                assert_eq!(
                    status, RESOURCE_EVALUATION_STATUS_FAILED,
                    "case {case:?}: an execution with rule logic to run cannot succeed, \
                     got {status}"
                );
                assert!(
                    reason.is_some_and(|reason| reason.contains("rule")),
                    "case {case:?}: a failed execution must say why"
                );
                assert_eq!(
                    compliance, COMPLIANCE_TYPE_INSUFFICIENT_DATA,
                    "case {case:?}: a rule covers the resource but decided nothing, \
                     got {compliance}"
                );
            } else if in_mode.is_empty() {
                // No rule evaluates in the mode at all: there was no rule
                // configuration to run, so nothing evaluated the resource.
                assert_eq!(
                    status, RESOURCE_EVALUATION_STATUS_FAILED,
                    "case {case:?}: an execution the account configured no rule for cannot \
                     succeed, got {status}"
                );
                assert_eq!(
                    reason.as_deref(),
                    Some(NO_RULE_CONFIGURATION_FAILURE_REASON),
                    "case {case:?}: expected the failure reason Config answers an account \
                     with no proactive rule with, got {reason:?}"
                );
                assert_eq!(
                    compliance, COMPLIANCE_TYPE_NOT_APPLICABLE,
                    "case {case:?}: no rule verdict applies to the resource, got {compliance}"
                );
            } else {
                // The account evaluates in the mode, but no rule covers this
                // resource: the execution ran and nothing applied to it.
                assert_eq!(
                    status, RESOURCE_EVALUATION_STATUS_SUCCEEDED,
                    "case {case:?}: an execution with nothing to run succeeds, got {status}"
                );
                assert!(
                    reason.is_none(),
                    "case {case:?}: an execution that did not fail carries no reason"
                );
                assert_eq!(
                    compliance, COMPLIANCE_TYPE_NOT_APPLICABLE,
                    "case {case:?}: no rule verdict applies to the resource, got {compliance}"
                );
            }
        }
    }

    #[test]
    fn a_finished_execution_rolls_up_the_recorded_results_of_the_rules_it_matched() {
        let evaluated = details("AWS::EC2::Volume", "vol-0");
        // One row: the case, the results recorded for a resource (type, id,
        // compliance), and the status and compliance the execution settles to.
        type RollUpCase<'a> = (&'a str, &'a [(&'a str, &'a str, &'a str)], &'a str, &'a str);
        let cases: [RollUpCase; 6] = [
            (
                "a compliant result",
                &[("AWS::EC2::Volume", "vol-0", COMPLIANCE_TYPE_COMPLIANT)],
                RESOURCE_EVALUATION_STATUS_SUCCEEDED,
                COMPLIANCE_TYPE_COMPLIANT,
            ),
            (
                "a noncompliant result",
                &[("AWS::EC2::Volume", "vol-0", COMPLIANCE_TYPE_NON_COMPLIANT)],
                RESOURCE_EVALUATION_STATUS_SUCCEEDED,
                COMPLIANCE_TYPE_NON_COMPLIANT,
            ),
            (
                // The rule ran and decided the resource is outside what it judges.
                "a not-applicable result",
                &[(
                    "AWS::EC2::Volume",
                    "vol-0",
                    COMPLIANCE_TYPE_NOT_APPLICABLE,
                )],
                RESOURCE_EVALUATION_STATUS_SUCCEEDED,
                COMPLIANCE_TYPE_NOT_APPLICABLE,
            ),
            (
                // The rule ran and could not decide.
                "an insufficient-data result",
                &[(
                    "AWS::EC2::Volume",
                    "vol-0",
                    COMPLIANCE_TYPE_INSUFFICIENT_DATA,
                )],
                RESOURCE_EVALUATION_STATUS_SUCCEEDED,
                COMPLIANCE_TYPE_INSUFFICIENT_DATA,
            ),
            (
                "a result for another resource",
                &[("AWS::EC2::Volume", "vol-1", COMPLIANCE_TYPE_COMPLIANT)],
                RESOURCE_EVALUATION_STATUS_FAILED,
                COMPLIANCE_TYPE_INSUFFICIENT_DATA,
            ),
            (
                "no result at all",
                &[],
                RESOURCE_EVALUATION_STATUS_FAILED,
                COMPLIANCE_TYPE_INSUFFICIENT_DATA,
            ),
        ];

        for (case, recorded, expected_status, expected_compliance) in cases {
            let held = rule("rule", &[EVALUATION_MODE_PROACTIVE], None, recorded);
            let matched = matching_rules(
                [&held].into_iter(),
                EVALUATION_MODE_PROACTIVE,
                &evaluated,
            );
            let (status, _, compliance) =
                execution_outcome(true, &matched, EVALUATION_MODE_PROACTIVE, &evaluated);
            assert_eq!(
                (status.as_str(), compliance.as_str()),
                (expected_status, expected_compliance),
                "case {case:?}: expected {expected_status}/{expected_compliance}, got \
                 {status}/{compliance}"
            );
        }
    }

    #[test]
    fn only_the_clock_moves_an_execution_out_of_in_progress() {
        let started = evaluation(details("AWS::S3::Bucket", "bucket-1"));
        let no_rules: [&ConfigRuleEntry; 0] = [];

        // Rows differ along how far the clock has moved past the start. The account
        // holds no rule, so the terminal status is the bare account's `FAILED`; what
        // this case pins is that only the clock moves the execution there.
        let cases: [(&str, Duration, &str, bool); 4] = [
            ("at the start", Duration::ZERO, RESOURCE_EVALUATION_STATUS_IN_PROGRESS, false),
            (
                "one millisecond short of the span",
                EXECUTION_SETTLES_AFTER - Duration::from_millis(1),
                RESOURCE_EVALUATION_STATUS_IN_PROGRESS,
                false,
            ),
            (
                "exactly at the span",
                EXECUTION_SETTLES_AFTER,
                RESOURCE_EVALUATION_STATUS_FAILED,
                true,
            ),
            (
                "well past the span",
                EXECUTION_SETTLES_AFTER * 10,
                RESOURCE_EVALUATION_STATUS_FAILED,
                true,
            ),
        ];

        for (case, elapsed, expected_status, places_the_resource) in cases {
            let (status, compliance) =
                summary_of(&started, no_rules.into_iter(), started.started_at + elapsed);
            assert_eq!(
                status.status, expected_status,
                "case {case:?}: expected {expected_status}, got {}",
                status.status
            );
            assert_eq!(
                compliance.is_some(),
                places_the_resource,
                "case {case:?}: a running execution reports no compliance, a finished one \
                 does; got {compliance:?}"
            );
        }
    }
}
