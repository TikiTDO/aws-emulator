// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListResolverConfigs`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].
//!
//! What these cases pin is the rule the operation follows: a VPC's Resolver
//! behavior configuration is a per-VPC singleton the service maintains itself,
//! so the listing reports ONE entry per VPC of the account — the very
//! configuration `GetResolverConfig` reports for that VPC, at the status the
//! clock owes it — paged by the `MaxResults` range the model puts on THIS
//! operation and resumed only by a cursor this service minted for it. Which
//! VPCs the account HAS is a fact of its own Amazon EC2 inventory, which
//! `vpc_configuration_inventory` pins; these cases take the VPCs that inventory
//! already holds as the collection's starting point.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    service_state, AUTODEFINED_REVERSE_DISABLED, AUTODEFINED_REVERSE_DISABLING,
    AUTODEFINED_REVERSE_ENABLED, AUTODEFINED_REVERSE_ENABLING,
    AUTODEFINED_REVERSE_FLAG_DISABLE, AUTODEFINED_REVERSE_FLAG_ENABLE,
    AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING,
    AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
    AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING, PROVISIONING_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, emulator, emulator_on, error_type, ok, string_member, Emulator, ACCOUNT,
};

/// The bounds of `ListResolverConfigs$MaxResults`, from the range the model puts
/// on it (`smithy_query(query="operation",operation="ListResolverConfigs")`:
/// `ListResolverConfigsMaxResult`, range 5..100).
const MAX_RESULTS_BOUNDS: (i64, i64) = (5, 100);

/// Wait out the span the clock owes a configuration whose autodefined-reverse
/// status is still in flight, so the next call sees the settled status.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

/// The `ResolverConfigs` of one page.
fn page_items(body: &Value) -> Vec<Value> {
    body.get("ResolverConfigs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("the response carries a ResolverConfigs array, got {body}"))
}

/// Walk every page of `ListResolverConfigs` for `request`, returning the items
/// in the order they were reported.
async fn walk(router: &MultiServiceRouter, request: &Value, case: &str) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut body = request.clone();
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(router, "ListResolverConfigs", body).await;
        pages += 1;
        assert!(
            pages <= 200,
            "case {case:?}: paging did not terminate after {pages} pages"
        );

        let items = page_items(&page);
        if let Some(size) = request.get("MaxResults").and_then(Value::as_i64) {
            assert!(
                i64::try_from(items.len()).unwrap_or(i64::MAX) <= size,
                "case {case:?}: a page must hold at most MaxResults items, got {page}"
            );
        }
        walked.extend(items);

        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    walked
}

/// Read one VPC's Resolver behavior configuration through `GetResolverConfig`.
async fn get_config(router: &Emulator, resource_id: &str) -> Value {
    let body = ok(
        router,
        "GetResolverConfig",
        json!({ "ResourceId": resource_id }),
    )
    .await;
    body.get("ResolverConfig")
        .expect("the read carries a ResolverConfig")
        .clone()
}

/// Move one VPC's autodefined-reverse setting through `UpdateResolverConfig`.
///
/// The VPC is registered in the account's own Amazon EC2 store first: a per-VPC
/// configuration belongs to a VPC of the account, which the write resolves
/// against that store — the same way [`associate_rule_with_vpc`] registers the
/// VPC a rule is associated with.
async fn update_resolver_config(router: &Emulator, resource_id: &str, flag: &str) {
    router.hold_vpcs([resource_id]);
    ok(
        router,
        "UpdateResolverConfig",
        json!({ "ResourceId": resource_id, "AutodefinedReverseFlag": flag }),
    )
    .await;
}

/// The listed entry for `resource_id`, or `None` when the listing does not carry
/// one.
fn listed_entry(listing: &[Value], resource_id: &str) -> Option<Value> {
    let matching: Vec<&Value> = listing
        .iter()
        .filter(|item| string_member(item, "ResourceId") == resource_id)
        .collect();
    assert!(
        matching.len() <= 1,
        "the listing must carry at most one configuration per VPC, got {} for {resource_id:?}",
        matching.len()
    );
    matching.first().map(|item| (*item).clone())
}

