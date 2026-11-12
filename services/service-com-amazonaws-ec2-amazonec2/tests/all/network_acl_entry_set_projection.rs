// SPDX-License-Identifier: CC0-1.0

//! The ONE projection of a network ACL's rule table, whichever reader reports it and
//! whichever writer wrote it. Driven over the wire the service actually speaks: `ec2Query` —
//! an `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test: a network ACL's reported `entrySet` is that ACL's whole stored rule
//! table, projected ONE way — the egress rules first, then the ingress rules, each by
//! ascending rule number — and every reported entry carries both "the ICMP type and code"
//! (`NetworkAclEntry$IcmpTypeCode`) and "the range of ports the rule applies to"
//! (`$PortRange`), EMPTY for the one the rule's protocol has no use for, beside the rule
//! number, protocol, action, direction and range the rule was written with.
//!
//! So the list `CreateNetworkAcl` answers, the list `DescribeNetworkAcls` answers for that
//! same ACL — named by id, selected by a filter, or listed with the whole store — and the
//! list the ACL holds after each rule a caller adds or removes are all the SAME list. The
//! rows below differ along everything that rule generalises over: whether the ACL is the one
//! its VPC came with or one a caller created, the direction, the rule number (including each
//! published bound of the range a caller may write), the protocol (every protocol, TCP, UDP,
//! ICMP, ICMPv6), the address family, whether the rule allows or denies, and the ORDER the
//! rules were created in — so no single recorded request is the specification.
//!
//! What each operation on an ACL's entries refuses, and what a create stores for one rule,
//! is covered by [`super::network_acl_entries`]; the filters, id lists and pages of the
//! describe by [`super::describe_network_acls`]. This suite asserts only that every reader
//! of an ACL projects its entries identically.

use ec2::state::{
    ALL_IPV4_ADDRESSES, ALL_IP_PROTOCOLS, DEFAULT_NETWORK_ACL_ALLOW_RULE_NUMBER,
    DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER, NETWORK_ACL_ENTRY_RULE_NUMBERS,
    NETWORK_ACL_PROTOCOL_ICMP, NETWORK_ACL_PROTOCOL_ICMPV6, NETWORK_ACL_PROTOCOL_TCP,
    NETWORK_ACL_PROTOCOL_UDP, RULE_ACTION_ALLOW, RULE_ACTION_DENY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, filters, indexed};
use super::wire::{child, child_text, document, param, set_items};

/// The Region every call here is signed for. A network ACL carries no Region member, and the
/// default VPC a fresh account already has is seeded for the emulator's default Region.
const REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// Reading an ACL's entries off the wire
// ---------------------------------------------------------------------------

/// One entry of a reported network ACL, with every member of the modelled `NetworkAclEntry`
/// told apart from an ABSENT one: `None` is a member the response left out, and
/// `Some((None, None))` one it wrote as a present-but-empty element.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedEntry {
    rule_number: String,
    rule_action: String,
    protocol: String,
    cidr_block: Option<String>,
    ipv6_cidr_block: Option<String>,
    egress: String,
    port_range: Option<(Option<String>, Option<String>)>,
    icmp_type_code: Option<(Option<String>, Option<String>)>,
}

/// The entries the ACL `network_acl_id` carries in `body`, in the order the response reported
/// them.
///
/// Read off the TREE: an entry's members are children of its own `<item>`, so scanning the
/// body for an element name would mix two entries' ports together, and an ACL is the element
/// carrying both that id and an `entrySet` — which is how a create's `<networkAcl>` and a
/// describe's `<item>` are both written, so one function reads either.
fn entries_of(case: &str, reader: &str, body: &str, network_acl_id: &str) -> Vec<ReportedEntry> {
    let parsed = document(case, body);
    let acl = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "networkAclId").as_deref() == Some(network_acl_id)
                && child(*node, "entrySet").is_some()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?} / {reader}: no network ACL {network_acl_id} in {body}")
        });
    set_items(acl, "entrySet")
        .into_iter()
        .map(|entry| ReportedEntry {
            rule_number: child_text(entry, "ruleNumber").unwrap_or_default(),
            rule_action: child_text(entry, "ruleAction").unwrap_or_default(),
            protocol: child_text(entry, "protocol").unwrap_or_default(),
            cidr_block: child_text(entry, "cidrBlock"),
            ipv6_cidr_block: child_text(entry, "ipv6CidrBlock"),
            egress: child_text(entry, "egress").unwrap_or_default(),
            port_range: child(entry, "portRange")
                .map(|range| (child_text(range, "from"), child_text(range, "to"))),
            icmp_type_code: child(entry, "icmpTypeCode")
                .map(|icmp| (child_text(icmp, "type"), child_text(icmp, "code"))),
        })
        .collect()
}

