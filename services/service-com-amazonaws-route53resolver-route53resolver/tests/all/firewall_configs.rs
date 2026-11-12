// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DNS Firewall per-VPC configuration slice:
//! `GetFirewallConfig` and `UpdateFirewallConfig`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::MAX_STORED_FIREWALL_CONFIGS;

use super::harness::{call, error_type, expect_error, ok, string_member, Emulator, ACCOUNT};

/// Every value the model's `FirewallFailOpenStatus` enum declares
/// (`smithy_query(query="shape",shape="FirewallFailOpenStatus")`), which is what
/// `UpdateFirewallConfig$FirewallFailOpen` accepts.
const FAIL_OPEN_VALUES: &[&str] = &["DISABLED", "ENABLED", "USE_LOCAL_RESOURCE_SETTING"];

/// The `FirewallFailOpen` a VPC reports until an update names another value:
/// `FirewallConfig$FirewallFailOpen`'s doc, "By default, fail open is disabled".
const FAIL_OPEN_DEFAULT: &str = "DISABLED";

/// The `FirewallConfig` of a get/update response.
fn firewall_config(body: &Value) -> &Value {
    body.get("FirewallConfig")
        .expect("the response carries a FirewallConfig")
}

/// Read one VPC's firewall configuration over the wire.
async fn get_config(router: &Emulator, resource_id: &str) -> Value {
    let body = ok(
        router,
        "GetFirewallConfig",
        json!({ "ResourceId": resource_id }),
    )
    .await;
    firewall_config(&body).clone()
}

/// Set one VPC's failure mode over the wire, through a call that must succeed.
async fn update_config(router: &Emulator, resource_id: &str, fail_open: &str) -> Value {
    let body = ok(
        router,
        "UpdateFirewallConfig",
        json!({ "ResourceId": resource_id, "FirewallFailOpen": fail_open }),
    )
    .await;
    firewall_config(&body).clone()
}

/// Assert the identity members of a configuration: what the VPC it belongs to
/// and the account that owns it fix, whatever its `FirewallFailOpen` is.
fn assert_identity(case: &str, config: &Value, resource_id: &str) {
    let id = string_member(config, "Id");
    assert!(
        id.starts_with("rslvr-fc-"),
        "case {case:?}: a firewall configuration id is minted as rslvr-fc-<hex>, got {id:?}"
    );
    assert!(
        !id.is_empty() && id.len() <= 64,
        "case {case:?}: Id must satisfy ResourceId's length 1..64, got {id:?}"
    );
    assert_eq!(
        string_member(config, "ResourceId"),
        resource_id,
        "case {case:?}: ResourceId must be the VPC the configuration was asked for, got {config}"
    );
    let owner = string_member(config, "OwnerId");
    assert_eq!(
        owner, ACCOUNT,
        "case {case:?}: OwnerId must be the account that owns the VPC, got {owner:?}"
    );
    assert!(
        owner.len() >= 12 && owner.len() <= 32,
        "case {case:?}: OwnerId must satisfy AccountId's length 12..32, got {owner:?}"
    );
}

/// Every VPC has a firewall configuration the service maintains itself: reading
/// one that nothing has written reports the documented default, and reports the
/// same identity and value however often it is read.
///
/// The table spans what the rule generalises over — the form of the VPC id the
/// configuration is asked for (`ResourceId`, length 1..64, is the only
/// constraint the model puts on it): the two id widths EC2 mints, the shortest
/// and the longest id the model admits, and an id that does not read like one
/// AWS mints at all.
#[tokio::test]
async fn every_vpc_reports_a_firewall_configuration_at_its_documented_default() {
    let longest = format!("vpc-{}", "0".repeat(60));
    let cases: Vec<(&str, String)> = vec![
        ("the 8-hex-digit form", "vpc-1a2b3c4d".to_string()),
        (
            "the 17-hex-digit form",
            "vpc-0abcdef1234567890".to_string(),
        ),
        ("one character, the modelled minimum", "v".to_string()),
        ("64 characters, the modelled maximum", longest.clone()),
        ("an id in no form AWS mints", "my.private.network".to_string()),
    ];

    let router = super::harness::emulator();
    // Every row's VPC is one the account holds: both operations resolve their
    // `ResourceId` against the account's own Amazon EC2 inventory.
    router.hold_vpcs(cases.iter().map(|(_, resource_id)| resource_id));
    let mut seen_ids: Vec<(String, String)> = Vec::new();

    for (case, resource_id) in &cases {
        let config = get_config(&router, resource_id).await;
        assert_identity(case, &config, resource_id);
        let fail_open = string_member(&config, "FirewallFailOpen");
        assert_eq!(
            fail_open, FAIL_OPEN_DEFAULT,
            "case {case:?}: an untouched VPC reports the documented default, got {fail_open:?}"
        );

        // A read reports a configuration without creating one, so a second read
        // of the same VPC answers exactly the same object.
        let again = get_config(&router, resource_id).await;
        assert_eq!(
            again, config,
            "case {case:?}: two reads of one VPC must report the same configuration, got {again} then {config}"
        );

        seen_ids.push((case.to_string(), string_member(&config, "Id")));
    }

    for (case, id) in &seen_ids {
        let clashes = seen_ids
            .iter()
            .filter(|(_, other)| other == id)
            .count();
        assert_eq!(
            clashes, 1,
            "case {case:?}: each VPC's configuration carries its own Id, but {id:?} is reported for {clashes} of them"
        );
    }
}

