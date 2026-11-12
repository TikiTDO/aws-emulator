// SPDX-License-Identifier: CC0-1.0

//! The per-instance-type hardware facts `DescribeInstanceTypes` reports, driven over the
//! wire the service speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST answered
//! with XML; README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: AN INSTANCE TYPE'S HARDWARE IS A PUBLISHED FACT OF THAT TYPE, NOT A
//! FUNCTION OF ITS NAME. The size ladder in a type's name gives its vCPUs, its cores and its
//! memory; it does NOT give the EBS bandwidth and IOPS AWS dedicates to the type
//! (`EbsInfo$EbsOptimizedInfo`), how many volumes it can hold attached and whether that limit
//! is its own (`EbsInfo$MaximumEbsAttachments` / `$AttachmentLimitType`), the local devices it
//! carries and their size (`InstanceStorageInfo$Disks` / `$TotalSizeInGB`), its network
//! performance and interface limits, its accelerators (`InstanceTypeInfo$FpgaInfo`,
//! `NetworkInfo$EfaInfo`), its processor and that processor's sustained frequency, or the host
//! features it is offered with (Dedicated Hosts, CloudWatch action based recovery,
//! hibernation, reboot migration, a PTP hardware clock). Each of those is the figure AWS
//! publishes for the one type, so two sizes of one family do not scale by the size multiplier
//! and two families of one size do not agree.
//!
//! Every table below spans what the rule generalises over — the family, the size, the
//! accelerator, the burstable and the bare metal size, a type with local devices and one
//! without — so no row is the specification and the types any one report happened to name are
//! rows among many. The last test pins the other half of the rule: a type the published table
//! holds no row for is still described, by the derivation over its name.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, indexed};

/// The Region every call here is signed for; the catalog is the same in each.
const REGION: &str = "us-east-1";

/// The megabits in a megabyte, which is all that separates the `Mbps` figures of
/// `EbsOptimizedInfo` from its `MB/s` ones.
const MEGABITS_PER_MEGABYTE: f64 = 8.0;

/// The body `DescribeInstanceTypes` answers for exactly one instance type.
async fn described(case: &str, router: &MultiServiceRouter, instance_type: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstanceTypes",
        &indexed("InstanceType", &[instance_type]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for {instance_type}, got {status} with body {body}"
    );
    body
}

/// One member of a reported type, or a panic naming the case when it is absent.
fn required(case: &str, xml: &str, name: &str) -> String {
    element(xml, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> reported, got {xml}"))
}

/// One member of a reported type as a number.
fn number(case: &str, xml: &str, name: &str) -> f64 {
    let text = required(case, xml, name);
    text.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: <{name}> must be a number, got {text:?}")
    })
}

