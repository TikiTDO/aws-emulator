// SPDX-License-Identifier: CC0-1.0

//! The ORDER an enum-constraint refusal lists an enum's admitted values in, driven
//! end-to-end through the same `MultiServiceRouter` the emulator boots.
//!
//! ONE rule, asserted over a SET of enum-constrained members rather than over the one
//! member a report cited: **the bracketed set of an enum clause states the MEMBERSHIP the
//! model declares, in the order this service lists that enum's values** — its own listing
//! where this service publishes one, and the order the model declares for every enum where
//! it does not.
//!
//! The rows differ along what the rule generalises over: whether the enum has a published
//! listing, how many values it admits (one, two and five), which operation and member carry
//! it, where the member sits in the request (top level, inside a nested structure, inside a
//! structure in a list, and at the first, second and third element of that list), which
//! list member carries it (`Steps` and `OnExceptionSteps`) and which value the request
//! offends with (an unmodelled name, a modelled name in the wrong case, and the empty
//! string) — the set a clause states is a fact about the ENUM, so it can vary with none of
//! them.
//!
//! Every row's membership is checked against the value set the model declares for that
//! enum, read from this crate's generated per-enum catalog (`ops/types/shared_types.rs`)
//! rather than copied: a clause may neither add a value the model does not admit nor drop
//! one it does, whatever order it states them in.

use serde_json::{json, Value};
use service_com_amazonaws_transfer_transferservice::ops::types as model;

use crate::harness::{member_of, Emulator, ROLE};

/// The order this service lists `WorkflowStepType`'s values in, which is what its
/// enum-constraint refusals state. It is a permutation of the value set the model declares
/// (`model::WORKFLOW_STEP_TYPE_VALUES`), asserted per row below, and is NOT any order the
/// model carries.
const LISTED_WORKFLOW_STEP_TYPES: [&str; 5] = ["DELETE", "DECRYPT", "CUSTOM", "COPY", "TAG"];

/// `OverwriteExisting` as the model declares it
/// (`smithy_query(query="shape",shape="com.amazonaws.transfer#OverwriteExisting")`), which
/// is the order a clause states for an enum this service publishes no listing for.
const DECLARED_OVERWRITE_EXISTING: [&str; 2] = ["TRUE", "FALSE"];

/// `EncryptionType` as the model declares it (`query="shape"`, shape `EncryptionType`): a
/// single-valued enum.
const DECLARED_ENCRYPTION_TYPE: [&str; 1] = ["PGP"];

/// The order this service lists `ProfileType`'s values in, which is what its
/// enum-constraint refusals state. Like [`LISTED_WORKFLOW_STEP_TYPES`] it is a permutation
/// of the value set the model declares (`model::PROFILE_TYPE_VALUES`, which declares
/// `LOCAL, PARTNER`), asserted per row below, and is NOT any order the model carries.
const LISTED_PROFILE_TYPES: [&str; 2] = ["PARTNER", "LOCAL"];

/// `SigningAlg` as the model declares it (`query="shape"`, shape `SigningAlg`) — the other
/// FIVE-valued enum of this model, so a listing cannot be applied by the size of a set.
const DECLARED_SIGNING_ALG: [&str; 5] = ["SHA256", "SHA384", "SHA512", "SHA1", "NONE"];

/// `MapType` as the model declares it (`query="shape"`, shape `MapType`).
const DECLARED_MAP_TYPE: [&str; 2] = ["FILE", "DIRECTORY"];

/// A `ServerId` of the modelled form that was never minted: a row that offends an enum is
/// refused at the boundary, before any lookup, so no row needs a stored server.
const UNMINTED_SERVER_ID: &str = "s-0123456789abcdef0";

/// An `InputFileLocation` naming an S3 destination, for the step details that require one.
fn destination() -> Value {
    json!({"S3FileLocation": {"Bucket": "workflow-destination", "Key": "processed/"}})
}

/// A step that offends nothing, so a row's own offending step is the only violation.
fn valid_step(step_type: &str) -> Value {
    match step_type {
        "DELETE" => json!({"Type": "DELETE", "DeleteStepDetails": {"Name": "delete-step"}}),
        "TAG" => json!({"Type": "TAG", "TagStepDetails": {
            "Name": "tag-step",
            "Tags": [{"Key": "reviewed", "Value": "yes"}],
        }}),
        other => panic!("{other} is not a step type this suite builds"),
    }
}

