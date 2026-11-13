// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAcmeExternalAccountBinding` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: a delete of one ACME external account binding removes the stored record its ARN
//! names and nothing else — whatever that record carries and however often the call is
//! repeated — so afterwards the ARN answers to no record at all
//! (`DescribeAcmeExternalAccountBinding`, `GetAcmeExternalAccountBindingCredentials` and
//! `ListTagsForResource` report it missing and `ListAcmeExternalAccountBindings` no
//! longer lists it), while an ARN whose account segment is another account's is refused
//! as invalid input before any lookup and writes nothing.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT
//! operation than the write, so a response that is not computed from the persisted store
//! cannot pass. The wire harness, the endpoint body and the table of bindings are shared
//! with [`crate::certificate_lifecycle`], [`crate::acme_domain_validation_lifecycle`] and
//! [`crate::acme_external_account_binding_lifecycle`].
//!
//! The table below spans what the rule generalises over: every `TimeType` unit an
//! `Expiration` can be counted in and a request that carries none, role ARNs of several
//! forms, tag sets of several sizes and scripts, and — cycled across those rows — whether
//! the binding stood revoked when the delete arrived, since neither the expiry nor the
//! revocation may gate the removal.

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::endpoint_body;
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::acme_external_account_binding_lifecycle::{
    binding_body, binding_cases, create_binding, describe_binding,
};
use crate::acme_external_account_binding_revocation::{held_bindings, revoke_binding};
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT, REGION,
};

/// `AcmeExternalAccountBindingArn`'s modelled maximum length (`len 1..200`, from
/// `smithy_query(query="operation", operation="DeleteAcmeExternalAccountBinding")`).
const MAX_BINDING_ARN_LENGTH: usize = 200;

