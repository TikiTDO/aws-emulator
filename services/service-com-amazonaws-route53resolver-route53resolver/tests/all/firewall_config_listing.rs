// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListFirewallConfigs`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].
//!
//! What these cases pin is the rule the operation follows: a VPC's DNS Firewall
//! configuration is a per-VPC singleton the service maintains itself, so the
//! listing reports ONE entry per VPC of the account — the very configuration
//! `GetFirewallConfig` reports for that VPC — however that VPC became known,
//! paged by the `MaxResults` range the model puts on THIS operation and resumed
//! only by a cursor this service minted for it. Which VPCs the account HAS is a
//! fact of its own Amazon EC2 inventory, which `vpc_configuration_inventory`
//! pins; these cases take the VPCs that inventory already holds as the
//! collection's starting point.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member, Emulator, ACCOUNT};

/// The `FirewallFailOpen` a VPC reports until an update names another value:
/// `FirewallConfig$FirewallFailOpen`'s doc, "By default, fail open is disabled".
const FAIL_OPEN_DEFAULT: &str = "DISABLED";

/// The bounds of `ListFirewallConfigs$MaxResults`, from the range the model puts
/// on it (`smithy_query(query="operation",operation="ListFirewallConfigs")`:
/// `ListFirewallConfigsMaxResult`, range 5..10) — narrower than the 1..100 the
/// service's other list operations carry.
const MAX_RESULTS_BOUNDS: (i64, i64) = (5, 10);