/// The hardware figures of one instance type, as AWS publishes them.
#[derive(Debug, Clone, Copy)]
struct Published {
    /// `m6idn.16xlarge` and the like.
    instance_type: &'static str,
    /// (sustained Mbps, sustained IOPS, Mbps it reaches, IOPS it reaches).
    ebs: (i64, i64, i64, i64),
    /// The EBS volumes it can hold, and whether that limit is shared or dedicated. A figure
    /// the published row carries (f2's 32, r5b's 31) or the one the shared attachment pool
    /// leaves once the device count is a published fact (d3en.8xlarge's 11,
    /// m6idn.16xlarge's 25, t3's 27 — the pool rule, not a row).
    ebs_attachments: (i64, &'static str),
    /// Its local devices as (count, GB each), or `None` where it carries none.
    instance_store: Option<(i64, i64)>,
    /// Its network performance, as the published table writes it.
    network_performance: &'static str,
    /// (interfaces, private IPv4 addresses per interface), where AWS's figures for the type
    /// are its own rather than the ones its step of the size ladder publishes.
    network_limits: Option<(i64, i64)>,
    /// Its processor's maker and that processor's sustained frequency, in GHz.
    processor: (&'static str, f64),
}

/// Rule: the EBS, instance store, network and processor figures a type reports are the ones
/// AWS publishes for THAT type.
#[tokio::test]
async fn hardware_members_are_the_published_facts_of_the_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: the smallest, a middle and the largest size of a dense HDD storage family; three
    // sizes of an FPGA accelerated family whose EBS volumes have a limit of their own; the
    // smallest, a middle, the largest and the bare metal size of a network optimized family
    // with local NVMe; a small and the bare metal size of a family with EBS hardware of its
    // own; and three sizes of a burstable family whose EBS figures stop scaling at `large`.
    let rows = [
        Published {
            instance_type: "d3en.xlarge",
            ebs: (850, 5_000, 2_800, 15_000),
            ebs_attachments: (25, "shared"),
            instance_store: Some((2, 13_980)),
            network_performance: "Up to 25 Gigabit",
            network_limits: None,
            processor: ("Intel", 3.1),
        },
        Published {
            instance_type: "d3en.8xlarge",
            ebs: (5_000, 30_000, 5_000, 30_000),
            ebs_attachments: (11, "shared"),
            instance_store: Some((16, 13_980)),
            network_performance: "50 Gigabit",
            network_limits: None,
            processor: ("Intel", 3.1),
        },
        Published {
            instance_type: "d3en.12xlarge",
            ebs: (7_000, 40_000, 7_000, 40_000),
            ebs_attachments: (3, "shared"),
            instance_store: Some((24, 13_980)),
            network_performance: "75 Gigabit",
            network_limits: None,
            processor: ("Intel", 3.1),
        },
        Published {
            instance_type: "f2.6xlarge",
            ebs: (7_500, 30_000, 15_000, 60_000),
            ebs_attachments: (32, "dedicated"),
            instance_store: Some((1, 1_900)),
            network_performance: "Up to 12.5 Gigabit",
            network_limits: Some((8, 30)),
            processor: ("AMD", 3.6),
        },
        Published {
            instance_type: "f2.12xlarge",
            ebs: (15_000, 60_000, 15_000, 60_000),
            ebs_attachments: (32, "dedicated"),
            instance_store: Some((2, 1_900)),
            network_performance: "25 Gigabit",
            network_limits: Some((8, 30)),
            processor: ("AMD", 3.6),
        },
        Published {
            instance_type: "f2.48xlarge",
            ebs: (60_000, 240_000, 60_000, 240_000),
            ebs_attachments: (32, "dedicated"),
            instance_store: Some((8, 1_900)),
            network_performance: "100 Gigabit",
            network_limits: Some((15, 50)),
            processor: ("AMD", 3.6),
        },
        Published {
            instance_type: "m6idn.large",
            ebs: (1_562, 6_250, 25_000, 100_000),
            ebs_attachments: (26, "shared"),
            instance_store: Some((1, 118)),
            network_performance: "Up to 25 Gigabit",
            network_limits: Some((3, 10)),
            processor: ("Intel", 3.5),
        },
        Published {
            instance_type: "m6idn.16xlarge",
            ebs: (50_000, 200_000, 50_000, 200_000),
            ebs_attachments: (25, "shared"),
            instance_store: Some((2, 1_900)),
            network_performance: "100 Gigabit",
            network_limits: Some((15, 50)),
            processor: ("Intel", 3.5),
        },
        Published {
            instance_type: "m6idn.metal",
            ebs: (100_000, 400_000, 100_000, 400_000),
            ebs_attachments: (23, "shared"),
            instance_store: Some((4, 1_900)),
            network_performance: "200 Gigabit",
            network_limits: Some((15, 50)),
            processor: ("Intel", 3.5),
        },
        Published {
            instance_type: "r5b.xlarge",
            ebs: (2_500, 10_833, 10_000, 43_333),
            ebs_attachments: (31, "shared"),
            instance_store: None,
            network_performance: "Up to 10 Gigabit",
            network_limits: Some((4, 15)),
            processor: ("Intel", 3.1),
        },
        Published {
            instance_type: "r5b.metal",
            ebs: (60_000, 260_000, 60_000, 260_000),
            ebs_attachments: (31, "shared"),
            instance_store: None,
            network_performance: "25 Gigabit",
            network_limits: Some((15, 50)),
            processor: ("Intel", 3.1),
        },
        Published {
            instance_type: "t3.nano",
            ebs: (43, 250, 2_085, 11_800),
            ebs_attachments: (27, "shared"),
            instance_store: None,
            network_performance: "Up to 5 Gigabit",
            network_limits: Some((2, 2)),
            processor: ("Intel", 2.5),
        },
        Published {
            instance_type: "t3.large",
            ebs: (695, 4_000, 2_780, 15_700),
            ebs_attachments: (27, "shared"),
            instance_store: None,
            network_performance: "Up to 5 Gigabit",
            network_limits: Some((3, 12)),
            processor: ("Intel", 2.5),
        },
        Published {
            instance_type: "t3.xlarge",
            ebs: (695, 4_000, 2_780, 15_700),
            ebs_attachments: (27, "shared"),
            instance_store: None,
            network_performance: "Up to 5 Gigabit",
            network_limits: Some((4, 15)),
            processor: ("Intel", 2.5),
        },
    ];

    for row in rows {
        let case = row.instance_type;
        let body = described(case, &router, row.instance_type).await;
        let ebs_info = required(case, &body, "ebsInfo");
        let performance = required(case, &ebs_info, "ebsOptimizedInfo");
        let (baseline_mbps, baseline_iops, maximum_mbps, maximum_iops) = row.ebs;
        for (name, expected) in [
            ("baselineBandwidthInMbps", baseline_mbps),
            ("baselineIops", baseline_iops),
            ("maximumBandwidthInMbps", maximum_mbps),
            ("maximumIops", maximum_iops),
        ] {
            let reported = number(case, &performance, name) as i64;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {name} {expected}, got {reported}"
            );
        }
        // The throughput members are the bandwidth ones in the other unit, which is how the
        // published table's Mbps and MB/s columns relate.
        for (name, bandwidth) in [
            ("baselineThroughputInMBps", baseline_mbps),
            ("maximumThroughputInMBps", maximum_mbps),
        ] {
            let expected = bandwidth as f64 / MEGABITS_PER_MEGABYTE;
            let reported = number(case, &performance, name);
            assert!(
                (reported - expected).abs() < 0.001,
                "case {case:?}: expected {name} {expected}, got {reported}"
            );
        }

        let (attachments, limit_type) = row.ebs_attachments;
        let reported = number(case, &ebs_info, "maximumEbsAttachments") as i64;
        assert_eq!(
            reported, attachments,
            "case {case:?}: expected maximumEbsAttachments {attachments}, got {reported}"
        );
        let reported = required(case, &ebs_info, "attachmentLimitType");
        assert_eq!(
            reported, limit_type,
            "case {case:?}: expected attachmentLimitType {limit_type:?}, got {reported:?}"
        );

        let storage = element(&body, "instanceStorageInfo");
        match row.instance_store {
            Some((devices, gb_each)) => {
                let storage = storage.unwrap_or_else(|| {
                    panic!("case {case:?}: expected an instanceStorageInfo, got {body}")
                });
                let disks = required(case, &storage, "disks");
                let reported = number(case, &disks, "count") as i64;
                assert_eq!(
                    reported, devices,
                    "case {case:?}: expected {devices} local devices, got {reported}"
                );
                let reported = number(case, &disks, "sizeInGB") as i64;
                assert_eq!(
                    reported, gb_each,
                    "case {case:?}: expected local devices of {gb_each} GB, got {reported}"
                );
                let reported = number(case, &storage, "totalSizeInGB") as i64;
                assert_eq!(
                    reported,
                    devices * gb_each,
                    "case {case:?}: expected totalSizeInGB {}, got {reported}",
                    devices * gb_each
                );
            }
            None => assert!(
                storage.is_none(),
                "case {case:?}: a type with no local devices reports no instanceStorageInfo, \
                 got {storage:?}"
            ),
        }

        let network_info = required(case, &body, "networkInfo");
        let reported = required(case, &network_info, "networkPerformance");
        assert_eq!(
            reported, row.network_performance,
            "case {case:?}: expected networkPerformance {:?}, got {reported:?}",
            row.network_performance
        );
        if let Some((interfaces, ipv4)) = row.network_limits {
            let reported = number(case, &network_info, "maximumNetworkInterfaces") as i64;
            assert_eq!(
                reported, interfaces,
                "case {case:?}: expected maximumNetworkInterfaces {interfaces}, got {reported}"
            );
            let reported = number(case, &network_info, "ipv4AddressesPerInterface") as i64;
            assert_eq!(
                reported, ipv4,
                "case {case:?}: expected ipv4AddressesPerInterface {ipv4}, got {reported}"
            );
        }

        let processor_info = required(case, &body, "processorInfo");
        let (manufacturer, sustained_ghz) = row.processor;
        let reported = required(case, &processor_info, "manufacturer");
        assert_eq!(
            reported, manufacturer,
            "case {case:?}: expected processor manufacturer {manufacturer:?}, got {reported:?}"
        );
        let reported = number(case, &processor_info, "sustainedClockSpeedInGhz");
        assert!(
            (reported - sustained_ghz).abs() < 0.001,
            "case {case:?}: expected sustainedClockSpeedInGhz {sustained_ghz}, got {reported}"
        );
    }
}