/// Create an OUTBOUND Resolver endpoint on `subnets`, and return
/// `(endpoint id, HostVPCId)`.
///
/// The subnets are registered in the account's own Amazon EC2 store first: an
/// endpoint's addresses are placed in subnets of the caller's VPC, which the
/// create resolves against that store.
async fn outbound_endpoint(
    router: &Emulator,
    creator: &str,
    subnets: (&str, &str),
) -> (String, String) {
    router.hold_subnets([subnets.0, subnets.1]);
    let created = ok(
        router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": creator,
            "Direction": "OUTBOUND",
            "SecurityGroupIds": ["sg-00000000000000001"],
            "IpAddresses": [
                { "SubnetId": subnets.0, "Ip": "10.0.0.10" },
                { "SubnetId": subnets.1, "Ip": "10.0.1.10" },
            ],
        }),
    )
    .await;
    let endpoint = created
        .get("ResolverEndpoint")
        .expect("the create answers a ResolverEndpoint");
    (
        string_member(endpoint, "Id"),
        string_member(endpoint, "HostVPCId"),
    )
}

/// Create a FORWARD rule through `endpoint` and associate it with `vpc`.
///
/// The VPC is registered in the account's own Amazon EC2 store first: a rule is
/// associated WITH a VPC of the account, which the associate resolves against
/// that store.
async fn associate_rule_with_vpc(router: &Emulator, endpoint: &str, vpc: &str) {
    router.hold_vpcs([vpc]);
    let created = ok(
        router,
        "CreateResolverRule",
        json!({
            "CreatorRequestId": format!("rule-for-{vpc}"),
            "Name": "listed-vpc-rule",
            "RuleType": "FORWARD",
            "DomainName": "example.com",
            "ResolverEndpointId": endpoint,
            "TargetIps": [{ "Ip": "192.0.2.10" }],
        }),
    )
    .await;
    let rule = string_member(
        created
            .get("ResolverRule")
            .expect("the create answers a ResolverRule"),
        "Id",
    );
    ok(
        router,
        "AssociateResolverRule",
        json!({ "ResolverRuleId": rule, "VPCId": vpc, "Name": format!("{vpc}-association") }),
    )
    .await;
}

/// Reading a VPC's configuration creates nothing: the collection is the VPCs
/// the account HOLDS — the ambient default VPC of its Region to begin with — and
/// a read of a configuration for a VPC the account does not hold leaves that
/// collection exactly as it was.
#[tokio::test]
async fn reading_a_configuration_adds_no_vpc_to_the_listing() {
    let router = emulator();

    // Rows spanning the forms of VPC id `ResourceId` (length 1..64) admits, so
    // what is pinned is "a read registers nothing" and not one id's treatment.
    // Each one is a VPC the account holds, which is what the read resolves its
    // `ResourceId` against.
    let rows: Vec<(&str, String)> = vec![
        ("the 8-hex-digit form", "vpc-1a2b3c4d".to_string()),
        ("the 17-hex-digit form", "vpc-0abcdef1234567890".to_string()),
        ("one character, the modelled minimum", "q".to_string()),
        (
            "64 characters, the modelled maximum",
            format!("vpc-{}", "7".repeat(60)),
        ),
        ("an id in no form AWS mints", "my.private.network".to_string()),
    ];
    router.hold_vpcs(rows.iter().map(|(_, resource_id)| resource_id));

    // The collection an account starts with: the VPCs its own Amazon EC2
    // inventory holds, reported as a list and with no cursor owed.
    let page = ok(&router, "ListResolverConfigs", json!({})).await;
    let ambient = page_items(&page);
    assert_eq!(
        ambient
            .iter()
            .map(|item| string_member(item, "ResourceId"))
            .collect::<Vec<String>>(),
        router.held_vpc_ids(),
        "case \"a store nobody wrote to\": the listing must report the VPCs the account holds, got {page}"
    );
    assert_eq!(
        page.get("NextToken"),
        None,
        "case \"a store nobody wrote to\": no cursor is owed on a page that ends the collection, got {page}"
    );

    for (case, resource_id) in rows {
        let read = get_config(&router, &resource_id).await;
        assert_eq!(
            string_member(&read, "AutodefinedReverse"),
            AUTODEFINED_REVERSE_ENABLED,
            "case {case:?}: an untouched VPC reads at its documented default, got {read}"
        );

        let walked = walk(&router, &json!({}), case).await;
        assert_eq!(
            walked, ambient,
            "case {case:?}: reading a configuration must not add a VPC to the listing, got {walked:?}"
        );
    }
}

