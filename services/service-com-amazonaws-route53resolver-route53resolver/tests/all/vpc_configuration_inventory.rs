// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the collection the per-VPC configuration LISTINGS page
//! over: `ListFirewallConfigs` and `ListResolverConfigs`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, and the VPCs are created through the
//! account's own Amazon EC2 API (ec2Query, `POST /` with
//! `Action=CreateVpc`) mounted on the same engine — so what is asserted is the
//! HTTP behaviour of both services and not a store written from the test.
//!
//! What these cases pin is the rule both listings follow: a per-VPC singleton
//! configuration the service maintains itself exists for every VPC the ACCOUNT
//! holds, so the listing reports one entry per VPC of the account's own Amazon
//! EC2 inventory — beside any VPC this service's own records name — and every
//! entry is the very configuration the per-VPC read reports for that VPC,
//! however that VPC came to be known.

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::harness::{emulator_with_ec2, ok, string_member, Emulator, REGION};

/// The `FirewallFailOpen` a VPC reports until an update names another value:
/// `FirewallConfig$FirewallFailOpen`'s doc, "By default, fail open is disabled".
const FAIL_OPEN_DEFAULT: &str = "DISABLED";

/// The `AutodefinedReverse` a VPC reports until an update names another value:
/// `ResolverConfig$AutodefinedReverse`'s doc, which has Resolver creating those
/// rules unless the account turns them off.
const AUTODEFINED_REVERSE_DEFAULT: &str = "ENABLED";

/// The smallest page both listings admit: `ListFirewallConfigs$MaxResults` is
/// modelled `range 5..10` and `ListResolverConfigs$MaxResults` `range 5..100`
/// (`smithy_query(query="operation",operation="ListFirewallConfigs")`), so 5 is
/// a page size every one of them accepts.
const SMALLEST_PAGE: i64 = 5;

/// The Amazon EC2 API version its ec2Query wire carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// One per-VPC configuration slice: the read, the listing, and the member each
/// of them carries.
struct Slice {
    /// The operation that reports ONE VPC's configuration.
    read: &'static str,
    /// The operation that LISTS the configurations.
    list: &'static str,
    /// The member the read answers the configuration in.
    member: &'static str,
    /// The member the listing answers its page of configurations in.
    page_member: &'static str,
    /// The setting the configuration carries, and the value a VPC nobody has
    /// written reports for it.
    setting: (&'static str, &'static str),
}

/// Both slices this rule holds for, which is every per-VPC singleton
/// configuration the service maintains for a VPC of the account.
const SLICES: [Slice; 2] = [
    Slice {
        read: "GetFirewallConfig",
        list: "ListFirewallConfigs",
        member: "FirewallConfig",
        page_member: "FirewallConfigs",
        setting: ("FirewallFailOpen", FAIL_OPEN_DEFAULT),
    },
    Slice {
        read: "GetResolverConfig",
        list: "ListResolverConfigs",
        member: "ResolverConfig",
        page_member: "ResolverConfigs",
        setting: ("AutodefinedReverse", AUTODEFINED_REVERSE_DEFAULT),
    },
];

/// Percent-encode one awsQuery form value, leaving only RFC 3986's unreserved
/// characters literal — a CIDR block carries a `/`, so it has to travel
/// escaped.
fn form_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of every `<name>…</name>` element of an XML body, in document
/// order.
fn elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found: Vec<String> = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let from = start + open.len();
        let Some(end) = rest[from..].find(&close) else {
            break;
        };
        found.push(rest[from..from + end].to_string());
        rest = &rest[from + end + close.len()..];
    }
    found
}

/// The text of the first `<name>…</name>` element of an XML body.
fn element(xml: &str, name: &str) -> Option<String> {
    elements(xml, name).into_iter().next()
}

/// Call one Amazon EC2 action over its ec2Query wire, signed for `region`, and
/// require HTTP 200.
async fn ec2_ok_in(
    router: &Emulator,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        // The SigV4 credential scope is what routes the request to Amazon EC2.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{region}/ec2/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=not-verified-in-this-test"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "Amazon EC2 {action} in {region} must succeed, got {status}: {text}"
    );
    text
}

/// Call one Amazon EC2 action over its ec2Query wire and require HTTP 200.
async fn ec2_ok(router: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    ec2_ok_in(router, REGION, action, params).await
}

/// Create one VPC in the account's own Amazon EC2 inventory and answer the
/// `vpcId` that service minted for it.
async fn create_vpc(router: &Emulator, cidr_block: &str) -> String {
    let answered = ec2_ok(router, "CreateVpc", &[("CidrBlock", cidr_block)]).await;
    let vpc_id = element(&answered, "vpcId")
        .unwrap_or_else(|| panic!("CreateVpc must answer a vpcId, got {answered}"));
    assert!(
        vpc_id.starts_with("vpc-"),
        "CreateVpc must answer a minted vpc id, got {vpc_id:?}"
    );
    vpc_id
}