/// `entries` in the order an ACL reports them: the egress rules first, then the ingress
/// rules, each by ascending rule number — the rule this suite holds every reader to, stated
/// once here so each expectation below is the rule applied rather than a list written out.
fn in_report_order(mut entries: Vec<ReportedEntry>) -> Vec<ReportedEntry> {
    entries.sort_by_key(|entry| {
        (
            // `false` sorts first, so the egress rules lead.
            entry.egress != "true",
            // A rule number is "a positive integer" (`NetworkAclEntry$RuleNumber` targets an
            // Integer), so an entry whose rule number does not parse is one this order says
            // nothing about — louder than a value normalised to zero and compared anyway.
            entry.rule_number.parse::<i32>().unwrap_or_else(|error| {
                panic!(
                    "an entry's ruleNumber must be an integer ({error}), got {:?} on {entry:?}",
                    entry.rule_number,
                )
            }),
        )
    });
    entries
}

// ---------------------------------------------------------------------------
// The rules a table row writes, and what each must read back as
// ---------------------------------------------------------------------------

/// The protocol number `number` as a request spells it: "The protocol number"
/// (`CreateNetworkAclEntryRequest$Protocol`).
fn protocol(number: i32) -> String {
    number.to_string()
}

/// ONE rule a row asks for, as both the request that writes it and the entry it must read
/// back as are built from.
#[derive(Debug, Clone)]
struct Rule {
    rule_number: i32,
    egress: bool,
    rule_action: &'static str,
    protocol: String,
    cidr_block: Option<String>,
    ipv6_cidr_block: Option<String>,
    port_range: Option<(i32, i32)>,
    icmp_type_code: Option<(i32, i32)>,
}

impl Rule {
    /// A rule at `rule_number` in the direction `egress` names, over `protocol`.
    fn new(rule_number: i32, egress: bool, rule_action: &'static str, protocol: String) -> Self {
        Self {
            rule_number,
            egress,
            rule_action,
            protocol,
            cidr_block: None,
            ipv6_cidr_block: None,
            port_range: None,
            icmp_type_code: None,
        }
    }

    /// Over "the IPv4 network range to allow or deny, in CIDR notation" (`$CidrBlock`).
    fn cidr(mut self, cidr_block: &str) -> Self {
        self.cidr_block = Some(cidr_block.to_string());
        self
    }

    /// Over "the IPv6 network range to allow or deny" (`$Ipv6CidrBlock`).
    fn ipv6_cidr(mut self, ipv6_cidr_block: &str) -> Self {
        self.ipv6_cidr_block = Some(ipv6_cidr_block.to_string());
        self
    }

    /// With "the range of ports the rule applies to" (`$PortRange`), which TCP and UDP
    /// require.
    fn ports(mut self, from: i32, to: i32) -> Self {
        self.port_range = Some((from, to));
        self
    }

    /// With "the ICMP or ICMPv6 type and code" (`$IcmpTypeCode`), which ICMP requires.
    fn icmp(mut self, icmp_type: i32, code: i32) -> Self {
        self.icmp_type_code = Some((icmp_type, code));
        self
    }

