// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHEN a domain-scoped listing is denied: the authorization
//! answer does not depend on anything else the request carries.
//!
//! The rule: a request whose members all satisfy the constraints the model declares is
//! decided by the authorization stage before the service applies any check of its own,
//! so a domain-scoped listing whose `domainIdentifier` satisfies `DomainId`'s pattern
//! `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId") but names no domain
//! this store serves answers 403 `AccessDeniedException` with
//! `User is not permitted to perform operation: <OperationName>` — whatever else the
//! request carries. A `nextToken` is the case this suite is built around: the model
//! constrains it only by length (1..8192, smithy_query shape="PaginationToken"), so its
//! content is opaque to the client and only the service knows which tokens it issued;
//! deciding that is the service's own check, which belongs after authorization, not
//! before it.
//!
//! `domain_scoped_read_authorization` and `domain_scoped_write_authorization` assert the
//! denial itself over the reads and the writes; this suite asserts its PLACE in the
//! request walk, over every paginated domain-scoped listing this emulator serves.
//!
//! Each rule is asserted over a TABLE, not one request:
//!
//! * every paginated domain-scoped listing, crossed with domain identifiers differing in
//!   separator, length and content (each naming no stored domain), crossed with what else
//!   the request carries: no other member, a token this service would refuse, a token it
//!   can read, the longest token the model admits, a page size at each published
//!   `maxResults` bound, and a token and a page size together;
//! * the same listings under the domain that holds their resources, which answer 200 —
//!   so authorization refuses the unserved scope and nothing more;
//! * tokens that are not an encoding this service produced, under that served domain,
//!   which ARE refused as invalid input — so the denial above has not swallowed the token
//!   check, it only follows it;
//! * the same listings, token and all, once the served domain has been deleted: the
//!   identifier names no domain any more, so the answer is the denial again.
//!
//! The harness (`Emulator`, `Answer`) is the one every suite in this crate drives the
//! service with.

use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// The one `DataZoneEntityType` member (smithy_query shape="DataZoneEntityType"), which
/// is also the `TargetEntityType` member the policy-grant listing addresses.
const ENTITY_TYPE: &str = "DOMAIN_UNIT";

/// A `ManagedPolicyType` member (smithy_query shape="ManagedPolicyType"), the required
/// query member `ListPolicyGrants` carries.
const POLICY_TYPE: &str = "CREATE_GLOSSARY";

/// How long a test waits for a domain lifecycle step the clock owes, looking again until
/// the condition holds. Deliberately far longer than the span itself, so a loaded host
/// slows the test down instead of failing it.
const LIFECYCLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long to wait between two looks while polling for that step.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Domain identifiers no domain in the store carries, each satisfying `DomainId`'s
/// modelled pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId") so
/// that nothing refuses them before a handler runs. The rows differ along what the
/// pattern leaves open: which separator, the shortest and the longest tail it admits,
/// and a tail mixing the other characters of the class.
fn unserved_domain_ids() -> Vec<(&'static str, String)> {
    vec![
        ("an underscore separator", "dzd_nolistdomain000".to_string()),
        ("a hyphen separator", "dzd-nolistdomain001".to_string()),
        ("the shortest tail the pattern admits", "dzd_l".to_string()),
        (
            "the longest tail the pattern admits",
            format!("dzd_{}", "l".repeat(36)),
        ),
        (
            "a tail mixing digits, hyphens and underscores",
            "dzd_7-absent_2".to_string(),
        ),
    ]
}

/// A `nextToken` in the opaque form this service issues (the hex of `nosuchkey`, a key
/// nothing is stored under), satisfying `PaginationToken`'s modelled length of 1..8192.
/// Used only where the request is denied before any token is read.
const READABLE_TOKEN: &str = "6e6f737563686b6579";

/// A `nextToken` that is not an encoding this service ever produced, so the service
/// refuses it as invalid input. Satisfies the same modelled length, which is the only
/// constraint the model puts on the member.
const UNREADABLE_TOKEN: &str = "notatoken";

/// What else a request carries beside its domain scope: a label, and the query members to
/// append. `maxResults` is filled in per listing, from that listing's own modelled range.
enum Extra {
    /// Nothing but the members the operation requires.
    Nothing,
    /// One `nextToken` value.
    Token(String),
    /// A page size at one end of this listing's own modelled `maxResults` range.
    PageSize(PageSizeBound),
    /// An unreadable token AND the smallest page size together.
    TokenAndPageSize,
}

