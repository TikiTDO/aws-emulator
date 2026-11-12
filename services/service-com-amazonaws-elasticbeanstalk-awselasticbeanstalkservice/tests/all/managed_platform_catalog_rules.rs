// SPDX-License-Identifier: CC0-1.0

//! The rules the AWS-MANAGED platform catalog is published under, driven end to
//! end over the service's awsQuery HTTP surface.
//!
//! Three rules, one per divergence this suite was written for:
//!
//! 1. `ListPlatformBranches` reports the platform-branch inventory Elastic
//!    Beanstalk publishes, which includes a platform whose branch is not a
//!    runtime an application is deployed on at all — the custom-platform
//!    builder — and such a branch reports its `SupportedTierList` as an EMPTY
//!    list, because no environment of either tier runs on it.
//! 2. A platform version number belongs to a platform-family RELEASE, not to a
//!    branch: Elastic Beanstalk re-publishes one platform category on one
//!    operating system generation at a time, so every branch of that family
//!    reports the same `PlatformVersion`, and only each branch's newest
//!    published release carries `PlatformLifecycleState` `Recommended`
//!    (`PlatformDescription$PlatformLifecycleState`: "Possible values:
//!    `Recommended` | `null` … Each platform branch has a single recommended
//!    platform version").
//! 3. A managed platform version reports no detail its catalog entry does not
//!    define: `Frameworks` describes the platform DEFINITION its owner built the
//!    version from, which a version the service itself publishes has none of —
//!    so its one framework entry states neither a name nor a version — while
//!    each `CustomAmiList` entry names its virtualization type and the image the
//!    version runs for that type, which is the service's own.
//!
//! Every row is read from the service's own published catalog
//! (`eb::state::SOLUTION_STACKS`, what `ListAvailableSolutionStacks` reports)
//! or spans a set of catalog entries written out here from Elastic Beanstalk's
//! published platform inventory; none is a transcription of a recorded
//! response.
//!
//! The HTTP harness ([`crate::lifecycle::Emulator`]) is the one the lifecycle
//! suite drives the service with.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{elements, Answer, Emulator, TEST_REGION};

/// `PlatformLifecycleState` of a version that is its branch's recommended one,
/// from `PlatformDescription$PlatformLifecycleState`.
const RECOMMENDED: &str = "Recommended";

/// The `CustomAmi$VirtualizationType` values a managed platform version reports
/// an entry for, spelled out here rather than read from the service: the two EC2
/// virtualization types, in the order `DescribePlatformVersion` lists them.
const VIRTUALIZATION_TYPES: &[&str] = &["pv", "hvm"];

/// One branch of Elastic Beanstalk's published platform-branch inventory.
///
/// SOURCE: the "Elastic Beanstalk supported platforms" and "Retired platform
/// branches" documents, which is what the real `ListPlatformBranches`
/// enumerates. The rows span the shapes an entry takes: the custom-platform
/// BUILDER branch (supporting no environment tier), a current Linux runtime
/// branch of a platform the divergence report did not cite, a current Windows
/// branch (web server tier only) and a branch of a platform that has retired
/// whole.
struct BranchRow {
    platform_name: &'static str,
    branch_name: &'static str,
    /// The life-cycle state as the wire carries it, capitalized.
    lifecycle_state: &'static str,
    branch_order: i32,
    /// The `SupportedTierList` members, in the order the branch reports them.
    supported_tiers: &'static [&'static str],
}

const BRANCH_ROWS: &[BranchRow] = &[
    BranchRow {
        platform_name: "Packer",
        branch_name: "Elastic Beanstalk Packer Builder",
        lifecycle_state: "Supported",
        branch_order: 10000,
        supported_tiers: &[],
    },
    BranchRow {
        platform_name: "Go",
        branch_name: "Go 1 running on 64bit Amazon Linux 2023",
        lifecycle_state: "Supported",
        branch_order: 12000,
        supported_tiers: &["WebServer/Standard", "Worker/SQS/HTTP"],
    },
    BranchRow {
        platform_name: ".NET on Windows Server",
        branch_name: "IIS 10.0 running on 64bit Windows Server 2025",
        lifecycle_state: "Supported",
        branch_order: 10500,
        supported_tiers: &["WebServer/Standard"],
    },
    BranchRow {
        platform_name: "GlassFish",
        branch_name: "Preconfigured Docker - GlassFish 5.0 running on 64bit Amazon Linux 2",
        lifecycle_state: "Retired",
        branch_order: 10000,
        supported_tiers: &["WebServer/Standard"],
    },
];

