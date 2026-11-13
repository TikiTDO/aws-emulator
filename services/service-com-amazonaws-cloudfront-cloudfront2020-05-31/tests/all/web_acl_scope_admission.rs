// SPDX-License-Identifier: CC0-1.0

//! WHICH web ACL a distribution — or a distribution tenant — may be put behind, end to end
//! over HTTP.
//!
//! The rule under test: CloudFront is protected by the web ACLs of ONE WAFv2 namespace. The
//! `CLOUDFRONT` scope is the one "for a global resource type, such as a Amazon CloudFront
//! distribution", and a WAFv2 ARN carries the namespace its resource lives in as the first
//! segment of its resource part
//! (`arn:<partition>:wafv2:<region>:<account>:<global|regional>/<kind>/<name>/<id>`) — so an
//! association naming a web ACL of the REGIONAL namespace, or another WAFv2 resource of
//! either namespace, names nothing WAF would enforce in front of a distribution: it is
//! refused `InvalidArgument` (HTTP 400) with CloudFront's own sentence, and NOTHING is
//! mutated — the distribution still names the web ACL it had, at the version it had.
//!
//! Both operations that put a resource of this service behind a web ACL are held to it:
//! `AssociateDistributionWebACL` (the distribution's own `WebACLId`) and
//! `AssociateDistributionTenantWebACL` (a tenant's `Customizations$WebAcl`), which model the
//! same `InvalidArgument`.
//!
//! The rows differ along what the rule generalises over: the NAMESPACE segment the ARN
//! carries, the RESOURCE KIND it names, the region and account segments, and the partition —
//! including a refused ARN and an admitted one that differ in the namespace segment alone, so
//! the namespace is the only thing deciding. The admitted forms are
//! [`crate::web_acl_association::WEB_ACLS`], whose entries span the WAF generation and the
//! bare WAF Classic id a `WebACLId` also carries.
//!
//! The wire harness, the association helpers and the readers are the ones
//! [`crate::distribution_lifecycle`], [`crate::web_acl_association`],
//! [`crate::distribution_tenant_lifecycle`] and [`crate::tenant_web_acl_association`] already
//! build, so these assertions travel the same path a client takes.

use crate::distribution_lifecycle::{config_table, emulator, Answer};
use crate::distribution_tenant_lifecycle::multi_tenant_distribution;
use crate::tenant_web_acl_association::{associate, create_tenant, read_tenant, web_acl_of, STARTS};
use crate::web_acl_association::{associate_web_acl, create, read_back, WEB_ACLS};

/// The sentence CloudFront refuses an association naming a web ACL it may not be put behind
/// with. The whole text is fixed: it states that the web ACL named is not one CloudFront can
/// use and what to name instead, so nothing of the request appears in it.
const INVALID_WEB_ACL: &str = "The specified web ACL isn't valid. Specify a web ACL that was \
     previously validated and try again.";

/// Web ACL identifiers an association REFUSES, spanning what the rule generalises over: the
/// namespace segment (`regional`), the resource kind a `global` ARN can name instead of a web
/// ACL, the region and account segments, and the partition.
///
/// The first row is the regional twin of `WEB_ACLS[0]` — the same account, name and id in the
/// other namespace — so the pair differs in the namespace segment alone.
const UNASSOCIABLE_WEB_ACLS: &[(&str, &str)] = &[
    (
        "the regional twin of an admitted global web ACL",
        "arn:aws:wafv2:us-east-1:123456789012:regional/webacl/ExampleWebACL/\
         a1b2c3d4-5678-90ab-cdef-EXAMPLE11111",
    ),
    (
        "a regional web ACL in another region",
        "arn:aws:wafv2:us-west-2:210987654321:regional/webacl/RegionalWebACL/\
         22223333-4444-5555-6666-EXAMPLE77777",
    ),
    (
        "a regional web ACL in another partition",
        "arn:aws-us-gov:wafv2:us-gov-west-1:123456789012:regional/webacl/GovRegionalACL/\
         33334444-5555-6666-7777-EXAMPLE88888",
    ),
    (
        "an IP set of the global namespace",
        "arn:aws:wafv2:us-east-1:123456789012:global/ipset/ExampleIPSet/\
         44445555-6666-7777-8888-EXAMPLE99999",
    ),
    (
        "a rule group of the global namespace",
        "arn:aws:wafv2:us-east-1:123456789012:global/rulegroup/ExampleRuleGroup/\
         55556666-7777-8888-9999-EXAMPLE00000",
    ),
    (
        "a regex pattern set of the regional namespace",
        "arn:aws:wafv2:eu-west-1:123456789012:regional/regexpatternset/ExampleSet/\
         66667777-8888-9999-0000-EXAMPLE11111",
    ),
];