/// Every VPC the service knows the account has is listed exactly once, with the
/// configuration its reader reports for that VPC, and the entries stay in the
/// order the service came to know of the VPCs.
///
/// The rows span what the rule generalises over — HOW the service came to know
/// of the VPC, which is every way one of its records can name one: a Resolver
/// behaviour configuration written for it, a DNS Firewall configuration, a
/// DNSSEC validation configuration, a Resolver rule associated with it, and a
/// Resolver endpoint hosted in it. The setting a row leaves untouched must still
/// be reported at its documented default.
#[tokio::test]
async fn every_vpc_the_service_knows_of_is_listed_once_with_its_own_configuration() {
    let router = emulator();
    let (endpoint, host_vpc) =
        outbound_endpoint(&router, "endpoint-for-listing", ("subnet-bbbb1", "subnet-bbbb2")).await;

    // The VPCs in the order the service comes to know of them, paired with the
    // autodefined-reverse status each must report once the move it asked for has
    // settled.
    let mut expected: Vec<(String, String, String)> = vec![(
        "a Resolver endpoint hosted in it".to_string(),
        host_vpc.clone(),
        AUTODEFINED_REVERSE_ENABLED.to_string(),
    )];

    update_resolver_config(&router, "vpc-00000000000000021", AUTODEFINED_REVERSE_FLAG_DISABLE).await;
    expected.push((
        "a Resolver behaviour configuration written for it".to_string(),
        "vpc-00000000000000021".to_string(),
        AUTODEFINED_REVERSE_DISABLED.to_string(),
    ));

    router.hold_vpcs(["vpc-00000000000000022"]);
    ok(
        &router,
        "UpdateFirewallConfig",
        json!({ "ResourceId": "vpc-00000000000000022", "FirewallFailOpen": "ENABLED" }),
    )
    .await;
    expected.push((
        "a DNS Firewall configuration written for it".to_string(),
        "vpc-00000000000000022".to_string(),
        AUTODEFINED_REVERSE_ENABLED.to_string(),
    ));

    router.hold_vpcs(["vpc-00000000000000023"]);
    ok(
        &router,
        "UpdateResolverDnssecConfig",
        json!({ "ResourceId": "vpc-00000000000000023", "Validation": "ENABLE" }),
    )
    .await;
    expected.push((
        "a DNSSEC validation configuration written for it".to_string(),
        "vpc-00000000000000023".to_string(),
        AUTODEFINED_REVERSE_ENABLED.to_string(),
    ));

    associate_rule_with_vpc(&router, &endpoint, "vpc-00000000000000024").await;
    expected.push((
        "a Resolver rule associated with it".to_string(),
        "vpc-00000000000000024".to_string(),
        AUTODEFINED_REVERSE_ENABLED.to_string(),
    ));

    // Past the due instant of the one move asked for above, so the status every
    // entry reports is settled and the listing can be compared with the reader
    // without the clock moving between the two calls.
    settle().await;

    let listed = walk(&router, &json!({}), "every known VPC").await;
    assert_eq!(
        listed.len(),
        expected.len(),
        "case \"every known VPC\": the listing must report one configuration per known VPC, got {listed:?}"
    );

    for (case, resource_id, autodefined_reverse) in &expected {
        let entry = listed_entry(&listed, resource_id).unwrap_or_else(|| {
            panic!("case {case:?}: {resource_id:?} must be listed, got {listed:?}")
        });
        assert_eq!(
            string_member(&entry, "AutodefinedReverse"),
            *autodefined_reverse,
            "case {case:?}: the listed setting must be the VPC's, got {entry}"
        );
        assert_eq!(
            string_member(&entry, "OwnerId"),
            ACCOUNT,
            "case {case:?}: OwnerId must be the account that owns the VPC, got {entry}"
        );
        let id = string_member(&entry, "Id");
        assert!(
            id.starts_with("rslvr-rc-") && !id.is_empty() && id.len() <= 64,
            "case {case:?}: a configuration Id is rslvr-rc-<hex> inside ResourceId's length 1..64, got {id:?}"
        );

        // The listed entry is the very configuration the reader reports for that
        // VPC, so one VPC has ONE configuration across the service.
        let read = get_config(&router, resource_id).await;
        assert_eq!(
            entry, read,
            "case {case:?}: the listing must agree with GetResolverConfig, got {entry} then {read}"
        );
    }

    // The order is the account's own inventory first, in the order that
    // inventory reports it, and then the VPCs only this service's records name,
    // in the order the service came to know of them — so a VPC the service
    // learns of later only appends past what a caller already read.
    let order: Vec<String> = listed
        .iter()
        .map(|item| string_member(item, "ResourceId"))
        .collect();
    let held = router.held_vpc_ids();
    let mut by_rule: Vec<String> = held.clone();
    by_rule.extend(
        expected
            .iter()
            .map(|(_, resource_id, _)| resource_id.clone())
            .filter(|resource_id| !held.contains(resource_id)),
    );
    assert_eq!(
        order, by_rule,
        "case \"every known VPC\": the inventory's VPCs must lead, then the VPCs only a record names"
    );

    update_resolver_config(
        &router,
        "vpc-00000000000000025",
        AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING,
    )
    .await;
    settle().await;
    let after = walk(&router, &json!({}), "after one more VPC becomes known").await;
    assert_eq!(
        after[..listed.len()],
        listed[..],
        "case \"after one more VPC becomes known\": the entries already reported must stay where they were, got {after:?}"
    );
    assert_eq!(
        string_member(&after[listed.len()], "ResourceId"),
        "vpc-00000000000000025",
        "case \"after one more VPC becomes known\": the new VPC must be the entry added, got {after:?}"
    );
}