/// The whole message a refusal for ONE enum violation states, written from the rule rather
/// than from the implementation: the AWS envelope over a clause naming the member path and
/// the value set, in the order the service lists it.
fn expected_message(path: &str, listed: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        listed.join(", ")
    )
}

/// The values a message lists between the brackets of its enum clause.
fn bracketed(message: &str) -> Vec<String> {
    let opened = message
        .split_once('[')
        .unwrap_or_else(|| panic!("the clause states a bracketed value set: {message:?}"))
        .1;
    let set = opened
        .split_once(']')
        .unwrap_or_else(|| panic!("the clause closes its bracketed value set: {message:?}"))
        .0;
    set.split(", ").map(str::to_string).collect()
}

/// `values` sorted, so two orders of one membership compare equal.
fn membership(values: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    sorted.sort();
    sorted
}

/// One row: the case, the operation, the request, the member path the clause names, the
/// order the clause must state, and the value set the MODEL declares for that enum.
type Row<'a> = (&'a str, &'a str, Value, &'a str, Vec<&'a str>, &'a [&'a str]);

/// RULE: a value outside an enumerated member's modelled value set is refused HTTP 400
/// `ValidationException` whose message states the member path and the enum's MEMBERSHIP as
/// the model declares it, listed in the order this service lists that enum — its own
/// listing for an enum it publishes one for, the model's declaration order otherwise.
#[tokio::test]
async fn an_enum_clause_lists_the_value_set_in_the_order_this_service_lists_it() {
    let emulator = Emulator::new();

    let rows: Vec<Row<'_>> = vec![
        // ---------------- the enum this service publishes a listing for ----------------
        (
            "CreateWorkflow: the first step's Type, an unmodelled name",
            "CreateWorkflow",
            json!({"Steps": [{"Type": "ENCRYPT", "CopyStepDetails": {
                "Name": "step-copy",
                "DestinationFileLocation": destination(),
            }}]}),
            "steps.1.member.type",
            LISTED_WORKFLOW_STEP_TYPES.to_vec(),
            model::WORKFLOW_STEP_TYPE_VALUES,
        ),
        (
            "CreateWorkflow: the second step's Type, a modelled name in the wrong case",
            "CreateWorkflow",
            json!({"Steps": [valid_step("DELETE"), {"Type": "copy", "CopyStepDetails": {
                "DestinationFileLocation": destination(),
            }}]}),
            "steps.2.member.type",
            LISTED_WORKFLOW_STEP_TYPES.to_vec(),
            model::WORKFLOW_STEP_TYPE_VALUES,
        ),
        (
            "CreateWorkflow: the third step's Type, the empty string",
            "CreateWorkflow",
            json!({"Steps": [valid_step("DELETE"), valid_step("TAG"), {"Type": ""}]}),
            "steps.3.member.type",
            LISTED_WORKFLOW_STEP_TYPES.to_vec(),
            model::WORKFLOW_STEP_TYPE_VALUES,
        ),
        (
            "CreateWorkflow: the same enum reached through OnExceptionSteps",
            "CreateWorkflow",
            json!({
                "Steps": [valid_step("DELETE")],
                "OnExceptionSteps": [{"Type": "NOTIFY", "DeleteStepDetails": {"Name": "x"}}],
            }),
            "onExceptionSteps.1.member.type",
            LISTED_WORKFLOW_STEP_TYPES.to_vec(),
            model::WORKFLOW_STEP_TYPE_VALUES,
        ),
        (
            "CreateProfile: ProfileType at the top level of the input",
            "CreateProfile",
            json!({"As2Id": "partner-as2-id", "ProfileType": "BOTH"}),
            "profileType",
            LISTED_PROFILE_TYPES.to_vec(),
            model::PROFILE_TYPE_VALUES,
        ),
        // ------------- enums this service publishes no listing for: the model's -------------
        (
            "CreateWorkflow: a DECRYPT step's EncryptionType, inside a nested structure",
            "CreateWorkflow",
            json!({"Steps": [{"Type": "DECRYPT", "DecryptStepDetails": {
                "Name": "step-decrypt",
                "Type": "AES",
                "DestinationFileLocation": destination(),
            }}]}),
            "steps.1.member.decryptStepDetails.type",
            DECLARED_ENCRYPTION_TYPE.to_vec(),
            model::ENCRYPTION_TYPE_VALUES,
        ),
        (
            "CreateWorkflow: a COPY step's OverwriteExisting, inside a nested structure",
            "CreateWorkflow",
            json!({"Steps": [{"Type": "COPY", "CopyStepDetails": {
                "Name": "step-copy",
                "DestinationFileLocation": destination(),
                "OverwriteExisting": "MAYBE",
            }}]}),
            "steps.1.member.copyStepDetails.overwriteExisting",
            DECLARED_OVERWRITE_EXISTING.to_vec(),
            model::OVERWRITE_EXISTING_VALUES,
        ),
        (
            "CreateConnector: SigningAlg, the other five-valued enum of this model",
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {"SigningAlgorithm": "SHA3"},
            }),
            "as2Config.signingAlgorithm",
            DECLARED_SIGNING_ALG.to_vec(),
            model::SIGNING_ALG_VALUES,
        ),
        (
            "CreateUser: MapType, inside a structure in a list",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "u-enum",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": [
                    {"Entry": "/", "Target": "/bucket/home"},
                    {"Entry": "/shared", "Target": "/bucket/shared", "Type": "SYMLINK"},
                ],
            }),
            "homeDirectoryMappings.2.member.type",
            DECLARED_MAP_TYPE.to_vec(),
            model::MAP_TYPE_VALUES,
        ),
    ];

    // Every clause stated for the listed enum, so the order can be checked to be a
    // property of the enum rather than of the request that offended it.
    let mut step_type_clauses: Vec<(String, Vec<String>)> = Vec::new();

    for (case, op, request, path, listed, declared) in rows {
        let answer = emulator.call(op, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = expected_message(path, &listed);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The invariant that holds whatever the order is: the values stated are exactly
        // the ones the model declares for that enum.
        let stated = bracketed(&message);
        assert_eq!(
            membership(&stated.iter().map(String::as_str).collect::<Vec<&str>>()),
            membership(declared),
            "case {case:?}: expected the model's membership {declared:?}, got {stated:?}"
        );
        if declared == model::WORKFLOW_STEP_TYPE_VALUES {
            step_type_clauses.push((case.to_string(), stated));
        }
    }

    // One enum, one ordered set: every row that offended the listed enum stated the same
    // list, whichever member carried it, wherever it sat and whatever it offended with.
    assert!(
        step_type_clauses.len() >= 4,
        "the table must offend the listed enum from several positions, got \
         {step_type_clauses:?}"
    );
    let first = step_type_clauses[0].1.clone();
    for (case, stated) in &step_type_clauses {
        assert_eq!(
            *stated, first,
            "case {case:?}: expected one ordered set per enum, {first:?}, got {stated:?}"
        );
    }

    // No row was served: a refused create persists nothing.
    let listed_workflows = emulator.call("ListWorkflows", json!({})).await;
    assert_eq!(
        listed_workflows.status, 200,
        "case \"the store after the table\": expected 200, got {} with body {}",
        listed_workflows.status, listed_workflows.body
    );
    let stored = listed_workflows
        .get("Workflows")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        stored.is_empty(),
        "case \"the store after the table\": expected an empty store, got {stored:?}"
    );
}

