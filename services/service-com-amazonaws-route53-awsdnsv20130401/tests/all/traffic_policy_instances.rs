// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Route 53 traffic policy INSTANCE slice:
//! `CreateTrafficPolicyInstance` and the `GetTrafficPolicyInstance` read that
//! has to report what the create persisted.
//!
//! The rules under test:
//!
//! 1. `CreateTrafficPolicyInstance` applies ONE VERSION of a traffic policy to a
//!    `(hosted zone, DNS name, TTL)` and answers with the instance it
//!    persisted: a freshly minted `Id`, the hosted zone's bare id, the name in
//!    its canonical form, the `TTL` the request carried, the `TrafficPolicyId`
//!    and `TrafficPolicyVersion` it named, and a `TrafficPolicyType` READ OFF
//!    that policy version's document — the request carries no type at all.
//!    `GetTrafficPolicyInstance` reports exactly the same instance afterwards,
//!    so the table spans the record type, the document's entry-point form, the
//!    policy version, both published `TTL` bounds, and the spelling and depth
//!    of the DNS name.
//! 2. The operation CREATES the resource record sets the policy generates, so
//!    the hosted zone gains one record set at the instance's name and type
//!    carrying the instance's id, which `ListResourceRecordSets` reports.
//! 3. The instance is `Creating` until those record sets are in place and
//!    `Applied` from then on — the transition the create owes, which the read is
//!    documented to confirm — and reading it neither moves that state nor
//!    changes what a later read sees.
//! 4. A request naming a hosted zone, a traffic policy or a policy VERSION the
//!    account does not hold, or a DNS name the zone does not own, is refused and
//!    persists nothing; a second instance over the same zone, name and type is
//!    the modelled `TrafficPolicyInstanceAlreadyExists`; an `Id` that names no
//!    instance is the modelled `NoSuchTrafficPolicyInstance`.
//! 5. The account's traffic-policy-instance count follows the instances it
//!    holds — not the reads of them.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the hosted-zone and traffic-policy suites use, so what is asserted is
//! the HTTP/restXml behaviour a Route 53 client sees.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    ACCOUNT_LIMIT_MAX_TRAFFIC_POLICY_INSTANCES, TRAFFIC_POLICY_INSTANCE_APPLIED,
    TRAFFIC_POLICY_INSTANCE_CREATING, TRAFFIC_POLICY_INSTANCE_PROPAGATION,
};

use crate::hosted_zone_records::{
    call, change_batch_body, change_element, create_zone_ok, router, Reply, RecordRow, ENDPOINT,
};
use crate::traffic_policies::{
    create_policy, document_with_start_endpoint, document_with_start_rule, policy_of,
};
use crate::traffic_policy_not_found_envelope::{
    assert_identity_only, assert_no_such_traffic_policy,
};
use crate::traffic_policy_versions::create_version;

/// The published maximum of `TTL` (`range 0..2147483647`,
/// smithy_query(query="shape",shape="TTL")).
const TTL_MAX: i64 = 2_147_483_647;
/// The other published bound of `TTL`.
const TTL_MIN: i64 = 0;

/// How much longer than [`TRAFFIC_POLICY_INSTANCE_PROPAGATION`] a test waits
/// before it expects the record sets to be in place, so the assertion does not
/// race the span itself.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// Create a traffic policy instance.
///
/// `pub(crate)` so the suite asserting the account's instance COUNT builds its
/// population through the SAME create this one does.
pub(crate) async fn create_instance(
    router: &MultiServiceRouter,
    hosted_zone_id: &str,
    name: &str,
    ttl: i64,
    policy_id: &str,
    policy_version: i32,
) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/trafficpolicyinstance",
        &format!(
            "<CreateTrafficPolicyInstanceRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <HostedZoneId>{hosted_zone_id}</HostedZoneId><Name>{name}</Name><TTL>{ttl}</TTL>\
             <TrafficPolicyId>{policy_id}</TrafficPolicyId>\
             <TrafficPolicyVersion>{policy_version}</TrafficPolicyVersion>\
             </CreateTrafficPolicyInstanceRequest>"
        ),
    )
    .await
}

