// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RevokeVpcEndpointAccess`: the other half of the OpenSearch
//! Service VPC endpoint access surface that [`super::vpc_endpoint_access`] drives the
//! grant and the listing of.
//!
//! The rule under test: **a revoke names ONE principal — the account or the service its
//! `Account`/`Service` member names, exactly as a grant does — and takes that principal's
//! authorization off the domain named in the path and nothing else: the listing loses that
//! one record, every other principal's record stands, and the call grants nothing. A
//! principal the domain does not allow has no access left to take away, so the revoke
//! succeeds with its empty body and the set is unchanged, while a domain name that
//! resolves to nothing names no set at all and is refused with the modelled
//! missing-resource error.**
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness and the grant/listing calls come from [`super::domain_lifecycle`] and
//! [`super::vpc_endpoint_access`]; this suite adds no second copy of either.
//!
//! The rows differ along what the rule generalises over: which member names the revoked
//! principal (an account, a service), the digits and length of an account id, whether the
//! principal named holds a grant at all, whether the same revoke is repeated, how many
//! other principals the domain allows beside it, and whether `ServiceOptions` rides along
//! with the revoke. The recorded case — revoke an account that was never authorized, then
//! revoke the one that was — is two rows of that table.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::PRINCIPAL_TYPE_AWS_ACCOUNT;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS};
use super::vpc_endpoint_access::{
    authorize, created_domain, list, principal_list, principal_of, service_principal,
};

/// `RevokeVpcEndpointAccess`:
/// `POST /opensearch/domain/{DomainName}/revokeVpcEndpointAccess`.
///
/// `pub(crate)` because the suite that asserts this service's refusal WORDING drives the
/// same operation through this wrapper rather than growing a second copy of it.
pub(crate) async fn revoke(emulator: &Emulator, domain: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{domain}/revokeVpcEndpointAccess"),
            Some(body),
        )
        .await
}

/// The principals a domain reports as authorized right now, in the order the listing
/// reports them.
async fn authorized(emulator: &Emulator, domain: &str, case: &str) -> Vec<String> {
    let answer = list(emulator, domain, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from ListVpcEndpointAccess, got {} with {:?}",
        answer.status, answer.body
    );
    principal_list(&answer, case)
        .iter()
        .map(|entry| principal_of(entry, case))
        .collect()
}

/// The set of principals every row of [`revoke_rows`] starts from: one grant per kind of
/// principal the model declares, with account ids of differing length and digits, so a
/// revoke that took out the wrong record — or more than one — shows up.
fn granted() -> Vec<Value> {
    vec![
        json!({ "Account": "123456789012" }),
        json!({ "Account": "000000000000" }),
        json!({ "Account": "7" }),
        json!({
            "Service": service_principal(),
            "ServiceOptions": { "SupportedRegions": ["eu-west-1"] },
        }),
    ]
}

/// The principals [`granted`] leaves authorized, as the listing names them.
fn granted_principals() -> Vec<String> {
    vec![
        "000000000000".to_string(),
        "123456789012".to_string(),
        "7".to_string(),
        service_principal().to_string(),
    ]
}

/// One row: a revoke request against the set [`granted`] left, and the principal it must
/// take away — `None` for a revoke that must take nothing away at all.
struct Revocation {
    /// What tells this row from the others, for the case label.
    label: &'static str,
    /// The request body, which carries everything but the domain name (that is the path
    /// label).
    body: Value,
    /// The principal that must be gone from the listing afterwards, or `None` where the
    /// revoke named a principal the domain never allowed.
    removes: Option<&'static str>,
    /// How many times the row sends that same request: a revoke is idempotent, so sending
    /// it twice must answer the same way and leave the same set.
    repeats: usize,
}

