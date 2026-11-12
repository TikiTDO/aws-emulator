// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetResolverDnssecConfig` as a REPORTING operation.
//!
//! [`super::resolver_dnssec_configs`] pins what the DNSSEC validation slice reports —
//! the statuses an update moves a configuration through, and the identity it keeps.
//! What these cases pin is the other half of the contract, which no response can show
//! on its own: reading a VPC's configuration reports it without writing a record, and
//! the `ResourceId` a read may ask about is bounded by the member's modelled length.
//!
//! Every case goes over the wire the service actually speaks, through the shared
//! [`super::harness`]; the store is read directly only to count the records a call left
//! behind.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    service_state, DNSSEC_STATUS_DISABLED, DNSSEC_STATUS_ENABLING, VALIDATION_ENABLE,
};

use super::harness::{emulator, emulator_on, expect_error, ok, string_member, Emulator};

/// The `ResolverDNSSECConfig` a get or update answered with, owned so two answers can be
/// compared after both calls have returned.
fn config(body: &Value) -> Value {
    body.get("ResolverDNSSECConfig")
        .cloned()
        .unwrap_or_else(|| panic!("the response carries a ResolverDNSSECConfig, got {body}"))
}

async fn get(router: &Emulator, resource_id: &str) -> Value {
    config(
        &ok(
            router,
            "GetResolverDnssecConfig",
            json!({ "ResourceId": resource_id }),
        )
        .await,
    )
}

/// How many configurations for DNSSEC validation the store holds a record for.
fn stored(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the store is reachable from the engine the service is mounted on")
        .lock()
        .resolver_dnssec_configs
        .len()
}

/// Reading a VPC's configuration for DNSSEC validation reports it — validation off for a
/// VPC nothing has configured — and writes NO record: the store holds a configuration
/// only for the VPCs an update named, so no number of reads consumes the capacity the
/// slice is bounded by, and every read of one VPC answers the same object.
///
/// The table spans what the rule generalises over, the form of the VPC id the
/// configuration is asked for (`ResourceId`, length 1..64, is the only constraint the
/// model puts on it): the two id widths EC2 mints, both ends of the modelled length, and
/// an id in no form AWS mints.
#[tokio::test]
async fn reading_a_configuration_stores_nothing() {
    let cases: Vec<(&str, String)> = vec![
        ("the 8-hex-digit form", "vpc-9f8e7d6c".to_string()),
        ("the 17-hex-digit form", "vpc-0fedcba987654321f".to_string()),
        ("one character, the modelled minimum", "x".to_string()),
        (
            "64 characters, the modelled maximum",
            format!("vpc-{}", "7".repeat(60)),
        ),
        ("an id in no form AWS mints", "office/vpc #2".to_string()),
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
            string_member(&first, "ValidationStatus"),
            DNSSEC_STATUS_DISABLED,
            "case {case:?}: a VPC nothing configured reports validation off, got {first}"
        );
        assert_eq!(
            string_member(&first, "ResourceId"),
            *resource_id,
            "case {case:?}: ResourceId must be the VPC the request named, got {first}"
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
    let updated = ok(
        &router,
        "UpdateResolverDnssecConfig",
        json!({ "ResourceId": cases[0].1, "Validation": VALIDATION_ENABLE }),
    )
    .await;
    assert_eq!(
        string_member(&config(&updated), "ValidationStatus"),
        DNSSEC_STATUS_ENABLING,
        "an update to ENABLE reports the status the move goes through, got {updated}"
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

/// The `ResourceId` a read may ask about is bounded by the member's modelled length
/// (`ResourceId`, length 1..64): each published bound is served, and a request one
/// character past either bound — or naming no VPC at all, the member being required — is
/// refused.
#[tokio::test]
async fn a_resource_id_outside_the_modelled_length_is_refused() {
    let router = emulator();

    for (case, resource_id) in [
        ("1 character, the modelled minimum", "z".to_string()),
        ("64 characters, the modelled maximum", "z".repeat(64)),
    ] {
        router.hold_vpcs([&resource_id]);
        let reported = get(&router, &resource_id).await;
        assert_eq!(
            string_member(&reported, "ResourceId"),
            resource_id,
            "case {case:?}: a ResourceId within the modelled length is served, got {reported}"
        );
    }

    for (case, request) in [
        (
            "empty, one short of the modelled minimum",
            json!({ "ResourceId": "" }),
        ),
        (
            "65 characters, one past the modelled maximum",
            json!({ "ResourceId": "z".repeat(65) }),
        ),
        ("no ResourceId, which is required", json!({})),
    ] {
        expect_error(
            &router,
            "GetResolverDnssecConfig",
            request,
            case,
            "ValidationException",
        )
        .await;
    }
}
