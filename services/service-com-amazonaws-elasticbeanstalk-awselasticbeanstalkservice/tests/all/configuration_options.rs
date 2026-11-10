// SPDX-License-Identifier: CC0-1.0

//! `DescribeConfigurationOptions`, driven end to end over the service's awsQuery
//! HTTP surface.
//!
//! The rules under test:
//!
//! * the options reported are the ones the PLATFORM VERSION behind the request
//!   defines — the general catalog every platform version defines, the options
//!   its runtime adds, and the worker daemon's options where its branch supports
//!   that tier — so the four ways of naming one platform version (a stored
//!   configuration template, a stored environment, a solution stack name, a
//!   platform version ARN) report the same catalog, and the answer names that
//!   version in both forms;
//! * every description is TYPED: a change severity from the three documented
//!   values, a value type from the modelled value set, and the restrictions that
//!   option carries — a range on a numeric option, a length or a regular
//!   expression on a string one, a value set on a selection;
//! * `DefaultValue` is the option's default on that PLATFORM, which is what an
//!   option a configuration set does not set falls back to: it does not follow
//!   what the set stores, and `DescribeConfigurationSettings` reports that
//!   default for an option the set never wrote (the rule
//!   `all/configuration_resolution.rs` is the suite for);
//! * `Options` restricts the descriptions to the options it names, a
//!   specification carrying only a namespace selecting that whole namespace;
//! * a request that names a configuration set two ways, a platform version two
//!   ways, a set together with a platform version, a set without its
//!   application, nothing at all, or a platform version this account cannot
//!   read is refused with this service's own error code for that class.
//!
//! Each table row is a different point in those classes — a different platform
//! category, a different constraint kind, a different way of naming the
//! platform, a different restriction on the request — not a renamed copy of the
//! row before it.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns,
//! and the setup helper and catalogue lookup come from `all/
//! configuration_templates.rs`: nothing here reaches into the store directly,
//! so an answer that is not derived from the store and the service's own
//! catalog fails these tests. This suite adds NO dependency of its own —
//! Cargo.lock is the one file every fix landing on mainline shares.

use crate::lifecycle::{Answer, Emulator, TEST_REGION};
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application the configuration sets below are created under.
const APPLICATION: &str = "options-app";

/// One `ConfigurationOptionDescription` as the answer reports it. Read member by
/// member off ONE element, so an answer that mixes up which restriction belongs
/// to which option fails rather than matching several independent flat lists.
///
/// `pub(crate)` along with [`reported_options`]: the
/// `configuration_resolution` suite reads the catalogue a configuration set
/// resolves against off the same wire answer, so neither suite keeps a second
/// reader of this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportedOption {
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) default_value: Option<String>,
    pub(crate) change_severity: String,
    pub(crate) user_defined: Option<String>,
    pub(crate) value_type: String,
    value_options: Vec<String>,
    min_value: Option<String>,
    max_value: Option<String>,
    max_length: Option<String>,
    regex: Option<(String, String)>,
}

impl ReportedOption {
    fn key(&self) -> (String, String) {
        (self.namespace.clone(), self.name.clone())
    }
}

/// Every option description in `answer`, in the order the document lists them.
pub(crate) fn reported_options(answer: &Answer) -> Vec<ReportedOption> {
    answer
        .elements_at("Options/member")
        .iter()
        .map(|member| ReportedOption {
            namespace: member.value("Namespace").unwrap_or_default().to_string(),
            name: member.value("Name").unwrap_or_default().to_string(),
            default_value: member.value("DefaultValue").map(str::to_string),
            change_severity: member
                .value("ChangeSeverity")
                .unwrap_or_default()
                .to_string(),
            user_defined: member.value("UserDefined").map(str::to_string),
            value_type: member.value("ValueType").unwrap_or_default().to_string(),
            value_options: member
                .values_at("ValueOptions/member")
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            min_value: member.value("MinValue").map(str::to_string),
            max_value: member.value("MaxValue").map(str::to_string),
            max_length: member.value("MaxLength").map(str::to_string),
            regex: member.elements_at("Regex").first().map(|regex| {
                (
                    regex.value("Pattern").unwrap_or_default().to_string(),
                    regex.value("Label").unwrap_or_default().to_string(),
                )
            }),
        })
        .collect()
}

/// The `(Namespace, Name)` pairs `answer` reports.
fn reported_keys(answer: &Answer) -> Vec<(String, String)> {
    reported_options(answer)
        .iter()
        .map(ReportedOption::key)
        .collect()
}

/// The namespaces `answer` reports, de-duplicated, in document order.
fn reported_namespaces(answer: &Answer) -> Vec<String> {
    let mut namespaces: Vec<String> = Vec::new();
    for option in reported_options(answer) {
        if namespaces.last() != Some(&option.namespace) {
            namespaces.push(option.namespace);
        }
    }
    namespaces
}

