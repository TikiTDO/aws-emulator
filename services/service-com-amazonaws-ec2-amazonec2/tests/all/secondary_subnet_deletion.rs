// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteSecondarySubnet`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteSecondarySubnet`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test: the delete acts on the secondary subnet the request NAMES and reports
//! that subnet as the store then holds it — the id is read for its FORM before anything is
//! looked up, a subnet that still contains a secondary interface is refused, and a subnet the
//! delete accepts is handed to the clock in the non-terminal `delete-in-progress` state with
//! its IPv4 CIDR block associations `disassociating`, reaching `delete-complete` with them
//! `disassociated` one transition later without anybody asking. Every table row is a distinct
//! input along what that generalises over — how many CIDR associations the subnet carries,
//! which state it was in, how many tags it holds, whether the request carries a `ClientToken`
//! or a `DryRun`, and which way an id departs from the issued form — so no single recorded
//! request is the specification.
//!
//! `CreateSecondarySubnet` is still a scaffold, so the subnets here are written through the
//! state writer every operation that stores one goes through
//! (`ComAmazonawsEc2Amazonec2State::insert_secondary_subnet`) and read back through
//! `DescribeSecondarySubnets`, the implemented reader of this resource.

use std::time::{Duration, Instant, SystemTime};

use ec2::ops::types::Tag;
use ec2::state::{
    ATTACHMENT_STATUS_ATTACHED, ATTACHMENT_STATUS_DETACHED, ComAmazonawsEc2Amazonec2State as Ec2State,
    DEFAULT_SOURCE_DEST_CHECK, MAX_CLIENT_TOKEN_CHARS, SECONDARY_NETWORK_TYPE_RDMA,
    SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED, SECONDARY_SUBNET_ID_PREFIX,
    SECONDARY_SUBNET_STATE_CREATE_COMPLETE, SecondaryInterfaceAttachmentSpec,
    SecondaryInterfaceSpec, SecondarySubnetCidrSpec, SecondarySubnetSpec, TRANSITION_DELAY,
    availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::secondary_subnets::create_network;

/// The Region every call here is signed for, and the Region the seeded subnets' ARNs name.
const REGION: &str = "us-east-1";

/// The state a subnet a delete has started taking down reports, and the state of its IPv4
/// CIDR block associations while it does: the `SecondarySubnetState` and
/// `SecondarySubnetCidrBlockAssociationState` members
/// (`smithy_query(query="shape",shape="SecondarySubnetState")`) written out here rather than
/// imported, so a change to the service's own constants fails this suite instead of following
/// it.
const DELETE_IN_PROGRESS: &str = "delete-in-progress";
const DISASSOCIATING: &str = "disassociating";

/// The same pair once the deletion has finished.
const DELETE_COMPLETE: &str = "delete-complete";
const DISASSOCIATED: &str = "disassociated";

/// The code a value outside the form a `SecondarySubnetId` is issued in is refused with,
/// before any lookup — the recorded response's own code, as is the not-found code below.
const CODE_MALFORMED: &str = "InvalidSecondarySubnetId.Malformed";

/// The code a well-formed id the account holds no secondary subnet for is refused with.
const CODE_NOT_FOUND: &str = "InvalidSecondarySubnetID.NotFound";

/// The code a delete of a subnet something still holds a place in is refused with.
const CODE_DEPENDENCY_VIOLATION: &str = "DependencyViolation";

/// One form parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `DeleteSecondarySubnet` call carrying `params`, as its HTTP status and XML body.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DeleteSecondarySubnet", params).await
}

/// One `DescribeSecondarySubnets` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSecondarySubnets", params).await
}

/// Wait out one clock-owed transition, with a margin, so a move that was due is due.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// The state of the ONE secondary subnet a response carries, and the states of its IPv4 CIDR
/// block associations, read from the tree so the subnet's own `<state>` is never confused
/// with an association's.
fn reported_states(case: &str, body: &str) -> (String, Vec<String>) {
    let document = super::wire::document(case, body);
    let subnet = document
        .descendants()
        .find(|node| {
            node.is_element() && super::wire::child(*node, "secondarySubnetId").is_some()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no secondary subnet (<secondarySubnetId>) in {body}")
        });
    let state = super::wire::child_text(subnet, "state").unwrap_or_default();
    let associations = super::wire::set_items(subnet, "ipv4CidrBlockAssociationSet")
        .into_iter()
        .map(|item| super::wire::child_text(item, "state").unwrap_or_default())
        .collect();
    (state, associations)
}

