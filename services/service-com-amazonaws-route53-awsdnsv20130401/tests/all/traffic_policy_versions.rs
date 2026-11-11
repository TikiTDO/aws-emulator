// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CreateTrafficPolicyVersion`: a further version of a
//! traffic policy the account already holds.
//!
//! The rules under test:
//!
//! 1. A version create adds ONE version to the policy its `Id` names and
//!    answers with it: the policy's own `Id` and `Name`, a `Version` one past
//!    the highest that policy has reached, the `Document` and `Comment` THIS
//!    request carried, and a `Type` read off that document. `GetTrafficPolicy`
//!    on `(Id, Version)` reports exactly the same version afterwards, and every
//!    version created before it still reports what it was created with. The
//!    table spans the record type, the document's entry-point form, the number
//!    of endpoints, whether a comment was given, and the comment's length.
//! 2. Version numbers are per POLICY and only ever count upwards, so two
//!    policies number their versions independently, and a new version of a
//!    policy is not a new policy — the account's traffic policy count does not
//!    move for one.
//! 3. An `Id` the account holds no policy under names nothing to add a version
//!    to: the modelled `NoSuchTrafficPolicy` (HTTP 404), worded as the one
//!    fixed sentence Route 53 refuses a traffic policy it does not hold with,
//!    and nothing is persisted.
//! 4. The document is the version's definition, so a document the published
//!    traffic policy document format does not define is refused with the
//!    modelled `InvalidTrafficPolicyDocument` (HTTP 400) and adds no version —
//!    the same validator the policy's first version went through.
//! 5. A policy whose highest version is the last number a version can carry is
//!    refused with the modelled
//!    `TooManyTrafficPolicyVersionsForCurrentPolicy` (HTTP 400), and the
//!    versions it already holds are left as they were.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other traffic-policy suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    MAX_TRAFFIC_POLICY_VERSION, TRAFFIC_POLICY_FORMAT_VERSION,
};

use crate::hosted_zone_records::{call, escape_xml, router, Reply, ENDPOINT};
use crate::traffic_policies::{
    create_policy, document_with_start_endpoint, document_with_start_rule, get_policy, policy_count,
    policy_of, PolicyView,
};
use crate::traffic_policy_not_found_envelope::assert_no_such_traffic_policy;

/// `TrafficPolicyComment` is `len 0..1024`
/// (smithy_query(query="shape",shape="TrafficPolicyComment")).
const COMMENT_MAX_LEN: usize = 1024;

/// Create a further version of the policy `id` names. `comment` of `None`
/// sends no `Comment` element.
///
/// `pub(crate)` so the listing suite builds its multi-version policies, and the
/// traffic-policy-INSTANCE suite applies a created version, through the SAME
/// request rather than growing a second copy of it.
pub(crate) async fn create_version(
    router: &MultiServiceRouter,
    id: &str,
    document: &str,
    comment: Option<&str>,
) -> Reply {
    let comment = comment.map_or_else(String::new, |text| {
        format!("<Comment>{}</Comment>", escape_xml(text))
    });
    call(
        router,
        "POST",
        &format!("/2013-04-01/trafficpolicy/{id}"),
        &format!(
            "<CreateTrafficPolicyVersionRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <Document>{}</Document>{comment}\
             </CreateTrafficPolicyVersionRequest>",
            escape_xml(document)
        ),
    )
    .await
}

/// Create a policy and return the `TrafficPolicy` its create answered with.
async fn policy_with_first_version(
    router: &MultiServiceRouter,
    name: &str,
    document: &str,
    comment: Option<&str>,
) -> PolicyView {
    let created = create_policy(router, name, document, comment).await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateTrafficPolicy for {name:?}, got {} with body {}",
        created.status, created.body
    );
    policy_of(&created, name)
}

// ---------------------------------------------------------------------------
// A version create adds one version, and it reads back
// ---------------------------------------------------------------------------

/// One row of the version table: the request sent, and the `Type` the document
/// it carries names.
struct VersionRow {
    case: &'static str,
    document: String,
    comment: Option<String>,
    expect_type: &'static str,
}

