// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DNSSEC validation slice:
//! `UpdateResolverDnssecConfig` and `GetResolverDnssecConfig`.
//!
//! `ListResolverDnssecConfigs` has its own suite,
//! [`super::resolver_dnssec_config_listing`], because what it pins is the
//! paging contract of a `@paginated` list operation.
//!
//! Every case goes over the wire the service actually speaks, through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::{
    RESOLVER_D_N_S_S_E_C_VALIDATION_STATUS_VALUES, VALIDATION_VALUES,
};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    DNSSEC_STATUS_DISABLED, DNSSEC_STATUS_DISABLING, DNSSEC_STATUS_ENABLED, DNSSEC_STATUS_ENABLING,
    DNSSEC_STATUS_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING, DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING,
    PROVISIONING_DELAY, VALIDATION_DISABLE, VALIDATION_ENABLE,
    VALIDATION_USE_LOCAL_RESOURCE_SETTING,
};

use super::harness::{emulator, expect_error, ok, string_member, Emulator, ACCOUNT};

/// The `ResolverDNSSECConfig` of an update or get response.
fn config(body: &Value) -> &Value {
    body.get("ResolverDNSSECConfig")
        .unwrap_or_else(|| panic!("the response carries a ResolverDNSSECConfig, got {body}"))
}

/// Wait out the span the clock owes a configuration whose validation status is
/// still in flight, so the next call sees the settled status.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

async fn update(router: &Emulator, resource_id: &str, validation: &str) -> Value {
    ok(
        router,
        "UpdateResolverDnssecConfig",
        json!({ "ResourceId": resource_id, "Validation": validation }),
    )
    .await
}

async fn get(router: &Emulator, resource_id: &str) -> Value {
    ok(
        router,
        "GetResolverDnssecConfig",
        json!({ "ResourceId": resource_id }),
    )
    .await
}

/// Assert the reported configuration describes `resource_id` with
/// `validation_status`, and return its `Id`.
fn assert_config(case: &str, reported: &Value, resource_id: &str, validation_status: &str) -> String {
    let actual_status = string_member(reported, "ValidationStatus");
    assert_eq!(
        actual_status, validation_status,
        "case {case:?}: ValidationStatus must be {validation_status:?}, got {reported}"
    );
    // The reported status is a member of the modelled enum
    // (`smithy_query(query="shape",shape="ResolverDNSSECValidationStatus")`,
    // re-exported as RESOLVER_D_N_S_S_E_C_VALIDATION_STATUS_VALUES).
    assert!(
        RESOLVER_D_N_S_S_E_C_VALIDATION_STATUS_VALUES.contains(&actual_status.as_str()),
        "case {case:?}: ValidationStatus must be one of \
         {RESOLVER_D_N_S_S_E_C_VALIDATION_STATUS_VALUES:?}, got {actual_status:?}"
    );
    assert_eq!(
        string_member(reported, "ResourceId"),
        resource_id,
        "case {case:?}: ResourceId must be the VPC the request named, got {reported}"
    );
    assert_eq!(
        string_member(reported, "OwnerId"),
        ACCOUNT,
        "case {case:?}: OwnerId must be the account the emulator serves, got {reported}"
    );

    let id = string_member(reported, "Id");
    let hex = id.strip_prefix("rdsc-").unwrap_or_else(|| {
        panic!("case {case:?}: Id must be minted as rdsc-<hex>, got {id:?}")
    });
    assert!(
        hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "case {case:?}: Id must carry 16 lower-case hex characters, got {id:?}"
    );
    // `ResolverDnssecConfig$Id` targets `ResourceId`, length 1..64.
    assert!(
        !id.is_empty() && id.len() <= 64,
        "case {case:?}: Id must satisfy ResourceId's length 1..64, got {id:?}"
    );
    id
}

