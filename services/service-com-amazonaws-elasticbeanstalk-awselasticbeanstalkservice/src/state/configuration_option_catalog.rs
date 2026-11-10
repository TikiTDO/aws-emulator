// SPDX-License-Identifier: CC0-1.0

//! The service-owned configuration-option catalog: what a platform version
//! DEFINES, which `DescribeConfigurationOptions` reports and the environment
//! derivations resolve an option's value from.
//!
//! Split out of [`super`] so the option tables live beside the code that reads
//! them.

use super::*;

/// `ConfigurationOptionDescription$ChangeSeverity`: "There is no interruption
/// to the environment or application availability."
pub const CHANGE_SEVERITY_NO_INTERRUPTION: &str = "NoInterruption";
/// See [`CHANGE_SEVERITY_NO_INTERRUPTION`]. "The environment is entirely
/// restarted, all A resources are deleted and recreated, and the environment is
/// unavailable during the process."
pub const CHANGE_SEVERITY_RESTART_ENVIRONMENT: &str = "RestartEnvironment";
/// See [`CHANGE_SEVERITY_NO_INTERRUPTION`]. "The environment is available the
/// entire time. However, a short application outage occurs when the application
/// servers on the running Amazon EC2 instances are restarted."
pub const CHANGE_SEVERITY_RESTART_APPLICATION_SERVER: &str = "RestartApplicationServer";

/// `ConfigurationOptionValueType`: "List: Values for this option are multiple
/// selections from the possible values." Read from the modelled value set, so
/// this service cannot publish a type the model does not declare.
pub const OPTION_VALUE_TYPE_LIST: &str = CONFIGURATION_OPTION_VALUE_TYPE_VALUES[0];
/// See [`OPTION_VALUE_TYPE_LIST`]. "Scalar: Values for this option are a single
/// selection from the possible values, or an unformatted string, or numeric
/// value governed by the `MIN/MAX/Regex` constraints."
///
/// The member's documentation also mentions a `Boolean` and a `Json` type, but
/// the model's `ConfigurationOptionValueType` enum declares only `List` and
/// `Scalar` ([`CONFIGURATION_OPTION_VALUE_TYPE_VALUES`]), and the model is what
/// this service answers against: an option whose value is a boolean is a
/// `Scalar` whose [`BOOLEAN_VALUE_OPTIONS`] say which two values it takes, which
/// carries the same information inside the declared value set.
pub const OPTION_VALUE_TYPE_SCALAR: &str = CONFIGURATION_OPTION_VALUE_TYPE_VALUES[1];

/// The two values a boolean-valued configuration option takes, as its
/// `ValueOptions`. See [`OPTION_VALUE_TYPE_SCALAR`].
pub const BOOLEAN_VALUE_OPTIONS: &[&str] = &["false", "true"];

/// The `false` of that pair — the value a boolean option that is OFF carries,
/// read from the published set so the catalog and the handlers that read a
/// boolean option out of a configuration cannot spell it two ways.
pub const BOOLEAN_FALSE: &str = BOOLEAN_VALUE_OPTIONS[0];
/// The `true` of that pair. See [`BOOLEAN_FALSE`].
pub const BOOLEAN_TRUE: &str = BOOLEAN_VALUE_OPTIONS[1];

/// One configuration option a platform version defines — one
/// `ConfigurationOptionDescription` before it reaches the wire.
///
/// `default_value` is the option's documented default and `""` means the option
/// HAS no default (the member is then absent rather than an empty string);
/// `value_options` empty likewise means the option draws from no fixed value
/// set. The four restriction members are each present only where the option
/// carries that restriction, which is what makes the published description
/// typed: a numeric option carries a range, a string option a length or a
/// regular expression, and a selection carries its value set.
#[derive(Debug, Clone, Copy)]
pub struct ConfigurationOption {
    pub namespace: &'static str,
    pub name: &'static str,
    pub default_value: &'static str,
    /// One of [`CHANGE_SEVERITY_NO_INTERRUPTION`],
    /// [`CHANGE_SEVERITY_RESTART_ENVIRONMENT`] and
    /// [`CHANGE_SEVERITY_RESTART_APPLICATION_SERVER`].
    pub change_severity: &'static str,
    /// Whether the SERVICE owns this option's VALUE — one it sets on the
    /// environment it provisions (the topic it created, the queue it created)
    /// rather than one the caller supplies. It is what decides whether an
    /// `OptionsToRemove` may take the option out of a configuration set; it is
    /// NOT the published `UserDefined` flag, which answers whether the USER
    /// defined the option and so is false for every option this catalog
    /// publishes (see [`ConfigurationOption::to_description`]).
    pub service_owned: bool,
    /// One of [`OPTION_VALUE_TYPE_LIST`] and [`OPTION_VALUE_TYPE_SCALAR`].
    pub value_type: &'static str,
    pub value_options: &'static [&'static str],
    pub min_value: Option<i32>,
    pub max_value: Option<i32>,
    pub max_length: Option<i32>,
    /// `(Pattern, Label)` — "the regular expression pattern that a string
    /// configuration option value with this restriction must match" and "a
    /// unique name representing this regular expression".
    pub regex: Option<(&'static str, &'static str)>,
}