/// Read one traffic policy instance back.
///
/// `pub(crate)` so the count suite confirms through the SAME read that the
/// instances it counted are the ones the account holds.
pub(crate) async fn get_instance(router: &MultiServiceRouter, id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/trafficpolicyinstance/{id}"),
        "",
    )
    .await
}

/// List a hosted zone's record sets.
async fn list_records(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        "",
    )
    .await
}

/// How many traffic policy instances the account holds, as `GetAccountLimit`
/// reports it — the count that has to agree with the instances in the store.
///
/// `pub(crate)` so the count suite asserts `GetTrafficPolicyInstanceCount`
/// against the SAME population this operation reports.
pub(crate) async fn instance_count(router: &MultiServiceRouter) -> String {
    let reply = call(
        router,
        "GET",
        &format!("/2013-04-01/accountlimit/{ACCOUNT_LIMIT_MAX_TRAFFIC_POLICY_INSTANCES}"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from GetAccountLimit, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .first("Count")
        .unwrap_or_else(|| panic!("the reply must carry Count; body {}", reply.body))
        .to_string()
}

/// One `TrafficPolicyInstance` as the wire reports it, apart from its `State`:
/// the state follows the clock, so it is asserted on its own rather than being
/// compared for equality with a recorded value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InstanceView {
    id: String,
    hosted_zone_id: String,
    name: String,
    ttl: String,
    traffic_policy_id: String,
    traffic_policy_version: String,
    traffic_policy_type: String,
}

/// The instance a reply carrying a `TrafficPolicyInstance` reports.
fn instance_of(reply: &Reply, case: &str) -> InstanceView {
    let pick = |path: &str| {
        reply.first(path).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry TrafficPolicyInstance/{path}; body {}",
                reply.body
            )
        })
    };
    InstanceView {
        id: pick("TrafficPolicyInstance/Id").to_string(),
        hosted_zone_id: pick("TrafficPolicyInstance/HostedZoneId").to_string(),
        name: pick("TrafficPolicyInstance/Name").to_string(),
        ttl: pick("TrafficPolicyInstance/TTL").to_string(),
        traffic_policy_id: pick("TrafficPolicyInstance/TrafficPolicyId").to_string(),
        traffic_policy_version: pick("TrafficPolicyInstance/TrafficPolicyVersion").to_string(),
        traffic_policy_type: pick("TrafficPolicyInstance/TrafficPolicyType").to_string(),
    }
}

/// The `State` a reply reports for the instance it carries.
fn state_of(reply: &Reply, case: &str) -> String {
    reply
        .first("TrafficPolicyInstance/State")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry TrafficPolicyInstance/State; body {}",
                reply.body
            )
        })
        .to_string()
}

