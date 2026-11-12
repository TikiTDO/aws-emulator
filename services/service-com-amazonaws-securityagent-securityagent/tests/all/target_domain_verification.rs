// SPDX-License-Identifier: CC0-1.0

//! Target domain OWNERSHIP VERIFICATION in the SecurityAgent emulator, end to end over
//! the wire: `VerifyTargetDomain` against the records `CreateTargetDomain` wrote, read
//! back through `BatchGetTargetDomains` and through the agent space read that expands
//! the domains a space is associated with.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), so nothing here can pass against a handler that answers a canned
//! body: what a verification call did must come back out of the shared store through a
//! read before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: the
//! rows differ along the `DomainVerificationMethod` the domain was registered with —
//! each method puts its challenge somewhere else — along the form of the domain name,
//! along WHEN the record is read relative to the check's due instant, and, for the
//! refusals, along how the `targetDomainId` misses the store.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    TARGET_DOMAIN_STATUS_PENDING, TARGET_DOMAIN_STATUS_UNREACHABLE, VERIFICATION_METHOD_DNS_TXT,
    VERIFICATION_METHOD_HTTP_ROUTE, VERIFICATION_METHOD_PRIVATE_VPC, VERIFICATION_SETTLE_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;

/// How much longer than the check's own span a case waits before reading the outcome,
/// so a slow machine cannot read the record before the clock owes the change.
const SETTLE_SLACK: Duration = Duration::from_millis(500);

/// The sentence a refusal of a target domain the account does not hold states. Source:
/// the recorded 404 for an unknown `targetDomainId`, stated independently of the
/// constant the handler reports it from.
const TARGET_DOMAIN_NOT_FOUND: &str = "The specified target domain does not exist.";

use crate::support::{call, emulator, member, ok};

/// One registration a verification case is made against: the verification method, which
/// decides WHERE the challenge lives, and the form of the domain name.
///
/// Every `DomainVerificationMethod` the model admits
/// (`smithy_query(query="shape",shape="DomainVerificationMethod")`: `DNS_TXT`,
/// `HTTP_ROUTE`, `PRIVATE_VPC`) carries a row, so a new value cannot be added to the
/// enum without this table going stale, and each is paired with a different name form.
fn registration_rows() -> Vec<(String, String, &'static str)> {
    let names = [
        ("an apex domain", "example.com".to_string()),
        ("a subdomain", "shop.eu.example.co.uk".to_string()),
        ("mixed case", "ShOp.Example.COM".to_string()),
    ];
    let mut rows = Vec::new();
    for method in [
        VERIFICATION_METHOD_DNS_TXT,
        VERIFICATION_METHOD_HTTP_ROUTE,
        VERIFICATION_METHOD_PRIVATE_VPC,
    ] {
        for (index, (shape, name)) in names.iter().enumerate() {
            rows.push((
                format!("{method} on {shape}"),
                // A distinct domain per row, so each row is its own record. The method
                // goes into the label with its underscore replaced: a hostname label is
                // letters, digits and hyphens only.
                format!("v{index}-{}.{name}", method.to_lowercase().replace('_', "-")),
                method,
            ));
        }
    }
    rows
}

/// `CreateTargetDomain`, as the whole 200 payload of the record it registered.
async fn register(router: &MultiServiceRouter, domain: &str, method: &str, case: &str) -> Value {
    ok(
        router,
        "CreateTargetDomain",
        &json!({"targetDomainName": domain, "verificationMethod": method}),
        case,
    )
    .await
}

/// `VerifyTargetDomain` for `id`, requiring HTTP 200.
async fn verify(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    ok(
        router,
        "VerifyTargetDomain",
        &json!({"targetDomainId": id}),
        case,
    )
    .await
}

/// The stored record `BatchGetTargetDomains` reports for `id`.
async fn read(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    let payload = ok(
        router,
        "BatchGetTargetDomains",
        &json!({"targetDomainIds": [id]}),
        case,
    )
    .await;
    payload
        .get("targetDomains")
        .and_then(Value::as_array)
        .and_then(|records| records.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {id} must read back as a record: {payload}"))
}

/// The instant a `@timestampFormat("date-time")` member reports, in seconds since the
/// epoch, read back out of the RFC 3339 string the wire carries.
fn timestamp_secs(payload: &Value, name: &str, case: &str) -> f64 {
    let value = payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no {name}: {payload}"));
    match value {
        Value::String(text) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| panic!("case {case:?}: {text:?} is not a date-time: {error}"))
            .as_secs_f64(),
        other => panic!("case {case:?}: {name} is not a date-time string: {other}"),
    }
}