/// Which published bound of a listing's `maxResults` range a case uses.
enum PageSizeBound {
    Bottom,
    Top,
}

impl Extra {
    /// The case label this extra contributes to an assertion message.
    fn label(&self) -> String {
        match self {
            Extra::Nothing => "no other member".to_string(),
            Extra::Token(token) if token.len() > 64 => {
                format!("the longest nextToken the model admits ({} chars)", token.len())
            }
            Extra::Token(token) => format!("nextToken={token}"),
            Extra::PageSize(PageSizeBound::Bottom) => {
                "maxResults at the bottom of the modelled range".to_string()
            }
            Extra::PageSize(PageSizeBound::Top) => {
                "maxResults at the top of the modelled range".to_string()
            }
            Extra::TokenAndPageSize => "an unreadable nextToken and a page size".to_string(),
        }
    }

    /// The query members this extra adds to a listing whose modelled `maxResults` range
    /// tops out at `max_results`.
    fn query(&self, max_results: u32) -> Vec<(String, String)> {
        match self {
            Extra::Nothing => Vec::new(),
            Extra::Token(token) => vec![("nextToken".to_string(), token.clone())],
            Extra::PageSize(PageSizeBound::Bottom) => {
                vec![("maxResults".to_string(), "1".to_string())]
            }
            Extra::PageSize(PageSizeBound::Top) => {
                vec![("maxResults".to_string(), max_results.to_string())]
            }
            Extra::TokenAndPageSize => vec![
                ("nextToken".to_string(), UNREADABLE_TOKEN.to_string()),
                ("maxResults".to_string(), "1".to_string()),
            ],
        }
    }
}

/// Everything a request may carry beside the domain it is scoped to, each row satisfying
/// every modelled constraint on the member it fills, so none of them can be refused by
/// the validation stage.
fn extras() -> Vec<Extra> {
    vec![
        Extra::Nothing,
        // Not an encoding this service ever produced, so a token it would refuse.
        Extra::Token(UNREADABLE_TOKEN.to_string()),
        // One it can read, naming a key nothing is stored under.
        Extra::Token(READABLE_TOKEN.to_string()),
        // The longest value `PaginationToken`'s modelled length admits (max 8192).
        Extra::Token("a".repeat(8192)),
        Extra::PageSize(PageSizeBound::Bottom),
        Extra::PageSize(PageSizeBound::Top),
        Extra::TokenAndPageSize,
    ]
}

/// One paginated domain-scoped listing: the operation the model binds, its path under
/// `/v2/domains/{domainIdentifier}`, the query members the model marks required, the top
/// of its own modelled `maxResults` range, and the output member it reports its page in.
struct Listing {
    operation: &'static str,
    path: String,
    required: Vec<(String, String)>,
    max_results: u32,
    items_member: &'static str,
}

impl Listing {
    /// This listing, scoped to `domain` and carrying `extra`.
    fn uri(&self, domain: &str, extra: &Extra) -> String {
        let mut query = self.required.clone();
        query.extend(extra.query(self.max_results));
        let uri = format!("/v2/domains/{domain}/{}", self.path);
        if query.is_empty() {
            return uri;
        }
        let query: Vec<String> = query
            .iter()
            .map(|(member, value)| format!("{member}={value}"))
            .collect();
        format!("{uri}?{}", query.join("&"))
    }

    /// The `message` this listing's denial must carry: the fixed sentence plus the one
    /// variable part, the operation name of the request.
    fn denial_message(&self) -> String {
        format!(
            "User is not permitted to perform operation: {}",
            self.operation
        )
    }
}

/// One domain holding a resource for every listing below.
struct Served {
    domain: String,
    root_unit: String,
    project: String,
    asset: String,
}