/// The `InvalidArgument` refusal an association answers, asserted whole: the status, the
/// modelled error code and CloudFront's fixed sentence.
fn assert_refused(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400 for a web ACL outside CloudFront's namespace, got {} \
         with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidArgument",
        "case {case}: expected InvalidArgument, got {:?} with {}",
        answer.error_code(),
        answer.raw
    );
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, INVALID_WEB_ACL,
        "case {case}: expected the message {INVALID_WEB_ACL:?}, got {message:?}"
    );
}

/// A distribution is put behind a web ACL of CloudFront's own namespace and no other: every
/// admitted form lands and reads back, and every form outside that namespace is refused with
/// CloudFront's sentence while the distribution keeps the web ACL and the version it had.
#[tokio::test]
async fn a_distribution_takes_only_a_web_acl_of_cloudfronts_namespace() {
    let router = emulator();
    let row = &config_table()[0];
    let admitted = WEB_ACLS[0].1;

    for (index, (case, web_acl)) in UNASSOCIABLE_WEB_ACLS.iter().enumerate() {
        let reference = format!("ref-scope-dist-{index}");
        let body = row.xml().replace(&row.caller_reference, &reference);
        let (id, created_etag) = create(&router, case, &body).await;

        // The distribution starts behind a web ACL CloudFront DOES admit, so the refusal
        // below has something to leave in place.
        let seeded = associate_web_acl(&router, &id, admitted, Some(&created_etag)).await;
        assert_eq!(
            seeded.status, 200,
            "case {case}: expected HTTP 200 associating the admitted {admitted:?}, got {} \
             with {}",
            seeded.status, seeded.raw
        );
        let seeded_etag = seeded.header("etag").to_string();

        let refused = associate_web_acl(&router, &id, web_acl, Some(&seeded_etag)).await;
        assert_refused(&refused, case);

        // Nothing was mutated and no version was minted: the distribution still names the
        // web ACL it was behind, at the version the admitted association left it at.
        let (stored, read_etag, _) = read_back(&router, &id, case).await;
        assert_eq!(
            stored, admitted,
            "case {case}: expected the refused association to leave {admitted:?} in place, \
             got {stored:?}"
        );
        assert_eq!(
            read_etag, seeded_etag,
            "case {case}: expected the refused association to leave the version at \
             {seeded_etag:?}, got {read_etag:?}"
        );

        // The same call naming a form of CloudFront's own namespace lands on the same
        // distribution, so the namespace the ARN names is what decided the refusal.
        let moved_on = associate_web_acl(&router, &id, WEB_ACLS[1].1, Some(&seeded_etag)).await;
        assert_eq!(
            moved_on.status, 200,
            "case {case}: expected HTTP 200 associating {:?} after the refusal, got {} with {}",
            WEB_ACLS[1].1, moved_on.status, moved_on.raw
        );
        let (stored, _, _) = read_back(&router, &id, case).await;
        assert_eq!(
            stored,
            WEB_ACLS[1].1,
            "case {case}: expected the admitted association to store {:?}, got {stored:?}",
            WEB_ACLS[1].1
        );
    }
}