/// The `Message` a reply reports for the instance it carries.
///
/// An element whose text is empty carries no text at all on the wire, so a
/// missing `Message` and an empty one are the same answer: no explanation. That
/// is what `TrafficPolicyInstance$Message` requires of an instance that did not
/// fail.
fn message_of(reply: &Reply) -> String {
    reply
        .first("TrafficPolicyInstance/Message")
        .unwrap_or_default()
        .to_string()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form Route 53 mints a traffic
/// policy instance id in, which also fits the 36 characters
/// `TrafficPolicyInstanceId` allows.
fn is_traffic_policy_instance_id(id: &str) -> bool {
    let segments: Vec<&str> = id.split('-').collect();
    segments.len() == 5
        && segments
            .iter()
            .map(|segment| segment.len())
            .eq([8, 4, 4, 4, 12])
        && id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// Create a policy and return its id, failing the test when the create was
/// refused.
async fn policy_id(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let created = create_policy(router, name, document, None).await;
    assert_eq!(
        created.status, 201,
        "creating the policy {name:?}: expected 201, got {} with body {}",
        created.status, created.body
    );
    policy_of(&created, name).id
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// One row of the create table: what the request carries, and what every read
/// of the resulting instance must report.
struct CreateRow {
    case: &'static str,
    /// The zone this row's instance is created in, as the request spells it.
    zone_spelling: ZoneSpelling,
    /// The `Name` as the request writes it.
    name: String,
    /// The canonical form of that name, which every read must report.
    expect_name: &'static str,
    ttl: i64,
    /// The document the policy VERSION this row names carries.
    document: String,
    /// The version of the shared policy this row applies.
    version: i32,
    /// The `RRType` that version's document names.
    expect_type: &'static str,
}

/// How a row addresses its hosted zone: Route 53 accepts the bare id and the
/// `/hostedzone/`-prefixed form a response handed back, and both name the same
/// zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZoneSpelling {
    Bare,
    Prefixed,
}

#[tokio::test]
async fn a_created_traffic_policy_instance_reads_back_as_it_was_created() {
    let rows = [
        CreateRow {
            case: "an A policy at a subdomain, TTL at the published minimum",
            zone_spelling: ZoneSpelling::Bare,
            name: "www.example.com".to_string(),
            expect_name: "www.example.com.",
            ttl: TTL_MIN,
            document: document_with_start_endpoint("A", "192.0.2.1"),
            version: 1,
            expect_type: "A",
        },
        CreateRow {
            case: "an AAAA policy at the zone apex, addressed by the prefixed zone id",
            zone_spelling: ZoneSpelling::Prefixed,
            name: "example.com.".to_string(),
            expect_name: "example.com.",
            ttl: 60,
            document: document_with_start_endpoint("AAAA", "2001:db8::1"),
            version: 2,
            expect_type: "AAAA",
        },
        CreateRow {
            case: "a CNAME policy whose document starts at a rule, name spelled in mixed case",
            zone_spelling: ZoneSpelling::Bare,
            name: "Shop.Example.COM".to_string(),
            expect_name: "shop.example.com.",
            ttl: 300,
            document: document_with_start_rule("CNAME", "a.example.com", "b.example.com"),
            version: 3,
            expect_type: "CNAME",
        },
        CreateRow {
            case: "a TXT policy three labels deep, TTL at the published maximum",
            zone_spelling: ZoneSpelling::Bare,
            name: "a.b.c.example.com".to_string(),
            expect_name: "a.b.c.example.com.",
            ttl: TTL_MAX,
            document: document_with_start_endpoint("TXT", "v=spf1 -all"),
            version: 4,
            expect_type: "TXT",
        },
    ];

    // Every record type the table spans is one the model's own `RRType` enum
    // names, so no row asserts a type a traffic policy instance could not carry.
    for row in &rows {
        assert!(
            R_R_TYPE_VALUES.contains(&row.expect_type),
            "case {:?}: {} is not a modelled RRType",
            row.case,
            row.expect_type
        );
    }

    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instances-read-back").await;

    // ONE policy carrying one version per row, so the table also spans the
    // policy VERSION an instance is generated from.
    let policy = policy_id(&router, "instance-policy", &rows[0].document).await;
    for row in &rows[1..] {
        let added = create_version(&router, &policy, &row.document, None).await;
        assert_eq!(
            added.status, 201,
            "case {:?}: expected 201 from CreateTrafficPolicyVersion, got {} with body {}",
            row.case, added.status, added.body
        );
    }

    let mut minted: Vec<String> = Vec::new();
    for row in &rows {
        let requested_zone = match row.zone_spelling {
            ZoneSpelling::Bare => zone.clone(),
            ZoneSpelling::Prefixed => format!("/hostedzone/{zone}"),
        };
        let created =
            create_instance(&router, &requested_zone, &row.name, row.ttl, &policy, row.version)
                .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
            row.case, created.status, created.body
        );
        let answered = instance_of(&created, row.case);
        let expected = InstanceView {
            id: answered.id.clone(),
            hosted_zone_id: zone.clone(),
            name: row.expect_name.to_string(),
            ttl: row.ttl.to_string(),
            traffic_policy_id: policy.clone(),
            traffic_policy_version: row.version.to_string(),
            traffic_policy_type: row.expect_type.to_string(),
        };
        assert_eq!(
            answered, expected,
            "case {:?}: CreateTrafficPolicyInstance must answer with the instance it \
             persisted, got {answered:?}; body {}",
            row.case, created.body
        );
        assert!(
            is_traffic_policy_instance_id(&answered.id),
            "case {:?}: expected an 8-4-4-4-12 lowercase-hex instance id, got {:?}",
            row.case,
            answered.id
        );
        assert!(
            !minted.contains(&answered.id),
            "case {:?}: every created instance must get its own id, but {:?} was minted twice",
            row.case,
            answered.id
        );
        minted.push(answered.id.clone());
        assert!(
            message_of(&created).is_empty(),
            "case {:?}: an instance that did not fail must carry no explanation, got {:?}",
            row.case,
            message_of(&created)
        );

        // The Location names the resource the read is bound to, at the endpoint
        // the request was sent to.
        let expected_location = format!(
            "https://{ENDPOINT}/2013-04-01/trafficpolicyinstance/{}",
            answered.id
        );
        assert_eq!(
            created.location.as_deref(),
            Some(expected_location.as_str()),
            "case {:?}: expected the Location {expected_location:?}, got {:?}",
            row.case,
            created.location
        );

        // Round trip: the read reports the very same instance.
        let read = get_instance(&router, &answered.id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetTrafficPolicyInstance, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            instance_of(&read, row.case),
            answered,
            "case {:?}: the read must report the instance the create persisted; body {}",
            row.case,
            read.body
        );
    }

    // The account holds exactly the instances that were created.
    assert_eq!(
        instance_count(&router).await,
        rows.len().to_string(),
        "the account must hold one traffic policy instance per create"
    );
}

// ---------------------------------------------------------------------------
// The record sets the instance generates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_instance_generates_a_record_set_that_names_it() {
    // One row per record type and TTL, so what is asserted is "a record set at
    // the instance's name and type, cached for the instance's TTL and owned by
    // the instance" rather than one type's case.
    let rows = [
        ("an A policy", "A", "www.example.com", "www.example.com.", 60i64),
        (
            "an AAAA policy at another name",
            "AAAA",
            "ipv6.example.com",
            "ipv6.example.com.",
            TTL_MIN,
        ),
        (
            "a CNAME policy deeper in the zone",
            "CNAME",
            "a.b.example.com",
            "a.b.example.com.",
            86_400,
        ),
    ];

    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instances-make-records").await;
    // The zone's own default SOA and NS records, which the generated record sets
    // are counted on top of.
    let before = list_records(&router, &zone).await.all("Name").len();

    for (index, (case, rr_type, name, canonical, ttl)) in rows.iter().enumerate() {
        let policy = policy_id(
            &router,
            &format!("record-policy-{index}"),
            &document_with_start_endpoint(rr_type, "192.0.2.9"),
        )
        .await;
        let created = create_instance(&router, &zone, name, *ttl, &policy, 1).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
            created.status, created.body
        );
        let id = instance_of(&created, case).id;

        let listed = list_records(&router, &zone).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListResourceRecordSets, got {} with body {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.all("Name").len(),
            before + index + 1,
            "case {case:?}: each instance must add exactly one record set; body {}",
            listed.body
        );
        // The record set the instance generated is the one carrying its id, and
        // it sits at the instance's own name and type.
        let owned: Vec<&str> = listed.all("ResourceRecordSet/TrafficPolicyInstanceId");
        assert!(
            owned.contains(&id.as_str()),
            "case {case:?}: the zone must hold a record set naming the instance {id:?}, \
             got {owned:?}; body {}",
            listed.body
        );
        assert!(
            listed.all("ResourceRecordSet/Name").contains(canonical),
            "case {case:?}: expected a record set at {canonical:?}; body {}",
            listed.body
        );
        assert!(
            listed.all("ResourceRecordSet/Type").contains(rr_type),
            "case {case:?}: expected a record set of type {rr_type:?}; body {}",
            listed.body
        );
        // `TrafficPolicyInstance$TTL` is "the TTL that Amazon Route 53 assigned
        // to all of the resource record sets that it created in the specified
        // hosted zone", so the generated record set is cached for exactly the
        // TTL the instance reports. Each row uses a TTL of its own, so finding
        // it here is finding THIS row's record set.
        let ttl = ttl.to_string();
        assert!(
            listed
                .all("ResourceRecordSet/TTL")
                .contains(&ttl.as_str()),
            "case {case:?}: expected the generated record set to carry the instance's TTL \
             {ttl:?}; body {}",
            listed.body
        );
    }
}

