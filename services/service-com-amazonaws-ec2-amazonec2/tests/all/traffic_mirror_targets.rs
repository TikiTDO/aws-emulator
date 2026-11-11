// SPDX-License-Identifier: CC0-1.0

//! What `DeleteTrafficMirrorTarget` removes, what it refuses and what it leaves alone — end
//! to end over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"), through the crate's
//! shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. "Deletes the specified Traffic Mirror target" (`DeleteTrafficMirrorTarget`): the
//!    target the request names is REMOVED from the store, whatever that target carries and
//!    whichever kind of target it is, and the call answers with the id of the target it
//!    deleted — so the delete is observable through the operation itself, a repeat of it
//!    naming nothing. Every other target the store holds is left exactly as it was.
//! 2. An id this account holds no Traffic Mirror target for is refused with HTTP 400
//!    `InvalidTrafficMirrorTargetId.NotFound` whatever the id looks like — the member
//!    publishes no pattern, so there is no second code for an id of the wrong shape — and
//!    the message names the id the request gave. A refusal deletes nothing. A request that
//!    carries no id at all is a different refusal: `MissingParameter`.
//! 3. "You cannot delete a Traffic Mirror target that is in use by a Traffic Mirror
//!    session" (`DeleteTrafficMirrorTarget`): while any stored session mirrors TO the
//!    target, the delete is refused with `DependencyViolation` and removes nothing —
//!    however many sessions depend on it — and once the last of them is gone the same call
//!    succeeds.
//! 4. `DryRun` asks whether the call WOULD have succeeded: over a target the store holds it
//!    is answered `DryRunOperation` (HTTP 412) with nothing deleted, over an id that names
//!    nothing it is answered with the refusal that id would have got instead, and over a
//!    target a session depends on with that dependency's refusal.
//! 5. The target store is bounded by its own ceiling, and the bytes one target's
//!    caller-chosen text holds go back to the store's byte budget when the delete removes
//!    it — the charge the insert took, given back on the same terms.
//!
//! `CreateTrafficMirrorSession` is still a scaffold stub with an issue of its own, and the
//! targets here are seeded through the store's own write path
//! (`insert_traffic_mirror_target`), which is where `CreateTrafficMirrorTarget` writes them,
//! so this suite's fixtures do not depend on that create's own refusals
//! (`traffic_mirror_target_creation` covers those).
//!
//! Each table row is a distinct input along what the rule generalises over — which kind of
//! target it is, which members it carries, how many targets the store holds, which of them
//! is deleted, how many sessions depend on it, and the shape of an unknown id — so no row
//! is the specification. The recorded unit this suite was filed from is ONE row of the
//! table in [`an_unknown_target_id_is_refused_whatever_it_looks_like`].

use ec2::ops::types::{Tag, TrafficMirrorSession, TrafficMirrorTarget};
use ec2::state::{
    traffic_mirror_target_payload_bytes, ComAmazonawsEc2Amazonec2State as Ec2State,
    MAX_TRAFFIC_MIRROR_TARGETS, TRAFFIC_MIRROR_TARGET_ID_PREFIX,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};
use super::wire::is_long_id;

/// The Region every call here is signed for. A Traffic Mirror target is named by an id that
/// is unique across this store, and the modelled shape names no Region, so the Region only
/// selects the envelope.
const REGION: &str = "us-west-2";

/// The error code EC2 answers for a Traffic Mirror target id that names no target of the
/// account, as this crate's state module defines it.
const NOT_FOUND: &str = ec2::state::INVALID_TRAFFIC_MIRROR_TARGET_ID_NOT_FOUND;

/// The shapes of an id that names no Traffic Mirror target of the account: the long 17-hex
/// form the store mints, the all-zero suffix of that form (the shape the recorded unit this
/// suite was filed from used), the short 8-hex form of EC2's older ids, a non-hex suffix, a
/// suffix-less prefix, the id of another resource kind and text that is no id at all.
///
/// The ONE table of them, so every path that resolves a target id is held to the same rule:
/// an id this service never minted names no target whatever it looks like.
const UNKNOWN_TARGET_IDS: [&str; 8] = [
    "tmt-0123456789abcdef0",
    "tmt-00000000000000000",
    "tmt-00000000",
    "tmt-not-hex",
    "tmt-",
    "tms-0123456789abcdef0",
    "vpc-0123456789abcdef0",
    "not-an-id-at-all",
];