    /// The form fields of the `CreateNetworkAclEntry` request that writes this rule onto
    /// `network_acl_id`.
    fn params(&self, network_acl_id: &str) -> Vec<(String, String)> {
        let mut params = vec![
            param("NetworkAclId", network_acl_id),
            param("RuleNumber", &self.rule_number.to_string()),
            param("Egress", &self.egress.to_string()),
            param("RuleAction", self.rule_action),
            param("Protocol", &self.protocol),
        ];
        if let Some(cidr_block) = self.cidr_block.as_deref() {
            params.push(param("CidrBlock", cidr_block));
        }
        if let Some(ipv6_cidr_block) = self.ipv6_cidr_block.as_deref() {
            params.push(param("Ipv6CidrBlock", ipv6_cidr_block));
        }
        if let Some((from, to)) = self.port_range {
            params.push(param("PortRange.From", &from.to_string()));
            params.push(param("PortRange.To", &to.to_string()));
        }
        if let Some((icmp_type, code)) = self.icmp_type_code {
            params.push(param("Icmp.Type", &icmp_type.to_string()));
            params.push(param("Icmp.Code", &code.to_string()));
        }
        params
    }

    /// The entry this rule must be reported as: its own members, with the port range and the
    /// ICMP type and code both PRESENT — empty for the one its protocol has no use for.
    fn reported(&self) -> ReportedEntry {
        ReportedEntry {
            rule_number: self.rule_number.to_string(),
            rule_action: self.rule_action.to_string(),
            protocol: self.protocol.clone(),
            cidr_block: self.cidr_block.clone(),
            ipv6_cidr_block: self.ipv6_cidr_block.clone(),
            egress: self.egress.to_string(),
            port_range: Some(self.port_range.map_or((None, None), |(from, to)| {
                (Some(from.to_string()), Some(to.to_string()))
            })),
            icmp_type_code: Some(self.icmp_type_code.map_or((None, None), |(icmp, code)| {
                (Some(icmp.to_string()), Some(code.to_string()))
            })),
        }
    }
}

/// One rule-32767 entry every network ACL ends with, in the direction `egress` names: the
/// catch-all deny over every protocol and every IPv4 address, carrying both the port range
/// and the ICMP type and code as present-but-empty members.
fn catch_all(egress: bool) -> ReportedEntry {
    every_protocol_entry(DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER, egress, RULE_ACTION_DENY)
}

/// The allow-everything entry a VPC's DEFAULT network ACL carries in each direction, which
/// "allows all inbound and outbound traffic" (Amazon VPC User Guide, "Default network ACL").
fn allow_everything(egress: bool) -> ReportedEntry {
    every_protocol_entry(
        DEFAULT_NETWORK_ACL_ALLOW_RULE_NUMBER,
        egress,
        RULE_ACTION_ALLOW,
    )
}

/// An entry over every protocol and every IPv4 address — the shape both entries a network
/// ACL is created with take.
fn every_protocol_entry(rule_number: i32, egress: bool, rule_action: &str) -> ReportedEntry {
    ReportedEntry {
        rule_number: rule_number.to_string(),
        rule_action: rule_action.to_string(),
        protocol: ALL_IP_PROTOCOLS.to_string(),
        cidr_block: Some(ALL_IPV4_ADDRESSES.to_string()),
        ipv6_cidr_block: None,
        egress: egress.to_string(),
        port_range: Some((None, None)),
        icmp_type_code: Some((None, None)),
    }
}

/// The entries a network ACL a CALLER created comes with: the catch-all deny in each
/// direction, in report order.
fn created_acl_entries() -> Vec<ReportedEntry> {
    in_report_order(vec![catch_all(false), catch_all(true)])
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

/// One `DescribeNetworkAcls` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeNetworkAcls", params).await
}

/// A VPC created over the wire, by id.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let params = [param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// The account's default VPC id, read back through `DescribeVpcs`.
async fn default_vpc_id(router: &MultiServiceRouter) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"][..])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeVpcs: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("DescribeVpcs answered no vpcId: {body}"))
}

/// The id of the one DEFAULT network ACL of `vpc_id` — the ACL that VPC came with.
async fn default_acl_of_vpc(router: &MultiServiceRouter, case: &str, vpc_id: &str) -> String {
    let params = filters(&[("vpc-id", &[vpc_id][..]), ("default", &["true"][..])]);
    let (status, body) = describe(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let ids: Vec<String> = set_items(parsed.root_element(), "networkAclSet")
        .into_iter()
        .filter_map(|item| child_text(item, "networkAclId"))
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: a VPC has exactly one default network ACL, got {ids:?}"
    );
    ids[0].clone()
}

