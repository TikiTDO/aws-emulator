// SPDX-License-Identifier: CC0-1.0

//! Associating a WAF web ACL with a distribution, end to end over HTTP.
//!
//! The rule under test: associating a web ACL is a CONFIGURATION change to one stored
//! distribution — the identifier the request names becomes that distribution's `WebACLId`
//! ("a unique identifier that specifies the WAF web ACL, if any, to associate with this
//! distribution. To specify a web ACL created using the latest version of WAF, use the ACL
//! ARN"), so every later read answers with it; the call mints a fresh version token and
//! sends the distribution back to `InProgress` until the clock owes it `Deployed`, exactly
//! as any other write of its configuration does. Around that rule:
//!
//! * `If-Match` is OPTIONAL on this operation, so a request stating no precondition lands;
//!   one stating the current version lands; one stating a version that is no longer current
//!   fails the precondition (HTTP 412); and one stating a value that is no version token at
//!   all is refused as an invalid `If-Match` version (HTTP 400). A refused call changes
//!   nothing a later read can see.
//! * a distribution the store does not hold is `EntityNotFound` (HTTP 404) — the error this
//!   operation models for an entity it cannot find.
//! * a request that names NO web ACL is `InvalidArgument` (HTTP 400): an association of
//!   nothing is not an association. WHICH web ACL of CloudFront's own namespace was named is
//!   not resolved here — a web ACL is a WAF resource this service does not hold — while the
//!   NAMESPACE a `wafv2` ARN names is decided, and the forms that decision refuses are
//!   [`crate::web_acl_scope_admission`]'s table.
//!
//! The wire harness (the router, the request helpers and the restXml element scanner) is
//! the one [`crate::distribution_lifecycle`] builds, so these assertions travel the same
//! path a client takes.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    STATUS_DEPLOYED, STATUS_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, config_table, create_distribution, emulator, get_distribution, is_version_token,
    past_propagation, Answer, API_VERSION,
};

/// Web ACL identifiers this suite associates, spanning the forms the member takes that an
/// association ADMITS: the WAF generation (`wafv2`, and the `waf`/`waf-regional` services of
/// WAF Classic), the partition, the account and region segments, and the bare ACL id WAF
/// Classic is named by.
///
/// Every `wafv2` ARN here names the GLOBAL namespace, because that is the only one a
/// distribution may be put behind: the `CLOUDFRONT` scope is the one "for a global resource
/// type, such as a Amazon CloudFront distribution". The forms an association refuses — a
/// `wafv2` ARN of the regional namespace, and one naming another resource of either — are
/// the table [`crate::web_acl_scope_admission`] drives both association operations from.
///
/// The forms come from the model's own documentation of `DistributionConfig$WebACLId` —
/// "use the ACL ARN, for example
/// `arn:aws:wafv2:us-east-1:123456789012:global/webacl/ExampleWebACL/a1b2c3d4-5678-90ab-cdef-EXAMPLE11111`
/// … To specify a web ACL created using WAF Classic, use the ACL ID, for example
/// `a1b2c3d4-5678-90ab-cdef-EXAMPLE11111`".
///
/// `pub(crate)` so the disassociation suite takes a web ACL off each of these same forms.
pub(crate) const WEB_ACLS: &[(&str, &str)] = &[
    (
        "wafv2-global",
        "arn:aws:wafv2:us-east-1:123456789012:global/webacl/ExampleWebACL/\
         a1b2c3d4-5678-90ab-cdef-EXAMPLE11111",
    ),
    (
        "wafv2-global-other-account",
        "arn:aws:wafv2:us-east-1:210987654321:global/webacl/OtherWebACL/\
         99998888-7777-6666-5555-EXAMPLE44444",
    ),
    (
        "wafv2-other-partition",
        "arn:aws-us-gov:wafv2:us-gov-west-1:123456789012:global/webacl/GovWebACL/\
         11112222-3333-4444-5555-EXAMPLE66666",
    ),
    (
        "waf-classic-arn",
        "arn:aws:waf::123456789012:webacl/473e64fd-f30b-4765-81a0-62ad96dd167a",
    ),
    (
        "waf-classic-id",
        "473e64fd-f30b-4765-81a0-62ad96dd167a",
    ),
];

