// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service VPC endpoint access surface:
//! `AuthorizeVpcEndpointAccess` and `ListVpcEndpointAccess`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, only the two
//! paths it drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a grant persists the ONE principal its request names and the listing reports
//!   that same record, over a table of seven grants differing in which member names
//!   the principal (an account, a service), in the account id's length and digits, and
//!   in the service options carried with it (absent, empty, one Region, several);
//! * the principal type reported is the one the service answers for the kind of
//!   principal named — covered for both kinds, and pinned as a value in
//!   [`super::authorized_principal_type`];
//! * the principal IS the identity of an authorization: granting the same principal
//!   again replaces its one record and leaves every other principal's alone;
//! * a request that names no principal, or both kinds at once, is refused and the
//!   stored set stands — as is a service principal outside the modelled enum;
//! * the set belongs to the domain the request names: a grant on one domain is
//!   invisible on another, a name the registry does not resolve is refused by the GRANT
//!   and reported as the empty set by the listing, and deleting a domain releases its
//!   authorizations with it;
//! * one answer reports the whole set exactly once in the order the listing reports it
//!   and carries no `NextToken` member at all, while a token that names no principal of
//!   the set is refused;
//! * the set is bounded per domain: grants are accepted up to the quota and the one
//!   past it is refused with the modelled `LimitExceededException`, while a re-grant of
//!   a principal already in the set is still accepted at the bound because it does not
//!   grow it.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    A_W_S_SERVICE_PRINCIPAL_VALUES, PRINCIPAL_TYPE_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PRINCIPAL_TYPE_AWS_ACCOUNT, PRINCIPAL_TYPE_AWS_SERVICE, VPC_ENDPOINT_ACCESS_QUOTA,
};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS};

/// `AuthorizeVpcEndpointAccess`:
/// `POST /opensearch/domain/{DomainName}/authorizeVpcEndpointAccess`.
pub(crate) async fn authorize(emulator: &Emulator, domain: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{domain}/authorizeVpcEndpointAccess"),
            Some(body),
        )
        .await
}

/// `ListVpcEndpointAccess`: `GET /opensearch/domain/{DomainName}/listVpcEndpointAccess`,
/// optionally resumed from a page token.
pub(crate) async fn list(emulator: &Emulator, domain: &str, token: Option<&str>) -> Answer {
    let uri = match token {
        None => format!("{PREFIX}/opensearch/domain/{domain}/listVpcEndpointAccess"),
        Some(token) => {
            format!("{PREFIX}/opensearch/domain/{domain}/listVpcEndpointAccess?nextToken={token}")
        }
    };
    emulator.call("GET", &uri, None).await
}

/// A domain to grant access to, created through `CreateDomain` so the grants are made
/// against a domain that exists.
pub(crate) async fn created_domain(emulator: &Emulator, name: &str) {
    let created = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
}