/// One platform FAMILY: a platform category on one operating system generation,
/// and how many branches of it the catalog publishes at least.
///
/// SOURCE: the same published inventory [`BRANCH_ROWS`] comes from, read as the
/// `PlatformCategory` / `OperatingSystemName` / `OperatingSystemVersion` a
/// platform version reports. The rows span both operating systems, a family
/// whose branches differ only in runtime version, one whose branches differ in
/// servlet container, and two families the divergence report did not cite.
struct FamilyRow {
    category: &'static str,
    operating_system_name: &'static str,
    operating_system_version: &'static str,
    at_least_branches: usize,
}

const FAMILY_ROWS: &[FamilyRow] = &[
    FamilyRow {
        category: ".NET Core",
        operating_system_name: "Amazon Linux",
        operating_system_version: "2023",
        at_least_branches: 3,
    },
    FamilyRow {
        category: "Java",
        operating_system_name: "Amazon Linux",
        operating_system_version: "2023",
        at_least_branches: 5,
    },
    FamilyRow {
        category: "Tomcat",
        operating_system_name: "Amazon Linux",
        operating_system_version: "2023",
        at_least_branches: 4,
    },
    FamilyRow {
        category: "PHP",
        operating_system_name: "Amazon Linux",
        operating_system_version: "2023",
        at_least_branches: 4,
    },
    FamilyRow {
        category: "Python",
        operating_system_name: "Amazon Linux",
        operating_system_version: "2023",
        at_least_branches: 4,
    },
    FamilyRow {
        category: "Ruby",
        operating_system_name: "Amazon Linux",
        operating_system_version: "2023",
        at_least_branches: 3,
    },
    // The other operating system. Elastic Beanstalk publishes the Server and
    // the Server Core editions of one Windows generation as operating systems
    // of their own, so this family is one branch wide: what it pins is that a
    // Windows generation has a release line of its own.
    FamilyRow {
        category: ".NET",
        operating_system_name: "Windows Server",
        operating_system_version: "2022",
        at_least_branches: 1,
    },
];

/// The branch names [`a_managed_version_reports_no_definition_detail_of_its_own`]
/// describes, one per shape of catalog entry — a platform that ran its
/// application behind an application server (the Windows IIS, Tomcat, Ruby, PHP
/// and Python platforms, the slots a fabricated framework name used to appear
/// in), one that runs no language of its own, and one that states a language
/// with no framework.
const DESCRIBED_BRANCHES: &[&str] = &[
    "IIS 10.0 running on 64bit Windows Server 2022",
    "Tomcat 9 with Corretto 8 running on 64bit Amazon Linux 2023",
    "Ruby 3.3 running on 64bit Amazon Linux 2023",
    "PHP 8.2 running on 64bit Amazon Linux 2023",
    "Python 3.12 running on 64bit Amazon Linux 2023",
    "Docker running on 64bit Amazon Linux 2023",
    "Corretto 8 running on 64bit Amazon Linux 2023",
];

/// The catalog row for `branch_name`.
fn stack_of(branch_name: &str) -> &'static eb::state::SolutionStack {
    eb::state::SOLUTION_STACKS
        .iter()
        .find(|stack| stack.branch_name == branch_name)
        .unwrap_or_else(|| panic!("case {branch_name:?}: the catalog publishes this branch"))
}

/// One member of a `ListPlatformVersions` answer, as the members this suite
/// reads.
#[derive(Debug, Clone)]
struct Summary {
    branch_name: String,
    platform_version: String,
    category: String,
    operating_system_name: String,
    operating_system_version: String,
    lifecycle_state: Option<String>,
}

/// Every platform version the account can list, unfiltered.
async fn listed(emulator: &Emulator) -> Vec<Summary> {
    let answer = emulator.call("ListPlatformVersions", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case \"catalog\": expected 200 from ListPlatformVersions, got {} and body {}",
        answer.status, answer.body
    );
    let members = answer.elements_at("PlatformSummaryList/member");
    assert!(
        !members.is_empty(),
        "case \"catalog\": expected the published platform versions, got {}",
        answer.body
    );
    members
        .iter()
        .map(|member| Summary {
            branch_name: text(member, "PlatformBranchName"),
            platform_version: text(member, "PlatformVersion"),
            category: text(member, "PlatformCategory"),
            operating_system_name: text(member, "OperatingSystemName"),
            operating_system_version: text(member, "OperatingSystemVersion"),
            lifecycle_state: member.value("PlatformLifecycleState").map(str::to_string),
        })
        .collect()
}