/// The listing reports the status the clock owes each configuration at the
/// instant of the call: the transient status while the move is in flight, the
/// settled one once it falls due — and listing settles nothing early and
/// restarts no span.
///
/// The rows span the whole `AutodefinedReverseFlag` catalog
/// (`smithy_query(query="shape",shape="AutodefinedReverseFlag")`), each paired
/// with the transient and settled `ResolverAutodefinedReverseStatus` values
/// `ResolverConfig$AutodefinedReverse`'s doc pairs it with. `ENABLE` is asked of
/// a VPC that was moved off the default first, because a request for the setting
/// a configuration is already settled in moves nothing.
#[tokio::test]
async fn the_listing_reports_the_status_the_clock_owes() {
    let router = emulator();

    let rows: Vec<(&str, Option<&str>, &str, &str, &str)> = vec![
        (
            "disabling the autodefined rules",
            None,
            AUTODEFINED_REVERSE_FLAG_DISABLE,
            AUTODEFINED_REVERSE_DISABLING,
            AUTODEFINED_REVERSE_DISABLED,
        ),
        (
            "moving to the local resource setting",
            None,
            AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING,
            AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING,
        ),
        (
            "enabling them again after a disable",
            Some(AUTODEFINED_REVERSE_FLAG_DISABLE),
            AUTODEFINED_REVERSE_FLAG_ENABLE,
            AUTODEFINED_REVERSE_ENABLING,
            AUTODEFINED_REVERSE_ENABLED,
        ),
    ];

    // The rows that need a prior move go through it first, and one wait settles
    // every one of them.
    for (index, (_, prior, _, _, _)) in rows.iter().enumerate() {
        if let Some(prior) = prior {
            update_resolver_config(&router, &format!("vpc-owed-{index}"), prior).await;
        }
    }
    settle().await;

    for (index, (_, _, flag, _, _)) in rows.iter().enumerate() {
        update_resolver_config(&router, &format!("vpc-owed-{index}"), flag).await;
    }

    // Short of the due instant: every row reports the transient status of the
    // move it asked for.
    let in_flight = walk(&router, &json!({}), "while the moves are in flight").await;
    for (index, (case, _, _, transient, _)) in rows.iter().enumerate() {
        let resource_id = format!("vpc-owed-{index}");
        let entry = listed_entry(&in_flight, &resource_id).unwrap_or_else(|| {
            panic!("case {case:?}: {resource_id:?} must be listed, got {in_flight:?}")
        });
        assert_eq!(
            string_member(&entry, "AutodefinedReverse"),
            *transient,
            "case {case:?}: a move still in flight must be listed at its transient status, got {entry}"
        );
    }

    // Past the due instant, with no call in between to settle anything: every
    // row reports its settled status, keeping the identity it was listed with.
    settle().await;
    let settled = walk(&router, &json!({}), "once the moves are due").await;
    for (index, (case, _, _, _, settled_status)) in rows.iter().enumerate() {
        let resource_id = format!("vpc-owed-{index}");
        let entry = listed_entry(&settled, &resource_id).unwrap_or_else(|| {
            panic!("case {case:?}: {resource_id:?} must be listed, got {settled:?}")
        });
        assert_eq!(
            string_member(&entry, "AutodefinedReverse"),
            *settled_status,
            "case {case:?}: once the move is due the settled status is listed, got {entry}"
        );
        let before = listed_entry(&in_flight, &resource_id).expect("the row was listed in flight");
        assert_eq!(
            string_member(&entry, "Id"),
            string_member(&before, "Id"),
            "case {case:?}: the configuration's Id must not move as the status settles, got {entry}"
        );
        assert_eq!(
            entry,
            get_config(&router, &resource_id).await,
            "case {case:?}: the listing must agree with GetResolverConfig, got {entry}"
        );
    }
}

