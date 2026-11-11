// SPDX-License-Identifier: CC0-1.0

//! The modelled members of this service's REFERENCE read paths: the facts a Region, an
//! instance type, a published prefix list and an endpoint service of a Region carry, and the
//! configuration a created application status check carries.
//!
//! Every call goes over the wire the service speaks, through the shared `super::ec2_query`
//! harness: an `application/x-www-form-urlencoded` ec2Query POST dispatched through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! The rules under test — one per read path, each asserted over a SET of inputs rather than
//! over one recorded value:
//!
//! 1. A reported Region carries the geography AWS publishes for it, as the one-item list the
//!    `Region$Geography` member is.
//! 2. Every member `InstanceTypeInfo` publishes that follows from the instance type's name is
//!    REPORTED for every type: the network cards it has and the bandwidth its share of the
//!    host carries, the Nitro features its generation introduced, its processor's sustained
//!    clock speed and the EBS volumes its attachment pool leaves it.
//! 3. A created application status check reports every member the request left to the
//!    service, at the value its own documentation gives as the default — and the check a
//!    later describe reports is the one the create answered.
//! 4. A prefix list Amazon Web Services publishes reports what is a fact OF the list and
//!    nothing that is a fact of a list an account created: no `MaxEntries` and no `Version`.
//!    `DescribeManagedPrefixLists` reports every published list; `DescribePrefixLists`
//!    reports the gateway-endpoint services among them.
//! 5. An endpoint service of a Region is selected by its own name, whichever published name
//!    family it belongs to, and reports the address types it answers on.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{self, element, elements, filters, indexed};
use super::wire::{child, child_text, document, param, set_items};

/// The Region most calls here are signed for: the one the catalogs under test are read for.
const REGION: &str = "us-west-2";

// ---------------------------------------------------------------------------
// Rule 1: a Region reports its published geography
// ---------------------------------------------------------------------------

