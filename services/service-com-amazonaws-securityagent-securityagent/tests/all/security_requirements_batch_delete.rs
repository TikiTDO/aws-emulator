// SPDX-License-Identifier: CC0-1.0

//! The security requirement DELETE path of the SecurityAgent emulator, end to end over
//! the wire: `BatchDeleteSecurityRequirements` removing records a
//! `BatchCreateSecurityRequirements` wrote, with every outcome re-read through
//! `BatchGetSecurityRequirements` so a case counts only when the SHARED STORE agrees.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), as a `restJson1` `POST /<OperationName>` with a JSON body signed
//! for the `securityagent` signing name.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: the
//! delete is ATOMIC in the names it is given — unless the pack holds every one of them
//! nothing is deleted, each name it does not hold is reported in `errors` as not found
//! and every other requested name as one the service never processed, and otherwise
//! every named record is dropped and reported in `deletedSecurityRequirementNames`. So
//! the rows differ along the request's length, how many names are missing and where they
//! sit, whether a name is repeated, and the form of each name.

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    AWS_MANAGED_PACKS, BATCH_REQUIREMENT_ERROR_NOT_FOUND, BATCH_REQUIREMENT_ERROR_NOT_PROCESSED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member};

/// `call`, requiring `expected` as the HTTP status and naming the case when it is not.
async fn expect_status(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    expected: u16,
    case: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, expected,
        "case {case:?}: expected {operation} to answer {expected}, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// `call`, requiring a refusal with `status` and `code`, and naming the case otherwise.
async fn expect_refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    status: u16,
    code: &str,
    case: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (status, code),
        "case {case:?}: expected {operation} to answer {status} {code}, got {} ({}) with {}",
        answer.status,
        answer.error_code,
        answer.body
    );
    answer.body
}

/// A fresh customer managed pack, returning its minted `packId`. The model gives the
/// pack create `http {code: 201}`.
async fn create_pack(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = expect_status(
        router,
        "CreateSecurityRequirementPack",
        &json!({"name": name}),
        201,
        case,
    )
    .await;
    member(&created, "packId")
        .unwrap_or_else(|| panic!("case {case:?}: no packId was minted: {created}"))
}

/// Store one requirement per name in `names`. `BatchCreateSecurityRequirements` carries
/// `http {code: 201}` in the model.
async fn store(router: &MultiServiceRouter, pack_id: &str, names: &[&str], case: &str) {
    let entries: Vec<Value> = names
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "description": format!("the stored description of {name}"),
                "domain": "DataProtection",
                "evaluation": "Check it."
            })
        })
        .collect();
    let created = expect_status(
        router,
        "BatchCreateSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirements": entries}),
        201,
        case,
    )
    .await;
    assert!(
        errors(&created, case).is_empty(),
        "case {case:?}: the fixture batch did not store cleanly: {created}"
    );
}

/// `BatchDeleteSecurityRequirements` on `names`, requiring HTTP 200 (the model defaults
/// this operation's success status).
async fn batch_delete(
    router: &MultiServiceRouter,
    pack_id: &str,
    names: &[&str],
    case: &str,
) -> Value {
    expect_status(
        router,
        "BatchDeleteSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirementNames": names}),
        200,
        case,
    )
    .await
}

/// `BatchGetSecurityRequirements` on `names`, requiring HTTP 200.
async fn batch_get(
    router: &MultiServiceRouter,
    pack_id: &str,
    names: &[&str],
    case: &str,
) -> Value {
    expect_status(
        router,
        "BatchGetSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirementNames": names}),
        200,
        case,
    )
    .await
}

/// The `deletedSecurityRequirementNames` list of a delete response — `@required`, so it
/// is present (and empty) even when every name failed.
fn deleted(payload: &Value, case: &str) -> Vec<String> {
    payload
        .get("deletedSecurityRequirementNames")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: no deletedSecurityRequirementNames list in {payload}")
        })
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a deleted name is not a string: {name}"))
                .to_string()
        })
        .collect()
}

/// The `errors` list of a batch response — `@required`, so it is present (and empty)
/// even when every name succeeded.
fn errors(payload: &Value, case: &str) -> Vec<Value> {
    payload
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no errors list in {payload}"))
        .clone()
}

