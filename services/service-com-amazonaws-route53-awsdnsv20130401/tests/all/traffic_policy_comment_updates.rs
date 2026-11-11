// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateTrafficPolicyComment`.
//!
//! The rules under test:
//!
//! 1. The update REPLACES the comment of the one traffic policy version its
//!    `(Id, Version)` names with the `Comment` the request carries, and answers
//!    with that version as the store now holds it. Nothing else about the
//!    version moves — its `Document` is its definition and its `Type` is read
//!    off that document, and the `Id` and `Name` belong to the policy — and no
//!    other version of the policy, and no other policy, is touched.
//!    `GetTrafficPolicy` reports the same thing afterwards. The table spans the
//!    comment's length (including both published bounds of
//!    `TrafficPolicyComment`), its character set, whether the version already
//!    carried a comment, which version of the policy is addressed, and an
//!    update repeated with the comment the version already carries.
//! 2. Both halves of `(Id, Version)` address the version, so either half naming
//!    something the store does not hold is the modelled `NoSuchTrafficPolicy`
//!    (HTTP 404), worded as the one fixed sentence Route 53 answers a traffic
//!    policy it does not hold with, and persists nothing; a `Version` outside
//!    the modelled range, or a `Comment` past its modelled maximum, never
//!    reaches the handler at all.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other traffic-policy suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, escape_xml, router, Reply};
use crate::traffic_policies::{
    create_policy, document_with_start_endpoint, document_with_start_rule, get_policy,
    policy_count, policy_of, PolicyView,
};
use crate::traffic_policy_not_found_envelope::assert_no_such_traffic_policy;
use crate::traffic_policy_versions::create_version;

/// `TrafficPolicyComment` is `len 0..1024`
/// (smithy_query(query="shape",shape="TrafficPolicyComment")).
const COMMENT_MAX_LEN: usize = 1024;

/// `TrafficPolicyVersion` is `range 1..1000`
/// (smithy_query(query="shape",shape="TrafficPolicyVersion")).
const VERSION_MIN: i64 = 1;
/// The other published bound of `TrafficPolicyVersion`.
const VERSION_MAX: i64 = 1000;

/// Send one `UpdateTrafficPolicyComment` for `(id, version)`.
///
/// `pub(crate)` so the suite asserting the shape of the traffic-policy
/// not-found body drives this operation through the SAME request rather than
/// growing a second copy of it.
pub(crate) async fn update_comment(
    router: &MultiServiceRouter,
    id: &str,
    version: &str,
    comment: &str,
) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/trafficpolicy/{id}/{version}"),
        &format!(
            "<UpdateTrafficPolicyCommentRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <Comment>{}</Comment>\
             </UpdateTrafficPolicyCommentRequest>",
            escape_xml(comment)
        ),
    )
    .await
}

/// The `Comment` a reply reports for text the request sent.
///
/// The harness records an element only when it carries text, so a comment of
/// the empty text the `0` minimum allows is reported as no text at all: the two
/// are indistinguishable on this wire, and this is the one place that maps a
/// sent comment onto what a reply can report.
fn reported_comment(sent: &str) -> Option<String> {
    (!sent.is_empty()).then(|| sent.to_string())
}

/// A policy and every version of it, as the creates answered with them.
struct Held {
    id: String,
    /// Every version this policy holds, keyed by its `Version` as the wire
    /// reports it, so a test can check the versions it did NOT address are
    /// still exactly what they were.
    versions: BTreeMap<String, PolicyView>,
}

/// Create one policy whose versions differ in record type, entry-point form and
/// whether they were created with a comment at all.
async fn held_policy(router: &MultiServiceRouter, name: &str) -> Held {
    let created = create_policy(
        router,
        name,
        &document_with_start_endpoint("A", "192.0.2.1"),
        Some("the comment the first version was created with"),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateTrafficPolicy for {name:?}, got {} with body {}",
        created.status, created.body
    );
    let first = policy_of(&created, name);

    let mut versions = BTreeMap::new();
    versions.insert(first.version.clone(), first.clone());

    // A version created WITHOUT a comment, so the table can set one where there
    // was none, and one created with a comment of its own.
    for (document, comment) in [
        (document_with_start_endpoint("AAAA", "2001:db8::1"), None),
        (
            document_with_start_rule("CNAME", "a.example.com", "b.example.com"),
            Some("the comment the third version was created with"),
        ),
    ] {
        let reply = create_version(router, &first.id, &document, comment).await;
        assert_eq!(
            reply.status, 201,
            "expected 201 from CreateTrafficPolicyVersion for {name:?}, got {} with body {}",
            reply.status, reply.body
        );
        let version = policy_of(&reply, name);
        versions.insert(version.version.clone(), version);
    }

    Held {
        id: first.id,
        versions,
    }
}

