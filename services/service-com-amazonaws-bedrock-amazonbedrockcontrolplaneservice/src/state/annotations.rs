// SPDX-License-Identifier: CC0-1.0

//! The ANNOTATION SET of an Automated Reasoning policy build workflow: the corrections to
//! the policy's rules, variables and types a build carries, and the hash that identifies
//! the set a read reports.
//!
//! One file per concern of this service's store: the record the set is held on lives with
//! the rest of the build workflow family ([`super::automated_reasoning_policies`]), and the
//! rendering and the digest of that set — which every operation reporting an
//! `annotationSetHash` shares — live here.

use crate::ops::types::{
    AutomatedReasoningPolicyAnnotation, AutomatedReasoningPolicyTypeValueAnnotation,
};

/// The `annotationSetHash` of a build workflow's annotation set: 128 digits of `[0-9a-z]`,
/// as `AutomatedReasoningPolicyHash` requires
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyHash")`).
///
/// A function of the annotation SET alone, exactly as [`super::policy_definition_hash`] is a
/// function of the definition alone, so a read of a set reports the hash a write of that
/// same set reports and any change to the set — a different annotation, a different value
/// in one, or the same annotations in another order — changes it. That is what the member
/// documents it for: "a hash value representing the current state of the annotations …
/// used for optimistic concurrency control when updating annotations"
/// (`smithy_query(query="operation",operation="GetAutomatedReasoningPolicyAnnotations")`).
///
/// The digest is SHA-512 hex, whose 128 lowercase hex digits are exactly the width and the
/// alphabet the shape admits — the same digest and the same reason as the policy
/// definition's hash — taken over a canonical rendering of the set
/// ([`canonical_annotation_set`]) rather than over the request bytes, so a set written two
/// ways hashes alike. The EMPTY set hashes too: a build workflow whose annotations nothing
/// has corrected still reports a hash, and reports the same one until the set changes.
#[must_use]
pub fn annotation_set_hash(annotations: &[AutomatedReasoningPolicyAnnotation]) -> String {
    use ::sha2::{Digest, Sha512};

    let canonical = canonical_annotation_set(annotations).to_string();
    ::hex::encode(Sha512::digest(canonical.as_bytes()))
}

/// An annotation set as the one JSON value [`annotation_set_hash`] digests: each annotation
/// in the set's own order, rendered as the union member it is and every member of that
/// member in a fixed order, so the rendering depends on the set's CONTENT and on nothing
/// else.
///
/// The member names are the model's own
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyAnnotation")`), so an
/// annotation that differs from another only in which correction it asks for renders
/// differently.
fn canonical_annotation_set(
    annotations: &[AutomatedReasoningPolicyAnnotation],
) -> ::serde_json::Value {
    ::serde_json::Value::Array(annotations.iter().map(canonical_annotation).collect())
}

/// One annotation as its canonical JSON: the union member that is set, carrying that
/// member's own members.
fn canonical_annotation(annotation: &AutomatedReasoningPolicyAnnotation) -> ::serde_json::Value {
    match annotation {
        AutomatedReasoningPolicyAnnotation::addType(added) => ::serde_json::json!({
            "addType": {
                "name": added.name,
                "description": added.description,
                "values": added
                    .values
                    .iter()
                    .map(|value| ::serde_json::json!({
                        "value": value.value,
                        "description": value.description,
                    }))
                    .collect::<Vec<_>>(),
            },
        }),
        AutomatedReasoningPolicyAnnotation::updateType(updated) => ::serde_json::json!({
            "updateType": {
                "name": updated.name,
                "newName": updated.new_name,
                "description": updated.description,
                "values": updated
                    .values
                    .iter()
                    .map(canonical_type_value_annotation)
                    .collect::<Vec<_>>(),
            },
        }),
        AutomatedReasoningPolicyAnnotation::deleteType(deleted) => ::serde_json::json!({
            "deleteType": { "name": deleted.name },
        }),
        AutomatedReasoningPolicyAnnotation::addVariable(added) => ::serde_json::json!({
            "addVariable": {
                "name": added.name,
                "type": added.r#type,
                "description": added.description,
            },
        }),
        AutomatedReasoningPolicyAnnotation::updateVariable(updated) => ::serde_json::json!({
            "updateVariable": {
                "name": updated.name,
                "newName": updated.new_name,
                "description": updated.description,
            },
        }),
        AutomatedReasoningPolicyAnnotation::deleteVariable(deleted) => ::serde_json::json!({
            "deleteVariable": { "name": deleted.name },
        }),
        AutomatedReasoningPolicyAnnotation::addRule(added) => ::serde_json::json!({
            "addRule": { "expression": added.expression },
        }),
        AutomatedReasoningPolicyAnnotation::updateRule(updated) => ::serde_json::json!({
            "updateRule": {
                "ruleId": updated.rule_id,
                "expression": updated.expression,
            },
        }),
        AutomatedReasoningPolicyAnnotation::deleteRule(deleted) => ::serde_json::json!({
            "deleteRule": { "ruleId": deleted.rule_id },
        }),
        AutomatedReasoningPolicyAnnotation::addRuleFromNaturalLanguage(added) => {
            ::serde_json::json!({
                "addRuleFromNaturalLanguage": { "naturalLanguage": added.natural_language },
            })
        }
        AutomatedReasoningPolicyAnnotation::updateFromRulesFeedback(feedback) => {
            ::serde_json::json!({
                "updateFromRulesFeedback": {
                    "ruleIds": feedback.rule_ids,
                    "feedback": feedback.feedback,
                },
            })
        }
        AutomatedReasoningPolicyAnnotation::updateFromScenarioFeedback(feedback) => {
            ::serde_json::json!({
                "updateFromScenarioFeedback": {
                    "ruleIds": feedback.rule_ids,
                    "scenarioExpression": feedback.scenario_expression,
                    "feedback": feedback.feedback,
                },
            })
        }
        AutomatedReasoningPolicyAnnotation::ingestContent(ingested) => ::serde_json::json!({
            "ingestContent": { "content": ingested.content },
        }),
    }
}

/// One `AutomatedReasoningPolicyTypeValueAnnotation` — the correction to ONE value of a
/// custom type an `updateType` annotation carries — as its canonical JSON, on the terms
/// [`canonical_annotation`] renders the annotation itself on.
fn canonical_type_value_annotation(
    value: &AutomatedReasoningPolicyTypeValueAnnotation,
) -> ::serde_json::Value {
    match value {
        AutomatedReasoningPolicyTypeValueAnnotation::addTypeValue(added) => ::serde_json::json!({
            "addTypeValue": {
                "value": added.value,
                "description": added.description,
            },
        }),
        AutomatedReasoningPolicyTypeValueAnnotation::updateTypeValue(updated) => {
            ::serde_json::json!({
                "updateTypeValue": {
                    "value": updated.value,
                    "newValue": updated.new_value,
                    "description": updated.description,
                },
            })
        }
        AutomatedReasoningPolicyTypeValueAnnotation::deleteTypeValue(deleted) => {
            ::serde_json::json!({
                "deleteTypeValue": { "value": deleted.value },
            })
        }
    }
}