/// One target to seed: the members a create would have set on it.
///
/// The three `kind` words are the modelled `TrafficMirrorTargetType` values
/// (`smithy_query(query="shape",shape="TrafficMirrorTargetType")`), and each kind carries
/// the one resource member that kind of target points at
/// (`smithy_query(query="shape",shape="TrafficMirrorTarget")`).
#[derive(Clone, Debug)]
struct TargetRow {
    kind: &'static str,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The modelled record one [`TargetRow`] stands for, as the create will build it: the
/// resource the target points at is derived from `n`, so each seeded target names its own.
fn target(row: &TargetRow, n: usize) -> TrafficMirrorTarget {
    let network_interface_id = (row.kind == "network-interface").then(|| format!("eni-{n:017x}"));
    let network_load_balancer_arn = (row.kind == "network-load-balancer").then(|| {
        format!(
            "arn:aws:elasticloadbalancing:{REGION}:{account}:loadbalancer/net/mirror-{n}/{n:017x}",
            account = engine::emulator_account_id(),
        )
    });
    let gateway_load_balancer_endpoint_id =
        (row.kind == "gateway-load-balancer-endpoint").then(|| format!("vpce-{n:017x}"));
    TrafficMirrorTarget {
        // Minted by the store at the insert, so an id is never chosen twice.
        traffic_mirror_target_id: None,
        network_interface_id,
        network_load_balancer_arn,
        r#type: Some(row.kind.to_string()),
        description: row.description.map(str::to_string),
        owner_id: Some(engine::emulator_account_id().to_string()),
        tags: Some(
            row.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect(),
        ),
        gateway_load_balancer_endpoint_id,
    }
}

/// Seed one target in the store and answer the id it was minted with.
fn seed(engine: &engine::InMemoryEngine, case: &str, row: &TargetRow, n: usize) -> String {
    let id = engine
        .state::<Ec2State>()
        .lock()
        .insert_traffic_mirror_target(target(row, n), None);
    let id = id.unwrap_or_else(|| {
        panic!("case {case:?}: the store must have room for the few targets this suite seeds")
    });
    assert!(
        is_long_id(TRAFFIC_MIRROR_TARGET_ID_PREFIX, &id),
        "case {case:?}: expected a minted Traffic Mirror target id, got {id:?}"
    );
    id
}

/// Seed one Traffic Mirror session that mirrors to `target_id`, and answer its id.
fn seed_session(
    engine: &engine::InMemoryEngine,
    case: &str,
    target_id: &str,
    n: usize,
) -> String {
    let session = TrafficMirrorSession {
        traffic_mirror_session_id: None,
        traffic_mirror_target_id: Some(target_id.to_string()),
        traffic_mirror_filter_id: Some(format!("tmf-{n:017x}")),
        network_interface_id: Some(format!("eni-{n:017x}")),
        owner_id: Some(engine::emulator_account_id().to_string()),
        packet_length: None,
        session_number: Some(1 + n as i32),
        virtual_network_id: None,
        description: None,
        tags: Some(Vec::new()),
    };
    engine
        .state::<Ec2State>()
        .lock()
        .insert_traffic_mirror_session(session)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the store must have room for the sessions this suite seeds")
        })
}

/// One `DeleteTrafficMirrorTarget` call naming `id`, with `extra` form fields beside it.
async fn delete(router: &MultiServiceRouter, id: &str, extra: &[(&str, &str)]) -> (u16, String) {
    let mut params = vec![("TrafficMirrorTargetId".to_string(), id.to_string())];
    for (key, value) in extra {
        params.push(((*key).to_string(), (*value).to_string()));
    }
    call(router, REGION, "DeleteTrafficMirrorTarget", &params).await
}

/// Every Traffic Mirror target id the store holds, in the store's own order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .traffic_mirror_targets
        .keys()
        .cloned()
        .collect()
}

