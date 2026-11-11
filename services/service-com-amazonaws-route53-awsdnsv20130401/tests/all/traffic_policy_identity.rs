// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of what IDENTIFIES a Route 53 traffic policy: the `Id`
//! the service mints for it, and nothing else.
//!
//! The rules under test:
//!
//! 1. The policy RECORD is keyed on that `Id` independently of its version set.
//!    `DeleteTrafficPolicy` flags the one version it names and "Route 53 never
//!    fully deletes the traffic policy … If you retain the ID of the policy, you
//!    can get information about the policy", so an `Id` the service minted keeps
//!    resolving however many of its versions are flagged: its version listing is
//!    then an EMPTY collection — `TrafficPolicies` empty, `IsTruncated` false and
//!    the request's `maxitems` echoed — a further version can still be added to
//!    it, numbered one past the highest it ever reached, and that version can be
//!    deleted again. Only an `Id` no policy record is keyed under is the modelled
//!    `NoSuchTrafficPolicy` (HTTP 404), which is what distinguishes an empty
//!    collection from a missing resource.
//! 2. The `Name` is the policy's LABEL, not its identity and not a uniqueness
//!    constraint: a create carrying a `Name` another policy already holds is
//!    served (HTTP 201) with a newly minted `Id` of its own, its own `Version` 1,
//!    its own document, type and comment, and its own `Location` — so several
//!    policies may carry one name and each reads back as itself.
//!
//! The tables vary the axes the rules generalise over rather than renaming a
//! policy: how many versions the policy holds and the ORDER its versions are
//! deleted in, the `maxitems` the emptied listing is asked for, how many policies
//! share one name, whether those policies' documents and comments differ or are
//! an exact replay of each other, the record type and entry-point form each
//! document carries, and the name's own form and length.
//!
//! Every test drives the service through the real router, using the SAME harness
//! the other traffic-policy suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use service_com_amazonaws_route53_awsdnsv20130401::ops::list_traffic_policy_versions::DEFAULT_PAGE_SIZE;
use service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::MAX_TRAFFIC_POLICY_VERSION;

use crate::hosted_zone_records::{router, Reply, ENDPOINT};
use crate::traffic_policies::{
    create_policy, document_with_start_endpoint, document_with_start_rule, get_policy,
    policy_count, policy_of, PolicyView,
};
use crate::traffic_policy_deletion::{delete_policy, policy_with_versions};
use crate::traffic_policy_not_found_envelope::assert_no_such_traffic_policy;
use crate::traffic_policy_version_listing::list_versions;
use crate::traffic_policy_versions::create_version;

/// `TrafficPolicyName` is `len 0..512`
/// (smithy_query(query="shape",shape="TrafficPolicyName")).
const NAME_MAX_LEN: usize = 512;

/// The `Location` a traffic policy version is reported at: the URL of the
/// `(Id, Version)` pair `GetTrafficPolicy` reads it back through.
fn expected_location(id: &str, version: &str) -> String {
    format!("https://{ENDPOINT}/2013-04-01/trafficpolicy/{id}/{version}")
}

/// Assert `reply` reports the one traffic policy version `expected` records, and
/// return it.
fn assert_policy_is(reply: &Reply, case: &str, expected: &PolicyView) -> PolicyView {
    let got = policy_of(reply, case);
    assert_eq!(
        got, *expected,
        "case {case:?}: expected the traffic policy {expected:?}, got {got:?}; body {}",
        reply.body
    );
    got
}

// ---------------------------------------------------------------------------
// The record outlives its versions
// ---------------------------------------------------------------------------

/// One row of the emptied-policy table: the versions the policy is built with,
/// the order they are all deleted in, and the `maxitems` its emptied listing is
/// asked for.
struct EmptiedRow {
    case: &'static str,
    /// The record type of each version, in version order.
    version_types: &'static [&'static str],
    /// Every version number, in the order this row deletes them.
    delete_order: &'static [i32],
    /// The `maxitems` query the emptied listing carries, and the value the
    /// response has to echo. `None` sends no `maxitems` at all, which Route 53
    /// answers with the page bound it applied.
    max_items: Option<i32>,
}