/// Take one VPC back out of the account's Amazon EC2 inventory.
async fn delete_vpc(router: &Emulator, vpc_id: &str) {
    ec2_ok(router, "DeleteVpc", &[("VpcId", vpc_id)]).await;
}

/// Every VPC the account holds, as its own Amazon EC2 API reports them: the
/// inventory the per-VPC configuration listings are views of. An account holds
/// the ambient default VPC of its Region from the moment it exists, so this is
/// never assumed empty.
///
/// Amazon EC2 is a REGIONAL service and its `DescribeVpcs` reports the VPCs of
/// the Region it was asked in, while these listings are views of what the
/// ACCOUNT holds. So the inventory is read in BOTH the Region this suite works
/// in and the Region a fresh account's ambient network is seeded in, and the two
/// answers are the account's whole inventory.
async fn inventory_vpc_ids(router: &Emulator) -> Vec<String> {
    let mut held: Vec<String> = Vec::new();
    for region in [REGION, ec2::state::DEFAULT_REGION] {
        let answered = ec2_ok_in(router, region, "DescribeVpcs", &[]).await;
        for vpc_id in elements(&answered, "vpcId") {
            if !held.contains(&vpc_id) {
                held.push(vpc_id);
            }
        }
    }
    assert!(
        !held.is_empty(),
        "DescribeVpcs must report the VPCs the account holds"
    );
    held
}

/// The `ResourceId`s of a listing, sorted — the set a listing reports, compared
/// without pinning the order the collection is paged in.
fn listed_resource_ids(listing: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = listing
        .iter()
        .map(|item| string_member(item, "ResourceId"))
        .collect();
    ids.sort();
    ids
}

/// The same ids, sorted.
fn sorted(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut sorted: Vec<String> = ids.into_iter().collect();
    sorted.sort();
    sorted
}

/// Read one VPC's configuration through the slice's own reader.
async fn read_config(router: &Emulator, slice: &Slice, resource_id: &str) -> Value {
    let body = ok(router, slice.read, json!({ "ResourceId": resource_id })).await;
    body.get(slice.member)
        .unwrap_or_else(|| panic!("{} must answer a {}, got {body}", slice.read, slice.member))
        .clone()
}

/// Walk every page of the slice's listing, returning the entries in the order
/// they were reported.
async fn walk(router: &Emulator, slice: &Slice, max_results: Option<i64>, case: &str) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut body = json!({});
        if let Some(size) = max_results {
            body["MaxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(router, slice.list, body).await;
        pages += 1;
        assert!(
            pages <= 200,
            "case {case:?}: {} paging did not terminate after {pages} pages",
            slice.list
        );
        let items = page
            .get(slice.page_member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} must answer a {} array, got {page}",
                    slice.list, slice.page_member
                )
            });
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

/// The listed entry for `resource_id`, or `None` when the listing carries none —
/// and never two, because one VPC has ONE configuration.
fn listed_entry(listing: &[Value], resource_id: &str, case: &str) -> Option<Value> {
    let matching: Vec<&Value> = listing
        .iter()
        .filter(|item| string_member(item, "ResourceId") == resource_id)
        .collect();
    assert!(
        matching.len() <= 1,
        "case {case:?}: at most one configuration per VPC may be listed, got {} for {resource_id:?}",
        matching.len()
    );
    matching.first().map(|item| (*item).clone())
}

/// Write one VPC's DNS Firewall failure mode.
async fn update_firewall_config(router: &Emulator, resource_id: &str, fail_open: &str) {
    ok(
        router,
        "UpdateFirewallConfig",
        json!({ "ResourceId": resource_id, "FirewallFailOpen": fail_open }),
    )
    .await;
}

/// Create a DNS Firewall rule group and answer its id.
async fn create_rule_group(router: &Emulator, creator: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": creator, "Name": creator }),
    )
    .await;
    string_member(
        created
            .get("FirewallRuleGroup")
            .expect("the create answers a FirewallRuleGroup"),
        "Id",
    )
}

/// Associate `rule_group` with `vpc_id` at `priority`.
async fn associate_rule_group(router: &Emulator, rule_group: &str, vpc_id: &str, priority: i64) {
    ok(
        router,
        "AssociateFirewallRuleGroup",
        json!({
            "CreatorRequestId": format!("assoc-{vpc_id}-{priority}"),
            "FirewallRuleGroupId": rule_group,
            "VpcId": vpc_id,
            "Priority": priority,
            "Name": "listed-association",
            "MutationProtection": "DISABLED",
        }),
    )
    .await;
}

