// SPDX-License-Identifier: CC0-1.0

//! A `SolutionStackName` is validated against the service's PUBLISHED catalogue,
//! driven end to end over this service's awsQuery surface.
//!
//! RULE under test: an operation whose request names the platform version to run
//! or configure by `SolutionStackName` refuses a name this service's solution
//! stack catalogue does not publish — the catalogue
//! `ListAvailableSolutionStacks` reports — with `InvalidParameterValue`, HTTP 400
//! and the sentence `No Solution Stack named '<name>' found.`, and records
//! nothing for it; a name the catalogue DOES publish is accepted. The four
//! operations taking the member (`CreateEnvironment`,
//! `CreateConfigurationTemplate`, `UpdateEnvironment` and
//! `DescribeConfigurationOptions` — the ones whose modelled input carries it)
//! therefore answer alike, so a platform typo cannot become a live environment,
//! a saved configuration or a published option catalogue.
//!
//! The rows span what the rule generalises over: WHICH operation takes the name,
//! and HOW the name misses the catalogue — a name nothing publishes at all, a
//! published platform branch at a version the catalogue does not offer, a
//! published name in another case, the platform BRANCH's own name rather than
//! the stack's, and a published name carrying trailing whitespace. Every
//! unpublished name is DERIVED from the catalogue the service publishes (except
//! the wholly fictional one), so no row can quietly become a published name.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns:
//! nothing here reaches into the store, so what is asserted is what a client
//! sees.

use crate::harness::{Answer, Emulator, PAST_DUE, SOLUTION_STACK};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every request below names.
const APPLICATION: &str = "stack-validation-app";
/// The environment `UpdateEnvironment` is driven against, launched on a
/// published stack and waited into `Ready`.
const ENVIRONMENT: &str = "stack-valid-env";

/// One operation that takes a `SolutionStackName`: the action, and the other
/// parameters a well-formed request to it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    CreateEnvironment,
    CreateConfigurationTemplate,
    UpdateEnvironment,
    DescribeConfigurationOptions,
}

impl Target {
    fn action(self) -> &'static str {
        match self {
            Self::CreateEnvironment => "CreateEnvironment",
            Self::CreateConfigurationTemplate => "CreateConfigurationTemplate",
            Self::UpdateEnvironment => "UpdateEnvironment",
            Self::DescribeConfigurationOptions => "DescribeConfigurationOptions",
        }
    }

    /// The request this operation is driven with, naming `solution_stack` and
    /// `resource` (the environment or template name a create needs).
    fn params(self, solution_stack: &str, resource: &str) -> Vec<(String, String)> {
        let stack = ("SolutionStackName".to_string(), solution_stack.to_string());
        match self {
            Self::CreateEnvironment => vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), resource.to_string()),
                stack,
            ],
            Self::CreateConfigurationTemplate => vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), resource.to_string()),
                stack,
            ],
            Self::UpdateEnvironment => vec![
                ("EnvironmentName".to_string(), ENVIRONMENT.to_string()),
                stack,
            ],
            Self::DescribeConfigurationOptions => vec![stack],
        }
    }
}

/// The operations the rule covers.
const TARGETS: &[Target] = &[
    Target::CreateEnvironment,
    Target::CreateConfigurationTemplate,
    Target::UpdateEnvironment,
    Target::DescribeConfigurationOptions,
];