#[tokio::test]
async fn each_created_version_reads_back_as_it_was_created() {
    let long_comment = "c".repeat(COMMENT_MAX_LEN);
    let rows = [
        VersionRow {
            case: "a second version keeping the first version's record type",
            document: document_with_start_endpoint("A", "192.0.2.2"),
            comment: Some("one endpoint, moved".to_string()),
            expect_type: "A",
        },
        VersionRow {
            case: "a version whose record type differs from the first version's",
            document: document_with_start_endpoint("AAAA", "2001:db8::2"),
            comment: None,
            expect_type: "AAAA",
        },
        VersionRow {
            case: "a version whose entry point is a rule over two endpoints",
            document: document_with_start_rule("CNAME", "a.example.com", "b.example.com"),
            comment: Some("primary with a backup".to_string()),
            expect_type: "CNAME",
        },
        VersionRow {
            case: "a version whose document carries XML special characters",
            document: document_with_start_endpoint("TXT", "v=spf1 <include:example.com> ~all"),
            comment: Some("a&b<c>d\"e'f".to_string()),
            expect_type: "TXT",
        },
        VersionRow {
            case: "a version with a comment at the 1024-character maximum",
            document: document_with_start_rule("SRV", "1 1 80 a.example.com", "1 1 80 b.example.com"),
            comment: Some(long_comment.clone()),
            expect_type: "SRV",
        },
        VersionRow {
            case: "a version with no comment at all",
            document: document_with_start_endpoint("MX", "10 mail.example.com"),
            comment: None,
            expect_type: "MX",
        },
    ];

    // Every record type the table spans is one the model's own `RRType` enum
    // names, so no row asserts a type a traffic policy version could not carry.
    for row in &rows {
        assert!(
            R_R_TYPE_VALUES.contains(&row.expect_type),
            "case {:?}: {} is not a modelled RRType",
            row.case,
            row.expect_type
        );
    }

    let router = router();
    let first = policy_with_first_version(
        &router,
        "versioned",
        &document_with_start_endpoint("A", "192.0.2.1"),
        Some("the first version"),
    )
    .await;

    // Every version this suite has seen so far, so each create can be checked
    // against the ones before it.
    let mut held = vec![first.clone()];

    for (index, row) in rows.iter().enumerate() {
        let expected_version = (index + 2).to_string();
        let created = create_version(&router, &first.id, &row.document, row.comment.as_deref()).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateTrafficPolicyVersion, got {} with body {}",
            row.case, created.status, created.body
        );
        let answered = policy_of(&created, row.case);
        let expected = PolicyView {
            id: first.id.clone(),
            version: expected_version.clone(),
            name: first.name.clone(),
            record_type: row.expect_type.to_string(),
            document: row.document.clone(),
            comment: row.comment.clone(),
        };
        assert_eq!(
            answered, expected,
            "case {:?}: CreateTrafficPolicyVersion must answer with the version it persisted, \
             got {answered:?}; body {}",
            row.case, created.body
        );

        // The Location names the version just created, at the endpoint the
        // request was sent to.
        let expected_location = format!(
            "https://{ENDPOINT}/2013-04-01/trafficpolicy/{}/{expected_version}",
            first.id
        );
        assert_eq!(
            created.location.as_deref(),
            Some(expected_location.as_str()),
            "case {:?}: expected the Location {expected_location:?}, got {:?}",
            row.case,
            created.location
        );

        held.push(answered);

        // Round trip: every version the policy holds, including the one just
        // created, still reports what it was created with.
        for version in &held {
            let read = get_policy(&router, &version.id, &version.version).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected 200 from GetTrafficPolicy for version {}, got {} with body {}",
                row.case, version.version, read.status, read.body
            );
            let got = policy_of(&read, row.case);
            assert_eq!(
                &got, version,
                "case {:?}: GetTrafficPolicy must report version {} as it was created, \
                 got {got:?}; body {}",
                row.case, version.version, read.body
            );
        }

        // A new VERSION is not a new POLICY.
        let count = policy_count(&router).await;
        assert_eq!(
            count, "1",
            "case {:?}: a version create must not add a traffic policy, got Count {count:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn version_numbers_are_counted_per_policy() {
    // Two policies, each with versions of its own: the version number a create
    // hands out follows the HIGHEST version of the policy it names, so the
    // second policy's first further version is 2 however many versions the
    // first policy has.
    let router = router();
    let one = policy_with_first_version(
        &router,
        "first-policy",
        &document_with_start_endpoint("A", "192.0.2.1"),
        None,
    )
    .await;
    let two = policy_with_first_version(
        &router,
        "second-policy",
        &document_with_start_endpoint("A", "192.0.2.2"),
        None,
    )
    .await;
    assert_ne!(
        one.id, two.id,
        "two policies must not share an id, got {:?} twice",
        one.id
    );

    // Three further versions of the first policy, then one of the second.
    for expected in ["2", "3", "4"] {
        let created = create_version(
            &router,
            &one.id,
            &document_with_start_endpoint("A", "192.0.2.3"),
            None,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {expected:?}: expected 201 from CreateTrafficPolicyVersion, got {} with body {}",
            created.status, created.body
        );
        let answered = policy_of(&created, expected);
        assert_eq!(
            answered.version, expected,
            "case {expected:?}: expected Version {expected}, got {:?}",
            answered.version
        );
    }

    let created = create_version(
        &router,
        &two.id,
        &document_with_start_endpoint("A", "192.0.2.4"),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateTrafficPolicyVersion on the second policy, got {} with body {}",
        created.status, created.body
    );
    let answered = policy_of(&created, "the second policy's second version");
    assert_eq!(
        answered.version, "2",
        "the second policy's versions are counted on their own, expected Version 2, got {:?}",
        answered.version
    );
    assert_eq!(
        answered.name, two.name,
        "a version carries the name of the policy it belongs to, expected {:?}, got {:?}",
        two.name, answered.name
    );

    // Neither policy's first version moved, and no policy was added.
    for policy in [&one, &two] {
        let read = get_policy(&router, &policy.id, "1").await;
        let got = policy_of(&read, &policy.name);
        assert_eq!(
            &got, policy,
            "version 1 of {:?} must be left as it was created, got {got:?}; body {}",
            policy.name, read.body
        );
    }
    let count = policy_count(&router).await;
    assert_eq!(
        count, "2",
        "the account holds the two policies that were created, got Count {count:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_that_names_no_policy_is_refused() {
    let router = router();
    let held = policy_with_first_version(
        &router,
        "the-only-policy",
        &document_with_start_endpoint("A", "192.0.2.1"),
        Some("version one"),
    )
    .await;

    // Several forms of an id no policy carries, including one that differs
    // from the held id only in its last character.
    let mut near_miss = held.id.clone();
    near_miss.pop();
    near_miss.push(if held.id.ends_with('0') { '1' } else { '0' });
    let rows = [
        (
            "an id in the minted form that no policy carries",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("an id one character off the held one", near_miss),
        ("an id that is not in the minted form at all", "x".to_string()),
    ];

    for (case, id) in &rows {
        let reply = create_version(
            &router,
            id,
            &document_with_start_endpoint("A", "192.0.2.9"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404 for an id that names no policy, got {} with body {}",
            reply.status, reply.body
        );
        // Route 53 reports a traffic policy it does not hold by its code alone,
        // so every id above is refused with the same body: `Type` and `Code`
        // and no `Message` element.
        assert_no_such_traffic_policy(&reply, case);
    }

    // The policy that does exist still holds only the version it was created
    // with, and no policy was added.
    let read = get_policy(&router, &held.id, "1").await;
    let got = policy_of(&read, "after the refusals");
    assert_eq!(
        got, held,
        "a refused version create must leave the policy as it was, got {got:?}; body {}",
        read.body
    );
    let absent = get_policy(&router, &held.id, "2").await;
    assert_eq!(
        absent.status, 404,
        "a refused version create must persist no version, got {} with body {}",
        absent.status, absent.body
    );
    let count = policy_count(&router).await;
    assert_eq!(
        count, "1",
        "a refused version create must persist nothing, got Count {count:?}"
    );
}

#[tokio::test]
async fn a_document_the_format_does_not_define_is_refused() {
    // One row per structural rule of the published document format, the same
    // rules the policy's FIRST version is held to: the document is a JSON
    // object, it declares the format version, it names the record type it
    // creates, it defines the endpoints it can route to, and it has exactly one
    // entry point.
    let rows = [
        ("an empty document", String::new()),
        ("not JSON at all", "this is not a policy".to_string()),
        (
            "a JSON array rather than an object",
            "[\"not\",\"an\",\"object\"]".to_string(),
        ),
        (
            "an AWSPolicyFormatVersion no published format carries",
            document_with_start_endpoint("A", "192.0.2.1")
                .replace(TRAFFIC_POLICY_FORMAT_VERSION, "1999-01-01"),
        ),
        (
            "a RecordType outside the modelled RRType enum",
            document_with_start_endpoint("NOTATYPE", "192.0.2.1"),
        ),
        (
            "a StartEndpoint naming no endpoint in Endpoints",
            document_with_start_endpoint("A", "192.0.2.1").replace("\"only\",", "\"missing\","),
        ),
        (
            "a StartRule naming no rule in Rules",
            document_with_start_rule("A", "192.0.2.1", "192.0.2.2")
                .replace("\"StartRule\":\"failover_to_secondary\"", "\"StartRule\":\"missing\""),
        ),
    ];

    let router = router();
    let held = policy_with_first_version(
        &router,
        "well-formed",
        &document_with_start_endpoint("A", "192.0.2.1"),
        None,
    )
    .await;

    for (case, document) in &rows {
        let reply = create_version(&router, &held.id, document, None).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a document the format does not define, \
             got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidTrafficPolicyDocument",
            "case {case:?}: expected InvalidTrafficPolicyDocument, got body {}",
            reply.body
        );
        let absent = get_policy(&router, &held.id, "2").await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused version create must persist no version, got {} with body {}",
            absent.status, absent.body
        );
    }

    // The policy is still exactly as it was created, and a well-formed
    // document is still served afterwards — as version 2, because no refused
    // request consumed a version number.
    let read = get_policy(&router, &held.id, "1").await;
    let got = policy_of(&read, "after the refusals");
    assert_eq!(
        got, held,
        "a refused version create must leave the policy as it was, got {got:?}; body {}",
        read.body
    );
    let created = create_version(
        &router,
        &held.id,
        &document_with_start_endpoint("A", "192.0.2.50"),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected 201 for a well-formed document after the refusals, got {} with body {}",
        created.status, created.body
    );
    let answered = policy_of(&created, "the first version a refusal did not consume");
    assert_eq!(
        answered.version, "2",
        "a refused create must consume no version number, expected Version 2, got {:?}",
        answered.version
    );
}

#[tokio::test]
async fn a_comment_past_the_modelled_maximum_is_refused() {
    // The published bound of `TrafficPolicyComment` (0..1024): at the bound the
    // version is served, one character past it the request is refused and no
    // version is added.
    let router = router();
    let held = policy_with_first_version(
        &router,
        "comment-bounds",
        &document_with_start_endpoint("A", "192.0.2.1"),
        None,
    )
    .await;
    let document = document_with_start_endpoint("A", "192.0.2.2");
    let rows = [
        ("no comment, which the element being optional allows", None, 201),
        ("an empty comment, which the 0 minimum allows", Some(String::new()), 201),
        (
            "a comment at the maximum",
            Some("c".repeat(COMMENT_MAX_LEN)),
            201,
        ),
        (
            "a comment one character past the maximum",
            Some("c".repeat(COMMENT_MAX_LEN + 1)),
            400,
        ),
    ];

    let mut highest = 1u32;
    for (case, comment, status) in &rows {
        let reply = create_version(&router, &held.id, &document, comment.as_deref()).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status} from CreateTrafficPolicyVersion, \
             got {} with body {}",
            reply.status, reply.body
        );
        if *status == 201 {
            highest += 1;
            let answered = policy_of(&reply, case);
            assert_eq!(
                answered.version,
                highest.to_string(),
                "case {case:?}: expected Version {highest}, got {:?}",
                answered.version
            );
        }
        // Whatever the outcome, the policy holds exactly the versions that
        // were served.
        let absent = get_policy(&router, &held.id, &(highest + 1).to_string()).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: the policy must hold no version past {highest}, \
             got {} with body {}",
            absent.status, absent.body
        );
    }
}