/// An updated configuration is persisted: `GetFirewallConfig` reports the value
/// the update stored, with the identity the VPC had before it, and no other
/// VPC's configuration moves.
///
/// The table is every value the model's enum declares, applied to two different
/// VPCs, and each one is read back through `GetFirewallConfig`.
#[tokio::test]
async fn an_updated_firewall_configuration_is_read_back_from_state() {
    let router = super::harness::emulator();
    let subject = "vpc-0123456789abcdef0";
    let untouched = "vpc-fedcba9876543210f";
    router.hold_vpcs([subject, untouched]);

    let before = get_config(&router, subject).await;

    for value in FAIL_OPEN_VALUES {
        let case = format!("FirewallFailOpen {value:?}");
        let updated = ok(
            &router,
            "UpdateFirewallConfig",
            json!({ "ResourceId": subject, "FirewallFailOpen": value }),
        )
        .await;
        let updated = firewall_config(&updated).clone();
        assert_identity(&case, &updated, subject);
        assert_eq!(
            string_member(&updated, "FirewallFailOpen"),
            *value,
            "case {case:?}: the update must report the value it stored, got {updated}"
        );

        let read_back = get_config(&router, subject).await;
        assert_eq!(
            read_back, updated,
            "case {case:?}: GetFirewallConfig must report the stored configuration, got {read_back} after {updated}"
        );
        assert_eq!(
            string_member(&read_back, "Id"),
            string_member(&before, "Id"),
            "case {case:?}: an update must not change the configuration's Id, got {read_back} from {before}"
        );

        let other = get_config(&router, untouched).await;
        assert_eq!(
            string_member(&other, "FirewallFailOpen"),
            FAIL_OPEN_DEFAULT,
            "case {case:?}: updating one VPC must leave another at its default, got {other}"
        );
    }
}

/// A `FirewallFailOpen` the modelled enum does not declare is refused, and the
/// refusal writes nothing: the VPC keeps the configuration it had.
#[tokio::test]
async fn an_unmodelled_fail_open_status_is_refused_and_stores_nothing() {
    let router = super::harness::emulator();
    let subject = "vpc-00112233445566778";
    router.hold_vpcs([subject]);

    ok(
        &router,
        "UpdateFirewallConfig",
        json!({ "ResourceId": subject, "FirewallFailOpen": "ENABLED" }),
    )
    .await;

    for value in ["enabled", "OPEN", "USE_LOCAL", ""] {
        let case = format!("FirewallFailOpen {value:?}");
        expect_error(
            &router,
            "UpdateFirewallConfig",
            json!({ "ResourceId": subject, "FirewallFailOpen": value }),
            &case,
            "ValidationException",
        )
        .await;

        let config = get_config(&router, subject).await;
        assert_eq!(
            string_member(&config, "FirewallFailOpen"),
            "ENABLED",
            "case {case:?}: a refused update must leave the stored configuration alone, got {config}"
        );
    }
}