/// The text of `member`'s `path` element, or the empty string when it carries
/// none.
fn text(member: &Answer, path: &str) -> String {
    member.value(path).unwrap_or_default().to_string()
}

/// The inner XML of the ONE `tag` element in `xml`, failing `case` when the
/// element is absent — so "present and empty" is told apart from "absent",
/// which the element readers cannot do on their own.
fn present(xml: &str, tag: &str, case: &str) -> String {
    let found = elements(xml, tag);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {tag} element, got {} in {xml}",
        found.len()
    );
    found[0].to_string()
}

#[tokio::test]
async fn the_branch_catalog_publishes_every_platform_the_service_publishes() {
    let emulator = Emulator::new();

    for row in BRANCH_ROWS {
        let case = row.branch_name;
        let answer = emulator
            .call(
                "ListPlatformBranches",
                &[
                    ("Filters.member.1.Attribute", "PlatformName"),
                    ("Filters.member.1.Operator", "="),
                    ("Filters.member.1.Values.member.1", row.platform_name),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from ListPlatformBranches, got {} and body {}",
            answer.status, answer.body
        );
        let member = answer
            .elements_at("PlatformBranchSummaryList/member")
            .into_iter()
            .find(|member| member.value("BranchName") == Some(row.branch_name))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the platform {:?} to publish this branch, got {}",
                    row.platform_name, answer.body
                )
            });
        assert_eq!(
            member.value("PlatformName"),
            Some(row.platform_name),
            "case {case:?}: expected the branch's own platform, got {}",
            member.body
        );
        assert_eq!(
            member.value("LifecycleState"),
            Some(row.lifecycle_state),
            "case {case:?}: unexpected LifecycleState, got {}",
            member.body
        );
        assert_eq!(
            member.value("BranchOrder"),
            Some(row.branch_order.to_string().as_str()),
            "case {case:?}: unexpected BranchOrder, got {}",
            member.body
        );
        // The tier list is REPORTED whether or not it holds anything: a branch
        // no environment runs on reports it empty rather than leaving it out.
        let tiers = present(&member.body, "SupportedTierList", case);
        assert_eq!(
            elements(&tiers, "member"),
            row.supported_tiers.to_vec(),
            "case {case:?}: unexpected SupportedTierList, got {tiers:?}"
        );
    }
}

#[tokio::test]
async fn one_release_number_is_published_across_a_platform_familys_branches() {
    let emulator = Emulator::new();
    let catalog = listed(&emulator).await;

    // The recommended release of every branch, grouped by the family it belongs
    // to: one platform category on one operating system generation.
    let mut families: BTreeMap<(String, String, String), BTreeMap<String, String>> =
        BTreeMap::new();
    for summary in catalog
        .iter()
        .filter(|summary| summary.lifecycle_state.as_deref() == Some(RECOMMENDED))
    {
        families
            .entry((
                summary.category.clone(),
                summary.operating_system_name.clone(),
                summary.operating_system_version.clone(),
            ))
            .or_default()
            .insert(summary.branch_name.clone(), summary.platform_version.clone());
    }

    // Every family publishes ONE release number across its branches.
    for (family, branches) in &families {
        let case = &format!("{family:?}");
        let versions: BTreeSet<&String> = branches.values().collect();
        assert_eq!(
            versions.len(),
            1,
            "case {case:?}: a family is re-published as a whole, so its branches must share one \
             PlatformVersion, got {branches:?}"
        );
    }

    // And the rows above are families with several branches each, so the
    // assertion is about a shared number rather than a single-branch family
    // where sharing is vacuous.
    for row in FAMILY_ROWS {
        let case = &format!(
            "{} on {} {}",
            row.category, row.operating_system_name, row.operating_system_version
        );
        let branches = families
            .get(&(
                row.category.to_string(),
                row.operating_system_name.to_string(),
                row.operating_system_version.to_string(),
            ))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the catalog to publish this family, got {:?}",
                    families.keys().collect::<Vec<_>>()
                )
            });
        assert!(
            branches.len() >= row.at_least_branches,
            "case {case:?}: expected at least {} branches of this family, got {branches:?}",
            row.at_least_branches
        );
        let versions: BTreeSet<&String> = branches.values().collect();
        assert_eq!(
            versions.len(),
            1,
            "case {case:?}: expected one release number across the family, got {branches:?}"
        );
    }
}