/// The result or error entry naming `name`, or `None` when the list carries none.
fn entry_named<'a>(entries: &'a [Value], field: &str, name: &str) -> Option<&'a Value> {
    entries
        .iter()
        .find(|entry| member(entry, field).as_deref() == Some(name))
}

/// The entry of `errors` naming `name`, required to carry the not-found code and a
/// message naming it.
fn expect_not_found(failed: &[Value], name: &str, case: &str) {
    let error = entry_named(failed, "securityRequirementName", name).unwrap_or_else(|| {
        panic!("case {case:?}: {name:?} was not reported in errors: {failed:?}")
    });
    assert_eq!(
        member(error, "code").as_deref(),
        Some(BATCH_REQUIREMENT_ERROR_NOT_FOUND),
        "case {case:?}: {name:?} was reported with the wrong code: {error}"
    );
    assert!(
        member(error, "message").is_some_and(|message| message.contains(name)),
        "case {case:?}: {name:?} was reported without a message naming it: {error}"
    );
}

/// The entry of `errors` naming `name`, required to carry the not-processed code — what
/// a name the pack DOES hold carries when another requested name stopped the request.
fn expect_not_processed(failed: &[Value], name: &str, case: &str) {
    let error = entry_named(failed, "securityRequirementName", name).unwrap_or_else(|| {
        panic!("case {case:?}: {name:?} was not reported in errors: {failed:?}")
    });
    assert_eq!(
        member(error, "code").as_deref(),
        Some(BATCH_REQUIREMENT_ERROR_NOT_PROCESSED),
        "case {case:?}: {name:?} was reported with the wrong code: {error}"
    );
}

/// One row of the delete table: the names the pack holds, and the names the request asks
/// to delete.
///
/// The rows differ along the request's LENGTH, HOW MANY of its names the pack does not
/// hold and WHERE they sit (first, middle, last, nowhere, everywhere), and the FORM of
/// the names — ASCII, Unicode, whitespace-bearing — none of which the rule distinguishes.
/// No row repeats a name; a repeated name has its own case below.
fn delete_rows() -> Vec<(&'static str, Vec<&'static str>, Vec<&'static str>)> {
    vec![
        (
            "the only name is held",
            vec!["a-one"],
            vec!["a-one"],
        ),
        (
            "the only name is missing",
            vec!["b-one"],
            vec!["b-absent"],
        ),
        (
            "the first of three is missing",
            vec!["c-two", "c-three"],
            vec!["c-one", "c-two", "c-three"],
        ),
        (
            "the middle of three is missing",
            vec!["d-one", "d-three"],
            vec!["d-one", "d-two", "d-three"],
        ),
        (
            "the last of three is missing",
            vec!["e-one", "e-two"],
            vec!["e-one", "e-two", "e-three"],
        ),
        (
            "every name of four is held",
            vec!["f-one", "f-two", "f-three", "f-four"],
            vec!["f-one", "f-two", "f-three", "f-four"],
        ),
        (
            "every name of three is missing",
            vec!["g-kept"],
            vec!["g-one", "g-two", "g-three"],
        ),
        (
            "names in unusual forms",
            vec![
                "  surrounding whitespace  ",
                "requirement.with/odd-chars_0123",
                "exigence-de-sécurité-🔐",
            ],
            vec![
                "  surrounding whitespace  ",
                "requirement.with/odd-chars_0123",
                "exigence-de-sécurité-🔐",
                "jamais-créé",
            ],
        ),
    ]
}

/// Assert the whole rule on one answer, given the names the pack held when the call was
/// made, and return the names it holds afterwards.
///
/// Every row's request names each requirement at most once, so one requested name is one
/// expected entry of the answer.
fn expect_rule<'a>(
    answer: &Value,
    requested: &[&'a str],
    held: &[&'a str],
    case: &str,
) -> Vec<&'a str> {
    let missing: Vec<&str> = requested
        .iter()
        .copied()
        .filter(|name| !held.contains(name))
        .collect();

    if missing.is_empty() {
        // Every requested name resolved, so every named record is dropped and reported,
        // in REQUEST order: not a set, and not the stored order.
        assert_eq!(
            deleted(answer, case),
            *requested,
            "case {case:?}: wrong deleted names in {answer}"
        );
        assert!(
            errors(answer, case).is_empty(),
            "case {case:?}: a delete that deleted everything reported errors: {answer}"
        );
        return held
            .iter()
            .copied()
            .filter(|name| !requested.contains(name))
            .collect();
    }

    // One name it cannot resolve makes the whole request a no-op, and every requested
    // name still carries exactly one outcome.
    assert!(
        deleted(answer, case).is_empty(),
        "case {case:?}: a request naming {missing:?} deleted something: {answer}"
    );
    let failed = errors(answer, case);
    assert_eq!(
        failed.len(),
        requested.len(),
        "case {case:?}: expected one error per requested name, got {failed:?}"
    );
    for name in requested {
        if missing.contains(name) {
            expect_not_found(&failed, name, case);
        } else {
            expect_not_processed(&failed, name, case);
        }
    }
    held.to_vec()
}

