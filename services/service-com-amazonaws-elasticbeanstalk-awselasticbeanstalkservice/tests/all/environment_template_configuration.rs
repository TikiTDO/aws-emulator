// SPDX-License-Identifier: CC0-1.0

//! An environment launched from a configuration template, over this service's
//! awsQuery surface.
//!
//! RULE: `CreateEnvironment$TemplateName` names "the configuration template to
//! use with the environment", so the environment is launched WITH that template's
//! configuration — its option settings and the platform version it names — with
//! the request's own `OptionSettings` over them, which is the same precedence
//! `UpdateEnvironment` applies when it deploys a template. A `TemplateName` the
//! request's application has no template for names no configuration at all and is
//! refused.
//!
//! The rows span what the rule generalises over: how many options the template
//! carries, whether the request overrides one of them, adds one of its own, does
//! both, or carries none; and whether the request names the platform version
//! itself or leaves it to the template. Every assertion reads the environment
//! back through `DescribeConfigurationSettings` / `DescribeEnvironments`, so what
//! is asserted is the configuration the environment RUNS rather than what the
//! create echoed.

use crate::lifecycle::{Emulator, SOLUTION_STACK};

/// The application every case is created under.
const APPLICATION: &str = "template-app";

/// The option namespace the cases set their own options in: the one a client's
/// own environment variables live in, so the values are the caller's rather than
/// a value the platform's option catalogue publishes a default for.
const NAMESPACE: &str = "aws:elasticbeanstalk:application:environment";

/// One option setting as a `(OptionName, Value)` pair in [`NAMESPACE`].
type Option1 = (&'static str, &'static str);