/// Paging walks every configuration exactly once, whatever the page size: the
/// `NextToken` resumes where the previous page stopped, and the last page
/// carries none.
#[tokio::test]
async fn paging_walks_every_resolver_configuration_exactly_once() {
    let router = emulator();
    // More VPCs than the smallest page the model lets a caller ask for, so the
    // page sizes below need more than one page. The VPCs the account already
    // holds count towards the collection, so the total is the writes below plus
    // those.
    let written = usize::try_from(MAX_RESULTS_BOUNDS.0).expect("the bound is positive") + 8;
    for index in 0..written {
        update_resolver_config(
            &router,
            &format!("vpc-paged-{index}"),
            AUTODEFINED_REVERSE_FLAG_DISABLE,
        )
        .await;
    }
    settle().await;
    let known = router.held_vpc_ids().len();

    let expected: Vec<String> = walk(&router, &json!({}), "MaxResults omitted")
        .await
        .iter()
        .map(|item| string_member(item, "ResourceId"))
        .collect();
    assert_eq!(
        expected.len(),
        known,
        "the collection must hold one configuration per known VPC, got {expected:?}"
    );

    // Page sizes spanning the modelled MaxResults range: both published bounds,
    // sizes that do and do not divide the collection, and MaxResults omitted.
    for max_results in [Some(5), Some(6), Some(7), Some(13), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut request = json!({});
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        let walked: Vec<String> = walk(&router, &request, &case)
            .await
            .iter()
            .map(|item| string_member(item, "ResourceId"))
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk every configuration exactly once, in the same order"
        );
    }

    // A page with items left over carries a cursor, and one that ends the
    // collection carries none, so a caller knows when to stop.
    let smallest = usize::try_from(MAX_RESULTS_BOUNDS.0).expect("the bound is positive");
    let mut tail = ok(
        &router,
        "ListResolverConfigs",
        json!({ "MaxResults": MAX_RESULTS_BOUNDS.0 }),
    )
    .await;
    assert!(
        tail.get("NextToken").is_some(),
        "case \"a page with items left over\": a cursor is owed, got {tail}"
    );
    let mut walked = page_items(&tail).len();
    while let Some(token) = tail
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        tail = ok(
            &router,
            "ListResolverConfigs",
            json!({ "MaxResults": MAX_RESULTS_BOUNDS.0, "NextToken": token }),
        )
        .await;
        walked += page_items(&tail).len();
    }
    assert_eq!(
        walked, known,
        "case \"the last page\": the walk must end having reported every configuration, got {walked}"
    );
    assert_eq!(
        page_items(&tail).len(),
        known % smallest,
        "case \"the last page\": the final page must hold what is left of the collection, got {tail}"
    );
    assert_eq!(
        tail.get("NextToken"),
        None,
        "case \"the last page\": a page that ends the collection carries no cursor, got {tail}"
    );

    // A page the whole collection fits in ends it, so no cursor is owed at all.
    let single = ok(
        &router,
        "ListResolverConfigs",
        json!({ "MaxResults": MAX_RESULTS_BOUNDS.1 }),
    )
    .await;
    assert_eq!(
        page_items(&single).len(),
        known,
        "case \"the whole collection in one page\": every configuration must be reported, got {single}"
    );
    assert_eq!(
        single.get("NextToken"),
        None,
        "case \"the whole collection in one page\": no cursor is owed, got {single}"
    );
}