impl ConfigurationOption {
    /// A scalar option with no restriction beyond its type: the four
    /// `with_*` builders below add the ones an option carries, so a table entry
    /// states only what is true of THAT option.
    pub const fn new(
        namespace: &'static str,
        name: &'static str,
        change_severity: &'static str,
        default_value: &'static str,
    ) -> Self {
        Self {
            namespace,
            name,
            default_value,
            change_severity,
            service_owned: false,
            value_type: OPTION_VALUE_TYPE_SCALAR,
            value_options: &[],
            min_value: None,
            max_value: None,
            max_length: None,
            regex: None,
        }
    }

    /// "If specified, values for the configuration option are selected from
    /// this list."
    pub const fn with_values(mut self, value_options: &'static [&'static str]) -> Self {
        self.value_options = value_options;
        self
    }

    /// A boolean-valued option: a scalar whose value set is `true` | `false`.
    /// See [`OPTION_VALUE_TYPE_SCALAR`].
    pub const fn boolean(self) -> Self {
        self.with_values(BOOLEAN_VALUE_OPTIONS)
    }

    /// An option whose value is "multiple selections from the possible values".
    pub const fn multi_valued(mut self) -> Self {
        self.value_type = OPTION_VALUE_TYPE_LIST;
        self
    }

    /// "The configuration option must be a numeric value greater than this
    /// value" and "less than this value".
    pub const fn with_range(mut self, min_value: i32, max_value: i32) -> Self {
        self.min_value = Some(min_value);
        self.max_value = Some(max_value);
        self
    }

    /// "The configuration option must be a string value no longer than this
    /// value."
    pub const fn with_max_length(mut self, max_length: i32) -> Self {
        self.max_length = Some(max_length);
        self
    }

    /// "The configuration option must be a string value that satisfies this
    /// regular expression."
    pub const fn with_regex(mut self, pattern: &'static str, label: &'static str) -> Self {
        self.regex = Some((pattern, label));
        self
    }

    /// An option whose VALUE the SERVICE sets rather than the caller — see
    /// [`Self::service_owned`].
    pub const fn service_owned(mut self) -> Self {
        self.service_owned = true;
        self
    }

    /// The identity of an option: "a unique namespace identifying the option's
    /// associated Amazon Web Services resource" and "the name of the
    /// configuration option" together, which is also what an
    /// `OptionSpecification` names one by.
    pub fn key(&self) -> (&'static str, &'static str) {
        (self.namespace, self.name)
    }

    /// Whether `specification` names this option: every member the
    /// specification STATES has to match, so a specification carrying only a
    /// namespace selects that whole namespace — "if specified, restricts the
    /// descriptions to only the specified options".
    ///
    /// `ResourceName` is "a unique resource name for a time-based scaling
    /// configuration option": it names the resource an option is SET on, not an
    /// option the catalog DEFINES, so it never restricts which option is
    /// described — and a specification that states it ALONE names no option of
    /// the catalog at all, so it selects nothing rather than everything.
    pub fn is_named_by(&self, specification: &OptionSpecification) -> bool {
        let namespace = specification.namespace.as_deref();
        let option_name = specification.option_name.as_deref();
        if namespace.is_none() && option_name.is_none() {
            return false;
        }
        namespace.is_none_or(|namespace| namespace == self.namespace)
            && option_name.is_none_or(|option_name| option_name == self.name)
    }

    /// The modelled `ConfigurationOptionDescription` for this option.
    pub fn to_description(&self) -> ConfigurationOptionDescription {
        ConfigurationOptionDescription {
            namespace: Some(self.namespace.to_string()),
            name: Some(self.name.to_string()),
            default_value: stated(self.default_value),
            change_severity: Some(self.change_severity.to_string()),
            // "true: This configuration option was defined by the user …
            // false: This configuration was not defined by the user." Every
            // option this catalog publishes is one the PLATFORM VERSION
            // defines, so none of them was defined by the user; an option the
            // caller defines (an application environment variable, which the
            // caller names) is not in the catalog at all, which is why there is
            // no `true` here to read off a catalog entry.
            user_defined: Some(false),
            value_type: Some(self.value_type.to_string()),
            value_options: (!self.value_options.is_empty()).then(|| {
                self.value_options
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect()
            }),
            min_value: self.min_value,
            max_value: self.max_value,
            max_length: self.max_length,
            regex: self.regex.map(|(pattern, label)| OptionRestrictionRegex {
                pattern: Some(pattern.to_string()),
                label: Some(label.to_string()),
            }),
            // The option reference's own sentence about the option, from the
            // one table that holds them (see
            // [`super::configuration_option_descriptions`]); a unit test there
            // pins that every published option has one.
            description: option_description(self.namespace, self.name).map(str::to_string),
        }
    }
}