// ---------------------------------------------------------------------------
// The state the create owes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_instance_is_creating_until_its_record_sets_are_in_place() {
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-states").await;
    let policy = policy_id(
        &router,
        "state-policy",
        &document_with_start_endpoint("A", "192.0.2.1"),
    )
    .await;

    let created = create_instance(&router, &zone, "www.example.com", 60, &policy, 1).await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
        created.status, created.body
    );
    let id = instance_of(&created, "the create").id;
    assert_eq!(
        state_of(&created, "the create"),
        TRAFFIC_POLICY_INSTANCE_CREATING,
        "a create must answer with the instance still being created; body {}",
        created.body
    );

    // A read straight after the create still observes that state, and reading
    // it twice answers the same thing: a read moves nothing.
    for attempt in 0..2 {
        let read = get_instance(&router, &id).await;
        assert_eq!(
            state_of(&read, "an immediate read"),
            TRAFFIC_POLICY_INSTANCE_CREATING,
            "read {attempt}: the instance must still be being created; body {}",
            read.body
        );
    }

    tokio::time::sleep(TRAFFIC_POLICY_INSTANCE_PROPAGATION + SETTLE_MARGIN).await;
    for attempt in 0..2 {
        let read = get_instance(&router, &id).await;
        assert_eq!(
            state_of(&read, "a settled read"),
            TRAFFIC_POLICY_INSTANCE_APPLIED,
            "read {attempt}: the instance's record sets must be in place by now; body {}",
            read.body
        );
        assert!(
            message_of(&read).is_empty(),
            "read {attempt}: an instance that did not fail must carry no explanation"
        );
    }

    // Neither the reads nor the transition created or destroyed an instance.
    assert_eq!(
        instance_count(&router).await,
        "1",
        "reading an instance must not change how many the account holds"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// What one refused create names, and the refusal it must answer.
struct RefusalRow {
    case: &'static str,
    /// `None` uses the zone the test created.
    zone: Option<&'static str>,
    name: &'static str,
    /// `None` uses the policy the test created.
    policy: Option<&'static str>,
    version: i32,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_create_naming_something_the_account_does_not_hold_is_refused() {
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-refusals").await;
    let policy = policy_id(
        &router,
        "refusal-policy",
        &document_with_start_endpoint("A", "192.0.2.1"),
    )
    .await;

    let rows = [
        RefusalRow {
            case: "a hosted zone id that names no zone",
            zone: Some("Z0000000000000"),
            name: "www.example.com",
            policy: None,
            version: 1,
            status: 404,
            code: "NoSuchHostedZone",
        },
        RefusalRow {
            case: "a hosted zone id in its prefixed form that names no zone",
            zone: Some("/hostedzone/Z0000000000001"),
            name: "www.example.com",
            policy: None,
            version: 1,
            status: 404,
            code: "NoSuchHostedZone",
        },
        RefusalRow {
            case: "a traffic policy id that names no policy",
            zone: None,
            name: "www.example.com",
            policy: Some("00000000-0000-0000-0000-000000000000"),
            version: 1,
            status: 404,
            code: "NoSuchTrafficPolicy",
        },
        RefusalRow {
            case: "a version the policy does not hold",
            zone: None,
            name: "www.example.com",
            policy: None,
            version: 2,
            status: 404,
            code: "NoSuchTrafficPolicy",
        },
        RefusalRow {
            case: "a DNS name outside the hosted zone",
            zone: None,
            name: "www.example.org",
            policy: None,
            version: 1,
            status: 400,
            code: "InvalidInput",
        },
        RefusalRow {
            case: "a DNS name that merely looks like a subdomain of the zone",
            zone: None,
            name: "www.evil-example.com",
            policy: None,
            version: 1,
            status: 400,
            code: "InvalidInput",
        },
    ];

    for row in &rows {
        let reply = create_instance(
            &router,
            row.zone.unwrap_or(&zone),
            row.name,
            60,
            row.policy.unwrap_or(&policy),
            row.version,
        )
        .await;
        assert_eq!(
            reply.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {}",
            row.case, row.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            row.code,
            "case {:?}: expected the error code {:?}; body {}",
            row.case,
            row.code,
            reply.body
        );
        // A traffic policy the account does not hold is reported with the one sentence
        // that refusal states, and every other refusal here says why in its own.
        if row.code == "NoSuchTrafficPolicy" {
            assert_no_such_traffic_policy(&reply, row.case);
        } else {
            assert!(
                reply
                    .first("Error/Message")
                    .is_some_and(|message| !message.is_empty()),
                "case {:?}: the refusal must say why; body {}",
                row.case,
                reply.body
            );
        }
    }

    // Nothing was persisted by any of them.
    assert_eq!(
        instance_count(&router).await,
        "0",
        "a refused create must persist no traffic policy instance"
    );
}

#[tokio::test]
async fn a_ttl_past_the_published_maximum_is_refused() {
    // The published bounds of `TTL` (`range 0..2147483647`): at either bound the
    // instance is served, one past the maximum the request is refused before the
    // handler and persists nothing.
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-ttl-bounds").await;
    let rows = [
        ("the published minimum", TTL_MIN, 201),
        ("a TTL inside the range", 60, 201),
        ("the published maximum", TTL_MAX, 201),
        ("one second past the maximum", TTL_MAX + 1, 400),
    ];

    let mut served = 0usize;
    for (index, (case, ttl, status)) in rows.iter().enumerate() {
        // A fresh policy per row, so each row's instance has its own record set
        // to create rather than colliding with the row before it.
        let policy = policy_id(
            &router,
            &format!("ttl-policy-{index}"),
            &document_with_start_endpoint("A", "192.0.2.1"),
        )
        .await;
        let name = format!("ttl-{index}.example.com");
        let reply = create_instance(&router, &zone, &name, *ttl, &policy, 1).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status} from CreateTrafficPolicyInstance, \
             got {} with body {}",
            reply.status, reply.body
        );
        if *status == 201 {
            served += 1;
            assert_eq!(
                instance_of(&reply, case).ttl,
                ttl.to_string(),
                "case {case:?}: the instance must carry the TTL the request asked for; body {}",
                reply.body
            );
        }
        assert_eq!(
            instance_count(&router).await,
            served.to_string(),
            "case {case:?}: the account must hold exactly the instances that were served"
        );
    }
}

