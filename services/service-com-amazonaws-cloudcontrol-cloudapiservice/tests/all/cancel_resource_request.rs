// SPDX-License-Identifier: CC0-1.0

//! `CancelResourceRequest`: which resource operation requests can be canceled, and how the
//! two refusals are worded.
//!
//! The rule every case here holds the service to: **a cancel resolves its `RequestToken`
//! against the resource operation requests this service holds — a token no request answers
//! to is refused with the modelled `RequestTokenNotFoundException` (HTTP 404) naming that
//! token; a request whose `OperationStatus` is not one of the two a cancel acts on is
//! refused with `ValidationException` (HTTP 400) naming the token and the status it is
//! already in, because "only resource operations requests with a status of PENDING or
//! IN_PROGRESS can be canceled" (`CancelResourceRequest`) — which covers a request that is
//! over AND one already being canceled; and a request in one of those two IS canceled: it
//! abandons the work it owed, reports `CANCEL_IN_PROGRESS` and reaches `CANCEL_COMPLETE`.**
//!
//! The rows differ along what the rule generalises over: which operation started the
//! request (`CREATE`, `UPDATE`, `DELETE`), which status it was in when the cancel arrived
//! (`IN_PROGRESS`, `CANCEL_IN_PROGRESS`, `SUCCESS`, `FAILED`, `CANCEL_COMPLETE`), the form
//! of a token no request answers to, and what the abandoned work leaves behind — a resource
//! that was never provisioned, one left exactly as it was, and one that is still there.

use serde_json::{json, Map, Value};

use super::emulator::{
    code, message, progress, text, Emulator, STATUS_CANCEL_COMPLETE, STATUS_CANCEL_IN_PROGRESS,
    STATUS_FAILED, STATUS_IN_PROGRESS, STATUS_SUCCESS,
};

/// A type whose resources live in this service's own store, so a case is about the request
/// rather than about one owning service.
const STREAM: &str = "AWS::Kinesis::Stream";