/// The CloudWatch Logs retention periods a log group can be kept for, in days,
/// which is the value set of every `RetentionInDays` option below.
pub(super) const LOG_RETENTION_IN_DAYS: &[&str] = &[
    "1", "3", "5", "7", "14", "30", "60", "90", "120", "150", "180", "365", "400", "545", "731",
    "1827", "3653",
];

/// The namespace holding the proxy option of the Linux platforms, and the
/// option in it: "ProxyServer: specifies which web server should be used to
/// proxy connections to the application".
pub(super) const NAMESPACE_PROXY: &str = "aws:elasticbeanstalk:environment:proxy";
/// See [`NAMESPACE_PROXY`].
pub(super) const OPTION_PROXY_SERVER: &str = "ProxyServer";

/// The configuration options EVERY platform version this service publishes
/// defines: the Auto Scaling group and launch configuration, the instances and
/// the VPC they run in, the deployment command, the environment itself, health
/// reporting, log streaming, managed updates, notifications and the load
/// balancer. The namespaces and the defaults are Elastic Beanstalk's published
/// configuration-option reference ("General options for all environments").
///
/// Two of the defaults are NOT written out here: `MinSize` is
/// [`DEFAULT_MIN_SIZE`] and `EnvironmentType` is
/// [`ENVIRONMENT_TYPE_LOAD_BALANCED`], the same constants the environment
/// resource derivation reads, so the catalog cannot publish a default the rest
/// of this service does not honour. The Auto Scaling sizes' `MaxValue` is
/// [`MAX_PUBLISHED_ASG_SIZE`], the bound the option reference states for them.
pub const GENERAL_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    // aws:autoscaling:asg — "the minimum number of instances you want in your
    // Auto Scaling group" and the maximum, plus where they may be launched.
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        "Availability Zones",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "Any",
    )
    .with_values(&["Any", "Any 1", "Any 2", "Any 3"]),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        "Cooldown",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "360",
    ),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        "MaxSize",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "4",
    )
    .with_range(0, MAX_PUBLISHED_ASG_SIZE),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        OPTION_MIN_SIZE,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        DEFAULT_MIN_SIZE_VALUE,
    )
    .with_range(0, MAX_PUBLISHED_ASG_SIZE),
    // aws:autoscaling:launchconfiguration — the instances themselves.
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "DisableIMDSv1",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "EC2KeyName",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        "IamInstanceProfile",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "aws-elasticbeanstalk-ec2-role",
    )
    .with_max_length(500),
    // The single-valued form of the instance type an environment runs, which
    // the environment derivations read beside the Auto Scaling group's list
    // (`aws:ec2:instances` / `InstanceTypes`) — see
    // [`environment_instance_type`]. The default is the type an environment
    // that names none runs ([`DEFAULT_INSTANCE_TYPE`]), read from that one
    // constant so the published default cannot differ from what this service
    // launches.
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        OPTION_INSTANCE_TYPE,
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        DEFAULT_INSTANCE_TYPE,
    ),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "MonitoringInterval",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "5 minute",
    )
    .with_values(&["1 minute", "5 minute"]),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "RootVolumeSize",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .with_range(8, 1024),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "RootVolumeType",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .with_values(&["gp2", "gp3", "io1", "standard"]),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "SecurityGroups",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    // aws:autoscaling:updatepolicy:rollingupdate — how a configuration change
    // is rolled out across the group.
    ConfigurationOption::new(
        "aws:autoscaling:updatepolicy:rollingupdate",
        "RollingUpdateEnabled",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:autoscaling:updatepolicy:rollingupdate",
        "RollingUpdateType",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "Time",
    )
    .with_values(&["Health", "Immutable", "Time"]),
    // aws:ec2:instances — the instance types the group may launch.
    ConfigurationOption::new(
        "aws:ec2:instances",
        "EnableSpot",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:ec2:instances",
        "InstanceTypes",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    // "The architectures of the EC2 instance types" — several selections from
    // one value set, which is what the modelled `List` value type is for.
    ConfigurationOption::new(
        "aws:ec2:instances",
        "SupportedArchitectures",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "x86_64",
    )
    .multi_valued()
    .with_values(&["arm64", "x86_64"]),
    // aws:ec2:vpc — where the environment runs.
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "AssociatePublicIpAddress",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "ELBScheme",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "public",
    )
    .with_values(&["internal", "public"]),
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "ELBSubnets",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "Subnets",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new("aws:ec2:vpc", "VPCId", CHANGE_SEVERITY_RESTART_ENVIRONMENT, "")
        .with_max_length(255),
    // aws:elasticbeanstalk:application — the application's health check path.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:application",
        "Application Healthcheck URL",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_max_length(255),
    // aws:elasticbeanstalk:cloudwatch:logs — instance log streaming.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs",
        "DeleteOnTerminate",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs",
        "RetentionInDays",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "7",
    )
    .with_values(LOG_RETENTION_IN_DAYS),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs",
        "StreamLogs",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    // aws:elasticbeanstalk:cloudwatch:logs:health — environment health
    // streaming, a second log group with its own retention.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "DeleteOnTerminate",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "HealthStreamingEnabled",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "RetentionInDays",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "7",
    )
    .with_values(LOG_RETENTION_IN_DAYS),
    // aws:elasticbeanstalk:command — how an application version is deployed.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:command",
        "BatchSize",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "100",
    )
    .with_range(1, 100),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:command",
        "BatchSizeType",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "Percentage",
    )
    .with_values(&["Fixed", "Percentage"]),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:command",
        "DeploymentPolicy",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "AllAtOnce",
    )
    .with_values(&[
        "AllAtOnce",
        "Immutable",
        "Rolling",
        "RollingWithAdditionalBatch",
        "TrafficSplitting",
    ]),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:command",
        "IgnoreHealthCheck",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:command",
        "Timeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "600",
    )
    .with_range(1, 3600),
    // aws:elasticbeanstalk:environment — the environment itself.
    ConfigurationOption::new(
        NAMESPACE_ENVIRONMENT,
        OPTION_ENVIRONMENT_TYPE,
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        ENVIRONMENT_TYPE_LOAD_BALANCED,
    )
    .with_values(&[ENVIRONMENT_TYPE_LOAD_BALANCED, ENVIRONMENT_TYPE_SINGLE_INSTANCE]),
    ConfigurationOption::new(
        NAMESPACE_ENVIRONMENT,
        "LoadBalancerType",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "classic",
    )
    .with_values(&["application", "classic", "network"]),
    ConfigurationOption::new(
        NAMESPACE_ENVIRONMENT,
        "ServiceRole",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "aws-elasticbeanstalk-service-role",
    )
    .with_max_length(500),
    // aws:elasticbeanstalk:healthreporting:system — basic or enhanced health.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:healthreporting:system",
        "HealthCheckSuccessThreshold",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "Ok",
    )
    .with_values(&["Degraded", "Ok", "Severe", "Warning"]),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:healthreporting:system",
        "SystemType",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "basic",
    )
    .with_values(&["basic", "enhanced"]),
    // aws:elasticbeanstalk:hostmanager — instance log publication to Amazon S3.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:hostmanager",
        "LogPublicationControl",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    // aws:elasticbeanstalk:managedactions — managed platform updates, the
    // window they run in and the role that runs them.
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS,
        OPTION_MANAGED_ACTIONS_ENABLED,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        BOOLEAN_FALSE,
    )
    .boolean(),
    // "PreferredStartTime: configure a maintenance window for managed actions
    // in UTC", stated as `day:hour:minute` (`Tue:09:00`) — a string option whose
    // form IS its restriction, so the catalog publishes it as one.
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS,
        OPTION_PREFERRED_START_TIME,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_regex(PREFERRED_START_TIME_PATTERN, "DayOfWeekAndTime"),
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS,
        "ServiceRoleForManagedUpdates",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_max_length(500),
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE,
        OPTION_INSTANCE_REFRESH_ENABLED,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        BOOLEAN_FALSE,
    )
    .boolean(),
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE,
        OPTION_UPDATE_LEVEL,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_values(&[UPDATE_LEVEL_MINOR, UPDATE_LEVEL_PATCH]),
    // aws:elasticbeanstalk:monitoring — what the service does with an instance
    // enhanced health reports as failing.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:monitoring",
        "Automatically Terminate Unhealthy Instances",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "true",
    )
    .boolean(),
    // aws:elasticbeanstalk:sns:topics — environment event notifications, which
    // this service really publishes (see [`super::event_notifications`]); the
    // namespace and the option names are that module's, so the catalog and the
    // delivery cannot disagree about what they are called.
    ConfigurationOption::new(
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_ENDPOINT,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_PROTOCOL,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "email",
    )
    .with_values(&["email", "email-json", "http", "https", "sqs"]),
    // The topic the service creates for those notifications: it reports the ARN
    // it created, so this one is not a value a caller sets.
    ConfigurationOption::new(
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_TOPIC_ARN,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .service_owned(),
    ConfigurationOption::new(
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_TOPIC_NAME,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_max_length(255),
    // aws:elasticbeanstalk:xray — the X-Ray daemon.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:xray",
        "XRayEnabled",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    // aws:elb:healthcheck / aws:elb:loadbalancer — the classic load balancer,
    // which is what `LoadBalancerType`'s default above provisions.
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "HealthyThreshold",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "3",
    )
    .with_range(2, 10),
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "Interval",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "10",
    )
    .with_range(5, 300),
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "Timeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "5",
    )
    .with_range(2, 60),
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "UnhealthyThreshold",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "5",
    )
    .with_range(2, 10),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "CrossZone",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "true",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "SecurityGroups",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
];