const EMPTIED_ROWS: &[EmptiedRow] = &[
    EmptiedRow {
        case: "a one-version policy, emptied, listed without maxitems",
        version_types: &["A"],
        delete_order: &[1],
        max_items: None,
    },
    EmptiedRow {
        case: "a two-version policy, emptied in numerical order",
        version_types: &["A", "AAAA"],
        delete_order: &[1, 2],
        max_items: Some(1),
    },
    EmptiedRow {
        case: "a two-version policy, emptied latest first",
        version_types: &["CNAME", "TXT"],
        delete_order: &[2, 1],
        max_items: Some(100),
    },
    EmptiedRow {
        case: "a three-version policy, emptied middle, latest, first",
        version_types: &["A", "MX", "SRV"],
        delete_order: &[2, 3, 1],
        max_items: Some(2),
    },
    EmptiedRow {
        case: "a four-version policy, emptied out of numerical order",
        version_types: &["TXT", "A", "AAAA", "CNAME"],
        delete_order: &[3, 1, 4, 2],
        max_items: Some(MAX_TRAFFIC_POLICY_VERSION),
    },
];

#[tokio::test]
async fn a_policy_record_outlives_every_version_of_it() {
    for row in EMPTIED_ROWS {
        let case = row.case;
        assert_eq!(
            row.delete_order.len(),
            row.version_types.len(),
            "case {case:?}: the row must delete every version it builds, got {} delete(s) for \
             {} version(s)",
            row.delete_order.len(),
            row.version_types.len()
        );
        let router = router();
        let built = policy_with_versions(&router, case, "outliving", row.version_types).await;
        let id = built[0].id.clone();

        // Every version of the policy goes, in this row's order. Each delete is
        // served with an empty response: the operation's output shape has no
        // members at all.
        for &version in row.delete_order {
            let reply = delete_policy(&router, &id, &version.to_string()).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200 deleting version {version}, got {} with body {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_code().is_empty(),
                "case {case:?}: a served delete must carry no error, got {:?}; body {}",
                reply.error_code(),
                reply.body
            );
        }

        // The id still resolves, and its version listing is an EMPTY
        // collection: no version, nothing truncated, and the request's own
        // maxitems echoed back.
        let query = row
            .max_items
            .map_or_else(String::new, |max_items| format!("?maxitems={max_items}"));
        let listing = list_versions(&router, &id, &query).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: expected 200 listing the versions of an emptied policy, got {} \
             with body {}",
            listing.status, listing.body
        );
        let listed = listing.all("TrafficPolicy/Version");
        assert!(
            listed.is_empty(),
            "case {case:?}: an emptied policy must list no version, got {listed:?}; body {}",
            listing.body
        );
        assert_eq!(
            listing.first("IsTruncated"),
            Some("false"),
            "case {case:?}: an emptied listing ends where it starts, got {:?}; body {}",
            listing.first("IsTruncated"),
            listing.body
        );
        let expected_max_items = row.max_items.unwrap_or(DEFAULT_PAGE_SIZE).to_string();
        assert_eq!(
            listing.first("MaxItems"),
            Some(expected_max_items.as_str()),
            "case {case:?}: expected MaxItems {expected_max_items:?}, got {:?}; body {}",
            listing.first("MaxItems"),
            listing.body
        );

        // A further version is still added to the record: the id and name are
        // the POLICY's, the document, type and comment are THIS request's, and
        // the number is one past the highest the policy ever reached — a flagged
        // version never frees its number.
        let next = i32::try_from(built.len()).expect("the suite builds few versions") + 1;
        let document = document_with_start_rule("TXT", &format!("v={next}"), "v=backup");
        let comment = format!("the version after the policy was emptied ({next})");
        let created = create_version(&router, &id, &document, Some(&comment)).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 adding a version to an emptied policy, got {} with \
             body {}",
            created.status, created.body
        );
        let expected = PolicyView {
            id: id.clone(),
            version: next.to_string(),
            name: "outliving".to_string(),
            record_type: "TXT".to_string(),
            document: document.clone(),
            comment: Some(comment.clone()),
        };
        let added = assert_policy_is(&created, case, &expected);
        assert_eq!(
            created.location.as_deref(),
            Some(expected_location(&id, &added.version).as_str()),
            "case {case:?}: expected the Location of the new version, got {:?}; body {}",
            created.location,
            created.body
        );

        // Round trip: the added version reads back through GetTrafficPolicy,
        // and the policy's listing now reports exactly it.
        let read = get_policy(&router, &id, &added.version).await;
        assert_policy_is(&read, case, &expected);
        let listing = list_versions(&router, &id, "").await;
        let listed = listing.all("TrafficPolicy/Version");
        assert_eq!(
            listed,
            vec![added.version.as_str()],
            "case {case:?}: an emptied policy's listing must report the version added to it, \
             got {listed:?}; body {}",
            listing.body
        );

        // Every version the deletes flagged still reads back by the retained
        // id, exactly as it was created.
        for held in &built {
            let read = get_policy(&router, &held.id, &held.version).await;
            assert_policy_is(&read, case, held);
        }

        // And the added version is deletable in its turn, which empties the
        // policy again.
        let reply = delete_policy(&router, &id, &added.version).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 deleting the added version {}, got {} with body {}",
            added.version, reply.status, reply.body
        );
        let listing = list_versions(&router, &id, "").await;
        let listed = listing.all("TrafficPolicy/Version");
        assert!(
            listed.is_empty(),
            "case {case:?}: deleting the added version must empty the listing again, got \
             {listed:?}; body {}",
            listing.body
        );
    }
}

