// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyCacheSubnetGroup`, read back through
//! `DescribeCacheSubnetGroups`.
//!
//! The rules under test:
//!
//! * "Modifies an existing cache subnet group" — the modify changes the description and
//!   REPLACES the subnets of the one stored group, so the group a later read reports is
//!   the one this call wrote; a member the request omits is left as it was;
//! * the subnets a group may hold are the ones the ACCOUNT has, as Amazon EC2 — the
//!   service that owns subnets — reports them, which is the resolution
//!   `CreateCacheSubnetGroup` applies too, so a group cannot be modified onto a subnet it
//!   could not have been created on; an identifier that names nothing is the refusal, in
//!   that resolution's own sentence;
//! * a group this account does not hold is the modelled `CacheSubnetGroupNotFoundFault`,
//!   worded with the name's STORED identity, and a name no group could carry is refused
//!   as the bad parameter it is before it is resolved against anything;
//! * a group holds the subnets of ONE VPC, so a request whose subnets span two is the
//!   modelled `InvalidSubnet`, and the VPC the group reports is the one EC2
//!   reported for the subnets it now holds.
//!
//! The tests are tables: the rows span which members the request names (description only,
//! subnets only, both), how many subnets it names, which VPC they sit in, the case the
//! name is spelled in, and each way a request can be wrong, so no single recorded request
//! is what makes them pass.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness the sibling suites share; nothing reaches into the
//! service's state struct.

use crate::cache_cluster_lifecycle::{account_subnet_ids, subnet_group_params, subnet_pairs};
use crate::harness::{params, Emulator, Reply, REGION};

/// The group every row of the tables below modifies.
const GROUP: &str = "modify-subnets";

/// Create the group `name` holding `subnets`, and insist the create succeeded.
async fn create_group(emulator: &Emulator, case: &str, name: &str, subnets: &[String]) {
    let created = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params(name, "the group before the modify", subnets),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must succeed, got {} {}",
        created.status, created.body
    );
}

/// `ModifyCacheSubnetGroup` with a description and/or a subnet list.
async fn modify_group(
    emulator: &Emulator,
    name: &str,
    description: Option<&str>,
    subnets: Option<&[String]>,
) -> Reply {
    let mut pairs = params(&[("CacheSubnetGroupName", name)]);
    if let Some(description) = description {
        pairs.push((
            "CacheSubnetGroupDescription".to_string(),
            description.to_string(),
        ));
    }
    if let Some(subnets) = subnets {
        pairs.extend(subnet_pairs("SubnetIdentifier", subnets));
    }
    emulator.call("ModifyCacheSubnetGroup", &pairs).await
}

/// Describe the group `name` names.
async fn describe_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeCacheSubnetGroups",
            &params(&[("CacheSubnetGroupName", name)]),
        )
        .await
}

/// A VPC of the client's own with one subnet per zone of `zones`, minted through EC2's own
/// API — the owner of subnets, asked rather than assumed.
async fn client_network(emulator: &Emulator, cidr_prefix: &str, zones: &[&str]) -> Vec<String> {
    let ec2 = emulator.ec2(REGION);
    let vpc = ec2
        .create_vpc()
        .cidr_block(format!("{cidr_prefix}.0.0/16"))
        .send()
        .await
        .expect("EC2 must create the client's VPC")
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("CreateVpc must report the VPC it minted")
        .to_string();
    let mut subnets = Vec::with_capacity(zones.len());
    for (index, zone) in zones.iter().enumerate() {
        subnets.push(
            ec2.create_subnet()
                .vpc_id(&vpc)
                .cidr_block(format!("{cidr_prefix}.{}.0/24", index + 1))
                .availability_zone(*zone)
                .send()
                .await
                .unwrap_or_else(|error| panic!("EC2 must create a subnet in {zone}: {error:?}"))
                .subnet()
                .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
                .unwrap_or_else(|| panic!("CreateSubnet must report the subnet it minted"))
                .to_string(),
        );
    }
    subnets
}

/// The VPC and zone EC2 reports for `subnet_id` — what a group holding it must report.
async fn reported_placement(emulator: &Emulator, subnet_id: &str) -> (String, String) {
    let described = emulator
        .ec2(REGION)
        .describe_subnets()
        .subnet_ids(subnet_id)
        .send()
        .await
        .unwrap_or_else(|error| panic!("EC2 must report the subnet {subnet_id}: {error:?}"));
    let subnet = described
        .subnets()
        .iter()
        .find(|subnet| subnet.subnet_id() == Some(subnet_id))
        .unwrap_or_else(|| panic!("EC2 must hold the subnet {subnet_id}"));
    (
        subnet
            .vpc_id()
            .unwrap_or_else(|| panic!("EC2 must report a VPC for {subnet_id}"))
            .to_string(),
        subnet
            .availability_zone()
            .unwrap_or_else(|| panic!("EC2 must report a zone for {subnet_id}"))
            .to_string(),
    )
}