/// The upper bound the option reference states for both Auto Scaling group
/// sizes: "Maximum number of instances you want running. Must be a positive
/// integer that is less than 10000", and `MinSize` "no greater than MaxSize".
/// It is the `MaxValue` the published description of each carries.
///
/// It is NOT [`MAX_ENVIRONMENT_INSTANCES`], the number of instances one
/// environment's stored inventory holds here: that bound is this emulator's own
/// (a larger `MinSize` is held down to it rather than materialised), so
/// publishing it as the option's restriction would tell a caller the service
/// refuses a group size the real one accepts.
pub const MAX_PUBLISHED_ASG_SIZE: i32 = 10000;

/// `aws:autoscaling:asg` / `MinSize`'s default as the catalog publishes it — the
/// option value [`DEFAULT_MIN_SIZE`] is the count of, checked against it below
/// so the two cannot drift apart.
pub(super) const DEFAULT_MIN_SIZE_VALUE: &str = "1";
const _: () = assert!(DEFAULT_MIN_SIZE == 1, "MinSize's published default is DEFAULT_MIN_SIZE");

/// The options the Amazon SQS daemon of a WORKER environment defines
/// (`aws:elasticbeanstalk:sqsd`): the queue it reads and how it hands each
/// message to the application.
///
/// A platform version defines these only where its branch supports the worker
/// tier — the same rule [`managed_supported_addons`] applies to the
/// [`ADDON_WORKER_DAEMON_SQSD`] addition, read from the branch's
/// `supported_tiers` so the two cannot disagree about which platforms run the
/// daemon.
pub const WORKER_TIER_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "ErrorVisibilityTimeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "2",
    )
    .with_range(0, 43200),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "HttpConnections",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "50",
    )
    .with_range(1, 100),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "HttpPath",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "/",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "InactivityTimeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "299",
    )
    .with_range(1, 36000),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "MaxRetries",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "10",
    )
    .with_range(0, 100),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "MimeType",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "application/json",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "RetentionPeriod",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "345600",
    )
    .with_range(60, 1209600),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "VisibilityTimeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "300",
    )
    .with_range(0, 43200),
    // "WorkerQueueURL: the URL of the queue … If not specified, Elastic
    // Beanstalk creates the queue" — which is what this service's own
    // [`EnvironmentResources`] provisioning does, so the value is the
    // service's rather than the caller's.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "WorkerQueueURL",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .service_owned(),
];