/// Rule: the published figures do not scale with the size multiplier in the type's name.
///
/// The three claims here are what a formula over the name CANNOT produce: one family whose
/// EBS figures are equal across three sizes, one whose device size is constant while only the
/// count grows with the size, and two families of the SAME size that disagree on every
/// figure.
#[tokio::test]
async fn the_published_figures_do_not_follow_the_size_multiplier() {
    let (router, _engine) = super::ec2_query::emulator();

    let baseline = |body: &str, case: &str| -> i64 {
        let ebs_info = required(case, body, "ebsInfo");
        let performance = required(case, &ebs_info, "ebsOptimizedInfo");
        number(case, &performance, "baselineBandwidthInMbps") as i64
    };

    // One family, three sizes, one EBS figure: the ladder doubles the vCPUs and the figure
    // does not move.
    let mut figures = Vec::new();
    for instance_type in ["t3.large", "t3.xlarge", "t3.2xlarge"] {
        let body = described(instance_type, &router, instance_type).await;
        let vcpus = number(instance_type, &body, "defaultVCpus") as i64;
        figures.push((instance_type, vcpus, baseline(&body, instance_type)));
    }
    let (_, _, first) = figures[0];
    for (instance_type, vcpus, reported) in &figures {
        assert_eq!(
            *reported, first,
            "case {instance_type:?}: the family's sizes share one sustained EBS figure \
             ({first}) though this size has {vcpus} vCPUs, got {reported}"
        );
    }
    assert!(
        figures[0].1 < figures[2].1,
        "the rows must differ in size: got {figures:?}"
    );

    // One family, two sizes: the device SIZE is a fact of the family and only the count grows.
    let mut devices = Vec::new();
    for instance_type in ["d3en.2xlarge", "d3en.12xlarge"] {
        let body = described(instance_type, &router, instance_type).await;
        let storage = required(instance_type, &body, "instanceStorageInfo");
        let disks = required(instance_type, &storage, "disks");
        devices.push((
            instance_type,
            number(instance_type, &disks, "count") as i64,
            number(instance_type, &disks, "sizeInGB") as i64,
        ));
    }
    assert_eq!(
        devices[0].2, devices[1].2,
        "both sizes of the family carry devices of one size, got {devices:?}"
    );
    assert!(
        devices[0].1 < devices[1].1,
        "the larger size carries more of those devices, got {devices:?}"
    );

    // Two families, ONE size: every figure of the pair differs, so the size cannot be what
    // produced either.
    let left = described("m6idn.12xlarge", &router, "m6idn.12xlarge").await;
    let right = described("r5b.12xlarge", &router, "r5b.12xlarge").await;
    assert_ne!(
        baseline(&left, "m6idn.12xlarge"),
        baseline(&right, "r5b.12xlarge"),
        "two families of one size publish different EBS figures"
    );
    let left_network = required("m6idn.12xlarge", &left, "networkInfo");
    let right_network = required("r5b.12xlarge", &right, "networkInfo");
    assert_ne!(
        required("m6idn.12xlarge", &left_network, "networkPerformance"),
        required("r5b.12xlarge", &right_network, "networkPerformance"),
        "two families of one size publish different network performance"
    );
}