impl Served {
    /// Every paginated domain-scoped listing this emulator serves, each addressing the
    /// resources this fixture really stores, with the `maxResults` range its own input
    /// declares (50 for the `MaxResults` shape, 25 for `MaxResultsForListDomains`;
    /// SOURCE: smithy_query(query="operation", operation="ListAccountPools,ListProjects,\
    /// ListProjectMemberships,ListAssetRevisions,ListDomainUnitsForParent,\
    /// ListEntityOwners,ListPolicyGrants")).
    fn listings(&self) -> Vec<Listing> {
        vec![
            Listing {
                operation: "ListAccountPools",
                path: "account-pools".to_string(),
                required: Vec::new(),
                max_results: 50,
                items_member: "items",
            },
            Listing {
                operation: "ListProjects",
                path: "projects".to_string(),
                required: Vec::new(),
                max_results: 50,
                items_member: "items",
            },
            Listing {
                operation: "ListProjectMemberships",
                path: format!("projects/{}/memberships", self.project),
                required: Vec::new(),
                max_results: 50,
                items_member: "members",
            },
            Listing {
                operation: "ListAssetRevisions",
                path: format!("assets/{}/revisions", self.asset),
                required: Vec::new(),
                max_results: 50,
                items_member: "items",
            },
            Listing {
                operation: "ListDomainUnitsForParent",
                path: "domain-units".to_string(),
                required: vec![(
                    "parentDomainUnitIdentifier".to_string(),
                    self.root_unit.clone(),
                )],
                max_results: 25,
                items_member: "items",
            },
            Listing {
                operation: "ListEntityOwners",
                path: format!("entities/{ENTITY_TYPE}/{}/owners", self.root_unit),
                required: Vec::new(),
                max_results: 25,
                items_member: "owners",
            },
            Listing {
                operation: "ListPolicyGrants",
                path: format!(
                    "policies/managed/{ENTITY_TYPE}/{}/grants",
                    self.root_unit
                ),
                required: vec![("policyType".to_string(), POLICY_TYPE.to_string())],
                max_results: 25,
                items_member: "grantList",
            },
        ]
    }
}

