// SPDX-License-Identifier: CC0-1.0

//! What a platform-version response REPORTS about a published platform version,
//! driven end to end over the service's awsQuery HTTP surface.
//!
//! The rule under test: a platform-version response projects every member its
//! catalog record defines — the platform's category, the operating system it
//! runs on, the add-ons and tiers it supports, the languages it runs, its
//! description, its maintainer, its publication instants and the
//! life-cycle state of the version and of its branch — and reports no detail the
//! catalog does NOT define: the frameworks and the custom AMIs of a platform
//! version come from the platform definition its owner built it from, which a
//! version Elastic Beanstalk publishes has none of. `ListPlatformVersions`
//! and `DescribePlatformVersion` project the SAME record, the summary carrying
//! the members `PlatformSummary` models and the description the members
//! `PlatformDescription` models.
//!
//! The table is the service's OWN published catalog, `eb::state::SOLUTION_STACKS`
//! (what `ListAvailableSolutionStacks` reports), read row by row — and, within a
//! row, version by version over the versions that branch publishes
//! (`SolutionStack::published_versions`: its current release and the one that
//! release superseded) — rather than written out here: its rows span the
//! categories, the two operating systems
//! and their versions, platforms that run one language and platforms that run
//! none, and
//! branches that support both environment tiers as well as one that supports only
//! the web-server tier. Every expectation is computed from the catalog row plus
//! this suite's own statement of the rule, so no row is a transcription of a
//! recorded response.
//!
//! The HTTP harness ([`crate::lifecycle::Emulator`]) is the one the lifecycle
//! suite drives the service with.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{elements, instant, Answer, Emulator, TEST_REGION};

/// The add-ons EVERY platform version this service publishes supports: logs to
/// Amazon S3 and to CloudWatch Logs, and enhanced health through the health
/// daemon. Source: `PlatformFilter$Values` names `Log/S3`,
/// `Monitoring/Healthd` and `WorkerDaemon/SQSD` as `SupportedAddon` values, and
/// the recorded platform-version responses carry `Log/CloudWatch` beside them.
/// Sorted, which is the order a version reports them in.
const ALWAYS_SUPPORTED_ADDONS: &[&str] = &["Log/CloudWatch", "Log/S3", "Monitoring/Healthd"];

/// The add-on that runs a WORKER environment, so an add-on only of a version
/// whose branch supports [`WORKER_TIER`]. See [`ALWAYS_SUPPORTED_ADDONS`].
const WORKER_ADDON: &str = "WorkerDaemon/SQSD";

/// The worker tier, from `PlatformBranchSummary$SupportedTierList`: "Possible
/// values: `WebServer/Standard` | `Worker/SQS/HTTP`".
const WORKER_TIER: &str = "Worker/SQS/HTTP";

/// `PlatformLifecycleState` of a version that is its branch's recommended one,
/// from `PlatformDescription$PlatformLifecycleState` ("Possible values:
/// `Recommended` | `null`").
const RECOMMENDED: &str = "Recommended";

/// The runtime and the operating system text a branch name is built from:
/// `<runtime> running on <os>`. This suite's own split of the published form, so
/// the expectations do not come from the service's.
fn runtime_and_operating_system(branch_name: &str) -> (&str, &str) {
    branch_name
        .split_once(" running on ")
        .unwrap_or_else(|| panic!("the catalog branch {branch_name:?} names its operating system"))
}

/// The `OperatingSystemName` and `OperatingSystemVersion` a branch name states:
/// the operating system text is `64bit <name> <version>`, so the width prefix
/// goes, a trailing numeric component is the version and what precedes it is the
/// name.
fn operating_system(branch_name: &str) -> (String, Option<String>) {
    let (_, os) = runtime_and_operating_system(branch_name);
    let os = os.strip_prefix("64bit ").unwrap_or(os);
    match os.rsplit_once(' ') {
        Some((name, version))
            if !version.is_empty()
                && version
                    .chars()
                    .all(|character| character.is_ascii_digit() || character == '.') =>
        {
            (name.to_string(), Some(version.to_string()))
        }
        _ => (os.to_string(), None),
    }
}