/// Rule: the accelerators and the host features a type is offered with are published per
/// type.
#[tokio::test]
async fn accelerators_and_host_features_are_published_per_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: the three sizes of the FPGA accelerated family, each with its own device count,
    // and types from four other families that carry no FPGA at all.
    let fpga_rows = [
        ("f2.6xlarge", Some(1)),
        ("f2.12xlarge", Some(2)),
        ("f2.48xlarge", Some(8)),
        ("d3en.8xlarge", None),
        ("m6idn.16xlarge", None),
        ("r5b.metal", None),
        ("t3.micro", None),
    ];

    for (instance_type, expected) in fpga_rows {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let reported = element(&body, "fpgaInfo");
        match expected {
            Some(count) => {
                let info = reported.unwrap_or_else(|| {
                    panic!("case {case:?}: an FPGA type reports its fpgaInfo, got {body}")
                });
                let reported = number(case, &info, "count") as i64;
                assert_eq!(
                    reported, count,
                    "case {case:?}: expected {count} FPGA accelerators, got {reported}"
                );
                let name = required(case, &info, "name");
                let manufacturer = required(case, &info, "manufacturer");
                assert_eq!(
                    (name.as_str(), manufacturer.as_str()),
                    ("Virtex UltraScale+ (VU47P)", "Xilinx"),
                    "case {case:?}: expected the published accelerator, got {info}"
                );
                let each = number(case, &info, "sizeInMiB") as i64;
                let total = number(case, &info, "totalFpgaMemoryInMiB") as i64;
                assert_eq!(
                    total,
                    each * count,
                    "case {case:?}: the total FPGA memory is one device's ({each} MiB) times \
                     the count ({count}), got {total}"
                );
            }
            None => assert!(
                reported.is_none(),
                "case {case:?}: a type with no FPGA reports no fpgaInfo, got {reported:?}"
            ),
        }
    }

    // Rows: the host features, each a flag a client branches on, over the families above —
    // Dedicated Hosts offered on a burstable family and not on a storage family, CloudWatch
    // action based recovery on a bare metal size, hibernation refused by a family the
    // prerequisites do not name, and neither reboot migration nor a PTP clock on hardware
    // that has neither. A type with an Elastic Fabric Adapter is offered one at the sizes AWS
    // publishes it for, not at every large size.
    let feature_rows = [
        ("t3.micro", "dedicatedHostsSupported", "true"),
        ("t3.2xlarge", "dedicatedHostsSupported", "true"),
        ("d3en.8xlarge", "dedicatedHostsSupported", "false"),
        ("f2.12xlarge", "dedicatedHostsSupported", "true"),
        ("r5b.metal", "autoRecoverySupported", "true"),
        ("r5b.24xlarge", "autoRecoverySupported", "true"),
        ("r5b.large", "hibernationSupported", "false"),
        ("r5b.4xlarge", "hibernationSupported", "false"),
        ("t3.large", "hibernationSupported", "true"),
        ("d3en.12xlarge", "rebootMigrationSupport", "unsupported"),
        ("m6idn.16xlarge", "rebootMigrationSupport", "unsupported"),
        ("t3.nano", "rebootMigrationSupport", "unsupported"),
        ("d3en.12xlarge", "phcSupport", "unsupported"),
        ("f2.48xlarge", "phcSupport", "unsupported"),
        ("m6idn.16xlarge", "efaSupported", "false"),
        ("m6idn.24xlarge", "efaSupported", "false"),
        ("m6idn.32xlarge", "efaSupported", "true"),
        ("m6idn.metal", "efaSupported", "true"),
        ("f2.6xlarge", "efaSupported", "false"),
        ("f2.48xlarge", "efaSupported", "true"),
        ("f2.6xlarge", "encryptionInTransitSupported", "true"),
        ("f2.12xlarge", "encryptionInTransitSupported", "true"),
        ("t3.nano", "encryptionInTransitSupported", "false"),
    ];

    for (instance_type, member, expected) in feature_rows {
        let case = format!("{instance_type} {member}");
        let body = described(&case, &router, instance_type).await;
        // A member of networkInfo is read out of that block; the rest are the type's own.
        let scope = if member.starts_with("efa") || member.starts_with("encryptionInTransit") {
            required(&case, &body, "networkInfo")
        } else {
            body
        };
        let reported = required(&case, &scope, member);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {member} {expected:?}, got {reported:?}"
        );
    }
}