#[tokio::test]
async fn each_branch_marks_only_its_newest_published_release_recommended() {
    let emulator = Emulator::new();
    let catalog = listed(&emulator).await;

    // Every branch the catalog publishes a version of is one row, and within a
    // row every version that branch publishes (its family's current release and
    // the one that release superseded) is one case.
    for stack in eb::state::SOLUTION_STACKS {
        let case = stack.branch_name;
        let published = stack.published_versions();
        let mine: Vec<&Summary> = catalog
            .iter()
            .filter(|summary| summary.branch_name == stack.branch_name)
            .collect();
        assert_eq!(
            mine.iter()
                .map(|summary| summary.platform_version.clone())
                .collect::<BTreeSet<String>>(),
            published.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: expected exactly the versions this branch publishes, got {mine:?}"
        );
        let newest = published
            .first()
            .expect("a published branch publishes at least one version");
        for summary in &mine {
            let expected = (&summary.platform_version == newest).then_some(RECOMMENDED);
            assert_eq!(
                summary.lifecycle_state.as_deref(),
                expected,
                "case {case:?} v{}: only the newest release of a branch is its recommended one, \
                 got {:?}",
                summary.platform_version,
                summary.lifecycle_state
            );
        }
    }
}

#[tokio::test]
async fn a_managed_version_reports_no_definition_detail_of_its_own() {
    let emulator = Emulator::new();

    for branch_name in DESCRIBED_BRANCHES {
        let stack = stack_of(branch_name);
        // Both versions the branch publishes, so a superseded release is held to
        // the same rule as its branch's recommended one.
        for platform_version in stack.published_versions() {
            let case = &format!("{branch_name} v{platform_version}");
            let arn = eb::state::platform_arn_of(TEST_REGION, branch_name, &platform_version);
            let answer = emulator
                .call("DescribePlatformVersion", &[("PlatformArn", arn.as_str())])
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from DescribePlatformVersion, got {} and body {}",
                answer.status, answer.body
            );
            let described = present(&answer.body, "PlatformDescription", case);

            // The frameworks a platform version ships are stated by the
            // definition it was built from; a managed version states none, so
            // the ONE entry it reports carries neither a name nor a version
            // rather than an invented application server.
            let frameworks = present(&described, "Frameworks", case);
            let framework_members = elements(&frameworks, "member");
            assert_eq!(
                framework_members.len(),
                1,
                "case {case:?}: expected one framework entry, got {frameworks:?}"
            );
            for member in &framework_members {
                assert_eq!(
                    (
                        present(member, "Name", case),
                        present(member, "Version", case)
                    ),
                    (String::new(), String::new()),
                    "case {case:?}: a managed version states no framework, got {member:?}"
                );
            }

            // One custom-AMI entry per virtualization type, each naming its
            // type. Which entry names an image is
            // [`crate::published_platform_catalog`]' rule; here the shape of the
            // list is what the "no invented detail" rule needs.
            let amis = present(&described, "CustomAmiList", case);
            let members = elements(&amis, "member");
            assert_eq!(
                members
                    .iter()
                    .map(|member| present(member, "VirtualizationType", case))
                    .collect::<Vec<String>>(),
                VIRTUALIZATION_TYPES
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected one entry per virtualization type, got {amis:?}"
            );

            // What the CATALOG does state is still reported, so the rule above
            // is "no invented detail", not "no detail".
            let languages = present(&described, "ProgrammingLanguages", case);
            assert_eq!(
                elements(&languages, "member")
                    .iter()
                    .map(|member| present(member, "Name", case))
                    .collect::<Vec<String>>(),
                stack
                    .programming_languages
                    .iter()
                    .map(|(name, _)| (*name).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected the languages the catalog states, got {languages:?}"
            );
        }
    }
}

#[tokio::test]
async fn describing_a_managed_version_twice_reports_the_same_record() {
    let emulator = Emulator::new();
    let stack = stack_of("Python 3.12 running on 64bit Amazon Linux 2023");
    let arn = eb::state::platform_arn_of(TEST_REGION, stack.branch_name, stack.platform_version);
    let case = "a repeated describe";

    let first = emulator
        .call("DescribePlatformVersion", &[("PlatformArn", arn.as_str())])
        .await;
    let second = emulator
        .call("DescribePlatformVersion", &[("PlatformArn", arn.as_str())])
        .await;
    assert_eq!(
        present(&first.body, "PlatformDescription", case),
        present(&second.body, "PlatformDescription", case),
        "case {case:?}: a read must not change what a later caller sees, got {} then {}",
        first.body,
        second.body
    );
}
