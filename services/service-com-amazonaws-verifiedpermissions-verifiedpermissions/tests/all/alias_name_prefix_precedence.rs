// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for WHEN the alias-name prefix rule is answered: a member
//! whose value cannot be a name any alias carries is a malformed request, so it is
//! refused before state is consulted at all.
//!
//! `tests/all/alias_name_prefix.rs` pins WHAT the refusal looks like (its status,
//! code and `fieldList`) over the ways a name can miss the prefix. This suite pins
//! its PRECEDENCE: the same refusal comes back whatever state holds — whether the
//! prefixed name of the same shape is absent, `Active`, `PendingDeletion` or
//! already hard-deleted, and whichever Region the request was signed for — and no
//! state-dependent answer (the idempotent delete success, the read's not-found) is
//! ever given in its place.
//!
//! What the refusal DOES come after is the resolution of a resource the request
//! REFERENCES: `CreatePolicyStoreAlias`'s `policyStoreId` names the store the
//! alias is associated with, so a creation naming no store answers that 404 and
//! not this 400. `tests/all/not_found_precedes_payload_refusal.rs` pins that
//! precedence, so every creation row here names a store that exists.
//!
//! Every assertion goes over the wire through the shared [`crate::harness`], so
//! what a refused request did NOT write is visibly absent from the next request's
//! answer.

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;

use crate::harness::{
    call, call_in, error_code, fault_message, field_list, ok, ok_in, router,
    store_with_validation_off, store_with_validation_off_in, SIGNING_REGION,
};

/// The prefix every alias name carries, from the `aliasName` member's own model
/// documentation on `CreatePolicyStoreAlias`, `GetPolicyStoreAlias` and
/// `DeletePolicyStoreAlias` (`smithy_query(query="shape",
/// shape="DeletePolicyStoreAliasInput")`): "The alias name must always be prefixed
/// with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The JSON pointer a refusal of that member names — where the member sits in the
/// request body, which is the form `ValidationExceptionField$path` carries.
const ALIAS_NAME_POINTER: &str = "/aliasName";

/// The suffix the rows below hang their names off, so the unprefixed value and the
/// prefixed one differ only by the prefix.
const SUFFIX: &str = "photos";

/// Every request that reads `alias_name` as its `aliasName`, as
/// `(operation, body)`: the three operations the member appears on, with the
/// delete repeated for each `deletionMode` it accepts (absent, and both published
/// values of `DeletionMode`). `store_id` is what a creation associates, so a
/// creation row can only fail on the NAME.
fn requests_naming(alias_name: &str, store_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreatePolicyStoreAlias",
            json!({ "aliasName": alias_name, "policyStoreId": store_id }),
        ),
        ("GetPolicyStoreAlias", json!({ "aliasName": alias_name })),
        ("DeletePolicyStoreAlias", json!({ "aliasName": alias_name })),
        (
            "DeletePolicyStoreAlias",
            json!({ "aliasName": alias_name, "deletionMode": "SoftDelete" }),
        ),
        (
            "DeletePolicyStoreAlias",
            json!({ "aliasName": alias_name, "deletionMode": "HardDelete" }),
        ),
    ]
}

/// Assert that `operation` refused `request` as a malformed `aliasName`: HTTP 400,
/// the modelled `ValidationException`, its fixed summary message, and exactly one
/// `fieldList` entry naming the member.
async fn assert_refused_as_malformed_alias_name(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    case: &str,
    operation: &str,
    request: &Value,
) {
    let (status, body) = call_in(router, region, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: {operation} must answer ValidationException, got {body}"
    );
    assert_eq!(
        fault_message(&body),
        INVALID_INPUT_MESSAGE,
        "case {case:?}: {operation} must carry the fixed summary message, got {body}"
    );
    let fields = field_list(&body);
    assert_eq!(
        fields.len(),
        1,
        "case {case:?}: {operation} must name exactly one offending member, got {fields:?}"
    );
    assert_eq!(
        fields[0].0, ALIAS_NAME_POINTER,
        "case {case:?}: {operation} must name {ALIAS_NAME_POINTER}, got {fields:?}"
    );
    assert!(
        fields[0].1.contains(ALIAS_PREFIX),
        "case {case:?}: {operation} must state the required prefix, got {fields:?}"
    );
}

