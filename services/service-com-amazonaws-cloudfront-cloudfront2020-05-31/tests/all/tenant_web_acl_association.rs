// SPDX-License-Identifier: CC0-1.0

//! Associating and disassociating a WAF web ACL of a distribution TENANT, end to end over
//! HTTP.
//!
//! The rule under test: a tenant's web ACL lives in ONE member of its stored record —
//! `Customizations$WebAcl`, "the WAF web ACL" the tenant overrides or disables — so
//! associating one writes that member in `override` form with the ARN the request named,
//! disassociating removes it (and drops an otherwise empty `Customizations` altogether),
//! every other customization is carried over untouched, and each call versions the tenant
//! like any other change to its configuration. Around that rule:
//!
//! * the tenant's current version is a REQUIRED precondition on both calls: a request that
//!   states no `If-Match` at all is refused with `InvalidIfMatchVersion` (HTTP 400), a
//!   well-formed version that is not the current one fails the precondition (HTTP 412), and
//!   a value that is no version token is `InvalidIfMatchVersion` too. A refused call
//!   changes nothing a later read can see;
//! * a tenant id the store does not hold is `EntityNotFound` (HTTP 404) — the error both
//!   operations model for an entity they cannot find;
//! * a disassociation is IDEMPOTENT: a tenant with no web ACL customization is answered,
//!   not refused, and ends in the same state as one that had its customization removed;
//! * a request that names no web ACL at all is `InvalidArgument` (HTTP 400): a web ACL is a
//!   WAF resource this service cannot resolve, so whether one was named is the only thing
//!   about the value the association decides;
//! * the propagation a write owes the tenant is the CLOCK's, as it is for a distribution.
//!
//! The rows differ along what the rule generalises over: the FORM of the web ACL named
//! (each entry of [`crate::web_acl_association::WEB_ACLS`] — the WAF generation, the
//! account, the partition and the bare WAF Classic id), and what the tenant already carries
//! (no customizations at all, customizations that are not about a web ACL, a web ACL
//! already overridden, and a web ACL explicitly disabled).
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, and the tenants
//! are created through [`crate::distribution_tenant_lifecycle`]'s helpers, so these
//! assertions travel the same path a client takes.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CUSTOMIZATION_ACTION_OVERRIDE, INVALID_IF_MATCH_VERSION_MESSAGE, STATUS_DEPLOYED,
    STATUS_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_version_token, past_propagation, Answer, Node, API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::web_acl_association::{Stated, WEB_ACLS};

// ---------------------------------------------------------------------------
// Calling the tenant web ACL surface
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/distribution-tenant/{Id}/associate-web-acl`: the web ACL travels in the
/// request document, the version (when the row states one) in the `If-Match` header.
///
/// `pub(crate)` so a sibling suite that needs a tenant's web ACL customization written by a
/// CALL rather than by a create reaches it over this one wire path.
pub(crate) async fn associate(
    router: &MultiServiceRouter,
    id: &str,
    web_acl_arn: &str,
    if_match: Option<&str>,
) -> Answer {
    let body = format!(
        "<AssociateDistributionTenantWebACLRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <WebACLArn>{}</WebACLArn></AssociateDistributionTenantWebACLRequest>",
        escape(web_acl_arn)
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution-tenant/{id}/associate-web-acl"),
        Some(&body),
        if_match,
    )
    .await
}

/// `PUT /2020-05-31/distribution-tenant/{Id}/disassociate-web-acl`. The operation's input is
/// the id and the precondition alone, so the request carries no document.
///
/// `pub(crate)` so the suite that asserts how far one disassociation reaches, and the one
/// that watches a tenant leave a customization listing, each call it over the same wire path
/// rather than growing a copy of this.
pub(crate) async fn disassociate(
    router: &MultiServiceRouter,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution-tenant/{id}/disassociate-web-acl"),
        None,
        if_match,
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant/{Id}`, asserted served.
///
/// `pub(crate)` so a sibling suite reads a tenant back through the same implemented reader.
pub(crate) async fn read_tenant(router: &MultiServiceRouter, id: &str, case: &str) -> Answer {
    let answer = call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution-tenant/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from GetDistributionTenant, got {} with {}",
        answer.status, answer.raw
    );
    answer
}