/// Every Region a `DescribeRegions` answer carries reports the geography AWS publishes for
/// it, as a one-item `geographySet`.
///
/// The rows differ along the geography itself and along the partition the call is signed for:
/// a Region of the United States, of Europe named after a country rather than a city, of
/// Asia Pacific, of South America, an opt-in Region (reported only when named), a Region of
/// the China partition and one of GovCloud. The expected names are the country or area column
/// of the AWS global infrastructure "Regions and Availability Zones" table, read here through
/// the catalog the handler projects (`ec2::state::region_geography`) so one source answers
/// both; `il-central-1` is a Region the issue this suite was written for did not name.
#[tokio::test]
async fn every_region_reports_its_published_geography() {
    let (router, _engine) = ec2_query::emulator();
    let cases: [(&str, &str, &str, &str); 7] = [
        ("the United States", REGION, REGION, "United States of America"),
        (
            "a Region named after its country",
            "eu-west-1",
            "eu-west-1",
            "Ireland",
        ),
        ("Asia Pacific", "ap-northeast-1", "ap-northeast-1", "Japan"),
        ("South America", "sa-east-1", "sa-east-1", "Brazil"),
        // An opt-in Region answers nothing through its own endpoint, so it is asked about
        // from an enabled Region of the same partition — which `RegionNames` allows: "you
        // can specify any Regions, whether they are enabled and disabled for your account".
        ("an opt-in Region", "us-east-1", "il-central-1", "Israel"),
        ("the China partition", "cn-north-1", "cn-north-1", "China"),
        (
            "GovCloud",
            "us-gov-west-1",
            "us-gov-west-1",
            "United States of America",
        ),
    ];

    for (case, signed, region, geography) in cases {
        let params = indexed("RegionName", &[region]);
        let (status, body) = ec2_query::call(&router, signed, "DescribeRegions", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for {region}, got {status} {body}"
        );
        let parsed = document(case, &body);
        let item = parsed
            .descendants()
            .find(|node| node.is_element() && child_text(*node, "regionName").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: no Region in {body}"));
        let reported: Vec<String> = set_items(item, "geographySet")
            .into_iter()
            .map(|entry| child_text(entry, "name").unwrap_or_default())
            .collect();
        assert_eq!(
            reported,
            vec![geography.to_string()],
            "case {case:?}: expected the geography {geography:?} for {region}, got {body}"
        );
        assert_eq!(
            ec2::state::region_geography(region),
            Some(geography),
            "case {case:?}: the catalog must publish {geography:?} for {region}, got {:?}",
            ec2::state::region_geography(region)
        );
    }

    // The INVARIANT behind the rows: no Region of an answer is reported without its
    // geography, so a client reading the member never finds it missing for some Regions and
    // present for others.
    let (status, body) = ec2_query::call(&router, REGION, "DescribeRegions", &[]).await;
    assert_eq!(status, 200, "the whole answer: got {status} {body}");
    let parsed = document("the whole answer", &body);
    let regions: Vec<roxmltree::Node<'_, '_>> = parsed
        .descendants()
        .filter(|node| node.is_element() && child_text(*node, "regionName").is_some())
        .collect();
    assert!(
        regions.len() > 1,
        "the whole answer: expected the partition's Regions, got {body}"
    );
    for region in regions {
        let name = child_text(region, "regionName").unwrap_or_default();
        let geography = set_items(region, "geographySet")
            .into_iter()
            .filter_map(|entry| child_text(entry, "name"))
            .next();
        assert_eq!(
            geography.as_deref(),
            ec2::state::region_geography(&name),
            "the whole answer: {name} must report its published geography, got {geography:?}"
        );
        assert!(
            geography.is_some(),
            "the whole answer: {name} reports no geography at all, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an instance type reports every member its name yields
// ---------------------------------------------------------------------------

/// One `DescribeInstanceTypes` call naming one type, as that type's `<item>` body.
async fn instance_type(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = ec2_query::call(
        router,
        REGION,
        "DescribeInstanceTypes",
        &indexed("InstanceType", &[name]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 for {name}, got {status} {body}"
    );
    let set = element(&body, "instanceTypeSet")
        .unwrap_or_else(|| panic!("case {case:?}: no instanceTypeSet in {body}"));
    set
}

/// The text of the member `name` of the structure `structure` of one reported type.
fn nested(case: &str, item: &str, structure: &str, name: &str) -> String {
    let inner = element(item, structure)
        .unwrap_or_else(|| panic!("case {case:?}: no <{structure}> in {item}"));
    element(&inner, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in <{structure}> of {item}"))
}

/// What one row of the instance-type table expects of the members whose value follows from
/// the type's generation, its hypervisor and its hardware.
struct TypeCase {
    case: &'static str,
    instance_type: &'static str,
    /// `NetworkInfo$MaximumNetworkCards`.
    cards: i64,
    /// `InstanceTypeInfo$PhcSupport` and `NetworkInfo$FlexibleEnaQueuesSupport`, which the
    /// same Nitro hardware generation introduces.
    nitro_clock: &'static str,
    /// `InstanceTypeInfo$RebootMigrationSupport` and `InstanceTypeInfo$NitroTpmSupport`,
    /// which the Nitro hypervisor provides.
    hypervisor_feature: &'static str,
    /// `NetworkInfo$EnaSrdSupported`.
    ena_srd: bool,
    /// `ProcessorInfo$SustainedClockSpeedInGhz`.
    clock_ghz: &'static str,
    /// `EbsInfo$MaximumEbsAttachments`.
    ebs_attachments: i64,
}

/// Every member of `InstanceTypeInfo` whose value follows from the instance type's name is
/// reported, with the value that name's generation, hypervisor and hardware give it.
///
/// The rows differ along what the rules generalise over: the Nitro generation (a fifth, a
/// sixth and a seventh), the processor's designer (AWS Graviton, Intel, AMD), a bare metal
/// size (which has no hypervisor, so the features the hypervisor provides are not offered on
/// it), a pre-Nitro type, a type whose instance store takes attachments out of the pool its
/// EBS volumes share, and the two published multi-card types.
#[tokio::test]
async fn an_instance_type_reports_the_members_its_name_yields() {
    let (router, _engine) = ec2_query::emulator();
    let cases: [TypeCase; 8] = [
        TypeCase {
            case: "a sixth-generation Graviton type with an instance store",
            instance_type: "m6gd.4xlarge",
            cards: 1,
            nitro_clock: "unsupported",
            hypervisor_feature: "supported",
            ena_srd: true,
            clock_ghz: "2.5",
            // 28 attachments shared, less the primary network interface and the one
            // instance store device.
            ebs_attachments: 26,
        },
        TypeCase {
            case: "its bare metal size, which has no hypervisor",
            instance_type: "m6gd.metal",
            cards: 1,
            nitro_clock: "unsupported",
            hypervisor_feature: "unsupported",
            ena_srd: false,
            clock_ghz: "2.5",
            ebs_attachments: 25,
        },
        TypeCase {
            case: "a seventh-generation Graviton type",
            instance_type: "m7g.xlarge",
            cards: 1,
            nitro_clock: "supported",
            hypervisor_feature: "supported",
            ena_srd: true,
            clock_ghz: "2.6",
            ebs_attachments: 27,
        },
        TypeCase {
            case: "a fifth-generation Intel type",
            instance_type: "m5.large",
            cards: 1,
            nitro_clock: "unsupported",
            hypervisor_feature: "supported",
            ena_srd: false,
            clock_ghz: "3.1",
            ebs_attachments: 27,
        },
        TypeCase {
            case: "a sixth-generation AMD type",
            instance_type: "m6a.2xlarge",
            cards: 1,
            nitro_clock: "unsupported",
            hypervisor_feature: "supported",
            ena_srd: true,
            clock_ghz: "3.6",
            ebs_attachments: 27,
        },
        TypeCase {
            case: "a type that runs on the Xen hypervisor",
            instance_type: "t2.micro",
            cards: 1,
            nitro_clock: "unsupported",
            hypervisor_feature: "unsupported",
            ena_srd: false,
            clock_ghz: "2.4",
            ebs_attachments: 27,
        },
        TypeCase {
            case: "a published four-card type",
            instance_type: "p4d.24xlarge",
            cards: 4,
            nitro_clock: "unsupported",
            hypervisor_feature: "supported",
            // A family that reached the Nitro System before its sixth generation still
            // carries the adapter of its own generation, so ENA Express is not offered.
            ena_srd: false,
            clock_ghz: "2.4",
            // Its instance store is seven devices, each of which takes an attachment out of
            // the pool its EBS volumes share.
            ebs_attachments: 20,
        },
        TypeCase {
            case: "a published two-card type",
            instance_type: "c6in.32xlarge",
            cards: 2,
            nitro_clock: "unsupported",
            hypervisor_feature: "supported",
            ena_srd: true,
            clock_ghz: "3.5",
            ebs_attachments: 27,
        },
    ];

    for row in cases {
        let case = row.case;
        let item = instance_type(&router, case, row.instance_type).await;
        let network = element(&item, "networkInfo")
            .unwrap_or_else(|| panic!("case {case:?}: no <networkInfo> in {item}"));

        for (name, expected) in [
            ("maximumNetworkCards", row.cards.to_string()),
            ("defaultNetworkCardIndex", "0".to_string()),
            ("enaSrdSupported", row.ena_srd.to_string()),
            ("flexibleEnaQueuesSupport", row.nitro_clock.to_string()),
            (
                "secondaryNetworkSupported",
                (row.nitro_clock == "supported").to_string(),
            ),
        ] {
            let actual = element(&network, name)
                .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {network}"));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {name} {expected:?} for {}, got {actual:?}",
                row.instance_type
            );
        }

        for (name, expected) in [
            ("phcSupport", row.nitro_clock),
            ("rebootMigrationSupport", row.hypervisor_feature),
            ("nitroTpmSupport", row.hypervisor_feature),
        ] {
            let actual = element(&item, name)
                .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {item}"));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {name} {expected:?} for {}, got {actual:?}",
                row.instance_type
            );
        }

        let clock = nested(case, &item, "processorInfo", "sustainedClockSpeedInGhz");
        assert_eq!(
            clock, row.clock_ghz,
            "case {case:?}: expected a sustained clock speed of {} GHz for {}, got {clock:?}",
            row.clock_ghz, row.instance_type
        );

        let attachments = nested(case, &item, "ebsInfo", "maximumEbsAttachments");
        assert_eq!(
            attachments,
            row.ebs_attachments.to_string(),
            "case {case:?}: expected {} EBS attachments for {}, got {attachments:?}",
            row.ebs_attachments,
            row.instance_type
        );

        // The type's own network performance and its cards' are the same derivation, so the
        // card inventory holds exactly the cards the count claims, each with a figure in the
        // published form and its own index.
        let wrapped = item.clone();
        let parsed = document(case, &wrapped);
        let root = parsed.root_element();
        let network_node = child(root, "networkInfo")
            .unwrap_or_else(|| panic!("case {case:?}: no networkInfo node in {item}"));
        let reported_cards = set_items(network_node, "networkCards");
        assert_eq!(
            reported_cards.len() as i64,
            row.cards,
            "case {case:?}: expected {} network cards for {}, got {item}",
            row.cards,
            row.instance_type
        );
        for (index, card) in reported_cards.iter().enumerate() {
            assert_eq!(
                child_text(*card, "networkCardIndex"),
                Some(index.to_string()),
                "case {case:?}: card {index} must report its own index, got {item}"
            );
            let performance = child_text(*card, "networkPerformance").unwrap_or_default();
            assert!(
                performance.ends_with(" Gigabit"),
                "case {case:?}: card {index} must report a Gigabit figure, got \
                 {performance:?}"
            );
        }
        let performance = element(&network, "networkPerformance").unwrap_or_default();
        assert!(
            performance.ends_with(" Gigabit"),
            "case {case:?}: {} must report a Gigabit figure, got {performance:?}",
            row.instance_type
        );
    }
}

/// The network performance a size reports is its SHARE of its host: a size below the burst
/// ceiling reports that ceiling as an "Up to" figure, the family's largest virtualized size
/// reports the whole host's figure, and the figures never fall as the ladder rises.
///
/// The rows are the ladder of one family, from the smallest size to the largest and its bare
/// metal size — the dimension the rule generalises over.
#[tokio::test]
async fn network_performance_grows_with_the_size_of_the_instance() {
    let (router, _engine) = ec2_query::emulator();
    let ladder = [
        "m6gd.medium",
        "m6gd.large",
        "m6gd.xlarge",
        "m6gd.2xlarge",
        "m6gd.4xlarge",
        "m6gd.8xlarge",
        "m6gd.12xlarge",
        "m6gd.16xlarge",
    ];

    let mut previous = 0.0f64;
    for name in ladder {
        let item = instance_type(&router, name, name).await;
        let performance = nested(name, &item, "networkInfo", "networkPerformance");
        let figure = performance
            .trim_start_matches("Up to ")
            .trim_end_matches(" Gigabit")
            .parse::<f64>()
            .unwrap_or_else(|_| panic!("case {name:?}: {performance:?} is no Gigabit figure"));
        assert!(
            figure >= previous,
            "case {name:?}: a larger size cannot report less bandwidth than a smaller one, \
             got {figure} after {previous}"
        );
        // A size below the host's full share bursts to the published ceiling, which the
        // table writes as "Up to"; a size that has its share of the host reports that share
        // flat, and the largest virtualized size has the whole host.
        let ceiling = 10.0f64;
        let bursts = performance.starts_with("Up to ");
        if bursts {
            assert_eq!(
                figure, ceiling,
                "case {name:?}: a bursting size reports the ceiling, got {performance:?}"
            );
        } else {
            assert!(
                figure > ceiling,
                "case {name:?}: a size reporting a flat figure has more than the burst \
                 ceiling, got {performance:?}"
            );
        }
        if name == "m6gd.16xlarge" {
            assert_eq!(
                figure, 25.0,
                "case {name:?}: the largest virtualized size has the whole host's \
                 bandwidth, got {performance:?}"
            );
        }
        previous = figure;
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a created application status check reports its documented defaults
// ---------------------------------------------------------------------------

/// The three members whose default the create fills in: `(deviceIndex, ipVersion, ipScope)`.
type CheckAddressing = (Option<String>, Option<String>, Option<String>);

/// Those three members of one reported `<applicationStatusCheck>` body.
fn addressing(case: &str, body: &str, element_name: &str) -> (String, CheckAddressing) {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == element_name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{element_name}> in {body}"));
    (
        child_text(node, "applicationStatusCheckId").unwrap_or_default(),
        (
            child_text(node, "deviceIndex"),
            child_text(node, "ipVersion"),
            child_text(node, "ipScope"),
        ),
    )
}

/// One row of the addressing table: the request's extra fields, and the three members the
/// created check must then report.
struct CheckCase {
    case: &'static str,
    /// The addressing fields the request names, if any.
    extra: Vec<(String, String)>,
    device_index: &'static str,
    ip_version: &'static str,
    ip_scope: &'static str,
}

/// A created check reports the network device, IP version and IP scope its health checks use
/// — the values the request named, or the documented default for each it left out — and the
/// check a later describe reports carries the same three.
///
/// The rows differ along which of the three the request names: none of them, all three, and
/// one of them, with an IPv6 check and a device index above the first among the values.
#[tokio::test]
async fn a_created_check_reports_its_addressing_and_reads_back() {
    let (router, _engine) = ec2_query::emulator();
    // The documented defaults: device index 0 is the device every instance has, `ipv4` the
    // version its primary interface answers on, and `private` the one value `IpScope` admits.
    let cases: [CheckCase; 4] = [
        CheckCase {
            case: "no addressing named at all",
            extra: Vec::new(),
            device_index: "0",
            ip_version: "ipv4",
            ip_scope: "private",
        },
        CheckCase {
            case: "every member named",
            extra: vec![
                param("DeviceIndex", "2"),
                param("IpVersion", "ipv6"),
                param("IpScope", "private"),
            ],
            device_index: "2",
            ip_version: "ipv6",
            ip_scope: "private",
        },
        CheckCase {
            case: "only the IP version named",
            extra: vec![param("IpVersion", "ipv6")],
            device_index: "0",
            ip_version: "ipv6",
            ip_scope: "private",
        },
        CheckCase {
            case: "only the device index named",
            extra: vec![param("DeviceIndex", "7")],
            device_index: "7",
            ip_version: "ipv4",
            ip_scope: "private",
        },
    ];

    for row in cases {
        let case = row.case;
        let mut params = vec![param("Protocol", "http"), param("Port", "8080")];
        params.extend(row.extra);
        let (status, body) = ec2_query::call(
            &router,
            REGION,
            "CreateApplicationStatusCheck",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let expected = (
            Some(row.device_index.to_string()),
            Some(row.ip_version.to_string()),
            Some(row.ip_scope.to_string()),
        );
        let (id, created) = addressing(case, &body, "applicationStatusCheck");
        assert_eq!(
            created, expected,
            "case {case:?}: expected (deviceIndex, ipVersion, ipScope) {expected:?}, got {body}"
        );

        // The created check is the one the describe reports: the members are read back off
        // the store rather than off the create's own response object.
        let (status, body) = ec2_query::call(
            &router,
            REGION,
            "DescribeApplicationStatusChecks",
            &indexed("ApplicationStatusCheckId", &[id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the describe, got {status} {body}"
        );
        let (described_id, described) = addressing(case, &body, "item");
        assert_eq!(
            described_id, id,
            "case {case:?}: the describe must report the check {id}, got {body}"
        );
        assert_eq!(
            described, expected,
            "case {case:?}: the describe must report {expected:?}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a published prefix list reports only facts of itself
// ---------------------------------------------------------------------------

/// Every prefix list `DescribeManagedPrefixLists` reports is a list of the account or one
/// Amazon Web Services publishes, and a published one carries no `MaxEntries` and no
/// `Version`: both are facts of a list an account created.
///
/// The rows are the published lists of the Region — the whole catalog, so the assertions run
/// over its IPv4 and IPv6 lists and over the services with and without a gateway endpoint —
/// plus one list this account creates, which DOES carry both members.
#[tokio::test]
async fn a_published_prefix_list_reports_no_account_facts() {
    let (router, _engine) = ec2_query::emulator();

    // A list of the account's own, so the contrast is drawn inside ONE answer.
    let created = ec2_query::call(
        &router,
        REGION,
        "CreateManagedPrefixList",
        &[
            param("PrefixListName", "checkout-fleet"),
            param("AddressFamily", "IPv4"),
            param("MaxEntries", "5"),
        ],
    )
    .await;
    assert_eq!(created.0, 200, "the account's own list: got {created:?}");
    let own_id = element(&created.1, "prefixListId").unwrap_or_default();
    assert!(
        !own_id.is_empty(),
        "the account's own list: expected an id, got {}",
        created.1
    );

    let (status, body) =
        ec2_query::call(&router, REGION, "DescribeManagedPrefixLists", &[]).await;
    assert_eq!(status, 200, "the whole answer: got {status} {body}");
    let parsed = document("the whole answer", &body);
    let mut published = 0usize;
    let mut names: Vec<String> = Vec::new();
    for node in parsed
        .descendants()
        .filter(|node| node.is_element() && child_text(*node, "prefixListId").is_some())
    {
        let id = child_text(node, "prefixListId").unwrap_or_default();
        let owner = child_text(node, "ownerId").unwrap_or_default();
        if id == own_id {
            // The account's own list: both members are facts of it, so both are reported.
            assert!(
                child_text(node, "maxEntries").is_some()
                    && child_text(node, "version").is_some(),
                "the account's own list {id} must report its maximum and its version, \
                 got {body}"
            );
            continue;
        }
        published += 1;
        names.push(child_text(node, "prefixListName").unwrap_or_default());
        assert_eq!(
            owner, "AWS",
            "list {id}: a list that is not the account's is published by the service, got \
             {owner:?}"
        );
        assert_eq!(
            child_text(node, "state").as_deref(),
            Some("create-complete"),
            "list {id}: a published list's creation is settled, got {body}"
        );
        assert!(
            child_text(node, "prefixListArn")
                .is_some_and(|arn| arn.contains(REGION) && arn.ends_with(&id)),
            "list {id}: expected an ARN naming the Region and the list, got {body}"
        );
        assert_eq!(
            child_text(node, "maxEntries"),
            None,
            "list {id}: a published list carries no maximum, got {body}"
        );
        assert_eq!(
            child_text(node, "version"),
            None,
            "list {id}: a published list carries no version, got {body}"
        );
    }
    assert_eq!(
        published,
        ec2::state::managed_prefix_lists_in_region(REGION).len(),
        "expected every published list of {REGION}, got {names:?}"
    );

    // The catalog holds the lists AWS publishes for services that have no gateway endpoint,
    // and the IPv6 list of a service that publishes one — neither of which the legacy report
    // below carries.
    for name in [
        format!("com.amazonaws.{REGION}.route53-healthchecks"),
        format!("com.amazonaws.{REGION}.ipv6.s3"),
        format!("com.amazonaws.{REGION}.cloudfront.origin-facing"),
        format!("com.amazonaws.{REGION}.s3"),
    ] {
        assert!(
            names.contains(&name),
            "expected the published list {name}, got {names:?}"
        );
    }

    // `DescribePrefixLists` "describes available Amazon Web Services services in a prefix
    // list format": the Amazon S3 and Amazon DynamoDB service family among those lists,
    // each under the same id.
    let (status, body) = ec2_query::call(&router, REGION, "DescribePrefixLists", &[]).await;
    assert_eq!(status, 200, "the legacy report: got {status} {body}");
    let legacy = elements(&body, "prefixListName");
    assert!(
        !legacy.is_empty() && legacy.len() < names.len(),
        "the legacy report carries the S3 and DynamoDB service family only, got \
         {legacy:?} against {names:?}"
    );
    for name in &legacy {
        assert!(
            names.contains(name),
            "the legacy report's {name} must be one of the published lists, got {names:?}"
        );
        let label = name
            .strip_prefix(&format!("com.amazonaws.{REGION}."))
            .unwrap_or_default()
            .trim_start_matches("ipv6.");
        assert!(
            ["dynamodb", "s3", "s3express"].contains(&label),
            "the legacy report carries the S3 and DynamoDB service family only, got {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: an endpoint service of a Region is selected by its own name
// ---------------------------------------------------------------------------

/// One row of the endpoint-service table: the service's name, the `ServiceType` members it is
/// reachable through, and the address types it answers on.
struct ServiceCase {
    case: &'static str,
    service_name: String,
    service_types: Vec<&'static str>,
    ip_types: Vec<String>,
}

/// A `service-name` filter naming a service the Region offers reports that service — and only
/// that one — in both the names and the details, with the service types and address types it
/// is reachable through.
///
/// The rows differ along the published name family the service belongs to
/// (`com.amazonaws.<region>.<service>`, `aws.api.<region>.<service>`,
/// `api.aws.<region>.<service>`) and along the kind of endpoint it is reached through: an
/// interface service, the one service reachable both ways, and a gateway-only service.
#[tokio::test]
async fn an_endpoint_service_is_selected_by_its_name() {
    let (router, _engine) = ec2_query::emulator();
    let ipv4_and_ipv6 = vec!["ipv4".to_string(), "ipv6".to_string()];
    let cases: [ServiceCase; 5] = [
        ServiceCase {
            case: "an interface service",
            service_name: format!("com.amazonaws.{REGION}.access-analyzer"),
            service_types: vec!["Interface"],
            ip_types: ipv4_and_ipv6.clone(),
        },
        ServiceCase {
            case: "a service reachable both ways",
            service_name: format!("com.amazonaws.{REGION}.s3"),
            service_types: vec!["Gateway", "Interface"],
            ip_types: ipv4_and_ipv6.clone(),
        },
        ServiceCase {
            case: "a gateway-only service",
            service_name: format!("com.amazonaws.{REGION}.dynamodb"),
            service_types: vec!["Gateway"],
            ip_types: vec!["ipv4".to_string()],
        },
        ServiceCase {
            case: "a service of the aws.api family",
            service_name: format!("aws.api.{REGION}.emr-service-cell01"),
            service_types: vec!["Interface"],
            ip_types: ipv4_and_ipv6.clone(),
        },
        ServiceCase {
            case: "a service of the api.aws family",
            service_name: format!("api.aws.{REGION}.cassandra-streams"),
            service_types: vec!["Interface"],
            ip_types: ipv4_and_ipv6.clone(),
        },
    ];

    for row in cases {
        let ServiceCase {
            case,
            service_name,
            service_types,
            ip_types,
        } = row;
        let params = filters(&[("service-name", &[service_name.as_str()])]);
        let (status, body) = ec2_query::call(
            &router,
            REGION,
            "DescribeVpcEndpointServices",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for {service_name}, got {status} {body}"
        );
        let parsed = document(case, &body);
        let root = parsed.root_element();
        let reported_names: Vec<String> = set_items(root, "serviceNameSet")
            .into_iter()
            .map(|node| node.text().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported_names,
            vec![service_name.clone()],
            "case {case:?}: a name filter selects exactly that service, got {body}"
        );
        let details = set_items(root, "serviceDetailSet");
        assert_eq!(
            details.len(),
            1,
            "case {case:?}: expected one service detail for {service_name}, got {body}"
        );
        let detail = details[0];
        assert_eq!(
            child_text(detail, "serviceName"),
            Some(service_name.clone()),
            "case {case:?}: the detail must name the service, got {body}"
        );
        assert_eq!(
            child_text(detail, "serviceRegion").as_deref(),
            Some(REGION),
            "case {case:?}: the service is hosted in the Region its name carries, got {body}"
        );
        let reported_types: Vec<String> = set_items(detail, "serviceType")
            .into_iter()
            .map(|node| child_text(node, "serviceType").unwrap_or_default())
            .collect();
        assert_eq!(
            reported_types,
            service_types
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the service types {service_types:?}, got {body}"
        );
        let reported_ip_types: Vec<String> = set_items(detail, "supportedIpAddressTypeSet")
            .into_iter()
            .map(|node| node.text().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported_ip_types, ip_types,
            "case {case:?}: expected the address types {ip_types:?}, got {body}"
        );
    }
}