/// Every VPC has a configuration for DNSSEC validation the service maintains
/// itself: reading one that nothing has written reports validation off,
/// settled, with an identity that does not move between reads — and the first
/// update of that VPC reports the very same `Id`.
///
/// The table spans what the rule generalises over — the form of the VPC id the
/// configuration is asked for (`ResourceId`, length 1..64, is the only
/// constraint the model puts on it): the two id widths EC2 mints, the shortest
/// and the longest id the model admits, and an id that does not read like one
/// AWS mints at all.
#[tokio::test]
async fn every_vpc_reports_a_dnssec_configuration_with_validation_off() {
    let cases: Vec<(&str, String)> = vec![
        ("the 8-hex-digit form", "vpc-1a2b3c4d".to_string()),
        (
            "the 17-hex-digit form",
            "vpc-0abcdef1234567890".to_string(),
        ),
        ("one character, the modelled minimum", "v".to_string()),
        (
            "64 characters, the modelled maximum",
            format!("vpc-{}", "0".repeat(60)),
        ),
        ("an id in no form AWS mints", "my.private.network".to_string()),
    ];

    let router = emulator();
    // Every row's VPC is one the account holds: both operations resolve their
    // `ResourceId` against the account's own Amazon EC2 inventory.
    router.hold_vpcs(cases.iter().map(|(_, resource_id)| resource_id));
    let mut seen: Vec<(String, String)> = Vec::new();

    for (case, resource_id) in &cases {
        let reported = get(&router, resource_id).await;
        let id = assert_config(case, config(&reported), resource_id, DNSSEC_STATUS_DISABLED);

        // A read reports a configuration without creating one, so a second read
        // of the same VPC answers exactly the same object.
        let again = get(&router, resource_id).await;
        assert_eq!(
            config(&again),
            config(&reported),
            "case {case:?}: two reads of one VPC must report the same configuration"
        );

        // The identity a read reports is the one the configuration keeps: the
        // first update of that VPC answers with the same `Id`.
        let updated = update(&router, resource_id, VALIDATION_ENABLE).await;
        let updated_id = assert_config(case, config(&updated), resource_id, DNSSEC_STATUS_ENABLING);
        assert_eq!(
            updated_id, id,
            "case {case:?}: the first update must keep the Id the read reported, got {updated}"
        );
        seen.push((case.to_string(), id));
    }

    for (case, id) in &seen {
        let clashes = seen.iter().filter(|(_, other)| other == id).count();
        assert_eq!(
            clashes, 1,
            "case {case:?}: each VPC's configuration carries its own Id, but {id:?} is reported for {clashes} of them"
        );
    }
}

/// One row of the update table: the VPC, the setting the request asks for, and
/// the pair of statuses the configuration must report for it — the transient
/// one straight after the call, the settled one once the clock owes the move.
#[derive(Debug)]
struct UpdateCase {
    case: &'static str,
    resource_id: String,
    validation: &'static str,
    in_flight: &'static str,
    settled: &'static str,
}