#[tokio::test]
async fn a_second_instance_over_the_same_name_and_type_is_refused() {
    // An instance OWNS the record set it generated, and a zone holds at most one
    // record set at a given name and type, so a second instance over the same
    // zone, name and type has no record set to create. Spanning the record type
    // and the name spelling, because the rule is about the record set's
    // IDENTITY, not about one spelling of one name.
    let rows = [
        ("the same name, spelled the same way", "A", "www.example.com"),
        ("the same name in mixed case", "AAAA", "WWW.Example.com"),
        ("the same name with the root dot", "TXT", "www.example.com."),
    ];

    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-conflicts").await;

    for (index, (case, rr_type, second_spelling)) in rows.iter().enumerate() {
        let policy = policy_id(
            &router,
            &format!("conflict-policy-{index}"),
            &document_with_start_endpoint(rr_type, "192.0.2.1"),
        )
        .await;
        let first = create_instance(&router, &zone, "www.example.com", 60, &policy, 1).await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: expected 201 for the first instance, got {} with body {}",
            first.status, first.body
        );
        let held = instance_of(&first, case);

        let second = create_instance(&router, &zone, second_spelling, 90, &policy, 1).await;
        assert_eq!(
            second.status, 409,
            "case {case:?}: expected HTTP 409 for a second instance, got {} with body {}",
            second.status, second.body
        );
        assert_eq!(
            second.error_code(),
            "TrafficPolicyInstanceAlreadyExists",
            "case {case:?}: expected TrafficPolicyInstanceAlreadyExists; body {}",
            second.body
        );

        // The instance already holding the name is left exactly as it was.
        let read = get_instance(&router, &held.id).await;
        assert_eq!(
            instance_of(&read, case),
            held,
            "case {case:?}: a refused create must leave the held instance alone; body {}",
            read.body
        );
    }

    assert_eq!(
        instance_count(&router).await,
        rows.len().to_string(),
        "a refused create must persist no further instance"
    );
}

