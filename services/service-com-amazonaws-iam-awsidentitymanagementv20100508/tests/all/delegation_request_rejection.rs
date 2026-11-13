// SPDX-License-Identifier: CC0-1.0

//! `RejectDelegationRequest`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket sees.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.
//!
//! The rules being asserted:
//!
//! * an identifier the account holds NO delegation request under references a resource
//!   entity that does not exist, so the call is refused with the modelled
//!   `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) whose message
//!   describes the request asked about, and nothing is filed under that identifier — the
//!   read that follows refuses it too;
//! * for a request the account HOLDS, the rejection takes it to the `REJECTED` state,
//!   records the explanation the call carried as the request's rejection reason and sets
//!   the expiry a rejected request carries, which is what the read of that identifier
//!   reports afterwards, leaving every member the rejection is not about as the account
//!   held it;
//! * a caller whose identity-based policies do not allow the action is refused, and a
//!   refused call writes nothing.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: the
//! identifier's alphabet and both published length bounds, whether the call carries an
//! explanation and what it is (both published length bounds, the published alphabet), the
//! state the request was in before the call — every value `stateType` declares — and what
//! the caller is permitted.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
    DelegationRequest, STATE_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::delegation_request_reads::{held_delegation_requests, seed};
use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router, router_with_engine,
};

/// The published bounds of `notesType`, the type `Notes` is carried through: "len 0..500"
/// (`smithy_query(query="operation", operation="RejectDelegationRequest")`).
const MIN_NOTES_CHARACTERS: usize = 0;
const MAX_NOTES_CHARACTERS: usize = 500;

/// The published bounds of `delegationRequestIdType`, the type the identifier is carried
/// through: "len 16..128, pattern `^[\w-]+$`" (same `smithy_query` call).
const MIN_ID_CHARACTERS: usize = 16;
const MAX_ID_CHARACTERS: usize = 128;

/// The state a request the account has just had filed for it carries when the filing named
/// no target account: "a request that can be associated by any account"
/// (`CreateDelegationRequest$OwnerAccountId`), one of the values `stateType` declares.
const STATE_UNASSIGNED: &str = "UNASSIGNED";

/// The state this operation takes a delegation request to: "rejects a delegation request,
/// denying the requested temporary access. Once a request is rejected, it cannot be
/// accepted or updated later"
/// (`smithy_query(query="doc", shape="RejectDelegationRequest")`), one of the values
/// `stateType` declares.
const STATE_REJECTED: &str = "REJECTED";

/// How long after the rejection a rejected request expires: "rejected requests expire
/// after 7 days" (same `smithy_query(query="doc")` call), in seconds.
const REJECTED_LIFETIME_SECONDS: i64 = 7 * 24 * 60 * 60;

/// The delegation request identifiers the not-found suite spans.
///
/// Source: `delegationRequestIdType` on `smithy_query(query="operation",
/// operation="RejectDelegationRequest")` — length 16..128, pattern `^[\w-]+$` — so the
/// rows carry the whole published alphabet (letters in both cases, digits, `_`, `-`) and
/// BOTH published length bounds.
fn delegation_request_ids() -> Vec<String> {
    vec![
        // The published minimum length.
        "0f1e2d3c4b5a6978".to_string(),
        // The published maximum length.
        "r".repeat(MAX_ID_CHARACTERS),
        // The whole published alphabet, in a mixed case.
        "Partner_Delegation-2v".to_string(),
        // All digits, with a leading run of zeroes: an identifier is a handle, not a
        // number.
        "00000000000000024680".to_string(),
        // Hyphen-separated, as a minted handle often reads.
        "4c5d-6e7f-8a9b-0c1d".to_string(),
    ]
}

