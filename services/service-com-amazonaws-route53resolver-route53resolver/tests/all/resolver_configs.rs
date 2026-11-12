// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the per-VPC Resolver behavior configuration slice:
//! `UpdateResolverConfig` and `GetResolverConfig`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::{
    AUTODEFINED_REVERSE_FLAG_VALUES, RESOLVER_AUTODEFINED_REVERSE_STATUS_VALUES,
};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    AUTODEFINED_REVERSE_DISABLED, AUTODEFINED_REVERSE_DISABLING, AUTODEFINED_REVERSE_ENABLED,
    AUTODEFINED_REVERSE_ENABLING, AUTODEFINED_REVERSE_FLAG_DISABLE,
    AUTODEFINED_REVERSE_FLAG_ENABLE, AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING,
    AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
    AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING, MAX_STORED_RESOLVER_CONFIGS,
    PROVISIONING_DELAY,
};

use super::harness::{call, error_type, expect_error, ok, string_member, Emulator, ACCOUNT};

/// The `ResolverConfig` of a get or update response.
fn config(body: &Value) -> &Value {
    body.get("ResolverConfig")
        .unwrap_or_else(|| panic!("the response carries a ResolverConfig, got {body}"))
}

/// Wait out the span the clock owes a configuration whose autodefined-reverse
/// status is still in flight, so the next call sees the settled status.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

/// Read one VPC's Resolver configuration over the wire.
async fn get(router: &Emulator, resource_id: &str) -> Value {
    let body = ok(
        router,
        "GetResolverConfig",
        json!({ "ResourceId": resource_id }),
    )
    .await;
    config(&body).clone()
}

/// Set one VPC's autodefined-reverse setting over the wire, through a call that
/// must succeed.
async fn update(router: &Emulator, resource_id: &str, flag: &str) -> Value {
    let body = ok(
        router,
        "UpdateResolverConfig",
        json!({ "ResourceId": resource_id, "AutodefinedReverseFlag": flag }),
    )
    .await;
    config(&body).clone()
}

/// Assert the reported configuration describes `resource_id` with
/// `autodefined_reverse`, and return its `Id`.
fn assert_config(case: &str, reported: &Value, resource_id: &str, autodefined_reverse: &str) -> String {
    let actual = string_member(reported, "AutodefinedReverse");
    assert_eq!(
        actual, autodefined_reverse,
        "case {case:?}: AutodefinedReverse must be {autodefined_reverse:?}, got {reported}"
    );
    // The reported status is a member of the modelled enum
    // (`smithy_query(query="shape",shape="ResolverAutodefinedReverseStatus")`,
    // re-exported as RESOLVER_AUTODEFINED_REVERSE_STATUS_VALUES).
    assert!(
        RESOLVER_AUTODEFINED_REVERSE_STATUS_VALUES.contains(&actual.as_str()),
        "case {case:?}: AutodefinedReverse must be one of \
         {RESOLVER_AUTODEFINED_REVERSE_STATUS_VALUES:?}, got {actual:?}"
    );
    assert_eq!(
        string_member(reported, "ResourceId"),
        resource_id,
        "case {case:?}: ResourceId must be the VPC the request named, got {reported}"
    );
    let owner = string_member(reported, "OwnerId");
    assert_eq!(
        owner, ACCOUNT,
        "case {case:?}: OwnerId must be the account that owns the VPC, got {reported}"
    );

    let id = string_member(reported, "Id");
    let hex = id
        .strip_prefix("rslvr-rc-")
        .unwrap_or_else(|| panic!("case {case:?}: Id must be minted as rslvr-rc-<hex>, got {id:?}"));
    assert!(
        hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "case {case:?}: Id must carry 16 lower-case hex characters, got {id:?}"
    );
    // `ResolverConfig$Id` targets `ResourceId`, length 1..64.
    assert!(
        !id.is_empty() && id.len() <= 64,
        "case {case:?}: Id must satisfy ResourceId's length 1..64, got {id:?}"
    );
    id
}

