// SPDX-License-Identifier: CC0-1.0

//! What ONE `AssociateDistributionTenantWebACL` call writes, and how far it reaches, end to
//! end over HTTP.
//!
//! The rule under test: the association IS the `Customizations$WebAcl` member of the ONE
//! distribution tenant that `Id` resolves — "the WAF web ACL" the tenant overrides — so the
//! member holds exactly ONE web ACL at a time: each call overwrites it in `override` form
//! with the ARN the request named, versions that tenant, and leaves every other record the
//! account holds exactly as it was (a sibling tenant of the same multi-tenant distribution
//! keeps its own customization at its own version, and the parent distribution keeps its own
//! `WebACLId` at its own version). Because the member holds one web ACL, the version a call
//! mints is the only one a following call satisfies: the superseded token fails the
//! precondition and changes nothing.
//!
//! The rows differ along what the rule generalises over: the FORM of the web ACL named
//! (every entry of [`crate::web_acl_association::WEB_ACLS`] — the WAF generation, the
//! account, the partition and the bare WAF Classic id), the LENGTH of the chain of
//! associations a tenant is put through (each form in turn on one tenant, then the last one
//! repeated), what the tenant already carries (every entry of
//! [`crate::tenant_web_acl_association::STARTS`]), and WHICH tenant of a set the call names.
//!
//! The wire harness, the tenant creation helper and the readers are the ones
//! [`crate::distribution_lifecycle`], [`crate::distribution_tenant_lifecycle`],
//! [`crate::tenant_web_acl_association`] and [`crate::web_acl_association`] already build, so
//! these assertions travel the same path a client takes.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::CUSTOMIZATION_ACTION_OVERRIDE;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{emulator, is_version_token, Answer};
use crate::distribution_tenant_lifecycle::multi_tenant_distribution;
use crate::tenant_web_acl_association::{
    associate, create_tenant, read_tenant, web_acl_of, STARTS,
};
use crate::web_acl_association::{associate_web_acl, read_back, WEB_ACLS};

/// How many `WebAcl` elements the tenant's `Customizations` carries, and how many `Arn`
/// elements that one member carries: the member holds ONE web ACL, so a read reports it once
/// rather than accumulating what each call named.
fn web_acl_elements(answer: &Answer) -> (usize, usize) {
    let Some(customizations) = answer.root().at("Customizations") else {
        return (0, 0);
    };
    let web_acls = customizations.children_named("WebAcl");
    let arns = web_acls
        .iter()
        .map(|web_acl| web_acl.children_named("Arn").len())
        .sum();
    (web_acls.len(), arns)
}

/// Associate `web_acl` with `id` at `if_match`, assert it is served, and report the version
/// the answer minted.
async fn associate_ok(
    router: &MultiServiceRouter,
    id: &str,
    web_acl: &str,
    if_match: &str,
    case: &str,
) -> String {
    let answer = associate(router, id, web_acl, Some(if_match)).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from AssociateDistributionTenantWebACL, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.root().text_at("WebACLArn"),
        Some(web_acl),
        "case {case}: expected the answer to name web ACL {web_acl:?}, got {:?}",
        answer.root().text_at("WebACLArn")
    );
    let minted = answer.header("etag").to_string();
    assert!(
        is_version_token(&minted),
        "case {case}: expected a minted version token in the ETag header, got {minted:?}"
    );
    minted
}

/// Each association overwrites the tenant's one web ACL member: walking a tenant through
/// every web ACL form in turn, and then through the last form a second time, the read always
/// reports exactly one `override` customization naming the web ACL the LAST served call
/// named, the version it minted, and the tenant's other customizations.
#[tokio::test]
async fn each_association_replaces_the_tenants_one_web_acl() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-web-acl-replace").await;

    for (start_index, start) in STARTS.iter().enumerate() {
        let slug = format!("r{start_index}x");
        let (id, created_etag) =
            create_tenant(&router, &distribution_id, &slug, start.case, *start).await;
        let mut etag = created_etag;

        // Every form in turn on the SAME tenant, then the last form once more: a chain of
        // associations, each of which has to leave one web ACL behind rather than a second
        // one beside it.
        let chain: Vec<&str> = WEB_ACLS
            .iter()
            .map(|(_, arn)| *arn)
            .chain(std::iter::once(WEB_ACLS[WEB_ACLS.len() - 1].1))
            .collect();

        for (step, web_acl) in chain.iter().enumerate() {
            let case = format!("{}/step-{step}", start.case);
            let superseded = etag.clone();
            let minted = associate_ok(&router, &id, web_acl, &superseded, &case).await;

            let read = read_tenant(&router, &id, &case).await;
            let reported = web_acl_of(read.root());
            let expected = Some((
                CUSTOMIZATION_ACTION_OVERRIDE.to_string(),
                (*web_acl).to_string(),
            ));
            assert_eq!(
                reported, expected,
                "case {case}: expected the read to report web ACL customization {expected:?}, \
                 got {reported:?} in {}",
                read.raw
            );
            let counted = web_acl_elements(&read);
            assert_eq!(
                counted,
                (1, 1),
                "case {case}: expected exactly one WebAcl member carrying one Arn, got \
                 {counted:?} in {}",
                read.raw
            );
            assert_eq!(
                read.header("etag"),
                minted,
                "case {case}: expected the read at version {minted:?}, got {:?}",
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

            // The version the call superseded no longer satisfies the precondition, and the
            // refused call leaves the member and the version exactly as the served one left
            // them.
            let stale = associate(&router, &id, WEB_ACLS[0].1, Some(&superseded)).await;
            assert_eq!(
                stale.status, 412,
                "case {case}: expected HTTP 412 from the superseded version {superseded:?}, got \
                 {} with {}",
                stale.status, stale.raw
            );
            assert_eq!(
                stale.error_code(),
                "PreconditionFailed",
                "case {case}: expected PreconditionFailed from the superseded version, got {:?}",
                stale.error_code()
            );
            let after = read_tenant(&router, &id, &case).await;
            assert_eq!(
                web_acl_of(after.root()),
                expected,
                "case {case}: expected the refused association to leave {expected:?}, got {:?}",
                web_acl_of(after.root())
            );
            assert_eq!(
                after.header("etag"),
                minted,
                "case {case}: expected the refused association to leave the version at \
                 {minted:?}, got {:?}",
                after.header("etag")
            );

            etag = minted;
        }
    }
}