/// Assert every version of `held` reads back exactly as `held` records it.
async fn assert_versions_read_back(router: &MultiServiceRouter, held: &Held, case: &str) {
    for (version, expected) in &held.versions {
        let read = get_policy(router, &held.id, version).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetTrafficPolicy for version {version}, \
             got {} with body {}",
            read.status, read.body
        );
        let got = policy_of(&read, case);
        assert_eq!(
            &got, expected,
            "case {case:?}: GetTrafficPolicy must report version {version} as {expected:?}, \
             got {got:?}; body {}",
            read.body
        );
    }

    // An update is not a create: the policy holds no version past the ones that
    // were created, and the account holds no further policy.
    let past_the_last = held
        .versions
        .keys()
        .next_back()
        .and_then(|highest| highest.parse::<i64>().ok())
        .map(|highest| highest + 1)
        .expect("the held policy must carry at least one numeric version");
    let absent = get_policy(router, &held.id, &past_the_last.to_string()).await;
    assert_eq!(
        absent.status, 404,
        "case {case:?}: a comment update must add no version, got {} for version \
         {past_the_last} with body {}",
        absent.status, absent.body
    );
}

// ---------------------------------------------------------------------------
// The update replaces one version's comment, and it reads back
// ---------------------------------------------------------------------------

/// One row of the update table: the version addressed, and the comment sent.
struct UpdateRow {
    case: &'static str,
    version: &'static str,
    comment: String,
}

