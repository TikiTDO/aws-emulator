// SPDX-License-Identifier: CC0-1.0

//! Taking a WAF web ACL off a distribution — `DisassociateDistributionWebACL` — end to
//! end over HTTP.
//!
//! The sibling suite [`crate::web_acl_association`] owns the other half of this surface
//! (`AssociateDistributionWebACL`) and the shared helpers both use: the web ACL forms
//! under test, the create/read-back pair, the association call and the propagation wait.
//! This suite states the removing rule:
//!
//! * a disassociation CLEARS the one member of the stored configuration that names the
//!   association (`DistributionConfig$WebACLId`, whose empty value is what "no web ACL"
//!   is) and leaves every other member of that configuration exactly as it was — whatever
//!   the association held, and however it got there: named in the configuration the create
//!   submitted, or set by a later association call;
//! * it is a configuration write like any other, so it mints a fresh version token and
//!   sends the distribution back to `InProgress` until the CLOCK owes it `Deployed`; the
//!   version it replaced no longer satisfies an `If-Match`, and asking again for what is
//!   already true is the same answer again;
//! * `If-Match` is the OPTIONAL conditional header the model declares: absent, it states
//!   no precondition on a distribution the store HOLDS; stating the current version (bare
//!   or quoted) it lands; stating a version that is not current fails the precondition
//!   (HTTP 412); stating a value that is no version token at all is refused as an invalid
//!   `If-Match` version (HTTP 400); an id that ADDRESSES a distribution the store does not
//!   hold is `EntityNotFound` (HTTP 404); and because the header is evaluated against the
//!   current version of a distribution the id can address, an id that addresses none earns
//!   `InvalidIfMatchVersion` (HTTP 400) instead. No refusal writes anything a later read
//!   can see.
//!
//! Every assertion travels the router, so each case exercises routing, the restXml
//! reader, the handler, the persisted state and serialization.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    STATUS_DEPLOYED, STATUS_IN_PROGRESS,
};

use crate::distribution_lifecycle::{
    call, config, config_table, get_distribution, is_version_token, past_propagation, Answer,
    Config, API_VERSION,
};
use crate::web_acl_association::{associate_web_acl, create, read_back, Stated, WEB_ACLS};

// The sentences this service answers these two codes with, spelled out here rather than
// imported from the implementation: each is fixed service text naming nothing about the
// request, so a constant edited to some other wording fails these rows.
/// `InvalidIfMatchVersion`, as the recorded responses carry it.
const INVALID_IF_MATCH_VERSION: &str =
    "The If-Match version is missing or not valid for the resource.";

/// `PreconditionFailed`, as the recorded responses carry it on the web-ACL association
/// surface: this family reports the request as missing a requirement rather than reporting
/// the precondition of the request's header fields.
const PRECONDITION_FAILED: &str = "Your request is missing requirements for this \
     operation. Update your request and try again.";

/// One `DisassociateDistributionWebACL` call: the distribution in the path and the version
/// (when the row states one) in the `If-Match` header. The operation carries no body.
async fn disassociate_web_acl(
    router: &MultiServiceRouter,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/disassociate-web-acl"),
        None,
        if_match,
    )
    .await
}

// The `If-Match` a row sends is resolved by `Stated::header`, which the sibling
// association suite owns — one definition of the form-to-header mapping for both.

/// How the distribution came to name the web ACL a disassociation takes off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reached {
    /// Named in the `DistributionConfig` the create submitted.
    ByCreate,
    /// Named by a later `AssociateDistributionWebACL` call.
    ByAssociate,
    /// Never associated with anything: the member reads as the empty value.
    Never,
}

/// One row of the disassociation table: what the distribution is associated with, how it
/// got there, and what the removing call states as its `If-Match`.
#[derive(Debug, Clone, Copy)]
struct Removal {
    case: &'static str,
    /// An index into [`WEB_ACLS`], or `None` for a distribution that names no web ACL.
    web_acl: Option<usize>,
    reached: Reached,
    precondition: Stated,
}

