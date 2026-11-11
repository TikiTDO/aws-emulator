// SPDX-License-Identifier: CC0-1.0

//! How Clean Rooms WORDS an authorization failure, over every operation that answers one.
//!
//! The rule under test: an `AccessDeniedException` states the authorization decision in
//! its three parts — the calling principal's ARN, the IAM action (`cleanrooms:` plus the
//! operation name) and the ARN of the resource the operation addresses, or `*` when the
//! operation addresses no single resource because its identifiers all travel in the body.
//! One wording for the whole class, so a client that extracts the principal, the action
//! and the resource from the message gets all three whichever operation refused it.
//!
//! The table rows differ along what the rule generalises over: the operation (and so the
//! action and the resource it names), the HTTP binding the identifier arrives in (URI
//! label versus body), the reason the caller is unauthorized (an id no create minted, one
//! a delete removed, one owned by another account) and the kind of resource the decision
//! is taken on (a collaboration, a membership). Every request goes over the wire through
//! the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, collaboration_owned_elsewhere, error_code, error_message, minimal_collaboration,
    Emulator, REGION,
};

/// Well-formed `CollaborationIdentifier`/`UUID` values (the model's `UUID` form) that no
/// create in these tests mints.
const NEVER_MINTED: [&str; 2] = [
    "00000000-0000-0000-0000-000000000000",
    "0123456f-89ab-cdef-0123-456789abcdef",
];

/// The principal every emulated caller is authenticated as: the one IAM user of the
/// single account this emulator serves, which is the identity `sts:GetCallerIdentity`
/// reports for a credential that names no session.
fn caller_principal() -> String {
    format!("arn:aws:iam::{}:user/emulator", account_id())
}

/// The message the rule prescribes for `operation` on `resource`.
fn denial(operation: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: cleanrooms:{operation} on resource: {resource}",
        caller_principal()
    )
}

/// The `CollaborationArn` of `id` for the region these tests sign for.
fn collaboration_arn(id: &str) -> String {
    format!(
        "arn:aws:cleanrooms:{REGION}:{}:collaboration/{id}",
        account_id()
    )
}

/// One operation that resolves a `collaborationIdentifier`: how to call it, the IAM
/// action it names, and the resource its denial names.
struct CollaborationCall {
    operation: &'static str,
    method: &'static str,
    /// The path for a collaboration id.
    path: fn(&str) -> String,
    /// The body for a collaboration id, when the operation takes one.
    body: fn(&str) -> Option<Value>,
    /// Whether the denial names the collaboration's ARN (the identifier is a URI label)
    /// or `*` (the operation addresses no single resource).
    resource_scoped: bool,
}

fn collaboration_calls() -> Vec<CollaborationCall> {
    vec![
        CollaborationCall {
            operation: "GetCollaboration",
            method: "GET",
            path: |id| format!("/collaborations/{id}"),
            body: |_| None,
            resource_scoped: true,
        },
        CollaborationCall {
            operation: "CreateMembership",
            method: "POST",
            path: |_| "/memberships".to_string(),
            body: |id| {
                Some(json!({
                    "collaborationIdentifier": id,
                    "queryLogStatus": "ENABLED",
                }))
            },
            resource_scoped: false,
        },
        CollaborationCall {
            operation: "CreateCollaborationChangeRequest",
            method: "POST",
            path: |id| format!("/collaborations/{id}/changeRequests"),
            body: |_| {
                Some(json!({
                    "changes": [{
                        "specificationType": "MEMBER",
                        "specification": {
                            "member": {
                                "accountId": "111122223333",
                                "memberAbilities": [],
                                "displayName": "partner",
                            }
                        }
                    }]
                }))
            },
            resource_scoped: true,
        },
        CollaborationCall {
            operation: "GetCollaborationChangeRequest",
            method: "GET",
            path: |id| format!("/collaborations/{id}/changeRequests/{}", NEVER_MINTED[0]),
            body: |_| None,
            resource_scoped: true,
        },
        CollaborationCall {
            operation: "UpdateCollaborationChangeRequest",
            method: "PATCH",
            path: |id| format!("/collaborations/{id}/changeRequests/{}", NEVER_MINTED[0]),
            body: |_| Some(json!({ "action": "APPROVE" })),
            resource_scoped: true,
        },
        CollaborationCall {
            // The roster of a collaboration is its creator's, so a caller that cannot
            // reach the collaboration cannot remove a party from it either.
            operation: "DeleteMember",
            method: "DELETE",
            path: |id| format!("/collaborations/{id}/member/{}", account_id()),
            body: |_| None,
            resource_scoped: true,
        },
        CollaborationCall {
            operation: "GetSchema",
            method: "GET",
            path: |id| format!("/collaborations/{id}/schemas/relation_0"),
            body: |_| None,
            resource_scoped: true,
        },
    ]
}

/// Every way a collaboration identifier fails to resolve to a collaboration this caller
/// is authorized to: `(label, id)`.
async fn unauthorized_identifiers(emulator: &Emulator) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = NEVER_MINTED
        .iter()
        .map(|id| (format!("never minted {id}"), id.to_string()))
        .collect();

    // One a delete removed: authorization to a collaboration lives in the collaboration
    // and disappears with it.
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration("denied-deleted"))
        .await;
    assert_eq!(status, 200, "setup: CreateCollaboration failed: {created}");
    let deleted = created["collaboration"]["id"].as_str().unwrap().to_string();
    let (status, body) = emulator.delete_collaboration(&deleted).await;
    assert_eq!(
        status, 204,
        "setup: the owner's DeleteCollaboration must succeed, got {status} with {body}"
    );
    rows.push(("deleted".to_string(), deleted));

    // One stored but owned by another account, with this account no party to it: the
    // shared harness helper re-owns a created record, passing `None` for "not a party".
    rows.push((
        "owned by another account".to_string(),
        collaboration_owned_elsewhere(emulator, "denied-foreign", None).await,
    ));

    rows
}