/// The awsQuery parameters for an `OptionSettings` list of `options`.
fn option_params(prefix: &str, options: &[Option1]) -> Vec<(String, String)> {
    options
        .iter()
        .enumerate()
        .flat_map(|(index, (name, value))| {
            let position = index + 1;
            [
                (
                    format!("{prefix}.member.{position}.Namespace"),
                    NAMESPACE.to_string(),
                ),
                (
                    format!("{prefix}.member.{position}.OptionName"),
                    (*name).to_string(),
                ),
                (
                    format!("{prefix}.member.{position}.Value"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect()
}

fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The value of the option `name` in [`NAMESPACE`] in the configuration set
/// `set`, or `None` when the set carries no such option.
fn option_value(set: &crate::lifecycle::Answer, name: &str) -> Option<String> {
    let namespaces = set.values_at("OptionSettings/member/Namespace");
    let names = set.values_at("OptionSettings/member/OptionName");
    let values = set.values_at("OptionSettings/member/Value");
    for (index, option_name) in names.iter().enumerate() {
        if *option_name == name && namespaces.get(index) == Some(&NAMESPACE) {
            return values.get(index).map(|value| (*value).to_string());
        }
    }
    None
}

/// One row: the template's options, the request's own options, and the value each
/// option must have in the configuration the environment ends up running.
struct Row {
    case: &'static str,
    template: &'static str,
    environment: &'static str,
    template_options: &'static [Option1],
    request_options: &'static [Option1],
    /// The options the deployed configuration set must report, as
    /// `(OptionName, Value)`.
    expected: &'static [Option1],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the template's one option is inherited",
            template: "one-option",
            environment: "env-one-option",
            template_options: &[("TEMPLATE_ONLY", "template-value")],
            request_options: &[],
            expected: &[("TEMPLATE_ONLY", "template-value")],
        },
        Row {
            case: "several template options are inherited",
            template: "several-options",
            environment: "env-several-options",
            template_options: &[
                ("FIRST", "one"),
                ("SECOND", "two"),
                ("THIRD", "three"),
            ],
            request_options: &[],
            expected: &[("FIRST", "one"), ("SECOND", "two"), ("THIRD", "three")],
        },
        Row {
            case: "the request overrides one of the template's options",
            template: "overridden",
            environment: "env-overridden",
            template_options: &[("SHARED", "from-template"), ("KEPT", "template-value")],
            request_options: &[("SHARED", "from-request")],
            expected: &[("SHARED", "from-request"), ("KEPT", "template-value")],
        },
        Row {
            case: "the request adds an option the template does not carry",
            template: "added",
            environment: "env-added",
            template_options: &[("FROM_TEMPLATE", "template-value")],
            request_options: &[("FROM_REQUEST", "request-value")],
            expected: &[
                ("FROM_TEMPLATE", "template-value"),
                ("FROM_REQUEST", "request-value"),
            ],
        },
        Row {
            case: "a template carrying no options of its own",
            template: "no-options",
            environment: "env-no-options",
            template_options: &[],
            request_options: &[("FROM_REQUEST", "request-value")],
            expected: &[("FROM_REQUEST", "request-value")],
        },
    ]
}

/// The deployed configuration set of `environment`.
async fn deployed(emulator: &Emulator, case: &str, environment: &str) -> crate::lifecycle::Answer {
    let described = emulator
        .call(
            "DescribeConfigurationSettings",
            &[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", environment),
            ],
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribeConfigurationSettings, got {} and body {}",
        described.status, described.body
    );
    let sets = described.elements_at("ConfigurationSettings/member");
    assert_eq!(
        sets.len(),
        1,
        "case {case:?}: a launched environment runs exactly one configuration set, got body {}",
        described.body
    );
    let set = described
        .elements_at("ConfigurationSettings/member")
        .into_iter()
        .next()
        .expect("one configuration set");
    assert_eq!(
        set.value("DeploymentStatus"),
        Some("deployed"),
        "case {case:?}: the set an environment runs is the deployed one, got {}",
        set.body
    );
    set
}

#[tokio::test]
async fn an_environment_launched_from_a_template_runs_the_templates_configuration() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    for row in rows() {
        let case = row.case;
        let mut template_params = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            ("TemplateName".to_string(), row.template.to_string()),
            (
                "SolutionStackName".to_string(),
                SOLUTION_STACK.as_str().to_string(),
            ),
        ];
        template_params.extend(option_params("OptionSettings", row.template_options));
        let template = emulator
            .call(
                "CreateConfigurationTemplate",
                &borrowed(&template_params),
            )
            .await;
        assert_eq!(
            template.status, 200,
            "case {case:?}: expected 200 from CreateConfigurationTemplate, got {} and body {}",
            template.status, template.body
        );

        let mut environment_params = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            ("EnvironmentName".to_string(), row.environment.to_string()),
            ("TemplateName".to_string(), row.template.to_string()),
        ];
        environment_params.extend(option_params("OptionSettings", row.request_options));
        let launched = emulator
            .call("CreateEnvironment", &borrowed(&environment_params))
            .await;
        assert_eq!(
            launched.status, 200,
            "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
            launched.status, launched.body
        );
        // The environment runs the platform version the template names, which the
        // request itself did not: a launch from a template inherits that too.
        assert_eq!(
            launched.value("SolutionStackName"),
            Some(SOLUTION_STACK.as_str()),
            "case {case:?}: expected the template's solution stack, got {}",
            launched.body
        );
        assert_eq!(
            launched.value("TemplateName"),
            Some(row.template),
            "case {case:?}: the environment must report the template it was launched from, \
             got {}",
            launched.body
        );

        let set = deployed(&emulator, case, row.environment).await;
        for (name, value) in row.expected {
            let reported = option_value(&set, name);
            assert_eq!(
                reported.as_deref(),
                Some(*value),
                "case {case:?}: expected {name} to be {value:?} in the deployed configuration, \
                 got {reported:?}"
            );
        }
        // An option neither the template nor the request set is not invented into
        // the caller's own namespace.
        let absent = option_value(&set, "NEVER_SET");
        assert_eq!(
            absent, None,
            "case {case:?}: expected no value for an option nobody set, got {absent:?}"
        );
    }
}

/// A `TemplateName` the application has no template for names no configuration,
/// so the launch is refused with the sentence every operation that resolves a
/// template answers — and no environment is recorded.
#[tokio::test]
async fn a_launch_from_an_unknown_template_is_refused() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    for (case, template_name) in [
        ("a template the application never had", "never-created"),
        ("a template name that is only whitespace", " "),
    ] {
        let refused = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", APPLICATION),
                    ("EnvironmentName", "env-unresolvable"),
                    ("TemplateName", template_name),
                ],
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
            "case {case:?}: unexpected error code in {}",
            refused.body
        );
        // A template is named by the `(ApplicationName, TemplateName)` pair, and
        // the refusal names it the way the lookup did.
        let expected =
            format!("No Configuration Template named '{APPLICATION}/{template_name}' found.");
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }

    let environments = emulator
        .call("DescribeEnvironments", &[("ApplicationName", APPLICATION)])
        .await;
    assert!(
        environments
            .values_at("Environments/member/EnvironmentId")
            .is_empty(),
        "case \"a refused launch\": no environment may have been recorded, got body {}",
        environments.body
    );
}