/// The explanations a row supplies, and what the read must report afterwards.
///
/// Rows: no `Notes` member at all, both published length bounds of `notesType`, and a
/// value spanning the published alphabet
/// (`^[\u0009\u000A\u000D\u0020-\u007E\u00A1-\u00FF]*$`: tab, the printable ASCII range
/// and the Latin-1 range above `\u00A0`).
fn notes_rows() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("no explanation at all", None),
        (
            "the published minimum length",
            Some("n".repeat(MIN_NOTES_CHARACTERS)),
        ),
        (
            "a plain sentence",
            Some("the boundary the request asks past is not delegable".to_string()),
        ),
        (
            "the published maximum length",
            Some("n".repeat(MAX_NOTES_CHARACTERS)),
        ),
        (
            "the published alphabet",
            Some("caf\u{e9} \u{bf}no?\t ~!@#$%^&*()_+ \u{ff}".to_string()),
        ),
    ]
}

/// One `RejectDelegationRequest` call, presented as signed by `akid` when a row names one,
/// carrying `Notes` only when the row supplies them.
async fn reject(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    delegation_request_id: &str,
    notes: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("DelegationRequestId", delegation_request_id)];
    if let Some(notes) = notes {
        params.push(("Notes", notes));
    }
    call_signed_by(router, akid, "RejectDelegationRequest", &params).await
}

/// One `GetDelegationRequest` call for `delegation_request_id`: the implemented read the
/// round trip is proved through.
async fn get(router: &MultiServiceRouter, delegation_request_id: &str) -> (u16, String) {
    call(
        router,
        "GetDelegationRequest",
        &[("DelegationRequestId", delegation_request_id)],
    )
    .await
}

/// File a delegation request under the workflow id `workflow` and answer the identifier
/// IAM minted for it: the fixture every round-trip row starts from, driven through the
/// implemented `CreateDelegationRequest` rather than seeded, so what is rejected is a
/// record this emulator wrote.
async fn file(router: &MultiServiceRouter, case: &str, workflow: &str) -> String {
    let (status, body) = call(
        router,
        "CreateDelegationRequest",
        &[
            ("Description", "temporary access for a partner engagement"),
            (
                "NotificationChannel",
                "arn:aws:sns:us-east-1:123456789012:partner-updates",
            ),
            (
                "Permissions.PolicyTemplateArn",
                "arn:aws:iam::aws:policy-template/PartnerDropReadWrite",
            ),
            ("RequestorWorkflowId", workflow),
            // The published minimum `SessionDuration` (range 300..43200).
            ("SessionDuration", "300"),
            ("OnlySendByOwner", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateDelegationRequest must succeed, got {status}: {body}"
    );
    element(&body, "DelegationRequestId")
}

/// The members this operation does NOT write, which the read must report unchanged across
/// a rejection: everything the filing stored but the state, the rejection reason, the
/// expiry and the last-updated instant.
const UNTOUCHED_MEMBERS: &[&str] = &[
    "DelegationRequestId",
    "OwnerAccountId",
    "Description",
    "RequestMessage",
    "PermissionPolicy",
    "OwnerId",
    "ApproverId",
    "RequestorId",
    "RequestorName",
    "Notes",
    "RedirectUrl",
    "PolicyTemplateArn",
    "SessionDuration",
    "OnlySendByOwner",
    "CreateDate",
];

/// The instant the member `tag` reports, in seconds, and `None` when the response carries
/// no such member.
fn reported_instant(case: &str, body: &str, tag: &str) -> Option<i64> {
    let reported = elements(body, tag);
    assert!(
        reported.len() <= 1,
        "case {case:?}: expected at most one <{tag}>, got {reported:?}"
    );
    reported.first().map(|text| {
        DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected an ISO-8601 {tag}, got {text:?}: {error}")
            })
            .secs()
    })
}