/// The two helpers this suite drives its preconditions and its catalogue
/// lookups through live in the `configuration_templates` suite beside the other
/// configuration-set helpers, so this target keeps ONE of each:
/// [`created`](crate::configuration_templates::created) requires a 200 from a
/// setup call, and `published_stack_of_category` names a platform version the
/// service really publishes.
use crate::configuration_templates::{created, published_stack_of_category as published_stack};

// ---------------------------------------------------------------------------
// What a platform version defines
// ---------------------------------------------------------------------------

/// One row of the per-platform table: a published platform CATEGORY, the
/// namespace its runtime adds, a namespace that belongs to another runtime and
/// so must be absent, and whether the platform runs the Amazon SQS daemon.
struct PlatformCase {
    case: &'static str,
    category: &'static str,
    /// The `aws:elasticbeanstalk:container:…` namespace this runtime
    /// configures, where it has one.
    runtime_namespace: Option<&'static str>,
    /// A namespace another runtime owns, which this platform must not report.
    foreign_namespace: &'static str,
    /// The `ProxyServer` value set this platform offers, where it offers the
    /// option at all.
    proxy_values: Option<&'static [&'static str]>,
}

#[tokio::test]
async fn a_platform_version_defines_the_general_catalog_plus_what_its_runtime_and_tier_add() {
    let emulator = Emulator::new();

    // Rows differ along the runtime the platform version runs, which is what
    // the per-platform half of the catalog follows: a Python platform
    // configures its WSGI server, a PHP platform its `php.ini`, a Tomcat
    // platform its JVM, a .NET-on-Windows platform its IIS application pool
    // (and, proxying through IIS, offers no `ProxyServer` option at all), and a
    // Docker platform runs whatever the image holds and so adds no runtime
    // namespace.
    let cases = [
        PlatformCase {
            case: "python",
            category: "Python",
            runtime_namespace: Some("aws:elasticbeanstalk:container:python"),
            foreign_namespace: "aws:elasticbeanstalk:container:php:phpini",
            proxy_values: Some(&["apache", "nginx"]),
        },
        PlatformCase {
            case: "php",
            category: "PHP",
            runtime_namespace: Some("aws:elasticbeanstalk:container:php:phpini"),
            foreign_namespace: "aws:elasticbeanstalk:container:python",
            proxy_values: Some(&["apache", "nginx"]),
        },
        PlatformCase {
            case: "tomcat",
            category: "Tomcat",
            runtime_namespace: Some("aws:elasticbeanstalk:container:tomcat:jvmoptions"),
            foreign_namespace: "aws:elasticbeanstalk:container:ruby",
            proxy_values: Some(&["apache", "nginx"]),
        },
        PlatformCase {
            case: "node.js",
            category: "Node.js",
            runtime_namespace: Some("aws:elasticbeanstalk:container:nodejs"),
            foreign_namespace: "aws:elasticbeanstalk:container:python",
            proxy_values: Some(&["nginx", "none"]),
        },
        PlatformCase {
            case: ".net on windows server",
            category: ".NET",
            runtime_namespace: Some("aws:elasticbeanstalk:container:dotnet:apppool"),
            foreign_namespace: "aws:elasticbeanstalk:container:python",
            proxy_values: None,
        },
        PlatformCase {
            case: "docker",
            category: "Docker",
            runtime_namespace: None,
            foreign_namespace: "aws:elasticbeanstalk:container:nodejs",
            proxy_values: Some(&["nginx", "none"]),
        },
        PlatformCase {
            case: "go",
            category: "Go",
            runtime_namespace: None,
            foreign_namespace: "aws:elasticbeanstalk:container:tomcat:jvmoptions",
            proxy_values: Some(&["nginx", "none"]),
        },
    ];

    for case in cases {
        let stack = published_stack(case.category);
        let answer = created(
            &emulator,
            "DescribeConfigurationOptions",
            &[("SolutionStackName", &stack.name())],
        )
        .await;
        let options = reported_options(&answer);
        let keys = reported_keys(&answer);
        let namespaces = reported_namespaces(&answer);

        // The platform version is named both ways in the answer, whichever way
        // the request named it.
        assert_eq!(
            answer.value("SolutionStackName"),
            Some(stack.name().as_str()),
            "case {:?}: the answer must name the solution stack it describes, got {}",
            case.case,
            answer.body
        );
        assert_eq!(
            answer.value("PlatformArn"),
            Some(stack.platform_arn(TEST_REGION).as_str()),
            "case {:?}: the answer must name the platform version's ARN, got {}",
            case.case,
            answer.body
        );

        // The general catalog every platform version defines: three namespaces
        // spanning the instances, the deployment command and the environment.
        for namespace in [
            "aws:autoscaling:asg",
            "aws:elasticbeanstalk:command",
            "aws:elasticbeanstalk:environment",
        ] {
            assert!(
                namespaces.iter().any(|reported| reported == namespace),
                "case {:?}: every platform version defines {namespace}, got {namespaces:?}",
                case.case
            );
        }

        match case.runtime_namespace {
            Some(namespace) => assert!(
                namespaces.iter().any(|reported| reported == namespace),
                "case {:?}: this platform's runtime defines {namespace}, got {namespaces:?}",
                case.case
            ),
            None => assert!(
                !namespaces
                    .iter()
                    .any(|reported| reported.starts_with("aws:elasticbeanstalk:container:")),
                "case {:?}: this platform configures no application server, got {namespaces:?}",
                case.case
            ),
        }
        assert!(
            !namespaces
                .iter()
                .any(|reported| reported == case.foreign_namespace),
            "case {:?}: {} belongs to another runtime, got {namespaces:?}",
            case.case,
            case.foreign_namespace
        );

        let proxy = options.iter().find(|option| {
            option.namespace == "aws:elasticbeanstalk:environment:proxy"
                && option.name == "ProxyServer"
        });
        match case.proxy_values {
            Some(expected) => {
                let proxy = proxy.unwrap_or_else(|| {
                    panic!(
                        "case {:?}: this platform offers a ProxyServer option, got {namespaces:?}",
                        case.case
                    )
                });
                assert_eq!(
                    proxy.value_options, expected,
                    "case {:?}: unexpected ProxyServer value set, got {proxy:?}",
                    case.case
                );
            }
            None => assert!(
                proxy.is_none(),
                "case {:?}: this platform proxies through its own web server, got {proxy:?}",
                case.case
            ),
        }

        // The Amazon SQS daemon's options belong to a platform version whose
        // BRANCH supports the worker tier. The expectation is read from the
        // branch catalogue over the wire, so a row cannot disagree with what
        // `ListPlatformBranches` publishes for the same platform version.
        let tiers = supported_tiers(&emulator, stack).await;
        let worker_capable = tiers.iter().any(|tier| tier == eb::state::TIER_WORKER);
        let daemon_options = keys
            .iter()
            .filter(|(namespace, _)| namespace == "aws:elasticbeanstalk:sqsd")
            .count();
        assert_eq!(
            daemon_options > 0,
            worker_capable,
            "case {:?}: the Amazon SQS daemon's options follow the branch's worker tier \
             (worker capable: {worker_capable}), got {daemon_options} of them",
            case.case
        );

        // Every option is reported ONCE: a namespace and an option name
        // together identify one option.
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            keys.len(),
            "case {:?}: an option must be described once, got {} descriptions and {} distinct \
             options",
            case.case,
            keys.len(),
            unique.len()
        );

        // The order is this service's own and STABLE: by namespace, then by
        // option name, so two reads of one platform report one sequence.
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            keys, sorted,
            "case {:?}: the catalog is reported in a stable order, got {keys:?}",
            case.case
        );
    }
}