/// The delete is ATOMIC in the names it is given: unless the pack holds every requested
/// name, NOTHING is deleted — each name it does not hold is reported as not found and
/// every other requested name as one the service never processed — and otherwise every
/// named record is dropped and reported as deleted, in request order. What the store
/// holds afterwards is asserted through the read, not through the delete's own answer.
#[tokio::test]
async fn a_batch_delete_is_atomic_in_the_names_it_is_given() {
    let router = emulator();

    // The per-entry codes are WIRE values, and the not-found one is NOT the name of the
    // call-level `ResourceNotFoundException` shape, so both are pinned here as the
    // literals a client branches on, not only as the constants the handler reads.
    assert_eq!(
        BATCH_REQUIREMENT_ERROR_NOT_FOUND, "ResourceNotFound",
        "case \"a missing name's code\": got {BATCH_REQUIREMENT_ERROR_NOT_FOUND:?}"
    );
    assert_eq!(
        BATCH_REQUIREMENT_ERROR_NOT_PROCESSED, "NotProcessed",
        "case \"an unprocessed name's code\": got {BATCH_REQUIREMENT_ERROR_NOT_PROCESSED:?}"
    );

    for (case, held, requested) in delete_rows() {
        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;
        store(&router, &pack_id, &held, case).await;

        let answer = batch_delete(&router, &pack_id, &requested, case).await;
        let held_after = expect_rule(&answer, &requested, &held, case);

        // The store. Every name the delete reported as deleted is gone, and every name
        // the pack held that it did not delete is still readable — so a request it
        // refused left the pack exactly as it was.
        let read = batch_get(&router, &pack_id, &requested, case).await;
        let readable: Vec<String> = read
            .get("securityRequirements")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no securityRequirements list in {read}"))
            .iter()
            .filter_map(|record| member(record, "name"))
            .collect();
        let expected_readable: Vec<String> = requested
            .iter()
            .filter(|name| held_after.contains(name))
            .map(|name| (*name).to_string())
            .collect();
        for name in &expected_readable {
            assert!(
                readable.contains(name),
                "case {case:?}: {name:?} was not deleted, so it must keep its record, \
                 got {readable:?}"
            );
        }
        assert_eq!(
            readable.len(),
            expected_readable.len(),
            "case {case:?}: expected {expected_readable:?} still readable, got {readable:?}"
        );
        for name in requested.iter().filter(|name| !held_after.contains(name)) {
            expect_not_found(&errors(&read, case), name, case);
        }

        // A name the request never named is untouched either way.
        let untouched: Vec<&str> = held
            .iter()
            .copied()
            .filter(|name| !requested.contains(name))
            .collect();
        if !untouched.is_empty() {
            let kept = batch_get(&router, &pack_id, &untouched, case).await;
            assert!(
                errors(&kept, case).is_empty(),
                "case {case:?}: a name the delete never asked for went missing: {kept}"
            );
        }

        // The SAME request again, against what the pack holds now: the same rule, which
        // after a delete that emptied the request's names makes every one of them a name
        // the pack does not hold.
        let again = batch_delete(&router, &pack_id, &requested, case).await;
        expect_rule(&again, &requested, &held_after, case);
    }
}

