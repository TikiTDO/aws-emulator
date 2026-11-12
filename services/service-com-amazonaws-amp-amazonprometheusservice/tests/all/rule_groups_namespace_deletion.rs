// SPDX-License-Identifier: CC0-1.0

//! `DeleteRuleGroupsNamespace`, end to end over HTTP.
//!
//! The rule under test: a deletion is ACCEPTED only for a namespace the named workspace
//! actually holds and only while both the namespace and its workspace are `ACTIVE`; it
//! then marks that persisted record `DELETING` and the clock — nothing else — releases
//! the record and the rules groups definition it holds when the transition comes due.
//! Everything else is a modelled refusal that changes nothing: a workspace or a name the
//! store does not hold is `ResourceNotFoundException`, a namespace or workspace in any
//! other status is `ConflictException`, and replaying the `clientToken` of the deletion
//! already in flight is that same call rather than a conflict.
//!
//! Every suite spans what that rule generalises over — the namespace name across its
//! modelled length bounds and character class, the bytes of the rules file (text, one
//! byte, none at all, kilobytes of it), the tag count, whether the delete carried a
//! `clientToken` and in which form, the status the namespace or workspace is in, and the
//! signing region — rather than one recorded request's values.
//!
//! The request harness (`Amp`, `Reply`) and the create/describe/list calls are the ones
//! `workspace_lifecycle`, `rule_groups_namespace_lifecycle` and
//! `rule_groups_namespace_listing` already build.

use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace, describe_namespace};
use crate::rule_groups_namespace_listing::{list_namespaces, names_of};
use crate::rule_groups_namespace_update::{blob, put_body, put_namespace};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    NAMESPACE_STATUS_ACTIVE, NAMESPACE_STATUS_CREATING, NAMESPACE_STATUS_DELETING,
    NAMESPACE_STATUS_UPDATING,
};

/// `DELETE /workspaces/{workspaceId}/rulegroupsnamespaces/{name}` —
/// DeleteRuleGroupsNamespace. `clientToken` is an `@httpQuery` member, so it rides in the
/// query string when the caller sends one.
async fn delete_namespace(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    name: &str,
    client_token: Option<&str>,
) -> Reply {
    let mut uri = format!("/workspaces/{workspace_id}/rulegroupsnamespaces/{name}");
    if let Some(token) = client_token {
        uri.push_str(&format!("?clientToken={token}"));
    }
    amp.call("DELETE", &uri, region, "").await
}

/// Create a namespace and let the clock make it `ACTIVE` — the one status at which the
/// model allows a deletion.
async fn active_namespace(
    case: &str,
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    name: &str,
    data: &[u8],
) {
    let created = create_namespace(amp, region, workspace_id, &create_body(name, data, &[])).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: the create must answer 202, got {created:?}"
    );
    wait_for_due_transition();
}

/// One row of the deletion table: the namespace, the rules file it holds, the tags it
/// carries and the `clientToken` (if any) the delete presents.
struct Case {
    case: &'static str,
    name: String,
    data: Vec<u8>,
    tags: Vec<(String, String)>,
    client_token: Option<String>,
}

/// The table spans what the rule generalises over: the namespace name at each modelled
/// `RuleGroupsNamespaceName` bound (length 1..128) and across its character class
/// (`[0-9A-Za-z][-.0-9A-Z_a-z]*`), the bytes of the rules file the deletion releases with
/// the record, the tag count, and whether the delete carries a `clientToken` — and, when
/// it does, each form the modelled `IdempotencyToken` accepts (length 1..64, pattern
/// `^[!-~]+$`).
fn deletion_cases() -> Vec<Case> {
    vec![
        Case {
            case: "a prometheus rules file, no clientToken",
            name: "rulesfile1".to_string(),
            data: b"groups:\n  - name: cpu\n    rules:\n      - record: job:cpu\n        expr: sum(rate(cpu[5m]))\n".to_vec(),
            tags: vec![("team".to_string(), "observability".to_string())],
            client_token: None,
        },
        Case {
            case: "name at the length minimum of 1, single-character token",
            name: "a".to_string(),
            data: b"g".to_vec(),
            tags: Vec::new(),
            client_token: Some("t".to_string()),
        },
        Case {
            case: "name at the length maximum of 128, token at its maximum of 64",
            name: format!("n{}", "0".repeat(127)),
            data: (0..4096u32).map(|i| (i % 251) as u8).collect(),
            tags: (0..5)
                .map(|i| (format!("key-{i}"), format!("value-{i}")))
                .collect(),
            client_token: Some("z".repeat(64)),
        },
        Case {
            case: "name carrying every punctuation the pattern allows, punctuation token",
            name: "A1-b.c_D".to_string(),
            data: (0..=255u8).collect(),
            tags: vec![("env".to_string(), "prod".to_string())],
            client_token: Some("!~#$%&*+/=?^_`{|}".to_string()),
        },
        Case {
            case: "an empty rules file, uuid-shaped token",
            name: "empty".to_string(),
            data: Vec::new(),
            tags: Vec::new(),
            client_token: Some("7f1c9a60-2b4e-4a51-9f2f-8c7d6e5b4a30".to_string()),
        },
    ]
}