#[tokio::test]
async fn an_instance_over_a_record_set_a_change_batch_wrote_is_refused() {
    // The collision is about the record set, so it holds whoever wrote it: a
    // record set a change batch created occupies the same (name, type) identity
    // the instance's generated record set would need.
    let rows = [
        RecordRow {
            name: "www.example.com",
            rr_type: "A",
            ttl: 60,
            values: &["192.0.2.1"],
        },
        RecordRow {
            name: "text.example.com",
            rr_type: "TXT",
            ttl: 300,
            values: &["\"hello\""],
        },
    ];

    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-record-conflicts").await;
    let changes: String = rows.iter().map(|row| change_element("CREATE", row)).collect();
    let written = call(
        &router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone}/rrset"),
        &change_batch_body(&changes, None),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "expected 200 from ChangeResourceRecordSets, got {} with body {}",
        written.status, written.body
    );

    for (index, row) in rows.iter().enumerate() {
        let policy = policy_id(
            &router,
            &format!("occupied-policy-{index}"),
            &document_with_start_endpoint(row.rr_type, "192.0.2.1"),
        )
        .await;
        let reply = create_instance(&router, &zone, row.name, 60, &policy, 1).await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected HTTP 400 over an occupied record set, got {} with body {}",
            row.name, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case {:?}: expected InvalidInput; body {}",
            row.name,
            reply.body
        );
    }

    assert_eq!(
        instance_count(&router).await,
        "0",
        "a refused create must persist no traffic policy instance"
    );
}