/// The tiers the branch of `stack` supports, read over the wire from
/// `ListPlatformBranches` — the catalogue the service publishes, rather than a
/// second copy of it here.
async fn supported_tiers(emulator: &Emulator, stack: &eb::state::SolutionStack) -> Vec<String> {
    let branches = created(
        emulator,
        "ListPlatformBranches",
        &[
            ("Filters.member.1.Attribute", "BranchName"),
            ("Filters.member.1.Operator", "="),
            ("Filters.member.1.Values.member.1", stack.branch_name),
        ],
    )
    .await;
    branches
        .elements_at("PlatformBranchSummaryList/member")
        .first()
        .map(|branch| {
            branch
                .values_at("SupportedTierList/member")
                .iter()
                .map(|tier| (*tier).to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every platform CATEGORY the solution stack catalogue publishes, and the
/// `aws:elasticbeanstalk:container:…` namespace a version of that category must
/// then define — `None` where the category deliberately defines none (Go, .NET
/// on Linux and Java SE run the application directly behind the proxy, and
/// Docker runs whatever the image holds).
///
/// The list is checked against the categories `eb::state::SOLUTION_STACKS`
/// actually publishes below, so a category added or renamed in the catalogue
/// fails here rather than silently losing its runtime options.
const CATEGORY_RUNTIME_NAMESPACES: &[(&str, Option<&str>)] = &[
    (".NET", Some("aws:elasticbeanstalk:container:dotnet:apppool")),
    (".NET Core", None),
    ("Docker", None),
    ("Go", None),
    ("Java", None),
    ("Node.js", Some("aws:elasticbeanstalk:container:nodejs")),
    ("PHP", Some("aws:elasticbeanstalk:container:php:phpini")),
    ("Python", Some("aws:elasticbeanstalk:container:python")),
    ("Ruby", Some("aws:elasticbeanstalk:container:ruby")),
    ("Tomcat", Some("aws:elasticbeanstalk:container:tomcat:jvmoptions")),
];

#[tokio::test]
async fn every_published_platform_category_reports_the_runtime_options_it_defines() {
    let emulator = Emulator::new();

    let mut published: Vec<&str> = eb::state::SOLUTION_STACKS
        .iter()
        .map(|stack| stack.platform_category)
        .collect();
    published.sort_unstable();
    published.dedup();
    let mut listed: Vec<&str> = CATEGORY_RUNTIME_NAMESPACES
        .iter()
        .map(|(category, _)| *category)
        .collect();
    listed.sort_unstable();
    assert_eq!(
        published, listed,
        "case \"the published categories\": every platform category the catalogue publishes must \
         state the runtime options it defines"
    );

    for (category, runtime_namespace) in CATEGORY_RUNTIME_NAMESPACES {
        let stack = published_stack(category);
        let answer = created(
            &emulator,
            "DescribeConfigurationOptions",
            &[("SolutionStackName", &stack.name())],
        )
        .await;
        let namespaces = reported_namespaces(&answer);
        let container: Vec<&String> = namespaces
            .iter()
            .filter(|namespace| namespace.starts_with("aws:elasticbeanstalk:container:"))
            .collect();
        match runtime_namespace {
            Some(expected) => assert!(
                container.iter().any(|namespace| *namespace == expected),
                "case {category:?}: a version of this category defines {expected}, got \
                 {container:?}"
            ),
            None => assert!(
                container.is_empty(),
                "case {category:?}: a version of this category configures no application \
                 server, got {container:?}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Typed descriptions
// ---------------------------------------------------------------------------

/// One row of the constraint table: the option, and what its description must
/// carry. Each row is a different KIND of restriction.
struct OptionCase {
    case: &'static str,
    namespace: &'static str,
    name: &'static str,
    default_value: Option<&'static str>,
    change_severity: &'static str,
    user_defined: &'static str,
    value_type: &'static str,
    value_options: &'static [&'static str],
    min_value: Option<&'static str>,
    max_value: Option<&'static str>,
    max_length: Option<&'static str>,
    regex_label: Option<&'static str>,
}

#[tokio::test]
async fn every_option_description_carries_the_restrictions_that_option_has() {
    let emulator = Emulator::new();
    // A Python platform version, so the rows can span both the general catalog
    // and a runtime's own options.
    let stack = published_stack("Python");
    let answer = created(
        &emulator,
        "DescribeConfigurationOptions",
        &[("SolutionStackName", &stack.name())],
    )
    .await;
    let options = reported_options(&answer);

    // Rows differ along the restriction kind the published option reference
    // states for them: a numeric range, a value set, a boolean's two values,
    // several selections from one set, a maximum length, a regular expression,
    // an option with no default at all, and one the SERVICE rather than the
    // caller sets.
    let cases = [
        OptionCase {
            case: "a numeric option with a range",
            namespace: "aws:elb:healthcheck",
            name: "Interval",
            default_value: Some("10"),
            change_severity: eb::state::CHANGE_SEVERITY_NO_INTERRUPTION,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_SCALAR,
            value_options: &[],
            min_value: Some("5"),
            max_value: Some("300"),
            max_length: None,
            regex_label: None,
        },
        OptionCase {
            case: "a selection from a value set",
            namespace: "aws:elasticbeanstalk:command",
            name: "DeploymentPolicy",
            default_value: Some("AllAtOnce"),
            change_severity: eb::state::CHANGE_SEVERITY_NO_INTERRUPTION,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_SCALAR,
            value_options: &[
                "AllAtOnce",
                "Immutable",
                "Rolling",
                "RollingWithAdditionalBatch",
                "TrafficSplitting",
            ],
            min_value: None,
            max_value: None,
            max_length: None,
            regex_label: None,
        },
        OptionCase {
            case: "a boolean option",
            namespace: "aws:elasticbeanstalk:xray",
            name: "XRayEnabled",
            default_value: Some("false"),
            change_severity: eb::state::CHANGE_SEVERITY_NO_INTERRUPTION,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_SCALAR,
            value_options: eb::state::BOOLEAN_VALUE_OPTIONS,
            min_value: None,
            max_value: None,
            max_length: None,
            regex_label: None,
        },
        OptionCase {
            // "List: values for this option are multiple selections from the
            // possible values."
            case: "several selections from one value set",
            namespace: "aws:ec2:instances",
            name: "SupportedArchitectures",
            default_value: Some("x86_64"),
            change_severity: eb::state::CHANGE_SEVERITY_RESTART_ENVIRONMENT,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_LIST,
            value_options: &["arm64", "x86_64"],
            min_value: None,
            max_value: None,
            max_length: None,
            regex_label: None,
        },
        OptionCase {
            case: "a string option with a maximum length",
            namespace: "aws:autoscaling:launchconfiguration",
            name: "EC2KeyName",
            default_value: None,
            change_severity: eb::state::CHANGE_SEVERITY_RESTART_ENVIRONMENT,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_SCALAR,
            value_options: &[],
            min_value: None,
            max_value: None,
            max_length: Some("255"),
            regex_label: None,
        },
        OptionCase {
            // The managed-update maintenance window, stated as
            // `day:hour:minute`: the option's FORM is its restriction.
            case: "a string option restricted by a regular expression",
            namespace: "aws:elasticbeanstalk:managedactions",
            name: "PreferredStartTime",
            default_value: None,
            change_severity: eb::state::CHANGE_SEVERITY_NO_INTERRUPTION,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_SCALAR,
            value_options: &[],
            min_value: None,
            max_value: None,
            max_length: None,
            regex_label: Some("DayOfWeekAndTime"),
        },
        OptionCase {
            case: "an option with no default value",
            namespace: "aws:autoscaling:launchconfiguration",
            name: "RootVolumeType",
            default_value: None,
            change_severity: eb::state::CHANGE_SEVERITY_RESTART_ENVIRONMENT,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_SCALAR,
            value_options: &["gp2", "gp3", "io1", "standard"],
            min_value: None,
            max_value: None,
            max_length: None,
            regex_label: None,
        },
        OptionCase {
            // The topic the service itself creates for an environment's
            // notifications — an option whose VALUE the service sets. It
            // reports `UserDefined` false for the same reason every row above
            // does: the option is one the PLATFORM defines, not one the user
            // defined.
            case: "an option the service sets",
            namespace: "aws:elasticbeanstalk:sns:topics",
            name: "Notification Topic ARN",
            default_value: None,
            change_severity: eb::state::CHANGE_SEVERITY_NO_INTERRUPTION,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_SCALAR,
            value_options: &[],
            min_value: None,
            max_value: None,
            max_length: None,
            regex_label: None,
        },
        OptionCase {
            // A runtime's own option, on the platform that has that runtime.
            case: "the WSGI server's process count",
            namespace: "aws:elasticbeanstalk:container:python",
            name: "NumProcesses",
            default_value: Some("1"),
            change_severity: eb::state::CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
            user_defined: "false",
            value_type: eb::state::OPTION_VALUE_TYPE_SCALAR,
            value_options: &[],
            min_value: Some("1"),
            max_value: Some("1000"),
            max_length: None,
            regex_label: None,
        },
    ];

    for case in cases {
        let option = options
            .iter()
            .find(|option| option.namespace == case.namespace && option.name == case.name)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a description of {} / {}, got {}",
                    case.case, case.namespace, case.name, answer.body
                )
            });
        assert_eq!(
            option.default_value.as_deref(),
            case.default_value,
            "case {:?}: unexpected DefaultValue, got {option:?}",
            case.case
        );
        assert_eq!(
            option.change_severity, case.change_severity,
            "case {:?}: unexpected ChangeSeverity, got {option:?}",
            case.case
        );
        assert_eq!(
            option.user_defined.as_deref(),
            Some(case.user_defined),
            "case {:?}: unexpected UserDefined, got {option:?}",
            case.case
        );
        assert_eq!(
            option.value_type, case.value_type,
            "case {:?}: unexpected ValueType, got {option:?}",
            case.case
        );
        assert_eq!(
            option.value_options, case.value_options,
            "case {:?}: unexpected ValueOptions, got {option:?}",
            case.case
        );
        assert_eq!(
            option.min_value.as_deref(),
            case.min_value,
            "case {:?}: unexpected MinValue, got {option:?}",
            case.case
        );
        assert_eq!(
            option.max_value.as_deref(),
            case.max_value,
            "case {:?}: unexpected MaxValue, got {option:?}",
            case.case
        );
        assert_eq!(
            option.max_length.as_deref(),
            case.max_length,
            "case {:?}: unexpected MaxLength, got {option:?}",
            case.case
        );
        match case.regex_label {
            Some(label) => {
                let (pattern, reported_label) = option.regex.clone().unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a Regex restriction, got {option:?}",
                        case.case
                    )
                });
                assert_eq!(
                    reported_label, label,
                    "case {:?}: unexpected Regex Label, got {option:?}",
                    case.case
                );
                // The pattern is checked for the FORM it restricts the option
                // to — anchored at both ends, and admitting each day of the
                // week the maintenance window may name — rather than compiled:
                // a regular-expression crate would have to be recorded in
                // Cargo.lock, the one file this branch shares with every other
                // fix landing on mainline.
                assert!(
                    pattern.starts_with('^') && pattern.ends_with('$'),
                    "case {:?}: the published Pattern restricts the WHOLE value, so it is \
                     anchored at both ends, got {pattern:?}",
                    case.case
                );
                for day in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
                    assert!(
                        pattern.contains(day),
                        "case {:?}: a maintenance window may name {day}, so the published \
                         Pattern admits it, got {pattern:?}",
                        case.case
                    );
                }
            }
            None => assert!(
                option.regex.is_none(),
                "case {:?}: this option carries no Regex restriction, got {option:?}",
                case.case
            ),
        }
    }

    // The invariants every description obeys, over the WHOLE catalog rather
    // than the rows above: the two typed members come from their documented
    // value sets, a range is a range, and a regular expression is named.
    for option in &options {
        assert!(
            [
                eb::state::CHANGE_SEVERITY_NO_INTERRUPTION,
                eb::state::CHANGE_SEVERITY_RESTART_ENVIRONMENT,
                eb::state::CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
            ]
            .contains(&option.change_severity.as_str()),
            "case \"every description\": {option:?} must state one of the three documented \
             change severities"
        );
        assert!(
            eb::ops::types::CONFIGURATION_OPTION_VALUE_TYPE_VALUES
                .contains(&option.value_type.as_str()),
            "case \"every description\": {option:?} must state a modelled ValueType"
        );
        if let (Some(min_value), Some(max_value)) = (&option.min_value, &option.max_value) {
            let min: i64 = min_value.parse().unwrap_or_else(|error| {
                panic!("case \"every description\": {option:?} MinValue is an integer: {error}")
            });
            let max: i64 = max_value.parse().unwrap_or_else(|error| {
                panic!("case \"every description\": {option:?} MaxValue is an integer: {error}")
            });
            assert!(
                min <= max,
                "case \"every description\": {option:?} must not publish an empty range"
            );
        }
        if let Some((pattern, label)) = &option.regex {
            assert!(
                !pattern.is_empty() && !label.is_empty(),
                "case \"every description\": a Regex restriction carries a Pattern and a Label, \
                 got {option:?}"
            );
        }
        assert!(
            !option.namespace.is_empty() && !option.name.is_empty(),
            "case \"every description\": every option is named, got {option:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The four ways of naming one platform version
// ---------------------------------------------------------------------------

/// One row of the naming table: how the request names the platform version,
/// and the published version it then has to describe.
struct NamingCase {
    case: &'static str,
    /// The request members beyond the action itself.
    params: Vec<(&'static str, String)>,
    stack: &'static eb::state::SolutionStack,
}

#[tokio::test]
async fn a_configuration_set_reports_the_catalog_of_the_platform_version_it_is_for() {
    let emulator = Emulator::new();
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;

    // Two DIFFERENT platform versions, so a request that reported one
    // configuration set's catalog for the other's platform fails here.
    let template_stack = published_stack("Python");
    let environment_stack = published_stack("Tomcat");

    // The template SETS one option (`MinSize`) and leaves another
    // (`MaxSize`) unset, so the rule about `DefaultValue` has both cases.
    created(
        &emulator,
        "CreateConfigurationTemplate",
        &[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "options-template"),
            ("SolutionStackName", &template_stack.name()),
            ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
            ("OptionSettings.member.1.OptionName", "MinSize"),
            ("OptionSettings.member.1.Value", "3"),
        ],
    )
    .await;
    created(
        &emulator,
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", "options-env"),
            ("SolutionStackName", &environment_stack.name()),
        ],
    )
    .await;

    // Rows differ along WHICH of the four ways the request names the platform
    // version whose options it asks for.
    let cases = vec![
        NamingCase {
            case: "by solution stack name",
            params: vec![("SolutionStackName", template_stack.name())],
            stack: template_stack,
        },
        NamingCase {
            case: "by platform version ARN",
            params: vec![("PlatformArn", template_stack.platform_arn(TEST_REGION))],
            stack: template_stack,
        },
        NamingCase {
            case: "by configuration template",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("TemplateName", "options-template".to_string()),
            ],
            stack: template_stack,
        },
        NamingCase {
            case: "by environment",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("EnvironmentName", "options-env".to_string()),
            ],
            stack: environment_stack,
        },
    ];

    for NamingCase {
        case,
        params,
        stack,
    } in cases
    {
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = created(&emulator, "DescribeConfigurationOptions", &params).await;
        assert_eq!(
            answer.value("SolutionStackName"),
            Some(stack.name().as_str()),
            "case {case:?}: the answer names the platform version the options belong to, got {}",
            answer.body
        );
        assert_eq!(
            answer.value("PlatformArn"),
            Some(stack.platform_arn(TEST_REGION).as_str()),
            "case {case:?}: the answer names that version's ARN, got {}",
            answer.body
        );

        // The catalog is the PLATFORM's, so naming the same platform version
        // through a configuration set reports the same options.
        let by_stack = created(
            &emulator,
            "DescribeConfigurationOptions",
            &[("SolutionStackName", &stack.name())],
        )
        .await;
        assert_eq!(
            reported_options(&answer),
            reported_options(&by_stack),
            "case {case:?}: naming one platform version two ways reports one catalog"
        );

        // `DefaultValue` is the option's default on that platform: the value
        // an option the set does not carry falls back to. It does not follow
        // what the set stores — the template above sets `MinSize` to 3 and the
        // catalog still publishes the platform's default for it.
        let min_size = reported_options(&answer)
            .into_iter()
            .find(|option| option.namespace == "aws:autoscaling:asg" && option.name == "MinSize")
            .unwrap_or_else(|| panic!("case {case:?}: expected a MinSize description"));
        let documented_default = eb::state::DEFAULT_MIN_SIZE.to_string();
        assert_eq!(
            min_size.default_value.as_deref(),
            Some(documented_default.as_str()),
            "case {case:?}: MinSize's DefaultValue is the platform's default, got {min_size:?}"
        );
    }

    // What the SET stores is its OVERRIDE of the catalog above, and the
    // configuration set it reports is that override resolved over the
    // catalog: the option the template wrote carries the value it wrote, and
    // the sibling it never wrote carries the `DefaultValue` the catalog
    // publishes for it.
    let settings = created(
        &emulator,
        "DescribeConfigurationSettings",
        &[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "options-template"),
        ],
    )
    .await;
    let resolved: Vec<(String, String)> = settings
        .elements_at("ConfigurationSettings/member/OptionSettings/member")
        .iter()
        .filter(|setting| setting.value("Namespace") == Some("aws:autoscaling:asg"))
        .map(|setting| {
            (
                setting.value("OptionName").unwrap_or_default().to_string(),
                setting.value("Value").unwrap_or_default().to_string(),
            )
        })
        .collect();
    let published_max_size = reported_options(
        &created(
            &emulator,
            "DescribeConfigurationOptions",
            &[("SolutionStackName", &template_stack.name())],
        )
        .await,
    )
    .into_iter()
    .find(|option| option.namespace == "aws:autoscaling:asg" && option.name == "MaxSize")
    .and_then(|option| option.default_value)
    .unwrap_or_else(|| panic!("case \"the resolved settings\": expected a MaxSize default"));
    assert!(
        resolved.contains(&("MinSize".to_string(), "3".to_string())),
        "case \"the resolved settings\": the option the template wrote must carry its \
         value, got {resolved:?} in {}",
        settings.body
    );
    assert!(
        resolved.contains(&("MaxSize".to_string(), published_max_size.clone())),
        "case \"the resolved settings\": the option the template never wrote must carry the \
         published default {published_max_size:?}, got {resolved:?} in {}",
        settings.body
    );
}