/// Every VPC the account holds is reported by BOTH per-VPC configuration
/// listings, with exactly the configuration that slice's reader reports for it.
///
/// The rows span what the rule generalises over — how the VPC came to be known:
/// a VPC the account created and no Resolver call has ever named, one whose
/// firewall failure mode was written, and one a DNS Firewall rule group is
/// associated with. Each row is asserted against both slices, so the rule is not
/// pinned to one collection.
#[tokio::test]
async fn every_vpc_the_account_holds_is_listed_with_the_configuration_its_reader_reports() {
    let router = emulator_with_ec2();

    // A VPC the account created and nothing of this service has ever named.
    let untouched = create_vpc(&router, "10.1.0.0/16").await;

    // A VPC whose DNS Firewall failure mode was moved off its default.
    let written = create_vpc(&router, "10.2.0.0/16").await;
    update_firewall_config(&router, &written, "ENABLED").await;

    // A VPC a DNS Firewall rule group is associated with.
    let associated = create_vpc(&router, "10.3.0.0/16").await;
    let rule_group = create_rule_group(&router, "group-for-inventory").await;
    associate_rule_group(&router, &rule_group, &associated, 101).await;

    let rows = [
        ("a VPC no Resolver call has ever named", untouched.as_str()),
        ("a VPC whose firewall failure mode was written", written.as_str()),
        ("a VPC a rule group is associated with", associated.as_str()),
    ];

    // The collection is exactly the account's own VPC inventory.
    let expected_ids = sorted(inventory_vpc_ids(&router).await);
    for (_, resource_id) in rows {
        assert!(
            expected_ids.iter().any(|held| held == resource_id),
            "setup: {resource_id:?} must be one of the VPCs the account holds, got {expected_ids:?}"
        );
    }

    for slice in &SLICES {
        let listing = walk(&router, slice, None, slice.list).await;
        assert_eq!(
            listed_resource_ids(&listing),
            expected_ids,
            "case {:?}: the listing must report one configuration per VPC the account holds, got {listing:?}",
            slice.list
        );

        for (case, resource_id) in rows {
            let case = format!("{} / {case}", slice.list);
            let entry = listed_entry(&listing, resource_id, &case).unwrap_or_else(|| {
                panic!("case {case:?}: {resource_id:?} must be listed, got {listing:?}")
            });
            let read = read_config(&router, slice, resource_id).await;
            assert_eq!(
                entry, read,
                "case {case:?}: the listed configuration must be the one the reader reports, got {entry} then {read}"
            );
            assert_eq!(
                string_member(&entry, "ResourceId"),
                resource_id,
                "case {case:?}: the entry must name the VPC it configures, got {entry}"
            );
            assert!(
                !string_member(&entry, "Id").is_empty(),
                "case {case:?}: a configuration carries an Id, got {entry}"
            );
        }

        // A VPC nothing has written reports the slice's documented default, so
        // the entries are the VPCs' own configurations and not one copied over.
        let (member, default) = slice.setting;
        let untouched_entry = listed_entry(&listing, &untouched, slice.list)
            .expect("the untouched VPC is listed");
        assert_eq!(
            string_member(&untouched_entry, member),
            default,
            "case {:?}: a VPC nobody wrote must be listed at its documented default, got {untouched_entry}",
            slice.list
        );
    }

    // The listing is a pure read: a VPC's configuration is not created by being
    // listed, so two walks report the very same entries.
    for slice in &SLICES {
        let first = walk(&router, slice, None, "a first walk").await;
        let second = walk(&router, slice, Some(SMALLEST_PAGE), "a second walk").await;
        assert_eq!(
            first, second,
            "case {:?}: walking the collection twice must report the same entries",
            slice.list
        );
    }
}

/// A VPC the account no longer holds leaves the listing with it, unless one of
/// this service's own records still names the VPC — so the collection follows
/// the account's inventory rather than everything a caller has ever asked
/// about.
#[tokio::test]
async fn a_vpc_the_account_no_longer_holds_leaves_the_listing() {
    let router = emulator_with_ec2();
    let released = create_vpc(&router, "10.4.0.0/16").await;
    let remembered = create_vpc(&router, "10.5.0.0/16").await;
    update_firewall_config(&router, &remembered, "ENABLED").await;

    for slice in &SLICES {
        let listing = walk(&router, slice, None, "both VPCs held").await;
        for resource_id in [&released, &remembered] {
            assert!(
                listed_entry(&listing, resource_id, slice.list).is_some(),
                "case {:?}: {resource_id:?} must be listed while the account holds it, got {listing:?}",
                slice.list
            );
        }
    }

    delete_vpc(&router, &released).await;
    delete_vpc(&router, &remembered).await;

    for slice in &SLICES {
        let listing = walk(&router, slice, None, "both VPCs released").await;
        assert!(
            listed_entry(&listing, &released, slice.list).is_none(),
            "case {:?}: a VPC the account no longer holds must not be listed, got {listing:?}",
            slice.list
        );
        assert!(
            listed_entry(&listing, &remembered, slice.list).is_some(),
            "case {:?}: a VPC this service's own records name must stay listed, got {listing:?}",
            slice.list
        );
    }
}