/// Every form the association can take (the [`WEB_ACLS`] forms: a `wafv2` ARN of the global
/// namespace in two accounts, another partition, a WAF Classic ARN and a bare WAF Classic
/// id), reached both ways a configuration can come to name a web ACL, plus the
/// distribution that never named one — crossed with the forms the optional `If-Match`
/// takes.
const REMOVALS: &[Removal] = &[
    Removal {
        case: "wafv2-global-from-the-create-config",
        web_acl: Some(0),
        reached: Reached::ByCreate,
        precondition: Stated::Current,
    },
    Removal {
        case: "wafv2-global-other-account-from-an-association",
        web_acl: Some(1),
        reached: Reached::ByAssociate,
        precondition: Stated::Absent,
    },
    Removal {
        case: "other-partition-from-an-association",
        web_acl: Some(2),
        reached: Reached::ByAssociate,
        precondition: Stated::QuotedCurrent,
    },
    Removal {
        case: "waf-classic-arn-from-the-create-config",
        web_acl: Some(3),
        reached: Reached::ByCreate,
        precondition: Stated::Absent,
    },
    Removal {
        case: "waf-classic-id-from-an-association",
        web_acl: Some(4),
        reached: Reached::ByAssociate,
        precondition: Stated::Current,
    },
    Removal {
        case: "never-associated",
        web_acl: None,
        reached: Reached::Never,
        precondition: Stated::Current,
    },
];