/// The `AuthorizedPrincipalList` of a listing answer, with the case named on failure.
pub(crate) fn principal_list(answer: &Answer, case: &str) -> Vec<Value> {
    answer.body["AuthorizedPrincipalList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an AuthorizedPrincipalList, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// Whether a listing answer carries no `NextToken` member at all, which is how this
/// listing reports a complete result set — not the empty string, which a client testing
/// for the member's presence rather than its truthiness paginates on forever.
fn carries_no_token(answer: &Answer, case: &str) {
    assert_eq!(
        answer.body.get("NextToken"),
        None,
        "case {case}: the whole set is reported, so the answer carries no NextToken, got {:?}",
        answer.body
    );
}

/// Every authorized principal of a domain. One answer reports the domain's whole set —
/// it is bounded by [`VPC_ENDPOINT_ACCESS_QUOTA`] — so there is no page to walk.
async fn all_principals(emulator: &Emulator, domain: &str, case: &str) -> Vec<Value> {
    let answer = list(emulator, domain, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    carries_no_token(&answer, case);
    principal_list(&answer, case)
}

/// The `Principal` of one reported `AuthorizedPrincipal`.
///
/// `pub(crate)` because the sibling suites that read a listing answer name a principal
/// through this one reader rather than growing a second copy of it.
pub(crate) fn principal_of(entry: &Value, case: &str) -> String {
    entry["Principal"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string Principal, got {entry:?}"))
        .to_owned()
}

/// One grant the table drives: the request body, and the principal and principal type
/// the service must record it under.
struct Grant {
    /// What tells this row from the others, for the case label.
    label: &'static str,
    /// The request body, which carries everything but the domain name (that is the
    /// path label).
    body: Value,
    /// `AuthorizedPrincipal$Principal` the grant must be recorded under.
    principal: String,
    /// `AuthorizedPrincipal$PrincipalType` it must be recorded with.
    principal_type: &'static str,
    /// The `ServiceOptions` the record must carry, or `None` where the request carried
    /// none and the answer must carry no such member either.
    service_options: Option<Value>,
}

/// The only value `AWSServicePrincipal` declares, from this service's Smithy model
/// (`smithy_query(query="shape",shape="AWSServicePrincipal")`, which the generated
/// `A_W_S_SERVICE_PRINCIPAL_VALUES` carries): a request naming any other service is
/// refused at the request boundary, so this is the one service a grant can name.
pub(crate) fn service_principal() -> &'static str {
    A_W_S_SERVICE_PRINCIPAL_VALUES[0]
}

/// The grant table: one row per way one accepted grant differs from another — which
/// member names the principal, the digits and length of an account id (`AWSAccount` is
/// `pattern ^[0-9]+$` with no length bound), and the service options carried with it.
fn grant_rows() -> Vec<Grant> {
    vec![
        Grant {
            label: "account/12-digit/no-options",
            body: json!({ "Account": "123456789012" }),
            principal: "123456789012".to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            service_options: None,
        },
        Grant {
            label: "account/all-zero/empty-options",
            body: json!({ "Account": "000000000000", "ServiceOptions": {} }),
            principal: "000000000000".to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            service_options: Some(json!({})),
        },
        Grant {
            label: "account/one-digit/one-region",
            body: json!({
                "Account": "7",
                "ServiceOptions": { "SupportedRegions": ["us-east-1"] },
            }),
            principal: "7".to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            service_options: Some(json!({ "SupportedRegions": ["us-east-1"] })),
        },
        Grant {
            label: "account/long/no-regions",
            body: json!({
                "Account": "98765432109876543210",
                "ServiceOptions": { "SupportedRegions": [] },
            }),
            principal: "98765432109876543210".to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            service_options: Some(json!({ "SupportedRegions": [] })),
        },
        Grant {
            label: "service/no-options",
            body: json!({ "Service": service_principal() }),
            principal: service_principal().to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_SERVICE,
            service_options: None,
        },
        Grant {
            label: "service/one-region",
            body: json!({
                "Service": service_principal(),
                "ServiceOptions": { "SupportedRegions": ["eu-west-1"] },
            }),
            principal: service_principal().to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_SERVICE,
            service_options: Some(json!({ "SupportedRegions": ["eu-west-1"] })),
        },
        Grant {
            label: "service/three-regions",
            body: json!({
                "Service": service_principal(),
                "ServiceOptions": {
                    "SupportedRegions": ["us-east-1", "eu-west-1", "ap-south-1"],
                },
            }),
            principal: service_principal().to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_SERVICE,
            service_options: Some(json!({
                "SupportedRegions": ["us-east-1", "eu-west-1", "ap-south-1"],
            })),
        },
    ]
}

/// Assert one reported `AuthorizedPrincipal` is the record `grant` asked for.
fn assert_is_grant(entry: &Value, grant: &Grant, case: &str) {
    assert_eq!(
        entry["Principal"].as_str(),
        Some(grant.principal.as_str()),
        "case {case}: expected Principal {:?}, got {entry:?}",
        grant.principal
    );
    assert_eq!(
        entry["PrincipalType"].as_str(),
        Some(grant.principal_type),
        "case {case}: expected PrincipalType {:?}, got {entry:?}",
        grant.principal_type
    );
    // The type reported for an ACCOUNT grantee is the value `PrincipalType` declares for
    // it. A SERVICE grantee's is NOT one of that enum's values — the service answers it
    // in words — so the modelled set is what an account's type is held to, and the value
    // itself is pinned by [`super::authorized_principal_type`].
    let reported_type = entry["PrincipalType"].as_str().unwrap_or_default();
    if grant.principal_type == PRINCIPAL_TYPE_AWS_ACCOUNT {
        assert!(
            PRINCIPAL_TYPE_VALUES.contains(&reported_type),
            "case {case}: PrincipalType {reported_type:?} is outside the modelled values \
             {PRINCIPAL_TYPE_VALUES:?}"
        );
    }
    match grant.service_options.as_ref() {
        Some(expected) => assert_eq!(
            entry.get("ServiceOptions"),
            Some(expected),
            "case {case}: expected ServiceOptions {expected:?}, got {entry:?}"
        ),
        None => assert!(
            entry.get("ServiceOptions").is_none(),
            "case {case}: a grant that carried no ServiceOptions must report no such \
             member, got {entry:?}"
        ),
    }
}

#[tokio::test]
async fn a_grant_persists_the_principal_it_names_and_the_listing_reports_that_record() {
    for grant in grant_rows() {
        let case = format!("AuthorizeVpcEndpointAccess({})", grant.label);
        // A fresh emulator per row, so the row's grant is the whole set and the
        // listing's answer is about this row alone.
        let emulator = Emulator::new();
        created_domain(&emulator, "search-target").await;

        let granted = authorize(&emulator, "search-target", grant.body.clone()).await;
        assert_eq!(
            granted.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            granted.status, granted.body
        );
        assert_is_grant(&granted.body["AuthorizedPrincipal"], &grant, &case);

        // Read back through the implemented reader: the grant was persisted, and the
        // listing reports the SAME record the grant answered with.
        let listed = list(&emulator, "search-target", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from the listing, got {} with {:?}",
            listed.status, listed.body
        );
        let entries = principal_list(&listed, &case);
        assert_eq!(
            entries.len(),
            1,
            "case {case}: expected exactly the granted principal, got {entries:?}"
        );
        assert_is_grant(&entries[0], &grant, &case);
        assert_eq!(
            entries[0], granted.body["AuthorizedPrincipal"],
            "case {case}: the listing must report the record the grant answered with, \
             got {:?} then {:?}",
            granted.body["AuthorizedPrincipal"], entries[0]
        );
        // The whole set is reported, so the answer carries no `NextToken` member.
        carries_no_token(&listed, &case);

        // The listing reported the set and changed nothing: a second one answers
        // exactly the same.
        let again = list(&emulator, "search-target", None).await;
        assert_eq!(
            again.body, listed.body,
            "case {case}: a read must not change what the next caller sees, got {:?} then {:?}",
            listed.body, again.body
        );
    }
}

#[tokio::test]
async fn every_kind_of_principal_the_model_declares_is_recorded_by_some_grant() {
    // The value the service ANSWERS for each kind `PrincipalType` declares: the enum's
    // own spelling for an account, and the words the service answers for a service.
    // Both are pinned as values by [`super::authorized_principal_type`]; here they only
    // say which kind a row of the grant table covers.
    let answered_for = |declared: &str| match declared {
        "AWS_ACCOUNT" => PRINCIPAL_TYPE_AWS_ACCOUNT,
        "AWS_SERVICE" => PRINCIPAL_TYPE_AWS_SERVICE,
        other => panic!(
            "PrincipalType declares the kind {other:?}, which no row of the grant table \
             names a principal of"
        ),
    };
    let covered: Vec<&str> = grant_rows()
        .iter()
        .map(|grant| grant.principal_type)
        .collect();
    for principal_type in PRINCIPAL_TYPE_VALUES {
        let answered = answered_for(principal_type);
        assert!(
            covered.contains(&answered),
            "case PrincipalType({principal_type}): no row of the grant table records a \
             principal of that kind, which the service answers {answered:?} for; covered \
             {covered:?}"
        );
    }
}

#[tokio::test]
async fn granting_a_principal_again_replaces_its_one_record_and_leaves_the_others() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-shared").await;

    // One grant per DISTINCT principal of the table, in table order, so the set ends up
    // holding exactly the distinct principals however many rows named each.
    let rows = grant_rows();
    for grant in &rows {
        let case = format!("AuthorizeVpcEndpointAccess({})", grant.label);
        let granted = authorize(&emulator, "search-shared", grant.body.clone()).await;
        assert_eq!(
            granted.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            granted.status, granted.body
        );
        assert_is_grant(&granted.body["AuthorizedPrincipal"], grant, &case);
    }

    let mut distinct: Vec<String> = rows.iter().map(|grant| grant.principal.clone()).collect();
    distinct.sort();
    distinct.dedup();

    let listed = all_principals(&emulator, "search-shared", "re-grant").await;
    let reported: Vec<String> = listed
        .iter()
        .map(|entry| principal_of(entry, "re-grant"))
        .collect();
    assert_eq!(
        reported, distinct,
        "case re-grant: a repeated grant must replace one record rather than add a \
         second, expected {distinct:?}, got {reported:?}"
    );

    // The record each principal is left with is the LAST grant of it: the service
    // principal was granted three times, and the options of the third stand.
    let last_service_grant = rows
        .iter()
        .rfind(|grant| grant.principal_type == PRINCIPAL_TYPE_AWS_SERVICE)
        .expect("the table grants a service principal");
    let entry = listed
        .iter()
        .find(|entry| principal_of(entry, "re-grant") == last_service_grant.principal)
        .expect("the service principal is reported");
    assert_is_grant(entry, last_service_grant, "re-grant/last-wins");
}

#[tokio::test]
async fn a_request_that_names_no_principal_or_two_is_refused_and_the_set_stands() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-guarded").await;

    // One accepted grant, so a refused request has something to leave untouched.
    let standing = grant_rows().remove(0);
    authorize(&emulator, "search-guarded", standing.body.clone()).await;

    let refused: Vec<(&str, Value)> = vec![
        // No principal at all: nothing to grant access to.
        ("no-principal", json!({})),
        // No principal, yet options for one.
        (
            "options-only",
            json!({ "ServiceOptions": { "SupportedRegions": ["us-east-1"] } }),
        ),
        // Two principals: one call grants access to one principal.
        (
            "both-kinds",
            json!({ "Account": "123456789012", "Service": service_principal() }),
        ),
        (
            "both-kinds-with-options",
            json!({
                "Account": "210987654321",
                "Service": service_principal(),
                "ServiceOptions": { "SupportedRegions": ["us-east-1"] },
            }),
        ),
        // A service principal outside the modelled enum, refused at the request
        // boundary.
        ("unmodelled-service", json!({ "Service": "ec2.amazonaws.com" })),
        // An account id the modelled pattern refuses.
        ("non-numeric-account", json!({ "Account": "12345678901a" })),
    ];

    for (label, body) in refused {
        let case = format!("AuthorizeVpcEndpointAccess({label})");
        let answer = authorize(&emulator, "search-guarded", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );

        // A refused grant stored nothing: the set still holds exactly the accepted one.
        let listed = list(&emulator, "search-guarded", None).await;
        let entries = principal_list(&listed, &case);
        assert_eq!(
            entries.len(),
            1,
            "case {case}: a refused grant must leave the set as it was, got {entries:?}"
        );
        assert_is_grant(&entries[0], &standing, &case);
    }
}

#[tokio::test]
async fn the_authorization_set_belongs_to_the_domain_the_request_names() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-one").await;
    created_domain(&emulator, "search-two").await;

    let grant = grant_rows().remove(0);
    authorize(&emulator, "search-one", grant.body.clone()).await;

    // Another domain of the same account holds its own set, which this grant is not in.
    let other = list(&emulator, "search-two", None).await;
    assert_eq!(
        other.status, 200,
        "case ListVpcEndpointAccess(search-two): expected 200, got {} with {:?}",
        other.status, other.body
    );
    assert!(
        principal_list(&other, "other-domain").is_empty(),
        "case ListVpcEndpointAccess(search-two): a domain nobody was granted access to \
         must report an empty list, got {:?}",
        other.body
    );

    // A name the registry does not resolve has no set to grant INTO — the grant needs
    // the domain — while the listing of such a name reports that empty set.
    let refused = authorize(&emulator, "search-absent", grant.body.clone()).await;
    assert_eq!(
        refused.status, RECORDED_NOT_FOUND_STATUS,
        "case authorize(search-absent): expected {RECORDED_NOT_FOUND_STATUS} for a domain the \
         registry does not hold, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case authorize(search-absent): expected ResourceNotFoundException, got {:?}",
        refused.body
    );
    let absent = list(&emulator, "search-absent", None).await;
    assert_eq!(
        absent.status, 200,
        "case list(search-absent): expected the listing to be served, got {} with {:?}",
        absent.status, absent.body
    );
    assert!(
        principal_list(&absent, "search-absent").is_empty(),
        "case list(search-absent): a name no domain is held under reports an empty list, got \
         {:?}",
        absent.body
    );

    // An authorization is access TO a domain, so it goes when the domain does.
    let deleted = emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/search-one"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(search-one): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    let after = list(&emulator, "search-one", None).await;
    assert_eq!(
        after.status, 200,
        "case ListVpcEndpointAccess(search-one after delete): expected the listing to be \
         served, got {} with {:?}",
        after.status, after.body
    );
    assert!(
        principal_list(&after, "search-one after delete").is_empty(),
        "case ListVpcEndpointAccess(search-one after delete): the grant went with the domain, \
         got {:?}",
        after.body
    );
}