/// A tag, as the seeded secondary subnets carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// One IPv4 CIDR block association of a seeded subnet, in the state a subnet nobody has
/// deleted carries.
fn cidr(cidr_block: &str) -> SecondarySubnetCidrSpec {
    SecondarySubnetCidrSpec {
        cidr_block: cidr_block.to_string(),
        state: SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED.to_string(),
        state_reason: None,
    }
}

/// What one seeded secondary subnet is written with, beyond the ids the store mints itself.
struct SubnetFixture {
    /// The subnet's state before the delete, as the model's `SecondarySubnetState` spells it.
    state: &'static str,
    /// The reason for that state, for the states that carry one.
    state_reason: Option<&'static str>,
    /// The IPv4 CIDR blocks associated with the subnet — none, one or several.
    ipv4_cidr_blocks: Vec<SecondarySubnetCidrSpec>,
    tags: Vec<Tag>,
    /// The `ClientToken` the delete of this subnet carries, if any.
    client_token: Option<&'static str>,
}

/// Write one secondary subnet — and the secondary network it belongs to — into the engine's
/// store the way an operation creating one would, and answer the id it was given. `ordinal`
/// keeps the networks' CIDR blocks and the subnets' zones apart.
fn seed_subnet(engine: &engine::InMemoryEngine, ordinal: u8, fixture: &SubnetFixture) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let network_id = create_network(&mut guard, ordinal);
    let zones = availability_zones_in_region(REGION);
    let zone = &zones[usize::from(ordinal) % zones.len()];
    guard
        .insert_secondary_subnet(SecondarySubnetSpec {
            secondary_network_id: network_id,
            secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
            availability_zone: zone.zone_name.clone(),
            availability_zone_id: zone.zone_id.clone(),
            region: REGION.to_string(),
            ipv4_cidr_blocks: fixture.ipv4_cidr_blocks.clone(),
            state: fixture.state.to_string(),
            state_reason: fixture.state_reason.map(str::to_string),
            tags: fixture.tags.clone(),
        })
        .expect("the seeded store is far below its secondary subnet ceiling")
}

/// A plain subnet nobody has touched: one CIDR association, no tags, no token.
fn plain_fixture() -> SubnetFixture {
    SubnetFixture {
        state: SECONDARY_SUBNET_STATE_CREATE_COMPLETE,
        state_reason: None,
        ipv4_cidr_blocks: vec![cidr("10.40.0.0/24")],
        tags: Vec::new(),
        client_token: None,
    }
}

