// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateSecurityGroupRuleDescriptionsEgress`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=UpdateSecurityGroupRuleDescriptionsEgress`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the call writes the DESCRIPTION of each outbound rule it names
//! and nothing else: "you can replace an existing description, or add a description to a rule
//! that did not have one previously … you can remove a description for a security group rule
//! by omitting the description parameter in the request", where the rules are named either by
//! `SecurityGroupRuleDescriptions` (a rule id beside its description) or by `IpPermissions`
//! (a rule's own protocol, port range and destination, with the description on the grant).
//! Every row below is a distinct input along what that generalises over — the rule type, the
//! protocol, whether a port range is carried at all, which of replace/add/remove is asked,
//! which of the two ways the rule is named, how many rules one call describes, and whether the
//! rule was written by an authorize or by the service itself — so the one case a recording
//! happens to contain is one row among many rather than the specification.
//!
//! Every update is read back through `DescribeSecurityGroupRules`, and through
//! `DescribeSecurityGroups` for the aggregate view of the same rules, so the round trip is
//! proved through readers this operation does not write.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    let (router, _engine) = super::ec2_query::emulator();
    router
}

/// The id of the prefix list `REGION` publishes at position `index` of its own catalog, in
/// `PrefixListName` order.
///
/// A rule may only grant a prefix list the account can SEE, so a row that describes a
/// prefix-list rule names one of those rather than an id of the right shape that resolves to
/// nothing.
fn published_prefix_list(index: usize) -> String {
    let published = ec2::state::service_prefix_lists_in_region(REGION);
    published
        .get(index)
        .map(|list| list.prefix_list_id.clone())
        .unwrap_or_else(|| panic!("the Region publishes no prefix list at index {index}"))
}

// ---------------------------------------------------------------------------
// The four rule types, as a request names one and as a rule reports it
// ---------------------------------------------------------------------------

/// Which destination an outbound security group rule names — "you must specify exactly one of
/// the following destinations" (`AuthorizeSecurityGroupEgress`), which is also what identifies
/// the rule an `IpPermissions` entry describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ipv4,
    Ipv6,
    PrefixList,
    ReferencedGroup,
}

impl Kind {
    /// The `IpPermission` list and member this kind of destination is named in, which is how
    /// an authorize creates the rule a row starts from and how an update names it by rule
    /// properties.
    fn permission_member(self) -> (&'static str, &'static str) {
        match self {
            Self::Ipv4 => ("IpRanges", "CidrIp"),
            Self::Ipv6 => ("Ipv6Ranges", "CidrIpv6"),
            Self::PrefixList => ("PrefixListIds", "PrefixListId"),
            Self::ReferencedGroup => ("Groups", "GroupId"),
        }
    }

    /// The destination as the assertions below spell one: `<kind>=<value>`.
    fn expected(self, value: &str) -> String {
        let tag = match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
            Self::PrefixList => "prefix-list",
            Self::ReferencedGroup => "group",
        };
        format!("{tag}={value}")
    }
}

/// One security group rule as `DescribeSecurityGroupRules` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    id: String,
    group_id: String,
    owner_id: String,
    is_egress: String,
    protocol: String,
    from_port: Option<String>,
    to_port: Option<String>,
    /// The one destination the rule names, as [`Kind::expected`] spells one.
    destination: String,
    description: Option<String>,
    arn: String,
    tags: Vec<(String, String)>,
}