/// Rule, the other half: a type the published table holds no row for is still described, by
/// the derivation over its name — the table is where AWS's figures are recorded, not a gate
/// on being described at all. And every row of the table names a type of the catalog, so no
/// row is one the service can never read.
#[tokio::test]
async fn a_type_with_no_published_row_is_still_described() {
    let (router, _engine) = super::ec2_query::emulator();

    // Every published row's key is an instance type the service describes: a key that named
    // no type would be a row nothing reads, and its figures would never reach a caller.
    let mut rows = 0;
    for instance_type in service_com_amazonaws_ec2_amazonec2::state::published_instance_types() {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let reported = required(case, &body, "instanceType");
        assert_eq!(
            reported, instance_type,
            "case {case:?}: a published row must name a type the catalog describes, got \
             {reported:?}"
        );
        rows += 1;
    }
    assert!(rows > 0, "the published table must hold rows, got {rows}");

    // Rows: families of four different shapes, none of them in the table above.
    for instance_type in ["m5.large", "c6g.medium", "i3.2xlarge", "m5.metal"] {
        let case = instance_type;
        let body = described(case, &router, instance_type).await;
        let ebs_info = required(case, &body, "ebsInfo");
        let performance = required(case, &ebs_info, "ebsOptimizedInfo");
        let baseline = number(case, &performance, "baselineBandwidthInMbps") as i64;
        let maximum = number(case, &performance, "maximumBandwidthInMbps") as i64;
        assert!(
            baseline > 0 && maximum >= baseline,
            "case {case:?}: expected a derived EBS figure pair, got {baseline}/{maximum}"
        );
        let network_info = required(case, &body, "networkInfo");
        let performance = required(case, &network_info, "networkPerformance");
        assert!(
            performance.ends_with("Gigabit"),
            "case {case:?}: expected a derived network performance, got {performance:?}"
        );
        let attachments = number(case, &ebs_info, "maximumEbsAttachments") as i64;
        assert!(
            attachments > 0,
            "case {case:?}: expected a derived EBS attachment limit, got {attachments}"
        );
    }
}
