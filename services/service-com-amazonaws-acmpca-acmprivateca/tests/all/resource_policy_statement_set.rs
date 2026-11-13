// SPDX-License-Identifier: CC0-1.0

//! The rule `PutPolicy` judges a SUBMITTED DOCUMENT by, over the whole document rather
//! than over one statement of it.
//!
//! "If this policy does not contain all required statements or if it includes any statement
//! that is not allowed, the PutPolicy action returns an InvalidPolicyException"
//! (`PutPolicyRequest$Policy` docs). Two consequences are under test here, each over a
//! table:
//!
//! 1. EVERY statement of the document is judged, so a document is refused when ANY one of
//!    its statements grants an action a certificate authority cannot be shared for, or
//!    names an identity that is not here, or is about another resource — wherever that
//!    statement sits in the document and whether `Statement` carries one statement object
//!    or a list of them. The rows differ along statement count and the offending
//!    statement's position; the accepted rows are documents every statement of which is
//!    attachable.
//! 2. The document is judged BEFORE anything is written, so a refused document leaves the
//!    policy the CA already carried exactly as it was — which `GetPolicy` is what reports.
//!    The rows span each kind of refusal: the grammar, the action, the principal and the
//!    resource.
//!
//! The per-statement catalogs themselves (which actions and which principal forms resolve)
//! are tabled in `resource_policy_rules.rs`; this suite is about the document the
//! statements sit in. Every call goes over the wire through the harness in
//! [`super::private_ca_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    different_resource_message, DISALLOWED_POLICY_ACTION_MESSAGE, INVALID_POLICY_MESSAGE,
    POLICY_ALLOWED_ACTIONS, POLICY_NOT_FOUND_MESSAGE, POLICY_PRINCIPAL_NOT_FOUND_MESSAGE,
};

use crate::private_ca_lifecycle::{root_ca_request, string, Emulator, ACCOUNT, REGION};
use crate::resource_policy::error_message;

/// A private CA created over the wire, named after `label`.
///
/// No wait for the clock: the policy operations are not gated on a CA's lifecycle status,
/// so each row below is indifferent to which status the authority has reached.
async fn certificate_authority(emulator: &Emulator, label: &str) -> String {
    let created = emulator
        .ok(label, "CreateCertificateAuthority", &root_ca_request(label))
        .await;
    string(&created, "CertificateAuthorityArn", label)
}

/// A policy document carrying `statement` as its `Statement` element — one statement
/// object, or a list of them, exactly as submitted.
fn document(statement: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statement}).to_string()
}

/// One statement of a policy document: it grants `action` to `principal` on `resource`.
fn statement(sid: &str, principal: Value, action: Value, resource: Value) -> Value {
    json!({
        "Sid": sid,
        "Effect": "Allow",
        "Principal": principal,
        "Action": action,
        "Resource": resource
    })
}

/// A statement this service attaches: an allow-listed action granted to the one account
/// this emulator presents, on the CA the policy hangs on.
fn attachable_statement(sid: &str, ca_arn: &str) -> Value {
    statement(
        sid,
        json!({"AWS": ACCOUNT}),
        json!(format!("acm-pca:{}", POLICY_ALLOWED_ACTIONS[0])),
        json!(ca_arn),
    )
}

/// A statement granting a management-plane action a certificate authority's policy cannot
/// grant.
fn disallowed_action_statement(sid: &str, ca_arn: &str) -> Value {
    statement(
        sid,
        json!({"AWS": ACCOUNT}),
        json!("acm-pca:DeleteCertificateAuthority"),
        json!(ca_arn),
    )
}

/// A statement granting to an account id this emulator does not present.
fn absent_principal_statement(sid: &str, ca_arn: &str) -> Value {
    statement(
        sid,
        json!({"AWS": "999999999999"}),
        json!(format!("acm-pca:{}", POLICY_ALLOWED_ACTIONS[0])),
        json!(ca_arn),
    )
}

/// A statement about a certificate authority other than the one the policy hangs on.
fn other_resource_statement(sid: &str) -> Value {
    statement(
        sid,
        json!({"AWS": ACCOUNT}),
        json!(format!("acm-pca:{}", POLICY_ALLOWED_ACTIONS[0])),
        json!(format!(
            "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
             11111111-1111-1111-1111-111111111111"
        )),
    )
}