/// RULE (the same rule, several offending members at once): the order belongs to the
/// renderer every enum-constrained member goes through, so a refusal stating more than one
/// clause states each enum's own listing — the published one for the enum that has one, the
/// model's for the enum that does not — in the one envelope.
#[tokio::test]
async fn a_refusal_stating_several_clauses_lists_each_enum_as_that_enum_is_listed() {
    let emulator = Emulator::new();

    let answer = emulator
        .call(
            "CreateWorkflow",
            json!({"Steps": [
                {"Type": "ENCRYPT"},
                {"Type": "COPY", "CopyStepDetails": {
                    "Name": "step-copy",
                    "DestinationFileLocation": destination(),
                    "OverwriteExisting": "MAYBE",
                }},
            ]}),
        )
        .await;

    assert_eq!(
        answer.status, 400,
        "case \"two offending members\": expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case \"two offending members\": expected ValidationException, got body {}",
        answer.body
    );
    let message = member_of(&answer.body, "message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let expected = format!(
        "2 validation errors detected: Value at 'steps.1.member.type' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]; Value at \
         'steps.2.member.copyStepDetails.overwriteExisting' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        LISTED_WORKFLOW_STEP_TYPES.join(", "),
        DECLARED_OVERWRITE_EXISTING.join(", ")
    );
    assert_eq!(
        message, expected,
        "case \"two offending members\": expected {expected:?}, got {message:?}"
    );
}