/// A disassociation clears the stored web ACL whatever it held and however it got there,
/// leaves the rest of the configuration alone, mints a version the distribution then
/// reports, and sends it back into propagation; the version it replaced no longer
/// satisfies an `If-Match`, and asking again is the same answer again.
#[tokio::test]
async fn disassociating_a_web_acl_clears_it_from_the_distribution() {
    for row in REMOVALS {
        let case = row.case;
        let router = crate::distribution_lifecycle::emulator();
        let web_acl = row.web_acl.map(|index| WEB_ACLS[index].1);

        // The configuration each row creates: one naming the web ACL itself where the row
        // reaches the association through the create, one naming none otherwise. Rows also
        // differ in origin count, alias count, enabled flag, viewer protocol policy and
        // price class, through the lifecycle suite's `DistributionConfig` builder.
        let template = Config {
            web_acl_id: match row.reached {
                Reached::ByCreate => web_acl.map(str::to_string),
                Reached::ByAssociate | Reached::Never => None,
            },
            ..config(case, &format!("ref-disassoc-{case}"))
        };
        let (id, created_etag) = create(&router, case, &template.xml()).await;

        // However the row reaches it, the record holds the row's association before the
        // disassociation — and the empty value where the row names no web ACL.
        let mut etag = created_etag.clone();
        if row.reached == Reached::ByAssociate {
            let associated =
                associate_web_acl(&router, &id, web_acl.unwrap_or_default(), Some(&etag)).await;
            assert_eq!(
                associated.status, 200,
                "case {case}: expected HTTP 200 from AssociateDistributionWebACL, got {} with {}",
                associated.status, associated.raw
            );
            etag = associated.header("etag").to_string();
        }
        let (before, _, _) = read_back(&router, &id, case).await;
        assert_eq!(
            before,
            web_acl.unwrap_or_default(),
            "case {case}: expected the distribution to name web ACL {:?} before the \
             disassociation, got {before:?}",
            web_acl.unwrap_or_default()
        );

        // Let the last write finish propagating, so the status the disassociation leaves
        // behind is the one it owes rather than one left over from the setup.
        tokio::time::sleep(past_propagation()).await;
        let (_, _, status) = read_back(&router, &id, case).await;
        assert_eq!(
            status, STATUS_DEPLOYED,
            "case {case}: expected {STATUS_DEPLOYED} before the disassociation, got {status:?}"
        );

        let sent = row.precondition.header(&etag);
        let answer = disassociate_web_acl(&router, &id, sent.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 from DisassociateDistributionWebACL, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the answer to name distribution {id:?}, got {:?}",
            answer.root().text_at("Id")
        );
        let cleared_etag = answer.header("etag").to_string();
        assert!(
            is_version_token(&cleared_etag),
            "case {case}: expected a minted version token in the ETag header, got \
             {cleared_etag:?}"
        );
        assert_ne!(
            cleared_etag, etag,
            "case {case}: expected a version other than {etag:?}, got {cleared_etag:?}"
        );

        // The one record answers every read: no web ACL, the version the call minted, and
        // the propagation its new configuration is owed.
        let (stored, read_etag, status) = read_back(&router, &id, case).await;
        assert_eq!(
            stored, "",
            "case {case}: expected the distribution to name no web ACL, got {stored:?}"
        );
        assert_eq!(
            read_etag, cleared_etag,
            "case {case}: expected GetDistribution at version {cleared_etag:?}, got {read_etag:?}"
        );
        assert_eq!(
            status, STATUS_IN_PROGRESS,
            "case {case}: expected the reconfigured distribution to report \
             {STATUS_IN_PROGRESS}, got {status:?}"
        );
        // Nothing else about the configuration moved: it still reads back as the one the
        // create submitted, the association aside.
        let fetched = get_distribution(&router, &id).await;
        Config {
            web_acl_id: Some(String::new()),
            ..template.clone()
        }
        .assert_matches(
            fetched
                .root()
                .child("DistributionConfig")
                .unwrap_or_else(|| panic!("case {case}: no DistributionConfig")),
            "GetDistribution after DisassociateDistributionWebACL",
        );

        // The version the call replaced is no longer current.
        let stale = disassociate_web_acl(&router, &id, Some(&etag)).await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected HTTP 412 for the replaced version {etag:?}, got {} with {}",
            stale.status, stale.raw
        );

        // Asking again for what is already true is the same answer again.
        let again = disassociate_web_acl(&router, &id, Some(&cleared_etag)).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected HTTP 200 from the repeated disassociation, got {} with {}",
            again.status, again.raw
        );
        let repeated_etag = again.header("etag").to_string();
        let (stored, read_etag, _) = read_back(&router, &id, case).await;
        assert_eq!(
            stored, "",
            "case {case}: expected the repeated disassociation to leave no web ACL, got \
             {stored:?}"
        );
        assert_eq!(
            read_etag, repeated_etag,
            "case {case}: expected the repeated disassociation's version {repeated_etag:?}, got \
             {read_etag:?}"
        );

        // And the deployment it owes is the CLOCK's, not another call's.
        tokio::time::sleep(past_propagation()).await;
        let (_, _, status) = read_back(&router, &id, case).await;
        assert_eq!(
            status, STATUS_DEPLOYED,
            "case {case}: expected {STATUS_DEPLOYED} once the propagation was owed, got \
             {status:?}"
        );
    }
}

/// One refused disassociation: what the request names and states, and what it earns.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    case: &'static str,
    /// `None` names the stored distribution; `Some(id)` names that id instead.
    id: Option<&'static str>,
    precondition: Stated,
    status: u16,
    code: &'static str,
    /// The sentence the answer states, where the code carries fixed service text.
    message: Option<&'static str>,
}