/// The `Description` a published version reports: its solution stack name
/// without the version component, i.e. `<os> running <runtime>`.
fn description(branch_name: &str) -> String {
    let (runtime, os) = runtime_and_operating_system(branch_name);
    format!("{os} running {runtime}")
}

/// `state` with its first letter capitalized — this suite's statement of the
/// form a PLATFORM VERSION reports a branch life-cycle state in
/// (`PlatformDescription$PlatformBranchLifecycleState`: "`Beta` | `Supported` |
/// `Deprecated` | `Retired`").
fn capitalized(state: &str) -> String {
    let mut characters = state.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// The add-ons a version whose branch supports `tiers` must report.
fn expected_addons(tiers: &[String]) -> Vec<String> {
    let mut addons: Vec<String> = ALWAYS_SUPPORTED_ADDONS
        .iter()
        .map(|addon| (*addon).to_string())
        .collect();
    if tiers.iter().any(|tier| tier == WORKER_TIER) {
        addons.push(WORKER_ADDON.to_string());
    }
    addons.sort();
    addons
}

/// The branch record the store holds for `branch_name`, as
/// `(LifecycleState, SupportedTierList)` — the one place a version's branch
/// attributes are held, so the expectation for them is read from there rather
/// than written out per row.
fn branch_of(emulator: &Emulator, branch_name: &str) -> (String, Vec<String>) {
    let state = emulator.store();
    let store = state.lock();
    let branch = store
        .platform_branch_named(branch_name)
        .unwrap_or_else(|| panic!("the catalog holds the branch {branch_name:?}"));
    (branch.lifecycle_state.clone(), branch.supported_tiers.clone())
}

/// The ONE summary `ListPlatformVersions` reports for the version
/// `platform_version` of the platform branch `branch_name`, as its own XML.
///
/// Asked for with the two filters the operation models over those attributes, so
/// a branch publishing several versions still answers with exactly the one this
/// names.
async fn summary_of(
    emulator: &Emulator,
    branch_name: &str,
    platform_version: &str,
    case: &str,
) -> Answer {
    let answer = emulator
        .call(
            "ListPlatformVersions",
            &[
                ("Filters.member.1.Type", "PlatformBranchName"),
                ("Filters.member.1.Operator", "="),
                ("Filters.member.1.Values.member.1", branch_name),
                ("Filters.member.2.Type", "PlatformVersion"),
                ("Filters.member.2.Operator", "="),
                ("Filters.member.2.Values.member.1", platform_version),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from ListPlatformVersions, got {} and body {}",
        answer.status, answer.body
    );
    let mut members = answer.elements_at("PlatformSummaryList/member");
    assert_eq!(
        members.len(),
        1,
        "case {case:?}: a branch publishes one version under a version number, got {} in {}",
        members.len(),
        answer.body
    );
    members.remove(0)
}

/// Every version the catalog publishes, as `(the branch's catalog row, that version)`:
/// each branch's current release and the one it superseded
/// (`SolutionStack::published_versions`), flattened so every published version is one row
/// of the tables below.
fn published_catalog() -> Vec<(&'static eb::state::SolutionStack, String)> {
    eb::state::SOLUTION_STACKS
        .iter()
        .flat_map(|stack| {
            stack
                .published_versions()
                .into_iter()
                .map(move |platform_version| (stack, platform_version))
        })
        .collect()
}

/// The `PlatformLifecycleState` the version `platform_version` of `stack` must
/// report: only a branch's NEWEST release is its recommended one, and the
/// release it superseded carries no state at all.
fn expected_lifecycle_state(
    stack: &eb::state::SolutionStack,
    platform_version: &str,
) -> Option<&'static str> {
    (platform_version == stack.platform_version).then_some(RECOMMENDED)
}

/// The `PlatformDescription` `DescribePlatformVersion` reports for `arn`, as its
/// own XML.
async fn description_of(emulator: &Emulator, arn: &str, case: &str) -> Answer {
    let answer = emulator
        .call("DescribePlatformVersion", &[("PlatformArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from DescribePlatformVersion, got {} and body {}",
        answer.status, answer.body
    );
    let mut described = answer.elements_at("PlatformDescription");
    assert_eq!(
        described.len(),
        1,
        "case {case:?}: expected one PlatformDescription, got {}",
        answer.body
    );
    described.remove(0)
}

/// The `<member>` values of the list `wrapper` inside `answer`.
fn list(answer: &Answer, wrapper: &str) -> Vec<String> {
    answer
        .values_at(&format!("{wrapper}/member"))
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The `(Name, Version)` pairs of the `wrapper` list inside `answer` — how both
/// `ProgrammingLanguages` and `Frameworks` are modelled.
fn named_versions(answer: &Answer, wrapper: &str) -> Vec<(String, Option<String>)> {
    answer
        .elements_at(&format!("{wrapper}/member"))
        .into_iter()
        .map(|member| {
            (
                member.value("Name").unwrap_or_default().to_string(),
                member.value("Version").map(str::to_string),
            )
        })
        .collect()
}

/// A catalog row's `(Name, Version)` pairs as the response must carry them: a
/// row that states no version for a name reports the name without one.
fn catalog_pairs(pairs: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
    pairs
        .iter()
        .map(|(name, version)| {
            (
                (*name).to_string(),
                (!version.is_empty()).then(|| (*version).to_string()),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_published_version_summarises_its_catalog_record() {
    let emulator = Emulator::new();

    for (stack, platform_version) in published_catalog() {
        let case = &format!("{} v{platform_version}", stack.branch_name);
        let (branch_state, tiers) = branch_of(&emulator, stack.branch_name);
        let summary = summary_of(&emulator, stack.branch_name, &platform_version, case).await;
        let (os_name, os_version) = operating_system(stack.branch_name);

        assert_eq!(
            summary.value("PlatformArn"),
            Some(
                eb::state::platform_arn_of(TEST_REGION, stack.branch_name, &platform_version)
                    .as_str()
            ),
            "case {case:?}: the summary must name this catalog version, got {}",
            summary.body
        );
        assert_eq!(
            summary.value("PlatformCategory"),
            Some(stack.platform_category),
            "case {case:?}: expected the catalog's PlatformCategory {:?}, got {}",
            stack.platform_category,
            summary.body
        );
        assert_eq!(
            summary.value("OperatingSystemName"),
            Some(os_name.as_str()),
            "case {case:?}: expected OperatingSystemName {os_name:?}, got {}",
            summary.body
        );
        assert_eq!(
            summary.value("OperatingSystemVersion").map(str::to_string),
            os_version,
            "case {case:?}: expected OperatingSystemVersion {os_version:?}, got {}",
            summary.body
        );
        assert_eq!(
            list(&summary, "SupportedAddonList"),
            expected_addons(&tiers),
            "case {case:?}: expected the add-ons the branch's tiers {tiers:?} imply, got {}",
            summary.body
        );
        assert_eq!(
            list(&summary, "SupportedTierList"),
            tiers,
            "case {case:?}: a version runs exactly the tiers its branch supports, got {}",
            summary.body
        );
        assert_eq!(
            summary.value("PlatformLifecycleState"),
            expected_lifecycle_state(stack, &platform_version),
            "case {case:?}: only the newest version of a branch is its recommended one, got {}",
            summary.body
        );
        assert_eq!(
            summary.value("PlatformBranchLifecycleState").map(str::to_string),
            Some(capitalized(&branch_state)),
            "case {case:?}: expected the branch's own state {branch_state:?}, capitalized, got {}",
            summary.body
        );
        assert_eq!(
            summary.value("PlatformBranchName"),
            Some(stack.branch_name),
            "case {case:?}: expected the branch the version belongs to, got {}",
            summary.body
        );
        assert_eq!(
            summary.value("PlatformVersion"),
            Some(platform_version.as_str()),
            "case {case:?}: expected the catalog's PlatformVersion, got {}",
            summary.body
        );
    }
}

#[tokio::test]
async fn every_published_version_is_described_in_full_from_the_same_record() {
    let emulator = Emulator::new();

    for (stack, platform_version) in published_catalog() {
        let case = &format!("{} v{platform_version}", stack.branch_name);
        let (branch_state, tiers) = branch_of(&emulator, stack.branch_name);
        let arn = eb::state::platform_arn_of(TEST_REGION, stack.branch_name, &platform_version);
        let described = description_of(&emulator, &arn, case).await;
        let (os_name, os_version) = operating_system(stack.branch_name);

        // The members a summary carries too, from the one record.
        assert_eq!(
            described.value("PlatformCategory"),
            Some(stack.platform_category),
            "case {case:?}: expected the catalog's PlatformCategory, got {}",
            described.body
        );
        assert_eq!(
            described.value("OperatingSystemName"),
            Some(os_name.as_str()),
            "case {case:?}: expected OperatingSystemName {os_name:?}, got {}",
            described.body
        );
        assert_eq!(
            described.value("OperatingSystemVersion").map(str::to_string),
            os_version,
            "case {case:?}: expected OperatingSystemVersion {os_version:?}, got {}",
            described.body
        );
        assert_eq!(
            list(&described, "SupportedAddonList"),
            expected_addons(&tiers),
            "case {case:?}: expected the add-ons the branch's tiers {tiers:?} imply, got {}",
            described.body
        );
        assert_eq!(
            list(&described, "SupportedTierList"),
            tiers,
            "case {case:?}: a version runs exactly the tiers its branch supports, got {}",
            described.body
        );
        assert_eq!(
            described.value("PlatformLifecycleState"),
            expected_lifecycle_state(stack, &platform_version),
            "case {case:?}: only the newest version of a branch is its recommended one, got {}",
            described.body
        );
        assert_eq!(
            described
                .value("PlatformBranchLifecycleState")
                .map(str::to_string),
            Some(capitalized(&branch_state)),
            "case {case:?}: expected the branch's own state {branch_state:?}, capitalized, got {}",
            described.body
        );

        // And the detail only the description models.
        assert_eq!(
            described.value("SolutionStackName").map(str::to_string),
            Some(eb::state::solution_stack_name_of(
                stack.branch_name,
                &platform_version
            )),
            "case {case:?}: expected the solution stack name of THIS version, got {}",
            described.body
        );
        assert_eq!(
            described.value("Description").map(str::to_string),
            Some(description(stack.branch_name)),
            "case {case:?}: expected the solution stack without its version component, got {}",
            described.body
        );
        assert_eq!(
            described.value("Maintainer"),
            Some(eb::state::MANAGED_PLATFORM_MAINTAINER),
            "case {case:?}: a service-published version is maintained by the service, got {}",
            described.body
        );
        assert_eq!(
            named_versions(&described, "ProgrammingLanguages"),
            catalog_pairs(stack.programming_languages),
            "case {case:?}: expected the languages the catalog states, got {}",
            described.body
        );
        // A version Elastic Beanstalk itself publishes states no framework of
        // its own: the ONE entry it reports carries a `Name` and a `Version`
        // that are both PRESENT and EMPTY. Read off the XML, because the
        // element readers cannot tell an empty element from an absent one — the
        // same expectation [`crate::managed_platform_catalog_rules`] states, so
        // a change that drops either element fails in both suites.
        let frameworks = elements(&described.body, "Frameworks");
        assert_eq!(
            frameworks.len(),
            1,
            "case {case:?}: expected one Frameworks member, got {}",
            described.body
        );
        let framework_members = elements(frameworks[0], "member");
        assert_eq!(
            framework_members.len(),
            1,
            "case {case:?}: expected one framework entry, got {:?}",
            frameworks[0]
        );
        for member in &framework_members {
            assert_eq!(
                (elements(member, "Name"), elements(member, "Version")),
                (vec![""], vec![""]),
                "case {case:?}: a managed version states no framework, so its entry must report \
                 an empty Name and an empty Version, got {member:?}"
            );
        }
        // One entry per virtualization type, each naming its type. WHICH entry
        // names a machine image is the rule
        // [`crate::published_platform_catalog`] covers; what this suite asserts
        // is that the list the record projects carries both types, in order.
        let amis: Vec<(String, Option<String>)> = described
            .elements_at("CustomAmiList/member")
            .into_iter()
            .map(|member| {
                (
                    member
                        .value("VirtualizationType")
                        .unwrap_or_default()
                        .to_string(),
                    member.value("ImageId").map(str::to_string),
                )
            })
            .collect();
        assert_eq!(
            amis.iter()
                .map(|(virtualization_type, _)| virtualization_type.as_str())
                .collect::<Vec<&str>>(),
            // The two EC2 virtualization types, spelled out here rather than
            // read from the service, so this assertion stands on its own.
            vec!["pv", "hvm"],
            "case {case:?}: expected one entry per virtualization type, got {}",
            described.body
        );
        // And every entry reports an `ImageId` element, read off the XML for the
        // same reason the framework entry above is: present, with or without a
        // value.
        let ami_members: Vec<&str> = elements(&described.body, "CustomAmiList")
            .iter()
            .flat_map(|list| elements(list, "member"))
            .collect();
        for member in &ami_members {
            assert_eq!(
                elements(member, "ImageId").len(),
                1,
                "case {case:?}: the {:?} entry of a managed version must report an ImageId, got \
                 {member:?}",
                elements(member, "VirtualizationType")
            );
        }
        // The publication instants: both present, both the same instant, since
        // the catalog has not been re-published since it was seeded.
        let created = described.expect("DateCreated", case);
        let updated = described.expect("DateUpdated", case);
        assert_eq!(
            instant(created, case),
            instant(updated, case),
            "case {case:?}: a catalog version was published once, got {created:?} and {updated:?}"
        );
    }
}

#[tokio::test]
async fn the_two_calls_report_one_record_and_reading_it_changes_nothing() {
    let emulator = Emulator::new();
    // The rows the issue did not cite: a Windows platform (one tier, a
    // framework, a language with no version) and a Docker platform (no language
    // at all), plus the first row of the catalog.
    let rows = [
        "IIS 10.0 running on 64bit Windows Server 2019",
        "Docker running on 64bit Amazon Linux 2023",
        eb::state::SOLUTION_STACKS
            .first()
            .expect("the catalog publishes at least one version")
            .branch_name,
    ];

    for (stack, platform_version) in published_catalog()
        .into_iter()
        .filter(|(stack, _)| rows.contains(&stack.branch_name))
    {
        let branch_name = stack.branch_name;
        let case = &format!("{branch_name} v{platform_version}");
        let arn = eb::state::platform_arn_of(TEST_REGION, branch_name, &platform_version);

        let summary = summary_of(&emulator, branch_name, &platform_version, case).await;
        let first = description_of(&emulator, &arn, case).await;
        // Every member both shapes model must carry the same value in both.
        for member in [
            "PlatformArn",
            "PlatformOwner",
            "PlatformStatus",
            "PlatformCategory",
            "OperatingSystemName",
            "OperatingSystemVersion",
            "PlatformLifecycleState",
            "PlatformVersion",
            "PlatformBranchName",
            "PlatformBranchLifecycleState",
        ] {
            assert_eq!(
                summary.value(member),
                first.value(member),
                "case {case:?}: ListPlatformVersions and DescribePlatformVersion must report one \
                 record, they disagree on {member}: {:?} against {:?}",
                summary.value(member),
                first.value(member)
            );
        }
        for wrapper in ["SupportedTierList", "SupportedAddonList"] {
            assert_eq!(
                list(&summary, wrapper),
                list(&first, wrapper),
                "case {case:?}: the two calls must report one {wrapper}"
            );
        }

        // A read reports; it does not move what a later caller sees.
        let second = description_of(&emulator, &arn, case).await;
        assert_eq!(
            second.body, first.body,
            "case {case:?}: describing a published version twice must answer the same record"
        );
    }
}

/// One filter term over a member this change projects, and the rule it selects
/// by — stated over the catalog row, the tiers its branch supports and which of
/// the branch's published versions the row is, rather than over a recorded
/// answer.
struct FilterCase {
    case: &'static str,
    attribute: &'static str,
    value: &'static str,
    holds: fn(&eb::state::SolutionStack, &[String], &str) -> bool,
}

#[tokio::test]
async fn a_filter_term_on_a_projected_member_selects_by_the_catalog() {
    let emulator = Emulator::new();

    let cases: &[FilterCase] = &[
        // The operating system, on both of the two the catalog publishes.
        FilterCase {
            case: "OperatingSystemName = Amazon Linux",
            attribute: "OperatingSystemName",
            value: "Amazon Linux",
            holds: |stack, _tiers, _version| {
                operating_system(stack.branch_name).0 == "Amazon Linux"
            },
        },
        FilterCase {
            case: "OperatingSystemName = Windows Server",
            attribute: "OperatingSystemName",
            value: "Windows Server",
            holds: |stack, _tiers, _version| {
                operating_system(stack.branch_name).0 == "Windows Server"
            },
        },
        FilterCase {
            case: "OperatingSystemName = an operating system nothing runs on",
            attribute: "OperatingSystemName",
            value: "Plan 9",
            holds: |_stack, _tiers, _version| false,
        },
        // The add-ons: one every published version carries, and the worker
        // daemon, which only a branch supporting the worker tier does.
        FilterCase {
            case: "SupportedAddon = Log/CloudWatch",
            attribute: "SupportedAddon",
            value: "Log/CloudWatch",
            holds: |_stack, _tiers, _version| true,
        },
        FilterCase {
            case: "SupportedAddon = WorkerDaemon/SQSD",
            attribute: "SupportedAddon",
            value: WORKER_ADDON,
            holds: |_stack, tiers, _version| tiers.iter().any(|tier| tier == WORKER_TIER),
        },
        // The languages, which only the catalog states: one language it
        // publishes, and one it does not.
        FilterCase {
            case: "ProgrammingLanguageName = Java",
            attribute: "ProgrammingLanguageName",
            value: "Java",
            holds: |stack, _tiers, _version| {
                stack
                    .programming_languages
                    .iter()
                    .any(|(name, _)| *name == "Java")
            },
        },
        FilterCase {
            case: "ProgrammingLanguageName = Node.js",
            attribute: "ProgrammingLanguageName",
            value: "Node.js",
            holds: |stack, _tiers, _version| {
                stack
                    .programming_languages
                    .iter()
                    .any(|(name, _)| *name == "Node.js")
            },
        },
        FilterCase {
            case: "ProgrammingLanguageName = a language no platform runs",
            attribute: "ProgrammingLanguageName",
            value: "COBOL",
            holds: |_stack, _tiers, _version| false,
        },
        // The version life-cycle state, in the spelling the response carries and
        // in the lower-case one `PlatformFilter$Values` documents: a branch's
        // NEWEST published version is its recommended one, and the release that
        // one superseded is not.
        FilterCase {
            case: "PlatformLifecycleState = Recommended",
            attribute: "PlatformLifecycleState",
            value: "Recommended",
            holds: |stack, _tiers, version| version == stack.platform_version,
        },
        FilterCase {
            case: "PlatformLifecycleState = recommended",
            attribute: "PlatformLifecycleState",
            value: "recommended",
            holds: |stack, _tiers, version| version == stack.platform_version,
        },
    ];

    for FilterCase {
        case,
        attribute,
        value,
        holds,
    } in cases
    {
        let answer = emulator
            .call(
                "ListPlatformVersions",
                &[
                    ("Filters.member.1.Type", attribute),
                    ("Filters.member.1.Operator", "="),
                    ("Filters.member.1.Values.member.1", value),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} and body {}",
            answer.status, answer.body
        );
        let selected: Vec<String> = answer
            .values_at("PlatformSummaryList/member/PlatformArn")
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut expected: Vec<String> = eb::state::SOLUTION_STACKS
            .iter()
            .flat_map(|stack| {
                let (_, tiers) = branch_of(&emulator, stack.branch_name);
                stack
                    .published_versions()
                    .into_iter()
                    .filter(move |version| holds(stack, &tiers, version))
                    .map(|version| {
                        eb::state::platform_arn_of(TEST_REGION, stack.branch_name, &version)
                    })
                    .collect::<Vec<String>>()
            })
            .collect();
        expected.sort();
        let mut selected_sorted = selected.clone();
        selected_sorted.sort();
        assert_eq!(
            selected_sorted, expected,
            "case {case:?}: the term must select exactly the catalog versions it holds for, got \
             {selected:?} and body {}",
            answer.body
        );
    }
}