/// The document `ca_arn` carries, as `GetPolicy` displays it, or `None` when it carries
/// none — the read reports an absent policy with the POLICY's own sentence.
async fn carried_policy(emulator: &Emulator, case: &str, ca_arn: &str) -> Option<String> {
    let (status, body) = emulator
        .call("GetPolicy", &json!({"ResourceArn": ca_arn}))
        .await;
    match status {
        200 => Some(string(&body, "Policy", case)),
        400 => {
            let message = error_message(&body, case);
            assert_eq!(
                message, POLICY_NOT_FOUND_MESSAGE,
                "case {case:?}: expected GetPolicy to report the policy as the absent \
                 resource, got {message:?}"
            );
            None
        }
        other => panic!("case {case:?}: GetPolicy answered {other} {body:?}"),
    }
}

/// Rule 1: whichever statement of the document is not one this service attaches, and
/// wherever it sits, the whole document is refused and nothing is attached; a document
/// every statement of which is attachable is accepted and read back.
#[tokio::test]
async fn put_policy_judges_every_statement_of_the_document() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "statement-set").await;

    // The rows differ along statement count, the form `Statement` takes (one object versus
    // a list) and WHERE the offending statement sits — first, middle, last — so no row's
    // outcome can come from reading only the document's first statement.
    let refused: Vec<(&str, Value, String)> = vec![
        (
            "one statement object granting a disallowed action",
            disallowed_action_statement("only", &arn),
            DISALLOWED_POLICY_ACTION_MESSAGE.to_string(),
        ),
        (
            "one statement object naming an account that is not here",
            absent_principal_statement("only", &arn),
            POLICY_PRINCIPAL_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "the first of two statements granting a disallowed action",
            json!([
                disallowed_action_statement("first", &arn),
                attachable_statement("second", &arn)
            ]),
            DISALLOWED_POLICY_ACTION_MESSAGE.to_string(),
        ),
        (
            "the second of two statements granting a disallowed action",
            json!([
                attachable_statement("first", &arn),
                disallowed_action_statement("second", &arn)
            ]),
            DISALLOWED_POLICY_ACTION_MESSAGE.to_string(),
        ),
        (
            "the middle of three statements naming an account that is not here",
            json!([
                attachable_statement("first", &arn),
                absent_principal_statement("second", &arn),
                attachable_statement("third", &arn)
            ]),
            POLICY_PRINCIPAL_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "the last of four statements naming an account that is not here",
            json!([
                attachable_statement("first", &arn),
                attachable_statement("second", &arn),
                attachable_statement("third", &arn),
                absent_principal_statement("fourth", &arn)
            ]),
            POLICY_PRINCIPAL_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "the last of three statements about another authority",
            json!([
                attachable_statement("first", &arn),
                attachable_statement("second", &arn),
                other_resource_statement("third")
            ]),
            different_resource_message(&arn),
        ),
    ];

    for (case, statements, expected) in &refused {
        let submitted = document(statements.clone());
        let body = emulator
            .refused(
                case,
                "PutPolicy",
                &json!({"ResourceArn": arn, "Policy": submitted}),
                "InvalidPolicyException",
            )
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            &message, expected,
            "case {case:?}: expected the refusal the offending statement earns, got \
             {message:?}"
        );
        let carried = carried_policy(&emulator, case, &arn).await;
        assert!(
            carried.is_none(),
            "case {case:?}: expected the refused document to attach nothing, got \
             {carried:?}"
        );
    }

    // Documents every statement of which is attachable: one statement object, several
    // statements, and a statement naming the CA it hangs on in a list of resources.
    let accepted: Vec<(&str, Value)> = vec![
        ("one statement object", attachable_statement("only", &arn)),
        (
            "one statement in a list",
            json!([attachable_statement("only", &arn)]),
        ),
        (
            "four statements, each of them attachable",
            json!([
                attachable_statement("first", &arn),
                attachable_statement("second", &arn),
                attachable_statement("third", &arn),
                attachable_statement("fourth", &arn)
            ]),
        ),
        (
            "a statement whose Resource is a list naming the CA",
            statement(
                "resource-list",
                json!({"AWS": ACCOUNT}),
                json!(format!("acm-pca:{}", POLICY_ALLOWED_ACTIONS[0])),
                json!([arn, arn]),
            ),
        ),
    ];

    for (case, statements) in &accepted {
        let submitted = document(statements.clone());
        emulator
            .ok(
                case,
                "PutPolicy",
                &json!({"ResourceArn": arn, "Policy": submitted}),
            )
            .await;
        let carried = carried_policy(&emulator, case, &arn).await;
        assert_eq!(
            carried.as_deref(),
            Some(submitted.as_str()),
            "case {case:?}: expected the attached document back, got {carried:?}"
        );
        // Cleared again, so the next row is observably its own attach rather than the one
        // before it.
        emulator
            .ok(case, "DeletePolicy", &json!({"ResourceArn": arn}))
            .await;
    }
}