/// A delete of an `ACTIVE` namespace is accepted (modelled 202), reports the namespace
/// `DELETING` on the record a reader sees straight after, and the clock — nothing else —
/// releases that record, after which the namespace is gone from a describe and from the
/// listing, and a second delete is a missing resource.
#[tokio::test]
async fn delete_marks_the_namespace_deleting_then_the_clock_releases_it() {
    let amp = Amp::new();
    let cases = deletion_cases();

    for region in ["us-west-2", "eu-central-1"] {
        let workspace_id = amp.active_workspace(region, "deletion-host").await;

        // One workspace holds every row, so the listing assertions below also prove the
        // deletion releases ONLY the named record.
        for case in &cases {
            let label = format!("{region} / {}", case.case);
            let created = create_namespace(
                &amp,
                region,
                &workspace_id,
                &create_body(&case.name, &case.data, &case.tags),
            )
            .await;
            assert_eq!(
                created.status, 202,
                "case {label:?}: the create must answer 202, got {created:?}"
            );
        }
        wait_for_due_transition();

        for case in &cases {
            let label = format!("{region} / {}", case.case);

            let deleted = delete_namespace(
                &amp,
                region,
                &workspace_id,
                &case.name,
                case.client_token.as_deref(),
            )
            .await;
            assert_eq!(
                deleted.status, 202,
                "case {label:?}: DeleteRuleGroupsNamespace is modelled 202, got {deleted:?}"
            );

            // The record is still there, reported DELETING, with the rules file it holds
            // untouched: nothing but the clock releases it.
            let pending = describe_namespace(&amp, region, &workspace_id, &case.name).await;
            assert_eq!(
                pending.status, 200,
                "case {label:?}: the namespace is still stored while DELETING, got {pending:?}"
            );
            let namespace = &pending.body["ruleGroupsNamespace"];
            assert_eq!(
                text(&namespace["status"]["statusCode"]),
                NAMESPACE_STATUS_DELETING,
                "case {label:?}: an accepted delete reports DELETING, got {pending:?}"
            );
            assert_eq!(
                namespace["status"]["statusReason"].as_str(),
                Some(""),
                "case {label:?}: nothing failed, so the describe reports the empty reason \
                 the service sends, got {pending:?}"
            );
            assert_eq!(
                blob(&label, &namespace["data"]),
                case.data,
                "case {label:?}: a pending deletion must not change the stored rules file"
            );
            let created_at = namespace["createdAt"].as_f64().unwrap_or_default();
            let modified_at = namespace["modifiedAt"].as_f64().unwrap_or_default();
            assert!(
                modified_at >= created_at,
                "case {label:?}: the delete changed the record, so modifiedAt {modified_at} \
                 must not precede createdAt {created_at}"
            );

            // And the listing still carries it, because the record still exists.
            let listed = list_namespaces(&amp, region, &workspace_id, &[]).await;
            assert!(
                names_of(&label, &listed).contains(&case.name),
                "case {label:?}: a namespace that is still stored must still be listed, \
                 got {listed:?}"
            );
        }

        // Only time makes the removals due.
        wait_for_due_transition();

        let listed = list_namespaces(&amp, region, &workspace_id, &[]).await;
        assert_eq!(
            names_of("after the releases", &listed),
            Vec::<String>::new(),
            "case {region:?}: every released namespace must be gone from the listing, \
             got {listed:?}"
        );

        for case in &cases {
            let label = format!("{region} / {}", case.case);
            let gone = describe_namespace(&amp, region, &workspace_id, &case.name).await;
            assert_eq!(
                gone.status, 404,
                "case {label:?}: a released namespace must be gone, got {gone:?}"
            );
            assert_eq!(
                gone.error_code(),
                "ResourceNotFoundException",
                "case {label:?}: expected ResourceNotFoundException, got {gone:?}"
            );

            // A second delete of a record nothing holds is a missing resource, not a
            // second acceptance.
            let again = delete_namespace(
                &amp,
                region,
                &workspace_id,
                &case.name,
                case.client_token.as_deref(),
            )
            .await;
            assert_eq!(
                again.status, 404,
                "case {label:?}: deleting a released namespace must answer 404, got {again:?}"
            );
            assert_eq!(
                again.error_code(),
                "ResourceNotFoundException",
                "case {label:?}: expected ResourceNotFoundException, got {again:?}"
            );
        }
    }
}