#[tokio::test]
async fn every_collaboration_denial_names_the_principal_the_action_and_the_resource() {
    let emulator = Emulator::new();
    let identifiers = unauthorized_identifiers(&emulator).await;

    for call in collaboration_calls() {
        for (why, collaboration_id) in &identifiers {
            let label = format!("{} on a collaboration {why}", call.operation);
            let (status, body) = emulator
                .call(
                    call.method,
                    &(call.path)(collaboration_id),
                    (call.body)(collaboration_id),
                )
                .await;
            assert_eq!(
                status, 403,
                "case {label:?}: expected 403, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {label:?}: expected AccessDeniedException, got {body}"
            );
            let expected = if call.resource_scoped {
                denial(call.operation, &collaboration_arn(collaboration_id))
            } else {
                denial(call.operation, "*")
            };
            assert_eq!(
                error_message(&body),
                expected,
                "case {label:?}: expected the authorization-failure wording, got {:?}",
                error_message(&body)
            );
            assert_eq!(
                body["reason"].as_str(),
                Some("INSUFFICIENT_PERMISSIONS"),
                "case {label:?}: expected the modelled reason, got {:?}",
                body["reason"]
            );
        }
    }
}

#[tokio::test]
async fn delete_collaboration_denies_a_collaboration_another_account_owns() {
    // `DeleteCollaboration` "can only be called by the collaboration owner", so a
    // collaboration STORED under the id that another account created is the authorization
    // failure, worded like every other. It declares no 404 and is `@idempotent`, so an id
    // nothing is stored under — never minted, or already deleted — has no owner to refuse
    // on behalf of and keeps the delete's success.
    let emulator = Emulator::new();
    let foreign = collaboration_owned_elsewhere(&emulator, "delete-foreign", None).await;

    let (status, body) = emulator.delete_collaboration(&foreign).await;
    assert_eq!(
        status, 403,
        "case owned by another account: expected 403, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case owned by another account: expected AccessDeniedException, got {body}"
    );
    assert_eq!(
        error_message(&body),
        denial("DeleteCollaboration", &collaboration_arn(&foreign)),
        "case owned by another account: expected the authorization-failure wording, got {:?}",
        error_message(&body)
    );

    // The refusal changed nothing: the collaboration another account owns is still there.
    {
        let store = emulator.store();
        let store = store.lock();
        assert!(
            store.collaborations.contains_key(&foreign),
            "case owned by another account: a refused delete must remove nothing"
        );
    }

    let (status, created) = emulator
        .create_collaboration(minimal_collaboration("delete-own"))
        .await;
    assert_eq!(status, 200, "setup: CreateCollaboration failed: {created}");
    let own = created["collaboration"]["id"].as_str().unwrap().to_string();
    for (label, id) in [
        ("the caller's own collaboration", own.clone()),
        ("the same id again, now stored nowhere", own),
        ("an id no create minted", NEVER_MINTED[0].to_string()),
    ] {
        let (status, body) = emulator.delete_collaboration(&id).await;
        assert_eq!(
            status, 204,
            "case {label:?}: expected the idempotent 204, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_membership_scoped_denial_names_the_membership_it_was_taken_on() {
    // `StartProtectedQuery` refuses a caller that does not hold the ability the
    // collaboration grants for it; the operation binds the membership as its URI label,
    // so that membership is the resource the decision was taken on. Rows vary the
    // abilities the caller DOES hold, so the refusal is about the missing one and not
    // about holding none.
    for (row, abilities) in [
        vec!["CAN_RECEIVE_RESULTS"],
        vec!["CAN_RUN_JOB"],
        vec!["CAN_RECEIVE_RESULTS", "CAN_EXPORT_QUERY_ANALYSIS_LOG"],
    ]
    .into_iter()
    .enumerate()
    {
        let label = format!("holding {abilities:?}");
        let emulator = Emulator::new();
        let mut collaboration = minimal_collaboration(&format!("no-query-{row}"));
        collaboration["creatorMemberAbilities"] = json!(abilities);
        let (status, created) = emulator.create_collaboration(collaboration).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateCollaboration failed with {status}: {created}"
        );
        let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();

        let (status, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateMembership failed with {status}: {joined}"
        );
        let membership = joined["membership"].clone();
        let membership_id = membership["id"].as_str().unwrap().to_string();
        let membership_arn = membership["arn"].as_str().unwrap().to_string();

        let (status, body) = emulator
            .call(
                "POST",
                &format!("/memberships/{membership_id}/protectedQueries"),
                Some(json!({
                    "type": "SQL",
                    "sqlParameters": { "queryString": "SELECT COUNT(*) FROM orders" },
                })),
            )
            .await;
        assert_eq!(
            status, 403,
            "case {label:?}: expected 403, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {label:?}: expected AccessDeniedException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            denial("StartProtectedQuery", &membership_arn),
            "case {label:?}: the denial must name the membership it was taken on, got {:?}",
            error_message(&body)
        );
    }
}