/// The tenant's web ACL customization as a read reports it: `(Action, Arn)`, or `None` when
/// the record carries no `Customizations$WebAcl` at all. An `Arn` the customization leaves
/// out reads as the empty string.
///
/// `pub(crate)` so a sibling suite reports a tenant's web ACL customization in the same
/// terms.
pub(crate) fn web_acl_of(tenant: &Node) -> Option<(String, String)> {
    let web_acl = tenant.at("Customizations/WebAcl")?;
    Some((
        web_acl.text_at("Action").unwrap_or_default().to_string(),
        web_acl.text_at("Arn").unwrap_or_default().to_string(),
    ))
}

// ---------------------------------------------------------------------------
// The tenants under test
// ---------------------------------------------------------------------------

/// A `Customizations` element that names geographic restrictions and NO web ACL — the
/// customizations a web ACL association must carry over untouched. `LocationList`'s member
/// carries `xmlName` `Location` (`smithy_query(query="shape",shape="LocationList")`).
const GEO_ONLY: &str = "<Customizations>\
     <GeoRestrictions><RestrictionType>whitelist</RestrictionType>\
     <Locations><Location>DE</Location><Location>JP</Location></Locations>\
     </GeoRestrictions></Customizations>";

/// A `Customizations` element that already OVERRIDES a web ACL, beside geographic
/// restrictions.
const OVERRIDDEN: &str = "<Customizations>\
     <WebAcl><Action>override</Action>\
     <Arn>arn:aws:wafv2:us-east-1:000000000000:global/webacl/first/aaa</Arn></WebAcl>\
     <GeoRestrictions><RestrictionType>blacklist</RestrictionType>\
     <Locations><Location>CU</Location></Locations>\
     </GeoRestrictions></Customizations>";

/// A `Customizations` element that DISABLES the distribution's web ACL for the tenant —
/// `CustomizationActionType` is `override | disable`
/// (`smithy_query(query="shape",shape="CustomizationActionType")`) — and names nothing
/// else, so removing the customization empties the set.
const DISABLED: &str = "<Customizations><WebAcl><Action>disable</Action></WebAcl>\
     </Customizations>";

/// One starting point a tenant is in before a web ACL is associated with it.
///
/// `pub(crate)` so a sibling suite puts its tenants in these same starting states.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Start {
    pub(crate) case: &'static str,
    /// The `Customizations` element the create submits, or `None` to leave the member out.
    pub(crate) customizations: Option<&'static str>,
    /// The `(Action, Arn)` a read reports before the association, or `None` for a record
    /// that carries no web ACL customization.
    pub(crate) web_acl_before: Option<(&'static str, &'static str)>,
    /// Whether the tenant carries a customization OTHER than the web ACL, which every call
    /// here must leave in place.
    pub(crate) other_customizations: bool,
}

/// The starting points every rule below is asserted over: a tenant with no customizations
/// at all, one whose customizations are not about a web ACL, one whose web ACL is already
/// overridden (so the association replaces it) and one whose web ACL is disabled.
///
/// `pub(crate)` so a sibling suite spans the same starting states.
pub(crate) const STARTS: &[Start] = &[
    Start {
        case: "no customizations at all",
        customizations: None,
        web_acl_before: None,
        other_customizations: false,
    },
    Start {
        case: "customizations that name no web ACL",
        customizations: Some(GEO_ONLY),
        web_acl_before: None,
        other_customizations: true,
    },
    Start {
        case: "a web ACL already overridden",
        customizations: Some(OVERRIDDEN),
        web_acl_before: Some((
            "override",
            "arn:aws:wafv2:us-east-1:000000000000:global/webacl/first/aaa",
        )),
        other_customizations: true,
    },
    Start {
        case: "the web ACL disabled",
        customizations: Some(DISABLED),
        web_acl_before: Some(("disable", "")),
        other_customizations: false,
    },
];

