// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which of the three VPC endpoint access operations resolves the
//! `DomainName` in its path before answering.
//!
//! The rule under test: **of the operations over a domain's VPC endpoint access set, only
//! the two that CHANGE the set — `AuthorizeVpcEndpointAccess` and
//! `RevokeVpcEndpointAccess` — resolve the domain named in the path first and refuse a
//! name no domain is held under with the modelled missing-resource error; the one that
//! only REPORTS the set, `ListVpcEndpointAccess`, makes the domain's existence no
//! precondition and answers 200 with the set that name holds, which for such a name is
//! the empty set, reported exactly as a domain that exists and allows nobody reports
//! it.** The requirement is therefore per OPERATION, not per resource: a client walking a
//! set of names is told each name's grants rather than stopped at the first name that is
//! gone.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. This suite adds no second copy of anything the
//! sibling suites already have: the harness comes from [`super::domain_lifecycle`], the
//! table of names no domain is held under (and the `DeleteDomain` that makes one of them)
//! from [`super::package_listing_for_missing_domain`], the grant and listing calls from
//! [`super::vpc_endpoint_access`] and the withdrawal from
//! [`super::vpc_endpoint_access_revocation`]. The wording of the refusal is asserted in
//! [`super::fixed_sentence_refusals`], which owns that sentence, so these rows assert the
//! status and the error code only.
//!
//! What is NEW here, beside the sibling suites: the three operations are crossed with the
//! SAME table of names in one place, so the asymmetry between the read and BOTH mutating
//! calls is pinned rather than each call's own answer; the listing's answer for such a
//! name is compared member for member against the answer a held domain with no grants
//! gives; and the start of a page is asserted to be the start of that empty set, whether
//! the request names no token or the empty one.
//!
//! Which members that answer carries beside `AuthorizedPrincipalList` is therefore
//! asserted RELATIVELY here — the answer for a name no domain is held under is the answer
//! a served listing gives, whatever that is — because the recorded evidence for this rule
//! pins the list being present and empty and says nothing about a continuation token.
//! What a complete result set reports for the token member is one rule for every listing
//! answer of this operation, and [`super::vpc_endpoint_access`] owns it.

use serde_json::json;

use super::domain_lifecycle::{error_code, Answer, Emulator, RECORDED_NOT_FOUND_STATUS};
use super::package_listing_for_missing_domain::missing_names;
use super::vpc_endpoint_access::{authorize, created_domain, list, principal_list, principal_of};
use super::vpc_endpoint_access_revocation::revoke;

/// A domain this account DOES hold, with one principal authorized on it: the control that
/// tells a listing reading the stored set apart from one that answers the empty set
/// whatever it is asked.
const GRANTED_DOMAIN: &str = "access-present";

/// A domain this account holds that nobody was granted access to: the answer a name no
/// domain is held under must be indistinguishable from.
const EMPTY_DOMAIN: &str = "access-empty";

/// The principal [`GRANTED_DOMAIN`] allows.
const GRANTED_PRINCIPAL: &str = "123456789012";

/// The emulator every table starts from: one domain with a grant, one without.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    created_domain(&emulator, GRANTED_DOMAIN).await;
    created_domain(&emulator, EMPTY_DOMAIN).await;
    let granted = authorize(
        &emulator,
        GRANTED_DOMAIN,
        json!({ "Account": GRANTED_PRINCIPAL }),
    )
    .await;
    assert_eq!(
        granted.status, 200,
        "case AuthorizeVpcEndpointAccess({GRANTED_DOMAIN}): expected 200, got {} with {:?}",
        granted.status, granted.body
    );
    emulator
}

/// The `Principal` of every `AuthorizedPrincipal` a listing answer reports, in the order
/// it reports them — each read through [`principal_of`], the one reader of that member.
fn principals(answer: &Answer, case: &str) -> Vec<String> {
    principal_list(answer, case)
        .iter()
        .map(|entry| principal_of(entry, case))
        .collect()
}