/// The status a verification answer reports, requiring the member to be there.
fn status(payload: &Value, case: &str) -> String {
    member(payload, "status")
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no status: {payload}"))
}

/// The status a stored record reports, under BOTH names the wire carries it under
/// (`verificationStatus` is the modelled member, `status` the recorded one beside it),
/// which one stored field feeds — so they cannot disagree.
fn stored_status(record: &Value, case: &str) -> String {
    let modelled = member(record, "verificationStatus").unwrap_or_else(|| {
        panic!("case {case:?}: the record carries no verificationStatus: {record}")
    });
    assert_eq!(
        member(record, "status").as_deref(),
        Some(modelled.as_str()),
        "case {case:?}: the record must report one status under both names: {record}"
    );
    modelled
}

/// The reason a payload gives for the status it reports, which is never empty.
fn reason(payload: &Value, case: &str) -> String {
    let reason = member(payload, "verificationStatusReason").unwrap_or_else(|| {
        panic!("case {case:?}: the answer says nothing about why: {payload}")
    });
    assert!(
        !reason.trim().is_empty(),
        "case {case:?}: verificationStatusReason must say something, got {reason:?}"
    );
    reason
}

/// RULE: a verification call starts the ownership check for the domain its id names and
/// answers from that record — the domain stays `PENDING` while the check runs, and once
/// the check falls due the record reports the outcome of reading a challenge no proof
/// was published under: `UNREACHABLE`, with a reason for that status and still no
/// `verifiedAt`. The outcome is the RECORD's, so every read of it — the domains' own
/// batch read, the agent space read that expands them, and a later verification call —
/// reports the same status, and a domain no verification call named is untouched.
#[tokio::test]
async fn a_verification_call_starts_the_check_and_the_record_then_reports_its_outcome() {
    let router = emulator();
    let rows = registration_rows();

    // Registered, then verified: the check starts, so the answer is the PENDING it runs
    // in. (case, id, the registration payload, the answer of the first verify)
    let mut cases: Vec<(String, String, Value, Value)> = Vec::new();
    for (case, domain, method) in &rows {
        let created = register(&router, domain, method, case).await;
        let id = member(&created, "targetDomainId")
            .unwrap_or_else(|| panic!("case {case:?}: no targetDomainId was minted: {created}"));

        let started = verify(&router, &id, case).await;
        assert_eq!(
            status(&started, case),
            TARGET_DOMAIN_STATUS_PENDING,
            "case {case:?}: the check does not settle inside the call that starts it: \
             {started}"
        );
        assert_eq!(
            member(&started, "targetDomainId").as_deref(),
            Some(id.as_str()),
            "case {case:?}: the answer must name the record it was made on: {started}"
        );
        assert_eq!(
            member(&started, "domainName").as_deref(),
            Some(domain.as_str()),
            "case {case:?}: the answer must report the stored domain name: {started}"
        );
        assert_eq!(
            started.get("createdAt"),
            created.get("createdAt"),
            "case {case:?}: the registration time must not move: {started}"
        );
        assert_eq!(
            started.get("verifiedAt"),
            None,
            "case {case:?}: a domain whose ownership is not proven has no verifiedAt: \
             {started}"
        );
        assert_eq!(
            reason(&started, case),
            reason(&created, case),
            "case {case:?}: while the check runs the record still says why it is \
             pending: {started}"
        );
        // The record changed when the check started, and that is not before it was
        // registered.
        let created_secs = timestamp_secs(&created, "createdAt", case);
        let updated_secs = timestamp_secs(&started, "updatedAt", case);
        assert!(
            updated_secs >= created_secs,
            "case {case:?}: updatedAt {updated_secs} is before createdAt {created_secs}: \
             {started}"
        );

        // A second call while the check is in flight does not push its due instant
        // back, so it reports the same record.
        let again = verify(&router, &id, case).await;
        assert_eq!(
            (status(&again, case), again.get("updatedAt")),
            (TARGET_DOMAIN_STATUS_PENDING.to_string(), started.get("updatedAt")),
            "case {case:?}: a call made while the check runs must not restart it: {again}"
        );

        // And the domains' own read agrees while the check runs.
        let record = read(&router, &id, case).await;
        assert_eq!(
            stored_status(&record, case),
            TARGET_DOMAIN_STATUS_PENDING,
            "case {case:?}: the stored record is PENDING while the check runs: {record}"
        );

        cases.push((case.clone(), id, created, started));
    }

    // One registration no verification call ever names, so the outcome below is the
    // work of the calls that were made and not of time passing.
    let untouched_case = "a domain no verification call names";
    let untouched = register(
        &router,
        "untouched.example.com",
        VERIFICATION_METHOD_DNS_TXT,
        untouched_case,
    )
    .await;
    let untouched_id = member(&untouched, "targetDomainId").expect("the registration mints an id");

    // An agent space associated with every verified domain: its read expands the stored
    // domains, so it must report the same outcome as their own read.
    let space_case = "the agent space the verified domains are associated with";
    let ids: Vec<String> = cases.iter().map(|(_, id, _, _)| id.clone()).collect();
    let space = ok(
        &router,
        "CreateAgentSpace",
        &json!({"name": "verification-space", "targetDomainIds": ids}),
        space_case,
    )
    .await;
    let space_id = member(&space, "agentSpaceId").expect("the create mints an agentSpaceId");

    // Wait the check's own span out ONCE, for every row: the outcome is due from then
    // on, and the next call that reads a record is what materialises it.
    tokio::time::sleep(VERIFICATION_SETTLE_AFTER + SETTLE_SLACK).await;

    for (case, id, created, started) in &cases {
        // The domains' own read reports the outcome.
        let settled = read(&router, id, case).await;
        assert_eq!(
            stored_status(&settled, case),
            TARGET_DOMAIN_STATUS_UNREACHABLE,
            "case {case:?}: a check that read no published proof leaves the domain \
             unreachable for verification: {settled}"
        );
        assert_eq!(
            settled.get("verifiedAt"),
            None,
            "case {case:?}: no proof was read, so there is no verification time: \
             {settled}"
        );
        let settled_reason = reason(&settled, case);
        assert_ne!(
            settled_reason,
            reason(created, case),
            "case {case:?}: the settled record must say why it is in THIS status, not \
             why it was pending: {settled}"
        );
        assert_eq!(
            settled.get("createdAt"),
            created.get("createdAt"),
            "case {case:?}: the registration time must not move: {settled}"
        );

        // A verification call made now reports that same outcome rather than returning
        // the record to PENDING, and the record it reports is the one the read above
        // reported.
        let after = verify(&router, id, case).await;
        assert_eq!(
            status(&after, case),
            TARGET_DOMAIN_STATUS_UNREACHABLE,
            "case {case:?}: the outcome of a settled check stands: {after}"
        );
        assert_eq!(
            reason(&after, case),
            settled_reason,
            "case {case:?}: the verification answer and the read must agree: {after}"
        );
        assert_eq!(
            after.get("verifiedAt"),
            None,
            "case {case:?}: no proof was read, so there is no verification time: {after}"
        );
        // The record changed when the outcome fell due, which is after the check was
        // started and no later than the span the check takes.
        let started_secs = timestamp_secs(started, "updatedAt", case);
        let settled_secs = timestamp_secs(&after, "updatedAt", case);
        assert!(
            settled_secs > started_secs
                && settled_secs <= started_secs + VERIFICATION_SETTLE_AFTER.as_secs_f64() + 1.0,
            "case {case:?}: updatedAt {settled_secs} is not the instant the check fell \
             due after {started_secs}: {after}"
        );

        // And the read still reports the settled record: the verification call above
        // started no new check.
        let reread = read(&router, id, case).await;
        assert_eq!(
            stored_status(&reread, case),
            TARGET_DOMAIN_STATUS_UNREACHABLE,
            "case {case:?}: a repeat verification call must not hide the outcome behind \
             PENDING again: {reread}"
        );
    }

    // The agent space read expands the stored domains, so it reports exactly the same
    // statuses as their own read.
    let spaces = ok(
        &router,
        "BatchGetAgentSpaces",
        &json!({"agentSpaceIds": [space_id]}),
        space_case,
    )
    .await;
    let embedded = spaces
        .get("agentSpaces")
        .and_then(Value::as_array)
        .and_then(|records| records.first())
        .and_then(|record| record.get("targetDomains"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {space_case:?}: no expanded domains: {spaces}"));
    assert_eq!(
        embedded.len(),
        cases.len(),
        "case {space_case:?}: expected the {} associated domains, got {embedded:?}",
        cases.len()
    );
    for domain in &embedded {
        assert_eq!(
            stored_status(domain, space_case),
            TARGET_DOMAIN_STATUS_UNREACHABLE,
            "case {space_case:?}: an expanded domain must read as its own record does: \
             {domain}"
        );
    }

    // The domain no verification call named never had a check to settle, so it is still
    // the record the registration wrote.
    let still_pending = read(&router, &untouched_id, untouched_case).await;
    assert_eq!(
        stored_status(&still_pending, untouched_case),
        TARGET_DOMAIN_STATUS_PENDING,
        "case {untouched_case:?}: a domain nothing verified must not move: {still_pending}"
    );
    assert_eq!(
        member(&still_pending, "verificationStatusReason"),
        member(&untouched, "verificationStatusReason"),
        "case {untouched_case:?}: its reason must not move either: {still_pending}"
    );
}

/// RULE: a verification call the service cannot act on changes nothing — an id the
/// account does not hold (including one it has already deleted) is HTTP 404
/// `ResourceNotFoundException` stating the service's own fixed sentence for that
/// resource class, and an id that could never name a record is HTTP 400
/// `ValidationException`; the domain the account does hold is untouched by every one of
/// them.
#[tokio::test]
async fn a_verification_call_the_service_cannot_act_on_is_refused_and_starts_no_check() {
    let router = emulator();
    // Several registrations, so the stored id the near-miss rows mutate carries letters
    // as well as digits: a minted id is a string of digits and letters, so WHICH of the
    // registrations below provides it is whichever one first minted an id with a letter in
    // it — keeping only the last would make the rows below depend on how one run's ids came
    // out.
    let mut kept = String::new();
    for index in 0..10 {
        let case = format!("stored domain {index}");
        let created = register(
            &router,
            &format!("kept{index}.example.com"),
            VERIFICATION_METHOD_HTTP_ROUTE,
            &case,
        )
        .await;
        let minted = member(&created, "targetDomainId").expect("the registration mints an id");
        if !kept.chars().any(|c| c.is_ascii_alphabetic()) {
            kept = minted;
        }
    }
    assert!(
        kept.chars().any(|c| c.is_ascii_alphabetic()),
        "expected a minted targetDomainId carrying a letter, got {kept:?}"
    );
    let deleted_case = "the domain deleted up front";
    let deleted = member(
        &register(
            &router,
            "deleted.example.com",
            VERIFICATION_METHOD_PRIVATE_VPC,
            deleted_case,
        )
        .await,
        "targetDomainId",
    )
    .expect("the registration mints an id");
    ok(
        &router,
        "DeleteTargetDomain",
        &json!({"targetDomainId": deleted}),
        deleted_case,
    )
    .await;

    // (case, the id the request carries, the status and code due) — the rows differ
    // along HOW the id misses the store.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "an id already deleted",
            deleted.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id never minted",
            "ffffffffffffffffffffffffffffffff".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the stored id upper-cased",
            kept.to_uppercase(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the stored id with a suffix",
            format!("{kept}0"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the stored id with its last character dropped",
            kept[..kept.len() - 1].to_string(),
            404,
            "ResourceNotFoundException",
        ),
        ("an empty id", String::new(), 400, "ValidationException"),
    ];

    for (case, target_domain_id, expected_status, code) in rows {
        assert_ne!(
            target_domain_id, kept,
            "case {case:?}: the row must not name the domain that is kept"
        );
        let answer = call(
            &router,
            "VerifyTargetDomain",
            &json!({"targetDomainId": target_domain_id}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (expected_status, code),
            "case {case:?}: expected {expected_status} {code} for {target_domain_id:?}, \
             got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        if expected_status == 404 {
            let message = member(&answer.body, "message")
                .unwrap_or_else(|| panic!("case {case:?}: no message: {}", answer.body));
            assert_eq!(
                message, TARGET_DOMAIN_NOT_FOUND,
                "case {case:?}: expected {TARGET_DOMAIN_NOT_FOUND:?}, got {message:?}"
            );
            assert!(
                !message.contains(&target_domain_id),
                "case {case:?}: the refusal must carry nothing the request sent, got \
                 {message:?}"
            );
        }

        // The domain the account holds is untouched: no refused call started a check on
        // anything.
        let record = read(&router, &kept, case).await;
        assert_eq!(
            stored_status(&record, case),
            TARGET_DOMAIN_STATUS_PENDING,
            "case {case:?}: a refused verification call must change no record: {record}"
        );
    }

    // And the id the account DOES hold is served, so the refusals above are about the
    // lookup and nothing else.
    let served = verify(&router, &kept, "the domain the account holds").await;
    assert_eq!(
        status(&served, "the domain the account holds"),
        TARGET_DOMAIN_STATUS_PENDING,
        "case \"the domain the account holds\": expected the check to start: {served}"
    );
}
