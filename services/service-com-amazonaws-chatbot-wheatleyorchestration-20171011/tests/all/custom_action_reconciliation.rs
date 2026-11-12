// SPDX-License-Identifier: CC0-1.0

//! The read-modify-write loop over a custom action, and what the WRITE path answers for an
//! action this account does not hold — driven over the wire (restJson1) through the same
//! `MultiServiceRouter` the production router serves with.
//!
//! Two rules, one per suite below:
//!
//! * a resource's read projects EVERY member a write persists on it, so the document a read
//!   returns is a complete input for the next write: feeding that document back through the
//!   write (changing one member, or changing none) leaves every other member exactly as it
//!   was. For a custom action the members both paths share are `Definition`, `AliasName` and
//!   `Attachments` (`smithy_query` shape=CustomAction, and the `CreateCustomAction` /
//!   `UpdateCustomAction` input shapes), so a get-change-update-get reconciliation can
//!   neither erase the chat-channel alias nor erase the notification buttons.
//! * a request addressing a resource by an ARN of the form this account's own resources are
//!   minted under, which the store holds nothing under, is refused with the operation's
//!   modelled not-found error — status, `x-amzn-errortype` and the message member its error
//!   shape declares — and never with the authorization failure, which is about the principal
//!   and not about the resource's absence. Each operation words its own error class, so the
//!   read's sentence and the write's differ.
//!
//! The tables span those classes rather than one recorded request: the reconciliation rows
//! differ in which optional members the action carries, in how many attachments, criteria and
//! variables they hold, in the criteria operator, and in the length and charset of the alias
//! — including the published bounds of those shapes — while the refusal rows differ in the
//! action name's length and charset and in whether the action never existed or was deleted.

use serde_json::{json, Map, Value};

use crate::wire::Emulator;

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins what a client actually sees.
const ACCOUNT: &str = "000000000000";

/// The modelled error an ARN of the minted form holding no custom action is refused with.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The command text a reconciliation row writes through: the one member it changes.
const RECONCILED_COMMAND_TEXT: &str = "s3 ls --recursive";

/// A custom action is account-scoped, so the ARN a create mints carries an EMPTY region
/// segment (`state::custom_action_arn`).
fn custom_action_arn(action_name: &str) -> String {
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}")
}

/// Create `request` and answer the `CustomActionArn` it reports.
async fn create_ok(emulator: &Emulator, request: &Value) -> String {
    let created = emulator.call("/create-custom-action", request).await;
    assert_eq!(
        created.status, 200,
        "creating {request}: expected 200, got {} with {}",
        created.status, created.body
    );
    created.body["CustomActionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {request}: no CustomActionArn in {}", created.body))
        .to_string()
}