// ---------------------------------------------------------------------------
// Restricting the descriptions
// ---------------------------------------------------------------------------

/// One row of the restriction table: what the request's `Options`
/// specifications state, and the options they then select.
struct FilterCase {
    case: &'static str,
    restriction: Vec<(&'static str, &'static str)>,
    expected: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn options_restricts_the_descriptions_to_the_options_it_names() {
    let emulator = Emulator::new();
    let stack = published_stack("Python");
    let whole_catalog = reported_keys(
        &created(
            &emulator,
            "DescribeConfigurationOptions",
            &[("SolutionStackName", &stack.name())],
        )
        .await,
    );
    assert!(
        whole_catalog.len() > 20,
        "case \"the whole catalog\": a platform version defines many options, got \
         {whole_catalog:?}"
    );

    // Rows differ along WHAT a specification states — a namespace alone, a
    // namespace and an option name, two specifications at once, an option name
    // alone, and a namespace this platform does not define — not along which
    // option happens to be named.
    let cases = vec![
        FilterCase {
            case: "a namespace alone selects that whole namespace",
            restriction: vec![("Options.member.1.Namespace", "aws:elb:healthcheck")],
            expected: vec![
                ("aws:elb:healthcheck", "HealthyThreshold"),
                ("aws:elb:healthcheck", "Interval"),
                ("aws:elb:healthcheck", "Timeout"),
                ("aws:elb:healthcheck", "UnhealthyThreshold"),
            ],
        },
        FilterCase {
            case: "a namespace and an option name select one option",
            restriction: vec![
                ("Options.member.1.Namespace", "aws:autoscaling:asg"),
                ("Options.member.1.OptionName", "MaxSize"),
            ],
            expected: vec![("aws:autoscaling:asg", "MaxSize")],
        },
        FilterCase {
            case: "two specifications select both options",
            restriction: vec![
                ("Options.member.1.Namespace", "aws:autoscaling:asg"),
                ("Options.member.1.OptionName", "MinSize"),
                (
                    "Options.member.2.Namespace",
                    "aws:elasticbeanstalk:environment",
                ),
                ("Options.member.2.OptionName", "EnvironmentType"),
            ],
            expected: vec![
                ("aws:autoscaling:asg", "MinSize"),
                ("aws:elasticbeanstalk:environment", "EnvironmentType"),
            ],
        },
        FilterCase {
            // An option name without a namespace selects that option in every
            // namespace that defines one by that name, and two namespaces do.
            case: "an option name alone selects it wherever it is defined",
            restriction: vec![("Options.member.1.OptionName", "RetentionInDays")],
            expected: vec![
                ("aws:elasticbeanstalk:cloudwatch:logs", "RetentionInDays"),
                (
                    "aws:elasticbeanstalk:cloudwatch:logs:health",
                    "RetentionInDays",
                ),
            ],
        },
        FilterCase {
            case: "a namespace this platform version does not define selects nothing",
            restriction: vec![(
                "Options.member.1.Namespace",
                "aws:elasticbeanstalk:container:php:phpini",
            )],
            expected: Vec::new(),
        },
        FilterCase {
            // `ResourceName` names the resource an option is SET on — a
            // time-based scaling action — not an option the catalog defines, so
            // a specification stating it alone names no option of the catalog.
            case: "a specification naming only a ResourceName selects nothing",
            restriction: vec![("Options.member.1.ResourceName", "nightly")],
            expected: Vec::new(),
        },
        FilterCase {
            // The same `ResourceName` beside a namespace and an option name
            // does not narrow the catalog further: the option is still
            // described.
            case: "a ResourceName beside an option still selects that option",
            restriction: vec![
                ("Options.member.1.Namespace", "aws:autoscaling:asg"),
                ("Options.member.1.OptionName", "MinSize"),
                ("Options.member.1.ResourceName", "nightly"),
            ],
            expected: vec![("aws:autoscaling:asg", "MinSize")],
        },
    ];

    for FilterCase {
        case,
        restriction,
        expected,
    } in cases
    {
        let mut params = vec![("SolutionStackName", stack.name())];
        params.extend(
            restriction
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = created(&emulator, "DescribeConfigurationOptions", &params).await;
        let keys = reported_keys(&answer);
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(namespace, name)| ((*namespace).to_string(), (*name).to_string()))
            .collect();
        assert_eq!(
            keys, expected,
            "case {case:?}: unexpected restricted catalog, got {}",
            answer.body
        );
        // A restricted answer still names the platform version it describes.
        assert_eq!(
            answer.value("SolutionStackName"),
            Some(stack.name().as_str()),
            "case {case:?}: a restricted answer still names its platform version, got {}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the refusal table: the request, and the error code this service
/// refuses that class with.
struct RefusalCase {
    case: &'static str,
    params: Vec<(&'static str, String)>,
    expected_code: &'static str,
}

#[tokio::test]
async fn a_request_that_names_no_one_platform_version_is_refused() {
    let emulator = Emulator::new();
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let stack = published_stack("Python");

    // Rows differ along WHY the request names no single readable platform
    // version: it names a configuration set two ways, a platform version two
    // ways, a set without the application it belongs to, nothing at all, or a
    // platform version this account cannot read.
    let cases = vec![
        RefusalCase {
            case: "a configuration set named two ways",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("TemplateName", "options-template".to_string()),
                ("EnvironmentName", "options-env".to_string()),
            ],
            expected_code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a platform version named two ways",
            params: vec![
                ("SolutionStackName", stack.name()),
                ("PlatformArn", stack.platform_arn(TEST_REGION)),
            ],
            expected_code: "InvalidParameterCombination",
        },
        RefusalCase {
            // A configuration set is FOR a platform version, so naming both
            // asks for one catalog from two sources.
            case: "a configuration set and a solution stack",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("TemplateName", "options-template".to_string()),
                ("SolutionStackName", stack.name()),
            ],
            expected_code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a configuration set and a platform version ARN",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("EnvironmentName", "options-env".to_string()),
                ("PlatformArn", stack.platform_arn(TEST_REGION)),
            ],
            expected_code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a template without its application",
            params: vec![("TemplateName", "options-template".to_string())],
            expected_code: "MissingRequiredParameter",
        },
        RefusalCase {
            case: "an environment without its application",
            params: vec![("EnvironmentName", "options-env".to_string())],
            expected_code: "MissingRequiredParameter",
        },
        RefusalCase {
            case: "nothing at all",
            params: Vec::new(),
            expected_code: "MissingRequiredParameter",
        },
        RefusalCase {
            case: "a solution stack this service does not publish",
            params: vec![(
                "SolutionStackName",
                "64bit Fictional Linux v0.0.1 running Nothing".to_string(),
            )],
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a platform version this account does not hold",
            params: vec![(
                "PlatformArn",
                format!("arn:aws:elasticbeanstalk:{TEST_REGION}::platform/Nothing/1.0.0"),
            )],
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a configuration template the application does not hold",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("TemplateName", "absent-template".to_string()),
            ],
            expected_code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "an environment the application does not hold",
            params: vec![
                ("ApplicationName", APPLICATION.to_string()),
                ("EnvironmentName", "absent-env".to_string()),
            ],
            expected_code: "InvalidParameterValue",
        },
    ];

    for RefusalCase {
        case,
        params,
        expected_code,
    } in cases
    {
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let answer = emulator
            .call("DescribeConfigurationOptions", &params)
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected a 400, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {case:?}: unexpected error code, got {}",
            answer.body
        );
        assert!(
            !answer.error_message().is_empty(),
            "case {case:?}: a refusal says what was wrong, got {}",
            answer.body
        );
    }
}
