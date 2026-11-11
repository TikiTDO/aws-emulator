// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeprovisionByoipCidr`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeprovisionByoipCidr`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test: `DeprovisionByoipCidr` RELEASES AN ADDRESS RANGE THE STORE HOLDS —
//! a `Cidr` that is not an address range is refused, a range the account has not
//! provisioned (a different block, or the same block at a different prefix length) is
//! refused as not found, a range whose `ByoipCidrState` is not one it can be released from
//! is refused as being in the wrong state, and otherwise the range moves to
//! `pending-deprovision` and the CLOCK ends the release — the range leaves the account, or
//! lands in `failed-deprovision` when the account still holds an Elastic IP address out of
//! it. A `DryRun` request is answered from the permission check alone, before any range is
//! looked up.
//!
//! Every table row is a distinct input along what that generalises over — which address
//! family, which prefix length, which of the modelled `ByoipCidrState` values the range is
//! in, whether the block is written in canonical form — so the recorded
//! "deprovision a range nobody provisioned" case is one row among many rather than the
//! specification. The ranges used are from the documentation blocks RFC 5737 and RFC 3849
//! set aside (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`, `2001:db8::/32`) and
//! from the RFC 2544 benchmarking block (`198.18.0.0/15`), so no row names an address range
//! that belongs to anybody.
//!
//! The store is seeded directly, the way `describe_vpcs` and `launch_templates` seed
//! theirs: these rows need a range in a chosen `ByoipCidrState`, including states no
//! provision can leave one in, which the API has no path to. The `ProvisionByoipCidr`
//! path a caller takes is covered by `byoip_cidrs` instead.
//!
//! Each refusal's MESSAGE is checked only for the substitution the service makes — the
//! value the request sent, named back in EC2's "the value you sent, in parentheses" form —
//! never for wording this emulator has no rule for.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::ops::types::BYOIP_CIDR_STATE_VALUES;
use ec2::state::{
    BYOIP_CIDR_STATE_ADVERTISED, BYOIP_CIDR_STATE_FAILED_DEPROVISION,
    BYOIP_CIDR_STATE_FAILED_PROVISION, BYOIP_CIDR_STATE_PENDING_DEPROVISION,
    BYOIP_CIDR_STATE_PROVISIONED, BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
    ByoipCidrRecord, ComAmazonawsEc2Amazonec2State as Ec2State, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, unescaped};

/// The Region every call here is signed for. A BYOIP range is named by the range itself and
/// this store holds the account's ranges rather than a Region's, so the Region only selects
/// the ec2Query envelope.
const REGION: &str = "us-west-2";

/// EC2's error code for an address range this account has not provisioned.
const NOT_FOUND: &str = "InvalidCidr.NotFound";

/// EC2's error code for a range whose current state does not admit the release.
const INCORRECT_STATE: &str = "IncorrectState";

/// EC2's error code for a `Cidr` that is not an address range at all.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's error code for a `Cidr` that reached the handler empty.
const MISSING_PARAMETER: &str = "MissingParameter";

/// The code the BOUNDARY answers for a `@required` member the request left out, before any
/// handler of this service runs.
const BOUNDARY_MISSING_MEMBER: &str = "MalformedInput";

/// EC2's error code for a `DryRun` request that would have been served.
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// The `ByoipCidrState`s this service releases a range from, as the rule names them and
/// INDEPENDENTLY of the service constant: a range that finished provisioning, and a range
/// whose provision or whose earlier release failed ("Ensure that all EIPs from the range
/// have been deallocated and try again" on `failed-deprovision`,
/// `smithy_query(query="shape",shape="ByoipCidrState")`). Every other value of that enum is
/// a range AWS is advertising or one part way through a move the clock has not finished.
///
/// What this list pins is MEMBERSHIP, not the spelling of each word — the words come from
/// the same `BYOIP_CIDR_STATE_*` constants the service uses, so a typo cannot make the
/// table pass by accident. Importing the service's own `RELEASABLE_BYOIP_CIDR_STATES` here
/// instead would make the check unable to fail.
const RELEASABLE_STATES: &[&str] = &[
    BYOIP_CIDR_STATE_PROVISIONED,
    BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
    BYOIP_CIDR_STATE_FAILED_PROVISION,
    BYOIP_CIDR_STATE_FAILED_DEPROVISION,
];

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `DeprovisionByoipCidr` call carrying `params`, as its HTTP status and XML body.
async fn deprovision(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DeprovisionByoipCidr", params).await
}

