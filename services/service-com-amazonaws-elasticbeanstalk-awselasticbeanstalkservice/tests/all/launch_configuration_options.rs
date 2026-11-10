// SPDX-License-Identifier: CC0-1.0

//! The launch-configuration options `DescribeConfigurationOptions` publishes for
//! a platform version, and who DEFINED each published option, driven end to end
//! over this service's awsQuery surface.
//!
//! RULES under test:
//!
//! * every configuration option this service HONOURS on an environment is
//!   published in the catalogue of the platform versions that honour it — a
//!   client enumerates this catalogue to decide what it may set, so an option it
//!   can legally set on an environment, and that the environment's own reports
//!   are derived from, is described exactly once, with a `ValueType` from the
//!   modelled `ConfigurationOptionValueType` set and a `ChangeSeverity` from the
//!   three the member documents. `aws:autoscaling:launchconfiguration` /
//!   `InstanceType` is such an option: the single-valued form of the instance
//!   type an environment runs;
//! * `ConfigurationOptionDescription$UserDefined` is "true: this configuration
//!   option was defined by the user … false: this configuration was not defined
//!   by the user", and every option the catalogue publishes is one the PLATFORM
//!   VERSION defines, so each is published with `UserDefined=false` — the flag a
//!   client reads to decide what it may pass in `OptionsToRemove` ("you can
//!   remove only `UserDefined` options from a configuration").
//!
//! The rows span what the rules generalise over: the namespace an option
//! configures and the kind of value it takes (a selection, a boolean, a numeric
//! range, a free-form string), the platform family the version belongs to, and
//! the two ways a request names the platform version whose catalogue it wants (a
//! solution stack name and a platform version ARN). The published `DefaultValue`
//! of the instance type is asserted against the type the environment's own
//! instances report, so the catalogue cannot publish a default this service does
//! not honour.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns and
//! the option reader is `all/configuration_options.rs`'s, so no second copy of
//! either can drift.

use crate::configuration_options::{reported_options, ReportedOption};
use crate::harness::{Answer, Emulator, PAST_DUE};
use crate::lifecycle::TEST_REGION;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application the environment below belongs to.
const APPLICATION: &str = "launch-config-app";

/// The namespace that carries the instances' own options, and the option in it
/// that names the single instance type an environment runs.
const NAMESPACE_LAUNCH_CONFIGURATION: &str = "aws:autoscaling:launchconfiguration";
/// See [`NAMESPACE_LAUNCH_CONFIGURATION`].
const OPTION_INSTANCE_TYPE: &str = "InstanceType";

/// The `ChangeSeverity` values the member documents, read from the service's own
/// constants so this suite cannot accept one the member does not document.
fn change_severities() -> Vec<&'static str> {
    vec![
        eb::state::CHANGE_SEVERITY_NO_INTERRUPTION,
        eb::state::CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        eb::state::CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
    ]
}

/// The `ValueType` values the model's `ConfigurationOptionValueType` declares.
fn value_types() -> Vec<&'static str> {
    vec![
        eb::state::OPTION_VALUE_TYPE_LIST,
        eb::state::OPTION_VALUE_TYPE_SCALAR,
    ]
}

/// Require a 200 from a call.
async fn ok(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// The option descriptions `DescribeConfigurationOptions` publishes for the
/// request `params`.
async fn published_options(
    emulator: &Emulator,
    case: &str,
    params: &[(&str, &str)],
) -> (Vec<ReportedOption>, Answer) {
    let answer = ok(emulator, case, "DescribeConfigurationOptions", params).await;
    (reported_options(&answer), answer)
}

/// The descriptions in `options` for one `(Namespace, OptionName)` pair.
fn selected<'a>(
    options: &'a [ReportedOption],
    namespace: &str,
    name: &str,
) -> Vec<&'a ReportedOption> {
    options
        .iter()
        .filter(|option| option.namespace == namespace && option.name == name)
        .collect()
}

/// One published option the rules are asserted over: the namespace and name
/// that identify it, and whether the catalogue publishes a `DefaultValue` for
/// it.
struct OptionRow {
    case: &'static str,
    namespace: &'static str,
    name: &'static str,
    /// The default the catalogue must publish, where the option has one.
    default_value: Option<String>,
}

/// The options every published platform version defines that these rules are
/// asserted over — one per kind of value an option takes, so the rules are not
/// asserted of one option alone.
fn option_rows() -> Vec<OptionRow> {
    vec![
        OptionRow {
            // The option the issue is about: a free-form string naming the
            // instance type, whose default is the type an environment that
            // names none runs.
            case: "the launch configuration's instance type",
            namespace: NAMESPACE_LAUNCH_CONFIGURATION,
            name: OPTION_INSTANCE_TYPE,
            default_value: Some(eb::state::DEFAULT_INSTANCE_TYPE.to_string()),
        },
        OptionRow {
            case: "the environment type, a selection the platform publishes",
            namespace: "aws:elasticbeanstalk:environment",
            name: "EnvironmentType",
            default_value: Some(eb::state::ENVIRONMENT_TYPE_LOAD_BALANCED.to_string()),
        },
        OptionRow {
            case: "the Auto Scaling minimum, a numeric option with a range",
            namespace: "aws:autoscaling:asg",
            name: "MinSize",
            default_value: Some(eb::state::DEFAULT_MIN_SIZE.to_string()),
        },
        OptionRow {
            case: "the instance metadata switch, a boolean",
            namespace: NAMESPACE_LAUNCH_CONFIGURATION,
            name: "DisableIMDSv1",
            default_value: Some(eb::state::BOOLEAN_FALSE.to_string()),
        },
        OptionRow {
            case: "the instances' security groups, a free-form string with no default",
            namespace: NAMESPACE_LAUNCH_CONFIGURATION,
            name: "SecurityGroups",
            default_value: None,
        },
    ]
}