/// `clientToken` carries `@idempotencyToken` on an `@idempotent` operation: replaying the
/// deletion already in flight is that same call and is accepted again, while a DIFFERENT
/// token naming a namespace that is already `DELETING` is a conflict. One row per form of
/// token the modelled `IdempotencyToken` accepts.
#[tokio::test]
async fn replaying_the_delete_token_is_accepted_and_another_token_conflicts() {
    let amp = Amp::new();
    let region = "us-east-2";
    let workspace_id = amp.active_workspace(region, "idempotent-delete").await;

    let tokens = [
        ("single character, the length minimum", "t".to_string()),
        (
            "a uuid-shaped token",
            "1b2c3d4e-5f60-4a71-8b92-0c1d2e3f4a5b".to_string(),
        ),
        ("64 characters, the length maximum", "y".repeat(64)),
        ("printable punctuation", "!~#$%&*+/=?^_`{|}".to_string()),
    ];

    for (index, (case, token)) in tokens.iter().enumerate() {
        let name = format!("ns{index}");
        let data = format!("groups: [{{name: {name}}}]").into_bytes();
        active_namespace(case, &amp, region, &workspace_id, &name, &data).await;

        let first = delete_namespace(&amp, region, &workspace_id, &name, Some(token)).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first delete must answer 202, got {first:?}"
        );

        let replay = delete_namespace(&amp, region, &workspace_id, &name, Some(token)).await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: replaying the token must answer 202, got {replay:?}"
        );

        // The replay started nothing new: the record is still the one DELETING, with its
        // rules file intact.
        let pending = describe_namespace(&amp, region, &workspace_id, &name).await;
        assert_eq!(
            text(&pending.body["ruleGroupsNamespace"]["status"]["statusCode"]),
            NAMESPACE_STATUS_DELETING,
            "case {case:?}: the replay must leave the namespace DELETING, got {pending:?}"
        );
        assert_eq!(
            blob(case, &pending.body["ruleGroupsNamespace"]["data"]),
            data,
            "case {case:?}: the replay must not touch the stored rules file"
        );

        // A different token is a different call, and a namespace that is already being
        // deleted cannot take another deletion.
        let other =
            delete_namespace(&amp, region, &workspace_id, &name, Some(&format!("other-{index}")))
                .await;
        assert_eq!(
            other.status, 409,
            "case {case:?}: another token on a DELETING namespace must conflict, got {other:?}"
        );
        assert_eq!(
            other.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {other:?}"
        );

        // And a delete with no token at all is likewise not the call in flight.
        let untokened = delete_namespace(&amp, region, &workspace_id, &name, None).await;
        assert_eq!(
            untokened.status, 409,
            "case {case:?}: a delete with no token on a DELETING namespace must conflict, \
             got {untokened:?}"
        );

        // Once the clock has released the record, the very same token names nothing.
        wait_for_due_transition();
        let after = delete_namespace(&amp, region, &workspace_id, &name, Some(token)).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: the token must not resurrect a released namespace, got {after:?}"
        );
    }
}