/// Every form the admitted catalog carries is still associated and read back, so the
/// refusal above narrows the member to CloudFront's namespace rather than to one ARN.
#[tokio::test]
async fn every_admitted_form_still_lands_on_a_distribution() {
    let router = emulator();
    let row = &config_table()[0];

    for (index, (case, web_acl)) in WEB_ACLS.iter().enumerate() {
        let reference = format!("ref-scope-admitted-{index}");
        let body = row.xml().replace(&row.caller_reference, &reference);
        let (id, created_etag) = create(&router, case, &body).await;

        let answer = associate_web_acl(&router, &id, web_acl, Some(&created_etag)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 associating {web_acl:?}, got {} with {}",
            answer.status, answer.raw
        );
        let (stored, _, _) = read_back(&router, &id, case).await;
        assert_eq!(
            stored, *web_acl,
            "case {case}: expected the distribution to name {web_acl:?}, got {stored:?}"
        );
    }
}

/// A TENANT is held to the same namespace as its parent distribution: the same forms are
/// refused with the same sentence, the tenant's customization and version are untouched, and
/// an admitted form lands on the same tenant afterwards.
#[tokio::test]
async fn a_tenant_takes_only_a_web_acl_of_cloudfronts_namespace() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-scope-tenant").await;
    let start = STARTS[0];

    for (index, (case, web_acl)) in UNASSOCIABLE_WEB_ACLS.iter().enumerate() {
        let slug = format!("scope{index}");
        let (id, created_etag) =
            create_tenant(&router, &distribution_id, &slug, case, start).await;

        let refused = associate(&router, &id, web_acl, Some(&created_etag)).await;
        assert_refused(&refused, case);

        // The tenant carries no web ACL customization and still answers at the version its
        // create minted: the refusal wrote nothing.
        let read = read_tenant(&router, &id, case).await;
        assert_eq!(
            web_acl_of(read.root()),
            start
                .web_acl_before
                .map(|(action, arn)| (action.to_string(), arn.to_string())),
            "case {case}: expected the refused association to leave the tenant's web ACL \
             customization as it was, got {:?}",
            web_acl_of(read.root())
        );
        assert_eq!(
            read.header("etag"),
            created_etag,
            "case {case}: expected the refused association to leave the version at \
             {created_etag:?}, got {:?}",
            read.header("etag")
        );

        let admitted = associate(&router, &id, WEB_ACLS[0].1, Some(&created_etag)).await;
        assert_eq!(
            admitted.status, 200,
            "case {case}: expected HTTP 200 associating {:?} after the refusal, got {} with {}",
            WEB_ACLS[0].1, admitted.status, admitted.raw
        );
        let read = read_tenant(&router, &id, case).await;
        assert_eq!(
            web_acl_of(read.root()).map(|(_, arn)| arn),
            Some(WEB_ACLS[0].1.to_string()),
            "case {case}: expected the tenant to carry {:?}, got {:?}",
            WEB_ACLS[0].1,
            web_acl_of(read.root())
        );
    }
}

/// The refusal is decided about the MEMBER, so it reaches a tenant in every starting state:
/// one with no customizations, one whose customizations are not about a web ACL, one whose
/// web ACL is already overridden, and one whose web ACL is disabled.
#[tokio::test]
async fn the_refusal_leaves_every_starting_customization_as_it_was() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-scope-tenant-starts").await;
    let web_acl = UNASSOCIABLE_WEB_ACLS[0].1;

    for (index, start) in STARTS.iter().enumerate() {
        let case = format!("{}/{}", start.case, UNASSOCIABLE_WEB_ACLS[0].0);
        let slug = format!("start{index}");
        let (id, created_etag) =
            create_tenant(&router, &distribution_id, &slug, &case, *start).await;

        let refused = associate(&router, &id, web_acl, Some(&created_etag)).await;
        assert_refused(&refused, &case);

        let read = read_tenant(&router, &id, &case).await;
        assert_eq!(
            web_acl_of(read.root()),
            start
                .web_acl_before
                .map(|(action, arn)| (action.to_string(), arn.to_string())),
            "case {case}: expected the tenant's web ACL customization to be what the create \
             stored, got {:?}",
            web_acl_of(read.root())
        );
        assert_eq!(
            read.header("etag"),
            created_etag,
            "case {case}: expected the version to stay at {created_etag:?}, got {:?}",
            read.header("etag")
        );
    }
}