/// The rules a `DescribeSecurityGroupRules` body reports, in the order it reported them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "securityGroupRuleSet")
    else {
        panic!("case {case:?}: no <securityGroupRuleSet> in {body}");
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            let destination = if let Some(cidr) = child_text(item, "cidrIpv4") {
                format!("ipv4={cidr}")
            } else if let Some(cidr) = child_text(item, "cidrIpv6") {
                format!("ipv6={cidr}")
            } else if let Some(prefix_list_id) = child_text(item, "prefixListId") {
                format!("prefix-list={prefix_list_id}")
            } else if let Some(group_id) =
                child(item, "referencedGroupInfo").and_then(|info| child_text(info, "groupId"))
            {
                format!("group={group_id}")
            } else {
                "none".to_string()
            };
            let tags = child(item, "tagSet")
                .map(|set| {
                    set.children()
                        .filter(|node| node.is_element() && node.tag_name().name() == "item")
                        .map(|tag| {
                            (
                                child_text(tag, "key").unwrap_or_default(),
                                child_text(tag, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Reported {
                id: child_text(item, "securityGroupRuleId").unwrap_or_default(),
                group_id: child_text(item, "groupId").unwrap_or_default(),
                owner_id: child_text(item, "groupOwnerId").unwrap_or_default(),
                is_egress: child_text(item, "isEgress").unwrap_or_default(),
                protocol: child_text(item, "ipProtocol").unwrap_or_default(),
                from_port: child_text(item, "fromPort"),
                to_port: child_text(item, "toPort"),
                destination,
                description: child_text(item, "description"),
                arn: child_text(item, "securityGroupRuleArn").unwrap_or_default(),
                tags,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/// `CreateSecurityGroup` for `group_name` in the account's default VPC, answering its id.
async fn create_group(case: &str, router: &MultiServiceRouter, group_name: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateSecurityGroup",
        &[
            param("GroupName", group_name),
            param("GroupDescription", "a group whose rule descriptions change"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSecurityGroup, got {status} \
         with body {body}"
    );
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no <groupId> in {body}"))
}

/// One rule to create before its description is updated: the protocol, the port range, the
/// kind of destination, the destination itself and the description it starts with.
#[derive(Debug, Clone)]
struct Rule<'a> {
    protocol: &'a str,
    ports: Option<(i32, i32)>,
    kind: Kind,
    destination: &'a str,
    description: Option<&'a str>,
}

/// The `IpPermissions.<position>.…` fields naming the one rule `rule` describes, with
/// `description` as its grant's description where there is one.
fn permission_params(position: usize, rule: &Rule<'_>, description: Option<&str>) -> Vec<Param> {
    let (list, member) = rule.kind.permission_member();
    let base = format!("IpPermissions.{position}");
    let mut params = vec![
        param(&format!("{base}.IpProtocol"), rule.protocol),
        param(&format!("{base}.{list}.1.{member}"), rule.destination),
    ];
    if let Some((from_port, to_port)) = rule.ports {
        params.push(param(&format!("{base}.FromPort"), &from_port.to_string()));
        params.push(param(&format!("{base}.ToPort"), &to_port.to_string()));
    }
    if let Some(description) = description {
        params.push(param(
            &format!("{base}.{list}.1.Description"),
            description,
        ));
    }
    params
}

/// Authorize the ONE outbound rule `rule` describes on `group_id` and answer the
/// `SecurityGroupRuleId` the service minted for it.
///
/// The rule a row describes is created through the operation that creates one, so what a row
/// starts from is a rule the service itself stored rather than a record a test wrote.
async fn authorize_egress(
    case: &str,
    router: &MultiServiceRouter,
    group_id: &str,
    rule: &Rule<'_>,
) -> String {
    let mut params = vec![param("GroupId", group_id)];
    params.extend(permission_params(1, rule, rule.description));
    let (status, body) =
        super::ec2_query::call(router, REGION, "AuthorizeSecurityGroupEgress", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AuthorizeSecurityGroupEgress, got {status} \
         with body {body}"
    );
    element(&body, "securityGroupRuleId")
        .unwrap_or_else(|| panic!("case {case:?}: no <securityGroupRuleId> in {body}"))
}

/// One `UpdateSecurityGroupRuleDescriptionsEgress` call carrying `params`.
async fn update(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "UpdateSecurityGroupRuleDescriptionsEgress",
        params,
    )
    .await
}

/// One entry of a `SecurityGroupRuleDescriptions` list, at `position` (counting from 1).
///
/// The list is sent as `SecurityGroupRuleDescription.<n>` — the name the wire gives it
/// (`UpdateSecurityGroupRuleDescriptionsEgressRequest$SecurityGroupRuleDescriptions` carries
/// `xmlName` `SecurityGroupRuleDescription`), the way a client's own encoder writes it. A row
/// asking for the description to be REMOVED sends no `Description` at all.
fn description_params(position: usize, rule_id: &str, description: Option<&str>) -> Vec<Param> {
    let base = format!("SecurityGroupRuleDescription.{position}");
    let mut params = vec![param(&format!("{base}.SecurityGroupRuleId"), rule_id)];
    if let Some(description) = description {
        params.push(param(&format!("{base}.Description"), description));
    }
    params
}

/// The rules of `group_id`, as `DescribeSecurityGroupRules` reports them through the
/// `group-id` filter.
async fn rules_of(case: &str, router: &MultiServiceRouter, group_id: &str) -> Vec<Reported> {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeSecurityGroupRules",
        &filters(&[("group-id", &[group_id])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroupRules, got {status} \
         with body {body}"
    );
    reported(case, &body)
}

/// The one rule `rule_id` names, as `DescribeSecurityGroupRules` reports it.
async fn rule(case: &str, router: &MultiServiceRouter, group_id: &str, rule_id: &str) -> Reported {
    rules_of(case, router, group_id)
        .await
        .into_iter()
        .find(|reported| reported.id == rule_id)
        .unwrap_or_else(|| panic!("case {case:?}: no rule {rule_id} of group {group_id}"))
}

/// The `<ipPermissionsEgress>` section of `group_id` as `DescribeSecurityGroups` reports it —
/// the AGGREGATE view of the same rules, which this operation never writes directly.
async fn aggregated_egress(case: &str, router: &MultiServiceRouter, group_id: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeSecurityGroups",
        &[param("GroupId.1", group_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroups, got {status} \
         with body {body}"
    );
    let open = "<ipPermissionsEgress>";
    let close = "</ipPermissionsEgress>";
    let start = body
        .find(open)
        .unwrap_or_else(|| panic!("case {case:?}: no {open} in {body}"));
    let end = body[start..]
        .find(close)
        .unwrap_or_else(|| panic!("case {case:?}: no {close} in {body}"))
        + start;
    body[start..end].to_string()
}

/// The `<return>` member a successful update answers with.
fn returned(case: &str, body: &str) -> String {
    element(body, "return").unwrap_or_else(|| panic!("case {case:?}: no <return> in {body}"))
}

// ---------------------------------------------------------------------------
// One rule, described in place
// ---------------------------------------------------------------------------

/// How a row names the rule it describes: by the rule's id, or by the rule's own properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedBy {
    RuleId,
    Properties,
}

/// What one row of the description table starts from and asks for.
struct Row {
    name: &'static str,
    kind: Kind,
    protocol: &'static str,
    ports: Option<(i32, i32)>,
    /// The destination the rule is authorized with, which an update never changes.
    destination: &'static str,
    /// The destination the rule REPORTS, which differs from the one sent where AWS
    /// canonicalises it.
    stored_destination: &'static str,
    /// The description the rule starts with, `None` for a rule authorized without one.
    was: Option<&'static str>,
    /// The description the update asks for, `None` for the request that removes it.
    asks: Option<&'static str>,
    named_by: NamedBy,
}

/// Each rule a call names takes the description the request carries — replaced, added or
/// removed — while every other member of the rule, and the rule's identity, stay exactly as
/// they were, whatever the rule's type, its protocol, its port range and however the request
/// named it.
#[tokio::test]
async fn a_named_rule_takes_the_requested_description_and_keeps_everything_else() {
    let peer_group_slot = "sg-of-the-web-tier";
    let prefix_list = published_prefix_list(0);
    let rows = [
        Row {
            name: "an IPv4 rule's description is replaced, named by rule id",
            kind: Kind::Ipv4,
            protocol: "tcp",
            ports: Some((22, 22)),
            destination: "203.0.113.0/24",
            stored_destination: "203.0.113.0/24",
            was: Some("ssh to the office"),
            asks: Some("ssh to the branch office"),
            named_by: NamedBy::RuleId,
        },
        Row {
            name: "a rule authorized without a description gains one, named by rule id",
            kind: Kind::Ipv4,
            protocol: "tcp",
            ports: Some((8080, 8090)),
            // A block whose host bits are set is stored canonicalised, which the update
            // leaves exactly as the authorize stored it.
            destination: "198.51.100.17/24",
            stored_destination: "198.51.100.0/24",
            was: None,
            asks: Some("the app tier"),
            named_by: NamedBy::RuleId,
        },
        Row {
            name: "an IPv6 rule's description is removed by omitting it, named by rule id",
            kind: Kind::Ipv6,
            protocol: "udp",
            ports: Some((53, 53)),
            destination: "2001:db8::/32",
            stored_destination: "2001:db8::/32",
            was: Some("dns"),
            asks: None,
            named_by: NamedBy::RuleId,
        },
        Row {
            name: "an all-protocols rule with no port range takes a description, named by \
                   rule properties",
            kind: Kind::Ipv4,
            protocol: "-1",
            ports: None,
            destination: "192.0.2.0/25",
            stored_destination: "192.0.2.0/25",
            was: None,
            asks: Some("everything, for now"),
            named_by: NamedBy::Properties,
        },
        Row {
            name: "an ICMP rule's type and code are left alone while its description changes",
            kind: Kind::Ipv4,
            protocol: "icmp",
            ports: Some((8, 0)),
            destination: "203.0.113.128/25",
            stored_destination: "203.0.113.128/25",
            was: Some("echo request"),
            asks: Some("echo request (monitoring)"),
            named_by: NamedBy::Properties,
        },
        Row {
            name: "an IPv6 rule's description is removed by omitting it, named by rule \
                   properties",
            kind: Kind::Ipv6,
            protocol: "tcp",
            ports: Some((443, 443)),
            destination: "2001:db8:1::/48",
            stored_destination: "2001:db8:1::/48",
            was: Some("https, somewhere"),
            asks: None,
            named_by: NamedBy::Properties,
        },
        Row {
            name: "a prefix-list rule's description is replaced, named by rule id",
            kind: Kind::PrefixList,
            protocol: "tcp",
            ports: Some((443, 443)),
            destination: "",
            stored_destination: "",
            was: Some("the published ranges of a service"),
            asks: Some("the published ranges: tier-2/app (primary)"),
            named_by: NamedBy::RuleId,
        },
        Row {
            name: "a referenced-group rule's description is added, named by rule properties",
            kind: Kind::ReferencedGroup,
            protocol: "tcp",
            ports: Some((3306, 3306)),
            destination: peer_group_slot,
            stored_destination: peer_group_slot,
            was: None,
            asks: Some("to the database tier"),
            named_by: NamedBy::Properties,
        },
    ];

    for row in rows {
        let case = row.name;
        let router = emulator();
        let group_id = create_group(case, &router, "described-rules").await;
        // The destination a row cannot write down until the emulator has minted it: the
        // peer group's id and the Region's own published prefix list.
        let peer_group = create_group(case, &router, "web-tier").await;
        let destination = match row.kind {
            Kind::PrefixList => prefix_list.clone(),
            Kind::ReferencedGroup => peer_group.clone(),
            _ => row.destination.to_string(),
        };
        let stored_destination = match row.kind {
            Kind::PrefixList => prefix_list.clone(),
            Kind::ReferencedGroup => peer_group.clone(),
            _ => row.stored_destination.to_string(),
        };
        let authorized = Rule {
            protocol: row.protocol,
            ports: row.ports,
            kind: row.kind,
            destination: &destination,
            description: row.was,
        };
        let rule_id = authorize_egress(case, &router, &group_id, &authorized).await;
        let before = rule(case, &router, &group_id, &rule_id).await;
        assert_eq!(
            before.description.as_deref(),
            row.was,
            "case {case:?}: the authorized rule must start with the row's description, \
             got {actual:?}",
            actual = before.description,
        );
        assert_eq!(
            before.destination,
            row.kind.expected(&stored_destination),
            "case {case:?}: expected the rule to name {expected}, got {actual:?}",
            expected = row.kind.expected(&stored_destination),
            actual = before.destination,
        );

        let params = match row.named_by {
            NamedBy::RuleId => {
                let mut params = vec![param("GroupId", &group_id)];
                params.extend(description_params(1, &rule_id, row.asks));
                params
            }
            NamedBy::Properties => {
                let mut params = vec![param("GroupId", &group_id)];
                params.extend(permission_params(1, &authorized, row.asks));
                params
            }
        };
        let (status, body) = update(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {status} with body {body}"
        );
        assert_eq!(
            returned(case, &body),
            "true",
            "case {case:?}: expected <return>true</return>, got body {body}"
        );

        let after = rule(case, &router, &group_id, &rule_id).await;
        let expected = Reported {
            description: row.asks.map(str::to_string),
            ..before.clone()
        };
        assert_eq!(
            after, expected,
            "case {case:?}: expected the rule to differ only in its description \
             ({expected:?}), got {after:?}"
        );
        // The group's OTHER outbound rule — the allow-all rule every new group carries — is
        // not a rule the request named, so it is untouched.
        let others: Vec<Reported> = rules_of(case, &router, &group_id)
            .await
            .into_iter()
            .filter(|reported| reported.id != rule_id)
            .collect();
        assert!(
            others.iter().all(|reported| reported.description.is_none()),
            "case {case:?}: expected the rules the request did not name to keep no \
             description, got {others:?}"
        );
    }
}

/// One call describes SEVERAL rules at once — by several rule ids, and by one permission whose
/// grants name several rules — and each rule takes its own description.
#[tokio::test]
async fn one_call_describes_every_rule_it_names() {
    let case = "two rules by id and two rules by one permission";
    let router = emulator();
    let group_id = create_group(case, &router, "many-described-rules").await;

    let first = Rule {
        protocol: "tcp",
        ports: Some((80, 80)),
        kind: Kind::Ipv4,
        destination: "203.0.113.0/24",
        description: Some("http, first"),
    };
    let second = Rule {
        protocol: "tcp",
        ports: Some((80, 80)),
        kind: Kind::Ipv4,
        destination: "198.51.100.0/24",
        description: None,
    };
    let untouched = Rule {
        protocol: "udp",
        ports: Some((123, 123)),
        kind: Kind::Ipv4,
        destination: "192.0.2.0/24",
        description: Some("ntp"),
    };
    let first_id = authorize_egress(case, &router, &group_id, &first).await;
    let second_id = authorize_egress(case, &router, &group_id, &second).await;
    let untouched_id = authorize_egress(case, &router, &group_id, &untouched).await;

    let mut params = vec![param("GroupId", &group_id)];
    params.extend(description_params(1, &first_id, Some("http, described")));
    params.extend(description_params(2, &second_id, Some("http, also described")));
    let (status, body) = update(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the update, got {status} with body {body}"
    );
    assert_eq!(
        rule(case, &router, &group_id, &first_id)
            .await
            .description
            .as_deref(),
        Some("http, described"),
        "case {case:?}: expected the first named rule to take its own description"
    );
    assert_eq!(
        rule(case, &router, &group_id, &second_id)
            .await
            .description
            .as_deref(),
        Some("http, also described"),
        "case {case:?}: expected the second named rule to take its own description"
    );
    assert_eq!(
        rule(case, &router, &group_id, &untouched_id)
            .await
            .description
            .as_deref(),
        Some("ntp"),
        "case {case:?}: expected the rule the request did not name to keep its description"
    );

    // The same, by ONE permission naming two destinations: each grant's description is the
    // description of the rule that grant names.
    let case = "one permission naming two destinations";
    let params = vec![
        param("GroupId", &group_id),
        param("IpPermissions.1.IpProtocol", "tcp"),
        param("IpPermissions.1.FromPort", "80"),
        param("IpPermissions.1.ToPort", "80"),
        param("IpPermissions.1.IpRanges.1.CidrIp", "203.0.113.0/24"),
        param("IpPermissions.1.IpRanges.1.Description", "http, by property"),
        param("IpPermissions.1.IpRanges.2.CidrIp", "198.51.100.0/24"),
        param("IpPermissions.1.IpRanges.2.Description", "http, by property too"),
    ];
    let (status, body) = update(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the update, got {status} with body {body}"
    );
    assert_eq!(
        rule(case, &router, &group_id, &first_id)
            .await
            .description
            .as_deref(),
        Some("http, by property"),
        "case {case:?}: expected the first grant's rule to take the first description"
    );
    assert_eq!(
        rule(case, &router, &group_id, &second_id)
            .await
            .description
            .as_deref(),
        Some("http, by property too"),
        "case {case:?}: expected the second grant's rule to take the second description"
    );
}

/// The description a rule takes is reported by the AGGREGATE view of the group's outbound
/// rules as well, which no update writes directly — including on the allow-all rule the
/// service itself wrote when the group was created.
#[tokio::test]
async fn the_aggregate_view_reports_the_description() {
    let case = "the service-written allow-all egress rule takes a description";
    let router = emulator();
    let group_id = create_group(case, &router, "aggregate-view").await;

    // The rule every new group carries, which no caller authorized: "a security group has an
    // outbound rule that allows all outbound traffic".
    let rules = rules_of(case, &router, &group_id).await;
    let [allow_all] = rules.as_slice() else {
        panic!("case {case:?}: expected the new group to hold exactly one outbound rule, got {rules:?}");
    };
    assert_eq!(
        allow_all.description, None,
        "case {case:?}: expected the allow-all rule to carry no description"
    );

    let mut params = vec![param("GroupId", &group_id)];
    params.extend(description_params(
        1,
        &allow_all.id,
        Some("all outbound traffic, on purpose"),
    ));
    let (status, body) = update(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the update, got {status} with body {body}"
    );

    let after = rule(case, &router, &group_id, &allow_all.id).await;
    assert_eq!(
        after,
        Reported {
            description: Some("all outbound traffic, on purpose".to_string()),
            ..allow_all.clone()
        },
        "case {case:?}: expected the allow-all rule to differ only in its description, \
         got {after:?}"
    );
    let aggregate = aggregated_egress(case, &router, &group_id).await;
    assert!(
        aggregate.contains("<description>all outbound traffic, on purpose</description>"),
        "case {case:?}: expected the aggregate outbound view to report the new description, \
         got {aggregate}"
    );

    // And the removal is reported there too: an update carrying no description takes the one
    // the rule had out of both views.
    let case = "the description is removed from both views";
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(description_params(1, &allow_all.id, None));
    let (status, body) = update(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the update, got {status} with body {body}"
    );
    assert_eq!(
        rule(case, &router, &group_id, &allow_all.id)
            .await
            .description,
        None,
        "case {case:?}: expected the description to be removed"
    );
    let aggregate = aggregated_egress(case, &router, &group_id).await;
    assert!(
        !aggregate.contains("<description>"),
        "case {case:?}: expected the aggregate outbound view to report no description, \
         got {aggregate}"
    );
}

/// A description is held to the bounds the model publishes for one: "up to 255 characters in
/// length" (`IpRange$Description`), so one AT the bound is stored and one past it is refused
/// with nothing written.
#[tokio::test]
async fn a_description_is_held_to_its_published_bound() {
    let limit = ec2::state::MAX_SECURITY_GROUP_TEXT_CHARS;
    let rows = [
        ("one character under the bound", limit - 1, true),
        ("exactly at the bound", limit, true),
        ("one character past the bound", limit + 1, false),
    ];
    for (case, length, accepted) in rows {
        let router = emulator();
        let group_id = create_group(case, &router, "bounded-descriptions").await;
        let authorized = Rule {
            protocol: "tcp",
            ports: Some((22, 22)),
            kind: Kind::Ipv4,
            destination: "203.0.113.0/24",
            description: Some("as authorized"),
        };
        let rule_id = authorize_egress(case, &router, &group_id, &authorized).await;
        let description = "d".repeat(length);
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(description_params(1, &rule_id, Some(&description)));
        let (status, body) = update(&router, &params).await;
        let stored = rule(case, &router, &group_id, &rule_id).await.description;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for a {length}-character description, \
                 got {status} with body {body}"
            );
            assert_eq!(
                stored.as_deref(),
                Some(description.as_str()),
                "case {case:?}: expected the {length}-character description to be stored, \
                 got {stored:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a {length}-character description, \
                 got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got body {body}"
            );
            assert_eq!(
                stored.as_deref(),
                Some("as authorized"),
                "case {case:?}: expected the refused request to write nothing, got {stored:?}"
            );
        }
    }
}

/// A request the service cannot serve is refused with the error class EC2 gives it, and
/// nothing is written: the rule keeps the description it had.
#[tokio::test]
async fn an_unservable_request_is_refused_and_writes_nothing() {
    /// What one row sends, built from the group id and the rule id the fixture minted.
    type Sends = fn(&str, &str) -> Vec<Param>;
    let rows: [(&str, Sends, &str); 10] = [
        (
            "a rule id of the right shape that names no rule",
            |group_id, _| {
                let mut params = vec![param("GroupId", group_id)];
                params.extend(description_params(
                    1,
                    "sgr-00000000000000000",
                    Some("nothing to describe"),
                ));
                params
            },
            "InvalidSecurityGroupRuleId.NotFound",
        ),
        (
            "a value that is not a security group rule id at all",
            |group_id, _| {
                let mut params = vec![param("GroupId", group_id)];
                params.extend(description_params(1, "not-a-rule-id", Some("no")));
                params
            },
            "InvalidSecurityGroupRuleId.Malformed",
        ),
        (
            "one rule id named twice",
            |group_id, rule_id| {
                let mut params = vec![param("GroupId", group_id)];
                params.extend(description_params(1, rule_id, Some("one thing")));
                params.extend(description_params(2, rule_id, Some("another thing")));
                params
            },
            "InvalidParameterValue",
        ),
        (
            "rule properties that match no rule of the group",
            |group_id, _| {
                vec![
                    param("GroupId", group_id),
                    param("IpPermissions.1.IpProtocol", "tcp"),
                    param("IpPermissions.1.FromPort", "22"),
                    param("IpPermissions.1.ToPort", "22"),
                    param("IpPermissions.1.IpRanges.1.CidrIp", "10.0.0.0/8"),
                    param("IpPermissions.1.IpRanges.1.Description", "no such rule"),
                ]
            },
            "InvalidPermission.NotFound",
        ),
        (
            "rule properties that match the rule except for its port range",
            |group_id, _| {
                vec![
                    param("GroupId", group_id),
                    param("IpPermissions.1.IpProtocol", "tcp"),
                    param("IpPermissions.1.FromPort", "23"),
                    param("IpPermissions.1.ToPort", "23"),
                    param("IpPermissions.1.IpRanges.1.CidrIp", "203.0.113.0/24"),
                    param("IpPermissions.1.IpRanges.1.Description", "no such rule"),
                ]
            },
            "InvalidPermission.NotFound",
        ),
        (
            "a permission naming no protocol",
            |group_id, _| {
                vec![
                    param("GroupId", group_id),
                    param("IpPermissions.1.IpRanges.1.CidrIp", "203.0.113.0/24"),
                    param("IpPermissions.1.IpRanges.1.Description", "which protocol?"),
                ]
            },
            "MissingParameter",
        ),
        (
            "both ways of naming the rules at once",
            |group_id, rule_id| {
                let mut params = vec![
                    param("GroupId", group_id),
                    param("IpPermissions.1.IpProtocol", "tcp"),
                    param("IpPermissions.1.FromPort", "22"),
                    param("IpPermissions.1.ToPort", "22"),
                    param("IpPermissions.1.IpRanges.1.CidrIp", "203.0.113.0/24"),
                ];
                params.extend(description_params(1, rule_id, Some("by id as well")));
                params
            },
            "InvalidParameterCombination",
        ),
        (
            "neither way of naming the rules",
            |group_id, _| vec![param("GroupId", group_id)],
            "MissingParameter",
        ),
        (
            "a group id of the right shape that names no group",
            |_, rule_id| {
                let mut params = vec![param("GroupId", "sg-00000000000000000")];
                params.extend(description_params(1, rule_id, Some("no such group")));
                params
            },
            "InvalidGroup.NotFound",
        ),
        (
            "a value that is not a security group id at all",
            |_, rule_id| {
                let mut params = vec![param("GroupId", "not-a-group-id")];
                params.extend(description_params(1, rule_id, Some("no such group")));
                params
            },
            "InvalidGroupId.Malformed",
        ),
    ];

    for (case, sends, expected) in rows {
        let router = emulator();
        let group_id = create_group(case, &router, "refused-descriptions").await;
        let authorized = Rule {
            protocol: "tcp",
            ports: Some((22, 22)),
            kind: Kind::Ipv4,
            destination: "203.0.113.0/24",
            description: Some("as authorized"),
        };
        let rule_id = authorize_egress(case, &router, &group_id, &authorized).await;
        let params = sends(&group_id, &rule_id);
        let (status, body) = update(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected error code {expected}, got body {body}"
        );
        let stored = rule(case, &router, &group_id, &rule_id).await.description;
        assert_eq!(
            stored.as_deref(),
            Some("as authorized"),
            "case {case:?}: expected the refused request to write nothing, got {stored:?}"
        );
    }
}

/// An INBOUND rule is not a rule this operation can describe: its id names nothing here, and
/// the inbound rule keeps the description it had.
#[tokio::test]
async fn an_inbound_rule_is_not_described_by_the_outbound_call() {
    let case = "an inbound rule's id named on the outbound call";
    let router = emulator();
    let group_id = create_group(case, &router, "one-side-at-a-time").await;
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "AuthorizeSecurityGroupIngress",
        &[
            param("GroupId", &group_id),
            param("IpPermissions.1.IpProtocol", "tcp"),
            param("IpPermissions.1.FromPort", "22"),
            param("IpPermissions.1.ToPort", "22"),
            param("IpPermissions.1.IpRanges.1.CidrIp", "203.0.113.0/24"),
            param("IpPermissions.1.IpRanges.1.Description", "inbound ssh"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AuthorizeSecurityGroupIngress, got {status} \
         with body {body}"
    );
    let inbound_id = element(&body, "securityGroupRuleId")
        .unwrap_or_else(|| panic!("case {case:?}: no <securityGroupRuleId> in {body}"));

    let mut params = vec![param("GroupId", &group_id)];
    params.extend(description_params(1, &inbound_id, Some("outbound ssh")));
    let (status, body) = update(&router, &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidSecurityGroupRuleId.NotFound",
        "case {case:?}: expected the inbound rule to name no outbound rule, got body {body}"
    );
    assert_eq!(
        rule(case, &router, &group_id, &inbound_id)
            .await
            .description
            .as_deref(),
        Some("inbound ssh"),
        "case {case:?}: expected the inbound rule to keep its description"
    );
}

/// "If you have the required permissions, the error response is DryRunOperation" (`$DryRun`):
/// a dry run of a request that would have succeeded reports that and writes nothing.
#[tokio::test]
async fn a_dry_run_reports_the_operation_and_writes_nothing() {
    let rows = [NamedBy::RuleId, NamedBy::Properties];
    for named_by in rows {
        let case = match named_by {
            NamedBy::RuleId => "a dry run naming the rule by id",
            NamedBy::Properties => "a dry run naming the rule by its properties",
        };
        let router = emulator();
        let group_id = create_group(case, &router, "dry-run-descriptions").await;
        let authorized = Rule {
            protocol: "tcp",
            ports: Some((22, 22)),
            kind: Kind::Ipv4,
            destination: "203.0.113.0/24",
            description: Some("as authorized"),
        };
        let rule_id = authorize_egress(case, &router, &group_id, &authorized).await;
        let mut params = vec![param("GroupId", &group_id), param("DryRun", "true")];
        match named_by {
            NamedBy::RuleId => {
                params.extend(description_params(1, &rule_id, Some("never written")))
            }
            NamedBy::Properties => {
                params.extend(permission_params(1, &authorized, Some("never written")))
            }
        }
        let (status, body) = update(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from the dry run, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got body {body}"
        );
        let stored = rule(case, &router, &group_id, &rule_id).await.description;
        assert_eq!(
            stored.as_deref(),
            Some("as authorized"),
            "case {case:?}: expected the dry run to write nothing, got {stored:?}"
        );
    }
}

/// A group named by NAME is the same group as one named by id — "[Default VPC] The name of the
/// security group" (`$GroupName`) — so the update reaches the same rule either way.
#[tokio::test]
async fn a_group_named_by_name_describes_the_same_rule() {
    let case = "the group named by name";
    let router = emulator();
    let group_name = "named-by-name";
    let group_id = create_group(case, &router, group_name).await;
    let authorized = Rule {
        protocol: "tcp",
        ports: Some((22, 22)),
        kind: Kind::Ipv4,
        destination: "203.0.113.0/24",
        description: Some("as authorized"),
    };
    let rule_id = authorize_egress(case, &router, &group_id, &authorized).await;
    let mut params = vec![param("GroupName", group_name)];
    params.extend(description_params(1, &rule_id, Some("described by name")));
    let (status, body) = update(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the update, got {status} with body {body}"
    );
    assert_eq!(
        rule(case, &router, &group_id, &rule_id)
            .await
            .description
            .as_deref(),
        Some("described by name"),
        "case {case:?}: expected the rule named through the group's name to be described"
    );
}