/// The web server the Linux platforms proxy through, where the platform offers
/// a choice of two ("ProxyServer: nginx or apache").
pub(super) const PROXY_NGINX_OR_APACHE: ConfigurationOption = ConfigurationOption::new(
    NAMESPACE_PROXY,
    OPTION_PROXY_SERVER,
    CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
    "nginx",
)
.with_values(&["apache", "nginx"]);

/// The same option on a platform that proxies through nginx or not at all.
pub(super) const PROXY_NGINX_OR_NONE: ConfigurationOption = ConfigurationOption::new(
    NAMESPACE_PROXY,
    OPTION_PROXY_SERVER,
    CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
    "nginx",
)
.with_values(&["nginx", "none"]);

/// The options a Python platform version defines beyond the general catalog.
pub(super) const PYTHON_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_APACHE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:python",
        "NumProcesses",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "1",
    )
    .with_range(1, 1000),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:python",
        "NumThreads",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "15",
    )
    .with_range(1, 1000),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:python",
        "WSGIPath",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "application",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:python:staticfiles",
        "/static",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "static",
    )
    .with_max_length(255),
];

/// The options a PHP platform version defines beyond the general catalog: the
/// `php.ini` directives the platform sets.
pub(super) const PHP_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_APACHE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "allow_url_fopen",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "On",
    )
    .with_values(&["Off", "On"]),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "composer_options",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "",
    )
    .with_max_length(200),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "display_errors",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "Off",
    )
    .with_values(&["Off", "On"]),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "document_root",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "",
    )
    .with_max_length(255),
    // "max_execution_time" is a count of seconds; the directive's own upper
    // bound is far outside `OptionRestrictionMaxValue`'s integer range, so the
    // catalog publishes the default and no range rather than a bound the model
    // cannot carry.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "max_execution_time",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "60",
    ),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "memory_limit",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "256M",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "zlib.output_compression",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "Off",
    )
    .with_values(&["Off", "On"]),
];