/// The rows the delete table runs. They differ along everything the rule generalises over:
/// how many IPv4 CIDR associations the subnet carries (none, one, several), which state it
/// was in before the delete, how many tags it holds, and whether the delete carries a
/// `ClientToken`.
fn delete_fixtures() -> Vec<SubnetFixture> {
    vec![
        SubnetFixture {
            state: SECONDARY_SUBNET_STATE_CREATE_COMPLETE,
            state_reason: None,
            ipv4_cidr_blocks: vec![cidr("10.10.0.0/24")],
            tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
            client_token: Some("delete-token"),
        },
        SubnetFixture {
            state: SECONDARY_SUBNET_STATE_CREATE_COMPLETE,
            state_reason: None,
            // Several associations on one subnet: every one of them is let go of.
            ipv4_cidr_blocks: vec![cidr("10.11.0.0/24"), cidr("10.11.1.0/28")],
            tags: Vec::new(),
            client_token: None,
        },
        SubnetFixture {
            // A subnet whose creation failed carries no CIDR association at all, so the
            // delete has none to let go of.
            state: "create-failed",
            state_reason: Some("The secondary network had no capacity in the zone"),
            ipv4_cidr_blocks: Vec::new(),
            tags: vec![tag("Project", "atlas")],
            client_token: Some("x"),
        },
        SubnetFixture {
            state: SECONDARY_SUBNET_STATE_CREATE_COMPLETE,
            state_reason: None,
            ipv4_cidr_blocks: vec![cidr("10.13.0.0/12")],
            tags: vec![tag("Name", "batch")],
            client_token: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// What the delete does
// ---------------------------------------------------------------------------

/// A delete reports the subnet it names as the store then holds it — `delete-in-progress`
/// with every IPv4 CIDR block association `disassociating`, every other member the one it was
/// stored with — the clock moves both states on with nobody asking, the subnet stays
/// reportable in them, and a subnet nobody deleted is untouched.
#[tokio::test]
async fn deleting_a_secondary_subnet_hands_it_to_the_clock_and_the_store_keeps_reporting_it() {
    for (ordinal, fixture) in delete_fixtures().into_iter().enumerate() {
        let case = format!(
            "{} association(s) in {}, {} tag(s), token {:?}",
            fixture.ipv4_cidr_blocks.len(),
            fixture.state,
            fixture.tags.len(),
            fixture.client_token
        );
        let (router, engine) = super::ec2_query::emulator();
        let ordinal = u8::try_from(ordinal).unwrap_or_default();
        let id = seed_subnet(&engine, ordinal, &fixture);
        // A second subnet nobody asks to delete, so the delete's reach is visible.
        let untouched = seed_subnet(&engine, ordinal + 100, &plain_fixture());
        let by_id = indexed("SecondarySubnetId", &[&id]);
        let (status, before) = describe(&router, &by_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read before, got {status} \
             with body {before}"
        );

        let mut params = vec![p("SecondarySubnetId", id.clone())];
        if let Some(token) = fixture.client_token {
            params.push(p("ClientToken", token));
        }
        let (status, deleted) = delete(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} with body {deleted}"
        );
        let (state, associations) = reported_states(&case, &deleted);
        assert_eq!(
            state, DELETE_IN_PROGRESS,
            "case {case:?}: expected the state of a subnet being deleted, got {deleted}"
        );
        assert_eq!(
            associations,
            vec![DISASSOCIATING.to_string(); fixture.ipv4_cidr_blocks.len()],
            "case {case:?}: every association of a subnet being deleted is let go of, \
             got {deleted}"
        );
        // A delete REPORTS the subnet it deleted; it mints nothing, so every member the
        // service owns is the one the store already held.
        for member in [
            "secondarySubnetId",
            "secondarySubnetArn",
            "secondaryNetworkId",
            "secondaryNetworkType",
            "ownerId",
            "availabilityZone",
            "availabilityZoneId",
            "stateReason",
        ] {
            assert_eq!(
                element(&deleted, member),
                element(&before, member),
                "case {case:?}: <{member}> must be the stored subnet's, got {deleted}"
            );
        }
        match fixture.client_token {
            Some(token) => assert_eq!(
                element(&deleted, "clientToken").as_deref(),
                Some(token),
                "case {case:?}: the token the request carried is echoed, got {deleted}"
            ),
            None => assert!(
                elements(&deleted, "clientToken").is_empty(),
                "case {case:?}: the token is reported only if the request carried one, \
                 got {deleted}"
            ),
        }

        // The same subnet, read back: the deletion is in the store, not just in the answer.
        let (status, read) = describe(&router, &by_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read back, got {status} with body {read}"
        );
        let (state, associations) = reported_states(&case, &read);
        assert_eq!(
            (state.as_str(), associations.as_slice()),
            (
                DELETE_IN_PROGRESS,
                vec![DISASSOCIATING.to_string(); fixture.ipv4_cidr_blocks.len()].as_slice()
            ),
            "case {case:?}: a caller acting straight after the delete sees the transient \
             states, got {read}"
        );

        past_the_transition().await;

        let (status, after) = describe(&router, &by_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 after the transition, got {status} \
             with body {after}"
        );
        let (state, associations) = reported_states(&case, &after);
        assert_eq!(
            (state.as_str(), associations.as_slice()),
            (
                DELETE_COMPLETE,
                vec![DISASSOCIATED.to_string(); fixture.ipv4_cidr_blocks.len()].as_slice()
            ),
            "case {case:?}: the clock owes the subnet both moves, got {after}"
        );
        // Reading it twice reports the same thing: a read writes nothing a later caller
        // would not have seen anyway.
        let (_, again) = describe(&router, &by_id).await;
        assert_eq!(
            reported_states(&case, &again),
            reported_states(&case, &after),
            "case {case:?}: a read must not change what a later caller sees: {after} \
             then {again}"
        );
        // The record is still the one that was stored, tags and all.
        for stored in &fixture.tags {
            let key = stored.key.clone().unwrap_or_default();
            let value = stored.value.clone().unwrap_or_default();
            assert!(
                after.contains(&format!("<key>{key}</key>"))
                    && after.contains(&format!("<value>{value}</value>")),
                "case {case:?}: a deleted subnet still reports the tag {key}={value}, \
                 got {after}"
            );
        }

        // The subnet nobody deleted is in neither delete state.
        let (_, kept) = describe(&router, &indexed("SecondarySubnetId", &[&untouched])).await;
        let (state, associations) = reported_states(&case, &kept);
        assert_eq!(
            (state.as_str(), associations.as_slice()),
            (
                SECONDARY_SUBNET_STATE_CREATE_COMPLETE,
                vec![SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED.to_string()].as_slice()
            ),
            "case {case:?}: a subnet nobody deleted is untouched, got {kept}"
        );

        // The delete states are values of the documented `state` filter, so they select the
        // deleted subnet and only it.
        let (_, selected) = describe(&router, &filters(&[("state", &[DELETE_COMPLETE])])).await;
        assert_eq!(
            elements(&selected, "secondarySubnetId"),
            vec![id.clone()],
            "case {case:?}: state={DELETE_COMPLETE} selects the deleted subnet, got {selected}"
        );
    }
}

/// A repeat of a delete reports the same deletion and does NOT restart its clock: a repeat
/// made after the move fell due still answers `delete-complete`.
#[tokio::test]
async fn repeating_a_delete_reports_the_same_deletion_without_restarting_its_clock() {
    let (router, engine) = super::ec2_query::emulator();
    let id = seed_subnet(&engine, 0, &plain_fixture());
    let params = vec![p("SecondarySubnetId", id.clone())];

    let (status, first) = delete(&router, &params).await;
    assert_eq!(
        status, 200,
        "first delete: expected HTTP 200, got {status} with body {first}"
    );
    assert_eq!(
        reported_states("first delete", &first),
        (
            DELETE_IN_PROGRESS.to_string(),
            vec![DISASSOCIATING.to_string()]
        ),
        "first delete: expected the states of a subnet being deleted, got {first}"
    );

    let (status, immediate_repeat) = delete(&router, &params).await;
    assert_eq!(
        status, 200,
        "immediate repeat: expected HTTP 200, got {status} with body {immediate_repeat}"
    );
    assert_eq!(
        reported_states("immediate repeat", &immediate_repeat),
        reported_states("first delete", &first),
        "immediate repeat: expected the deletion already under way, got {immediate_repeat}"
    );
    assert_eq!(
        element(&immediate_repeat, "secondarySubnetId").as_deref(),
        Some(id.as_str()),
        "immediate repeat: expected the same subnet, got {immediate_repeat}"
    );

    past_the_transition().await;

    // The repeat AFTER the move fell due is what shows the clock was never restarted: a
    // restarted deletion would answer `delete-in-progress` again.
    let (status, late_repeat) = delete(&router, &params).await;
    assert_eq!(
        status, 200,
        "late repeat: expected HTTP 200, got {status} with body {late_repeat}"
    );
    assert_eq!(
        reported_states("late repeat", &late_repeat),
        (
            DELETE_COMPLETE.to_string(),
            vec![DISASSOCIATED.to_string()]
        ),
        "late repeat: a repeat must not restart the deletion's clock, got {late_repeat}"
    );
}

// ---------------------------------------------------------------------------
// What the delete refuses
// ---------------------------------------------------------------------------

/// The ids the refusal table runs, with the code each is answered with: every way a value can
/// fall outside the form this service issues a `SecondarySubnetId` in — `<prefix>-<17
/// lowercase hex digits>`, leading `0`, not all `0` — and then the one way it can be inside
/// that form and still resolve to nothing. The all-zero suffix is the row the recorded corpus
/// probes; the others vary the prefix, the suffix length, its alphabet, its case and its
/// leading digit.
fn refusal_rows() -> Vec<(String, &'static str)> {
    vec![
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-00000000000000000"),
            CODE_MALFORMED,
        ),
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-10000000000000000"),
            CODE_MALFORMED,
        ),
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-0abc1234"),
            CODE_MALFORMED,
        ),
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-0123456789abcdefg"),
            CODE_MALFORMED,
        ),
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-0123456789ABCDEF0"),
            CODE_MALFORMED,
        ),
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-0123456789abcdef00"),
            CODE_MALFORMED,
        ),
        (
            SECONDARY_SUBNET_ID_PREFIX.to_uppercase() + "-0123456789abcdef0",
            CODE_MALFORMED,
        ),
        ("subnet-0123456789abcdef0".to_string(), CODE_MALFORMED),
        ("not-a-secondary-subnet-id".to_string(), CODE_MALFORMED),
        (String::new(), CODE_MALFORMED),
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-0123456789abcdef0"),
            CODE_NOT_FOUND,
        ),
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-0ffffffffffffffff"),
            CODE_NOT_FOUND,
        ),
    ]
}