/// `MaxResults` is modelled `range 5..100` for THIS operation, so a request
/// outside it is refused rather than clamped into a page the caller did not ask
/// for — including the 1..4 a caller of the service's other list operations may
/// ask for.
///
/// The refusal is the boundary validator's, so only its HTTP status is asserted:
/// a handler-returned error and a boundary one differ in the error code they
/// render (see `crate::scaffolded`).
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    update_resolver_config(&router, "vpc-bounded-0", AUTODEFINED_REVERSE_FLAG_DISABLE).await;

    for (case, max_results, accepted) in [
        ("far below the published minimum", -1, false),
        ("zero, which the range excludes", 0, false),
        ("one, which the other list operations take", 1, false),
        ("one below the published minimum", 4, false),
        ("the published minimum", 5, true),
        ("inside the range", 37, true),
        ("the published maximum", 100, true),
        ("one above the published maximum", 101, false),
    ] {
        let (status, body) = call(
            &router,
            "ListResolverConfigs",
            json!({ "MaxResults": max_results }),
        )
        .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: MaxResults={max_results} must answer HTTP {expected}, got {status}: {body}"
        );
    }
}

/// A `NextToken` this service did not mint for this collection is refused with
/// the `InvalidParameterException` the operation models — the token is a request
/// PARAMETER this operation rejects as invalid, not a stale cursor — rather than
/// silently read as "start over" and answering page one again. The message is
/// built from the request, so it names the very token that was rejected.
#[tokio::test]
async fn a_next_token_this_query_did_not_mint_is_refused() {
    let router = emulator();
    for index in 0..8 {
        update_resolver_config(
            &router,
            &format!("vpc-cursor-{index}"),
            AUTODEFINED_REVERSE_FLAG_DISABLE,
        )
        .await;
    }
    // The VPCs the account holds are the collection, so what the resumed page
    // must hold is read off its size rather than assumed.
    let known = router.held_vpc_ids().len();

    let page = ok(
        &router,
        "ListResolverConfigs",
        json!({ "MaxResults": MAX_RESULTS_BOUNDS.0 }),
    )
    .await;
    let minted = page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a page with items left over carries a cursor")
        .to_string();

    // Rows spanning the ways a cursor can fail to be this query's: nothing like
    // a cursor, a cursor whose payload was edited, a truncated one, a plausible
    // plaintext offset nobody minted, and a cursor minted for ANOTHER collection
    // of this same service.
    let tampered = {
        let mut bytes = minted.clone().into_bytes();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };
        String::from_utf8(bytes).expect("still ASCII")
    };
    let other_collection = ok(
        &router,
        "ListFirewallConfigs",
        json!({ "MaxResults": MAX_RESULTS_BOUNDS.0 }),
    )
    .await
    .get("NextToken")
    .and_then(Value::as_str)
    .expect("the known VPCs leave items over at the smallest page")
    .to_string();

    let mut messages: Vec<String> = Vec::new();
    let rejected = [
        ("not a cursor at all", "not-a-cursor".to_string()),
        ("an empty string", String::new()),
        ("a tampered cursor", tampered),
        (
            "the minted cursor with its tail cut off",
            minted[..minted.len() - 4].to_string(),
        ),
        (
            "a plaintext offset",
            hex::encode("ListResolverConfigs/all/v1:5"),
        ),
        ("a cursor minted for another collection", other_collection),
    ];
    for (case, token) in &rejected {
        let (status, body) = call(
            &router,
            "ListResolverConfigs",
            json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a cursor this query did not mint must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("InvalidParameterException"),
            "case {case:?}: the refusal must name InvalidParameterException, got {body}"
        );
        let message = string_member(&body, "Message");
        assert_eq!(
            message,
            format!(
                "Value ( {token} ) for parameter NextToken is invalid. The token is invalid."
            ),
            "case {case:?}: the message must name the rejected token, got {body}"
        );
        messages.push(message);
    }

    // The message is built from the request and not from fixed text: two
    // different tokens give two different messages.
    let distinct_tokens: BTreeSet<&String> = rejected.iter().map(|(_, token)| token).collect();
    let distinct_messages: BTreeSet<&String> = messages.iter().collect();
    assert_eq!(
        distinct_messages.len(),
        distinct_tokens.len(),
        "every distinct token must give a distinct Message, got {messages:?}"
    );

    // The cursor this query did mint still resumes the walk, so the refusals
    // above are about the token and not about refusing to resume.
    let resumed = ok(
        &router,
        "ListResolverConfigs",
        json!({ "MaxResults": MAX_RESULTS_BOUNDS.0, "NextToken": minted }),
    )
    .await;
    assert_eq!(
        page_items(&resumed).len(),
        known - usize::try_from(MAX_RESULTS_BOUNDS.0).expect("the bound is positive"),
        "case \"the minted cursor\": the walk must resume where the first page stopped, got {resumed}"
    );
}