/// One `ListResourceRequests` page, as the service reports it.
async fn listed_tokens(emulator: &Emulator) -> Vec<String> {
    let mut request = Map::new();
    request.insert("MaxResults".to_string(), json!(100));
    let listed = emulator
        .cc_ok("ListResourceRequests", Value::Object(request))
        .await;
    listed["ResourceRequestStatusSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| text(summary, "RequestToken"))
        .collect()
}

/// Cancel `request_token` and require the call to succeed, answering the `ProgressEvent` it
/// reports.
async fn cancel_ok(emulator: &Emulator, request_token: &str) -> Value {
    let (status, body) = emulator.cancel(request_token).await;
    assert_eq!(
        status, 200,
        "cancelling {request_token:?}: expected HTTP 200, got {status} with body {body:?}"
    );
    progress(&body)
}

#[tokio::test]
async fn a_running_request_is_canceled_and_its_work_is_abandoned() {
    let emulator = Emulator::new();

    // One running request per operation, each against its own resource: the resource an
    // UPDATE and a DELETE need is provisioned first, so cancelling them leaves something
    // for the case to look at. The two creates are independent, so they are started
    // together and waited for once.
    let seeded = [
        json!({ "StreamName": "patched", "ShardCount": 1 }),
        json!({ "StreamName": "doomed", "ShardCount": 2 }),
    ];
    let mut accepted = Vec::new();
    for desired in &seeded {
        let event = emulator.create_ok(STREAM, desired, None).await;
        accepted.push(text(&event, "RequestToken"));
    }
    let mut provisioned = Vec::new();
    for request_token in &accepted {
        provisioned.push(text(&emulator.settled(request_token).await, "Identifier"));
    }
    let (patched, doomed) = (provisioned[0].clone(), provisioned[1].clone());

    let creating = text(
        &emulator
            .create_ok(STREAM, &json!({ "StreamName": "never-made" }), None)
            .await,
        "RequestToken",
    );
    let (status, body) = emulator
        .update(
            STREAM,
            &patched,
            &json!([{ "op": "replace", "path": "/ShardCount", "value": 9 }]),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "the update: expected HTTP 200, got {status} with body {body:?}"
    );
    let updating = text(&progress(&body), "RequestToken");
    let (status, body) = emulator.delete(STREAM, &doomed).await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} with body {body:?}"
    );
    let deleting = text(&progress(&body), "RequestToken");

    // Each cancel answers the request it names, in the status a cancellation that is still
    // running reports, and says when to ask again.
    let rows = [
        ("a create", &creating, "CREATE"),
        ("an update", &updating, "UPDATE"),
        ("a delete", &deleting, "DELETE"),
    ];
    for (case, request_token, operation) in rows {
        let canceled = cancel_ok(&emulator, request_token).await;
        assert_eq!(
            text(&canceled, "RequestToken"),
            *request_token,
            "case {case:?}: the cancel must report the request its token names, got \
             {canceled:?}"
        );
        assert_eq!(
            text(&canceled, "Operation"),
            operation,
            "case {case:?}: the cancel must report the operation the request ran, got \
             {canceled:?}"
        );
        assert_eq!(
            text(&canceled, "OperationStatus"),
            STATUS_CANCEL_IN_PROGRESS,
            "case {case:?}: a cancellation that has not completed must report \
             CANCEL_IN_PROGRESS, got {canceled:?}"
        );
        assert!(
            canceled["RetryAfter"].as_f64().is_some(),
            "case {case:?}: a request still being canceled must say when to ask again, got \
             {canceled:?}"
        );

        // "Only resource operations requests with a status of PENDING or IN_PROGRESS can
        // be canceled": a request already being canceled is in neither, so a repeat cancel
        // is refused rather than accepted — and the instant the cancellation completes
        // stays exactly where the first cancel put it, which is what makes this
        // `@idempotent` operation idempotent.
        let (status, body) = emulator.cancel(request_token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second cancel while the first is in flight: expected HTTP \
             400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("ValidationException"),
            "case {case:?}: a second cancel while the first is in flight: expected \
             ValidationException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!("Request {request_token} is already in status {STATUS_CANCEL_IN_PROGRESS}"),
            "case {case:?}: a second cancel while the first is in flight: unexpected wording"
        );
        let after = progress(&emulator.status(request_token).await.1);
        assert_eq!(
            after["RetryAfter"], canceled["RetryAfter"],
            "case {case:?}: a refused cancel must not move when the cancellation completes, \
             expected {:?}, got {:?}",
            canceled["RetryAfter"], after["RetryAfter"]
        );
    }

    // Each one then reaches the status a canceled request ends in, and nothing it owed was
    // applied: the create provisioned nothing, the update changed nothing, the delete
    // removed nothing.
    for (case, request_token, _) in rows {
        let settled = emulator
            .reaching(request_token, &[STATUS_CANCEL_COMPLETE])
            .await;
        assert!(
            settled["RetryAfter"].is_null(),
            "case {case:?}: a canceled request has nothing left to ask again for, got \
             {settled:?}"
        );
        assert!(
            settled["ResourceModel"].is_null(),
            "case {case:?}: a canceled request left no resource model behind, got {settled:?}"
        );
    }
    let (status, body) = emulator.get(STREAM, "never-made").await;
    assert_eq!(
        status, 404,
        "the canceled create must have provisioned nothing, got {status} with body {body:?}"
    );
    let kept = emulator.properties(STREAM, &patched).await;
    assert_eq!(
        kept,
        json!({ "StreamName": "patched", "ShardCount": 1 }),
        "the canceled update must have changed nothing, got {kept:?}"
    );
    let survivor = emulator.properties(STREAM, &doomed).await;
    assert_eq!(
        survivor,
        json!({ "StreamName": "doomed", "ShardCount": 2 }),
        "the canceled delete must have removed nothing, got {survivor:?}"
    );

    // And every canceled request is still one this service holds, resolvable by its token
    // and reported by the listing.
    let tokens = listed_tokens(&emulator).await;
    for (case, request_token, _) in rows {
        assert!(
            tokens.contains(request_token),
            "case {case:?}: a canceled request must still be listed, got {tokens:?}"
        );
    }
}