#[tokio::test]
async fn every_honoured_option_is_published_once_typed_and_not_user_defined() {
    let emulator = Emulator::new();
    let severities = change_severities();
    let types = value_types();

    // Platform versions of two different families, each named BOTH ways a
    // request can name one, so neither rule is keyed to one catalogue row or to
    // one spelling of the platform version.
    let first = eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack");
    let other = eb::state::SOLUTION_STACKS
        .iter()
        .find(|stack| stack.platform_category != first.platform_category)
        .expect("the service publishes more than one platform category");

    for stack in [first, other] {
        let name = stack.name();
        let arn = stack.platform_arn(TEST_REGION);
        for (named_by, params) in [
            ("SolutionStackName", vec![("SolutionStackName", name.as_str())]),
            ("PlatformArn", vec![("PlatformArn", arn.as_str())]),
        ] {
            let case = format!("{name} named by {named_by}");
            let (options, answer) = published_options(&emulator, &case, &params).await;

            for row in option_rows() {
                let case = format!("{case}: {}", row.case);
                let described = selected(&options, row.namespace, row.name);
                assert_eq!(
                    described.len(),
                    1,
                    "case {case:?}: {}/{} must be described exactly once, got {} descriptions \
                     in body {}",
                    row.namespace,
                    row.name,
                    described.len(),
                    answer.body
                );
                let option = described[0];
                assert_eq!(
                    option.default_value, row.default_value,
                    "case {case:?}: unexpected DefaultValue, got {option:?}"
                );
                assert!(
                    types.contains(&option.value_type.as_str()),
                    "case {case:?}: ValueType must be a modelled value, got {option:?}"
                );
                assert!(
                    severities.contains(&option.change_severity.as_str()),
                    "case {case:?}: ChangeSeverity must be a documented value, got {option:?}"
                );
            }

            // RULE 2 holds of the WHOLE catalogue, not only of the rows above:
            // every option it publishes is one the platform version defines.
            for option in &options {
                assert_eq!(
                    option.user_defined.as_deref(),
                    Some("false"),
                    "case {case:?}: {}/{} is published by the platform, so it is not user \
                     defined, got {option:?}",
                    option.namespace,
                    option.name
                );
            }
        }
    }
}

#[tokio::test]
async fn the_published_instance_type_default_is_the_type_an_environment_runs() {
    let emulator = Emulator::new();
    let stack = eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack")
        .name();
    let case = "the published default";
    let (options, answer) = published_options(
        &emulator,
        case,
        &[("SolutionStackName", stack.as_str())],
    )
    .await;
    let described = selected(&options, NAMESPACE_LAUNCH_CONFIGURATION, OPTION_INSTANCE_TYPE);
    let default_value = described
        .first()
        .and_then(|option| option.default_value.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the instance type must be published with its default, got body {}", answer.body)
        });

    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    // Two environments: one that sets no instance type, which must then run the
    // type the catalogue publishes as the default, and one that SETS the
    // published option, which must run the type it named — so the published
    // option is one the service really honours.
    let chosen = "c5.xlarge";
    for (environment, options) in [
        ("default-type-env", Vec::new()),
        (
            "named-type-env",
            vec![
                (
                    "OptionSettings.member.1.Namespace",
                    NAMESPACE_LAUNCH_CONFIGURATION,
                ),
                ("OptionSettings.member.1.OptionName", OPTION_INSTANCE_TYPE),
                ("OptionSettings.member.1.Value", chosen),
            ],
        ),
    ] {
        let mut params = vec![
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", environment),
            ("SolutionStackName", stack.as_str()),
        ];
        params.extend(options);
        ok(&emulator, environment, "CreateEnvironment", &params).await;
    }
    std::thread::sleep(PAST_DUE);

    for (environment, expected) in [
        ("default-type-env", default_value.as_str()),
        ("named-type-env", chosen),
    ] {
        let health = ok(
            &emulator,
            environment,
            "DescribeInstancesHealth",
            &[
                ("EnvironmentName", environment),
                ("AttributeNames.member.1", "InstanceType"),
            ],
        )
        .await;
        let reported = health.values_at("InstanceHealthList/member/InstanceType");
        assert!(
            !reported.is_empty(),
            "case {environment:?}: expected the environment's instances, got body {}",
            health.body
        );
        for instance_type in reported {
            assert_eq!(
                instance_type, expected,
                "case {environment:?}: expected the instances to run {expected}, got \
                 {instance_type:?} and body {}",
                health.body
            );
        }
    }
}