#[tokio::test]
async fn an_id_no_policy_was_minted_under_is_refused() {
    // The distinction rule 1 turns on: an emptied policy answers an empty
    // collection, while an id that names no policy record at all is refused.
    let case = "an emptied policy beside ids that name no policy";
    let router = router();
    let built = policy_with_versions(&router, case, "the-only-policy", &["A", "AAAA"]).await;
    let id = built[0].id.clone();
    for held in &built {
        let reply = delete_policy(&router, &id, &held.version).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 deleting version {}, got {} with body {}",
            held.version, reply.status, reply.body
        );
    }

    let mut near_miss = id.clone();
    near_miss.pop();
    near_miss.push(if id.ends_with('0') { '1' } else { '0' });
    let rows = [
        (
            "an id in the minted form that no policy carries",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("an id one character off the emptied one", near_miss),
        ("an id that is not in the minted form at all", "x".to_string()),
    ];

    for (case, unknown) in &rows {
        for reply in [
            list_versions(&router, unknown, "").await,
            create_version(
                &router,
                unknown,
                &document_with_start_endpoint("A", "192.0.2.9"),
                None,
            )
            .await,
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404 for an id that names no policy, got {} with \
                 body {}",
                reply.status, reply.body
            );
            // Route 53 reports a traffic policy it does not hold by its code
            // alone: `Type` and `Code`, with no `Message` element.
            assert_no_such_traffic_policy(&reply, case);
        }
    }

    // The emptied policy, by contrast, still serves both operations.
    let listing = list_versions(&router, &id, "").await;
    assert_eq!(
        listing.status, 200,
        "case {case:?}: an emptied policy must still list its versions, got {} with body {}",
        listing.status, listing.body
    );
    let added = create_version(
        &router,
        &id,
        &document_with_start_endpoint("A", "192.0.2.10"),
        None,
    )
    .await;
    assert_eq!(
        added.status, 201,
        "case {case:?}: an emptied policy must still take a version, got {} with body {}",
        added.status, added.body
    );
}

// ---------------------------------------------------------------------------
// A name is a label, not an identity
// ---------------------------------------------------------------------------