#[tokio::test]
async fn each_update_replaces_only_the_addressed_versions_comment() {
    let repeated = "a comment sent twice".to_string();
    let rows = [
        UpdateRow {
            case: "replacing a comment the version already carried",
            version: "1",
            comment: "a replacement for the first version's comment".to_string(),
        },
        UpdateRow {
            case: "setting a comment on a version created without one",
            version: "2",
            comment: "a comment where the version had none".to_string(),
        },
        UpdateRow {
            case: "a comment at the 1024-character maximum",
            version: "3",
            comment: "c".repeat(COMMENT_MAX_LEN),
        },
        UpdateRow {
            case: "a comment of the empty text the 0 minimum allows",
            version: "1",
            comment: String::new(),
        },
        UpdateRow {
            case: "a comment carrying XML special characters",
            version: "2",
            comment: "a&b<c>d\"e'f".to_string(),
        },
        UpdateRow {
            case: "a comment of non-ASCII text",
            version: "3",
            comment: "politique de trafic — proche de l'Europe".to_string(),
        },
        UpdateRow {
            case: "a comment the version does not carry yet, sent once",
            version: "2",
            comment: repeated.clone(),
        },
        UpdateRow {
            case: "the same comment sent again, which changes nothing",
            version: "2",
            comment: repeated,
        },
    ];

    let router = router();
    let mut held = held_policy(&router, "comment-updates").await;

    for row in &rows {
        let before = held
            .versions
            .get(row.version)
            .unwrap_or_else(|| panic!("case {:?}: the table must address a held version", row.case))
            .clone();

        let reply = update_comment(&router, &held.id, row.version, &row.comment).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from UpdateTrafficPolicyComment, got {} with body {}",
            row.case, reply.status, reply.body
        );

        // Only the comment moves: the id, name, version, type and document the
        // version already had are reported unchanged.
        let expected = PolicyView {
            comment: reported_comment(&row.comment),
            ..before
        };
        let answered = policy_of(&reply, row.case);
        assert_eq!(
            answered, expected,
            "case {:?}: the update must answer with the version it persisted, \
             expected {expected:?}, got {answered:?}; body {}",
            row.case, reply.body
        );

        held.versions.insert(row.version.to_string(), expected);

        // Round trip: the updated version, and every version the request did
        // not address, report what the store now holds.
        assert_versions_read_back(&router, &held, row.case).await;

        let count = policy_count(&router).await;
        assert_eq!(
            count, "1",
            "case {:?}: a comment update must not add a traffic policy, got Count {count:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_update_reaches_only_the_policy_its_id_names() {
    // Two policies each holding a version 1: the `Id` picks which policy's
    // version the comment belongs to, so the other policy's same-numbered
    // version is left exactly as it was.
    let router = router();
    let one = held_policy(&router, "first-policy").await;
    let two = held_policy(&router, "second-policy").await;
    assert_ne!(
        one.id, two.id,
        "two policies must not share an id, got {:?} twice",
        one.id
    );

    let mut one = one;
    for version in ["1", "2", "3"] {
        let comment = format!("only the first policy's version {version}");
        let reply = update_comment(&router, &one.id, version, &comment).await;
        assert_eq!(
            reply.status, 200,
            "version {version}: expected 200 from UpdateTrafficPolicyComment, got {} with body {}",
            reply.status, reply.body
        );
        let before = one
            .versions
            .get(version)
            .expect("the policy must hold the version being updated")
            .clone();
        let expected = PolicyView {
            comment: reported_comment(&comment),
            ..before
        };
        let answered = policy_of(&reply, version);
        assert_eq!(
            answered, expected,
            "version {version}: expected {expected:?}, got {answered:?}; body {}",
            reply.body
        );
        one.versions.insert(version.to_string(), expected);
    }

    assert_versions_read_back(&router, &one, "the updated policy").await;
    assert_versions_read_back(&router, &two, "the policy no update addressed").await;
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_or_version_the_store_does_not_hold_is_refused() {
    let router = router();
    let held = held_policy(&router, "the-only-policy").await;

    // An id that differs from the held one only in its last character, so a
    // refusal cannot pass by matching a prefix.
    let mut near_miss = held.id.clone();
    near_miss.pop();
    near_miss.push(if held.id.ends_with('0') { '1' } else { '0' });

    let rows = [
        (
            "an id in the minted form that no policy carries",
            "00000000-0000-0000-0000-000000000000".to_string(),
            VERSION_MIN.to_string(),
        ),
        (
            "an id one character off the held one",
            near_miss,
            VERSION_MIN.to_string(),
        ),
        (
            "an id that is not in the minted form at all",
            "x".to_string(),
            VERSION_MIN.to_string(),
        ),
        (
            "a version one past the highest the policy holds",
            held.id.clone(),
            "4".to_string(),
        ),
        (
            "the highest version the model allows, which this policy does not hold",
            held.id.clone(),
            VERSION_MAX.to_string(),
        ),
    ];

    for (case, id, version) in &rows {
        let reply = update_comment(&router, id, version, "a comment for nothing").await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404 for an (Id, Version) the store does not hold, \
             got {} with body {}",
            reply.status, reply.body
        );
        // Route 53 reports a traffic policy it does not hold by its code alone,
        // so both halves of `(Id, Version)`, in every form above, are refused
        // with the same body: `Type` and `Code` and no `Message` element.
        assert_no_such_traffic_policy(&reply, case);
    }

    assert_versions_read_back(&router, &held, "after the refusals").await;
}

#[tokio::test]
async fn an_input_outside_the_modelled_bounds_never_reaches_the_store() {
    // The published bounds of `TrafficPolicyVersion` (1..1000) and of
    // `TrafficPolicyComment` (0..1024): one step past either is refused with
    // HTTP 400 before the handler, and the versions the policy holds are left
    // as they were.
    let router = router();
    let held = held_policy(&router, "bounds").await;

    let rows = [
        (
            "a version below the modelled minimum",
            (VERSION_MIN - 1).to_string(),
            "a comment for an impossible version".to_string(),
        ),
        (
            "a version above the modelled maximum",
            (VERSION_MAX + 1).to_string(),
            "a comment for an impossible version".to_string(),
        ),
        (
            "a comment one character past the modelled maximum",
            VERSION_MIN.to_string(),
            "c".repeat(COMMENT_MAX_LEN + 1),
        ),
    ];

    for (case, version, comment) in &rows {
        let reply = update_comment(&router, &held.id, version, comment).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for an input outside the modelled bounds, \
             got {} with body {}",
            reply.status, reply.body
        );
    }

    assert_versions_read_back(&router, &held, "after the bound refusals").await;
}