/// The rows every table here is driven over: one target of each kind the modelled
/// `TrafficMirrorTargetType` enum names, carrying between them no caller-chosen text at
/// all, a description and one tag, and a description holding punctuation and non-ASCII text
/// beside two tags.
const TARGET_ROWS: &[(&str, TargetRow)] = &[
    (
        "a network interface target carrying only what the service derives",
        TargetRow {
            kind: "network-interface",
            description: None,
            tags: &[],
        },
    ),
    (
        "a Network Load Balancer target with a description and one tag",
        TargetRow {
            kind: "network-load-balancer",
            description: Some("web tier mirror"),
            tags: &[("Team", "platform")],
        },
    ),
    (
        "a Gateway Load Balancer endpoint target with punctuation and non-ASCII text",
        TargetRow {
            kind: "gateway-load-balancer-endpoint",
            description: Some("mirror: ingress/egress — grün"),
            tags: &[("Team", "network"), ("Env", "staging")],
        },
    ),
    (
        "a network interface target with an empty tag value",
        TargetRow {
            kind: "network-interface",
            description: Some("d"),
            tags: &[("Only", "")],
        },
    ),
];

/// RULE 1: whatever a stored target carries and whichever kind it is, the delete removes it
/// from the store, answers with its id, and leaves every other target alone — and a repeat
/// of the same call reports the id as naming nothing, so the deletion is observable through
/// this operation alone.
///
/// The rows differ along what the rule generalises over: the kind of target, which members
/// it carries, how many targets the store holds beside it, and which of them the request
/// names.
#[tokio::test]
async fn every_stored_target_is_deleted_and_reported_by_id() {
    for (case, row) in TARGET_ROWS {
        // Two neighbours are seeded around the target under test, and the row's own position
        // among them moves with the table, so "the first", "a middle one" and "the last"
        // target of the store are each deleted by some row.
        for position in 0..3 {
            let case = format!("{case}, deleted at position {position} of three");
            let (router, engine) = super::ec2_query::emulator();
            let neighbour = TargetRow {
                kind: "network-interface",
                description: None,
                tags: &[],
            };
            let seeded: Vec<String> = (0..3)
                .map(|n| {
                    let seeded_row = if n == position {
                        row.clone()
                    } else {
                        neighbour.clone()
                    };
                    seed(&engine, &case, &seeded_row, n)
                })
                .collect();
            let chosen = seeded[position].clone();

            let (status, body) = delete(&router, &chosen, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "trafficMirrorTargetId").as_deref(),
                Some(chosen.as_str()),
                "case {case:?}: expected the id of the deleted target, got {body}"
            );

            let left: Vec<String> = seeded.iter().filter(|id| **id != chosen).cloned().collect();
            assert_eq!(
                stored_ids(&engine),
                left,
                "case {case:?}: the delete must remove exactly the target it named, the store \
                 holds {:?}",
                stored_ids(&engine)
            );

            // The repeat: the id now names nothing, which is how a caller tells the deletion
            // it performed from one it did not.
            let (status, body) = delete(&router, &chosen, &[]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from the repeat, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND} from the repeat, got {body}"
            );
            assert_eq!(
                stored_ids(&engine),
                left,
                "case {case:?}: a refused repeat must delete nothing, the store holds {:?}",
                stored_ids(&engine)
            );
        }
    }
}