/// One ACL created in `vpc_id`, as its id and the entries the CREATE itself answered.
async fn create_acl(
    router: &MultiServiceRouter,
    case: &str,
    vpc_id: &str,
) -> (String, Vec<ReportedEntry>) {
    let params = [param("VpcId", vpc_id)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateNetworkAcl", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateNetworkAcl, got {status} with body {body}"
    );
    let network_acl_id = element(&body, "networkAclId")
        .unwrap_or_else(|| panic!("case {case:?}: no networkAclId in {body}"));
    let entries = entries_of(case, "CreateNetworkAcl", &body, &network_acl_id);
    (network_acl_id, entries)
}

/// Write `rule` onto `network_acl_id` through `CreateNetworkAclEntry`.
async fn create_entry(router: &MultiServiceRouter, case: &str, network_acl_id: &str, rule: &Rule) {
    let params = rule.params(network_acl_id);
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateNetworkAclEntry", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for rule {rule:?}, got {status} with body {body}"
    );
}

/// Every reader of the ACL `network_acl_id` reports exactly `expected`.
///
/// The three ways a caller reaches one ACL through `DescribeNetworkAcls`: named by id,
/// selected by a filter on its VPC, and listed with the whole store. A projection that any
/// one of them computed differently would be a list the caller cannot rely on.
async fn assert_every_reader(
    router: &MultiServiceRouter,
    case: &str,
    network_acl_id: &str,
    vpc_id: &str,
    expected: &[ReportedEntry],
) {
    let readers = [
        (
            "DescribeNetworkAcls by id",
            indexed("NetworkAclId", &[network_acl_id]),
        ),
        (
            "DescribeNetworkAcls filtered by vpc-id",
            filters(&[("vpc-id", &[vpc_id][..])]),
        ),
        ("DescribeNetworkAcls over the whole store", Vec::new()),
    ];
    for (reader, params) in readers {
        let (status, body) = describe(router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?} / {reader}: expected HTTP 200, got {status} with body {body}"
        );
        let actual = entries_of(case, reader, &body, network_acl_id);
        assert_eq!(
            actual.as_slice(),
            expected,
            "case {case:?} / {reader}: expected the entries {expected:?}, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ACLs a VPC and a caller create
// ---------------------------------------------------------------------------

/// Every reader projects the entries a network ACL is CREATED with the same way: the two
/// rules of a VPC's own default ACL in each direction, and the catch-all deny in each
/// direction for an ACL a caller created — egress first, each by ascending rule number, each
/// carrying the port range and the ICMP type and code as present-but-empty members.
#[tokio::test]
async fn every_reader_projects_the_entries_an_acl_is_created_with_the_same_way() {
    let (router, _engine) = super::ec2_query::emulator();
    let default_vpc = default_vpc_id(&router).await;
    let made_vpc = create_vpc(&router, "10.60.0.0/16").await;

    // A DEFAULT ACL, for the VPC a fresh account already has and for one created over the
    // wire: allow everything at rule 100 and deny everything at 32767, in each direction.
    let default_entries = in_report_order(
        [false, true]
            .into_iter()
            .flat_map(|egress| [allow_everything(egress), catch_all(egress)])
            .collect(),
    );
    for (case, vpc_id) in [
        (
            "the default ACL of the VPC a fresh account already has",
            default_vpc.as_str(),
        ),
        (
            "the default ACL of a VPC created over the wire",
            made_vpc.as_str(),
        ),
    ] {
        let network_acl_id = default_acl_of_vpc(&router, case, vpc_id).await;
        assert_every_reader(&router, case, &network_acl_id, vpc_id, &default_entries).await;
    }

    // An ACL a CALLER created: the catch-all deny in each direction and nothing else — and
    // the create's own answer is the list every reader goes on to report.
    for (case, vpc_id) in [
        ("an ACL created in the default VPC", default_vpc.as_str()),
        ("an ACL created in a VPC of its own", made_vpc.as_str()),
    ] {
        let (network_acl_id, created) = create_acl(&router, case, vpc_id).await;
        let expected = created_acl_entries();
        assert_eq!(
            created.as_slice(),
            expected.as_slice(),
            "case {case:?}: CreateNetworkAcl must answer {expected:?}, got {created:?}"
        );
        assert_every_reader(&router, case, &network_acl_id, vpc_id, &expected).await;
    }
}

// ---------------------------------------------------------------------------
// The rules a caller adds
// ---------------------------------------------------------------------------

/// One row of the table: the rules to write onto a fresh ACL, in the order the client sends
/// them.
struct Row {
    case: &'static str,
    rules: Vec<Rule>,
}

/// Every rule a caller writes is in the next read of the ACL, placed by the one order an ACL
/// reports its entries in — whatever order the rules were created in, and whichever reader
/// is asked.
///
/// The rows differ in the direction, the rule number (including each published bound of
/// "Positive integer from 1 to 32766", `CreateNetworkAclEntryRequest$RuleNumber`), the
/// protocol, the address family, the action, the port or ICMP numbers, and the creation
/// order.
#[tokio::test]
async fn each_rule_a_caller_writes_takes_its_place_in_the_one_reported_order() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.61.0.0/16").await;

    let rows = vec![
        Row {
            case: "a TCP rule in each direction at the same rule number, ingress first",
            rules: vec![
                Rule::new(100, false, RULE_ACTION_ALLOW, protocol(NETWORK_ACL_PROTOCOL_TCP))
                    .cidr("10.0.0.0/8")
                    .ports(80, 80),
                Rule::new(100, true, RULE_ACTION_ALLOW, protocol(NETWORK_ACL_PROTOCOL_TCP))
                    .cidr(ALL_IPV4_ADDRESSES)
                    .ports(443, 443),
            ],
        },
        Row {
            case: "three ingress rules written in DESCENDING rule order",
            rules: vec![
                Rule::new(300, false, RULE_ACTION_DENY, protocol(NETWORK_ACL_PROTOCOL_UDP))
                    .cidr("192.0.2.0/24")
                    .ports(53, 53),
                Rule::new(200, false, RULE_ACTION_ALLOW, protocol(NETWORK_ACL_PROTOCOL_ICMP))
                    .cidr(ALL_IPV4_ADDRESSES)
                    .icmp(8, -1),
                Rule::new(100, false, RULE_ACTION_ALLOW, ALL_IP_PROTOCOLS.to_string())
                    .cidr("198.51.100.0/24"),
            ],
        },
        Row {
            case: "egress and ingress interleaved, with the egress numbers out of order",
            rules: vec![
                Rule::new(500, true, RULE_ACTION_ALLOW, ALL_IP_PROTOCOLS.to_string())
                    .cidr(ALL_IPV4_ADDRESSES),
                Rule::new(400, false, RULE_ACTION_DENY, protocol(NETWORK_ACL_PROTOCOL_TCP))
                    .cidr("203.0.113.0/24")
                    .ports(22, 22),
                Rule::new(50, true, RULE_ACTION_ALLOW, protocol(NETWORK_ACL_PROTOCOL_UDP))
                    .cidr("10.61.0.0/16")
                    .ports(161, 162),
                Rule::new(
                    600,
                    false,
                    RULE_ACTION_ALLOW,
                    protocol(NETWORK_ACL_PROTOCOL_ICMPV6),
                )
                .ipv6_cidr("::/0")
                .icmp(128, 0),
            ],
        },
        Row {
            // Each published bound of the rule numbers a caller may write, read off the
            // range the crate states that constraint as.
            case: "one rule at each published bound of the rule-number range",
            rules: vec![
                Rule::new(
                    *NETWORK_ACL_ENTRY_RULE_NUMBERS.start(),
                    false,
                    RULE_ACTION_ALLOW,
                    ALL_IP_PROTOCOLS.to_string(),
                )
                .cidr(ALL_IPV4_ADDRESSES),
                Rule::new(
                    *NETWORK_ACL_ENTRY_RULE_NUMBERS.end(),
                    true,
                    RULE_ACTION_DENY,
                    ALL_IP_PROTOCOLS.to_string(),
                )
                .cidr("172.16.0.0/12"),
            ],
        },
        Row {
            case: "one rule number in both directions over the two address families",
            rules: vec![
                Rule::new(150, true, RULE_ACTION_DENY, ALL_IP_PROTOCOLS.to_string())
                    .ipv6_cidr("2001:db8::/32"),
                Rule::new(150, false, RULE_ACTION_DENY, ALL_IP_PROTOCOLS.to_string())
                    .cidr("10.62.0.0/16"),
            ],
        },
    ];

    for row in &rows {
        let (network_acl_id, _) = create_acl(&router, row.case, &vpc_id).await;
        let mut written: Vec<ReportedEntry> = created_acl_entries();
        for rule in &row.rules {
            create_entry(&router, row.case, &network_acl_id, rule).await;
            written.push(rule.reported());
            let expected = in_report_order(written.clone());
            assert_every_reader(&router, row.case, &network_acl_id, &vpc_id, &expected).await;
        }
    }
}

// ---------------------------------------------------------------------------
// The rules a caller removes and replaces
// ---------------------------------------------------------------------------

/// A rule a caller takes off the ACL stops being reported and the rules around it keep their
/// places, and a rule a caller REPLACES keeps its place while its members become the
/// replacement's — so the reported list tracks the stored rule table through every mutation
/// rather than being fixed at create time.
#[tokio::test]
async fn removing_and_replacing_a_rule_leaves_the_one_reported_order() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.63.0.0/16").await;
    let case = "a rule removed from the middle of each direction";
    let (network_acl_id, _) = create_acl(&router, case, &vpc_id).await;

    let rules = vec![
        Rule::new(100, false, RULE_ACTION_ALLOW, protocol(NETWORK_ACL_PROTOCOL_TCP))
            .cidr("10.0.0.0/8")
            .ports(80, 80),
        Rule::new(200, false, RULE_ACTION_DENY, ALL_IP_PROTOCOLS.to_string())
            .cidr(ALL_IPV4_ADDRESSES),
        Rule::new(100, true, RULE_ACTION_ALLOW, protocol(NETWORK_ACL_PROTOCOL_UDP))
            .cidr(ALL_IPV4_ADDRESSES)
            .ports(53, 53),
        Rule::new(200, true, RULE_ACTION_ALLOW, protocol(NETWORK_ACL_PROTOCOL_ICMP))
            .cidr(ALL_IPV4_ADDRESSES)
            .icmp(8, 0),
    ];
    for rule in &rules {
        create_entry(&router, case, &network_acl_id, rule).await;
    }

    // Each delete takes exactly the rule it names out of the direction it names, and the
    // rest of the list is the same list.
    let mut left: Vec<Rule> = rules.clone();
    for (rule_number, egress) in [(100, false), (200, true)] {
        let params = [
            param("NetworkAclId", &network_acl_id),
            param("RuleNumber", &rule_number.to_string()),
            param("Egress", &egress.to_string()),
        ];
        let (status, body) =
            super::ec2_query::call(&router, REGION, "DeleteNetworkAclEntry", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting rule {rule_number} (egress {egress}), \
             got {status} with body {body}"
        );
        left.retain(|rule| rule.rule_number != rule_number || rule.egress != egress);
        let mut expected = created_acl_entries();
        expected.extend(left.iter().map(Rule::reported));
        let expected = in_report_order(expected);
        assert_every_reader(&router, case, &network_acl_id, &vpc_id, &expected).await;
    }

    // A replacement keeps the rule's place and takes over its members: the surviving
    // every-protocol ingress rule becomes a TCP rule with a port range of its own.
    let case = "a rule replaced in place";
    let replacement = Rule::new(200, false, RULE_ACTION_ALLOW, protocol(NETWORK_ACL_PROTOCOL_TCP))
        .cidr("172.31.0.0/16")
        .ports(8080, 8443);
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "ReplaceNetworkAclEntry",
        &replacement.params(&network_acl_id),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ReplaceNetworkAclEntry, got {status} with body \
         {body}"
    );
    for rule in &mut left {
        if rule.rule_number == replacement.rule_number && rule.egress == replacement.egress {
            *rule = replacement.clone();
        }
    }
    let mut expected = created_acl_entries();
    expected.extend(left.iter().map(Rule::reported));
    let expected = in_report_order(expected);
    assert_every_reader(&router, case, &network_acl_id, &vpc_id, &expected).await;
}