/// Create one distribution tenant under `distribution_id`, in the starting state `start`,
/// and report `(Id, ETag)`.
///
/// `slug` names the tenant and its domain, which are both unique in the account.
///
/// `pub(crate)` so a sibling suite creates its tenants over the same wire path.
pub(crate) async fn create_tenant(
    router: &MultiServiceRouter,
    distribution_id: &str,
    slug: &str,
    case: &str,
    start: Start,
) -> (String, String) {
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{slug}</Name>\
         <Domains><member><Domain>{slug}.example.com</Domain></member></Domains>\
         <Parameters><member><Name>tenantName</Name><Value>{slug}</Value></member></Parameters>\
         {}{}</CreateDistributionTenantRequest>",
        escape(distribution_id),
        start.customizations.unwrap_or_default(),
        managed_certificate_request()
    );
    let created = create_distribution_tenant(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateDistributionTenant, got {} with {}",
        created.status, created.raw
    );
    (
        created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
        created.header("etag").to_string(),
    )
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// The association writes the tenant's web ACL customization and the disassociation takes
/// it off again: the calls report the tenant and a fresh version token, every later read
/// answers from the one record, the tenant's other customizations survive both calls, and a
/// second disassociation is a no-op rather than a failure.
#[tokio::test]
async fn associating_and_disassociating_move_the_tenants_web_acl() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-web-acl").await;

    for (acl_index, (acl_case, web_acl)) in WEB_ACLS.iter().enumerate() {
        for (start_index, start) in STARTS.iter().enumerate() {
            let case = format!("{acl_case}/{}", start.case);
            let slug = format!("t{acl_index}x{start_index}");
            let (id, created_etag) =
                create_tenant(&router, &distribution_id, &slug, &case, *start).await;

            // What the create stored is what the read answers from, web ACL customization
            // included.
            let before = read_tenant(&router, &id, &case).await;
            let reported = web_acl_of(before.root());
            let expected = start
                .web_acl_before
                .map(|(action, arn)| (action.to_string(), arn.to_string()));
            assert_eq!(
                reported, expected,
                "case {case}: expected the created tenant to report web ACL customization \
                 {expected:?}, got {reported:?}"
            );

            // Associating names the tenant, echoes the web ACL now stored on it, and mints
            // a version token other than the one it was read at.
            let associated = associate(&router, &id, web_acl, Some(&created_etag)).await;
            assert_eq!(
                associated.status, 200,
                "case {case}: expected HTTP 200 from AssociateDistributionTenantWebACL, got {} \
                 with {}",
                associated.status, associated.raw
            );
            assert_eq!(
                associated.root().text_at("Id"),
                Some(id.as_str()),
                "case {case}: expected the answer to name tenant {id:?}, got {:?}",
                associated.root().text_at("Id")
            );
            assert_eq!(
                associated.root().text_at("WebACLArn"),
                Some(*web_acl),
                "case {case}: expected the answer to name web ACL {web_acl:?}, got {:?}",
                associated.root().text_at("WebACLArn")
            );
            let associated_etag = associated.header("etag").to_string();
            assert!(
                is_version_token(&associated_etag),
                "case {case}: expected a minted version token in the ETag header, got \
                 {associated_etag:?}"
            );
            assert_ne!(
                associated_etag, created_etag,
                "case {case}: expected the association to mint a version token other than \
                 {created_etag:?}, got {associated_etag:?}"
            );

            // The one record answers the read: the web ACL this call associated, in
            // `override` form, at the version it minted.
            let read = read_tenant(&router, &id, &case).await;
            assert_eq!(
                web_acl_of(read.root()),
                Some((
                    CUSTOMIZATION_ACTION_OVERRIDE.to_string(),
                    (*web_acl).to_string()
                )),
                "case {case}: expected the read to report the associated web ACL, got {:?} in {}",
                web_acl_of(read.root()),
                read.raw
            );
            assert_eq!(
                read.header("etag"),
                associated_etag,
                "case {case}: expected the read at version {associated_etag:?}, got {:?}",
                read.header("etag")
            );
            assert_eq!(
                read.root().at("Customizations/GeoRestrictions").is_some(),
                start.other_customizations,
                "case {case}: expected the tenant's other customizations to survive the \
                 association (present={}), got {}",
                start.other_customizations,
                read.raw
            );

            // Disassociating removes that one member and nothing else.
            let cleared = disassociate(&router, &id, Some(&associated_etag)).await;
            assert_eq!(
                cleared.status, 200,
                "case {case}: expected HTTP 200 from DisassociateDistributionTenantWebACL, got {} \
                 with {}",
                cleared.status, cleared.raw
            );
            assert_eq!(
                cleared.root().text_at("Id"),
                Some(id.as_str()),
                "case {case}: expected the answer to name tenant {id:?}, got {:?}",
                cleared.root().text_at("Id")
            );
            let cleared_etag = cleared.header("etag").to_string();
            assert!(
                is_version_token(&cleared_etag),
                "case {case}: expected a minted version token in the ETag header, got \
                 {cleared_etag:?}"
            );
            assert_ne!(
                cleared_etag, associated_etag,
                "case {case}: expected the disassociation to mint a version token other than \
                 {associated_etag:?}, got {cleared_etag:?}"
            );

            let read = read_tenant(&router, &id, &case).await;
            assert_eq!(
                web_acl_of(read.root()),
                None,
                "case {case}: expected the read to report no web ACL customization, got {:?} in {}",
                web_acl_of(read.root()),
                read.raw
            );
            assert_eq!(
                read.root().at("Customizations/GeoRestrictions").is_some(),
                start.other_customizations,
                "case {case}: expected the tenant's other customizations to survive the \
                 disassociation (present={}), got {}",
                start.other_customizations,
                read.raw
            );
            // A customization set the disassociation empties is reported not at all, rather
            // than as an element describing nothing.
            assert_eq!(
                read.root().at("Customizations").is_some(),
                start.other_customizations,
                "case {case}: expected Customizations present={} after the disassociation, got {}",
                start.other_customizations,
                read.raw
            );

            // Removing what is not there reaches the same state: the repeat is served.
            let again = disassociate(&router, &id, Some(&cleared_etag)).await;
            assert_eq!(
                again.status, 200,
                "case {case}: expected HTTP 200 from a repeated disassociation, got {} with {}",
                again.status, again.raw
            );
            let read = read_tenant(&router, &id, &case).await;
            assert_eq!(
                web_acl_of(read.root()),
                None,
                "case {case}: expected the repeat to leave no web ACL customization, got {:?}",
                web_acl_of(read.root())
            );
        }
    }
}