/// `DeleteAcmeExternalAccountBinding`, asserted to succeed and to answer with the empty
/// body its modelled `Unit` output carries.
async fn delete_binding(router: &MultiServiceRouter, case: &str, arn: &str) {
    let (status, response) = call(
        router,
        "DeleteAcmeExternalAccountBinding",
        &json!({ "AcmeExternalAccountBindingArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteAcmeExternalAccountBinding({arn}) must succeed, \
         got {status} {response}"
    );
    assert!(
        response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: the operation models no output, so its answer must carry no \
         members, got {response}"
    );
}

/// Every binding ARN `acme_endpoint_arn` still holds, read back through
/// `ListAcmeExternalAccountBindings` (one page: the endpoints here hold a handful).
async fn listed_binding_arns(
    router: &MultiServiceRouter,
    case: &str,
    acme_endpoint_arn: &str,
) -> Vec<String> {
    let (status, listed) = call(
        router,
        "ListAcmeExternalAccountBindings",
        &json!({ "AcmeEndpointArn": acme_endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAcmeExternalAccountBindings must succeed, got {status} {listed}"
    );
    assert!(
        listed["NextToken"].is_null(),
        "case {case:?}: these endpoints hold one page of bindings, got {listed}"
    );
    let mut arns: Vec<String> = listed["ExternalAccountBindings"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ExternalAccountBindings in {listed}"))
        .iter()
        .filter_map(|summary| summary["AcmeExternalAccountBindingArn"].as_str())
        .map(str::to_string)
        .collect();
    arns.sort();
    arns
}

/// Assert that `operation`, called with the binding ARN member, reports `arn` missing with
/// the service's own not-found sentence.
async fn assert_reported_missing(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    arn: &str,
) {
    let (status, refusal) = call(
        router,
        operation,
        &json!({ "AcmeExternalAccountBindingArn": arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must report the deleted binding missing, \
         got {status} {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException from {operation}, got {refusal}"
    );
    assert_eq!(
        error_message(&refusal),
        state::acme_external_account_binding_not_found_message(arn),
        "case {case:?}: expected the service's not-found sentence from {operation}, \
         got {refusal}"
    );
}

/// The ARN member of a `CreateAcmeExternalAccountBinding` answer.
fn created_arn(case: &str, created: &Value) -> String {
    created["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no ARN in {created}"))
        .to_string()
}

/// RULE: the delete removes the one record its ARN names whatever that record carries,
/// leaves the endpoint's other bindings alone, is a success when repeated, and frees the
/// idempotency token that created it — for every row of the table, revoked and standing.
#[tokio::test]
async fn deleting_a_binding_removes_it_whatever_it_carries() {
    let (router, engine) = router_with_engine();

    for (index, case) in binding_cases().into_iter().enumerate() {
        // Cycled across the rows, so "the revocation does not gate the delete" is
        // asserted over the whole table rather than on one configuration.
        let revoked = index % 2 == 1;
        let what = format!(
            "{}, deleted while {}",
            case.0,
            if revoked { "revoked" } else { "standing" }
        );

        // One endpoint per row, holding a sibling binding the row never deletes: the call
        // must act on the one ARN it names.
        let endpoint_arn = create_endpoint(&router, &what, &endpoint_body()).await;
        let sibling_case = binding_cases()[0].clone();
        let sibling = created_arn(
            &what,
            &create_binding(&router, &what, &binding_body(&sibling_case, &endpoint_arn)).await,
        );

        let token = format!("token-for-{}", case.0);
        let mut body = binding_body(&case, &endpoint_arn);
        body["IdempotencyToken"] = json!(token);
        let created = create_binding(&router, &what, &body).await;
        let arn = created_arn(&what, &created);

        if revoked {
            revoke_binding(&router, &what, &arn).await;
        }

        // The binding stands before the delete, in the state the row asked for.
        let before = describe_binding(&router, &what, &arn).await;
        assert_eq!(
            before["RevokedAt"].is_null(),
            !revoked,
            "case {what:?}: expected RevokedAt to be {} before the delete, got {before}",
            if revoked { "recorded" } else { "absent" }
        );

        let held_before = held_bindings(&engine);
        delete_binding(&router, &what, &arn).await;
        assert_eq!(
            held_bindings(&engine),
            held_before - 1,
            "case {what:?}: the delete must drop exactly one stored binding"
        );

        // The ARN answers to no record, through each reader of a binding other than the
        // delete itself, so a response not computed from the store cannot pass.
        for operation in [
            "DescribeAcmeExternalAccountBinding",
            "GetAcmeExternalAccountBindingCredentials",
        ] {
            assert_reported_missing(&router, &what, operation, &arn).await;
        }

        // The tags the binding carried went with the record.
        let (tag_status, tag_answer) =
            call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            tag_status, 400,
            "case {what:?}: a deleted binding holds no tags to list, got {tag_status} {tag_answer}"
        );
        assert_eq!(
            error_code(&tag_answer),
            "ResourceNotFoundException",
            "case {what:?}: expected ResourceNotFoundException for the deleted ARN's tags, \
             got {tag_answer}"
        );

        // The endpoint no longer lists it, and still lists the sibling.
        let remaining = listed_binding_arns(&router, &what, &endpoint_arn).await;
        assert_eq!(
            remaining,
            vec![sibling.clone()],
            "case {what:?}: expected only {sibling} to remain, got {remaining:?}"
        );

        // The operation is `@idempotent`: a repeated delete of a binding that is already
        // gone is the state it asks for, so it succeeds and still writes nothing.
        delete_binding(&router, &format!("{what} repeated"), &arn).await;
        let repeated = listed_binding_arns(&router, &what, &endpoint_arn).await;
        assert_eq!(
            repeated,
            vec![sibling.clone()],
            "case {what:?}: a repeated delete must write nothing, got {repeated:?}"
        );

        // The idempotency token went with the record, so replaying the create that made
        // the deleted binding mints a fresh one rather than naming an ARN nothing answers
        // to.
        let again = created_arn(&what, &create_binding(&router, &what, &body).await);
        assert_ne!(
            again, arn,
            "case {what:?}: the re-created binding must carry its own ARN, got {again}"
        );
        describe_binding(&router, &what, &again).await;
        let mut expected = vec![again, sibling.clone()];
        expected.sort();
        let now_listed = listed_binding_arns(&router, &what, &endpoint_arn).await;
        assert_eq!(
            now_listed, expected,
            "case {what:?}: expected the endpoint to hold the sibling and the re-created \
             binding, got {now_listed:?}"
        );
    }
}

/// RULE: an ARN whose account segment is another account's names nothing the caller could
/// delete, so the call is refused as invalid input (`ValidationException`, HTTP 400)
/// before any lookup, states the verdict for the resource type the member's own pattern
/// fixes, withholds the submitted ARN, and writes nothing.
///
/// The rows differ along what the rule generalises over: the Region, the partition and the
/// account id of the foreign ARN.
#[tokio::test]
async fn deleting_a_binding_of_another_account_is_refused_and_writes_nothing() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "owned host", &endpoint_body()).await;
    let owned = created_arn(
        "owned binding",
        &create_binding(
            &router,
            "owned binding",
            &binding_body(&binding_cases()[1].clone(), &endpoint_arn),
        )
        .await,
    );

    let foreign: &[(&str, String)] = &[
        (
            "another account in this Region",
            format!(
                "arn:aws:acm:{REGION}:111122223333:acme-endpoint/\
                 2b260b3a-a1db-47b2-98dd-b80925898547/acme-external-account-binding/\
                 7c4e4a9c-7f1e-4b1a-9c3a-6d2f0b1a5e11"
            ),
        ),
        (
            "another account in another Region",
            "arn:aws:acm:eu-central-1:210987654321:acme-endpoint/\
             33333333-3333-4333-8333-333333333333/acme-external-account-binding/\
             44444444-4444-4444-8444-444444444444"
                .to_string(),
        ),
        (
            "another account in another partition",
            "arn:aws-us-gov:acm:us-gov-west-1:444455556666:acme-endpoint/55555555/\
             acme-external-account-binding/66666666"
                .to_string(),
        ),
        (
            "an account of all nines",
            format!(
                "arn:aws:acm:{REGION}:999999999999:acme-endpoint/a/\
                 acme-external-account-binding/b"
            ),
        ),
    ];

    for (case, arn) in foreign {
        let (status, refusal) = call(
            &router,
            "DeleteAcmeExternalAccountBinding",
            &json!({ "AcmeExternalAccountBindingArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {arn} must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {case:?}: expected ValidationException for {arn}, got {refusal}"
        );
        let message = error_message(&refusal);
        assert!(
            !message.contains(arn.as_str()),
            "case {case:?}: a cross-account refusal must withhold the submitted ARN, \
             got {refusal}"
        );
        assert_eq!(
            message,
            crate::acme_authorization::cross_account_arn_message("acme-external-account-binding"),
            "case {case:?}: expected the cross-account verdict for the binding's own resource \
             type, got {refusal}"
        );
    }

    // None of those refusals wrote anything: the caller's own binding still stands.
    assert_eq!(
        held_bindings(&engine),
        1,
        "expected the refusals to have deleted nothing"
    );
    let record = describe_binding(&router, "owned binding", &owned).await;
    assert!(
        record["RevokedAt"].is_null(),
        "expected the owned binding to be untouched, got {record}"
    );
    let remaining = listed_binding_arns(&router, "owned endpoint", &endpoint_arn).await;
    assert_eq!(
        remaining,
        vec![owned],
        "expected the refusals to have deleted nothing, got {remaining:?}"
    );
}

/// RULE: an ARN of the caller's own account that no record answers to is already in the
/// state the `@idempotent` delete asks for, so the call succeeds and writes nothing; an
/// ARN past the member's modelled length is invalid input, not a missing resource.
///
/// The rows differ along what the rule generalises over: whether the endpoint the ARN
/// hangs under exists, the Region it names, and the ARN's length — at the modelled bound
/// and one byte past it.
#[tokio::test]
async fn deleting_a_binding_that_no_record_answers_to_succeeds() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "idempotent host", &endpoint_body()).await;
    let owned = created_arn(
        "owned binding",
        &create_binding(
            &router,
            "owned binding",
            &binding_body(&binding_cases()[0].clone(), &endpoint_arn),
        )
        .await,
    );

    // An ARN of exactly `AcmeExternalAccountBindingArn`'s maximum modelled length, built
    // by padding the binding id of a well-formed ARN.
    let at_bound = {
        let prefix = format!(
            "arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/e/acme-external-account-binding/"
        );
        format!(
            "{prefix}{}",
            "a".repeat(MAX_BINDING_ARN_LENGTH - prefix.len())
        )
    };
    assert_eq!(
        at_bound.len(),
        MAX_BINDING_ARN_LENGTH,
        "the bound row must sit exactly on the modelled maximum, got {at_bound}"
    );

    let gone: &[(&str, String)] = &[
        (
            "an id under an endpoint this account holds",
            state::acme_external_account_binding_arn(
                &endpoint_arn,
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
        (
            "an endpoint this account never created",
            format!(
                "arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/\
                 11111111-1111-4111-8111-111111111111/acme-external-account-binding/22222222"
            ),
        ),
        (
            "another Region of this account",
            format!(
                "arn:aws:acm:eu-west-1:{ACCOUNT}:acme-endpoint/a/\
                 acme-external-account-binding/b"
            ),
        ),
        ("an ARN at the member's maximum length", at_bound.clone()),
    ];

    for (case, arn) in gone {
        delete_binding(&router, case, arn).await;
    }

    // One byte past the bound is a constraint violation, not a resource that is already
    // gone — and never the scaffold's refusal.
    let past_bound = format!("{at_bound}a");
    let (status, refusal) = call(
        &router,
        "DeleteAcmeExternalAccountBinding",
        &json!({ "AcmeExternalAccountBindingArn": past_bound }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {past_bound:?}: an ARN past the modelled length must be refused, \
         got {status} {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        "ValidationException",
        "case {past_bound:?}: expected ValidationException past the modelled length, \
         got {refusal}"
    );

    // Nothing the caller holds was touched by any of it.
    assert_eq!(
        held_bindings(&engine),
        1,
        "expected the deletes of absent ARNs to leave the stored binding alone"
    );
    let remaining = listed_binding_arns(&router, "idempotent host", &endpoint_arn).await;
    assert_eq!(
        remaining,
        vec![owned.clone()],
        "expected the deletes of absent ARNs to leave the stored binding alone, \
         got {remaining:?}"
    );
    describe_binding(&router, "owned binding", &owned).await;
}

/// RULE: deleting a binding withdraws only what further registrations it could admit, so
/// the ACME accounts already registered through it are left standing — exactly as a
/// revocation leaves them.
///
/// The rows differ along what the rule generalises over: which of the endpoint's bindings
/// the account was registered through, and whether the account's binding is the one
/// deleted or a sibling.
#[tokio::test]
async fn deleting_a_binding_leaves_the_accounts_it_registered_standing() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "account host", &endpoint_body()).await;

    // Two bindings on one endpoint, each with an account registered through it.
    let mut arns = Vec::new();
    for case in binding_cases().into_iter().take(2) {
        let created = create_binding(&router, case.0, &binding_body(&case, &endpoint_arn)).await;
        arns.push(created_arn(case.0, &created));
    }

    // The accounts, written as an ACME client's own `newAccount` request would leave them:
    // each records the binding that admitted it.
    let endpoint_id = endpoint_arn
        .rsplit_once("acme-endpoint/")
        .expect("an ACME endpoint ARN")
        .1
        .to_string();
    let urls: Vec<String> = {
        let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
        let mut store = store.lock();
        let created_at = store
            .acme_endpoints
            .get(&endpoint_arn)
            .expect("a stored ACME endpoint")
            .created_at;
        arns.iter()
            .enumerate()
            .map(|(index, binding_arn)| {
                let account_id = format!("c000000{index}-0000-4000-8000-00000000000{index}");
                let record = state::AcmeAccountRecord {
                    acme_endpoint_arn: endpoint_arn.clone(),
                    account_url: state::acme_account_url(REGION, &endpoint_id, &account_id),
                    public_key_thumbprint: format!("thumbprint-{account_id}"),
                    // The account key that thumbprint is the digest of; nothing here
                    // verifies a signature under it, so what matters is that each account
                    // carries its own.
                    public_key_jwk: json!({
                        "crv": "P-256",
                        "kty": "EC",
                        "x": format!("x-{account_id}"),
                        "y": format!("y-{account_id}"),
                    })
                    .to_string(),
                    status: state::ACME_ACCOUNT_STATUS_VALID.to_string(),
                    acme_external_account_binding_arn: Some(binding_arn.clone()),
                    contacts: vec![format!("mailto:{account_id}@example.com")],
                    created_at,
                };
                let url = record.account_url.clone();
                store
                    .acme_accounts
                    .insert((endpoint_arn.clone(), url.clone()), record);
                url
            })
            .collect()
    };

    delete_binding(&router, "the first binding", &arns[0]).await;

    let (status, listed) = call(
        &router,
        "ListAcmeAccounts",
        &json!({ "AcmeEndpointArn": endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListAcmeAccounts must succeed after the delete, got {status} {listed}"
    );
    let accounts = listed["AcmeAccounts"]
        .as_array()
        .unwrap_or_else(|| panic!("no AcmeAccounts in {listed}"))
        .clone();
    let mut reported: Vec<(String, Option<String>)> = accounts
        .iter()
        .map(|account| {
            (
                account["AccountUrl"].as_str().unwrap_or_default().to_string(),
                account["AcmeExternalAccountBindingArn"]
                    .as_str()
                    .map(str::to_string),
            )
        })
        .collect();
    reported.sort();
    let mut expected: Vec<(String, Option<String>)> = urls
        .iter()
        .cloned()
        .zip(arns.iter().cloned().map(Some))
        .collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "expected both registered accounts to stand with the binding each records, \
         got {reported:?}"
    );

    // Only the binding went: the other one still stands, and the deleted one does not.
    let remaining = listed_binding_arns(&router, "account host", &endpoint_arn).await;
    assert_eq!(
        remaining,
        vec![arns[1].clone()],
        "expected only the surviving binding to be listed, got {remaining:?}"
    );
}