/// The options a Ruby platform version defines beyond the general catalog.
pub(super) const RUBY_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_APACHE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:ruby",
        "RackEnv",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "production",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:ruby",
        "RailsEnv",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "production",
    )
    .with_max_length(255),
];

/// The options a Node.js platform version defines beyond the general catalog.
pub(super) const NODEJS_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_NONE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:nodejs",
        "NodeCommand",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "",
    )
    .with_max_length(255),
];

/// The options a Tomcat platform version defines beyond the general catalog:
/// the JVM the application server runs in.
pub(super) const TOMCAT_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_APACHE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "JVM Options",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "Xms",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "256m",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "Xmx",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "256m",
    )
    .with_max_length(255),
];

/// The options a .NET-on-Windows platform version defines beyond the general
/// catalog: the IIS application pool the application runs in. A Windows
/// platform proxies through IIS itself, so it defines no `ProxyServer` option.
pub(super) const DOTNET_WINDOWS_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:dotnet:apppool",
        "Enable 32-bit Applications",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:dotnet:apppool",
        "Target Runtime",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "4.0",
    )
    .with_values(&["2.0", "4.0"]),
];

/// The options a platform version defines because of the RUNTIME it runs,
/// by the [`SolutionStack::platform_category`] the catalog publishes it under.
///
/// This is the one place a platform-specific namespace is stated: the
/// `aws:elasticbeanstalk:container:<runtime>` namespaces exist only on the
/// platform whose application server they configure, and the proxy option's
/// value set follows the platform too (the Python, PHP and Ruby platforms
/// proxy through nginx or apache; the rest through nginx or nothing). A
/// category not listed here — Go and .NET on Linux run the application
/// directly behind the proxy, Docker runs whatever the image holds — defines
/// the proxy option alone.
pub(super) const PLATFORM_CATEGORY_CONFIGURATION_OPTIONS: &[(&str, &[ConfigurationOption])] = &[
    (".NET", DOTNET_WINDOWS_CONFIGURATION_OPTIONS),
    ("Node.js", NODEJS_CONFIGURATION_OPTIONS),
    ("PHP", PHP_CONFIGURATION_OPTIONS),
    ("Python", PYTHON_CONFIGURATION_OPTIONS),
    ("Ruby", RUBY_CONFIGURATION_OPTIONS),
    ("Tomcat", TOMCAT_CONFIGURATION_OPTIONS),
];