/// RULE: an `aliasName` that does not carry the prefix the model documents on the
/// member is a malformed member, so every operation taking it refuses the request
/// with HTTP 400 `ValidationException` naming that member — BEFORE state is read,
/// so the answer does not depend on what state holds and the request changes
/// nothing.
///
/// The rows differ along the state the correctly prefixed name of the same shape is
/// in — never created, `Active`, `PendingDeletion` (soft-deleted) and removed
/// (hard-deleted) — crossed with two ways the prefix is missing and with every
/// operation and `deletionMode` that reads the member. Each row also asserts the
/// state the prefixed name was left in afterwards, so a refused call that had
/// quietly deleted, pended or re-created something would fail here.
#[tokio::test]
async fn the_prefix_refusal_precedes_every_state_dependent_answer() {
    // (case, how the prefixed twin is set up, the `state` a read must report for
    // it afterwards — `None` when the read must be a not-found).
    let twin_states: &[(&str, Option<&str>, Option<&str>)] = &[
        ("the prefixed name was never created", None, None),
        ("the prefixed name is Active", Some(""), Some("Active")),
        (
            "the prefixed name is PendingDeletion",
            Some("SoftDelete"),
            Some("PendingDeletion"),
        ),
        ("the prefixed name was hard-deleted", Some("HardDelete"), None),
    ];
    // Two ways the prefix is missing, so the precedence is asserted on more than
    // one malformed form: the suffix alone, and a name whose prefix is only a
    // look-alike.
    let malformed: &[(&str, String)] = &[
        ("suffix only", SUFFIX.to_string()),
        ("look-alike prefix", format!("policy-store-aliases/{SUFFIX}")),
    ];

    for (twin_case, setup, expected_state) in twin_states {
        let router = router();
        let store_id = store_with_validation_off(&router).await;
        let prefixed = format!("{ALIAS_PREFIX}{SUFFIX}");

        if let Some(deletion_mode) = setup {
            ok(
                &router,
                "CreatePolicyStoreAlias",
                &json!({ "aliasName": prefixed, "policyStoreId": store_id }),
            )
            .await;
            if !deletion_mode.is_empty() {
                ok(
                    &router,
                    "DeletePolicyStoreAlias",
                    &json!({ "aliasName": prefixed, "deletionMode": deletion_mode }),
                )
                .await;
            }
        }

        for (malformed_case, alias_name) in malformed {
            for (operation, request) in requests_naming(alias_name, &store_id) {
                let case = format!("{twin_case}, {malformed_case}, request {request}");
                assert_refused_as_malformed_alias_name(
                    &router,
                    SIGNING_REGION,
                    &case,
                    operation,
                    &request,
                )
                .await;
            }
        }

        // What the refusals must have left behind: exactly the state the setup put
        // the prefixed name in.
        let (status, body) = call(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": prefixed }),
        )
        .await;
        match expected_state {
            Some(state) => assert_eq!(
                (status, body["state"].as_str().unwrap_or_default()),
                (200, *state),
                "case {twin_case:?}: the refused calls must have left the prefixed alias in \
                 {state}, got {status} {body}"
            ),
            None => assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {twin_case:?}: the refused calls must not have created the prefixed \
                 alias, got {status} {body}"
            ),
        }
    }
}

/// RULE (the same one, on the other thing the answer must not depend on): the
/// malformed-member refusal is independent of the Region the request was signed
/// for — the scope an alias name is unique in — so the member is refused with the
/// same status, code and `fieldList` in every Region, including a request that
/// names none.
///
/// The rows differ along the signing Region, including a request that names none at
/// all, and along the operation and `deletionMode` that read the member. The
/// creation rows name the Region's own store, the reference every creation
/// resolves before its payload is read.
#[tokio::test]
async fn the_prefix_refusal_does_not_depend_on_the_region() {
    // The empty string sends no credential scope at all, which is how a request
    // that names no Region reaches the service.
    let regions: &[&str] = &["us-east-1", "eu-west-2", "ap-southeast-1", ""];

    for region in regions {
        let router = router();
        let store_id = store_with_validation_off_in(&router, region).await;

        for (operation, request) in requests_naming(SUFFIX, &store_id) {
            let case = format!(
                "region {:?}, request {request}",
                if region.is_empty() { "unsigned" } else { region }
            );
            assert_refused_as_malformed_alias_name(&router, region, &case, operation, &request)
                .await;
        }

        // The well-formed name the refusals talked about still names nothing in
        // this Region, so none of them wrote anything.
        let prefixed = format!("{ALIAS_PREFIX}{SUFFIX}");
        let (status, body) = call_in(
            &router,
            region,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": prefixed }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {region:?}: a refused request must not have created an alias, got \
             {status} {body}"
        );

        // And the Region's own well-formed answers are untouched by the refusals:
        // a creation of the prefixed name still succeeds there.
        let created = ok_in(
            &router,
            region,
            "CreatePolicyStoreAlias",
            &json!({ "aliasName": prefixed, "policyStoreId": store_id }),
        )
        .await;
        assert_eq!(
            created["aliasName"].as_str().unwrap_or_default(),
            prefixed,
            "case {region:?}: a well-formed creation must still succeed, got {created}"
        );
    }
}