/// An update moves the validation setting of the configuration the VPC already
/// has, keeping its identity, and both the update and a later read compute every
/// member from the stored record.
///
/// The table spans what the rule generalises over: each of the three values the
/// modelled `Validation` enum declares, and a `ResourceId` at both ends of the
/// member's modelled 1..64 length.
#[tokio::test]
async fn an_update_moves_the_validation_status_and_is_read_back_from_state() {
    let cases = vec![
        UpdateCase {
            case: "ENABLE on a VPC at its default",
            resource_id: "vpc-0123456789abcdef0".to_string(),
            validation: VALIDATION_ENABLE,
            in_flight: DNSSEC_STATUS_ENABLING,
            settled: DNSSEC_STATUS_ENABLED,
        },
        UpdateCase {
            // Validation off is the setting a VPC at its default already has,
            // so nothing moves and no transient status appears.
            case: "DISABLE on a VPC at its default",
            resource_id: "vpc-00000000000000001".to_string(),
            validation: VALIDATION_DISABLE,
            in_flight: DNSSEC_STATUS_DISABLED,
            settled: DNSSEC_STATUS_DISABLED,
        },
        UpdateCase {
            case: "USE_LOCAL_RESOURCE_SETTING on a VPC at its default",
            resource_id: "vpc-00000000000000002".to_string(),
            validation: VALIDATION_USE_LOCAL_RESOURCE_SETTING,
            in_flight: DNSSEC_STATUS_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            settled: DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING,
        },
        UpdateCase {
            case: "a one-character ResourceId, the minimum of the modelled length",
            resource_id: "v".to_string(),
            validation: VALIDATION_ENABLE,
            in_flight: DNSSEC_STATUS_ENABLING,
            settled: DNSSEC_STATUS_ENABLED,
        },
        UpdateCase {
            case: "a 64-character ResourceId, the maximum of the modelled length",
            resource_id: format!("vpc-{}", "a".repeat(60)),
            validation: VALIDATION_USE_LOCAL_RESOURCE_SETTING,
            in_flight: DNSSEC_STATUS_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            settled: DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING,
        },
    ];

    let router = emulator();
    router.hold_vpcs(cases.iter().map(|case| &case.resource_id));
    let mut created: Vec<(String, String, &'static str)> = Vec::with_capacity(cases.len());

    for case in &cases {
        let before = get(&router, &case.resource_id).await;
        let before_id = assert_config(
            case.case,
            config(&before),
            &case.resource_id,
            DNSSEC_STATUS_DISABLED,
        );

        let updated = update(&router, &case.resource_id, case.validation).await;
        let id = assert_config(
            case.case,
            config(&updated),
            &case.resource_id,
            case.in_flight,
        );
        assert_eq!(
            id, before_id,
            "case {:?}: an update must not change the configuration's Id, got {updated} after {before}",
            case.case
        );

        // The configuration is persisted, not computed per call: a read
        // straight after the update resolves the same record, and still
        // reports the transient status because the move is not due yet.
        let read = get(&router, &case.resource_id).await;
        let read_id = assert_config(case.case, config(&read), &case.resource_id, case.in_flight);
        assert_eq!(
            read_id, id,
            "case {:?}: the read must resolve the configuration the update wrote, got {read}",
            case.case
        );
        created.push((case.resource_id.clone(), id, case.settled));
    }

    // Every configuration got its own `Id`.
    let mut ids: Vec<&str> = created.iter().map(|(_, id, _)| id.as_str()).collect();
    ids.sort_unstable();
    let total = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "each configuration must carry its own Id, got {ids:?}"
    );

    settle().await;

    for (resource_id, id, settled) in &created {
        let case = format!("settled configuration of {resource_id}");
        let read = get(&router, resource_id).await;
        let read_id = assert_config(&case, config(&read), resource_id, settled);
        assert_eq!(
            read_id, *id,
            "case {case:?}: the Id must not change as the status settles, got {read}"
        );

        // A read changes nothing a later caller sees.
        let again = get(&router, resource_id).await;
        assert_eq!(
            config(&again),
            config(&read),
            "case {case:?}: a second read must report the same configuration"
        );
    }
}

/// One row of the transition table: the setting a settled configuration is
/// updated to, and the statuses it must then report.
#[derive(Debug)]
struct TransitionCase {
    case: &'static str,
    from: &'static str,
    to: &'static str,
    in_flight: &'static str,
    settled: &'static str,
}

/// Updating a VPC that already has a configuration moves its validation status
/// and keeps the configuration itself: the same `Id` and the same owner.
///
/// The table spans the transitions between the three settings, including a
/// repeat of the setting the configuration already reports.
#[tokio::test]
async fn an_update_of_an_existing_configuration_keeps_its_identity() {
    let cases = [
        TransitionCase {
            case: "ENABLED then DISABLE",
            from: VALIDATION_ENABLE,
            to: VALIDATION_DISABLE,
            in_flight: DNSSEC_STATUS_DISABLING,
            settled: DNSSEC_STATUS_DISABLED,
        },
        TransitionCase {
            case: "DISABLED then ENABLE",
            from: VALIDATION_DISABLE,
            to: VALIDATION_ENABLE,
            in_flight: DNSSEC_STATUS_ENABLING,
            settled: DNSSEC_STATUS_ENABLED,
        },
        TransitionCase {
            case: "ENABLED then USE_LOCAL_RESOURCE_SETTING",
            from: VALIDATION_ENABLE,
            to: VALIDATION_USE_LOCAL_RESOURCE_SETTING,
            in_flight: DNSSEC_STATUS_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            settled: DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING,
        },
        TransitionCase {
            case: "USE_LOCAL_RESOURCE_SETTING then ENABLE",
            from: VALIDATION_USE_LOCAL_RESOURCE_SETTING,
            to: VALIDATION_ENABLE,
            in_flight: DNSSEC_STATUS_ENABLING,
            settled: DNSSEC_STATUS_ENABLED,
        },
        TransitionCase {
            // The setting the configuration already reports: nothing moves, so
            // the settled status is what the update answers with.
            case: "ENABLED then ENABLE again",
            from: VALIDATION_ENABLE,
            to: VALIDATION_ENABLE,
            in_flight: DNSSEC_STATUS_ENABLED,
            settled: DNSSEC_STATUS_ENABLED,
        },
    ];

    let router = emulator();

    for (index, case) in cases.iter().enumerate() {
        let resource_id = format!("vpc-transition-{index}");
        router.hold_vpcs([&resource_id]);
        let first = update(&router, &resource_id, case.from).await;
        let id = string_member(config(&first), "Id");
        settle().await;

        let second = update(&router, &resource_id, case.to).await;
        let second_id = assert_config(case.case, config(&second), &resource_id, case.in_flight);
        assert_eq!(
            second_id, id,
            "case {:?}: an update of an existing configuration keeps its Id, got {second}",
            case.case
        );

        settle().await;
        let read = get(&router, &resource_id).await;
        let read_id = assert_config(case.case, config(&read), &resource_id, case.settled);
        assert_eq!(
            read_id, id,
            "case {:?}: the read must resolve the same configuration, got {read}",
            case.case
        );
    }
}