/// The revoke table: one row per way one accepted revoke differs from another.
fn revoke_rows() -> Vec<Revocation> {
    vec![
        Revocation {
            label: "account/12-digit/holds a grant",
            body: json!({ "Account": "123456789012" }),
            removes: Some("123456789012"),
            repeats: 1,
        },
        Revocation {
            label: "account/all-zero/holds a grant",
            body: json!({ "Account": "000000000000" }),
            removes: Some("000000000000"),
            repeats: 1,
        },
        Revocation {
            label: "account/one-digit/holds a grant",
            body: json!({ "Account": "7" }),
            removes: Some("7"),
            repeats: 1,
        },
        Revocation {
            label: "account/12-digit/holds a grant/revoked twice",
            body: json!({ "Account": "123456789012" }),
            removes: Some("123456789012"),
            repeats: 2,
        },
        Revocation {
            label: "service/holds a grant",
            body: json!({ "Service": service_principal() }),
            removes: Some(service_principal()),
            repeats: 1,
        },
        Revocation {
            label: "service/holds a grant/ServiceOptions carried along",
            body: json!({
                "Service": service_principal(),
                "ServiceOptions": { "SupportedRegions": ["us-east-1"] },
            }),
            removes: Some(service_principal()),
            repeats: 1,
        },
        Revocation {
            // The recorded case: an account that was NEVER authorized.
            label: "account/never authorized",
            body: json!({ "Account": "111111111111" }),
            removes: None,
            repeats: 1,
        },
        Revocation {
            label: "account/one-digit/never authorized/revoked twice",
            body: json!({ "Account": "4" }),
            removes: None,
            repeats: 2,
        },
        Revocation {
            label: "account/long/never authorized",
            body: json!({ "Account": "98765432109876543210" }),
            removes: None,
            repeats: 1,
        },
    ]
}

#[tokio::test]
async fn a_revoke_takes_away_the_one_principal_it_names_and_nothing_else() {
    for row in revoke_rows() {
        let case = format!("RevokeVpcEndpointAccess({})", row.label);
        // A fresh emulator per row, so the row's set is the whole set and the listing's
        // answer is about this row alone.
        let emulator = Emulator::new();
        created_domain(&emulator, "search-revoked").await;
        for grant in granted() {
            let granted = authorize(&emulator, "search-revoked", grant.clone()).await;
            assert_eq!(
                granted.status, 200,
                "case {case}: expected 200 granting {grant:?}, got {} with {:?}",
                granted.status, granted.body
            );
        }

        for attempt in 1..=row.repeats {
            let answer = revoke(&emulator, "search-revoked", row.body.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {case} (attempt {attempt}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            // The model declares no output member for this operation, so a success carries
            // nothing that could name a principal or a grant.
            assert!(
                answer.body.as_object().is_none_or(|body| body.is_empty())
                    || answer.body.is_null(),
                "case {case} (attempt {attempt}): expected an empty body, got {:?}",
                answer.body
            );
        }

        // The expected set is computed from what the row declares, never read back out of
        // the answer being checked.
        let expected: Vec<String> = granted_principals()
            .into_iter()
            .filter(|principal| Some(principal.as_str()) != row.removes)
            .collect();
        let remaining = authorized(&emulator, "search-revoked", &case).await;
        assert_eq!(
            remaining, expected,
            "case {case}: expected the listing to report {expected:?}, got {remaining:?}"
        );
    }
}

#[tokio::test]
async fn a_revoke_on_one_domain_leaves_another_domains_grants_alone() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-left").await;
    created_domain(&emulator, "search-right").await;

    let shared = json!({ "Account": "123456789012" });
    for domain in ["search-left", "search-right"] {
        let granted = authorize(&emulator, domain, shared.clone()).await;
        assert_eq!(
            granted.status, 200,
            "case AuthorizeVpcEndpointAccess({domain}): expected 200, got {} with {:?}",
            granted.status, granted.body
        );
    }

    let revoked = revoke(&emulator, "search-left", shared.clone()).await;
    assert_eq!(
        revoked.status, 200,
        "case RevokeVpcEndpointAccess(search-left): expected 200, got {} with {:?}",
        revoked.status, revoked.body
    );

    let left = authorized(&emulator, "search-left", "search-left after the revoke").await;
    assert!(
        left.is_empty(),
        "case RevokeVpcEndpointAccess(search-left): expected the revoked domain to allow \
         nobody, got {left:?}"
    );
    let right = authorized(&emulator, "search-right", "search-right after the revoke").await;
    assert_eq!(
        right,
        vec!["123456789012".to_string()],
        "case RevokeVpcEndpointAccess(search-left): expected the other domain's grant to \
         stand, got {right:?}"
    );
}

#[tokio::test]
async fn a_revoke_never_grants_anything() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-ungranted").await;

    // (case, the request body) — every shape of revoke, against a domain that allows
    // nobody at all.
    let rows: Vec<(&str, Value)> = vec![
        ("an account", json!({ "Account": "123456789012" })),
        ("a service", json!({ "Service": service_principal() })),
        (
            "an account with service options",
            json!({
                "Account": "7",
                "ServiceOptions": { "SupportedRegions": ["us-east-1"] },
            }),
        ),
    ];

    for (label, body) in rows {
        let case = format!("RevokeVpcEndpointAccess({label}) on a domain allowing nobody");
        let answer = revoke(&emulator, "search-ungranted", body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let remaining = authorized(&emulator, "search-ungranted", &case).await;
        assert!(
            remaining.is_empty(),
            "case {case}: a revoke must grant nothing, got {remaining:?}"
        );
    }
}

#[tokio::test]
async fn a_revoke_that_names_no_principal_or_two_is_refused_and_the_set_stands() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-guarded-revoke").await;
    let granted = authorize(&emulator, "search-guarded-revoke", json!({ "Account": "123456789012" }))
        .await;
    assert_eq!(
        granted.status, 200,
        "case AuthorizeVpcEndpointAccess(search-guarded-revoke): expected 200, got {} with {:?}",
        granted.status, granted.body
    );

    // (case, the request body) — a revoke names EXACTLY one principal, so naming none and
    // naming both are the two ways of getting that wrong.
    let rows: Vec<(&str, Value)> = vec![
        ("no principal at all", json!({})),
        (
            "no principal, only service options",
            json!({ "ServiceOptions": { "SupportedRegions": ["us-east-1"] } }),
        ),
        (
            "both an account and a service",
            json!({ "Account": "123456789012", "Service": service_principal() }),
        ),
    ];

    for (label, body) in rows {
        let case = format!("RevokeVpcEndpointAccess({label})");
        let answer = revoke(&emulator, "search-guarded-revoke", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );
        let remaining = authorized(&emulator, "search-guarded-revoke", &case).await;
        assert_eq!(
            remaining,
            vec!["123456789012".to_string()],
            "case {case}: a refused revoke must leave the set exactly as it was, got \
             {remaining:?}"
        );
    }
}