/// A name the request repeats names ONE record, so the outcome is per requirement rather
/// than per request position: the record is dropped once and reported once, and a
/// repeated name that the pack does not hold is reported once as missing.
#[tokio::test]
async fn a_name_repeated_within_one_request_is_reported_once() {
    // (case, how many times the name is sent, whether the pack holds it)
    let rows: Vec<(&str, usize, bool)> = vec![
        ("a held name sent twice", 2, true),
        ("a held name sent three times", 3, true),
        ("a held name sent five times", 5, true),
        ("a missing name sent twice", 2, false),
        ("a missing name sent four times", 4, false),
    ];

    let router = emulator();
    for (case, occurrences, held) in rows {
        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;
        if held {
            store(&router, &pack_id, &["repeated-requirement"], case).await;
        }

        let names: Vec<&str> = (0..occurrences).map(|_| "repeated-requirement").collect();
        let answer = batch_delete(&router, &pack_id, &names, case).await;

        if held {
            assert_eq!(
                deleted(&answer, case),
                vec!["repeated-requirement"],
                "case {case:?}: expected the name reported deleted exactly once, got {answer}"
            );
            assert!(
                errors(&answer, case).is_empty(),
                "case {case:?}: a repeated held name was reported in errors: {answer}"
            );
            continue;
        }

        assert!(
            deleted(&answer, case).is_empty(),
            "case {case:?}: a name the pack never held was reported deleted: {answer}"
        );
        let failed = errors(&answer, case);
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected the missing name reported once, got {failed:?}"
        );
        expect_not_found(&failed, "repeated-requirement", case);
    }
}

/// A name is a requirement's identity only WITHIN its pack, so a delete addressed to one
/// pack cannot reach another pack's record of the same name.
#[tokio::test]
async fn a_delete_reaches_only_the_pack_it_names() {
    let router = emulator();
    let case = "the same name in two packs";
    let shared_name = "shared-requirement";

    let target = create_pack(&router, "the pack the delete names", case).await;
    let other = create_pack(&router, "the pack beside it", case).await;
    store(&router, &target, &[shared_name], case).await;
    store(&router, &other, &[shared_name], case).await;

    let answer = batch_delete(&router, &target, &[shared_name], case).await;
    assert_eq!(
        deleted(&answer, case),
        vec![shared_name],
        "case {case:?}: the delete did not drop the named pack's record: {answer}"
    );

    let kept = batch_get(&router, &other, &[shared_name], case).await;
    assert!(
        errors(&kept, case).is_empty(),
        "case {case:?}: the other pack's record of the same name was dropped too: {kept}"
    );
    assert_eq!(
        kept.get("securityRequirements")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "case {case:?}: the other pack no longer holds its record: {kept}"
    );
}

/// The refusals the delete answers for the REQUEST as a whole, over a table of bad
/// requests: an empty pack id, a pack id that names nothing, and a pack AWS manages.
/// Each one removes nothing.
#[tokio::test]
async fn whole_request_refusals() {
    let router = emulator();

    // An empty pack id names no pack and never could: a bad request, not a missing one.
    expect_refusal(
        &router,
        "BatchDeleteSecurityRequirements",
        &json!({"packId": "", "securityRequirementNames": ["anything"]}),
        400,
        "ValidationException",
        "an empty packId",
    )
    .await;

    // An unknown pack: a requirement is reached through the pack that holds it, so there
    // is no pack to delete from — 404 for the whole call, naming the id it could not
    // find.
    let case = "a packId nothing minted";
    let unknown = "00000000000000000000000000000fff";
    let refusal = expect_refusal(
        &router,
        "BatchDeleteSecurityRequirements",
        &json!({"packId": unknown, "securityRequirementNames": ["anything"]}),
        404,
        "ResourceNotFoundException",
        case,
    )
    .await;
    assert!(
        member(&refusal, "message").is_some_and(|message| message.contains(unknown)),
        "case {case:?}: the refusal did not name the pack it could not find: {refusal}"
    );

    // The contents of a pack AWS ships are the service's, not a caller's to remove: the
    // delete conflicts with that pack's state. The row is every entry of the seeded
    // catalog, so the rule is not asserted on one.
    for managed in AWS_MANAGED_PACKS {
        let case = managed.pack_id;
        let refusal = expect_refusal(
            &router,
            "BatchDeleteSecurityRequirements",
            &json!({
                "packId": managed.pack_id,
                "securityRequirementNames": ["anything"]
            }),
            409,
            "ConflictException",
            case,
        )
        .await;
        assert!(
            member(&refusal, "message").is_some_and(|message| message.contains(managed.pack_id)),
            "case {case:?}: the refusal did not name the pack: {refusal}"
        );
    }
}