/// Assert a listing answer is served with the empty set.
fn assert_empty_page(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the listing to be served with 200, got {} with {:?}",
        answer.status, answer.body
    );
    let entries = principal_list(answer, case);
    assert!(
        entries.is_empty(),
        "case {case}: expected an empty AuthorizedPrincipalList, got {entries:?}"
    );
}

#[tokio::test]
async fn only_the_mutating_siblings_resolve_the_domain_the_request_names() {
    let emulator = seeded().await;

    // The answer a HELD domain that allows nobody gives: what a name no domain is held
    // under must be indistinguishable from, member for member.
    let held_empty = list(&emulator, EMPTY_DOMAIN, None).await;
    assert_empty_page(
        &held_empty,
        &format!("ListVpcEndpointAccess({EMPTY_DOMAIN}, a held domain with no grants)"),
    );

    for (row, name) in missing_names(&emulator).await {
        // Both calls that CHANGE the set cannot be served without the domain, each with
        // the error the model declares for a resource that is not there.
        for (form, answer) in [
            (
                "AuthorizeVpcEndpointAccess",
                authorize(&emulator, &name, json!({ "Account": GRANTED_PRINCIPAL })).await,
            ),
            (
                "RevokeVpcEndpointAccess",
                revoke(&emulator, &name, json!({ "Account": GRANTED_PRINCIPAL })).await,
            ),
        ] {
            let case = format!("{form}({name:?}, {row})");
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
                "case {case}: expected the modelled missing-resource refusal, got {} / {} \
                 with {:?}",
                answer.status,
                error_code(&answer),
                answer.body
            );
        }

        // The call that only REPORTS the set is served: the name holds no set, which is
        // the empty one, reported exactly as a held domain with no grants reports it.
        let case = format!("ListVpcEndpointAccess({name:?}, {row})");
        let listed = list(&emulator, &name, None).await;
        assert_empty_page(&listed, &case);
        assert_eq!(
            listed.body, held_empty.body,
            "case {case}: expected the same answer a held domain with no grants gives \
             ({:?}), got {:?}",
            held_empty.body, listed.body
        );
    }

    // And the listing of the name that IS held still reports its grant, so the empty
    // answers above are those names' stored sets and not a blanket empty answer.
    let case = format!("ListVpcEndpointAccess({GRANTED_DOMAIN}, after the absent rows)");
    let still = list(&emulator, GRANTED_DOMAIN, None).await;
    assert_eq!(
        still.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        still.status, still.body
    );
    let reported = principals(&still, &case);
    assert_eq!(
        reported,
        vec![GRANTED_PRINCIPAL.to_string()],
        "case {case}: expected the one granted principal, got {reported:?}"
    );
}

#[tokio::test]
async fn a_listing_of_a_name_no_domain_is_held_under_starts_at_the_start_of_the_empty_set() {
    let emulator = seeded().await;

    // (what tells the row apart, the token sent). The EMPTY token means the start of a
    // set, exactly as naming no token does, so both ask for the whole set.
    let tokens: [(&str, Option<&str>); 2] =
        [("no token at all", None), ("the empty token", Some(""))];
    let names = missing_names(&emulator).await;

    for (label, token) in tokens {
        for (row, name) in &names {
            let answer = list(&emulator, name, token).await;
            assert_empty_page(
                &answer,
                &format!("ListVpcEndpointAccess({name:?}, {label}) / {row}"),
            );
        }

        // The same request against the domain that IS held asks for ITS whole set, so the
        // empty answers above are the sets of those names and not what this token means.
        let case = format!("ListVpcEndpointAccess({GRANTED_DOMAIN}, {label})");
        let answer = list(&emulator, GRANTED_DOMAIN, token).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = principals(&answer, &case);
        assert_eq!(
            reported,
            vec![GRANTED_PRINCIPAL.to_string()],
            "case {case}: expected the one granted principal, got {reported:?}"
        );
    }
}