/// Rule 2: the document is judged before any state is written, so each refusal leaves the
/// policy the CA already carried exactly as it was — and the delete that follows removes
/// that one policy.
#[tokio::test]
async fn a_refused_document_leaves_the_carried_policy_untouched() {
    let emulator = Emulator::new();
    let case = "a CA already carrying a policy";
    let arn = certificate_authority(&emulator, "refusal-leaves-policy").await;

    let carried = document(json!([
        attachable_statement("first", &arn),
        statement(
            "second",
            json!({"Service": "acm.amazonaws.com"}),
            json!(format!("acm-pca:{}", POLICY_ALLOWED_ACTIONS[1])),
            json!(arn),
        )
    ]));
    emulator
        .ok(
            case,
            "PutPolicy",
            &json!({"ResourceArn": arn, "Policy": carried}),
        )
        .await;
    assert_eq!(
        carried_policy(&emulator, case, &arn).await.as_deref(),
        Some(carried.as_str()),
        "case {case:?}: expected the CA to carry the document it was given"
    );

    // One row per kind of refusal: the document is not a policy at all, its statements are
    // not statements, it grants an action a CA cannot be shared for, it names an identity
    // that is not here, and it is about another authority.
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a document that is not JSON",
            "not a policy at all".to_string(),
            INVALID_POLICY_MESSAGE.to_string(),
        ),
        (
            "a document carrying no statement",
            json!({"Version": "2012-10-17"}).to_string(),
            INVALID_POLICY_MESSAGE.to_string(),
        ),
        (
            "a document whose statement list is empty",
            document(json!([])),
            INVALID_POLICY_MESSAGE.to_string(),
        ),
        (
            "a statement with no Effect",
            document(json!({
                "Sid": "no-effect",
                "Principal": {"AWS": ACCOUNT},
                "Action": format!("acm-pca:{}", POLICY_ALLOWED_ACTIONS[0]),
                "Resource": arn
            })),
            INVALID_POLICY_MESSAGE.to_string(),
        ),
        (
            "a statement granting a disallowed action",
            document(disallowed_action_statement("disallowed", &arn)),
            DISALLOWED_POLICY_ACTION_MESSAGE.to_string(),
        ),
        (
            "a statement naming an account that is not here",
            document(absent_principal_statement("absent", &arn)),
            POLICY_PRINCIPAL_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a statement about another authority",
            document(other_resource_statement("elsewhere")),
            different_resource_message(&arn),
        ),
    ];

    for (row, submitted, expected) in &refused {
        let body = emulator
            .refused(
                row,
                "PutPolicy",
                &json!({"ResourceArn": arn, "Policy": submitted}),
                "InvalidPolicyException",
            )
            .await;
        let message = error_message(&body, row);
        assert_eq!(
            &message, expected,
            "case {row:?}: expected the refusal that kind of document earns, got {message:?}"
        );
        let still = carried_policy(&emulator, row, &arn).await;
        assert_eq!(
            still.as_deref(),
            Some(carried.as_str()),
            "case {row:?}: expected the refusal to leave the carried policy as it was, got \
             {still:?}"
        );
    }

    // The policy the refusals left alone is the one the delete removes.
    emulator
        .ok(case, "DeletePolicy", &json!({"ResourceArn": arn}))
        .await;
    let gone = carried_policy(&emulator, case, &arn).await;
    assert!(
        gone.is_none(),
        "case {case:?}: expected the delete to remove the carried policy, got {gone:?}"
    );
}
