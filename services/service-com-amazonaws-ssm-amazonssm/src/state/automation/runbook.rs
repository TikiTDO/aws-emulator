// SPDX-License-Identifier: CC0-1.0

//! The Automation runbook an execution runs: the steps its content declares, the
//! variables each step's inputs are resolved against, and the choice a branch step makes.
//!
//! An Automation runbook declares its steps as `mainSteps`, each with a `name`, the
//! `action` that step performs and the `inputs` that action takes, plus the flow members
//! that say where the walk goes next (`nextStep`, `isEnd`) and what a failure does
//! (`onFailure`, `isCritical`). This module reads those declarations out of the content
//! the document store already holds and nothing else: the content is read through the ONE
//! document parser this service reads document content with
//! ([`super::super::load_single_document`]), so a runbook is the same tree every other
//! document read derives its report from, already bounded in size and nesting when it was
//! persisted.
//!
//! Everything here is PURE — a function of the content and of the execution's own
//! parameters — so the walk in [`super`] is the only place that moves a status, and the
//! reading of a runbook is pinned by this module's own tests.

use std::collections::BTreeMap;

use ::serde_json::{Map, Value};

/// How many steps of one runbook this emulator walks.
///
/// The step count comes from caller-supplied content, and every step the walk enters
/// holds a step-execution record for as long as the execution is reported, so the walk
/// needs a bound that does not depend on the content being friendly. This is the
/// emulator's own guard rather than a published quota.
pub const MAX_RUNBOOK_STEPS: usize = 200;

/// The `onFailure` a step that declares none runs with: "The action to take if the step
/// fails. The default value is `Abort`" (`StepExecution$OnFailure`).
pub const ON_FAILURE_ABORT: &str = "Abort";

/// The `onFailure` that lets the walk carry on past a failed step: "Continue will ignore
/// the failure of current step and allow automation to run the next step"
/// (`StepExecution$ValidNextSteps`).
pub const ON_FAILURE_CONTINUE: &str = "Continue";

/// The `onFailure` form that names the step to go to instead: `onFailure: "step:<name>"`.
pub const ON_FAILURE_STEP_PREFIX: &str = "step:";

/// The `maxAttempts` a step that declares none runs with: "The maximum number of tries to
/// run the action of the step. The default value is `1`" (`StepExecution$MaxAttempts`).
pub const DEFAULT_MAX_ATTEMPTS: i32 = 1;

/// One step of a runbook, exactly as the content declares it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RunbookStep {
    /// "The name of this execution step" (`StepExecution$StepName`), unique within the
    /// runbook, which is what `nextStep` and the `step:` form of `onFailure` name.
    pub name: String,
    /// "The action this step performs. The action determines the behavior of the step"
    /// (`StepExecution$Action`).
    pub action: String,
    /// The step's `inputs` as the content declares them, before the execution's variables
    /// are resolved into them ([`resolve`]).
    pub inputs: Map<String, Value>,
    /// "The next step after the step succeeds" (`StepExecution$NextStep`); the step
    /// declared after this one when the content names none.
    pub next_step: Option<String>,
    /// "The flag which can be used to end automation no matter whether the step succeeds
    /// or fails" (`StepExecution$IsEnd`).
    pub is_end: Option<bool>,
    /// "The action to take if the step fails" (`StepExecution$OnFailure`).
    pub on_failure: Option<String>,
    /// "The flag which can be used to help decide whether the failure of current step
    /// leads to the Automation failure" (`StepExecution$IsCritical`).
    pub is_critical: Option<bool>,
    /// "The maximum number of tries to run the action of the step"
    /// (`StepExecution$MaxAttempts`).
    pub max_attempts: Option<i32>,
    /// "The timeout seconds of the step" (`StepExecution$TimeoutSeconds`).
    pub timeout_seconds: Option<i64>,
}

impl RunbookStep {
    /// Whether a failure of this step fails the whole execution: `isCritical` as the step
    /// declares it, and the critical default a step that declares none carries.
    pub fn critical(&self) -> bool {
        self.is_critical.unwrap_or(true)
    }

    /// What a failure of this step does to the walk: `onFailure` as the step declares it,
    /// and [`ON_FAILURE_ABORT`] for a step that declares none.
    pub fn on_failure(&self) -> &str {
        self.on_failure
            .as_deref()
            .filter(|declared| !declared.is_empty())
            .unwrap_or(ON_FAILURE_ABORT)
    }
}