/// An id this service cannot resolve is refused with EC2's own code for the question it could
/// not answer, and nothing is deleted: something that is not a secondary subnet id at all is
/// refused as malformed BEFORE any lookup, while a well-formed id the account holds no subnet
/// for is the kind's own not-found code. The malformed refusal names the value the request
/// carried, in the sentence EC2 words this error class with.
#[tokio::test]
async fn an_id_this_service_cannot_resolve_is_refused_and_deletes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let id = seed_subnet(&engine, 0, &plain_fixture());

    for (value, expected_code) in refusal_rows() {
        let (status, body) = delete(&router, &[p("SecondarySubnetId", value.clone())]).await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {value:?}: expected {expected_code}, got {body}"
        );
        if expected_code == CODE_MALFORMED && !value.is_empty() {
            assert_eq!(
                error_message(&body),
                format!("The secondary-subnet ID '{value}' is malformed"),
                "case {value:?}: expected the sentence EC2 refuses a malformed id with, \
                 got {body}"
            );
        }
        assert!(
            elements(&body, "secondarySubnetId").is_empty(),
            "case {value:?}: a refused delete reports no subnet, got {body}"
        );
    }

    // The subnet the account really holds is untouched by any of the refusals.
    let (_, read) = describe(&router, &indexed("SecondarySubnetId", &[&id])).await;
    assert_eq!(
        reported_states("after the refusals", &read),
        (
            SECONDARY_SUBNET_STATE_CREATE_COMPLETE.to_string(),
            vec![SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED.to_string()]
        ),
        "after the refusals: the stored subnet is untouched, got {read}"
    );
}