/// One bystander tenant as a read reports it before the call under test: the state it was
/// created in, its id, the version it stands at, and the `(Action, Arn)` of its web ACL
/// customization (`None` when it carries none).
#[derive(Debug, Clone)]
struct Snapshot {
    case: &'static str,
    id: String,
    etag: String,
    web_acl: Option<(String, String)>,
}

/// Associating a web ACL with ONE tenant reaches that tenant only: it carries the
/// customization at the version the answer reported, while every sibling tenant and the
/// parent distribution are untouched — same web ACL, same version.
#[tokio::test]
async fn an_association_reaches_only_the_named_tenant() {
    // One row per web ACL form the association names, each naming a different position of
    // the tenant set, so both the forms and the positions are spanned.
    for (index, (acl_case, web_acl)) in WEB_ACLS.iter().enumerate() {
        let target = index % STARTS.len();
        let case = format!("{acl_case}/{}", STARTS[target].case);
        let case = case.as_str();
        // A fresh emulator per row, so what the row observes is what the row caused.
        let router = emulator();
        let distribution_id =
            multi_tenant_distribution(&router, &format!("ref-assoc-scope-{index}")).await;

        // The parent distribution names a web ACL of its own — the one a tenant's
        // customization overrides, and which the tenant's association must leave in place.
        // The forms are a ring, so the distribution's own ACL is not the one associated.
        let distribution_web_acl = WEB_ACLS[(index + 1) % WEB_ACLS.len()].1;
        let associated =
            associate_web_acl(&router, &distribution_id, distribution_web_acl, None).await;
        assert_eq!(
            associated.status, 200,
            "case {case}: expected HTTP 200 from AssociateDistributionWebACL, got {} with {}",
            associated.status, associated.raw
        );
        let (distribution_acl_before, distribution_etag_before, _) =
            read_back(&router, &distribution_id, case).await;
        assert_eq!(
            distribution_acl_before, distribution_web_acl,
            "case {case}: expected the distribution to name web ACL {distribution_web_acl:?} \
             before the association, got {distribution_acl_before:?}"
        );

        // One tenant per starting state, so whichever position the call names, the bystanders
        // span the whole set of states a tenant can be in.
        let mut tenants: Vec<Snapshot> = Vec::with_capacity(STARTS.len());
        for (position, row) in STARTS.iter().enumerate() {
            let slug = format!("as{index}{position}");
            let creating = format!("{case}/creating-{}", row.case);
            let (id, _) = create_tenant(&router, &distribution_id, &slug, &creating, *row).await;
            let read = read_tenant(&router, &id, &creating).await;
            tenants.push(Snapshot {
                case: row.case,
                id,
                etag: read.header("etag").to_string(),
                web_acl: web_acl_of(read.root()),
            });
        }

        // The call under test: the tenant at `target`, at the version it stands at.
        let named = tenants[target].clone();
        let minted = associate_ok(&router, &named.id, web_acl, &named.etag, case).await;

        let read = read_tenant(&router, &named.id, case).await;
        let reported = web_acl_of(read.root());
        let expected = Some((
            CUSTOMIZATION_ACTION_OVERRIDE.to_string(),
            (*web_acl).to_string(),
        ));
        assert_eq!(
            reported, expected,
            "case {case}: expected the named tenant to report {expected:?}, got {reported:?} in {}",
            read.raw
        );
        assert_eq!(
            read.header("etag"),
            minted,
            "case {case}: expected the named tenant to read at version {minted:?}, got {:?}",
            read.header("etag")
        );

        // Every bystander tenant is exactly as it was: same customization, same version.
        for (position, sibling) in tenants.iter().enumerate() {
            if position == target {
                continue;
            }
            let read = read_tenant(&router, &sibling.id, case).await;
            assert_eq!(
                web_acl_of(read.root()),
                sibling.web_acl,
                "case {case}: expected sibling tenant {position} ({}) to keep web ACL \
                 customization {:?}, got {:?}",
                sibling.case,
                sibling.web_acl,
                web_acl_of(read.root())
            );
            assert_eq!(
                read.header("etag"),
                sibling.etag,
                "case {case}: expected sibling tenant {position} ({}) to stay at version {:?}, \
                 got {:?}",
                sibling.case,
                sibling.etag,
                read.header("etag")
            );
        }

        // The parent distribution's own web ACL is another record's member, so the tenant's
        // association says nothing about it.
        let (distribution_acl_after, distribution_etag_after, _) =
            read_back(&router, &distribution_id, case).await;
        assert_eq!(
            distribution_acl_after, distribution_web_acl,
            "case {case}: expected the distribution to keep web ACL {distribution_web_acl:?}, \
             got {distribution_acl_after:?}"
        );
        assert_eq!(
            distribution_etag_after, distribution_etag_before,
            "case {case}: expected the distribution to stay at version \
             {distribution_etag_before:?}, got {distribution_etag_after:?}"
        );
    }
}