/// RULE: an identifier the account holds no delegation request under references "a
/// resource entity that does not exist", so the rejection is refused with the modelled
/// `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) whose message describes
/// the request asked about — whether or not the call carried an explanation — and the
/// refusal files nothing under that identifier, so the read of it is refused the same way
/// afterwards.
#[tokio::test]
async fn an_identifier_the_account_holds_nothing_under_is_refused_as_a_missing_entity() {
    for delegation_request_id in delegation_request_ids() {
        let (router, engine) = router_with_engine();
        // A row where the account holds ANOTHER delegation request, so what is asserted is
        // the lookup of the identifier the request named rather than whether the account
        // holds any request at all.
        let held = DelegationRequest {
            delegation_request_id: Some("held0123456789ab".to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        };
        seed(&engine, &held);

        for (label, notes) in notes_rows() {
            let case = format!("{delegation_request_id} with {label}");
            let (status, body) =
                reject(&router, None, &delegation_request_id, notes.as_deref()).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {:?}: {body}",
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                format!("Delegation request not found: {delegation_request_id}"),
                "case {case:?}: the refusal must describe the delegation request asked \
                 about, got {:?}",
                element(&body, "Message")
            );
        }

        // The refused rejections filed nothing: the account holds exactly the request it
        // held before, and the read of the refused identifier is still a missing entity.
        assert_eq!(
            held_delegation_requests(&engine),
            vec!["held0123456789ab".to_string()],
            "case {delegation_request_id:?}: a refused rejection persists nothing, got {:?}",
            held_delegation_requests(&engine)
        );
        let (status, body) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 404,
            "case {delegation_request_id:?}: the read of the refused identifier must still \
             be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {delegation_request_id:?}: expected NoSuchEntity from the read, got \
             {:?}: {body}",
            error_code(&body)
        );
    }
}

/// RULE: the identifier a delegation request is held under is case-SENSITIVE — it is a
/// minted handle rather than an IAM entity name, so a spelling that differs only in case
/// names no request the account holds and is refused as a missing entity, while the
/// spelling the account does hold is served.
#[tokio::test]
async fn an_identifier_differing_only_in_case_is_a_missing_entity() {
    let (router, engine) = router_with_engine();
    let held = "CasedHandle-7788CD";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(held.to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        },
    );

    for case in [held.to_lowercase(), held.to_uppercase()] {
        let (status, body) = reject(&router, None, &case, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("Delegation request not found: {case}"),
            "case {case:?}: the refusal must quote the identifier the request carried, \
             got {:?}",
            element(&body, "Message")
        );
    }

    let (status, body) = reject(&router, None, held, None).await;
    assert_eq!(
        status, 200,
        "case {held:?}: expected the rejection to be served, got {status}: {body}"
    );
}

/// RULE: for a request the account holds, the rejection takes it to the `REJECTED` state,
/// records the explanation the call carried as the request's rejection reason and gives the
/// request the expiry a rejected one carries ("rejected requests expire after 7 days") —
/// which is what the read of that identifier reports afterwards, with the last-updated
/// instant moved and every member the rejection is not about left as the account held it.
#[tokio::test]
async fn a_rejection_is_what_the_read_reports() {
    let router = router();

    for (row, (label, notes)) in notes_rows().into_iter().enumerate() {
        let case = format!("rejection with {label}");
        let delegation_request_id = file(&router, &case, &format!("workflow-reject-{row}")).await;

        let (status, before) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the filed request must be readable, got {status}: {before}"
        );
        assert_eq!(
            element(&before, "State"),
            STATE_UNASSIGNED,
            "case {case:?}: a filing with no target account is unassigned, got {:?}",
            element(&before, "State")
        );
        let filed_at = reported_instant(&case, &before, "CreateDate")
            .unwrap_or_else(|| panic!("case {case:?}: the filed request must report a CreateDate"));

        let (status, body) = reject(&router, None, &delegation_request_id, notes.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rejection to be served, got {status}: {body}"
        );

        let (status, after) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rejected request must be readable, got {status}: {after}"
        );
        assert_eq!(
            element(&after, "State"),
            STATE_REJECTED,
            "case {case:?}: a rejected request reaches REJECTED, got {:?}",
            element(&after, "State")
        );
        // The explanation the call carried is what the read reports as the rejection
        // reason; a call carrying none leaves the reason the request had, which for a
        // freshly filed request is none at all.
        assert_eq!(
            elements(&after, "RejectionReason"),
            notes.iter().cloned().collect::<Vec<String>>(),
            "case {case:?}: expected the explanation the rejection carried, got {:?}: {after}",
            elements(&after, "RejectionReason")
        );
        let rejected_at = reported_instant(&case, &after, "UpdatedTime").unwrap_or_else(|| {
            panic!("case {case:?}: a rejected request reports an UpdatedTime")
        });
        assert!(
            rejected_at >= filed_at,
            "case {case:?}: expected the last-updated instant at or after the filing's \
             {filed_at}, got {rejected_at}"
        );
        let expires_at = reported_instant(&case, &after, "ExpirationTime").unwrap_or_else(|| {
            panic!("case {case:?}: a rejected request reports an ExpirationTime")
        });
        assert_eq!(
            expires_at,
            rejected_at + REJECTED_LIFETIME_SECONDS,
            "case {case:?}: expected a rejected request to expire \
             {REJECTED_LIFETIME_SECONDS} seconds after the rejection at {rejected_at}, \
             got {expires_at}"
        );
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                elements(&after, member),
                elements(&before, member),
                "case {case:?}: the rejection must leave {member} as the account held it, \
                 got {:?} after {:?}",
                elements(&after, member),
                elements(&before, member)
            );
        }
    }
}

