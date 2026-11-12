// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetResolverConfig` as a REPORTING operation.
//!
//! [`super::resolver_configs`] pins what the per-VPC Resolver behavior slice reports —
//! the statuses an update moves a configuration through and the identity it keeps. What
//! these cases pin is the other half of the contract, which no single response can show:
//! reading a VPC's configuration reports it without writing a record, so no number of
//! reads consumes the capacity the slice is bounded by, and the status a read reports is
//! the one the clock owes at the instant of the read — a read neither settles a move
//! early nor restarts the span one is still in flight for.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1, `POST /` with
//! `X-Amz-Target: Route53Resolver.<OperationName>`), through the shared
//! [`super::harness`]; the store is read directly only to count the records a call left
//! behind.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    service_state, AUTODEFINED_REVERSE_DISABLED, AUTODEFINED_REVERSE_DISABLING,
    AUTODEFINED_REVERSE_ENABLED, AUTODEFINED_REVERSE_FLAG_DISABLE, PROVISIONING_DELAY,
};

use super::harness::{emulator_on, ok, string_member, Emulator, ACCOUNT};

/// The `ResolverConfig` a get or update answered with, owned so two answers can be
/// compared after both calls have returned.
fn config(body: &Value) -> Value {
    body.get("ResolverConfig")
        .cloned()
        .unwrap_or_else(|| panic!("the response carries a ResolverConfig, got {body}"))
}

/// Read one VPC's Resolver behavior configuration over the wire.
async fn get(router: &Emulator, resource_id: &str) -> Value {
    config(
        &ok(
            router,
            "GetResolverConfig",
            json!({ "ResourceId": resource_id }),
        )
        .await,
    )
}

/// Set one VPC's autodefined-reverse setting over the wire.
async fn update(router: &Emulator, resource_id: &str, flag: &str) -> Value {
    config(
        &ok(
            router,
            "UpdateResolverConfig",
            json!({ "ResourceId": resource_id, "AutodefinedReverseFlag": flag }),
        )
        .await,
    )
}

/// How many VPC Resolver behavior configurations the store holds a record for.
fn stored(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the store is reachable from the engine the service is mounted on")
        .lock()
        .resolver_configs
        .len()
}

/// Reading a VPC's Resolver behavior configuration reports it — autodefined reverse
/// enabled, the documented default, for a VPC nothing has configured — and writes NO
/// record: the store holds a configuration only for the VPCs an update named, so no
/// number of reads consumes the capacity the slice is bounded by, and every read of one
/// VPC answers the same object.
///
/// The table spans what the rule generalises over, the form of the VPC id the
/// configuration is asked for (`ResourceId`, length 1..64, is the only constraint the
/// model puts on it): the two id widths EC2 mints, both ends of the modelled length, and
/// an id in no form AWS mints.
#[tokio::test]
async fn reading_a_configuration_stores_nothing() {
    let cases: Vec<(&str, String)> = vec![
        ("the 8-hex-digit form", "vpc-5c4b3a29".to_string()),
        ("the 17-hex-digit form", "vpc-05f4e3d2c1b0a9f8e".to_string()),
        ("one character, the modelled minimum", "q".to_string()),
        (
            "64 characters, the modelled maximum",
            format!("vpc-{}", "3".repeat(60)),
        ),
        ("an id in no form AWS mints", "lab vpc #7".to_string()),
    ];

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    // Every row's VPC is one the account holds: the read resolves its
    // `ResourceId` against the account's own Amazon EC2 inventory.
    router.hold_vpcs(cases.iter().map(|(_, resource_id)| resource_id));

    for (case, resource_id) in &cases {
        // Three reads of the same VPC: the first reports the configuration the service
        // maintains for it, and none of them writes.
        let first = get(&router, resource_id).await;
        assert_eq!(
            string_member(&first, "AutodefinedReverse"),
            AUTODEFINED_REVERSE_ENABLED,
            "case {case:?}: a VPC nothing configured reports its documented default, got {first}"
        );
        assert_eq!(
            string_member(&first, "ResourceId"),
            *resource_id,
            "case {case:?}: ResourceId must be the VPC the request named, got {first}"
        );
        assert_eq!(
            string_member(&first, "OwnerId"),
            ACCOUNT,
            "case {case:?}: OwnerId must be the account that owns the VPC, got {first}"
        );
        for repeat in 1..3 {
            let again = get(&router, resource_id).await;
            assert_eq!(
                again, first,
                "case {case:?}: read {repeat} must report the configuration the first read did, got {again}"
            );
        }
        let held = stored(&engine);
        assert_eq!(
            held, 0,
            "case {case:?}: reading a configuration must store no record, got {held} stored"
        );
    }

    // An update is what stores a record — one per VPC it named — and reading every other
    // VPC again adds none.
    let updated = update(&router, &cases[0].1, AUTODEFINED_REVERSE_FLAG_DISABLE).await;
    assert_eq!(
        string_member(&updated, "AutodefinedReverse"),
        AUTODEFINED_REVERSE_DISABLING,
        "an update to DISABLE reports the status the move goes through, got {updated}"
    );
    let held = stored(&engine);
    assert_eq!(
        held, 1,
        "the update must store exactly the one configuration it named, got {held} stored"
    );
    for (case, resource_id) in &cases[1..] {
        get(&router, resource_id).await;
        let held = stored(&engine);
        assert_eq!(
            held, 1,
            "case {case:?}: a read after an update must still store nothing, got {held} stored"
        );
    }
}