/// The options every OTHER Linux platform category defines: the proxy choice
/// its web tier offers. See [`PLATFORM_CATEGORY_CONFIGURATION_OPTIONS`].
pub(super) const LINUX_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[PROXY_NGINX_OR_NONE];

/// The configuration options the platform version `stack` defines, in a stable
/// order (by namespace, then by option name) so two reads of one platform
/// report the same catalog in the same sequence.
///
/// The three sources are the general catalog every platform defines, the
/// options the platform's RUNTIME adds, and the worker daemon's options, which
/// a platform defines only where it supports that tier (`supports_worker_tier`,
/// read from the branch catalog). A platform version this service cannot place
/// in its catalog — a CUSTOM one, whose runtime is inside a platform definition
/// archive this emulator does not read — defines the general catalog alone,
/// rather than a runtime's options guessed from nothing.
pub fn published_configuration_options(
    stack: Option<&SolutionStack>,
    supports_worker_tier: bool,
) -> Vec<&'static ConfigurationOption> {
    let runtime = stack
        .map(|stack| {
            PLATFORM_CATEGORY_CONFIGURATION_OPTIONS
                .iter()
                .find(|(category, _)| *category == stack.platform_category)
                .map_or(LINUX_CONFIGURATION_OPTIONS, |(_, options)| *options)
        })
        .unwrap_or_default();
    let worker = if supports_worker_tier {
        WORKER_TIER_CONFIGURATION_OPTIONS
    } else {
        &[]
    };
    let mut options: Vec<&'static ConfigurationOption> = GENERAL_CONFIGURATION_OPTIONS
        .iter()
        .chain(runtime)
        .chain(worker)
        .collect();
    options.sort_by_key(|option| option.key());
    options
}

/// The platform version a configuration set is FOR, from the two forms it is
/// named by: a platform version is named BOTH by its solution stack name and by
/// its ARN, and this service's catalog publishes both, so either form resolves
/// the one published version. A version the catalog does not publish — a CUSTOM
/// one — resolves to none, which is what leaves its catalog the general one.
///
/// The ONE place that resolution is written: every operation that has to know
/// which platform version a stored configuration set belongs to (the option
/// catalog it publishes, the defaults its settings resolve against) reads the
/// same answer.
pub fn configuration_platform(
    region: &str,
    solution_stack_name: Option<&str>,
    platform_arn: Option<&str>,
) -> Option<&'static SolutionStack> {
    // The option catalog is the BRANCH's: every version of a branch publishes
    // the same configuration options, so the branch's catalog row answers for
    // whichever of its versions the request named.
    named_platform_version(region, solution_stack_name, platform_arn).map(|version| version.stack)
}

/// The managed platform version a request names, by either of the two names a
/// platform version has: its `SolutionStackName` or its `PlatformArn`.
///
/// The ONE place that resolution is written, so every operation that reports
/// BOTH names for the version an environment or a configuration runs reports two
/// names of the SAME version — a request naming the release a branch's current
/// version superseded gets that release's solution stack name back, not the
/// newer one's. A `SolutionStackName` names the version
/// `ListAvailableSolutionStacks` offers under it; an ARN names whichever of the
/// branch's published versions it belongs to.
pub fn named_platform_version(
    region: &str,
    solution_stack_name: Option<&str>,
    platform_arn: Option<&str>,
) -> Option<ManagedPlatformVersion> {
    solution_stack_name
        .and_then(solution_stack_named)
        .map(|stack| ManagedPlatformVersion {
            stack,
            platform_version: stack.platform_version.to_string(),
        })
        .or_else(|| platform_arn.and_then(|arn| managed_platform_version(region, arn)))
}