#[tokio::test]
async fn one_answer_reports_the_whole_set_exactly_once_and_an_unresolvable_token_is_refused() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-paged").await;

    // Far more principals than the ten this listing used to cap an answer at, so a
    // capped answer could not pass.
    let count = 13;
    let mut granted: Vec<String> = Vec::new();
    for index in 0..count {
        let account = format!("{:012}", index + 1);
        let case = format!("AuthorizeVpcEndpointAccess({account})");
        let answer = authorize(&emulator, "search-paged", json!({ "Account": account })).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        granted.push(account);
    }
    granted.sort();

    let listed = all_principals(&emulator, "search-paged", "one answer").await;
    let reported: Vec<String> = listed
        .iter()
        .map(|entry| principal_of(entry, "one answer"))
        .collect();
    assert_eq!(
        listed.len(),
        count,
        "case one answer: expected all {count} principals in one answer, got {reported:?}"
    );
    assert_eq!(
        reported, granted,
        "case one answer: the answer reports the whole set exactly once, expected {granted:?}, \
         got {reported:?}"
    );

    // A token naming a principal the set holds is the place the answer starts at, so
    // resuming from it reports exactly the rest of the set.
    let resumed = list(&emulator, "search-paged", Some(&granted[4])).await;
    assert_eq!(
        resumed.status, 200,
        "case ListVpcEndpointAccess(resumed): expected 200, got {} with {:?}",
        resumed.status, resumed.body
    );
    carries_no_token(&resumed, "resumed");
    let tail: Vec<String> = principal_list(&resumed, "resumed")
        .iter()
        .map(|entry| principal_of(entry, "resumed"))
        .collect();
    assert_eq!(
        tail,
        granted[4..].to_vec(),
        "case ListVpcEndpointAccess(resumed): expected the rest of the set, got {tail:?}"
    );

    // The EMPTY token is the start of the set, exactly as naming no token is.
    let echoed = list(&emulator, "search-paged", Some("")).await;
    assert_eq!(
        echoed.status, 200,
        "case ListVpcEndpointAccess(nextToken=): expected 200 for the empty token, \
         got {} with {:?}",
        echoed.status, echoed.body
    );
    assert_eq!(
        principal_list(&echoed, "paging/echoed"),
        listed,
        "case ListVpcEndpointAccess(nextToken=): expected the whole set back, got {:?}",
        echoed.body
    );

    // A token this listing never handed out names no place in the set.
    for label in ["000000000000", "not-a-token", "99999999999999999999"] {
        let case = format!("ListVpcEndpointAccess(nextToken={label})");
        let answer = list(&emulator, "search-paged", Some(label)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 for a token naming no place in the set, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_authorization_set_is_bounded_per_domain() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-bounded").await;
    created_domain(&emulator, "search-spare").await;

    for index in 0..VPC_ENDPOINT_ACCESS_QUOTA {
        let account = format!("{:012}", index + 1);
        let case = format!("AuthorizeVpcEndpointAccess({account})");
        let answer = authorize(&emulator, "search-bounded", json!({ "Account": account })).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 inside the quota, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // One principal past the quota is refused rather than stored.
    let past = authorize(
        &emulator,
        "search-bounded",
        json!({ "Service": service_principal() }),
    )
    .await;
    assert_eq!(
        past.status, 409,
        "case AuthorizeVpcEndpointAccess(past the quota): expected 409, got {} with {:?}",
        past.status, past.body
    );
    assert_eq!(
        error_code(&past),
        "LimitExceededException",
        "case AuthorizeVpcEndpointAccess(past the quota): expected LimitExceededException, \
         got {:?}",
        past.body
    );

    let held = all_principals(&emulator, "search-bounded", "bounded").await;
    assert_eq!(
        held.len(),
        VPC_ENDPOINT_ACCESS_QUOTA,
        "case bounded: a refused grant must leave the set at the quota, got {} entries",
        held.len()
    );

    // A re-grant of a principal already in the set does not grow it, so the bound does
    // not refuse it.
    let regrant = authorize(
        &emulator,
        "search-bounded",
        json!({
            "Account": "000000000001",
            "ServiceOptions": { "SupportedRegions": ["us-west-2"] },
        }),
    )
    .await;
    assert_eq!(
        regrant.status, 200,
        "case AuthorizeVpcEndpointAccess(re-grant at the quota): expected 200, got {} \
         with {:?}",
        regrant.status, regrant.body
    );
    assert_eq!(
        regrant.body["AuthorizedPrincipal"]["ServiceOptions"],
        json!({ "SupportedRegions": ["us-west-2"] }),
        "case AuthorizeVpcEndpointAccess(re-grant at the quota): the re-grant must \
         replace the record, got {:?}",
        regrant.body
    );
    let after = all_principals(&emulator, "search-bounded", "bounded/re-grant").await;
    assert_eq!(
        after.len(),
        VPC_ENDPOINT_ACCESS_QUOTA,
        "case bounded/re-grant: a re-grant must not grow the set, got {} entries",
        after.len()
    );

    // The quota partitions by DOMAIN: another domain's set is empty and takes a grant.
    let spare = authorize(
        &emulator,
        "search-spare",
        json!({ "Service": service_principal() }),
    )
    .await;
    assert_eq!(
        spare.status, 200,
        "case AuthorizeVpcEndpointAccess(search-spare): a full domain must not exhaust \
         another's allowance, got {} with {:?}",
        spare.status, spare.body
    );
}