/// One row of the precondition table: what the request states as its `If-Match`, and what
/// the service answers.
#[derive(Debug, Clone, Copy)]
struct Precondition {
    case: &'static str,
    stated: Stated,
    expected_status: u16,
    expected_code: &'static str,
}

/// Every kind of `If-Match` a request can state. The tenant's current version is REQUIRED
/// here, so stating nothing is "the `If-Match` version is missing", the same refusal as a
/// value that is no version token at all; a well-formed token that is not the stored one is
/// a failed precondition instead.
const PRECONDITIONS: &[Precondition] = &[
    Precondition {
        case: "no-if-match-at-all",
        stated: Stated::Absent,
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
    Precondition {
        case: "current-version",
        stated: Stated::Current,
        expected_status: 200,
        expected_code: "",
    },
    Precondition {
        case: "quoted-current-version",
        stated: Stated::QuotedCurrent,
        expected_status: 200,
        expected_code: "",
    },
    // A version token's shape is `E` + 13 of the id alphabet, so this row is a well-formed
    // version that is simply not the stored one.
    Precondition {
        case: "stale-version",
        stated: Stated::Literal("E1STALEVERSION"),
        expected_status: 412,
        expected_code: "PreconditionFailed",
    },
    Precondition {
        case: "not-a-version-token",
        stated: Stated::Literal("not-a-version"),
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
    Precondition {
        case: "empty-if-match",
        stated: Stated::Literal(""),
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
    Precondition {
        case: "lowercase-version-token",
        stated: Stated::Literal("e1abcdefghijklm"),
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
    Precondition {
        case: "version-token-one-character-short",
        stated: Stated::Literal("E1ABCDEFGHIJK"),
        expected_status: 400,
        expected_code: "InvalidIfMatchVersion",
    },
];

/// The tenant's version governs both calls, and a refused call leaves the tenant exactly as
/// it was — same web ACL customization, same version.
#[tokio::test]
async fn the_tenants_version_is_a_required_precondition() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-web-acl-if-match").await;
    let web_acl = WEB_ACLS[0].1;

    for (index, row) in PRECONDITIONS.iter().enumerate() {
        for (which, associating) in [("associate", true), ("disassociate", false)] {
            let case = format!("{which}/{}", row.case);
            let slug = format!("p{index}{}", if associating { "a" } else { "d" });
            // The disassociation rows start from a tenant that HAS a web ACL, so a refused
            // call is visible as the customization it did not remove.
            let start = if associating { STARTS[0] } else { STARTS[2] };
            let (id, etag) = create_tenant(&router, &distribution_id, &slug, &case, start).await;

            let stated: Option<String> = row.stated.header(&etag);
            let answer = if associating {
                associate(&router, &id, web_acl, stated.as_deref()).await
            } else {
                disassociate(&router, &id, stated.as_deref()).await
            };
            assert_eq!(
                answer.status, row.expected_status,
                "case {case}: expected HTTP {} from If-Match {stated:?}, got {} with {}",
                row.expected_status, answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                row.expected_code,
                "case {case}: expected error code {:?}, got {:?}",
                row.expected_code,
                answer.error_code()
            );
            if row.expected_code == "InvalidIfMatchVersion" {
                let message = answer.root().find_text("Message").unwrap_or_default();
                assert_eq!(
                    message, INVALID_IF_MATCH_VERSION_MESSAGE,
                    "case {case}: expected the missing-or-invalid If-Match sentence, got \
                     {message:?}"
                );
            }

            let read = read_tenant(&router, &id, &case).await;
            let reported = web_acl_of(read.root());
            if row.expected_status == 200 {
                let expected = associating.then(|| {
                    (
                        CUSTOMIZATION_ACTION_OVERRIDE.to_string(),
                        web_acl.to_string(),
                    )
                });
                assert_eq!(
                    reported, expected,
                    "case {case}: expected the served call to leave web ACL customization \
                     {expected:?}, got {reported:?}"
                );
                assert_ne!(
                    read.header("etag"),
                    etag,
                    "case {case}: expected the served call to mint a new version, got {:?}",
                    read.header("etag")
                );
            } else {
                let expected = start
                    .web_acl_before
                    .map(|(action, arn)| (action.to_string(), arn.to_string()));
                assert_eq!(
                    reported, expected,
                    "case {case}: expected a refused call to leave web ACL customization \
                     {expected:?}, got {reported:?}"
                );
                assert_eq!(
                    read.header("etag"),
                    etag,
                    "case {case}: expected a refused call to leave the version at {etag:?}, got \
                     {:?}",
                    read.header("etag")
                );
            }
        }
    }
}

/// A tenant id in the shape a tenant id takes but holding no tenant is `EntityNotFound` on
/// both calls, while an id outside that shape was never a tenant id at all and is
/// `InvalidArgument`. The `If-Match` stated is a well-formed version token, so only the id
/// can decide the answer.
#[tokio::test]
async fn an_unheld_tenant_is_not_found() {
    let router = emulator();
    let web_acl = WEB_ACLS[0].1;

    for (case, id, status, code) in [
        (
            "never-minted-tenant-shape",
            "dt_1A2B3C4D5E6F7",
            404,
            "EntityNotFound",
        ),
        ("not-an-id-at-all", "nope", 400, "InvalidArgument"),
        (
            "a-distribution-id",
            "EDFDVBD6EXAMPLE",
            400,
            "InvalidArgument",
        ),
    ] {
        for (which, answer) in [
            (
                "associate",
                associate(&router, id, web_acl, Some("E1STALEVERSION")).await,
            ),
            (
                "disassociate",
                disassociate(&router, id, Some("E1STALEVERSION")).await,
            ),
        ] {
            assert_eq!(
                answer.status, status,
                "case {case}/{which}: expected HTTP {status} for tenant {id:?}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case}/{which}: expected {code} for tenant {id:?}, got {:?}",
                answer.error_code()
            );
        }
    }
}

/// A request that names NO web ACL is refused with `InvalidArgument`, and nothing is stored
/// or minted for it: an association of nothing is not an association.
#[tokio::test]
async fn an_association_naming_no_web_acl_is_refused() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-web-acl-blank").await;

    // An empty element, and one carrying only whitespace — the two ways a document can
    // carry the member without naming a web ACL.
    for (index, (case, value)) in [("empty-element", ""), ("whitespace-only", "   ")]
        .into_iter()
        .enumerate()
    {
        let slug = format!("b{index}x");
        let (id, etag) = create_tenant(&router, &distribution_id, &slug, case, STARTS[0]).await;

        let answer = associate(&router, &id, value, Some(&etag)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for WebACLArn {value:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for WebACLArn {value:?}, got {:?}",
            answer.error_code()
        );

        let read = read_tenant(&router, &id, case).await;
        assert_eq!(
            web_acl_of(read.root()),
            None,
            "case {case}: expected a refused association to store no web ACL, got {:?}",
            web_acl_of(read.root())
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected a refused association to leave the version at {etag:?}, got \
             {:?}",
            read.header("etag")
        );
    }
}

/// The propagation each call owes the tenant is the CLOCK's: the tenant is `InProgress`
/// straight after the write and `Deployed` once the span has passed, without any call making
/// it so, and the customization the write landed survives the wait.
#[tokio::test]
async fn the_clock_settles_the_write() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-web-acl-settle").await;
    let case = "settle";
    let (id, etag) = create_tenant(&router, &distribution_id, "s0x", case, STARTS[0]).await;
    let web_acl = WEB_ACLS[0].1;

    // Start from a deployed tenant, so the status the association reports is the one it
    // caused rather than the create's.
    tokio::time::sleep(past_propagation()).await;
    let read = read_tenant(&router, &id, case).await;
    assert_eq!(
        read.root().text_at("Status"),
        Some(STATUS_DEPLOYED),
        "case {case}: expected the created tenant to settle to {STATUS_DEPLOYED}, got {:?}",
        read.root().text_at("Status")
    );

    let associated = associate(&router, &id, web_acl, Some(&etag)).await;
    assert_eq!(
        associated.status, 200,
        "case {case}: expected HTTP 200 from AssociateDistributionTenantWebACL, got {} with {}",
        associated.status, associated.raw
    );
    let read = read_tenant(&router, &id, case).await;
    assert_eq!(
        read.root().text_at("Status"),
        Some(STATUS_IN_PROGRESS),
        "case {case}: expected {STATUS_IN_PROGRESS} straight after the association, got {:?}",
        read.root().text_at("Status")
    );

    tokio::time::sleep(past_propagation()).await;

    let read = read_tenant(&router, &id, case).await;
    assert_eq!(
        read.root().text_at("Status"),
        Some(STATUS_DEPLOYED),
        "case {case}: expected {STATUS_DEPLOYED} once the propagation was owed, got {:?}",
        read.root().text_at("Status")
    );
    assert_eq!(
        web_acl_of(read.root()),
        Some((
            CUSTOMIZATION_ACTION_OVERRIDE.to_string(),
            web_acl.to_string()
        )),
        "case {case}: expected the association to survive the propagation, got {:?}",
        web_acl_of(read.root())
    );
}