/// Repeating an update while the move it started is still in flight writes
/// nothing: the configuration keeps the status it is moving through and settles
/// on the schedule the first call made due, rather than having the move pushed
/// out by every repeat.
#[tokio::test]
async fn repeating_an_in_flight_update_does_not_restart_the_clock() {
    let router = emulator();
    let resource_id = "vpc-in-flight";
    router.hold_vpcs([resource_id]);

    let first = update(&router, resource_id, VALIDATION_ENABLE).await;
    let id = assert_config(
        "the first ENABLE",
        config(&first),
        resource_id,
        DNSSEC_STATUS_ENABLING,
    );

    // Half the span the clock owes, then the very same request again.
    tokio::time::sleep(PROVISIONING_DELAY / 2).await;
    let repeat = update(&router, resource_id, VALIDATION_ENABLE).await;
    let repeat_id = assert_config(
        "the repeated ENABLE",
        config(&repeat),
        resource_id,
        DNSSEC_STATUS_ENABLING,
    );
    assert_eq!(
        repeat_id, id,
        "the repeated update must resolve the same configuration, got {repeat}"
    );

    // Past the first call's due instant, but short of where a restarted clock
    // would have put it.
    tokio::time::sleep(PROVISIONING_DELAY - Duration::from_millis(100)).await;
    let read = get(&router, resource_id).await;
    assert_config(
        "the configuration after the repeat",
        config(&read),
        resource_id,
        DNSSEC_STATUS_ENABLED,
    );
}

/// A `Validation` the modelled enum does not declare is refused, and the refusal
/// writes nothing: the VPC still reports the configuration it had.
#[tokio::test]
async fn a_validation_outside_the_modelled_enum_is_refused() {
    let router = emulator();
    router.hold_vpcs(["vpc-refused"]);

    // One case per way a value can fall outside VALIDATION_VALUES: wrong case,
    // a value of the *status* enum rather than of `Validation`, an empty
    // string, and an unrelated word.
    let cases = ["enable", "ENABLED", "", "SOMETHING_ELSE"];
    for case in cases {
        assert!(
            !VALIDATION_VALUES.contains(&case),
            "case {case:?}: the case must not be a modelled Validation value"
        );
        expect_error(
            &router,
            "UpdateResolverDnssecConfig",
            json!({ "ResourceId": "vpc-refused", "Validation": case }),
            case,
            "ValidationException",
        )
        .await;
        // The refusal wrote nothing: the VPC still reports validation off, the
        // setting it had before the refused call.
        let read = get(&router, "vpc-refused").await;
        assert_config(case, config(&read), "vpc-refused", DNSSEC_STATUS_DISABLED);
    }

    // Every value the enum does declare is accepted on that same VPC.
    for validation in VALIDATION_VALUES {
        let updated = update(&router, "vpc-refused", validation).await;
        assert!(
            config(&updated).get("ValidationStatus").is_some(),
            "case {validation:?}: a modelled Validation must be accepted, got {updated}"
        );
    }
}
