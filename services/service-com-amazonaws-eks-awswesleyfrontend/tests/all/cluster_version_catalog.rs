// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CATALOG members of `DescribeClusterVersions` — the
//! parts of a `ClusterVersionInformation` that are published by the service
//! rather than derived from the clock: `kubernetesPatchVersion`,
//! `controlPlaneScalingTiers` and `controlPlaneComponentConfig`.
//!
//! `cluster_versions.rs` covers the calendar itself (which rows are reported,
//! their dates, their clock-derived status, the filters and the paging). This
//! suite covers the RULE that every row the catalog reports is MEMBER-COMPLETE and
//! that what it publishes is usable: a tier name it reports is a tier
//! `CreateCluster` accepts, and a component default it reports sits inside the
//! constraint it publishes beside it.
//!
//! Each assertion runs over a TABLE: every row of the shipped calendar (not one
//! cited version), every published tier, and every published parameter — and each
//! one names its case and prints the actual value.

use std::sync::Arc;

use service_com_amazonaws_eks_awswesleyfrontend::state::{self, ClusterVersionRecord};
use service_com_amazonaws_eks_awswesleyfrontend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{minimal_request, REGION};
use crate::wire::{call, encode_label, Answer};

/// The tier names `ControlPlaneScalingConfig$tier` accepts, from the model:
/// `smithy_query(query="shape", shape="ProvisionedControlPlaneTier")` gives
/// `standard | tier-xl | tier-2xl | tier-4xl | tier-8xl`. `standard` is the tier
/// a cluster runs on when it asks for no provisioned control plane, so the
/// PROVISIONED tiers the catalog publishes are the other four.
const MODELLED_TIERS: &[&str] = &["standard", "tier-xl", "tier-2xl", "tier-4xl", "tier-8xl"];

/// The emulator under test, plus the engine behind it so a suite can install its
/// own calendar rows the way the production store seeds them.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