/// One row of the write table: the case, the description the request names (if any), the
/// subnets it names (if any), and the subnets the group must hold afterwards.
type WriteCase = (
    &'static str,
    Option<&'static str>,
    Option<Vec<String>>,
    Vec<String>,
);

/// One row of the refusal table: the case, the name the request spells, the subnets it
/// names (if any), and the status plus wire code it must be refused with.
type RefusalRow = (&'static str, String, Option<Vec<String>>, u16, &'static str);

/// What a modify writes is what a later read reports, member by member: the rows span
/// which members the request names and how many subnets it names.
#[tokio::test]
async fn a_modify_writes_the_group_a_later_read_reports() {
    let emulator = Emulator::new();
    let defaults = account_subnet_ids(&emulator).await;
    let client = client_network(&emulator, "10.21", &["us-east-1a", "us-east-1b"]).await;

    // Each row: the case, the description the request names (if any), the subnets it names
    // (if any), and the subnets the group must hold afterwards.
    let rows: Vec<WriteCase> = vec![
        (
            "the description alone, the subnets left as they were",
            Some("a renamed group"),
            None,
            defaults[..2].to_vec(),
        ),
        (
            "one subnet, replacing the two the group held",
            None,
            Some(defaults[..1].to_vec()),
            defaults[..1].to_vec(),
        ),
        (
            "every default subnet of the account",
            None,
            Some(defaults.clone()),
            defaults.clone(),
        ),
        (
            "the subnets of the client's own VPC",
            Some("moved into the client's VPC"),
            Some(client.clone()),
            client.clone(),
        ),
        (
            "both members at once, back to one default subnet",
            Some("back again"),
            Some(defaults[1..2].to_vec()),
            defaults[1..2].to_vec(),
        ),
    ];

    create_group(&emulator, "the group under test", GROUP, &defaults[..2]).await;
    for (case, description, subnets, expected) in rows {
        let before = describe_group(&emulator, GROUP).await;
        let held_before: Vec<String> = before
            .all("Subnets/Subnet/SubnetIdentifier")
            .into_iter()
            .map(str::to_string)
            .collect();
        let description_before = before
            .text("CacheSubnetGroupDescription")
            .unwrap_or_default()
            .to_string();

        let modified = modify_group(&emulator, GROUP, description, subnets.as_deref()).await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: the modify must answer 200, got {} {}",
            modified.status, modified.body
        );

        // The answer and a later read are the same record.
        let described = describe_group(&emulator, GROUP).await;
        for (where_from, reply) in [("the modify's answer", &modified), ("a later read", &described)]
        {
            let reported: Vec<&str> = reply.all("Subnets/Subnet/SubnetIdentifier");
            let mut sorted = reported.clone();
            sorted.sort_unstable();
            let mut wanted: Vec<&str> = expected.iter().map(String::as_str).collect();
            wanted.sort_unstable();
            assert_eq!(
                sorted, wanted,
                "case {case:?} ({where_from}): expected the group to hold {wanted:?}, got \
                 {reported:?} in {}",
                reply.body
            );
            let expected_description = description.unwrap_or(description_before.as_str());
            assert_eq!(
                reply.text("CacheSubnetGroupDescription"),
                Some(expected_description),
                "case {case:?} ({where_from}): expected the description \
                 {expected_description:?}, got {:?}",
                reply.text("CacheSubnetGroupDescription")
            );
            assert_eq!(
                reply.text("CacheSubnetGroupName"),
                Some(GROUP),
                "case {case:?} ({where_from}): expected the stored name, got {:?}",
                reply.text("CacheSubnetGroupName")
            );
        }

        // Every subnet the group reports carries the VPC and the zone EC2 reports for it,
        // which is what makes the placement the account's own network.
        let (vpc, _) = reported_placement(&emulator, &expected[0]).await;
        assert_eq!(
            described.text("VpcId"),
            Some(vpc.as_str()),
            "case {case:?}: expected the VPC EC2 reports for the group's subnets ({vpc}), got \
             {:?}",
            described.text("VpcId")
        );
        for subnet_id in &expected {
            let (_, zone) = reported_placement(&emulator, subnet_id).await;
            let reported_zones = described.all("SubnetAvailabilityZone/Name");
            assert!(
                reported_zones.contains(&zone.as_str()),
                "case {case:?}: expected the zone EC2 reports for {subnet_id} ({zone}) among \
                 {reported_zones:?}"
            );
        }
        // A row that named no subnets changed none of them.
        if subnets.is_none() {
            let still: Vec<String> = described
                .all("Subnets/Subnet/SubnetIdentifier")
                .into_iter()
                .map(str::to_string)
                .collect();
            assert_eq!(
                still, held_before,
                "case {case:?}: a modify that named no subnets must leave the ones the group \
                 held, expected {held_before:?}, got {still:?}"
            );
        }
    }
}