/// Associating a DNS Firewall rule group with a VPC leaves the configuration
/// that VPC already had exactly as it was — the same `Id` and the same failure
/// mode — rather than replacing it with a freshly made one.
///
/// The rows span the failure mode the VPC holds before the association: the
/// documented default and every other value `FirewallFailOpenStatus` declares
/// (`smithy_query(query="shape",shape="FirewallFailOpenStatus")`).
#[tokio::test]
async fn associating_a_rule_group_leaves_the_vpcs_firewall_configuration_in_place() {
    let router = emulator_with_ec2();
    let rule_group = create_rule_group(&router, "group-for-association").await;

    // (case, the failure mode written before the association, if any)
    let rows: [(&str, Option<&str>); 3] = [
        ("a VPC left at its documented default", None),
        ("a VPC moved to ENABLED", Some("ENABLED")),
        (
            "a VPC moved to USE_LOCAL_RESOURCE_SETTING",
            Some("USE_LOCAL_RESOURCE_SETTING"),
        ),
    ];

    for (index, (case, written)) in rows.iter().enumerate() {
        let vpc_id = create_vpc(&router, &format!("10.{}.0.0/16", 10 + index)).await;
        if let Some(fail_open) = written {
            update_firewall_config(&router, &vpc_id, fail_open).await;
        }

        let baseline = read_config(&router, &SLICES[0], &vpc_id).await;
        assert_eq!(
            string_member(&baseline, "FirewallFailOpen"),
            written.unwrap_or(FAIL_OPEN_DEFAULT),
            "case {case:?}: the VPC must report the failure mode it holds, got {baseline}"
        );
        let listing = walk(&router, &SLICES[0], None, case).await;
        assert_eq!(
            listed_entry(&listing, &vpc_id, case),
            Some(baseline.clone()),
            "case {case:?}: the configuration the reader reports must be the one listed, got {listing:?}"
        );

        let priority = 101 + i64::try_from(index).expect("the row count is small");
        associate_rule_group(&router, &rule_group, &vpc_id, priority).await;

        let after = read_config(&router, &SLICES[0], &vpc_id).await;
        assert_eq!(
            after, baseline,
            "case {case:?}: the association must leave the pre-existing configuration in place, got {after} after {baseline}"
        );
        let listing = walk(&router, &SLICES[0], Some(SMALLEST_PAGE), case).await;
        assert_eq!(
            listed_entry(&listing, &vpc_id, case),
            Some(baseline),
            "case {case:?}: the listing must still report that one configuration, got {listing:?}"
        );
    }
}

/// Paging walks the account's VPCs exactly once whatever the page size, so a
/// paginator-driven client enumerating the inventory sees every VPC's
/// configuration and none of them twice.
#[tokio::test]
async fn paging_walks_every_vpc_of_the_inventory_exactly_once() {
    let router = emulator_with_ec2();

    // More VPCs than the smallest page both listings admit, so every page size
    // below needs more than one page.
    let held = usize::try_from(SMALLEST_PAGE).expect("the bound is positive") + 3;
    let mut created: Vec<String> = Vec::new();
    for index in 0..held {
        created.push(create_vpc(&router, &format!("10.{}.0.0/16", 100 + index)).await);
    }
    // One of them moved off its default, so the collection mixes VPCs this
    // service holds a record for with VPCs only the inventory names.
    update_firewall_config(&router, &created[1], "ENABLED").await;
    let expected_ids = sorted(inventory_vpc_ids(&router).await);

    for slice in &SLICES {
        let baseline: Vec<String> = walk(&router, slice, None, "MaxResults omitted")
            .await
            .iter()
            .map(|item| string_member(item, "ResourceId"))
            .collect();
        assert_eq!(
            sorted(baseline.clone()),
            expected_ids,
            "case {:?}: the collection must hold one configuration per VPC the account holds",
            slice.list
        );
        assert!(
            baseline.len() > usize::try_from(SMALLEST_PAGE).expect("the bound is positive"),
            "case {:?}: the collection must need more than one page, got {baseline:?}",
            slice.list
        );

        for max_results in [Some(SMALLEST_PAGE), Some(SMALLEST_PAGE + 1), Some(10), None] {
            let case = format!("{} / MaxResults={max_results:?}", slice.list);
            let walked: Vec<String> = walk(&router, slice, max_results, &case)
                .await
                .iter()
                .map(|item| string_member(item, "ResourceId"))
                .collect();
            assert_eq!(
                walked, baseline,
                "case {case:?}: paging must walk every configuration exactly once, in the same order"
            );
        }
    }
}