/// The failure mode a VPC reports is the one the LAST update set, whatever the
/// order of the updates and however many there were; applying the same value
/// twice changes nothing.
///
/// The rows differ in the length of the update sequence and in which values it
/// walks, so what is pinned is "the last write decides" rather than one pair of
/// values — and each row is read back through `GetFirewallConfig`.
#[tokio::test]
async fn the_last_update_decides_the_reported_failure_mode() {
    let router = super::harness::emulator();

    let sequences: &[&[&str]] = &[
        &["ENABLED"],
        &["ENABLED", "DISABLED"],
        &["DISABLED", "USE_LOCAL_RESOURCE_SETTING"],
        &["USE_LOCAL_RESOURCE_SETTING", "ENABLED", "DISABLED"],
        &["ENABLED", "ENABLED", "ENABLED"],
        &[
            "DISABLED",
            "ENABLED",
            "USE_LOCAL_RESOURCE_SETTING",
            "DISABLED",
            "ENABLED",
        ],
    ];

    for (index, sequence) in sequences.iter().enumerate() {
        let case = format!("sequence {sequence:?}");
        let resource_id = format!("vpc-sequence-{index}");
        router.hold_vpcs([&resource_id]);

        let mut last_answer = Value::Null;
        for fail_open in sequence.iter() {
            last_answer = update_config(&router, &resource_id, fail_open).await;
        }
        let expected = sequence
            .last()
            .expect("every sequence carries at least one update");
        assert_identity(&case, &last_answer, &resource_id);
        assert_eq!(
            string_member(&last_answer, "FirewallFailOpen"),
            *expected,
            "case {case:?}: the update must report the value it stored, got {last_answer}"
        );

        let read_back = get_config(&router, &resource_id).await;
        assert_eq!(
            read_back, last_answer,
            "case {case:?}: the read must report the value the last update set, got {read_back}"
        );
    }
}

/// `ResourceId`'s modelled length (1..64) bounds BOTH operations: a value past
/// the maximum, or the empty string the minimum excludes, is refused. The
/// lengths inside the bound are served by
/// [`every_vpc_reports_a_firewall_configuration_at_its_documented_default`],
/// whose table already carries the shortest and the longest id the model admits.
#[tokio::test]
async fn a_resource_id_outside_the_modelled_length_is_refused() {
    let router = super::harness::emulator();

    for (case, resource_id) in [
        ("one character past the maximum", "v".repeat(65)),
        ("far past the maximum", "v".repeat(200)),
        ("empty, which length 1..64 excludes", String::new()),
    ] {
        expect_error(
            &router,
            "UpdateFirewallConfig",
            json!({ "ResourceId": resource_id, "FirewallFailOpen": "ENABLED" }),
            case,
            "ValidationException",
        )
        .await;
        expect_error(
            &router,
            "GetFirewallConfig",
            json!({ "ResourceId": resource_id }),
            case,
            "ValidationException",
        )
        .await;
    }
}

/// Both operations require the members the model marks required, and a request
/// that omits one is refused and writes nothing.
#[tokio::test]
async fn a_request_missing_a_required_member_is_refused() {
    let router = super::harness::emulator();
    router.hold_vpcs(["vpc-missing-member"]);

    for (case, operation, body) in [
        (
            "UpdateFirewallConfig without ResourceId",
            "UpdateFirewallConfig",
            json!({ "FirewallFailOpen": "ENABLED" }),
        ),
        (
            "UpdateFirewallConfig without FirewallFailOpen",
            "UpdateFirewallConfig",
            json!({ "ResourceId": "vpc-missing-member" }),
        ),
        (
            "GetFirewallConfig without ResourceId",
            "GetFirewallConfig",
            json!({}),
        ),
    ] {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a missing required member must be refused, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {parsed}"
        );
    }

    let config = get_config(&router, "vpc-missing-member").await;
    assert_eq!(
        string_member(&config, "FirewallFailOpen"),
        FAIL_OPEN_DEFAULT,
        "a refused update must leave the VPC at its default, got {config}"
    );
}

/// The store is bounded: a VPC past the bound is refused, while a VPC already
/// stored is still updatable there, because updating one allocates no record.
#[tokio::test]
async fn the_configuration_store_is_bounded() {
    let router = super::harness::emulator();
    router.hold_vpcs(
        (0..MAX_STORED_FIREWALL_CONFIGS)
            .map(|index| format!("vpc-bound-{index}"))
            .chain(["vpc-one-too-many".to_string()]),
    );

    for index in 0..MAX_STORED_FIREWALL_CONFIGS {
        update_config(&router, &format!("vpc-bound-{index}"), "ENABLED").await;
    }

    expect_error(
        &router,
        "UpdateFirewallConfig",
        json!({ "ResourceId": "vpc-one-too-many", "FirewallFailOpen": "ENABLED" }),
        "one VPC past the bound",
        "ThrottlingException",
    )
    .await;

    let updated = update_config(&router, "vpc-bound-0", "DISABLED").await;
    assert_eq!(
        string_member(&updated, "FirewallFailOpen"),
        "DISABLED",
        "a VPC already stored must stay updatable at the bound, got {updated}"
    );
}