/// One `DeprovisionByoipCidr` call for `cidr`.
async fn deprovision_cidr(router: &MultiServiceRouter, cidr: &str) -> (u16, String) {
    deprovision(router, &[p("Cidr", cidr)]).await
}

/// Write `ranges` — `(cidr, state)` pairs — into the store as ranges the account holds.
fn seed(engine: &engine::InMemoryEngine, ranges: &[(&str, &str)]) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    for (cidr, state) in ranges {
        assert!(
            guard.insert_byoip_cidr(ByoipCidrRecord::in_state(cidr, state)),
            "the store has room for the seeded range {cidr}, so it must have been stored"
        );
    }
}

/// The `ByoipCidrState` the store holds for `cidr`, or `None` when it holds no such range.
/// Read straight from the store, so it reports what a later caller would be told without
/// a call of its own materialising anything.
fn stored_state(engine: &engine::InMemoryEngine, cidr: &str) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .byoip_cidrs
        .get(cidr)
        .map(|record| record.state().to_string())
}

/// Every range the store holds, by CIDR.
fn stored_cidrs(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.byoip_cidrs.keys().cloned().collect()
}

/// The `StatusMessage` the store holds for the range `cidr`.
fn stored_status_message(engine: &engine::InMemoryEngine, cidr: &str) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .byoip_cidrs
        .get(cidr)
        .and_then(|record| record.byoip_cidr.status_message.clone())
}

/// Allocate one Elastic IP address out of Amazon's pool through `AllocateAddress`, and
/// answer `(allocation id, public address)`.
async fn allocate_one_address(router: &MultiServiceRouter) -> (String, String) {
    let (status, body) = super::ec2_query::call(router, REGION, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "AllocateAddress: expected HTTP 200, got {status} with body {body}"
    );
    let allocation_id = element(&body, "allocationId").unwrap_or_default();
    let public_ip = element(&body, "publicIp").unwrap_or_default();
    assert!(
        !allocation_id.is_empty() && !public_ip.is_empty(),
        "AllocateAddress: expected an allocation id and an address, got body {body}"
    );
    (allocation_id, public_ip)
}

/// The `/24` that contains `public_ip`, and a `/24` that does not.
fn covering_and_neighbouring_range(public_ip: &str) -> (String, String) {
    let octets = public_ip
        .parse::<std::net::Ipv4Addr>()
        .unwrap_or_else(|_| panic!("AllocateAddress answered the address {public_ip:?}"))
        .octets();
    (
        format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2]),
        format!(
            "{}.{}.{}.0/24",
            octets[0],
            octets[1],
            octets[2].wrapping_add(1)
        ),
    )
}

/// Wait until a release started before now has certainly fallen due. Derived from the
/// service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// A range the account has not provisioned
// ---------------------------------------------------------------------------

/// "Releases the specified address range that you provisioned" — a range this account does
/// not hold is a refusal, and the prefix is part of the range's name: "The prefix must be
/// the same prefix that you specified when you provisioned the address range"
/// (`DeprovisionByoipCidrRequest$Cidr`). Nothing the store holds moves.
#[tokio::test]
async fn an_address_range_the_account_does_not_hold_is_refused_as_not_found() {
    // `(case, the Cidr under test)`, against a store that holds 192.0.2.0/24 only.
    let rows: &[(&str, &str)] = &[
        ("another IPv4 /24", "203.0.113.0/24"),
        ("an IPv4 /16", "198.51.0.0/16"),
        ("an IPv6 /48", "2001:db8:1::/48"),
        ("a /25 inside the range the account holds", "192.0.2.0/25"),
        ("a /23 containing the range the account holds", "192.0.2.0/23"),
        ("the neighbouring /24", "192.0.3.0/24"),
    ];

    let (router, engine) = emulator();
    seed(&engine, &[("192.0.2.0/24", BYOIP_CIDR_STATE_PROVISIONED)]);

    for (case, cidr) in rows {
        let (status, body) = deprovision_cidr(&router, cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {cidr}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} for {cidr}, got {code:?} with body {body}"
        );
        // The message is built from the request: it names the range the caller asked about.
        let message = element(&body, "Message").map(|text| unescaped(&text));
        assert_eq!(
            message.as_deref(),
            Some(format!("The CIDR ({cidr}) does not exist.").as_str()),
            "case {case:?}: expected the message to name {cidr}, got {message:?}"
        );
        assert_eq!(
            stored_cidrs(&engine),
            vec!["192.0.2.0/24".to_string()],
            "case {case:?}: a refusal must leave the store alone, got {:?}",
            stored_cidrs(&engine)
        );
        assert_eq!(
            stored_state(&engine, "192.0.2.0/24").as_deref(),
            Some(BYOIP_CIDR_STATE_PROVISIONED),
            "case {case:?}: the range the account holds must not move, got {:?}",
            stored_state(&engine, "192.0.2.0/24")
        );
    }
}