#[tokio::test]
async fn a_policy_at_its_last_version_number_is_refused() {
    // The documented per-policy ceiling, which is also the highest number
    // `TrafficPolicyVersion` allows: versions up to it are served, and the
    // create that would go past it is refused.
    let router = router();
    let held = policy_with_first_version(
        &router,
        "filled-up",
        &document_with_start_endpoint("A", "192.0.2.1"),
        None,
    )
    .await;
    let document = document_with_start_endpoint("A", "192.0.2.2");

    for version in 2..=MAX_TRAFFIC_POLICY_VERSION {
        let reply = create_version(&router, &held.id, &document, None).await;
        assert_eq!(
            reply.status, 201,
            "version {version}: expected 201 while the policy is below its ceiling, \
             got {} with body {}",
            reply.status, reply.body
        );
    }

    let refused = create_version(&router, &held.id, &document, None).await;
    assert_eq!(
        refused.status, 400,
        "expected HTTP 400 past the per-policy version ceiling, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TooManyTrafficPolicyVersionsForCurrentPolicy",
        "expected TooManyTrafficPolicyVersionsForCurrentPolicy, got body {}",
        refused.body
    );
    assert_eq!(
        refused.first("Error/Message").map(|message| message
            .contains(&MAX_TRAFFIC_POLICY_VERSION.to_string())),
        Some(true),
        "the refusal must report the limit it enforces; body {}",
        refused.body
    );

    // The versions the policy holds are untouched: the last one it reached
    // still reads, and the one past it was never created.
    let last = get_policy(&router, &held.id, &MAX_TRAFFIC_POLICY_VERSION.to_string()).await;
    assert_eq!(
        last.status, 200,
        "the policy's last version must still read, got {} with body {}",
        last.status, last.body
    );
    let first_read = get_policy(&router, &held.id, "1").await;
    let got = policy_of(&first_read, "after the ceiling refusal");
    assert_eq!(
        got, held,
        "a refused version create must leave the policy's versions as they were, \
         got {got:?}; body {}",
        first_read.body
    );
}