/// The name is resolved as the API documents it (`CacheSubnetGroupName` "is stored as a
/// lowercase string"), so the case a modify spells it in does not matter — and the modify
/// touches only the group it names.
#[tokio::test]
async fn a_modify_resolves_the_name_case_insensitively_and_touches_only_that_group() {
    let emulator = Emulator::new();
    let defaults = account_subnet_ids(&emulator).await;

    for name in ["first-subnets", "second-subnets"] {
        create_group(&emulator, name, name, &defaults[..2]).await;
    }

    for (case, spelled) in [
        ("the stored spelling", "first-subnets"),
        ("upper case", "FIRST-SUBNETS"),
        ("mixed case", "First-Subnets"),
    ] {
        let modified = modify_group(
            &emulator,
            spelled,
            Some(&format!("modified as {spelled}")),
            Some(&defaults[..1]),
        )
        .await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: the modify must answer 200, got {} {}",
            modified.status, modified.body
        );
        assert_eq!(
            modified.text("CacheSubnetGroupName"),
            Some("first-subnets"),
            "case {case:?}: expected the stored, lowercased name, got {:?}",
            modified.text("CacheSubnetGroupName")
        );
    }

    let untouched = describe_group(&emulator, "second-subnets").await;
    assert_eq!(
        untouched.all("Subnets/Subnet/SubnetIdentifier").len(),
        2,
        "the group the modifies did not name must still hold its own subnets, got {}",
        untouched.body
    );
    assert_eq!(
        untouched.text("CacheSubnetGroupDescription"),
        Some("the group before the modify"),
        "the group the modifies did not name must keep its own description, got {:?}",
        untouched.text("CacheSubnetGroupDescription")
    );
}