/// The steps `content` declares, in the content's own order, or why that content declares
/// no runbook this service can walk.
///
/// A runbook's steps are its `mainSteps`, and a step with no `name` or no `action` is not
/// one the walk could enter or run, so content missing either declares no runnable
/// runbook rather than a step the walk would have to guess at. Two steps of one runbook
/// cannot share a name: `nextStep` names a step by it.
pub fn steps_of(content: &str) -> Result<Vec<RunbookStep>, String> {
    let tree = super::super::load_single_document(content)
        .map_err(|_| "Content could not be read as one document tree".to_string())?;
    let document = super::super::document_as_json(&tree, super::super::MAX_DOCUMENT_DEPTH)
        .ok_or_else(|| "Content could not be read as one document tree".to_string())?;
    let declared = document
        .get("mainSteps")
        .and_then(Value::as_array)
        .ok_or_else(|| "Content declares no mainSteps, so it declares no steps to run".to_string())?;
    if declared.len() > MAX_RUNBOOK_STEPS {
        return Err(format!(
            "Content declares {} steps; a runbook this emulator runs declares at most \
             {MAX_RUNBOOK_STEPS}",
            declared.len()
        ));
    }
    let mut steps: Vec<RunbookStep> = Vec::with_capacity(declared.len());
    for step in declared {
        let Some(declaration) = step.as_object() else {
            return Err("Every step of mainSteps must be a mapping".to_string());
        };
        let Some(name) = text_of(declaration, "name") else {
            return Err("Every step of mainSteps must declare a name".to_string());
        };
        let Some(action) = text_of(declaration, "action") else {
            return Err(format!("Step {name} declares no action"));
        };
        if steps.iter().any(|held| held.name == name) {
            return Err(format!(
                "Step {name} is declared twice; a step name is unique within a runbook"
            ));
        }
        steps.push(RunbookStep {
            name,
            action,
            inputs: declaration
                .get("inputs")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            next_step: text_of(declaration, "nextStep"),
            is_end: declaration.get("isEnd").and_then(Value::as_bool),
            on_failure: text_of(declaration, "onFailure"),
            is_critical: declaration.get("isCritical").and_then(Value::as_bool),
            max_attempts: declaration
                .get("maxAttempts")
                .and_then(Value::as_i64)
                .and_then(|attempts| i32::try_from(attempts).ok()),
            timeout_seconds: declaration.get("timeoutSeconds").and_then(Value::as_i64),
        });
    }
    if steps.is_empty() {
        return Err("Content declares no steps to run".to_string());
    }
    Ok(steps)
}

/// A non-empty string member of a declaration.
fn text_of(declaration: &Map<String, Value>, member: &str) -> Option<String> {
    declaration
        .get(member)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The reference form a runbook's inputs carry a variable in: `{{ Name }}`, with any
/// amount of space inside the braces.
///
/// One regular expression, compiled once for the process: the references are read out of
/// caller-supplied content, so the scan is the `regex` crate's rather than a hand-rolled
/// one, and it cannot nest (a reference holds no brace).
static REFERENCE: ::std::sync::LazyLock<::regex::Regex> = ::std::sync::LazyLock::new(|| {
    ::regex::Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").expect("the reference form is a valid regex")
});

/// How many bytes of RESOLVED step inputs one execution may produce.
///
/// Resolving a reference replaces `{{ Name }}` — some seven bytes of a runbook's own
/// content — with the value the execution holds for it, which the modelled
/// `AutomationParameterValue` admits up to 512 bytes of, 50 to a parameter. The expansion
/// is therefore the PRODUCT of two values a caller chooses: how many references the
/// content carries (bounded by [`super::super::MAX_DOCUMENT_CONTENT`], which admits
/// thousands of them) and how long each resolves to. This is the cap on that product: the
/// resolved inputs one execution produces and STORES, across every step it walks, may not
/// exceed twice the content a whole document may hold, after which the step that ran over
/// it fails. Nothing else bounds the product, so it is charged here rather than per step.
pub const MAX_RESOLVED_INPUT_BYTES: usize = 2 * super::super::MAX_DOCUMENT_CONTENT;

/// `value` with every `{{ Name }}` reference in it replaced by what `variables` holds for
/// that name, or the name the execution has no value for.
///
/// "Fully-resolved values passed into the step before execution"
/// (`StepExecution$Inputs`): a step's inputs are resolved BEFORE the action runs, so a
/// reference the execution cannot resolve is a failure of the step rather than a value
/// passed on as the literal text of the reference. Strings are substituted inside, and a
/// list or a mapping is resolved member by member, so a reference resolves wherever the
/// content carries one.
///
/// Bounded by a whole [`MAX_RESOLVED_INPUT_BYTES`] budget of its own; an execution's walk
/// charges every step against ONE budget ([`resolve_inputs`]).
pub fn resolve(value: &Value, variables: &BTreeMap<String, String>) -> Result<Value, String> {
    let mut budget = MAX_RESOLVED_INPUT_BYTES;
    resolve_within(value, variables, &mut budget)
}

/// [`resolve`], charging what it produces against `budget`.
fn resolve_within(
    value: &Value,
    variables: &BTreeMap<String, String>,
    budget: &mut usize,
) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => Value::String(resolve_text(text, variables, budget)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_within(item, variables, budget))
                .collect::<Result<Vec<Value>, String>>()?,
        ),
        Value::Object(members) => {
            let mut resolved = Map::new();
            for (member, held) in members {
                resolved.insert(member.clone(), resolve_within(held, variables, budget)?);
            }
            Value::Object(resolved)
        }
        other => other.clone(),
    })
}