/// A read reports the status the clock owes at the instant of the read, and changes
/// nothing by reading it: reads taken while a move is in flight report the transient
/// status without settling it early, reads taken after the move is due report the settled
/// one without any further call, and neither restarts the span nor touches the
/// configuration's identity.
///
/// The rows differ in how many times the configuration is read before the move falls
/// due — none, one, and many — because that count is exactly what a read must not
/// change.
#[tokio::test]
async fn a_read_reports_the_status_the_clock_owes_and_changes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    let cases = [
        ("never read while in flight", 0_usize),
        ("read once while in flight", 1),
        ("read many times while in flight", 8),
    ];

    for (case, reads_in_flight) in cases {
        let resource_id = format!("vpc-in-flight-{reads_in_flight}");
        router.hold_vpcs([&resource_id]);
        let updated = update(&router, &resource_id, AUTODEFINED_REVERSE_FLAG_DISABLE).await;
        let id = string_member(&updated, "Id");
        assert_eq!(
            string_member(&updated, "AutodefinedReverse"),
            AUTODEFINED_REVERSE_DISABLING,
            "case {case:?}: the update reports the transient status, got {updated}"
        );

        for read in 0..reads_in_flight {
            let in_flight = get(&router, &resource_id).await;
            assert_eq!(
                string_member(&in_flight, "AutodefinedReverse"),
                AUTODEFINED_REVERSE_DISABLING,
                "case {case:?}: read {read} is short of the due instant, so the transient status \
                 still stands, got {in_flight}"
            );
            assert_eq!(
                in_flight, updated,
                "case {case:?}: read {read} must report the record the update wrote, got {in_flight}"
            );
        }

        // Past the due instant, with no call in between to settle the move: the settled
        // status is reported however many reads came before it, so no read restarted the
        // span or settled it early.
        tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
        let settled = get(&router, &resource_id).await;
        assert_eq!(
            string_member(&settled, "AutodefinedReverse"),
            AUTODEFINED_REVERSE_DISABLED,
            "case {case:?}: once the move is due the settled status is reported, got {settled}"
        );
        assert_eq!(
            string_member(&settled, "Id"),
            id,
            "case {case:?}: the configuration's Id must not move as the status settles, got {settled}"
        );
    }

    // One record per VPC an update named, and not one more: the reads above stored
    // nothing.
    let held = stored(&engine);
    assert_eq!(
        held,
        cases.len(),
        "only the updates store a record, so the store holds one per VPC they named, got {held}"
    );
}