/// A `DistributionConfig` under `caller_reference` naming `connection_mode`, which is what
/// decides whether the distribution is multi-tenant: the mode "specifies whether the
/// connection mode is through a standard distribution (direct) or a multi-tenant
/// distribution with distribution tenants (tenant-only)".
fn config_xml(caller_reference: &str, connection_mode: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment><Enabled>true</Enabled>\
         <ConnectionMode>{connection_mode}</ConnectionMode>\
         </DistributionConfig>"
    )
}

/// One `AssociateDistributionWebACL` call: the web ACL travels in the request document,
/// the version (when the row states one) in the `If-Match` header.
///
/// `pub(crate)` so the disassociation suite can reach an association the way a client
/// does rather than only through a create's own configuration.
pub(crate) async fn associate_web_acl(
    router: &MultiServiceRouter,
    id: &str,
    web_acl_arn: &str,
    if_match: Option<&str>,
) -> Answer {
    let body = format!(
        "<AssociateDistributionWebACLRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <WebACLArn>{web_acl_arn}</WebACLArn></AssociateDistributionWebACLRequest>"
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/associate-web-acl"),
        Some(&body),
        if_match,
    )
    .await
}

/// Create one distribution from `body` and report `(Id, ETag)`.
pub(crate) async fn create(
    router: &MultiServiceRouter,
    case: &str,
    body: &str,
) -> (String, String) {
    let created = create_distribution(router, body).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
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

/// What a read of the distribution reports: `(WebACLId, ETag, Status)`.
pub(crate) async fn read_back(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
) -> (String, String, String) {
    let fetched = get_distribution(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {case}: expected HTTP 200 from GetDistribution, got {} with {}",
        fetched.status, fetched.raw
    );
    (
        fetched
            .root()
            .text_at("DistributionConfig/WebACLId")
            .unwrap_or_default()
            .to_string(),
        fetched.header("etag").to_string(),
        fetched
            .root()
            .text_at("Status")
            .unwrap_or_default()
            .to_string(),
    )
}

/// Associating a web ACL stores it on the distribution: the call reports the distribution
/// it changed, the web ACL it associated and a FRESH version token, every later read
/// answers with that web ACL at that version, and the distribution is propagating its new
/// configuration. Associating a second web ACL replaces the first.
#[tokio::test]
async fn associating_a_web_acl_stores_it_on_the_distribution() {
    let router = emulator();

    for (index, (acl_case, web_acl)) in WEB_ACLS.iter().enumerate() {
        for row in config_table() {
            let case = format!("{acl_case}/{}", row.case);
            let reference = format!("ref-assoc-{acl_case}-{}", row.case);
            let body = row.xml().replace(&row.caller_reference, &reference);
            let (id, created_etag) = create(&router, &case, &body).await;

            // Before the association the distribution names no web ACL: an absent
            // `WebACLId` resolves to the empty string, which is what "no web ACL" is.
            let (before, _, _) = read_back(&router, &id, &case).await;
            assert_eq!(
                before, "",
                "case {case}: expected a fresh distribution to name no web ACL, got {before:?}"
            );

            let associated = associate_web_acl(&router, &id, web_acl, Some(&created_etag)).await;
            assert_eq!(
                associated.status, 200,
                "case {case}: expected HTTP 200 from AssociateDistributionWebACL, got {} with {}",
                associated.status, associated.raw
            );
            assert_eq!(
                associated.root().text_at("Id"),
                Some(id.as_str()),
                "case {case}: expected the answer to name distribution {id:?}, got {:?}",
                associated.root().text_at("Id")
            );
            assert_eq!(
                associated.root().text_at("WebACLArn"),
                Some(*web_acl),
                "case {case}: expected the answer to name web ACL {web_acl:?}, got {:?}",
                associated.root().text_at("WebACLArn")
            );
            let etag = associated.header("etag").to_string();
            assert!(
                is_version_token(&etag),
                "case {case}: expected a minted version token in the ETag header, got {etag:?}"
            );
            assert_ne!(
                etag, created_etag,
                "case {case}: expected the association to mint a version token other than \
                 {created_etag:?}, got {etag:?}"
            );

            // The one record answers every read: the web ACL, the version the association
            // minted, and the status its new configuration is owed.
            let (stored, read_etag, status) = read_back(&router, &id, &case).await;
            assert_eq!(
                stored, *web_acl,
                "case {case}: expected GetDistribution to report WebACLId {web_acl:?}, got \
                 {stored:?}"
            );
            assert_eq!(
                read_etag, etag,
                "case {case}: expected GetDistribution at version {etag:?}, got {read_etag:?}"
            );
            assert_eq!(
                status, STATUS_IN_PROGRESS,
                "case {case}: expected the reconfigured distribution to report \
                 {STATUS_IN_PROGRESS}, got {status:?}"
            );

            // A second association replaces the first — the member holds ONE web ACL — so
            // each row replaces with the NEXT row's web ACL, never with its own.
            let replacement = WEB_ACLS[(index + 1) % WEB_ACLS.len()].1;
            let replaced = associate_web_acl(&router, &id, replacement, Some(&etag)).await;
            assert_eq!(
                replaced.status, 200,
                "case {case}: expected HTTP 200 from the replacing association, got {} with {}",
                replaced.status, replaced.raw
            );
            let (stored, _, _) = read_back(&router, &id, &case).await;
            assert_eq!(
                stored, replacement,
                "case {case}: expected the replacing association to store {replacement:?}, got \
                 {stored:?}"
            );
        }
    }
}

/// The propagation the association owes is the CLOCK's: a caller that waits sees the
/// distribution deployed again, without any call making it so.
#[tokio::test]
async fn the_clock_settles_the_association() {
    let router = emulator();
    let row = &config_table()[0];
    let (id, etag) = create(&router, "settle", &row.xml()).await;

    let associated = associate_web_acl(&router, &id, WEB_ACLS[0].1, Some(&etag)).await;
    assert_eq!(
        associated.status, 200,
        "case settle: expected HTTP 200 from AssociateDistributionWebACL, got {} with {}",
        associated.status, associated.raw
    );
    let (_, _, status) = read_back(&router, &id, "settle").await;
    assert_eq!(
        status, STATUS_IN_PROGRESS,
        "case settle: expected {STATUS_IN_PROGRESS} straight after the association, got \
         {status:?}"
    );

    tokio::time::sleep(past_propagation()).await;

    let (stored, _, status) = read_back(&router, &id, "settle").await;
    assert_eq!(
        status, STATUS_DEPLOYED,
        "case settle: expected {STATUS_DEPLOYED} once the propagation was owed, got {status:?}"
    );
    assert_eq!(
        stored,
        WEB_ACLS[0].1,
        "case settle: expected the association to survive the propagation, got {stored:?}"
    );
}

/// What a request states as its `If-Match`.
///
/// `pub(crate)` so the disassociation suite states its preconditions in the same terms.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Stated {
    /// No header at all.
    Absent,
    /// The distribution's current version, bare — the form CloudFront's own `ETag`
    /// response header reports.
    Current,
    /// The distribution's current version, quoted, as HTTP entity tags normally are.
    QuotedCurrent,
    /// Whatever the row names, verbatim.
    Literal(&'static str),
}

impl Stated {
    /// The `If-Match` header value this form sends against the version `current`, or
    /// `None` where the form states no header at all.
    ///
    /// The ONE place the form-to-header mapping lives, so the association and
    /// disassociation suites state their preconditions identically.
    pub(crate) fn header(self, current: &str) -> Option<String> {
        match self {
            Stated::Absent => None,
            Stated::Current => Some(current.to_string()),
            Stated::QuotedCurrent => Some(format!("\"{current}\"")),
            Stated::Literal(value) => Some(value.to_string()),
        }
    }
}

/// One row of the precondition table: what the request states as its `If-Match`, and what
/// the service answers.
#[derive(Debug, Clone, Copy)]
struct Precondition {
    case: &'static str,
    stated: Stated,
    /// The row's expected status and wire error code (`""` when it is served).
    expected_status: u16,
    expected_code: &'static str,
}

/// Every kind of `If-Match` a request can state. The header is OPTIONAL on this
/// operation, so stating nothing states no precondition; a version token that is not
/// current fails the precondition; and a value that is no version token at all is an
/// invalid `If-Match` version — "the `If-Match` version is missing or not valid".
const PRECONDITIONS: &[Precondition] = &[
    Precondition {
        case: "no-if-match-at-all",
        stated: Stated::Absent,
        expected_status: 200,
        expected_code: "",
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
    // A version token's shape is `E` + 13 of the id alphabet, so this row is a
    // well-formed version that is simply not the stored one.
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

/// `If-Match` governs the association: it lands at the current version and where no
/// precondition is stated, and a refused call leaves the distribution exactly as it was —
/// same web ACL, same version.
#[tokio::test]
async fn if_match_governs_the_association() {
    let router = emulator();

    for row in PRECONDITIONS {
        let case = row.case;
        let reference = format!("ref-if-match-{case}");
        let (id, etag) = create(&router, case, &config_xml(&reference, "direct")).await;

        let stated: Option<String> = row.stated.header(&etag);
        let web_acl = WEB_ACLS[0].1;
        let answer = associate_web_acl(&router, &id, web_acl, stated.as_deref()).await;
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

        let (stored, read_etag, _) = read_back(&router, &id, case).await;
        if row.expected_status == 200 {
            assert_eq!(
                stored, web_acl,
                "case {case}: expected the association to store {web_acl:?}, got {stored:?}"
            );
            assert_ne!(
                read_etag, etag,
                "case {case}: expected the association to mint a new version, got {read_etag:?}"
            );
        } else {
            assert_eq!(
                stored, "",
                "case {case}: expected a refused association to store no web ACL, got {stored:?}"
            );
            assert_eq!(
                read_etag, etag,
                "case {case}: expected a refused association to leave the version at {etag:?}, \
                 got {read_etag:?}"
            );
        }
    }
}

/// A request that names NO web ACL is refused with `InvalidArgument`, and nothing is
/// stored or minted for it: an association of nothing is not an association.
#[tokio::test]
async fn a_request_naming_no_web_acl_is_refused() {
    let router = emulator();
    let row = &config_table()[0];
    let (id, etag) = create(&router, "no-web-acl", &row.xml()).await;

    // An empty element, and one carrying only whitespace — the two ways a document can
    // carry the member without naming a web ACL.
    for (case, value) in [("empty-element", ""), ("whitespace-only", "   ")] {
        let answer = associate_web_acl(&router, &id, value, Some(&etag)).await;
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
        let (stored, read_etag, _) = read_back(&router, &id, case).await;
        assert_eq!(
            stored, "",
            "case {case}: expected a refused association to store no web ACL, got {stored:?}"
        );
        assert_eq!(
            read_etag, etag,
            "case {case}: expected a refused association to leave the version at {etag:?}, got \
             {read_etag:?}"
        );
    }
}

/// A distribution the store does not hold is `EntityNotFound`, whatever the id looks
/// like: one shaped like a distribution id the service never minted, and one of another
/// shape.
#[tokio::test]
async fn an_unheld_distribution_is_not_found() {
    let router = emulator();
    let web_acl = WEB_ACLS[0].1;

    for (case, id) in [
        ("never-minted", "EDFDVBD6EXAMPLE"),
        ("not-an-id-at-all", "nope"),
        ("minted-shape-unknown", "E1111111111111"),
    ] {
        let answer = associate_web_acl(&router, id, web_acl, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for distribution {id:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound for distribution {id:?}, got {:?}",
            answer.error_code()
        );
    }
}

/// The association is a configuration change like any other, so it lands on a multi-tenant
/// distribution exactly as it does on a standard one — a multi-tenant distribution has no
/// configuration of its own that the web ACL member depends on.
#[tokio::test]
async fn a_multi_tenant_distribution_takes_a_web_acl_too() {
    let router = emulator();

    for (connection_mode, case) in [("tenant-only", "multi-tenant"), ("direct", "standard")] {
        let reference = format!("ref-mode-{case}");
        let (id, etag) = create(&router, case, &config_xml(&reference, connection_mode)).await;
        let web_acl = WEB_ACLS[0].1;

        let answer = associate_web_acl(&router, &id, web_acl, Some(&etag)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 associating {web_acl:?} with a {connection_mode} \
             distribution, got {} with {}",
            answer.status, answer.raw
        );
        let (stored, _, _) = read_back(&router, &id, case).await;
        assert_eq!(
            stored, web_acl,
            "case {case}: expected the {connection_mode} distribution to name web ACL \
             {web_acl:?}, got {stored:?}"
        );
    }
}