/// Every way a modify can be wrong, and the refusal it gets: the rows span a group this
/// account does not hold, a name no group could carry, an identifier that names no subnet,
/// a value that is not a subnet id at all, one named twice, more subnets than a group
/// holds, and subnets spanning two VPCs.
#[tokio::test]
async fn a_modify_is_refused_with_the_rule_it_broke() {
    let emulator = Emulator::new();
    let defaults = account_subnet_ids(&emulator).await;
    let other_vpc = client_network(&emulator, "10.22", &["us-east-1a"]).await;
    create_group(&emulator, "the group under test", GROUP, &defaults[..2]).await;

    // Source of the 255 bound: `CacheSubnetGroupName`'s own constraint — "Must contain no
    // more than 255 alphanumeric characters or hyphens".
    let too_long = "g".repeat(256);
    // More subnets than one group holds (`state::SUBNETS_PER_GROUP` is 20), each a
    // well-formed id, so the request is refused on the count and not on a form.
    let too_many: Vec<String> = (0..21).map(|index| format!("subnet-{index:017}")).collect();
    let spanning: Vec<String> = vec![defaults[0].clone(), other_vpc[0].clone()];

    // Each row: the case, the name, the subnets, the status and the wire code.
    let rows: Vec<RefusalRow> = vec![
        (
            "a group this account does not hold",
            "never-created-subnets".to_string(),
            None,
            400,
            "CacheSubnetGroupNotFoundFault",
        ),
        (
            // The group's absence is settled before the subnets are resolved, so this is
            // the not-found and not a subnet refusal.
            "a group this account does not hold, named beside valid subnets",
            "never-created-subnets".to_string(),
            Some(vec![defaults[0].clone()]),
            400,
            "CacheSubnetGroupNotFoundFault",
        ),
        (
            "a name one character past the published bound",
            too_long.clone(),
            None,
            400,
            "InvalidParameterValue",
        ),
        (
            "a name with an underscore",
            "under_score".to_string(),
            None,
            400,
            "InvalidParameterValue",
        ),
        (
            "an identifier the account has no subnet for",
            GROUP.to_string(),
            Some(vec!["subnet-00000000deadbeef".to_string()]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a held subnet beside one the account does not have",
            GROUP.to_string(),
            Some(vec![defaults[0].clone(), "subnet-00000000deadbeef".to_string()]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a value that is not a subnet id at all",
            GROUP.to_string(),
            Some(vec!["vpc-00000000000000001".to_string()]),
            400,
            "InvalidSubnet",
        ),
        (
            "one subnet named twice",
            GROUP.to_string(),
            Some(vec![defaults[0].clone(), defaults[0].clone()]),
            400,
            "InvalidSubnet",
        ),
        (
            "more subnets than a cache subnet group holds",
            GROUP.to_string(),
            Some(too_many.clone()),
            400,
            "CacheSubnetQuotaExceededFault",
        ),
        (
            "subnets spanning two VPCs",
            GROUP.to_string(),
            Some(spanning.clone()),
            400,
            "InvalidSubnet",
        ),
    ];

    for (case, name, subnets, status, code) in rows {
        let refused = modify_group(&emulator, &name, None, subnets.as_deref()).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the wire code {code:?}, got {}",
            refused.body
        );
        // Nothing was written: the group still holds what it held.
        let described = describe_group(&emulator, GROUP).await;
        let held: Vec<&str> = described.all("Subnets/Subnet/SubnetIdentifier");
        let mut sorted = held.clone();
        sorted.sort_unstable();
        let mut wanted: Vec<&str> = defaults[..2].iter().map(String::as_str).collect();
        wanted.sort_unstable();
        assert_eq!(
            sorted, wanted,
            "case {case:?}: a refused modify must change nothing, expected {wanted:?}, got \
             {held:?}"
        );
    }
}

/// The two refusals a modify shares with its sibling operations: the not-found names the
/// name's STORED identity (`CacheSubnetGroupName` "is stored as a lowercase string", so the
/// service names the group under that one identity whatever case the request spelled), and
/// the unresolved-subnet sentence names the identifiers the request listed, which are EC2's
/// own and carry no case rule.
#[tokio::test]
async fn a_refused_modify_is_worded_from_the_stored_identity() {
    let emulator = Emulator::new();
    let defaults = account_subnet_ids(&emulator).await;
    create_group(&emulator, "the group under test", GROUP, &defaults[..1]).await;

    for (case, spelled) in [
        ("a name nothing created", "Never-Created-Subnets"),
        ("a prefix of a group that exists", "modify"),
    ] {
        let refused = modify_group(&emulator, spelled, Some("nothing to change"), None).await;
        assert_eq!(
            refused.error_code(),
            "CacheSubnetGroupNotFoundFault",
            "case {case:?}: expected the modelled not-found, got {}",
            refused.body
        );
        let stored = spelled.to_ascii_lowercase();
        assert_eq!(
            refused.error_message(),
            format!("Cache Subnet Group {stored} does not exist."),
            "case {case:?}: expected the not-found sentence for {stored:?}, got {:?}",
            refused.error_message()
        );
    }

    // The unresolved-subnet refusal names exactly the identifiers that resolved to
    // nothing, in the order the request listed them, and nothing of the ones that did.
    let refused = modify_group(
        &emulator,
        GROUP,
        None,
        Some(&[
            "subnet-00000000deadbeef".to_string(),
            defaults[0].clone(),
            "subnet-00000000deadbeee".to_string(),
        ]),
    )
    .await;
    assert_eq!(
        refused.error_message(),
        "Some input subnets in :[subnet-00000000deadbeef, subnet-00000000deadbeee] are invalid.",
        "expected the sentence naming the unresolved ids in request order, got {:?}",
        refused.error_message()
    );
}

/// The modify is recorded as an event of the group, beside its create — the log a
/// `DescribeEvents` reader sees, which is the other place a state change shows up.
#[tokio::test]
async fn a_subnet_group_modify_is_recorded_as_an_event() {
    let emulator = Emulator::new();
    let defaults = account_subnet_ids(&emulator).await;
    create_group(&emulator, "the group under test", "Logged-Modify", &defaults[..1]).await;

    let modified = modify_group(
        &emulator,
        "logged-modify",
        Some("a logged modify"),
        Some(&defaults[..2]),
    )
    .await;
    assert_eq!(
        modified.status, 200,
        "the modify must answer 200, got {} {}",
        modified.status, modified.body
    );

    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[
                ("SourceType", "cache-subnet-group"),
                ("SourceIdentifier", "logged-modify"),
            ]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "the event read must answer 200, got {} {}",
        events.status, events.body
    );
    let messages = events.all("Event/Message");
    assert_eq!(
        messages.len(),
        2,
        "expected the create and the modify to be logged, got {messages:?} from {}",
        events.body
    );
    assert!(
        messages
            .iter()
            .any(|message| message.to_ascii_lowercase().contains("modified")),
        "expected an event recording the modify, got {messages:?} from {}",
        events.body
    );
}