#[tokio::test]
async fn an_id_that_names_no_instance_is_refused() {
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-reads").await;
    let policy = policy_id(
        &router,
        "read-policy",
        &document_with_start_endpoint("A", "192.0.2.1"),
    )
    .await;
    let created = create_instance(&router, &zone, "www.example.com", 60, &policy, 1).await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
        created.status, created.body
    );
    let held = instance_of(&created, "the held instance").id;

    // One row per way an id can name no instance: a well-formed id nobody
    // minted, the id of another kind of resource, and an id that is not of the
    // minted form at all.
    let rows = [
        ("an unminted id of the right form", "00000000-0000-0000-0000-000000000000".to_string()),
        ("a traffic policy's id", policy.clone()),
        ("a hosted zone's id", zone.clone()),
        ("an id of no recognised form", "not-an-instance".to_string()),
    ];

    for (case, id) in &rows {
        let reply = get_instance(&router, id).await;
        // A traffic policy instance the account does not hold is reported by its
        // code alone: `Type` and `Code`, with no `Message` element.
        assert_identity_only(&reply, case, "NoSuchTrafficPolicyInstance");
    }

    // The instance the account does hold still reads.
    let read = get_instance(&router, &held).await;
    assert_eq!(
        read.status, 200,
        "the held instance must still read, got {} with body {}",
        read.status, read.body
    );
}