/// RULE: the rejection reason a request carries is ONE value of `notesType`, so a later
/// rejection states it again rather than appending to it, and a rejection that carries no
/// explanation leaves the reason the account holds alone — while each of them records the
/// request as rejected again.
#[tokio::test]
async fn a_later_rejection_states_the_reason_again() {
    let router = router();
    let case = "repeated rejections";
    let delegation_request_id = file(&router, case, "workflow-reject-repeated").await;

    // Rows: a reason written onto a request that had none, a shorter replacement, the
    // published maximum, and a rejection carrying none at all — which leaves the previous
    // value standing.
    let longest = "n".repeat(MAX_NOTES_CHARACTERS);
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        (
            "the first reason",
            Some("the boundary is not delegable"),
            "the boundary is not delegable",
        ),
        ("a shorter replacement", Some("denied"), "denied"),
        (
            "the published maximum",
            Some(longest.as_str()),
            longest.as_str(),
        ),
        ("no explanation at all", None, longest.as_str()),
    ];

    for (label, carried, expected) in rows {
        let case = format!("{case}: {label}");
        let (status, body) = reject(&router, None, &delegation_request_id, carried).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rejection to be served, got {status}: {body}"
        );
        let (status, after) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rejected request must be readable, got {status}: {after}"
        );
        assert_eq!(
            elements(&after, "RejectionReason"),
            vec![expected.to_string()],
            "case {case:?}: expected the reason the account now holds, got {:?}",
            elements(&after, "RejectionReason")
        );
        assert_eq!(
            element(&after, "State"),
            STATE_REJECTED,
            "case {case:?}: a rejected request reaches REJECTED, got {:?}",
            element(&after, "State")
        );
    }
}