/// RULE 2: an id this account holds no target for is refused with
/// `InvalidTrafficMirrorTargetId.NotFound`, whatever shape the id has, and the message names
/// the id the request gave.
///
/// The rows differ along the SHAPE of the unknown id: the long 17-hex form the store mints,
/// the all-zero suffix of that form (the shape the recorded unit this suite was filed from
/// used), the short 8-hex form of EC2's older ids, a non-hex suffix, a suffix-less prefix,
/// the id of another resource kind and text that is no id at all. Each is tried against an
/// EMPTY store and against one holding a target, because "no such target" must not depend on
/// the account holding none.
#[tokio::test]
async fn an_unknown_target_id_is_refused_whatever_it_looks_like() {
    for id in UNKNOWN_TARGET_IDS {
        for seeded_targets in [0usize, 1] {
            let case = format!("{id} with {seeded_targets} target(s) stored");
            let (router, engine) = super::ec2_query::emulator();
            for n in 0..seeded_targets {
                seed(&engine, &case, &TARGET_ROWS[0].1, n);
            }
            let before = stored_ids(&engine);

            let (status, body) = delete(&router, id, &[]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an id naming no target, got {status} with \
                 body {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("{id} does not exist"),
                "case {case:?}: expected the message to name the id the request gave, got {body}"
            );
            assert_eq!(
                stored_ids(&engine),
                before,
                "case {case:?}: a refused delete must delete nothing, the store holds {:?}",
                stored_ids(&engine)
            );
        }
    }
}

/// RULE 2, the other refusal: a request whose `TrafficMirrorTargetId` carries no value did
/// not name a target, which is `MissingParameter` rather than an id that names nothing.
#[tokio::test]
async fn a_request_naming_no_target_is_refused_as_a_missing_parameter() {
    for seeded_targets in [0usize, 2] {
        let case = format!("an empty TrafficMirrorTargetId with {seeded_targets} target(s) stored");
        let (router, engine) = super::ec2_query::emulator();
        for n in 0..seeded_targets {
            seed(&engine, &case, &TARGET_ROWS[0].1, n);
        }
        let before = stored_ids(&engine);

        let (status, body) = delete(&router, "", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 without an id, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "MissingParameter",
            "case {case:?}: expected MissingParameter, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "The request must contain the parameter trafficMirrorTargetId",
            "case {case:?}: expected the message to name the parameter, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            before,
            "case {case:?}: a refused delete must delete nothing, the store holds {:?}",
            stored_ids(&engine)
        );
    }
}

/// RULE 3: a target a stored session mirrors to cannot be deleted — `DependencyViolation`,
/// nothing removed — however many sessions depend on it, and the same call succeeds once the
/// last of them is gone.
///
/// The rows differ along the kind of target and along HOW MANY sessions depend on it, and a
/// second target nothing depends on is seeded beside it, so "in use" is read off the
/// sessions rather than off the store holding any session at all.
#[tokio::test]
async fn a_target_a_session_mirrors_to_cannot_be_deleted() {
    for (case, row) in TARGET_ROWS {
        for dependents in 1..3usize {
            let case = format!("{case} with {dependents} session(s) mirroring to it");
            let (router, engine) = super::ec2_query::emulator();
            let in_use = seed(&engine, &case, row, 0);
            let free = seed(&engine, &case, &TARGET_ROWS[0].1, 1);
            let sessions: Vec<String> = (0..dependents)
                .map(|n| seed_session(&engine, &case, &in_use, n))
                .collect();
            // One session mirrors to the OTHER target, so a target is only in use by the
            // sessions that name it.
            seed_session(&engine, &case, &free, dependents);
            let before = stored_ids(&engine);

            let (status, body) = delete(&router, &in_use, &[]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a target in use, got {status} with body \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "DependencyViolation",
                "case {case:?}: expected DependencyViolation, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("The trafficMirrorTarget '{in_use}' has dependencies and cannot be deleted."),
                "case {case:?}: expected the message to name the target the request gave, got \
                 {body}"
            );
            assert_eq!(
                stored_ids(&engine),
                before,
                "case {case:?}: a refused delete must delete nothing, the store holds {:?}",
                stored_ids(&engine)
            );

            // A dry run over the same target is refused with the reason the real call would
            // have failed for, rather than with `DryRunOperation`.
            let (status, body) = delete(&router, &in_use, &[("DryRun", "true")]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a dry run over a target in use is refused with that reason, got \
                 {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "DependencyViolation",
                "case {case:?}: expected DependencyViolation from the dry run, got {body}"
            );

            // Every session that named the target is deleted in turn: the target stays
            // undeletable until the LAST of them is gone.
            for (deleted, session) in sessions.iter().enumerate() {
                let (status, body) = call(
                    &router,
                    REGION,
                    "DeleteTrafficMirrorSession",
                    &[(
                        "TrafficMirrorSessionId".to_string(),
                        session.to_string(),
                    )],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the session delete to succeed, got {status} with \
                     body {body}"
                );
                let remaining = dependents - deleted - 1;
                let (status, body) = delete(&router, &in_use, &[]).await;
                let expected = if remaining == 0 { 200 } else { 400 };
                assert_eq!(
                    status, expected,
                    "case {case:?}: with {remaining} session(s) left, expected HTTP {expected}, \
                     got {status} with body {body}"
                );
            }

            // The last dependent gone, the delete succeeded above: the target is out of the
            // store and the one nothing depended on is untouched.
            assert_eq!(
                stored_ids(&engine),
                vec![free.clone()],
                "case {case:?}: only the target nothing depends on must be left, the store holds \
                 {:?}",
                stored_ids(&engine)
            );
        }
    }
}

/// RULE 4: `DryRun` asks whether the call would have succeeded — so a dry run over a stored
/// target is answered `DryRunOperation` with nothing deleted, while a dry run over an id
/// that names nothing is answered with the refusal that id would have got. A `DryRun` the
/// request sets to `false` is not a probe at all, so it deletes.
#[tokio::test]
async fn a_dry_run_deletes_nothing() {
    for (case, row) in TARGET_ROWS {
        let (router, engine) = super::ec2_query::emulator();
        let id = seed(&engine, case, row, 0);
        let before = stored_ids(&engine);

        let (status, body) = delete(&router, &id, &[("DryRun", "true")]).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 for a dry run, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            before,
            "case {case:?}: a dry run must delete nothing, the store holds {:?}",
            stored_ids(&engine)
        );

        // A dry run that would have been REFUSED is refused with that reason instead.
        let (status, body) = delete(&router, "tmt-0123456789abcdef0", &[("DryRun", "true")]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a dry run over an unknown id is refused with that id's reason, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} for a dry run over an unknown id, got {body}"
        );

        // `DryRun=false` is a real request.
        let (status, body) = delete(&router, &id, &[("DryRun", "false")]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 with DryRun=false, got {status} with body {body}"
        );
        assert!(
            stored_ids(&engine).is_empty(),
            "case {case:?}: the target must be gone, the store holds {:?}",
            stored_ids(&engine)
        );
    }
}

/// RULE 5: the byte charge one stored target holds is given back when it is removed, on
/// exactly the terms the insert took it, and the store's own ceiling bounds how many targets
/// it holds.
#[test]
fn the_target_store_gives_its_byte_charge_back_and_is_bounded() {
    let (_router, engine) = super::ec2_query::emulator();
    let store = engine.state::<Ec2State>();

    // The byte charge round-trips, for a target of each shape in the table.
    for (case, row) in TARGET_ROWS {
        let mut store = store.lock();
        let before = store.stored_bytes();
        let record = target(row, 0);
        let charge = traffic_mirror_target_payload_bytes(&record, None);
        let id = store
            .insert_traffic_mirror_target(record, None)
            .unwrap_or_else(|| panic!("case {case:?}: the store must have room for one target"));
        assert_eq!(
            store.stored_bytes(),
            before + charge,
            "case {case:?}: the insert must charge exactly {charge} bytes, the store holds {}",
            store.stored_bytes()
        );
        let removed = store.remove_traffic_mirror_target(&id);
        assert!(
            removed.is_some(),
            "case {case:?}: the store must give back the target it stored"
        );
        assert_eq!(
            store.stored_bytes(),
            before,
            "case {case:?}: the delete must give the charge back, the store holds {}",
            store.stored_bytes()
        );
    }

    // The ceiling: the store takes exactly `MAX_TRAFFIC_MIRROR_TARGETS` targets and refuses
    // the one after them, and a removal makes room again.
    let mut store = store.lock();
    let record = target(&TARGET_ROWS[0].1, 0);
    for n in 0..MAX_TRAFFIC_MIRROR_TARGETS {
        assert!(
            store.has_traffic_mirror_target_capacity(&record, None),
            "case {n}: the store must take {MAX_TRAFFIC_MIRROR_TARGETS} targets, it refused at {n}"
        );
        assert!(
            store.insert_traffic_mirror_target(target(&TARGET_ROWS[0].1, n), None).is_some(),
            "case {n}: the insert must store the target the capacity check admitted"
        );
    }
    assert!(
        !store.has_traffic_mirror_target_capacity(&record, None),
        "the store must be full at {MAX_TRAFFIC_MIRROR_TARGETS} targets, it holds {}",
        store.traffic_mirror_targets.len()
    );
    assert!(
        store.insert_traffic_mirror_target(record.clone(), None).is_none(),
        "the insert must store nothing once the store is full, it holds {}",
        store.traffic_mirror_targets.len()
    );
    let full = store.traffic_mirror_targets.len();
    let first = store
        .traffic_mirror_targets
        .keys()
        .next()
        .cloned()
        .expect("the full store holds a target");
    assert!(
        store.remove_traffic_mirror_target(&first).is_some(),
        "the store must give back the target it stored"
    );
    assert!(
        store.has_traffic_mirror_target_capacity(&record, None),
        "a removal must make room again, the store holds {} of {full}",
        store.traffic_mirror_targets.len()
    );
}