#[tokio::test]
async fn a_terminal_request_is_refused_in_the_status_it_reached() {
    let emulator = Emulator::new();

    // One request per terminal status: a create that succeeded, a create that the owning
    // handler failed because the resource was already there, and a request that was
    // canceled.
    let succeeded = text(
        &emulator
            .create_ok(STREAM, &json!({ "StreamName": "settled" }), None)
            .await,
        "RequestToken",
    );
    emulator.settled(&succeeded).await;
    let failed = text(
        &emulator
            .create_ok(STREAM, &json!({ "StreamName": "settled" }), None)
            .await,
        "RequestToken",
    );
    let event = emulator.terminal(&failed).await;
    assert_eq!(
        text(&event, "OperationStatus"),
        STATUS_FAILED,
        "a second create of the same resource must fail, got {event:?}"
    );
    let canceled = text(
        &emulator
            .create_ok(STREAM, &json!({ "StreamName": "abandoned" }), None)
            .await,
        "RequestToken",
    );
    cancel_ok(&emulator, &canceled).await;
    emulator
        .reaching(&canceled, &[STATUS_CANCEL_COMPLETE])
        .await;

    let rows = [
        ("a request that succeeded", &succeeded, STATUS_SUCCESS),
        ("a request the handler failed", &failed, STATUS_FAILED),
        (
            "a request already canceled",
            &canceled,
            STATUS_CANCEL_COMPLETE,
        ),
    ];
    for (case, request_token, reached) in rows {
        let (status, body) = emulator.cancel(request_token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!("Request {request_token} is already in status {reached}"),
            "case {case:?}: unexpected wording"
        );
        assert!(
            progress(&body).is_null(),
            "case {case:?}: a refused cancel reports no ProgressEvent, got {body:?}"
        );
        // The refusal changed nothing: the request is still in the status it had reached.
        let after = emulator.terminal(request_token).await;
        assert_eq!(
            text(&after, "OperationStatus"),
            reached,
            "case {case:?}: a refused cancel must leave the request as it was, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_token_no_request_answers_to_is_the_modelled_refusal() {
    let emulator = Emulator::new();

    // One request this service DID mint, so the store is not simply empty, and so the case
    // can show that a token the service issued resolves while these do not.
    let minted = text(
        &emulator
            .create_ok(STREAM, &json!({ "StreamName": "issued" }), None)
            .await,
        "RequestToken",
    );

    // The rows differ along the form of the token: a well-formed random UUID, the nil UUID,
    // a short word, a value in the `+/=` half of the `RequestToken` pattern, and one at the
    // modelled maximum length of 128 characters.
    let tokens = [
        "3f8c6c4e-5d7a-4a1f-9a21-6f2b0c7d8e90",
        "00000000-0000-0000-0000-000000000000",
        "bogus",
        "AAAA=",
        &"t".repeat(128),
    ];
    for token in tokens {
        let (status, body) = emulator.cancel(token).await;
        assert_eq!(
            status, 404,
            "token {token:?}: expected HTTP 404, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("RequestTokenNotFoundException"),
            "token {token:?}: expected RequestTokenNotFoundException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            format!("Request with token {token} was not found"),
            "token {token:?}: unexpected wording"
        );
        assert_ne!(
            token, minted,
            "token {token:?}: the case needs a token this service did not mint"
        );
    }

    // A cancel of an unknown token minted nothing: none of them is a request this service
    // holds, so none of them is listed, and the one real request is untouched.
    let listed = listed_tokens(&emulator).await;
    for token in tokens {
        assert!(
            !listed.contains(&token.to_string()),
            "token {token:?}: a refused cancel must record no request, got {listed:?}"
        );
    }
    assert_eq!(
        listed,
        vec![minted.clone()],
        "only the request this service minted may be listed, got {listed:?}"
    );
    let still = emulator.status(&minted).await.1;
    assert_eq!(
        text(&progress(&still), "OperationStatus"),
        STATUS_IN_PROGRESS,
        "the request this service minted must be untouched, got {still:?}"
    );
}