/// What one secondary interface seeded into a subnet is attached to, if anything: the
/// `AttachmentStatus` values (`smithy_query(query="shape",shape="AttachmentStatus")`) an
/// interface an instance holds carries, and `None` for one attached to nothing.
fn interface_attachment_rows() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("an interface an instance holds", Some(ATTACHMENT_STATUS_ATTACHED)),
        (
            "an interface whose attachment has been taken away",
            Some(ATTACHMENT_STATUS_DETACHED),
        ),
        ("an interface attached to nothing", None),
    ]
}

/// "A secondary subnet must not contain any secondary interfaces prior to deletion": the
/// delete is refused while the subnet still holds one, whatever that interface is attached to,
/// and the subnet is left exactly as it was — while a subnet of the same account that holds
/// none is deleted in the same call sequence, so the refusal is a judgement about the
/// DEPENDENCY and not about the operation.
#[tokio::test]
async fn a_subnet_that_still_contains_a_secondary_interface_cannot_be_deleted() {
    for (case, attachment_status) in interface_attachment_rows() {
        let (router, engine) = super::ec2_query::emulator();
        let held = seed_subnet(&engine, 0, &plain_fixture());
        let free = seed_subnet(&engine, 1, &plain_fixture());
        {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let zone = availability_zones_in_region(REGION)[0].clone();
            let secondary_network_id = guard
                .secondary_subnets
                .get(&held)
                .and_then(|subnet| subnet.secondary_network_id.clone())
                .unwrap_or_default();
            guard
                .insert_secondary_interface(SecondaryInterfaceSpec {
                    secondary_subnet_id: held.clone(),
                    secondary_network_id,
                    secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
                    availability_zone: zone.zone_name.clone(),
                    availability_zone_id: zone.zone_id.clone(),
                    region: REGION.to_string(),
                    private_ipv4_addresses: vec!["10.40.0.10".to_string()],
                    source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
                    attachment: attachment_status.map(|status| SecondaryInterfaceAttachmentSpec {
                        instance_id: "i-0123456789abcdef0".to_string(),
                        device_index: 1,
                        network_card_index: 0,
                        delete_on_termination: true,
                        status: status.to_string(),
                        attach_time: SystemTime::now(),
                    }),
                    tags: Vec::new(),
                })
                .expect("the seeded store is far below its secondary interface ceiling");
        }

        let (status, body) = delete(&router, &[p("SecondarySubnetId", held.clone())]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            CODE_DEPENDENCY_VIOLATION,
            "case {case:?}: expected {CODE_DEPENDENCY_VIOLATION}, got {body}"
        );
        assert!(
            error_message(&body).contains(&held),
            "case {case:?}: expected the refusal to name the subnet it refused, got {}",
            error_message(&body)
        );

        let (_, read) = describe(&router, &indexed("SecondarySubnetId", &[&held])).await;
        assert_eq!(
            reported_states(case, &read),
            (
                SECONDARY_SUBNET_STATE_CREATE_COMPLETE.to_string(),
                vec![SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED.to_string()]
            ),
            "case {case:?}: a refused delete leaves the subnet exactly as it was, got {read}"
        );

        // The subnet that holds no interface deletes, in the same account and the same call
        // sequence.
        let (status, deleted) = delete(&router, &[p("SecondarySubnetId", free.clone())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the subnet holding nothing must delete, got {status} \
             with body {deleted}"
        );
        assert_eq!(
            reported_states(case, &deleted),
            (
                DELETE_IN_PROGRESS.to_string(),
                vec![DISASSOCIATING.to_string()]
            ),
            "case {case:?}: expected the states of a subnet being deleted, got {deleted}"
        );
    }
}

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and deletes
/// nothing; a dry run of a request that WOULD have been refused is refused with that reason
/// instead.
#[tokio::test]
async fn a_dry_run_delete_reports_the_permitted_outcome_without_deleting() {
    let (router, engine) = super::ec2_query::emulator();
    let id = seed_subnet(&engine, 0, &plain_fixture());

    let (status, body) = delete(
        &router,
        &[p("SecondarySubnetId", id.clone()), p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );

    let (_, read) = describe(&router, &indexed("SecondarySubnetId", &[&id])).await;
    assert_eq!(
        reported_states("DryRun=true", &read),
        (
            SECONDARY_SUBNET_STATE_CREATE_COMPLETE.to_string(),
            vec![SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED.to_string()]
        ),
        "DryRun=true: a dry run deletes nothing, got {read}"
    );

    // A dry run naming a subnet the account does not hold is refused with that reason, and
    // one naming something that is not a secondary subnet id at all with the malformed reason
    // the lookup never gets to.
    for (value, expected_code) in [
        (
            format!("{SECONDARY_SUBNET_ID_PREFIX}-0123456789abcdef0"),
            CODE_NOT_FOUND,
        ),
        ("not-a-secondary-subnet-id".to_string(), CODE_MALFORMED),
    ] {
        let (status, body) = delete(
            &router,
            &[p("SecondarySubnetId", value.clone()), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 400,
            "DryRun=true on {value:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "DryRun=true on {value:?}: expected {expected_code}, got {body}"
        );
    }
}

/// A `ClientToken` past the published limit is refused, and deletes nothing — while one AT
/// the limit is served, so the bound itself is what is read.
#[tokio::test]
async fn a_client_token_at_the_published_limit_is_served_and_one_past_it_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let refused = seed_subnet(&engine, 0, &plain_fixture());
    let served = seed_subnet(&engine, 1, &plain_fixture());

    let too_long = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let (status, body) = delete(
        &router,
        &[
            p("SecondarySubnetId", refused.clone()),
            p("ClientToken", too_long.clone()),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a token of {} characters: expected HTTP 400, got {status} with body {body}",
        too_long.chars().count()
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a token of {} characters: expected InvalidParameterValue, got {body}",
        too_long.chars().count()
    );
    let (_, read) = describe(&router, &indexed("SecondarySubnetId", &[&refused])).await;
    assert_eq!(
        reported_states("a refused token", &read),
        (
            SECONDARY_SUBNET_STATE_CREATE_COMPLETE.to_string(),
            vec![SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED.to_string()]
        ),
        "a refused token: nothing is deleted, got {read}"
    );

    let at_limit = "t".repeat(MAX_CLIENT_TOKEN_CHARS);
    let (status, body) = delete(
        &router,
        &[
            p("SecondarySubnetId", served.clone()),
            p("ClientToken", at_limit.clone()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "a token of {} characters: expected HTTP 200, got {status} with body {body}",
        at_limit.chars().count()
    );
    assert_eq!(
        element(&body, "clientToken").as_deref(),
        Some(at_limit.as_str()),
        "a token at the limit is echoed, got {body}"
    );
}

/// Every id this store mints for a secondary subnet is one the service's own reader of the
/// form accepts — so the delete can never refuse an id it issued as malformed. The reader is
/// exercised through the WIRE here: a minted id is deleted, which is the answer a well-formed
/// id of a stored subnet gets.
#[tokio::test]
async fn every_minted_secondary_subnet_id_is_one_the_delete_accepts() {
    let (router, engine) = super::ec2_query::emulator();
    for ordinal in 0..4u8 {
        let id = seed_subnet(&engine, ordinal, &plain_fixture());
        assert!(
            super::wire::is_long_id(SECONDARY_SUBNET_ID_PREFIX, &id),
            "minted id {id:?}: expected EC2's long resource id form"
        );
        let (status, body) = delete(&router, &[p("SecondarySubnetId", id.clone())]).await;
        assert_eq!(
            status, 200,
            "minted id {id:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "secondarySubnetId").as_deref(),
            Some(id.as_str()),
            "minted id {id:?}: expected the subnet it names, got {body}"
        );
    }
    // Nothing above slept, so every one of those deletions is still under way — the moves
    // are stored rather than slept for.
    let (_, deleting) = describe(&router, &filters(&[("state", &[DELETE_IN_PROGRESS])])).await;
    assert_eq!(
        elements(&deleting, "secondarySubnetId").len(),
        4,
        "every deleted subnet is still being deleted, got {deleting}"
    );
    // And the clock owes each of them the terminal state, whoever asks next.
    past_the_transition().await;
    let (_, complete) = describe(&router, &filters(&[("state", &[DELETE_COMPLETE])])).await;
    assert_eq!(
        elements(&complete, "secondarySubnetId").len(),
        4,
        "the clock owes every deletion its terminal state, got {complete}"
    );
}

/// The store's own view of one deletion: the due instant is RECORDED by the delete and taken
/// away once applied, so nothing sleeps and no subnet carries a move that has been made.
#[tokio::test]
async fn the_deletion_is_recorded_as_a_due_instant_and_applied_exactly_once() {
    let (router, engine) = super::ec2_query::emulator();
    let id = seed_subnet(&engine, 0, &plain_fixture());

    let before = Instant::now();
    let (status, body) = delete(&router, &[p("SecondarySubnetId", id.clone())]).await;
    assert_eq!(
        status, 200,
        "the recorded deletion: expected HTTP 200, got {status} with body {body}"
    );
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let due = guard
            .secondary_subnet_delete_complete_at
            .get(&id)
            .copied()
            .unwrap_or_else(|| panic!("the delete must record when the deletion falls due"));
        assert!(
            due >= before + TRANSITION_DELAY && due <= Instant::now() + TRANSITION_DELAY,
            "the recorded deletion: the due instant must be one transition away"
        );
    }

    past_the_transition().await;
    let (_, read) = describe(&router, &indexed("SecondarySubnetId", &[&id])).await;
    assert_eq!(
        reported_states("the recorded deletion", &read),
        (
            DELETE_COMPLETE.to_string(),
            vec![DISASSOCIATED.to_string()]
        ),
        "the recorded deletion: expected the deleted subnet, got {read}"
    );
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    assert!(
        !guard
            .secondary_subnet_delete_complete_at
            .contains_key(&id),
        "an applied move leaves nothing owed behind"
    );
    assert!(
        guard.secondary_subnets.contains_key(&id),
        "a subnet in either delete state is still stored, so it is still reported"
    );
}