/// Each refusal the operation models, one row per class — and every one leaves the store
/// exactly as it was.
#[tokio::test]
async fn refusals_answer_the_modelled_error_for_each_class() {
    let amp = Amp::new();
    let region = "ap-south-1";

    // A workspace id of the shape the service mints that no workspace holds.
    let absent_workspace = "ws-00000000-0000-0000-0000-000000000000";
    let absent = delete_namespace(&amp, region, absent_workspace, "orphan", None).await;
    assert_eq!(
        absent.status, 404,
        "case \"workspace does not exist\": expected 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code(),
        "ResourceNotFoundException",
        "case \"workspace does not exist\": expected ResourceNotFoundException, got {absent:?}"
    );

    let workspace_id = amp.active_workspace(region, "refusal-host").await;

    let missing = delete_namespace(&amp, region, &workspace_id, "never-created", None).await;
    assert_eq!(
        missing.status, 404,
        "case \"namespace does not exist\": expected 404, got {missing:?}"
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "case \"namespace does not exist\": expected ResourceNotFoundException, got {missing:?}"
    );

    // A namespace still CREATING: "Update/Deletion is disallowed until namespace is
    // ACTIVE and workspace status is ACTIVE."
    let creating_data = b"groups: [{name: creating}]".to_vec();
    let created = create_namespace(
        &amp,
        region,
        &workspace_id,
        &create_body("creating", &creating_data, &[]),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"namespace still CREATING\": the create must answer 202, got {created:?}"
    );
    let too_early = delete_namespace(&amp, region, &workspace_id, "creating", None).await;
    assert_eq!(
        too_early.status, 409,
        "case \"namespace still CREATING\": expected 409, got {too_early:?}"
    );
    assert_eq!(
        too_early.error_code(),
        "ConflictException",
        "case \"namespace still CREATING\": expected ConflictException, got {too_early:?}"
    );
    let survived = describe_namespace(&amp, region, &workspace_id, "creating").await;
    assert_eq!(
        text(&survived.body["ruleGroupsNamespace"]["status"]["statusCode"]),
        NAMESPACE_STATUS_CREATING,
        "case \"namespace still CREATING\": a refused delete must leave the status alone, \
         got {survived:?}"
    );
    assert_eq!(
        blob("namespace still CREATING", &survived.body["ruleGroupsNamespace"]["data"]),
        creating_data,
        "case \"namespace still CREATING\": a refused delete must not touch the rules file"
    );

    // A namespace still settling an update is in the same position.
    wait_for_due_transition();
    let updating_data = b"groups: [{name: updated}]".to_vec();
    let updated = put_namespace(
        &amp,
        region,
        &workspace_id,
        "creating",
        &put_body(&updating_data),
    )
    .await;
    assert_eq!(
        updated.status, 202,
        "case \"namespace UPDATING\": the update must answer 202, got {updated:?}"
    );
    let during_update = delete_namespace(&amp, region, &workspace_id, "creating", None).await;
    assert_eq!(
        during_update.status, 409,
        "case \"namespace UPDATING\": expected 409, got {during_update:?}"
    );
    assert_eq!(
        during_update.error_code(),
        "ConflictException",
        "case \"namespace UPDATING\": expected ConflictException, got {during_update:?}"
    );
    let still_updating = describe_namespace(&amp, region, &workspace_id, "creating").await;
    assert_eq!(
        text(&still_updating.body["ruleGroupsNamespace"]["status"]["statusCode"]),
        NAMESPACE_STATUS_UPDATING,
        "case \"namespace UPDATING\": a refused delete must leave the status alone, \
         got {still_updating:?}"
    );

    // A workspace that is no longer usable: the namespace is ACTIVE, but the workspace it
    // lives in is being deleted, so the deletion is refused as well.
    let doomed_workspace = amp.active_workspace(region, "doomed-host").await;
    active_namespace(
        "workspace DELETING",
        &amp,
        region,
        &doomed_workspace,
        "orphaned",
        b"groups: [{name: orphaned}]",
    )
    .await;
    let workspace_delete = amp.delete(region, &doomed_workspace, None).await;
    assert_eq!(
        workspace_delete.status, 202,
        "case \"workspace DELETING\": the workspace delete must answer 202, got {workspace_delete:?}"
    );
    let while_workspace_deleting =
        delete_namespace(&amp, region, &doomed_workspace, "orphaned", None).await;
    assert_eq!(
        while_workspace_deleting.status, 409,
        "case \"workspace DELETING\": expected 409, got {while_workspace_deleting:?}"
    );
    assert_eq!(
        while_workspace_deleting.error_code(),
        "ConflictException",
        "case \"workspace DELETING\": expected ConflictException, got {while_workspace_deleting:?}"
    );
    let namespace_still_active =
        describe_namespace(&amp, region, &doomed_workspace, "orphaned").await;
    assert_eq!(
        text(&namespace_still_active.body["ruleGroupsNamespace"]["status"]["statusCode"]),
        NAMESPACE_STATUS_ACTIVE,
        "case \"workspace DELETING\": a refused delete must leave the namespace ACTIVE, \
         got {namespace_still_active:?}"
    );

    // Once the workspace itself is released, its namespaces go with it, so the delete
    // reports the missing WORKSPACE.
    wait_for_due_transition();
    let after_workspace_gone =
        delete_namespace(&amp, region, &doomed_workspace, "orphaned", None).await;
    assert_eq!(
        after_workspace_gone.status, 404,
        "case \"workspace released\": expected 404, got {after_workspace_gone:?}"
    );
    assert_eq!(
        after_workspace_gone.error_code(),
        "ResourceNotFoundException",
        "case \"workspace released\": expected ResourceNotFoundException, \
         got {after_workspace_gone:?}"
    );
}