/// RULE: the rejection is what takes a delegation request to `REJECTED`, whatever state it
/// was in before — so the rows are every value `stateType` declares, read from the model's
/// own value set (`ops::types::STATE_TYPE_VALUES`) rather than from the states one workflow
/// happens to pass through. The notes `UpdateDelegationRequest` added are a different
/// member from the reason a rejection records, so the rejection leaves them alone.
#[tokio::test]
async fn a_rejection_takes_a_request_in_any_state_to_rejected() {
    for (row, state) in STATE_TYPE_VALUES.iter().enumerate() {
        let (router, engine) = router_with_engine();
        let delegation_request_id = format!("rejectfromstate{row:0>3}");
        seed(
            &engine,
            &DelegationRequest {
                delegation_request_id: Some(delegation_request_id.clone()),
                description: Some("a request in a state the rejection moves".to_string()),
                requestor_id: Some(account_id().to_string()),
                state: Some((*state).to_string()),
                notes: Some("the notes the request already carried".to_string()),
                ..DelegationRequest::default()
            },
        );

        let case = format!("a request in {state}");
        let (status, body) = reject(
            &router,
            None,
            &delegation_request_id,
            Some("denied after review"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rejection to be served, got {status}: {body}"
        );

        let (status, after) = get(&router, &delegation_request_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rejected request must be readable, got {status}: {after}"
        );
        assert_eq!(
            element(&after, "State"),
            STATE_REJECTED,
            "case {case:?}: a rejected request reaches REJECTED, got {:?}",
            element(&after, "State")
        );
        assert_eq!(
            elements(&after, "RejectionReason"),
            vec!["denied after review".to_string()],
            "case {case:?}: expected the explanation the rejection carried, got {:?}",
            elements(&after, "RejectionReason")
        );
        assert_eq!(
            elements(&after, "Notes"),
            vec!["the notes the request already carried".to_string()],
            "case {case:?}: the rejection must leave the notes alone, got {:?}",
            elements(&after, "Notes")
        );
        assert_eq!(
            element(&after, "Description"),
            "a request in a state the rejection moves",
            "case {case:?}: the rejection must leave the description alone, got {:?}",
            element(&after, "Description")
        );
    }
}

/// RULE: a value outside a bound its member declares is refused at the boundary with HTTP
/// 400 `ValidationError` naming the member and the bound — one past each published bound,
/// so the modelled refusal is what answers it rather than this operation's not-found
/// refusal, and nothing is written.
#[tokio::test]
async fn a_value_past_a_published_bound_is_refused_before_the_lookup() {
    let (router, engine) = router_with_engine();
    let delegation_request_id = "bounded9876543210";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(delegation_request_id.to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        },
    );

    let short_id = "s".repeat(MIN_ID_CHARACTERS - 1);
    let long_id = "l".repeat(MAX_ID_CHARACTERS + 1);
    let long_notes = "n".repeat(MAX_NOTES_CHARACTERS + 1);
    let rows: Vec<(&str, &str, Option<&str>, String)> = vec![
        (
            "an identifier one character under the published minimum",
            short_id.as_str(),
            None,
            format!(
                "1 validation error detected: Value at 'delegationRequestId' failed to \
                 satisfy constraint: Member must have length greater than or equal to \
                 {MIN_ID_CHARACTERS}"
            ),
        ),
        (
            "an identifier one character over the published maximum",
            long_id.as_str(),
            None,
            format!(
                "1 validation error detected: Value at 'delegationRequestId' failed to \
                 satisfy constraint: Member must have length less than or equal to \
                 {MAX_ID_CHARACTERS}"
            ),
        ),
        (
            "an explanation one character over the published maximum",
            delegation_request_id,
            Some(long_notes.as_str()),
            format!(
                "1 validation error detected: Value at 'notes' failed to satisfy \
                 constraint: Member must have length less than or equal to \
                 {MAX_NOTES_CHARACTERS}"
            ),
        ),
    ];

    for (case, id, notes, expected) in rows {
        let (status, body) = reject(&router, None, id, notes).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            element(&body, "Message")
        );
    }

    // The refused calls wrote nothing: the request the account holds is untouched.
    let (status, after) = get(&router, delegation_request_id).await;
    assert_eq!(
        status, 200,
        "the held request must still be readable, got {status}: {after}"
    );
    assert_eq!(
        element(&after, "State"),
        STATE_UNASSIGNED,
        "a refused call must not move the state, got {:?}",
        element(&after, "State")
    );
    assert!(
        elements(&after, "RejectionReason").is_empty(),
        "a refused call must not record a rejection reason, got {:?}",
        elements(&after, "RejectionReason")
    );
}