/// Read the action `arn` back, asserting the read succeeded, and answer the `CustomAction`
/// it reports.
async fn get_ok(emulator: &Emulator, arn: &str) -> Value {
    let answer = emulator
        .call("/get-custom-action", &json!({"CustomActionArn": arn}))
        .await;
    assert_eq!(
        answer.status, 200,
        "reading {arn}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["CustomAction"].clone()
}

/// The update request a client reconciling the action builds FROM THE READ alone: every
/// member the read reports that the update also accepts is carried back, with the command
/// text replaced when `command_text` names a new one. Nothing here comes from the create, so
/// a member the read drops is a member the reconciliation cannot send.
fn reconciled_update(read: &Value, command_text: Option<&str>) -> Value {
    let mut request = Map::new();
    request.insert("CustomActionArn".to_string(), read["CustomActionArn"].clone());
    request.insert(
        "Definition".to_string(),
        match command_text {
            Some(text) => json!({"CommandText": text}),
            None => read["Definition"].clone(),
        },
    );
    // The optional members `CustomAction` shares with the update's input shape.
    for member in ["AliasName", "Attachments"] {
        if let Some(value) = read.get(member) {
            request.insert(member.to_string(), value.clone());
        }
    }
    Value::Object(request)
}

/// One reconciliation row: the create it starts from, and the `CustomAction` a read must
/// report for it — stated from the create's own members, not from what the read answered, so
/// a read that drops a member fails the row instead of defining it away.
struct Case {
    label: &'static str,
    create: Value,
    expected: Value,
}

/// The rows, each differing along what the read-projection rule generalises over: which
/// optional members the action carries, how many attachments it defines, how many criteria
/// and variables each attachment holds, which criteria operator it names, and the length and
/// charset of the alias.
///
/// The operators are the model's whole published
/// `CustomActionAttachmentCriteriaOperator` value set (`smithy_query`: `HAS_VALUE`,
/// `EQUALS`); the maxima the rows sit on are the published ones of
/// `CustomActionAttachmentCriteriaList` and `CustomActionAttachmentVariables` (len 1..5) and
/// of `CustomActionAliasName` (len 1..30, pattern `^[A-Za-z0-9-_]+$`).
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no optional members: nothing to lose, nothing to invent",
            create: json!({
                "ActionName": "required-only",
                "Definition": {"CommandText": "ssm start-session"},
            }),
            expected: json!({
                "CustomActionArn": custom_action_arn("required-only"),
                "ActionName": "required-only",
                "Definition": {"CommandText": "ssm start-session"},
            }),
        },
        Case {
            label: "the shortest allowed alias and no attachments",
            create: json!({
                "ActionName": "short-alias",
                "Definition": {"CommandText": "s3 ls"},
                "AliasName": "a",
            }),
            expected: json!({
                "CustomActionArn": custom_action_arn("short-alias"),
                "ActionName": "short-alias",
                "Definition": {"CommandText": "s3 ls"},
                "AliasName": "a",
            }),
        },
        Case {
            label: "the longest allowed alias, over the shape's whole charset",
            create: json!({
                "ActionName": "Long_Alias-9",
                "Definition": {"CommandText": "logs tail /aws/lambda/$Function"},
                "AliasName": "Ab9-_".repeat(6),
            }),
            expected: json!({
                "CustomActionArn": custom_action_arn("Long_Alias-9"),
                "ActionName": "Long_Alias-9",
                "Definition": {"CommandText": "logs tail /aws/lambda/$Function"},
                "AliasName": "Ab9-_".repeat(6),
            }),
        },
        Case {
            label: "an alias and one attachment with one HAS_VALUE criterion",
            create: json!({
                "ActionName": "one-attachment",
                "Definition": {"CommandText": "cloudwatch describe-alarms"},
                "AliasName": "alarms",
                "Attachments": [{
                    "NotificationType": "aws-cloudwatch-alarm",
                    "ButtonText": "Describe alarms",
                    "Criteria": [{"Operator": "HAS_VALUE", "VariableName": "AlarmName"}],
                    "Variables": {"AlarmName": "$.detail.alarmName"},
                }],
            }),
            expected: json!({
                "CustomActionArn": custom_action_arn("one-attachment"),
                "ActionName": "one-attachment",
                "Definition": {"CommandText": "cloudwatch describe-alarms"},
                "AliasName": "alarms",
                "Attachments": [{
                    "NotificationType": "aws-cloudwatch-alarm",
                    "ButtonText": "Describe alarms",
                    "Criteria": [{"Operator": "HAS_VALUE", "VariableName": "AlarmName"}],
                    "Variables": {"AlarmName": "$.detail.alarmName"},
                }],
            }),
        },
        Case {
            label: "attachments but no alias, with the criteria list and the variable map at \
                    their maxima",
            create: json!({
                "ActionName": "attachments-only",
                "Definition": {"CommandText": "codedeploy stop-deployment --deployment-id $Id"},
                "Attachments": [
                    {
                        "NotificationType": "aws-codedeploy-deployment",
                        "ButtonText": "Stop deployment",
                        "Criteria": [
                            {"Operator": "EQUALS", "VariableName": "State", "Value": "FAILED"},
                            {"Operator": "HAS_VALUE", "VariableName": "Id"},
                            {"Operator": "EQUALS", "VariableName": "Region", "Value": "us-west-2"},
                            {"Operator": "HAS_VALUE", "VariableName": "Account"},
                            {"Operator": "EQUALS", "VariableName": "App", "Value": "checkout"},
                        ],
                        "Variables": {
                            "State": "$.detail.state",
                            "Id": "$.detail.deploymentId",
                            "Region": "$.region",
                            "Account": "$.account",
                            "App": "$.detail.application",
                        },
                    },
                    {"ButtonText": "Open event"},
                ],
            }),
            expected: json!({
                "CustomActionArn": custom_action_arn("attachments-only"),
                "ActionName": "attachments-only",
                "Definition": {"CommandText": "codedeploy stop-deployment --deployment-id $Id"},
                "Attachments": [
                    {
                        "NotificationType": "aws-codedeploy-deployment",
                        "ButtonText": "Stop deployment",
                        "Criteria": [
                            {"Operator": "EQUALS", "VariableName": "State", "Value": "FAILED"},
                            {"Operator": "HAS_VALUE", "VariableName": "Id"},
                            {"Operator": "EQUALS", "VariableName": "Region", "Value": "us-west-2"},
                            {"Operator": "HAS_VALUE", "VariableName": "Account"},
                            {"Operator": "EQUALS", "VariableName": "App", "Value": "checkout"},
                        ],
                        "Variables": {
                            "State": "$.detail.state",
                            "Id": "$.detail.deploymentId",
                            "Region": "$.region",
                            "Account": "$.account",
                            "App": "$.detail.application",
                        },
                    },
                    {"ButtonText": "Open event"},
                ],
            }),
        },
        Case {
            label: "an alias and an EMPTY attachment list: the list is reported as supplied",
            create: json!({
                "ActionName": "no-buttons",
                "Definition": {"CommandText": "lambda invoke --function-name $Function"},
                "AliasName": "run-lambda",
                "Attachments": [],
            }),
            expected: json!({
                "CustomActionArn": custom_action_arn("no-buttons"),
                "ActionName": "no-buttons",
                "Definition": {"CommandText": "lambda invoke --function-name $Function"},
                "AliasName": "run-lambda",
                "Attachments": [],
            }),
        },
    ]
}