/// Every condition the disassociation refuses: a distribution the store does not hold under
/// an id that could ADDRESS one (whether or not a version is stated), an id that addresses
/// none at all (the header is evaluated first, and no version could be valid for such an
/// id), a well-formed version that is not current, and four values that are no version token
/// at all.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "never-minted-id",
        id: Some("EDFDVBD6EXAMPLE"),
        precondition: Stated::Absent,
        status: 400,
        code: "InvalidIfMatchVersion",
        message: Some(INVALID_IF_MATCH_VERSION),
    },
    Refusal {
        case: "an-addressable-id-no-distribution-carries",
        id: Some("E1ABCDEFGHIJKL"),
        precondition: Stated::Literal("E1STALEVERSION"),
        status: 404,
        code: "EntityNotFound",
        message: None,
    },
    Refusal {
        case: "not-an-id-at-all",
        id: Some("nope"),
        precondition: Stated::Literal("E1STALEVERSION"),
        status: 400,
        code: "InvalidIfMatchVersion",
        message: Some(INVALID_IF_MATCH_VERSION),
    },
    Refusal {
        case: "stale-version",
        id: None,
        precondition: Stated::Literal("E1STALEVERSION"),
        status: 412,
        code: "PreconditionFailed",
        message: Some(PRECONDITION_FAILED),
    },
    Refusal {
        case: "not-a-version-token",
        id: None,
        precondition: Stated::Literal("not-a-version"),
        status: 400,
        code: "InvalidIfMatchVersion",
        message: Some(INVALID_IF_MATCH_VERSION),
    },
    Refusal {
        case: "empty-if-match",
        id: None,
        precondition: Stated::Literal(""),
        status: 400,
        code: "InvalidIfMatchVersion",
        message: Some(INVALID_IF_MATCH_VERSION),
    },
    Refusal {
        case: "lowercase-version-token",
        id: None,
        precondition: Stated::Literal("e1abcdefghijklm"),
        status: 400,
        code: "InvalidIfMatchVersion",
        message: Some(INVALID_IF_MATCH_VERSION),
    },
    Refusal {
        case: "version-token-one-character-short",
        id: None,
        precondition: Stated::Literal("E1ABCDEFGHIJK"),
        status: 400,
        code: "InvalidIfMatchVersion",
        message: Some(INVALID_IF_MATCH_VERSION),
    },
];

/// A refused disassociation answers the modelled error for its condition and writes
/// nothing: the web ACL, the version and the readability of the record all stand, and the
/// version those refusals left in place still lands the call.
#[tokio::test]
async fn a_refused_disassociation_writes_nothing() {
    let router = crate::distribution_lifecycle::emulator();
    let web_acl = WEB_ACLS[0].1;
    let row = &config_table()[0];
    let (id, created_etag) = create(&router, "refusals", &row.xml()).await;
    let associated = associate_web_acl(&router, &id, web_acl, Some(&created_etag)).await;
    assert_eq!(
        associated.status, 200,
        "case refusals: expected HTTP 200 from AssociateDistributionWebACL, got {} with {}",
        associated.status, associated.raw
    );
    let etag = associated.header("etag").to_string();

    for refusal in REFUSALS {
        let case = refusal.case;
        let target = refusal.id.unwrap_or(id.as_str());
        let sent = refusal.precondition.header(&etag);
        let answer = disassociate_web_acl(&router, target, sent.as_deref()).await;
        assert_eq!(
            answer.status, refusal.status,
            "case {case}: expected HTTP {} from If-Match {sent:?}, got {} with {}",
            refusal.status, answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            refusal.code,
            "case {case}: expected error code {:?}, got {:?} with {}",
            refusal.code,
            answer.error_code(),
            answer.raw
        );
        if let Some(expected) = refusal.message {
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case}: expected the message {expected:?}, got {message:?}"
            );
        }

        let (stored, read_etag, _) = read_back(&router, &id, case).await;
        assert_eq!(
            stored, web_acl,
            "case {case}: expected the refusal to leave web ACL {web_acl:?}, got {stored:?}"
        );
        assert_eq!(
            read_etag, etag,
            "case {case}: expected the refusal to leave the version at {etag:?}, got \
             {read_etag:?}"
        );
    }

    let landed = disassociate_web_acl(&router, &id, Some(&etag)).await;
    assert_eq!(
        landed.status, 200,
        "case after-refusals: expected HTTP 200 once the current version is stated, got {} with {}",
        landed.status, landed.raw
    );
    let (stored, _, _) = read_back(&router, &id, "after-refusals").await;
    assert_eq!(
        stored, "",
        "case after-refusals: expected the landed call to clear the web ACL, got {stored:?}"
    );
}