/// The `message` member an answer carries, absent when the body carries none.
fn message_member(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Look again until `GetDomain` reports the domain has reached `status`, or a generous
/// deadline passes. Waits for the CONDITION the clock owes, never for a fixed time.
async fn poll_until_domain_status(dz: &Emulator, domain: &str, status: &str) {
    let started = Instant::now();
    loop {
        let answer = dz.call("GET", &format!("/v2/domains/{domain}"), None).await;
        if answer.status == 200 && answer.s("status") == status {
            return;
        }
        assert!(
            started.elapsed() < LIFECYCLE_DEADLINE,
            "domain {domain:?} never reached {status:?}: last answer {} {:?}",
            answer.status,
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until the domain the delete removed is gone — the identifier names no
/// domain any more, so even its own read is denied — or a generous deadline passes.
async fn poll_until_domain_is_gone(dz: &Emulator, domain: &str) {
    let started = Instant::now();
    loop {
        let answer = dz.call("GET", &format!("/v2/domains/{domain}"), None).await;
        if answer.status == 403 {
            return;
        }
        assert!(
            started.elapsed() < LIFECYCLE_DEADLINE,
            "domain {domain:?} was never removed: last answer {} {:?}",
            answer.status,
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a domain with a resource for every listing, through the same HTTP surface a
/// client uses.
async fn served(dz: &Emulator, domain_name: &str) -> Served {
    let created = dz
        .call("POST", "/v2/domains", Some(json!({ "name": domain_name })))
        .await;
    assert_eq!(
        created.status, 201,
        "creating domain {domain_name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    let domain = created.s("id");
    let root_unit = created.s("rootDomainUnitId");
    // The domain takes resources only once the lifecycle step the clock owes has run.
    poll_until_domain_status(dz, &domain, "AVAILABLE").await;

    let project = dz.project(&domain, "listing scope owner").await;

    let asset = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": "listing scope asset",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        asset.status, 201,
        "CreateAsset: expected 201, got {} {:?}",
        asset.status, asset.body
    );

    let pool = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/account-pools"),
            Some(json!({
                "name": "listing scope pool",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [
                        { "awsAccountId": "111122223333", "supportedRegions": ["us-east-1"] }
                    ]
                },
            })),
        )
        .await;
    assert_eq!(
        pool.status, 201,
        "CreateAccountPool: expected 201, got {} {:?}",
        pool.status, pool.body
    );

    Served {
        domain,
        root_unit,
        project,
        asset: asset.s("id"),
    }
}

/// Assert one answer is the modelled denial of `listing`, whatever the request carried.
fn assert_denied(listing: &Listing, case: &str, domain: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (403, "AccessDeniedException"),
        "case {case:?}: expected the modelled denial, got {} {:?}",
        answer.status,
        answer.body
    );
    let message = message_member(answer);
    assert_eq!(
        message,
        Some(listing.denial_message()),
        "case {case:?}: the denial must name only the operation, got {message:?}"
    );
    assert!(
        message
            .as_ref()
            .is_none_or(|text| !text.contains(domain) && !text.contains("nextToken")),
        "case {case:?}: the denial must state nothing about what the request carried, \
         got {message:?}"
    );
    assert!(
        answer.body.get(listing.items_member).is_none(),
        "case {case:?}: a denied listing must report no page, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_listing_scoped_to_an_unserved_domain_is_denied_whatever_else_it_carries() {
    let dz = Emulator::new();
    let fixture = served(&dz, "listing-order-domain").await;

    for (domain_label, unserved) in unserved_domain_ids() {
        for listing in fixture.listings() {
            for extra in extras() {
                // The resource identifiers are the ones the fixture really stores and
                // every other member satisfies its modelled constraints, so the only
                // thing wrong with the request is the domain it is scoped to.
                let case = format!(
                    "{} through {domain_label} carrying {}",
                    listing.operation,
                    extra.label()
                );
                let answer = dz.call("GET", &listing.uri(&unserved, &extra), None).await;
                assert_denied(&listing, &case, &unserved, &answer);
            }
        }
    }
}

#[tokio::test]
async fn the_same_listings_answer_from_state_under_the_domain_that_holds_them() {
    let dz = Emulator::new();
    let fixture = served(&dz, "listing-order-served").await;

    // The control for the rule above: authorization refuses an UNSERVED scope and
    // nothing more, so each listing still answers its page from the store, with or
    // without a page size at either published bound.
    for listing in fixture.listings() {
        for extra in [
            Extra::Nothing,
            Extra::PageSize(PageSizeBound::Bottom),
            Extra::PageSize(PageSizeBound::Top),
        ] {
            let case = format!("{} carrying {}", listing.operation, extra.label());
            let answer = dz
                .call("GET", &listing.uri(&fixture.domain, &extra), None)
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a listing under its own domain must answer from state, \
                 got {} {:?}",
                answer.status, answer.body
            );
            assert!(
                answer
                    .body
                    .get(listing.items_member)
                    .is_some_and(Value::is_array),
                "case {case:?}: the answer must report its page in {:?}, got {:?}",
                listing.items_member,
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_token_this_service_never_issued_is_refused_inside_a_domain_it_may_list() {
    let dz = Emulator::new();
    let fixture = served(&dz, "listing-order-token").await;

    // Inside a domain the caller may list, the service's own check decides again: a
    // token that is not an encoding it produced is invalid input (these listings model
    // no `ResourceNotFoundException` for it). So the denial above has not swallowed the
    // token check — it only runs before it. The rows differ along how the value fails to
    // be one of this service's tokens: not hex at all, hex of an odd length, and the
    // longest value the modelled length admits, whose bytes are not text.
    let unreadable = [
        UNREADABLE_TOKEN.to_string(),
        "abc".to_string(),
        "a".repeat(8192),
    ];
    for listing in fixture.listings() {
        for token in &unreadable {
            let case = format!(
                "{} carrying an unreadable nextToken of {} chars",
                listing.operation,
                token.len()
            );
            let answer = dz
                .call(
                    "GET",
                    &listing.uri(&fixture.domain, &Extra::Token(token.clone())),
                    None,
                )
                .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected the modelled refusal of a token this service \
                 never issued, got {} {:?}",
                answer.status,
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn every_listing_is_denied_once_its_domain_is_gone_token_and_all() {
    let dz = Emulator::new();
    let fixture = served(&dz, "listing-order-deleted").await;

    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{}", fixture.domain), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "DeleteDomain: expected the modelled success status, got {} {:?}",
        deleted.status, deleted.body
    );
    // Once the deletion the clock owed has completed, the identifier names no domain, so
    // nothing is held on it any more.
    poll_until_domain_is_gone(&dz, &fixture.domain).await;

    for listing in fixture.listings() {
        for extra in [
            Extra::Nothing,
            Extra::Token(UNREADABLE_TOKEN.to_string()),
            Extra::Token(READABLE_TOKEN.to_string()),
        ] {
            let case = format!(
                "{} after the domain was removed, carrying {}",
                listing.operation,
                extra.label()
            );
            let answer = dz
                .call("GET", &listing.uri(&fixture.domain, &extra), None)
                .await;
            assert_denied(&listing, &case, &fixture.domain, &answer);
        }
    }
}