/// The option settings a configuration SET holds, RESOLVED: the option catalog
/// `catalog` publishes at its default values, with the settings `written` on the
/// configuration itself applied over them.
///
/// Elastic Beanstalk resolves a configuration through a layered chain — the
/// platform version's defaults, then the saved configuration, then the settings
/// set on the environment — so the configuration set a read reports is the
/// resolved one: it carries options nobody wrote, each at the `DefaultValue`
/// `DescribeConfigurationOptions` publishes for the same (`Namespace`,
/// `OptionName`) on that platform version, and an option the configuration DOES
/// set at the value it was set to.
///
/// An option the catalog defines no default for is in force at no value, so it
/// is reported only where the configuration sets it; an option the catalog does
/// not define at all — an application environment variable, which the caller
/// names — is reported because the configuration sets it.
///
/// This is why the WRITTEN settings are what the store keeps: the override an
/// `OptionsToRemove` takes out stops replacing the default, so that option
/// resolves to the inherited default again rather than disappearing from the
/// set.
pub fn resolved_option_settings(
    catalog: &[&'static ConfigurationOption],
    written: &[ConfigurationOptionSetting],
) -> Vec<ConfigurationOptionSetting> {
    let defaults: Vec<ConfigurationOptionSetting> = catalog
        .iter()
        .filter(|option| !option.default_value.is_empty())
        .map(|option| ConfigurationOptionSetting {
            // "A unique resource name for the configuration option setting" —
            // only a time-based scaling option is set on a resource of its own,
            // and a catalog default is set on none.
            resource_name: None,
            namespace: Some(option.namespace.to_string()),
            option_name: Some(option.name.to_string()),
            value: Some(option.default_value.to_string()),
        })
        .collect();
    merge_option_settings(&defaults, written)
}

/// `OptionSource$Origin` (and `$Identifier`) for a value in force because the
/// PLATFORM VERSION defines that default — nobody set it.
///
/// RECORDED BEHAVIOUR, not modelled: the awsQuery
/// `ConfigurationSettingsDescription` carries one `OptionSettingDetails` entry
/// per described option, naming where its effective value came from, and the
/// recorded entries carry `Service` for a platform default and `API` for a
/// value a request set.
pub const OPTION_SOURCE_SERVICE: &str = "Service";
/// `OptionSource$Origin` (and `$Identifier`) for a value the configuration set
/// itself holds — one a request wrote through the API. See
/// [`OPTION_SOURCE_SERVICE`].
pub const OPTION_SOURCE_API: &str = "API";

/// Where each option of a resolved configuration set got its effective value:
/// one entry per setting [`resolved_option_settings`] reports, in the same
/// order, so the two lists line up position for position.
///
/// The source is decided by the layer the value came from: an option the
/// configuration set WROTE is in force because a request set it ([`
/// OPTION_SOURCE_API`]), and one in force only because the platform version
/// defines that default is the SERVICE's ([`OPTION_SOURCE_SERVICE`]). This is
/// the only thing that tells a value a caller set apart from an inherited
/// default, which is what a client reads to confirm a setting it applied took
/// effect.
pub fn resolved_option_setting_details(
    catalog: &[&'static ConfigurationOption],
    written: &[ConfigurationOptionSetting],
) -> Vec<OptionSettingDetail> {
    let set_through_the_api: BTreeSet<(String, String, String)> =
        written.iter().map(option_setting_key).collect();
    resolved_option_settings(catalog, written)
        .iter()
        .map(|setting| {
            let origin = if set_through_the_api.contains(&option_setting_key(setting)) {
                OPTION_SOURCE_API
            } else {
                OPTION_SOURCE_SERVICE
            };
            OptionSettingDetail {
                namespace: setting.namespace.clone(),
                option_name: setting.option_name.clone(),
                option_source: Some(OptionSource {
                    identifier: Some(origin.to_string()),
                    origin: Some(origin.to_string()),
                }),
            }
        })
        .collect()
}