/// `text` with every reference in it resolved, or the FIRST name `variables` holds no
/// value for — or the budget it would have run over.
///
/// The expansion is MEASURED before it is built: a reference is some seven bytes of
/// content and the value it resolves to may be thousands, so the length the substitution
/// would produce is accumulated reference by reference and checked against what is left of
/// the budget FIRST. Nothing larger than the budget is ever allocated, and an execution
/// that runs over it spends the budget, so a later step of the same execution cannot
/// attempt its own full-size expansion.
fn resolve_text(
    text: &str,
    variables: &BTreeMap<String, String>,
    budget: &mut usize,
) -> Result<String, String> {
    let over_budget = || {
        format!(
            "Resolving the inputs of this automation's steps produced more than \
             {MAX_RESOLVED_INPUT_BYTES} bytes"
        )
    };
    let mut projected = text.len();
    if projected > *budget {
        *budget = 0;
        return Err(over_budget());
    }
    for captured in REFERENCE.captures_iter(text) {
        let reference = captured.get(0).map_or(0, |reference| reference.len());
        let name = captured.get(1).map_or("", |name| name.as_str());
        let Some(held) = variables.get(name) else {
            return Err(format!("Failed to resolve input parameter {name}"));
        };
        projected = projected
            .saturating_sub(reference)
            .saturating_add(held.len());
        if projected > *budget {
            *budget = 0;
            return Err(over_budget());
        }
    }
    // Every reference resolves and the whole substitution fits in what is left of the
    // budget, which is what this builds.
    let resolved = REFERENCE.replace_all(text, |captured: &::regex::Captures<'_>| {
        let name = captured.get(1).map_or("", |name| name.as_str());
        variables.get(name).cloned().unwrap_or_default()
    });
    *budget = budget.saturating_sub(resolved.len());
    Ok(resolved.into_owned())
}

/// A step's whole `inputs` mapping with every reference in it resolved ([`resolve`]), or
/// the name the execution has no value for.
///
/// `budget` is what is LEFT of the execution's [`MAX_RESOLVED_INPUT_BYTES`]: one budget
/// spans every step an execution walks, so the resolved inputs it produces and stores are
/// bounded however many references its runbook carries and however long each resolves to.
pub fn resolve_inputs(
    inputs: &Map<String, Value>,
    variables: &BTreeMap<String, String>,
    budget: &mut usize,
) -> Result<Map<String, Value>, String> {
    let mut resolved = Map::new();
    for (member, declared) in inputs {
        resolved.insert(member.clone(), resolve_within(declared, variables, budget)?);
    }
    Ok(resolved)
}