/// Every VPC has a Resolver configuration the service maintains itself: reading
/// one that nothing has written reports the documented default
/// (`ResolverConfig$AutodefinedReverse`: "This is enabled by default"), settled,
/// with an identity that does not move between reads.
///
/// The table spans what the rule generalises over — the form of the VPC id the
/// configuration is asked for (`ResourceId`, length 1..64, is the only
/// constraint the model puts on it): the two id widths EC2 mints, the shortest
/// and the longest id the model admits, and an id that does not read like one
/// AWS mints at all.
#[tokio::test]
async fn every_vpc_reports_a_resolver_configuration_at_its_documented_default() {
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

    let router = super::harness::emulator();
    // Every row's VPC is one the account holds: both operations resolve their
    // `ResourceId` against the account's own Amazon EC2 inventory.
    router.hold_vpcs(cases.iter().map(|(_, resource_id)| resource_id));
    let mut seen: Vec<(String, String)> = Vec::new();

    for (case, resource_id) in &cases {
        let reported = get(&router, resource_id).await;
        let id = assert_config(case, &reported, resource_id, AUTODEFINED_REVERSE_ENABLED);

        // A read reports a configuration without creating one, so a second read
        // of the same VPC answers exactly the same object.
        let again = get(&router, resource_id).await;
        assert_eq!(
            again, reported,
            "case {case:?}: two reads of one VPC must report the same configuration, got {again} then {reported}"
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
/// the pair of statuses the configuration must report for it — the transient one
/// straight after the call, the settled one once the clock owes the move.
#[derive(Debug)]
struct UpdateCase {
    case: &'static str,
    resource_id: String,
    flag: &'static str,
    in_flight: &'static str,
    settled: &'static str,
}

/// An update moves the VPC's autodefined-reverse setting off the one it had: the
/// configuration reports the transient status until the move the clock owes
/// falls due and the settled one after it, keeps the identity it had before the
/// call, and is read back from state through `GetResolverConfig`.
///
/// The table spans what the rule generalises over: the two values the modelled
/// `AutodefinedReverseFlag` enum declares that move a default VPC, applied to a
/// `ResourceId` at both ends of the member's modelled 1..64 length, plus the
/// walk on to the third value from a configuration that is already off its
/// default.
#[tokio::test]
async fn an_update_moves_the_status_through_its_transient_value_and_is_read_back() {
    let cases = vec![
        UpdateCase {
            case: "DISABLE on a VPC at its default",
            resource_id: "vpc-0123456789abcdef0".to_string(),
            flag: AUTODEFINED_REVERSE_FLAG_DISABLE,
            in_flight: AUTODEFINED_REVERSE_DISABLING,
            settled: AUTODEFINED_REVERSE_DISABLED,
        },
        UpdateCase {
            case: "USE_LOCAL_RESOURCE_SETTING on a VPC at its default",
            resource_id: "vpc-00000000000000001".to_string(),
            flag: AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING,
            in_flight: AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            settled: AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING,
        },
        UpdateCase {
            case: "a one-character ResourceId, the minimum of the modelled length",
            resource_id: "v".to_string(),
            flag: AUTODEFINED_REVERSE_FLAG_DISABLE,
            in_flight: AUTODEFINED_REVERSE_DISABLING,
            settled: AUTODEFINED_REVERSE_DISABLED,
        },
        UpdateCase {
            case: "a 64-character ResourceId, the maximum of the modelled length",
            resource_id: format!("vpc-{}", "a".repeat(60)),
            flag: AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING,
            in_flight: AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            settled: AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING,
        },
    ];

    let router = super::harness::emulator();
    router.hold_vpcs(cases.iter().map(|case| &case.resource_id));
    let mut written: Vec<(String, String, &'static str)> = Vec::with_capacity(cases.len());

    for case in &cases {
        let before = get(&router, &case.resource_id).await;
        let before_id = assert_config(
            case.case,
            &before,
            &case.resource_id,
            AUTODEFINED_REVERSE_ENABLED,
        );

        let updated = update(&router, &case.resource_id, case.flag).await;
        let id = assert_config(case.case, &updated, &case.resource_id, case.in_flight);
        assert_eq!(
            id, before_id,
            "case {:?}: an update must not change the configuration's Id, got {updated} from {before}",
            case.case
        );

        // The configuration is persisted, not computed per call: a read straight
        // after the update resolves the same record, and still reports the
        // transient status because the move is not due yet.
        let read = get(&router, &case.resource_id).await;
        assert_eq!(
            read, updated,
            "case {:?}: the read must report the record the update wrote, got {read} after {updated}",
            case.case
        );
        written.push((case.resource_id.clone(), id, case.settled));
    }

    settle().await;

    for (resource_id, id, settled) in &written {
        let case = format!("settled configuration of {resource_id}");
        let read = get(&router, resource_id).await;
        let read_id = assert_config(&case, &read, resource_id, settled);
        assert_eq!(
            read_id, *id,
            "case {case:?}: the Id must not change as the status settles, got {read}"
        );

        // Walking on to ENABLE moves the settled configuration back, through the
        // transient status of that direction.
        let back = update(&router, resource_id, AUTODEFINED_REVERSE_FLAG_ENABLE).await;
        let back_id = assert_config(&case, &back, resource_id, AUTODEFINED_REVERSE_ENABLING);
        assert_eq!(
            back_id, *id,
            "case {case:?}: walking the setting back keeps the configuration's Id, got {back}"
        );
    }

    settle().await;

    for (resource_id, _, _) in &written {
        let case = format!("re-enabled configuration of {resource_id}");
        let read = get(&router, resource_id).await;
        assert_config(&case, &read, resource_id, AUTODEFINED_REVERSE_ENABLED);
    }
}

/// Updating a VPC to the setting it already has moves nothing: a VPC at its
/// default stays settled (no transient status appears), and repeating an
/// in-flight update neither restarts the span the clock owes nor drives the
/// configuration back out of the status it is moving through.
#[tokio::test]
async fn an_update_to_the_setting_already_in_effect_moves_nothing() {
    let router = super::harness::emulator();
    router.hold_vpcs(["vpc-already-enabled", "vpc-in-flight"]);

    // A VPC at its default, asked for the setting it is already settled in.
    let settled_already = update(&router, "vpc-already-enabled", AUTODEFINED_REVERSE_FLAG_ENABLE).await;
    assert_config(
        "ENABLE on a VPC already enabled",
        &settled_already,
        "vpc-already-enabled",
        AUTODEFINED_REVERSE_ENABLED,
    );

    // A VPC with a move in flight, asked for the same setting again: half the
    // span the clock owes, then the very same request.
    let resource_id = "vpc-in-flight";
    let first = update(&router, resource_id, AUTODEFINED_REVERSE_FLAG_DISABLE).await;
    let id = assert_config(
        "the first DISABLE",
        &first,
        resource_id,
        AUTODEFINED_REVERSE_DISABLING,
    );

    tokio::time::sleep(PROVISIONING_DELAY / 2).await;
    let repeat = update(&router, resource_id, AUTODEFINED_REVERSE_FLAG_DISABLE).await;
    let repeat_id = assert_config(
        "the repeated DISABLE",
        &repeat,
        resource_id,
        AUTODEFINED_REVERSE_DISABLING,
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
        &read,
        resource_id,
        AUTODEFINED_REVERSE_DISABLED,
    );
}

/// An `AutodefinedReverseFlag` the modelled enum does not declare is refused,
/// and the refusal writes nothing: the VPC keeps the configuration it had.
///
/// The rows are one per way a value can fall outside the modelled enum: wrong
/// case, a value of the *status* enum rather than of the flag, the empty string,
/// and an unrelated word.
#[tokio::test]
async fn an_unmodelled_autodefined_reverse_flag_is_refused_and_stores_nothing() {
    let router = super::harness::emulator();
    let resource_id = "vpc-refused";
    router.hold_vpcs([resource_id]);

    update(&router, resource_id, AUTODEFINED_REVERSE_FLAG_DISABLE).await;
    settle().await;

    for case in ["disable", "DISABLED", "", "SOMETHING_ELSE"] {
        assert!(
            !AUTODEFINED_REVERSE_FLAG_VALUES.contains(&case),
            "case {case:?}: the case must not be a modelled AutodefinedReverseFlag value"
        );
        expect_error(
            &router,
            "UpdateResolverConfig",
            json!({ "ResourceId": resource_id, "AutodefinedReverseFlag": case }),
            case,
            "ValidationException",
        )
        .await;

        let read = get(&router, resource_id).await;
        assert_config(case, &read, resource_id, AUTODEFINED_REVERSE_DISABLED);
    }

    // Every value the enum does declare is accepted on that same VPC.
    for flag in AUTODEFINED_REVERSE_FLAG_VALUES {
        let updated = update(&router, resource_id, flag).await;
        assert!(
            updated.get("AutodefinedReverse").is_some(),
            "case {flag:?}: a modelled AutodefinedReverseFlag must be accepted, got {updated}"
        );
        settle().await;
    }
}

/// `ResourceId`'s modelled length (1..64) bounds BOTH operations: a value past
/// the maximum, or the empty string the minimum excludes, is refused. The
/// lengths inside the bound are served by
/// [`every_vpc_reports_a_resolver_configuration_at_its_documented_default`],
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
            "UpdateResolverConfig",
            json!({ "ResourceId": resource_id, "AutodefinedReverseFlag": AUTODEFINED_REVERSE_FLAG_DISABLE }),
            case,
            "ValidationException",
        )
        .await;
        expect_error(
            &router,
            "GetResolverConfig",
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
            "UpdateResolverConfig without ResourceId",
            "UpdateResolverConfig",
            json!({ "AutodefinedReverseFlag": AUTODEFINED_REVERSE_FLAG_DISABLE }),
        ),
        (
            "UpdateResolverConfig without AutodefinedReverseFlag",
            "UpdateResolverConfig",
            json!({ "ResourceId": "vpc-missing-member" }),
        ),
        (
            "GetResolverConfig without ResourceId",
            "GetResolverConfig",
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

    let read = get(&router, "vpc-missing-member").await;
    assert_config(
        "the VPC a refused update named",
        &read,
        "vpc-missing-member",
        AUTODEFINED_REVERSE_ENABLED,
    );
}

/// The store is bounded: a VPC that would need a record past the bound is
/// refused, while a VPC already stored is still updatable there, because
/// updating one allocates no record — and so is a request for the setting a
/// fresh VPC already has, which writes nothing at all.
#[tokio::test]
async fn the_configuration_store_is_bounded() {
    let router = super::harness::emulator();
    router.hold_vpcs(
        (0..MAX_STORED_RESOLVER_CONFIGS)
            .map(|index| format!("vpc-bound-{index}"))
            .chain([
                "vpc-one-too-many".to_string(),
                "vpc-at-the-bound-no-op".to_string(),
            ]),
    );

    for index in 0..MAX_STORED_RESOLVER_CONFIGS {
        update(
            &router,
            &format!("vpc-bound-{index}"),
            AUTODEFINED_REVERSE_FLAG_DISABLE,
        )
        .await;
    }

    expect_error(
        &router,
        "UpdateResolverConfig",
        json!({
            "ResourceId": "vpc-one-too-many",
            "AutodefinedReverseFlag": AUTODEFINED_REVERSE_FLAG_DISABLE,
        }),
        "one VPC past the bound",
        "LimitExceededException",
    )
    .await;

    // The refusal wrote nothing: that VPC still reports the default.
    let unwritten = get(&router, "vpc-one-too-many").await;
    assert_config(
        "the VPC the bound refused",
        &unwritten,
        "vpc-one-too-many",
        AUTODEFINED_REVERSE_ENABLED,
    );

    // A fresh VPC asked for the setting it already has needs no record, so the
    // bound does not refuse it.
    let no_op = update(
        &router,
        "vpc-at-the-bound-no-op",
        AUTODEFINED_REVERSE_FLAG_ENABLE,
    )
    .await;
    assert_config(
        "a no-op update at the bound",
        &no_op,
        "vpc-at-the-bound-no-op",
        AUTODEFINED_REVERSE_ENABLED,
    );

    // A VPC already stored stays updatable at the bound.
    let updated = update(&router, "vpc-bound-0", AUTODEFINED_REVERSE_FLAG_ENABLE).await;
    assert_config(
        "a VPC already stored, at the bound",
        &updated,
        "vpc-bound-0",
        AUTODEFINED_REVERSE_ENABLING,
    );
}