/// The `FirewallConfigs` of one page.
fn page_items(body: &Value) -> Vec<Value> {
    body.get("FirewallConfigs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("the response carries a FirewallConfigs array, got {body}"))
}

/// Walk every page of `ListFirewallConfigs` for `request`, returning the items
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
        let page = ok(router, "ListFirewallConfigs", body).await;
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

/// Read one VPC's firewall configuration through `GetFirewallConfig`.
async fn get_config(router: &Emulator, resource_id: &str) -> Value {
    let body = ok(
        router,
        "GetFirewallConfig",
        json!({ "ResourceId": resource_id }),
    )
    .await;
    body.get("FirewallConfig")
        .expect("the read carries a FirewallConfig")
        .clone()
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

/// Write one VPC's failure mode through `UpdateFirewallConfig`.
///
/// The VPC is registered in the account's own Amazon EC2 store first: a per-VPC
/// configuration belongs to a VPC of the account, which the write resolves
/// against that store — the same way [`associate_rule_with_vpc`] registers the
/// VPC a rule is associated with.
async fn update_firewall_config(router: &Emulator, resource_id: &str, fail_open: &str) {
    router.hold_vpcs([resource_id]);
    ok(
        router,
        "UpdateFirewallConfig",
        json!({ "ResourceId": resource_id, "FirewallFailOpen": fail_open }),
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
    let rows = [
        ("the 8-hex-digit form", "vpc-1a2b3c4d"),
        ("the 17-hex-digit form", "vpc-0abcdef1234567890"),
        ("an id in no form AWS mints", "my.private.network"),
    ];
    router.hold_vpcs(rows.iter().map(|(_, resource_id)| *resource_id));

    // The collection an account starts with: the VPCs its own Amazon EC2
    // inventory holds, reported as a list and with no cursor owed.
    let page = ok(&router, "ListFirewallConfigs", json!({})).await;
    let ambient = page_items(&page);
    assert_eq!(
        ambient.iter().map(|item| string_member(item, "ResourceId")).collect::<Vec<String>>(),
        router.held_vpc_ids(),
        "case \"a store nobody wrote to\": the listing must report the VPCs the account holds, got {page}"
    );
    assert_eq!(
        page.get("NextToken"),
        None,
        "case \"a store nobody wrote to\": no cursor is owed on a page that ends the collection, got {page}"
    );

    for (case, resource_id) in rows {
        let read = get_config(&router, resource_id).await;
        assert_eq!(
            string_member(&read, "FirewallFailOpen"),
            FAIL_OPEN_DEFAULT,
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
/// configuration its reader reports for that VPC, and the entries follow the
/// order the collection is defined in.
///
/// The rows span what the rule generalises over — HOW the service came to know
/// of the VPC, which is every way one of its records can name one: a firewall
/// configuration written for it, a Resolver behaviour configuration, a DNSSEC
/// validation configuration, a Resolver rule associated with it, and a Resolver
/// endpoint hosted in it. The failure mode a row leaves untouched must still be
/// reported at its documented default.
#[tokio::test]
async fn every_vpc_the_service_knows_of_is_listed_once_with_its_own_configuration() {
    let router = emulator();
    let (endpoint, host_vpc) =
        outbound_endpoint(&router, "endpoint-for-listing", ("subnet-aaaa1", "subnet-aaaa2")).await;

    // The VPCs in the order the service comes to know of them, paired with the
    // failure mode each must report once known.
    let mut expected: Vec<(String, String, String)> = vec![(
        "a Resolver endpoint hosted in it".to_string(),
        host_vpc.clone(),
        FAIL_OPEN_DEFAULT.to_string(),
    )];

    update_firewall_config(&router, "vpc-00000000000000011", "ENABLED").await;
    expected.push((
        "a firewall configuration written for it".to_string(),
        "vpc-00000000000000011".to_string(),
        "ENABLED".to_string(),
    ));

    router.hold_vpcs(["vpc-00000000000000012"]);
    ok(
        &router,
        "UpdateResolverConfig",
        json!({ "ResourceId": "vpc-00000000000000012", "AutodefinedReverseFlag": "DISABLE" }),
    )
    .await;
    expected.push((
        "a Resolver behaviour configuration written for it".to_string(),
        "vpc-00000000000000012".to_string(),
        FAIL_OPEN_DEFAULT.to_string(),
    ));

    router.hold_vpcs(["vpc-00000000000000013"]);
    ok(
        &router,
        "UpdateResolverDnssecConfig",
        json!({ "ResourceId": "vpc-00000000000000013", "Validation": "ENABLE" }),
    )
    .await;
    expected.push((
        "a DNSSEC validation configuration written for it".to_string(),
        "vpc-00000000000000013".to_string(),
        FAIL_OPEN_DEFAULT.to_string(),
    ));

    associate_rule_with_vpc(&router, &endpoint, "vpc-00000000000000014").await;
    expected.push((
        "a Resolver rule associated with it".to_string(),
        "vpc-00000000000000014".to_string(),
        FAIL_OPEN_DEFAULT.to_string(),
    ));

    let listed = walk(&router, &json!({}), "every known VPC").await;
    assert_eq!(
        listed.len(),
        expected.len(),
        "case \"every known VPC\": the listing must report one configuration per known VPC, got {listed:?}"
    );

    for (case, resource_id, fail_open) in &expected {
        let entry = listed_entry(&listed, resource_id).unwrap_or_else(|| {
            panic!("case {case:?}: {resource_id:?} must be listed, got {listed:?}")
        });
        assert_eq!(
            string_member(&entry, "FirewallFailOpen"),
            *fail_open,
            "case {case:?}: the listed failure mode must be the VPC's, got {entry}"
        );
        assert_eq!(
            string_member(&entry, "OwnerId"),
            ACCOUNT,
            "case {case:?}: OwnerId must be the account that owns the VPC, got {entry}"
        );
        let id = string_member(&entry, "Id");
        assert!(
            id.starts_with("rslvr-fc-") && !id.is_empty() && id.len() <= 64,
            "case {case:?}: a configuration Id is rslvr-fc-<hex> inside ResourceId's length 1..64, got {id:?}"
        );

        // The listed entry is the very configuration the reader reports for that
        // VPC, so one VPC has ONE configuration across the service.
        let read = get_config(&router, resource_id).await;
        assert_eq!(
            entry, read,
            "case {case:?}: the listing must agree with GetFirewallConfig, got {entry} then {read}"
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

    update_firewall_config(&router, "vpc-00000000000000015", "USE_LOCAL_RESOURCE_SETTING").await;
    let after = walk(&router, &json!({}), "after one more VPC becomes known").await;
    assert_eq!(
        after[..listed.len()],
        listed[..],
        "case \"after one more VPC becomes known\": the entries already reported must stay where they were, got {after:?}"
    );
    assert_eq!(
        string_member(&after[listed.len()], "ResourceId"),
        "vpc-00000000000000015",
        "case \"after one more VPC becomes known\": the new VPC must be the entry added, got {after:?}"
    );
}

/// Paging walks every configuration exactly once, whatever the page size: the
/// `NextToken` resumes where the previous page stopped, and the last page
/// carries none.
#[tokio::test]
async fn paging_walks_every_firewall_configuration_exactly_once() {
    let router = emulator();
    // More VPCs than the largest page the model lets a caller ask for, so every
    // page size below needs at least two pages. The VPCs the account already
    // holds count towards the collection, so the total is read off the listing
    // rather than assumed to be the writes below.
    let written = usize::try_from(MAX_RESULTS_BOUNDS.1).expect("the bound is positive") + 3;
    for index in 0..written {
        update_firewall_config(&router, &format!("vpc-paged-{index}"), "ENABLED").await;
    }
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
    for max_results in [Some(5), Some(6), Some(7), Some(10), None] {
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
    let first = ok(
        &router,
        "ListFirewallConfigs",
        json!({ "MaxResults": MAX_RESULTS_BOUNDS.0 }),
    )
    .await;
    assert!(
        first.get("NextToken").is_some(),
        "case \"a page with items left over\": a cursor is owed, got {first}"
    );
    let largest = usize::try_from(MAX_RESULTS_BOUNDS.1).expect("the bound is positive");
    let mut tail = ok(
        &router,
        "ListFirewallConfigs",
        json!({ "MaxResults": MAX_RESULTS_BOUNDS.1 }),
    )
    .await;
    let mut walked = page_items(&tail).len();
    while let Some(token) = tail
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        tail = ok(
            &router,
            "ListFirewallConfigs",
            json!({ "MaxResults": MAX_RESULTS_BOUNDS.1, "NextToken": token }),
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
        known - largest,
        "case \"the last page\": the final page must hold what is left of the collection, got {tail}"
    );
    assert_eq!(
        tail.get("NextToken"),
        None,
        "case \"the last page\": a page that ends the collection carries no cursor, got {tail}"
    );
}

/// `MaxResults` is modelled `range 5..10` for THIS operation, so a request
/// outside it is refused rather than clamped into a page the caller did not ask
/// for — including the 1..100 bounds the service's other list operations accept.
///
/// The refusal is the boundary validator's, so only its HTTP status is asserted:
/// a handler-returned error and a boundary one differ in the error code they
/// render (see `crate::scaffolded`).
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    update_firewall_config(&router, "vpc-bounded-0", "ENABLED").await;

    for (case, max_results, accepted) in [
        ("far below the published minimum", -1, false),
        ("zero, which the range excludes", 0, false),
        ("one below the published minimum", 4, false),
        ("the published minimum", 5, true),
        ("inside the range", 7, true),
        ("the published maximum", 10, true),
        ("one above the published maximum", 11, false),
        ("the maximum the other list operations take", 100, false),
    ] {
        let (status, body) = call(
            &router,
            "ListFirewallConfigs",
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
/// the `ValidationException` the operation models, rather than silently read as
/// "start over" and answering page one again.
#[tokio::test]
async fn a_next_token_this_query_did_not_mint_is_refused() {
    let router = emulator();
    for index in 0..8 {
        update_firewall_config(&router, &format!("vpc-cursor-{index}"), "ENABLED").await;
    }
    // The VPCs the account holds are the collection, so what the resumed page
    // must hold is read off its size rather than assumed.
    let known = router.held_vpc_ids().len();

    let page = ok(
        &router,
        "ListFirewallConfigs",
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
    // plaintext offset nobody minted, and a cursor minted for ANOTHER
    // collection of this same service.
    let tampered = {
        let mut bytes = minted.clone().into_bytes();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };
        String::from_utf8(bytes).expect("still ASCII")
    };
    let other_collection = ok(
        &router,
        "ListFirewallDomainLists",
        json!({ "MaxResults": 1 }),
    )
    .await
    .get("NextToken")
    .and_then(Value::as_str)
    .expect("the published domain lists leave items over at MaxResults 1")
    .to_string();

    for (case, token) in [
        ("not a cursor at all", "not-a-cursor".to_string()),
        ("an empty string", String::new()),
        ("a tampered cursor", tampered),
        (
            "the minted cursor with its tail cut off",
            minted[..minted.len() - 4].to_string(),
        ),
        (
            "a plaintext offset",
            hex::encode("ListFirewallConfigs/all/v1:5"),
        ),
        ("a cursor minted for another collection", other_collection),
    ] {
        let (status, body) = call(
            &router,
            "ListFirewallConfigs",
            json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a cursor this query did not mint must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {body}"
        );
    }

    // The cursor this query did mint still resumes the walk, so the refusals
    // above are about the token and not about refusing to resume.
    let resumed = ok(
        &router,
        "ListFirewallConfigs",
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
/// entries, and a walk neither creates a configuration nor moves one.
#[tokio::test]
async fn listing_the_configurations_changes_nothing() {
    let router = emulator();
    update_firewall_config(&router, "vpc-pure-0", "ENABLED").await;
    update_firewall_config(&router, "vpc-pure-1", "USE_LOCAL_RESOURCE_SETTING").await;

    let first = walk(&router, &json!({}), "a first walk").await;
    let second = walk(&router, &json!({ "MaxResults": MAX_RESULTS_BOUNDS.0 }), "a second walk").await;
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
}