/// One value as a step's `Inputs` reports it: "Fully-resolved values passed into the step
/// before execution" (`StepExecution$Inputs`), a map of string to STRING, so a scalar is
/// reported as its own text and a list or mapping as the JSON it is.
pub fn rendered(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The choices a branch step makes its decision from: "aws:branch" takes `Choices`, each
/// naming the step to go to when its condition holds, and an optional `Default` for when
/// none of them does.
#[derive(Debug, Clone)]
pub struct Branch {
    /// The choices in the content's own order: the first one whose condition holds is the
    /// one the branch takes.
    pub choices: Vec<Choice>,
    /// The step the branch takes when no choice holds, when the content names one.
    pub default: Option<String>,
}

/// One choice of a branch step: the step it names, and the condition that has to hold.
#[derive(Debug, Clone)]
pub struct Choice {
    /// The step this choice goes to.
    pub next_step: String,
    /// The condition as the content declares it, read by [`holds`].
    pub condition: Map<String, Value>,
}

/// The branch `inputs` declares, or why those inputs are not a branch this service can
/// decide.
pub fn branch_of(inputs: &Map<String, Value>) -> Result<Branch, String> {
    let declared = inputs
        .get("Choices")
        .and_then(Value::as_array)
        .ok_or_else(|| "The Choices of a branch step must be a list of choices".to_string())?;
    let mut choices = Vec::with_capacity(declared.len());
    for choice in declared {
        let Some(condition) = choice.as_object() else {
            return Err("Every choice of a branch step must be a mapping".to_string());
        };
        let Some(next_step) = text_of(condition, "NextStep") else {
            return Err("Every choice of a branch step must name its NextStep".to_string());
        };
        choices.push(Choice {
            next_step,
            condition: condition.clone(),
        });
    }
    Ok(Branch {
        choices,
        default: text_of(inputs, "Default"),
    })
}

/// How deep a choice's conditions may nest through `And`, `Or` and `Not`.
///
/// The conditions come from caller-supplied content, and the evaluation below descends
/// through them, so the descent carries its own bound rather than relying on the
/// content's.
const MAX_CONDITION_DEPTH: usize = 16;

/// Whether one choice's condition holds, or why the condition cannot be decided.
///
/// A condition compares one `Variable` with one operator, or composes other conditions
/// with `And`, `Or` or `Not`. The operators are the ones the Automation action reference
/// documents for `aws:branch`: `StringEquals`, `EqualsIgnoreCase`, `StartsWith`,
/// `EndsWith`, `Contains`, the five `Numeric*` comparisons and `BooleanEquals`. An
/// operator outside that set cannot be decided, so the step that declared it fails rather
/// than the branch quietly taking the next choice.
pub fn holds(condition: &Map<String, Value>, depth: usize) -> Result<bool, String> {
    if depth > MAX_CONDITION_DEPTH {
        return Err(format!(
            "A branch choice nests conditions more than {MAX_CONDITION_DEPTH} levels deep"
        ));
    }
    if let Some(all) = condition.get("And").and_then(Value::as_array) {
        return composed(all, depth, true);
    }
    if let Some(any) = condition.get("Or").and_then(Value::as_array) {
        return composed(any, depth, false);
    }
    if let Some(negated) = condition.get("Not").and_then(Value::as_object) {
        return Ok(!holds(negated, depth + 1)?);
    }
    let Some(left) = condition.get("Variable").map(rendered) else {
        return Err(
            "A branch choice must carry a Variable to compare, or And, Or or Not".to_string(),
        );
    };
    for (operator, right) in condition {
        let Some(decided) = compare(operator, &left, right)? else {
            continue;
        };
        return Ok(decided);
    }
    Err(format!(
        "A branch choice on {left:?} carries no comparison this service decides"
    ))
}

/// Whether every (`all`) or any (`!all`) of `conditions` holds.
fn composed(conditions: &[Value], depth: usize, all: bool) -> Result<bool, String> {
    let mut decided = all;
    for condition in conditions {
        let Some(condition) = condition.as_object() else {
            return Err("Every composed branch condition must be a mapping".to_string());
        };
        let held = holds(condition, depth + 1)?;
        decided = if all { decided && held } else { decided || held };
    }
    Ok(decided)
}

/// What `operator` decides about `left` and `right`, `None` when the member is not an
/// operator at all (`NextStep` and `Variable` are members of the same mapping), and `Err`
/// when it is an operator this service cannot decide for these values.
fn compare(operator: &str, left: &str, right: &Value) -> Result<Option<bool>, String> {
    let wanted = rendered(right);
    let numeric = |decide: fn(f64, f64) -> bool| -> Result<Option<bool>, String> {
        let (Ok(left), Ok(wanted)) = (left.parse::<f64>(), wanted.parse::<f64>()) else {
            return Err(format!(
                "A branch choice compares {left:?} with {wanted:?} numerically, and one of them \
                 is not a number"
            ));
        };
        Ok(Some(decide(left, wanted)))
    };
    match operator {
        "NextStep" | "Variable" => Ok(None),
        "StringEquals" => Ok(Some(left == wanted)),
        "EqualsIgnoreCase" => Ok(Some(left.eq_ignore_ascii_case(&wanted))),
        "StartsWith" => Ok(Some(left.starts_with(&wanted))),
        "EndsWith" => Ok(Some(left.ends_with(&wanted))),
        "Contains" => Ok(Some(left.contains(&wanted))),
        "NumericEquals" => numeric(|left, wanted| left == wanted),
        "NumericGreater" => numeric(|left, wanted| left > wanted),
        "NumericLesser" => numeric(|left, wanted| left < wanted),
        "NumericGreaterOrEquals" => numeric(|left, wanted| left >= wanted),
        "NumericLesserOrEquals" => numeric(|left, wanted| left <= wanted),
        "BooleanEquals" => {
            let (Ok(left), Ok(wanted)) = (left.parse::<bool>(), wanted.parse::<bool>()) else {
                return Err(format!(
                    "A branch choice compares {left:?} with {wanted:?} as booleans, and one of \
                     them is not a boolean"
                ));
            };
            Ok(Some(left == wanted))
        }
        _ => Err(format!(
            "A branch choice carries the comparison {operator:?}, which this service does not \
             decide"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A runbook's steps are read out of its content with every flow member the content
    /// declares, and content that declares no runnable step is refused rather than walked.
    #[test]
    fn a_runbooks_steps_are_read_out_of_its_content() {
        let content = r#"{
          "schemaVersion": "0.3",
          "mainSteps": [
            {"name": "decide", "action": "aws:branch", "inputs": {"Choices": [{"NextStep": "wait", "Variable": "{{ Pick }}", "StringEquals": "wait"}], "Default": "stop"}},
            {"name": "wait", "action": "aws:sleep", "inputs": {"Duration": "PT1S"}, "isEnd": true, "onFailure": "Continue", "isCritical": false, "maxAttempts": 2, "timeoutSeconds": 30},
            {"name": "stop", "action": "aws:pause", "inputs": {}, "nextStep": "wait"}
          ]
        }"#;
        let steps = steps_of(content).expect("this content declares steps");
        let names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["decide", "wait", "stop"],
            "case the step order: expected the content's own order, got {names:?}"
        );
        let wait = &steps[1];
        assert_eq!(
            (
                wait.action.as_str(),
                wait.is_end,
                wait.on_failure.as_deref(),
                wait.critical(),
                wait.max_attempts,
                wait.timeout_seconds
            ),
            (
                "aws:sleep",
                Some(true),
                Some("Continue"),
                false,
                Some(2),
                Some(30)
            ),
            "case the flow members of the second step: expected what the content declares, \
             got {wait:?}"
        );
        assert_eq!(
            steps[2].next_step.as_deref(),
            Some("wait"),
            "case nextStep: expected the step the content names, got {:?}",
            steps[2]
        );

        for (case, content) in [
            ("no mainSteps", r#"{"schemaVersion": "0.3"}"#),
            ("empty mainSteps", r#"{"mainSteps": []}"#),
            ("a step with no action", r#"{"mainSteps": [{"name": "one"}]}"#),
            ("a step with no name", r#"{"mainSteps": [{"action": "aws:sleep"}]}"#),
            (
                "two steps of one name",
                r#"{"mainSteps": [{"name": "one", "action": "aws:sleep"}, {"name": "one", "action": "aws:sleep"}]}"#,
            ),
        ] {
            let read = steps_of(content);
            assert!(
                read.is_err(),
                "case {case:?}: expected content with no runnable runbook to be refused, got \
                 {read:?}"
            );
        }
    }

    /// Every reference in a step's inputs is resolved before the step runs, wherever the
    /// content carries one, and a name the execution holds no value for fails the step.
    #[test]
    fn step_inputs_resolve_every_reference_they_carry() {
        let variables: BTreeMap<String, String> = [
            ("Name".to_string(), "web".to_string()),
            ("Count".to_string(), "3".to_string()),
        ]
        .into_iter()
        .collect();

        for (case, declared, expected) in [
            ("a whole value", "{{ Name }}", "web"),
            ("a value inside text", "the {{ Name }} node", "the web node"),
            ("two references", "{{ Name }}-{{ Count }}", "web-3"),
            ("no reference", "plain", "plain"),
            ("space inside the braces", "{{Name}}", "web"),
        ] {
            let resolved = resolve(&Value::String(declared.to_string()), &variables)
                .unwrap_or_else(|why| panic!("case {case:?}: expected a resolution, got {why:?}"));
            assert_eq!(
                resolved,
                Value::String(expected.to_string()),
                "case {case:?}: expected {expected:?}"
            );
        }

        let nested = ::serde_json::json!({"Values": ["{{ Name }}", 2], "Of": {"It": "{{ Count }}"}});
        assert_eq!(
            resolve(&nested, &variables).expect("every name resolves"),
            ::serde_json::json!({"Values": ["web", 2], "Of": {"It": "3"}}),
            "case a nested input: expected every reference in it resolved"
        );

        let missing = resolve(&Value::String("{{ Absent }}".to_string()), &variables);
        assert_eq!(
            missing,
            Err("Failed to resolve input parameter Absent".to_string()),
            "case an unresolvable reference: expected the name reported, got {missing:?}"
        );
    }

    /// Each documented comparison decides a choice, a composed condition decides from the
    /// conditions under it, and a comparison this service does not decide is refused.
    #[test]
    fn a_branch_choice_decides_by_its_comparison() {
        let condition = |members: ::serde_json::Value| -> Map<String, Value> {
            members
                .as_object()
                .expect("a choice is a mapping")
                .clone()
        };

        for (case, members, expected) in [
            (
                "StringEquals that holds",
                ::serde_json::json!({"NextStep": "a", "Variable": "web", "StringEquals": "web"}),
                true,
            ),
            (
                "StringEquals that does not",
                ::serde_json::json!({"NextStep": "a", "Variable": "web", "StringEquals": "db"}),
                false,
            ),
            (
                "EqualsIgnoreCase",
                ::serde_json::json!({"Variable": "WEB", "EqualsIgnoreCase": "web"}),
                true,
            ),
            (
                "StartsWith",
                ::serde_json::json!({"Variable": "webserver", "StartsWith": "web"}),
                true,
            ),
            (
                "EndsWith",
                ::serde_json::json!({"Variable": "webserver", "EndsWith": "server"}),
                true,
            ),
            (
                "Contains",
                ::serde_json::json!({"Variable": "webserver", "Contains": "bse"}),
                true,
            ),
            (
                "NumericEquals",
                ::serde_json::json!({"Variable": "3", "NumericEquals": 3}),
                true,
            ),
            (
                "NumericGreater",
                ::serde_json::json!({"Variable": "4", "NumericGreater": 3}),
                true,
            ),
            (
                "NumericLesser",
                ::serde_json::json!({"Variable": "2", "NumericLesser": 3}),
                true,
            ),
            (
                "NumericGreaterOrEquals",
                ::serde_json::json!({"Variable": "3", "NumericGreaterOrEquals": 3}),
                true,
            ),
            (
                "NumericLesserOrEquals",
                ::serde_json::json!({"Variable": "3", "NumericLesserOrEquals": 4}),
                true,
            ),
            (
                "BooleanEquals",
                ::serde_json::json!({"Variable": "true", "BooleanEquals": true}),
                true,
            ),
            (
                "And of two that hold",
                ::serde_json::json!({"And": [{"Variable": "a", "StringEquals": "a"}, {"Variable": "2", "NumericEquals": 2}]}),
                true,
            ),
            (
                "And with one that does not",
                ::serde_json::json!({"And": [{"Variable": "a", "StringEquals": "a"}, {"Variable": "2", "NumericEquals": 3}]}),
                false,
            ),
            (
                "Or with one that holds",
                ::serde_json::json!({"Or": [{"Variable": "a", "StringEquals": "b"}, {"Variable": "2", "NumericEquals": 2}]}),
                true,
            ),
            (
                "Not of one that does not hold",
                ::serde_json::json!({"Not": {"Variable": "a", "StringEquals": "b"}}),
                true,
            ),
        ] {
            let decided = holds(&condition(members), 0)
                .unwrap_or_else(|why| panic!("case {case:?}: expected a decision, got {why:?}"));
            assert_eq!(decided, expected, "case {case:?}: expected {expected}");
        }

        for (case, members) in [
            (
                "a comparison outside the documented set",
                ::serde_json::json!({"Variable": "a", "MatchesRegex": "a"}),
            ),
            (
                "no comparison at all",
                ::serde_json::json!({"NextStep": "a", "Variable": "a"}),
            ),
            (
                "no variable to compare",
                ::serde_json::json!({"NextStep": "a", "StringEquals": "a"}),
            ),
            (
                "a numeric comparison of text",
                ::serde_json::json!({"Variable": "web", "NumericEquals": 3}),
            ),
        ] {
            let decided = holds(&condition(members), 0);
            assert!(
                decided.is_err(),
                "case {case:?}: expected a choice this service cannot decide to be refused, got \
                 {decided:?}"
            );
        }
    }
}