#[tokio::test]
async fn reconciling_through_the_document_the_read_returned_changes_only_what_it_changed() {
    for case in cases() {
        let emulator = Emulator::new();
        let arn = create_ok(&emulator, &case.create).await;

        // The read projects every member the create persisted, so the reconciliation below
        // starts from a complete document.
        let read = get_ok(&emulator, &arn).await;
        assert_eq!(
            read, case.expected,
            "case {:?}: the read must report what the create persisted, got {read}",
            case.label
        );

        // The document a reconciling client can build FROM THE READ therefore carries every
        // member the create supplied that the write also accepts — asserted on the request
        // itself, because the write is partial and would keep a stored member the request
        // leaves out, which would hide a read that dropped it.
        let carried_back = reconciled_update(&read, None);
        for member in ["AliasName", "Attachments"] {
            assert_eq!(
                carried_back.get(member),
                case.create.get(member),
                "case {:?}: a client reconciling from the read must be able to send {member} \
                 exactly as the create did, got {carried_back}",
                case.label
            );
        }

        // Writing that document back unchanged must leave the action as it was.
        let echoed = emulator
            .call("/update-custom-action", &carried_back)
            .await;
        assert_eq!(
            echoed.status, 200,
            "case {:?}: writing the read's own document back answers 200, got {} with {}",
            case.label, echoed.status, echoed.body
        );
        let after_echo = get_ok(&emulator, &arn).await;
        assert_eq!(
            after_echo, case.expected,
            "case {:?}: an unchanged reconciliation must leave the action as it was, \
             got {after_echo}",
            case.label
        );

        // The same loop changing ONE member: everything else the read carried survives.
        let changed = emulator
            .call(
                "/update-custom-action",
                &reconciled_update(&after_echo, Some(RECONCILED_COMMAND_TEXT)),
            )
            .await;
        assert_eq!(
            changed.status, 200,
            "case {:?}: the reconciling update answers 200, got {} with {}",
            case.label, changed.status, changed.body
        );
        let mut expected = case.expected.clone();
        expected["Definition"] = json!({"CommandText": RECONCILED_COMMAND_TEXT});
        let after_change = get_ok(&emulator, &arn).await;
        assert_eq!(
            after_change, expected,
            "case {:?}: reconciliation must change only the definition, got {after_change}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_write_to_an_action_this_account_does_not_hold_is_the_modelled_not_found() {
    let emulator = Emulator::new();

    // A name the store held and no longer holds is the same case as a name it never held:
    // what decides the refusal is whether the action is there, not how its name is spelled.
    let deleted_name = "deleted_action-1";
    let deleted = create_ok(
        &emulator,
        &json!({
            "ActionName": deleted_name,
            "Definition": {"CommandText": "s3 ls"},
            "AliasName": "buckets",
        }),
    )
    .await;
    let removed = emulator
        .call("/delete-custom-action", &json!({"CustomActionArn": deleted}))
        .await;
    assert_eq!(
        removed.status, 200,
        "deleting {deleted_name:?} answers 200, got {} with {}",
        removed.status, removed.body
    );

    // Every row is an ARN of the form this account's custom actions are minted under: the
    // rows differ in the action name's length and charset, and in whether the action never
    // existed or was deleted.
    let absent = [
        ("a name nothing was created under", "never-created".to_string()),
        ("the longest allowed name", "z".repeat(64)),
        ("a name the account held and deleted", deleted_name.to_string()),
    ];

    for (label, action_name) in &absent {
        let arn = custom_action_arn(action_name);
        let answer = emulator
            .call(
                "/update-custom-action",
                &json!({
                    "CustomActionArn": arn,
                    "Definition": {"CommandText": "s3 ls"},
                    "AliasName": "buckets",
                }),
            )
            .await;
        assert_eq!(
            answer.status, 404,
            "case {label:?} ({arn}): expected HTTP 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(RESOURCE_NOT_FOUND_EXCEPTION),
            "case {label:?} ({arn}): expected x-amzn-errortype {RESOURCE_NOT_FOUND_EXCEPTION}, \
             got {:?} with {}",
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.code(),
            RESOURCE_NOT_FOUND_EXCEPTION,
            "case {label:?} ({arn}): expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, \
             got {}",
            answer.body
        );
        let written = answer.message_capitalized();
        assert_eq!(
            written,
            format!("Sdk action with id [{action_name}] does not exist in account [{ACCOUNT}]"),
            "case {label:?} ({arn}): the write's not-found names the action off the ARN and \
             the serving account, got {written:?} in {}",
            answer.body
        );

        // The read is its OWN error class, so it words the same miss differently: one shared
        // sentence cannot serve both operations.
        let read = emulator
            .call("/get-custom-action", &json!({"CustomActionArn": arn}))
            .await;
        assert_eq!(
            read.status, 404,
            "case {label:?} ({arn}): a refused write persists nothing, so the read is 404 too, \
             got {} with {}",
            read.status, read.body
        );
        let reported = read.message_capitalized();
        assert_eq!(
            reported,
            format!("sdk action [{action_name}] not found for account [{ACCOUNT}]"),
            "case {label:?} ({arn}): the read's not-found sentence is its own, got \
             {reported:?} in {}",
            read.body
        );
        assert_ne!(
            written, reported,
            "case {label:?} ({arn}): the write and the read word their own error classes, \
             got {written:?} for both"
        );
    }
}