async fn describe_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/cluster-versions".to_string()
    } else {
        format!("/cluster-versions?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

fn rows(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a clusterVersions list, got {:?}", answer.body))
        .clone()
}

/// The rows of the whole shipped calendar, which is the table every suite below
/// runs over.
///
/// Read with `includeAll`, because an unfiltered listing publishes only the
/// versions Amazon EKS still offers — a row whose extended support has ended is
/// reported only to a caller that asks for it — and these suites are about what
/// every row of the calendar reports, not about which rows are offered.
async fn whole_calendar(router: &MultiServiceRouter) -> Vec<serde_json::Value> {
    let answer = describe_versions(router, "includeAll=true").await;
    assert_eq!(
        answer.status, 200,
        "setup: expected HTTP 200 from the catalog, got {} ({:?})",
        answer.status, answer.body
    );
    let listed = rows(&answer);
    assert!(
        !listed.is_empty(),
        "setup: the catalog must publish at least one version, got {:?}",
        answer.body
    );
    listed
}

/// A row's `clusterVersion`, used to name the case an assertion failed on.
fn case_of(row: &serde_json::Value) -> String {
    row["clusterVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a clusterVersion string, got {row:?}"))
        .to_string()
}

/// The `VersionStatus` members, from the model:
/// `smithy_query(query="shape", shape="VersionStatus")` gives
/// `UNSUPPORTED | STANDARD_SUPPORT | EXTENDED_SUPPORT`.
const MODELLED_VERSION_STATUS: &[&str] = &["UNSUPPORTED", "STANDARD_SUPPORT", "EXTENDED_SUPPORT"];

/// The `ClusterVersionStatus` members, from the model:
/// `smithy_query(query="shape", shape="ClusterVersionStatus")` gives
/// `unsupported | standard-support | extended-support`. These are the tokens the
/// DEPRECATED `status` FILTER accepts; they are not what a reported row carries.
const MODELLED_CLUSTER_VERSION_STATUS: &[&str] =
    &["unsupported", "standard-support", "extended-support"];

// ---------------------------------------------------------------------------
// RULE: a row's `defaultPlatformVersion` is the platform version a cluster of
// that Kubernetes version is created at — an `eks.<n>` revision — so the catalog
// and the cluster slice cannot disagree about which platform a new cluster runs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_row_publishes_the_platform_version_a_new_cluster_runs() {
    let (router, _engine) = emulator();
    let listed = whole_calendar(&router).await;
    // The rows a cluster can be CREATED on are the ones an unfiltered listing
    // publishes: a row whose extended support has ended is a version Amazon EKS no
    // longer offers, so `CreateCluster` refuses it.
    let offered = rows(&describe_versions(&router, "").await);

    // Rows: every Kubernetes version the catalog offers, pinned on the create,
    // plus the create that names NO version at all (which lands on the row the
    // catalog marks `defaultVersion`).
    let mut cases: Vec<(String, Option<String>)> = offered
        .iter()
        .map(|row| (case_of(row), Some(case_of(row))))
        .collect();
    cases.push(("version absent".to_string(), None));

    for (case, version) in cases {
        let name = format!("platform-{}", case.replace(['.', ' '], "-"));
        let mut body = minimal_request(&name);
        if let Some(version) = &version {
            body["version"] = serde_json::Value::String(version.clone());
        }
        let created = call(&router, REGION, "POST", "/clusters", &body.to_string()).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected HTTP 200 creating a cluster, got {} ({:?})",
            created.status, created.body
        );
        let described = call(
            &router,
            REGION,
            "GET",
            &format!("/clusters/{}", encode_label(&name)),
            "",
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected HTTP 200 reading the cluster back, got {} ({:?})",
            described.status, described.body
        );
        let cluster = &described.body["cluster"];
        let ran_version = cluster["version"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a cluster version, got {cluster:?}"))
            .to_string();
        let row = listed
            .iter()
            .find(|row| case_of(row) == ran_version)
            .unwrap_or_else(|| {
                panic!("case {case}: the cluster runs {ran_version:?}, which the catalog must publish")
            });
        let published = row["defaultPlatformVersion"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a defaultPlatformVersion, got {row:?}")
            })
            .to_string();
        assert_eq!(
            cluster["platformVersion"].as_str(),
            Some(published.as_str()),
            "case {case}: a new cluster must run the platform version its own row publishes \
             ({published:?}), got {:?}",
            cluster["platformVersion"]
        );
        // An EKS platform version is `eks.<n>`, the n-th platform revision of a
        // Kubernetes minor version, so the published value is that form and its
        // revision is a positive number.
        let revision = published
            .strip_prefix("eks.")
            .unwrap_or_else(|| panic!("case {case}: expected an eks.<n> form, got {published:?}"))
            .parse::<u32>()
            .unwrap_or_else(|_| {
                panic!("case {case}: expected a numeric platform revision, got {published:?}")
            });
        assert!(
            revision >= 1,
            "case {case}: platform revisions are numbered from 1, got {published:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: both lifecycle members a row reports carry the SAME state in the current
// `VersionStatus` spelling — `status` "is deprecated. Use `versionStatus`
// instead, as that field matches for input and output of this action", so only
// the current member's tokens match in both directions, while the deprecated
// member's FILTER keeps the `ClusterVersionStatus` spelling.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn both_lifecycle_members_report_the_current_status_spelling() {
    let (router, engine) = emulator();

    // Rows: one calendar row per support class, placed by its OWN dates relative
    // to the clock, so every token the rule can produce is exercised; each row
    // carries the deprecated spelling of the same state for the filter leg.
    let now = std::time::SystemTime::now();
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    let cases = [
        ("4.1", "STANDARD_SUPPORT", "standard-support", 60, 425),
        ("4.2", "EXTENDED_SUPPORT", "extended-support", -30, 300),
        ("4.3", "UNSUPPORTED", "unsupported", -600, -200),
    ];
    let store = state::service_state(&*engine).expect("the eks store is reachable in-process");
    store.lock().cluster_versions = cases
        .iter()
        .map(|(version, _, _, standard, extended)| ClusterVersionRecord {
            cluster_version: (*version).to_string(),
            cluster_type: state::CLUSTER_TYPE_EKS.to_string(),
            default_version: false,
            release_date: now - 1000 * day,
            end_of_standard_support: offset(now, *standard, day),
            end_of_extended_support: offset(now, *extended, day),
        })
        .collect();

    let listed = whole_calendar(&router).await;
    for (version, expected, deprecated_spelling, _, _) in cases {
        let row = listed
            .iter()
            .find(|row| case_of(row) == version)
            .unwrap_or_else(|| panic!("case {version}: expected the row to be reported, got {listed:?}"));
        assert_eq!(
            row["versionStatus"].as_str(),
            Some(expected),
            "case {version}: expected versionStatus {expected:?}, got {:?}",
            row["versionStatus"]
        );
        assert_eq!(
            row["status"].as_str(),
            Some(expected),
            "case {version}: the deprecated member reports the same state in the same (current) \
             spelling, got {:?}",
            row["status"]
        );
        let reported = row["status"].as_str().unwrap_or_default();
        assert!(
            MODELLED_VERSION_STATUS.contains(&reported),
            "case {version}: a reported status must be a modelled VersionStatus member \
             ({MODELLED_VERSION_STATUS:?}), got {reported:?}"
        );
        assert!(
            !MODELLED_CLUSTER_VERSION_STATUS.contains(&reported),
            "case {version}: the ClusterVersionStatus spelling is the FILTER's, not the \
             report's, got {reported:?}"
        );

        // The input side is unchanged: the deprecated filter still takes its own
        // spelling, and the row it selects reports the current one.
        let filtered = describe_versions(&router, &format!("status={deprecated_spelling}")).await;
        assert_eq!(
            filtered.status, 200,
            "case {version}: expected HTTP 200 filtering on {deprecated_spelling:?}, got {} ({:?})",
            filtered.status, filtered.body
        );
        let selected: Vec<String> = rows(&filtered).iter().map(case_of).collect();
        assert_eq!(
            selected,
            vec![version.to_string()],
            "case {version}: the deprecated filter spelling {deprecated_spelling:?} must select \
             the row in that state, got {selected:?}"
        );
    }
}

/// `now` shifted by `days`, which may be negative — the calendar rows above are
/// placed relative to the clock so the suite never depends on the wall date.
fn offset(now: std::time::SystemTime, days: i64, day: std::time::Duration) -> std::time::SystemTime {
    if days >= 0 {
        now + day * u32::try_from(days).unwrap_or(0)
    } else {
        now - day * u32::try_from(-days).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// RULE: every published scaling tier reports the control-plane component
// configuration a cluster on that tier runs with — this service publishes one
// such configuration per version, so a tier's
// `controlPlaneComponentConfigOverrides` is that version's own configuration
// rather than a value invented per tier.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_tier_publishes_the_component_configuration_it_runs() {
    let (router, _engine) = emulator();

    // Rows: every row of the shipped calendar, and within each, every published
    // tier — the rule is about a tier, not about the first one.
    for row in whole_calendar(&router).await {
        let case = case_of(&row);
        let version_config = &row["controlPlaneComponentConfig"];
        assert!(
            version_config.is_object(),
            "case {case}: expected a controlPlaneComponentConfig object, got {row:?}"
        );
        let tiers = row["controlPlaneScalingTiers"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a controlPlaneScalingTiers list, got {row:?}")
            })
            .clone();
        for tier in &tiers {
            let tier_name = tier["tierName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a tierName, got {tier:?}"))
                .to_string();
            let overrides = &tier["controlPlaneComponentConfigOverrides"];
            assert!(
                overrides.is_object(),
                "case {case}/{tier_name}: a tier must publish the component configuration it \
                 runs with, got {overrides:?}"
            );
            assert_eq!(
                overrides, version_config,
                "case {case}/{tier_name}: one configuration is published per version, so a \
                 tier's must be that one, got {overrides:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: every row the catalog reports carries a `kubernetesPatchVersion` of its
// OWN minor version — the minor version plus a patch level — so a caller reading
// the catalog can pin a patch version without guessing one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_row_publishes_a_patch_version_of_its_own_minor_version() {
    let (router, engine) = emulator();

    // Rows: the whole shipped calendar, plus two minor versions the shipped table
    // does not carry at all, installed into state — the rule is about the row, not
    // about the versions this emulator happens to ship with.
    let store = state::service_state(&*engine).expect("the eks store is reachable in-process");
    let now = std::time::SystemTime::now();
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    store.lock().cluster_versions.extend([
        ClusterVersionRecord {
            cluster_version: "3.7".to_string(),
            cluster_type: state::CLUSTER_TYPE_EKS.to_string(),
            default_version: false,
            release_date: now - 10 * day,
            end_of_standard_support: now + 400 * day,
            end_of_extended_support: now + 760 * day,
        },
        ClusterVersionRecord {
            cluster_version: "3.100".to_string(),
            cluster_type: state::CLUSTER_TYPE_EKS.to_string(),
            default_version: false,
            release_date: now - 5 * day,
            end_of_standard_support: now + 400 * day,
            end_of_extended_support: now + 760 * day,
        },
    ]);

    for row in whole_calendar(&router).await {
        let case = case_of(&row);
        let patch = row["kubernetesPatchVersion"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a kubernetesPatchVersion string, got {row:?}")
            })
            .to_string();
        assert!(
            patch.starts_with(&format!("{case}.")),
            "case {case}: a patch version must be a patch level of its own minor version, got \
             {patch:?}"
        );
        let level = patch
            .strip_prefix(&format!("{case}."))
            .unwrap_or_default()
            .to_string();
        assert!(
            !level.is_empty() && level.chars().all(|digit| digit.is_ascii_digit()),
            "case {case}: expected <minor>.<patch level> with a numeric level, got {patch:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: every row publishes the provisioned control-plane scaling tiers, each
// with the three capabilities a caller sizes a control plane from, bounded by the
// modelled list length (0..10) and named by the modelled tier enum — and the same
// tiers for every row, because the tier catalog is published by the service.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_row_publishes_the_provisioned_scaling_tiers() {
    let (router, _engine) = emulator();
    let listed = whole_calendar(&router).await;

    let mut published_per_row: Vec<Vec<String>> = Vec::new();
    for row in &listed {
        let case = case_of(row);
        let tiers = row["controlPlaneScalingTiers"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected a controlPlaneScalingTiers list, got {row:?}")
            })
            .clone();
        assert!(
            !tiers.is_empty() && tiers.len() <= 10,
            "case {case}: the modelled list holds 0..10 tiers and a usable catalog holds at \
             least one, got {} ({tiers:?})",
            tiers.len()
        );

        let mut names: Vec<String> = Vec::new();
        // Each capability must grow with the tier, which is what makes the ladder
        // a caller can pick from, so the previous row's numbers are carried along.
        let mut previous: Option<(i64, i64, i64)> = None;
        for tier in &tiers {
            let tier_name = tier["tierName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a tierName, got {tier:?}"))
                .to_string();
            assert!(
                MODELLED_TIERS.contains(&tier_name.as_str()),
                "case {case}: a published tier must be a modelled ProvisionedControlPlaneTier \
                 member ({MODELLED_TIERS:?}), got {tier_name:?}"
            );
            assert_ne!(
                tier_name, "standard",
                "case {case}: `standard` is the unprovisioned tier, not a provisioned scaling \
                 tier, got {tier:?}"
            );
            let capabilities = [
                "apiRequestConcurrency",
                "podSchedulingRatePerSecond",
                "clusterDatabaseSizeGb",
            ]
            .map(|member| {
                tier[member].as_i64().unwrap_or_else(|| {
                    panic!("case {case}/{tier_name}: expected {member} to be an integer, got {tier:?}")
                })
            });
            for (member, value) in [
                "apiRequestConcurrency",
                "podSchedulingRatePerSecond",
                "clusterDatabaseSizeGb",
            ]
            .iter()
            .zip(capabilities)
            {
                assert!(
                    value > 0,
                    "case {case}/{tier_name}: expected a positive {member}, got {value}"
                );
            }
            let current = (capabilities[0], capabilities[1], capabilities[2]);
            if let Some((concurrency, pods, database)) = previous {
                assert!(
                    current.0 > concurrency && current.1 > pods && current.2 > database,
                    "case {case}/{tier_name}: every capability must grow with the tier, got \
                     {current:?} after {:?}",
                    (concurrency, pods, database)
                );
            }
            previous = Some(current);
            names.push(tier_name);
        }
        published_per_row.push(names);
    }

    let first = published_per_row
        .first()
        .expect("the calendar publishes at least one row")
        .clone();
    for (row, names) in listed.iter().zip(&published_per_row) {
        assert_eq!(
            *names, first,
            "case {}: the tier catalog is published by the service, so every version reports \
             the same tiers, got {names:?} against {first:?}",
            case_of(row)
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a tier name the catalog publishes is a tier `CreateCluster` accepts, and
// the cluster reads back running it — the catalog and the cluster slice cannot
// disagree about which tiers exist.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_published_tier_can_be_asked_for_and_reads_back() {
    let (router, _engine) = emulator();
    let listed = whole_calendar(&router).await;
    let tiers = listed[0]["controlPlaneScalingTiers"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a tier list, got {:?}", listed[0]))
        .clone();

    for tier in &tiers {
        let tier_name = tier["tierName"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a tierName, got {tier:?}"))
            .to_string();
        let name = format!("tiered-{}", tier_name.replace('-', ""));
        let mut body = minimal_request(&name);
        body["controlPlaneScalingConfig"] = serde_json::json!({ "tier": tier_name });
        let created = call(&router, REGION, "POST", "/clusters", &body.to_string()).await;
        assert_eq!(
            created.status, 200,
            "case {tier_name}: a published tier must be acceptable to CreateCluster, got {} \
             ({:?})",
            created.status, created.body
        );
        let described = call(
            &router,
            REGION,
            "GET",
            &format!("/clusters/{}", encode_label(&name)),
            "",
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {tier_name}: expected HTTP 200 reading the cluster back, got {} ({:?})",
            described.status, described.body
        );
        assert_eq!(
            described.body["cluster"]["controlPlaneScalingConfig"]["tier"].as_str(),
            Some(tier_name.as_str()),
            "case {tier_name}: the cluster must read back on the tier it was created with, got \
             {:?}",
            described.body["cluster"]["controlPlaneScalingConfig"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: every row publishes the control-plane component configuration — for each
// parameter a cluster may override, the default a cluster that overrides nothing
// runs with AND the constraint an override is chosen from — and every published
// default satisfies the constraint published beside it.
// ---------------------------------------------------------------------------

/// A duration parameter's published default, as `(defaultValue, min, max)`.
fn duration_parameter(case: &str, member: &str, value: &serde_json::Value) -> (String, String, String) {
    let default_value = value["defaultValue"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}/{member}: expected a defaultValue, got {value:?}"))
        .to_string();
    let min = value["constraints"]["min"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}/{member}: expected constraints.min, got {value:?}"))
        .to_string();
    let max = value["constraints"]["max"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}/{member}: expected constraints.max, got {value:?}"))
        .to_string();
    (default_value, min, max)
}

/// A single-unit duration (`eventTtl`: "Valid values are single-unit durations
/// such as `30m` or `1h`") in seconds, so two of them can be compared.
fn duration_seconds(case: &str, text: &str) -> u64 {
    let (digits, unit) = text.split_at(
        text.find(|character: char| !character.is_ascii_digit())
            .unwrap_or_else(|| panic!("case {case}: expected a single-unit duration, got {text:?}")),
    );
    let count: u64 = digits
        .parse()
        .unwrap_or_else(|_| panic!("case {case}: expected a duration count, got {text:?}"));
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        other => panic!("case {case}: expected a seconds/minutes/hours/days unit, got {other:?}"),
    };
    count * scale
}

#[tokio::test]
async fn every_row_publishes_component_defaults_inside_their_constraints() {
    let (router, _engine) = emulator();

    for row in whole_calendar(&router).await {
        let case = case_of(&row);
        let config = &row["controlPlaneComponentConfig"];
        assert!(
            config.is_object(),
            "case {case}: expected a controlPlaneComponentConfig object, got {config:?}"
        );

        // The two duration parameters the model documents as single-unit durations.
        for (member, value) in [
            ("eventTtl", &config["kubeApiServerConfig"]["eventTtl"]),
            (
                "horizontalPodAutoscalerSyncPeriod",
                &config["kubeControllerManagerConfig"]
                    ["horizontalPodAutoscalerControllerConfig"]
                    ["horizontalPodAutoscalerSyncPeriod"],
            ),
        ] {
            let (default_value, min, max) = duration_parameter(&case, member, value);
            let (default_secs, min_secs, max_secs) = (
                duration_seconds(&case, &default_value),
                duration_seconds(&case, &min),
                duration_seconds(&case, &max),
            );
            assert!(
                min_secs <= default_secs && default_secs <= max_secs,
                "case {case}/{member}: the published default must sit inside the published \
                 window, got {default_value:?} in {min:?}..{max:?}"
            );
            assert!(
                min_secs < max_secs,
                "case {case}/{member}: expected a non-empty window, got {min:?}..{max:?}"
            );
        }

        // The NodePort range: a default range, and a window each end is chosen from.
        let ports = &config["kubeApiServerConfig"]["serviceNodePortRange"];
        let default_min = ports["defaultValue"]["minPort"].as_i64().unwrap_or_else(|| {
            panic!("case {case}/serviceNodePortRange: expected a default minPort, got {ports:?}")
        });
        let default_max = ports["defaultValue"]["maxPort"].as_i64().unwrap_or_else(|| {
            panic!("case {case}/serviceNodePortRange: expected a default maxPort, got {ports:?}")
        });
        assert!(
            default_min < default_max,
            "case {case}/serviceNodePortRange: expected a non-empty default range, got \
             {default_min}-{default_max}"
        );
        for (member, default_port) in [("minPort", default_min), ("maxPort", default_max)] {
            let min = ports["constraints"][member]["min"].as_i64().unwrap_or_else(|| {
                panic!("case {case}/{member}: expected a constraint min, got {ports:?}")
            });
            let max = ports["constraints"][member]["max"].as_i64().unwrap_or_else(|| {
                panic!("case {case}/{member}: expected a constraint max, got {ports:?}")
            });
            assert!(
                min <= default_port && default_port <= max,
                "case {case}/{member}: the default port must sit inside its window, got \
                 {default_port} in {min}..{max}"
            );
            // A port number is a 16-bit unsigned integer, so no window may run past
            // 65535.
            assert!(
                max <= 65_535 && min >= 1,
                "case {case}/{member}: a port window must stay inside 1..65535, got {min}..{max}"
            );
        }

        // The pod-GC threshold: an integer default inside the range the MODEL
        // publishes for `terminatedPodGcThreshold` (1..1000000, from
        // `smithy_query(query="shape", shape="PodGcControllerConfigRequest")`).
        let threshold = &config["kubeControllerManagerConfig"]["podGcControllerConfig"]
            ["terminatedPodGcThreshold"];
        let default_value = threshold["defaultValue"].as_i64().unwrap_or_else(|| {
            panic!("case {case}/terminatedPodGcThreshold: expected a defaultValue, got {threshold:?}")
        });
        let min = threshold["constraints"]["min"].as_i64().unwrap_or_else(|| {
            panic!("case {case}/terminatedPodGcThreshold: expected constraints.min, got {threshold:?}")
        });
        let max = threshold["constraints"]["max"].as_i64().unwrap_or_else(|| {
            panic!("case {case}/terminatedPodGcThreshold: expected constraints.max, got {threshold:?}")
        });
        assert_eq!(
            (min, max),
            (1, 1_000_000),
            "case {case}/terminatedPodGcThreshold: expected the modelled range, got {min}..{max}"
        );
        assert!(
            min <= default_value && default_value <= max,
            "case {case}/terminatedPodGcThreshold: the default must sit inside the range, got \
             {default_value} in {min}..{max}"
        );

        // The scheduler's scoring strategy: a default strategy whose type is one of
        // the published allowed values, and resource weights inside the published
        // weight range, each naming a published resource.
        let scoring =
            &config["kubeSchedulerConfig"]["nodeResourcesFit"]["scoringStrategy"];
        let allowed: Vec<String> = scoring["constraints"]["scoringStrategy"]["allowedValues"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}/scoringStrategy: expected allowedValues, got {scoring:?}")
            })
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        // Source: the modelled `ScoringStrategyType` enum.
        assert_eq!(
            allowed,
            vec!["LeastAllocated".to_string(), "MostAllocated".to_string()],
            "case {case}/scoringStrategy: expected the modelled enum members, got {allowed:?}"
        );
        let default_type = scoring["defaultValue"]["type"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}/scoringStrategy: expected a default type, got {scoring:?}")
            })
            .to_string();
        assert!(
            allowed.contains(&default_type),
            "case {case}/scoringStrategy: the default must be one of the allowed values, got \
             {default_type:?} against {allowed:?}"
        );
        let allowed_resources: Vec<String> = scoring["constraints"]["resources"]["name"]
            ["allowedValues"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}/scoringStrategy: expected resource allowedValues, got {scoring:?}")
            })
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        // Source: `ResourceWeightValue`'s modelled range, 1..100.
        let weight_min = scoring["constraints"]["resources"]["weight"]["min"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}/scoringStrategy: expected a weight min"));
        let weight_max = scoring["constraints"]["resources"]["weight"]["max"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}/scoringStrategy: expected a weight max"));
        assert_eq!(
            (weight_min, weight_max),
            (1, 100),
            "case {case}/scoringStrategy: expected the modelled weight range, got \
             {weight_min}..{weight_max}"
        );
        let resources = scoring["defaultValue"]["resources"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}/scoringStrategy: expected default resources, got {scoring:?}")
            })
            .clone();
        assert!(
            !resources.is_empty(),
            "case {case}/scoringStrategy: expected at least one scored resource, got {scoring:?}"
        );
        for resource in &resources {
            let name = resource["name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}/scoringStrategy: expected a resource name, got {resource:?}")
                })
                .to_string();
            let weight = resource["weight"].as_i64().unwrap_or_else(|| {
                panic!("case {case}/scoringStrategy: expected a resource weight, got {resource:?}")
            });
            assert!(
                allowed_resources.contains(&name),
                "case {case}/scoringStrategy/{name}: a scored resource must be one the \
                 constraint allows ({allowed_resources:?})"
            );
            assert!(
                weight_min <= weight && weight <= weight_max,
                "case {case}/scoringStrategy/{name}: the weight must sit inside the published \
                 range, got {weight} in {weight_min}..{weight_max}"
            );
        }
    }
}