/// One row of the shared-name table: the name several policies carry, and one
/// `(document, comment, type)` triple per policy that carries it.
struct SharedNameRow {
    case: &'static str,
    name: String,
    /// `(document, comment, expected Type)` for each policy, in creation order.
    policies: Vec<(String, Option<String>, &'static str)>,
}

fn shared_name_rows() -> Vec<SharedNameRow> {
    vec![
        SharedNameRow {
            case: "two policies whose documents differ in record type",
            name: "shared-name".to_string(),
            policies: vec![
                (
                    document_with_start_endpoint("A", "192.0.2.1"),
                    Some("the first policy of this name".to_string()),
                    "A",
                ),
                (
                    document_with_start_rule("CNAME", "a.example.com", "b.example.com"),
                    None,
                    "CNAME",
                ),
            ],
        },
        SharedNameRow {
            case: "an exact replay of a create: same name, document and comment",
            name: "replayed".to_string(),
            policies: vec![
                (
                    document_with_start_endpoint("AAAA", "2001:db8::1"),
                    Some("sent twice".to_string()),
                    "AAAA",
                ),
                (
                    document_with_start_endpoint("AAAA", "2001:db8::1"),
                    Some("sent twice".to_string()),
                    "AAAA",
                ),
            ],
        },
        SharedNameRow {
            case: "three policies of one name, differing in comment only",
            name: "three-of-a-name".to_string(),
            policies: vec![
                (
                    document_with_start_endpoint("TXT", "v=1"),
                    Some("first".to_string()),
                    "TXT",
                ),
                (document_with_start_endpoint("TXT", "v=2"), None, "TXT"),
                (
                    document_with_start_endpoint("TXT", "v=3"),
                    Some("third".to_string()),
                    "TXT",
                ),
            ],
        },
        SharedNameRow {
            case: "a name carrying XML special characters, held twice",
            name: "quoted & escaped <name>".to_string(),
            policies: vec![
                (
                    document_with_start_endpoint("MX", "10 mail.example.com"),
                    Some("a&b<c>d\"e'f".to_string()),
                    "MX",
                ),
                (
                    document_with_start_endpoint("MX", "20 backup.example.com"),
                    None,
                    "MX",
                ),
            ],
        },
        SharedNameRow {
            // The published maximum of `TrafficPolicyName`, held by two
            // policies, so the rule does not stop at the bound.
            case: "a name at the 512-character maximum, held twice",
            name: "n".repeat(NAME_MAX_LEN),
            policies: vec![
                (
                    document_with_start_rule("SRV", "1 1 80 a.example.com", "1 1 80 b.example.com"),
                    None,
                    "SRV",
                ),
                (
                    document_with_start_endpoint("SRV", "1 1 443 c.example.com"),
                    Some("the second holder of the long name".to_string()),
                    "SRV",
                ),
            ],
        },
    ]
}

#[tokio::test]
async fn several_policies_may_carry_one_name() {
    for row in shared_name_rows() {
        let case = row.case;
        // Every record type the row spans is one the model's own `RRType` enum
        // names, so no row asserts a type a traffic policy could not carry.
        for (_, _, record_type) in &row.policies {
            assert!(
                R_R_TYPE_VALUES.contains(record_type),
                "case {case:?}: {record_type} is not a modelled RRType"
            );
        }

        let router = router();
        let mut created: Vec<PolicyView> = Vec::new();
        for (document, comment, record_type) in &row.policies {
            let reply = create_policy(&router, &row.name, document, comment.as_deref()).await;
            assert_eq!(
                reply.status, 201,
                "case {case:?}: expected 201 for policy {} of this name, got {} with body {}",
                created.len() + 1,
                reply.status,
                reply.body
            );
            let expected = PolicyView {
                id: policy_of(&reply, case).id,
                version: "1".to_string(),
                name: row.name.clone(),
                record_type: (*record_type).to_string(),
                document: document.clone(),
                comment: comment.clone(),
            };
            let got = assert_policy_is(&reply, case, &expected);
            // The `Location` the model requires names the policy this create
            // minted, not the one that already carried the name.
            assert_eq!(
                reply.location.as_deref(),
                Some(expected_location(&got.id, "1").as_str()),
                "case {case:?}: expected the Location of the minted policy, got {:?}; body {}",
                reply.location,
                reply.body
            );
            created.push(got);
        }

        // Every policy of the name has an id of its own.
        for (at, held) in created.iter().enumerate() {
            for other in &created[at + 1..] {
                assert_ne!(
                    held.id, other.id,
                    "case {case:?}: policies sharing a name must not share an id, got {:?} twice",
                    held.id
                );
            }
        }

        // And each one reads back as ITSELF: the shared name never merges two
        // policies, whatever their documents and comments.
        for held in &created {
            let read = get_policy(&router, &held.id, "1").await;
            assert_policy_is(&read, case, held);
        }

        let count = policy_count(&router).await;
        let expected_count = created.len().to_string();
        assert_eq!(
            count, expected_count,
            "case {case:?}: the account holds every policy of the name, expected Count \
             {expected_count:?}, got {count:?}"
        );
    }
}