/// RULE: IAM decides the call against the caller's identity-based policies before it
/// touches the request, so a caller no policy allows the action for is refused with HTTP
/// 403 `AccessDenied` naming the principal, the action and the request — whether or not the
/// account holds that request — and the refused call writes nothing.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_call_is_refused() {
    let (router, engine) = router_with_engine();
    let held = "accessdenied24680";
    seed(
        &engine,
        &DelegationRequest {
            delegation_request_id: Some(held.to_string()),
            state: Some(STATE_UNASSIGNED.to_string()),
            ..DelegationRequest::default()
        },
    );

    // Rows: a caller whose only grant is for ANOTHER action, and one explicitly denied this
    // action by a second statement.
    let rows = vec![
        (
            "no allow at all",
            vec![(
                "unrelated".to_string(),
                statement("Allow", "iam:GetDelegationRequest", "*"),
            )],
            "because no identity-based policy allows the iam:RejectDelegationRequest action"
                .to_string(),
        ),
        (
            "an explicit deny over the allow",
            vec![
                (
                    "allow".to_string(),
                    statement("Allow", "iam:RejectDelegationRequest", "*"),
                ),
                (
                    "deny".to_string(),
                    statement("Deny", "iam:RejectDelegationRequest", "*"),
                ),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
    ];

    for (row, (case, policies, reason)) in rows.into_iter().enumerate() {
        let user_name = format!("rejecter{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        for delegation_request_id in [held, "neverfiled2468013"] {
            let case = format!("{case} rejecting {delegation_request_id}");
            let (status, body) = reject(
                &router,
                Some(&access_key_id),
                delegation_request_id,
                Some("a reason the caller may not write"),
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected 403, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}: {body}",
                error_code(&body)
            );
            assert_access_denied_message(
                &case,
                &element(&body, "Message"),
                &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                "iam:RejectDelegationRequest",
                &format!("delegation request {delegation_request_id}"),
                &reason,
            );
        }
    }

    // A refused caller changed nothing: the request the account holds carries the state it
    // was seeded with, no rejection reason, and it is still the only request held.
    let (status, after) = get(&router, held).await;
    assert_eq!(
        status, 200,
        "the held request must still be readable, got {status}: {after}"
    );
    assert_eq!(
        element(&after, "State"),
        STATE_UNASSIGNED,
        "a refused caller must not move the state, got {:?}",
        element(&after, "State")
    );
    assert!(
        elements(&after, "RejectionReason").is_empty(),
        "a refused caller must not record a rejection reason, got {:?}",
        elements(&after, "RejectionReason")
    );
    assert_eq!(
        held_delegation_requests(&engine),
        vec![held.to_string()],
        "a refused rejection files nothing, got {:?}",
        held_delegation_requests(&engine)
    );
}

/// RULE: a grant is decided per ACTION, so a caller allowed the rejection and the read is
/// served both — the rejection the account holds a request for is recorded, and the read
/// reports it.
#[tokio::test]
async fn a_caller_a_statement_allows_is_served() {
    let router = router();
    let delegation_request_id = file(&router, "granted rejection", "workflow-reject-granted").await;
    let access_key_id = principal(
        &router,
        "granted rejection",
        "granted-rejecter",
        &[
            (
                "reject".to_string(),
                statement("Allow", "iam:RejectDelegationRequest", "*"),
            ),
            (
                "read".to_string(),
                statement("Allow", "iam:GetDelegationRequest", "*"),
            ),
        ],
    )
    .await;

    let (status, body) = reject(
        &router,
        Some(&access_key_id),
        &delegation_request_id,
        Some("denied by the account's reviewer"),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the granted rejection to be served, got {status}: {body}"
    );

    let (status, after) = call_signed_by(
        &router,
        Some(&access_key_id),
        "GetDelegationRequest",
        &[("DelegationRequestId", delegation_request_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the rejected request must be readable by the granted caller, got {status}: {after}"
    );
    assert_eq!(
        element(&after, "State"),
        STATE_REJECTED,
        "a rejected request reaches REJECTED, got {:?}",
        element(&after, "State")
    );
    assert_eq!(
        element(&after, "RejectionReason"),
        "denied by the account's reviewer",
        "expected the explanation the rejection carried, got {:?}",
        element(&after, "RejectionReason")
    );
}