/// Listing is a pure read: walking the collection twice reports the very same
/// entries, a walk stores no record for a VPC the store holds none for, and it
/// leaves the records it does hold exactly as they were.
#[tokio::test]
async fn listing_the_configurations_changes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    // One VPC the store holds a record for, and one it only knows of through
    // another record — so a walk has both kinds to leave alone.
    update_resolver_config(&router, "vpc-pure-0", AUTODEFINED_REVERSE_FLAG_DISABLE).await;
    router.hold_vpcs(["vpc-pure-1"]);
    ok(
        &router,
        "UpdateFirewallConfig",
        json!({ "ResourceId": "vpc-pure-1", "FirewallFailOpen": "ENABLED" }),
    )
    .await;
    settle().await;

    let stored_before = service_state(&*engine)
        .expect("the store is reachable from the engine the service is mounted on")
        .lock()
        .resolver_configs
        .len();

    let first = walk(&router, &json!({}), "a first walk").await;
    let second = walk(
        &router,
        &json!({ "MaxResults": MAX_RESULTS_BOUNDS.0 }),
        "a second walk",
    )
    .await;
    assert_eq!(
        first, second,
        "case \"a second walk\": listing twice must report the same entries"
    );

    for resource_id in ["vpc-pure-0", "vpc-pure-1"] {
        let entry = listed_entry(&first, resource_id)
            .unwrap_or_else(|| panic!("{resource_id:?} must be listed, got {first:?}"));
        let read = get_config(&router, resource_id).await;
        assert_eq!(
            entry, read,
            "case {resource_id:?}: a walk must leave the configuration exactly as it was, got {read}"
        );
    }

    let stored_after = service_state(&*engine)
        .expect("the store is reachable from the engine the service is mounted on")
        .lock()
        .resolver_configs
        .len();
    assert_eq!(
        stored_after, stored_before,
        "case \"two walks\": listing must store no configuration, got {stored_after} stored where {stored_before} were"
    );
}