#[tokio::test]
async fn a_revoke_naming_a_domain_the_registry_does_not_resolve_is_refused() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-present").await;

    // Names inside `DomainName`'s own 3..28 / `^[a-z][a-z0-9\-]+$` constraints, so the
    // request reaches the handler and what refuses it is the registry rather than the
    // request boundary.
    for name in ["search-absent", "abc", "search-absent-28-chars-long"] {
        let case = format!("RevokeVpcEndpointAccess({name})");
        let answer = revoke(&emulator, name, json!({ "Account": "123456789012" })).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // And a revoke on the domain that DOES exist still answers, so the refusals above are
    // about the name and not about the operation.
    let answer = revoke(&emulator, "search-present", json!({ "Account": "123456789012" })).await;
    assert_eq!(
        answer.status, 200,
        "case RevokeVpcEndpointAccess(search-present): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn a_revoked_principal_can_be_granted_access_again() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-regrant").await;

    // The round trip of the pair: grant, revoke, grant again — each step read back through
    // the listing, so what is asserted is the state a later caller sees.
    for round in 1..=3 {
        let case = format!("grant/revoke round {round}");
        let granted = authorize(&emulator, "search-regrant", json!({ "Account": "123456789012" })).await;
        assert_eq!(
            granted.status, 200,
            "case {case}: expected 200 from the grant, got {} with {:?}",
            granted.status, granted.body
        );
        assert_eq!(
            granted.body["AuthorizedPrincipal"]["PrincipalType"].as_str(),
            Some(PRINCIPAL_TYPE_AWS_ACCOUNT),
            "case {case}: expected the grant to record an account principal, got {:?}",
            granted.body
        );
        let after_grant = authorized(&emulator, "search-regrant", &case).await;
        assert_eq!(
            after_grant,
            vec!["123456789012".to_string()],
            "case {case}: expected the grant to be listed, got {after_grant:?}"
        );

        let revoked = revoke(&emulator, "search-regrant", json!({ "Account": "123456789012" })).await;
        assert_eq!(
            revoked.status, 200,
            "case {case}: expected 200 from the revoke, got {} with {:?}",
            revoked.status, revoked.body
        );
        let after_revoke = authorized(&emulator, "search-regrant", &case).await;
        assert!(
            after_revoke.is_empty(),
            "case {case}: expected the revoke to leave nobody listed, got {after_revoke:?}"
        );
    }
}