// ---------------------------------------------------------------------------
// A `Cidr` that is not an address range
// ---------------------------------------------------------------------------

/// "The address range, in CIDR notation" — a value that is not an address range of either
/// family names nothing to release, and a request carrying no `Cidr` at all is missing a
/// parameter rather than naming an unknown range (the member is modelled `clientOptional`,
/// so it reaches the handler as an empty string).
#[tokio::test]
async fn a_cidr_that_is_not_an_address_range_is_refused_before_any_lookup() {
    // `(case, the parameters, the expected error code)`.
    let rows: Vec<(&str, Params, &str)> = vec![
        (
            "not an address range at all",
            vec![p("Cidr", "not-an-address-range")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an address with no prefix length",
            vec![p("Cidr", "192.0.2.0")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv4 prefix length past 32",
            vec![p("Cidr", "192.0.2.0/33")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv6 prefix length past 128",
            vec![p("Cidr", "2001:db8::/129")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a prefix length that is not a number",
            vec![p("Cidr", "192.0.2.0/twenty-four")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an address range with a host part but no address",
            vec![p("Cidr", "/24")],
            INVALID_PARAMETER_VALUE,
        ),
        ("an empty Cidr", vec![p("Cidr", "")], MISSING_PARAMETER),
        // `Cidr` is modelled `@required`, so a request that carries no `Cidr` member at
        // all is refused by the boundary validator before any handler logic runs, with the
        // boundary's own code rather than the handler's.
        (
            "no Cidr parameter at all",
            Vec::new(),
            BOUNDARY_MISSING_MEMBER,
        ),
    ];

    let (router, engine) = emulator();
    seed(&engine, &[("192.0.2.0/24", BYOIP_CIDR_STATE_PROVISIONED)]);

    for (case, params, expected) in rows {
        let (status, body) = deprovision(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?} with body {body}"
        );
        // A refusal of a value the handler read names that value back, built from the
        // request — the boundary validator writes its own text, so only the handler's rows
        // are checked for it.
        if expected == INVALID_PARAMETER_VALUE {
            let sent = params
                .iter()
                .find(|(key, _)| key == "Cidr")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let message = element(&body, "Message").map(|text| unescaped(&text));
            assert_eq!(
                message.as_deref(),
                Some(format!("The CIDR ({sent}) is not valid.").as_str()),
                "case {case:?}: expected the message to name {sent:?}, got {message:?}"
            );
        }
        assert_eq!(
            stored_state(&engine, "192.0.2.0/24").as_deref(),
            Some(BYOIP_CIDR_STATE_PROVISIONED),
            "case {case:?}: a refusal must leave the store alone, got {:?}",
            stored_state(&engine, "192.0.2.0/24")
        );
    }
}

// ---------------------------------------------------------------------------
// Which states a range can be released from
// ---------------------------------------------------------------------------

/// "Before you can release an address range, you must stop advertising it and you must not
/// have any IP addresses allocated from its address range" (`DeprovisionByoipCidr`): a
/// range is releasable exactly when it is the account's and sitting still, and every other
/// `ByoipCidrState` is refused with EC2's `IncorrectState` and left where it was.
///
/// The row set is the WHOLE modelled catalog — `ByoipCidrState`'s own values
/// (`BYOIP_CIDR_STATE_VALUES`, `smithy_query(query="shape",shape="ByoipCidrState")`) — so
/// every state the model admits is exercised, not only the ones the rule accepts.
#[tokio::test]
async fn a_range_is_released_only_from_a_state_it_can_be_released_from() {
    let catalog: BTreeSet<&str> = BYOIP_CIDR_STATE_VALUES.iter().copied().collect();
    for state in RELEASABLE_STATES {
        assert!(
            catalog.contains(state),
            "releasable state {state:?} must be one of the modelled ByoipCidrState values, \
             got the catalog {catalog:?}"
        );
    }
    assert!(
        catalog.len() > RELEASABLE_STATES.len(),
        "the catalog must hold states a range cannot be released from too, got {catalog:?}"
    );

    for (index, state) in BYOIP_CIDR_STATE_VALUES.iter().enumerate() {
        // One range per row, from the RFC 2544 benchmarking block, so a row that releases
        // its range cannot affect the next one.
        let cidr = format!("198.18.{index}.0/24");
        let (router, engine) = emulator();
        seed(&engine, &[(cidr.as_str(), state)]);

        let (status, body) = deprovision_cidr(&router, &cidr).await;
        if RELEASABLE_STATES.contains(state) {
            assert_eq!(
                status, 200,
                "case {state:?}: expected HTTP 200 releasing {cidr}, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "cidr").as_deref(),
                Some(cidr.as_str()),
                "case {state:?}: expected the released range reported back, got body {body}"
            );
            assert_eq!(
                element(&body, "state").as_deref(),
                Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION),
                "case {state:?}: expected the range reported as \
                 {BYOIP_CIDR_STATE_PENDING_DEPROVISION}, got body {body}"
            );
            assert_eq!(
                stored_state(&engine, &cidr).as_deref(),
                Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION),
                "case {state:?}: expected the store to hold the range as \
                 {BYOIP_CIDR_STATE_PENDING_DEPROVISION}, got {:?}",
                stored_state(&engine, &cidr)
            );
        } else {
            assert_eq!(
                status, 400,
                "case {state:?}: expected HTTP 400 releasing {cidr}, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, INCORRECT_STATE,
                "case {state:?}: expected {INCORRECT_STATE} releasing {cidr}, \
                 got {code:?} with body {body}"
            );
            assert_eq!(
                stored_state(&engine, &cidr).as_deref(),
                Some(*state),
                "case {state:?}: a refusal must leave the range where it was, got {:?}",
                stored_state(&engine, &cidr)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The release is the clock's to finish
// ---------------------------------------------------------------------------

/// "pending-deprovision: You've submitted a request to deprovision an address range and
/// it's pending" (`ByoipCidrState`) — so the release is NOT done when the call returns: the
/// range is still the account's, in a state it cannot be released from again, and the clock
/// is what takes it out of the account. Once it has, the same request finds no range.
#[tokio::test]
async fn a_started_release_is_finished_by_the_clock_and_not_by_a_later_caller() {
    let cidr = "198.51.100.0/24";
    let (router, engine) = emulator();
    seed(&engine, &[(cidr, BYOIP_CIDR_STATE_PROVISIONED)]);

    let (status, body) = deprovision_cidr(&router, cidr).await;
    assert_eq!(
        status, 200,
        "release: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION),
        "release: expected the range reported as {BYOIP_CIDR_STATE_PENDING_DEPROVISION}, \
         got body {body}"
    );
    assert_eq!(
        stored_state(&engine, cidr).as_deref(),
        Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION),
        "release: expected the store to still hold the range, got {:?}",
        stored_state(&engine, cidr)
    );

    // A caller acting straight after the call still sees the transient state and its state
    // error: the span is never zero.
    let (status, body) = deprovision_cidr(&router, cidr).await;
    assert_eq!(
        status, 400,
        "second release: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INCORRECT_STATE,
        "second release: expected {INCORRECT_STATE}, got {code:?} with body {body}"
    );

    past_the_transition().await;

    let (status, body) = deprovision_cidr(&router, cidr).await;
    assert_eq!(
        status, 400,
        "release after the clock: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, NOT_FOUND,
        "release after the clock: expected {NOT_FOUND}, got {code:?} with body {body}"
    );
    assert_eq!(
        stored_cidrs(&engine),
        Vec::<String>::new(),
        "release after the clock: the range must have left the account, got {:?}",
        stored_cidrs(&engine)
    );
}

// ---------------------------------------------------------------------------
// Which written form of a range names it
// ---------------------------------------------------------------------------

/// A range is named by the block it covers, not by the characters the request wrote it with:
/// the same block with its host bits set, or an IPv6 block written with its zeros spelled
/// out, names the range the account holds, while the same base address at a DIFFERENT prefix
/// length is a different range — "The prefix must be the same prefix that you specified when
/// you provisioned the address range" (`DeprovisionByoipCidrRequest$Cidr`).
#[tokio::test]
async fn a_range_is_named_by_its_block_and_its_prefix_length() {
    // `(case, the seeded range, the Cidr under test, the expected status)`.
    let rows: &[(&str, &str, &str, u16)] = &[
        ("the block as provisioned", "192.0.2.0/24", "192.0.2.0/24", 200),
        ("the block with host bits set", "192.0.2.0/24", "192.0.2.5/24", 200),
        (
            "an IPv6 block with its zeros spelled out",
            "2001:db8::/32",
            "2001:0db8:0000:0000:0000:0000:0000:0000/32",
            200,
        ),
        ("an IPv6 block with host bits set", "2001:db8::/32", "2001:db8:1:2::9/32", 200),
        ("a longer prefix length", "192.0.2.0/24", "192.0.2.0/25", 400),
        ("a shorter prefix length", "192.0.2.0/24", "192.0.2.0/22", 400),
        ("an IPv6 range at another prefix length", "2001:db8::/32", "2001:db8::/48", 400),
    ];

    for (case, seeded, requested, expected_status) in rows {
        let (router, engine) = emulator();
        seed(&engine, &[(seeded, BYOIP_CIDR_STATE_PROVISIONED)]);

        let (status, body) = deprovision_cidr(&router, requested).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} for {requested} against {seeded}, \
             got {status} with body {body}"
        );
        if *expected_status == 200 {
            assert_eq!(
                element(&body, "cidr").as_deref(),
                Some(*seeded),
                "case {case:?}: expected the range reported in the form it is held, got {body}"
            );
            assert_eq!(
                stored_state(&engine, seeded).as_deref(),
                Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION),
                "case {case:?}: expected the seeded range to be releasing, got {:?}",
                stored_state(&engine, seeded)
            );
        } else {
            let code = error_code(&body);
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND}, got {code:?} with body {body}"
            );
            assert_eq!(
                stored_state(&engine, seeded).as_deref(),
                Some(BYOIP_CIDR_STATE_PROVISIONED),
                "case {case:?}: a refusal must leave the seeded range alone, got {:?}",
                stored_state(&engine, seeded)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// "Checks whether you have the required permissions for the action, without actually
/// making the request, and provides an error response. If you have the required permissions,
/// the error response is DryRunOperation" (`DeprovisionByoipCidrRequest$DryRun`): what is
/// checked is the CALLER's permissions, not the account's ranges, so the answer is
/// `DryRunOperation` for a range the account holds and for one it does not alike, in either
/// family, and no range moves. A `Cidr` that is no address range at all is still refused for
/// its value: `DryRun` is about permissions, not about the request's syntax.
#[tokio::test]
async fn a_dry_run_is_answered_from_the_permission_check_whatever_the_store_holds() {
    // `(case, the parameters under test)`.
    let rows: Vec<(&str, Params)> = vec![
        (
            "a range the account holds",
            vec![p("Cidr", "192.0.2.0/24"), p("DryRun", "true")],
        ),
        (
            "a range the account is advertising",
            vec![p("Cidr", "203.0.113.0/24"), p("DryRun", "true")],
        ),
        (
            "a range the account has not provisioned",
            vec![p("Cidr", "198.51.100.0/24"), p("DryRun", "true")],
        ),
        (
            "an IPv6 range the account has not provisioned",
            vec![p("Cidr", "2001:db8::/32"), p("DryRun", "true")],
        ),
        (
            "a range written with host bits set",
            vec![p("Cidr", "192.0.2.7/24"), p("DryRun", "true")],
        ),
    ];

    let seeded: &[(&str, &str)] = &[
        ("192.0.2.0/24", BYOIP_CIDR_STATE_PROVISIONED),
        ("203.0.113.0/24", BYOIP_CIDR_STATE_ADVERTISED),
    ];
    let (router, engine) = emulator();
    seed(&engine, seeded);

    for (case, params) in rows {
        let (status, body) = deprovision(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {code:?} with body {body}"
        );
        for (cidr, state) in seeded {
            assert_eq!(
                stored_state(&engine, cidr).as_deref(),
                Some(*state),
                "case {case:?}: a dry run must not move {cidr}, got {:?}",
                stored_state(&engine, cidr)
            );
        }
    }

    // A value that is no address range is refused for the VALUE, dry run or not.
    for (case, params) in [
        (
            "a dry run of a Cidr that is not an address range",
            vec![p("Cidr", "not-an-address-range"), p("DryRun", "true")],
        ),
        (
            "a dry run with an empty Cidr",
            vec![p("Cidr", ""), p("DryRun", "true")],
        ),
    ] {
        let expected = if params[0].1.is_empty() {
            MISSING_PARAMETER
        } else {
            INVALID_PARAMETER_VALUE
        };
        let (status, body) = deprovision(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?} with body {body}"
        );
    }

    // `DryRun=false` is not a dry run: the request is served.
    let (status, body) = deprovision(
        &router,
        &[p("Cidr", "192.0.2.0/24"), p("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION),
        "DryRun=false: expected the range reported as {BYOIP_CIDR_STATE_PENDING_DEPROVISION}, \
         got body {body}"
    );
}

// ---------------------------------------------------------------------------
// Addresses still allocated out of the range
// ---------------------------------------------------------------------------

/// "... and you must not have any IP addresses allocated from its address range"
/// (`DeprovisionByoipCidr`): AWS takes the request anyway and reports the outcome on the
/// range — "failed-deprovision: The request to deprovision the address range was
/// unsuccessful. Ensure that all EIPs from the range have been deallocated and try again"
/// (`ByoipCidrState`). So a range the account still holds an Elastic IP address out of is
/// accepted, lands in `failed-deprovision` with a `StatusMessage` naming what held it, stays
/// the account's, and can be released again; a range holding no address of the account is
/// released and leaves.
///
/// The address is allocated through `AllocateAddress`, so the two operations read the one
/// address store rather than the test asserting against a copy of it.
#[tokio::test]
async fn a_range_with_an_address_still_allocated_from_it_fails_its_release() {
    let (router, engine) = emulator();
    let (allocation_id, public_ip) = allocate_one_address(&router).await;
    let (covering, neighbouring) = covering_and_neighbouring_range(&public_ip);
    seed(
        &engine,
        &[
            (covering.as_str(), BYOIP_CIDR_STATE_PROVISIONED),
            (neighbouring.as_str(), BYOIP_CIDR_STATE_PROVISIONED),
        ],
    );

    // Both releases START: the address decides how the release ENDS, not whether it begins.
    for cidr in [&covering, &neighbouring] {
        let (status, body) = deprovision_cidr(&router, cidr).await;
        assert_eq!(
            status, 200,
            "release of {cidr}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION),
            "release of {cidr}: expected {BYOIP_CIDR_STATE_PENDING_DEPROVISION}, got body {body}"
        );
    }

    past_the_transition().await;

    // A call about neither range is what makes the clock's work visible: the outcome was
    // owed at its due instant whether or not anybody asked about these ranges.
    let (status, body) = deprovision_cidr(&router, "203.0.113.0/24").await;
    assert_eq!(
        status, 400,
        "a call about another range: expected HTTP 400, got {status} with body {body}"
    );

    assert_eq!(
        stored_state(&engine, &covering).as_deref(),
        Some(BYOIP_CIDR_STATE_FAILED_DEPROVISION),
        "the range holding {public_ip}: expected {BYOIP_CIDR_STATE_FAILED_DEPROVISION}, got {:?}",
        stored_state(&engine, &covering)
    );
    let message = stored_status_message(&engine, &covering).unwrap_or_default();
    assert!(
        message.contains(&allocation_id),
        "the range holding {public_ip}: expected its status message to name {allocation_id}, \
         got {message:?}"
    );
    assert_eq!(
        stored_cidrs(&engine),
        vec![covering.clone()],
        "the range holding no address of the account must have left, got {:?}",
        stored_cidrs(&engine)
    );

    // "try again": `failed-deprovision` is a state a range can be released from, and the
    // retry clears the message of the attempt that failed.
    let (status, body) = deprovision_cidr(&router, &covering).await;
    assert_eq!(
        status, 200,
        "retrying the failed release: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION),
        "retrying the failed release: expected {BYOIP_CIDR_STATE_PENDING_DEPROVISION}, \
         got body {body}"
    );
    assert_eq!(
        stored_status_message(&engine, &covering),
        None,
        "retrying the failed release: expected the earlier message cleared, got {:?}",
        stored_status_message(&engine, &covering)
    );

    // The outcome is decided when the release ENDS, not when it starts: give the address
    // back while the range is `pending-deprovision` and the retry under way succeeds.
    // (The allocation is dropped straight from the store, the way the ranges here are
    // seeded, so this suite drives only the BYOIP calls it is about.)
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard.addresses.remove(&allocation_id);
    }
    past_the_transition().await;
    let (status, body) = deprovision_cidr(&router, "203.0.113.0/24").await;
    assert_eq!(
        status, 400,
        "a call about another range: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        stored_cidrs(&engine),
        Vec::<String>::new(),
        "the release must succeed once the address is back: expected no range left, got {:?}",
        stored_cidrs(&engine)
    );
}

/// A range carrying a `Description` of `description_bytes` characters — the member a
/// provision's charge against the store's byte budget is dominated by.
fn range_with_description(cidr: &str, description_bytes: usize) -> ByoipCidrRecord {
    let mut record = ByoipCidrRecord::in_state(cidr, BYOIP_CIDR_STATE_PROVISIONED);
    record.byoip_cidr.description = Some("d".repeat(description_bytes));
    record
}

/// The byte budget (`ec2::state::MAX_STORED_BYTES`) is asked about EVERY write, and about
/// the NET change when a write replaces a range the store already holds: re-provisioning one
/// range stores no further record, so the record ceiling cannot bound it, and the charge is
/// dominated by caller-chosen text.
///
/// The rows are the steps of one sequence, differing in the direction of the net change — a
/// first write, a replacement that holds less than the record it replaces, a replacement
/// that would cross the budget, and a further small range that still fits — and each one
/// states whether it must be stored. A refused write must leave the store exactly as it was.
#[test]
fn a_replacement_is_charged_the_net_change_against_the_byte_budget() {
    let (_router, engine) = emulator();
    let store = engine.state::<Ec2State>();
    let half = ec2::state::MAX_STORED_BYTES / 2;
    // (case, the range, the description's size in characters, whether it must be stored)
    let steps: [(&str, &str, usize, bool); 4] = [
        (
            "a first range holding about half the budget",
            "198.51.100.0/24",
            half,
            true,
        ),
        (
            "a replacement holding less than the range it replaces",
            "198.51.100.0/24",
            half - 1024,
            true,
        ),
        (
            "a replacement that would take the store past the budget",
            "198.51.100.0/24",
            ec2::state::MAX_STORED_BYTES,
            false,
        ),
        (
            "a further small range, which the budget still has room for",
            "203.0.113.0/24",
            1024,
            true,
        ),
    ];

    for (case, cidr, description_bytes, stored_expected) in steps {
        let (bytes_before, held_before) = {
            let guard = store.lock();
            (
                guard.stored_bytes(),
                guard
                    .byoip_cidrs
                    .get(cidr)
                    .and_then(|record| record.byoip_cidr.description.clone()),
            )
        };
        let stored = store
            .lock()
            .insert_byoip_cidr(range_with_description(cidr, description_bytes));
        assert_eq!(
            stored, stored_expected,
            "case {case:?}: expected a {description_bytes}-character description to be \
             stored={stored_expected}, got {stored}"
        );

        let (bytes_after, held_after) = {
            let guard = store.lock();
            (
                guard.stored_bytes(),
                guard
                    .byoip_cidrs
                    .get(cidr)
                    .and_then(|record| record.byoip_cidr.description.clone()),
            )
        };
        assert!(
            bytes_after <= ec2::state::MAX_STORED_BYTES,
            "case {case:?}: expected at most {} stored bytes, got {bytes_after}",
            ec2::state::MAX_STORED_BYTES
        );
        if stored_expected {
            assert_eq!(
                held_after.as_deref().map(str::len),
                Some(description_bytes),
                "case {case:?}: expected the stored range to carry the new description, \
                 got {:?} characters",
                held_after.as_deref().map(str::len)
            );
        } else {
            assert_eq!(
                bytes_after, bytes_before,
                "case {case:?}: a refused write must charge nothing, got {bytes_after} \
                 after {bytes_before}"
            );
            assert_eq!(
                held_after.as_deref().map(str::len),
                held_before.as_deref().map(str::len),
                "case {case:?}: a refused write must leave the stored range as it was, \
                 got {:?} after {:?}",
                held_after.as_deref().map(str::len),
                held_before.as_deref().map(str::len)
            );
        }
    }
}