/// `params` as the borrowed pairs the harness sends.
fn as_params(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Require a 200 from a setup call.
async fn given(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// The application and the `Ready` environment every row is driven against.
async fn fixture(emulator: &Emulator) {
    given(
        emulator,
        "setup",
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    given(
        emulator,
        "setup",
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", ENVIRONMENT),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
        ],
    )
    .await;
    // The launch the clock owes, so `UpdateEnvironment` is driven against an
    // environment in the one status that accepts an update.
    std::thread::sleep(PAST_DUE);
}

/// One name that misses the catalogue: how it misses it, and the name itself,
/// derived from a stack the service really publishes.
struct UnpublishedName {
    case: String,
    name: String,
}

/// The names under test, each a different way of missing the published
/// catalogue.
fn unpublished_names() -> Vec<UnpublishedName> {
    let published = eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack");
    let name = published.name();
    vec![
        UnpublishedName {
            case: "a stack name nothing publishes".to_string(),
            name: "64bit Fictional Linux v0.0.1 running Nothing".to_string(),
        },
        UnpublishedName {
            // The branch is published; this version of it is not.
            case: "a published branch at a version the catalogue does not offer".to_string(),
            name: eb::state::solution_stack_name_of(published.branch_name, "0.0.0"),
        },
        UnpublishedName {
            case: "a published stack name upper-cased".to_string(),
            name: name.to_uppercase(),
        },
        UnpublishedName {
            // `ListPlatformBranches` reports a branch as `<runtime> running on
            // <os>`; that is not the name a solution stack is published under.
            case: "the platform branch's name rather than the stack's".to_string(),
            name: published.branch_name.to_string(),
        },
        UnpublishedName {
            case: "a published stack name with trailing whitespace".to_string(),
            name: format!("{name} "),
        },
    ]
}

#[tokio::test]
async fn an_unpublished_solution_stack_name_is_refused_by_every_operation_that_takes_one() {
    let emulator = Emulator::new();
    fixture(&emulator).await;

    for (index, unpublished) in unpublished_names().iter().enumerate() {
        for target in TARGETS {
            let case = format!("{} on {}", unpublished.case, target.action());
            let resource = format!("unpublished-{index}");
            let refused = emulator
                .call(
                    target.action(),
                    &as_params(&target.params(&unpublished.name, &resource)),
                )
                .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400, got {} and body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "InvalidParameterValue",
                "case {case:?}: unexpected error code, got body {}",
                refused.body
            );
            assert_eq!(
                refused.error_message(),
                eb::state::missing_solution_stack_message(&unpublished.name),
                "case {case:?}: the refusal names the stack the request asked for, got body {}",
                refused.body
            );
        }
    }

    // Nothing the refusals named was recorded: no environment, no configuration
    // template, and the environment `UpdateEnvironment` was refused for still
    // runs the platform it was launched on.
    let environments = given(
        &emulator,
        "nothing recorded",
        "DescribeEnvironments",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let mut names = environments.values_at("Environments/member/EnvironmentName");
    names.sort_unstable();
    assert_eq!(
        names,
        vec![ENVIRONMENT],
        "case \"nothing recorded\": only the fixture environment may exist, got body {}",
        environments.body
    );
    assert_eq!(
        environments.value("Environments/member/SolutionStackName"),
        Some(SOLUTION_STACK.as_str()),
        "case \"nothing recorded\": a refused update must not move the platform, got body {}",
        environments.body
    );
    let applications = given(
        &emulator,
        "nothing recorded",
        "DescribeApplications",
        &[("ApplicationNames.member.1", APPLICATION)],
    )
    .await;
    assert_eq!(
        applications.values_at("Applications/member/ConfigurationTemplates/member"),
        Vec::<&str>::new(),
        "case \"nothing recorded\": no configuration template may exist, got body {}",
        applications.body
    );
}

#[tokio::test]
async fn a_published_solution_stack_name_is_accepted_by_every_operation_that_takes_one() {
    let emulator = Emulator::new();
    fixture(&emulator).await;

    // Two published stacks of different platform families, so the acceptance is
    // not keyed to one catalogue row.
    let first = eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack");
    let other = eb::state::SOLUTION_STACKS
        .iter()
        .find(|stack| stack.platform_category != first.platform_category)
        .expect("the service publishes more than one platform category");

    for (index, stack) in [first, other].into_iter().enumerate() {
        let name = stack.name();
        for target in TARGETS {
            let case = format!("{name} on {}", target.action());
            let resource = format!("published-{index}");
            let accepted = given(
                &emulator,
                &case,
                target.action(),
                &as_params(&target.params(&name, &resource)),
            )
            .await;
            // The answer names the version the request named, and the catalogue
            // resolves its other name for it.
            assert_eq!(
                accepted.value("SolutionStackName"),
                Some(name.as_str()),
                "case {case:?}: the answer must name the stack the request named, got body {}",
                accepted.body
            );
            assert_eq!(
                accepted.value("PlatformArn"),
                Some(stack.platform_arn(crate::lifecycle::TEST_REGION).as_str()),
                "case {case:?}: the published name must resolve to its platform ARN, got body {}",
                accepted.body
            );
            if *target == Target::UpdateEnvironment {
                // The update the clock owes, so the next row's update is not
                // refused by a status this one left behind.
                std::thread::sleep(PAST_DUE);
            }
        }
    }
}
