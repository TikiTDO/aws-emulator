// SPDX-License-Identifier: CC0-1.0

//! `CreateConfigurationTemplate` and `DescribeConfigurationSettings`, driven end
//! to end over the service's awsQuery HTTP surface.
//!
//! The rules under test:
//!
//! * a configuration template is the configuration the request names, PERSISTED
//!   under its `(ApplicationName, TemplateName)` pair: the platform version it
//!   is for (reported as both a `SolutionStackName` and a `PlatformArn` whenever
//!   the service's own catalogue publishes the other form), the option settings
//!   it inherits from what it is based on with the request's `OptionSettings`
//!   applied over them, and never an environment — "templates aren't associated
//!   with any environment. The `EnvironmentName` response element is always
//!   null";
//! * the same values come back from `DescribeConfigurationSettings` and the
//!   template's name from `DescribeApplications`, so the answer is the store's
//!   content and not a projection of the request;
//! * a request that names no configuration source, names two contradictory
//!   ones, or names a resource the store does not hold is refused with this
//!   service's own error code for that class.
//!
//! What a configuration set REPORTS is the resolved set — the platform version's
//! published option catalogue at its defaults with the configuration's own
//! settings over them, which `all/configuration_resolution.rs` is the suite for
//! — so the rows below read the settings a write put in force through
//! [`overridden_options`] rather than restating that catalogue.
//!
//! Each table row is a different point in those classes — which source the
//! configuration came from, how many option settings it carries, whether an
//! option is overridden, whether a description or tags were supplied — not a
//! renamed copy of the row before it.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns:
//! nothing here reaches into the store directly, so a create that does not
//! persist, or a describe answering from anything but the store, fails these
//! tests.

use crate::lifecycle::{account, Answer, Emulator, TEST_REGION};
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every template below is created under.
const APPLICATION: &str = "template-app";

/// One option setting, as the rows write them: `(Namespace, OptionName, Value,
/// ResourceName)`. `ResourceName` is empty where the request omits it — "a
/// unique resource name for the option setting", which only a time-based scaling
/// option carries.
///
/// `pub(crate)` along with [`option_params`], [`expected_options`] and
/// [`reported_options`] below: the `update_environment` suite in this same test
/// target sends and reads option settings over the same wire, so it drives them
/// through these rather than keeping a second encoder and reader of its own.
pub(crate) type Option4 = (&'static str, &'static str, &'static str, &'static str);

/// Borrow an owned parameter list for [`Emulator::call`].
pub(crate) fn as_params(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The awsQuery form parameters that send `settings` as the request's
/// `OptionSettings` list (`OptionSettings.member.<n>.<Member>`: the list's
/// member is unnamed in the model, so it serializes as `member`).
///
/// Takes a slice of plain string tuples rather than [`Option4`] itself, so a
/// suite whose row derives a value at run time (from the catalogue the service
/// publishes, say) sends it through this encoder too; a `&[Option4]` is accepted
/// unchanged.
pub(crate) fn option_params(settings: &[(&str, &str, &str, &str)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (namespace, option_name, value, resource_name)) in settings.iter().enumerate() {
        let position = index + 1;
        params.push((
            format!("OptionSettings.member.{position}.Namespace"),
            (*namespace).to_string(),
        ));
        params.push((
            format!("OptionSettings.member.{position}.OptionName"),
            (*option_name).to_string(),
        ));
        params.push((
            format!("OptionSettings.member.{position}.Value"),
            (*value).to_string(),
        ));
        if !resource_name.is_empty() {
            params.push((
                format!("OptionSettings.member.{position}.ResourceName"),
                (*resource_name).to_string(),
            ));
        }
    }
    params
}

/// The awsQuery form parameters that send `options` as a request's
/// `OptionsToRemove` list: `(Namespace, OptionName, ResourceName)`, the three
/// members `OptionSpecification` names one option by, with an empty
/// `ResourceName` left off the request.
///
/// `pub(crate)` beside [`option_params`]: `UpdateEnvironment` and
/// `UpdateConfigurationTemplate` both take the list, and the suites for them
/// send it through this one encoder.
pub(crate) fn remove_params(options: &[(&str, &str, &str)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (namespace, option_name, resource_name)) in options.iter().enumerate() {
        let at = index + 1;
        params.push((
            format!("OptionsToRemove.member.{at}.Namespace"),
            (*namespace).to_string(),
        ));
        params.push((
            format!("OptionsToRemove.member.{at}.OptionName"),
            (*option_name).to_string(),
        ));
        if !resource_name.is_empty() {
            params.push((
                format!("OptionsToRemove.member.{at}.ResourceName"),
                (*resource_name).to_string(),
            ));
        }
    }
    params
}

/// The option settings an answer reports, as `(Namespace, OptionName, Value,
/// ResourceName)` in the order the document lists them. Read member by member,
/// so an answer that mixes up which value belongs to which option fails rather
/// than matching four independent flat lists.
pub(crate) fn reported_options(answer: &Answer) -> Vec<(String, String, String, String)> {
    answer
        .elements_at("OptionSettings/member")
        .iter()
        .map(|member| {
            (
                member.value("Namespace").unwrap_or_default().to_string(),
                member.value("OptionName").unwrap_or_default().to_string(),
                member.value("Value").unwrap_or_default().to_string(),
                member.value("ResourceName").unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `settings` in the same shape [`reported_options`] returns, SORTED by the key
/// an option is identified by — the order [`overridden_options`] answers in,
/// because a resolved configuration set lists the platform catalogue's options
/// in the catalogue's own order rather than in the order a request wrote them.
pub(crate) fn expected_options(
    settings: &[(&str, &str, &str, &str)],
) -> Vec<(String, String, String, String)> {
    let mut expected: Vec<(String, String, String, String)> = settings
        .iter()
        .map(|(namespace, option_name, value, resource_name)| {
            (
                (*namespace).to_string(),
                (*option_name).to_string(),
                (*value).to_string(),
                (*resource_name).to_string(),
            )
        })
        .collect();
    expected.sort();
    expected
}

/// The option settings `answer` reports that OVERRIDE the platform version's
/// published defaults: the resolved configuration set with every option still
/// sitting at the `DefaultValue` that version's catalogue gives for it taken
/// out, sorted like [`expected_options`].
///
/// A read of a configuration set reports the RESOLVED settings — the platform
/// version's option catalogue at its defaults with the configuration's own
/// settings over them, which `all/configuration_resolution.rs` is the suite for
/// — so a suite about what a write put IN FORCE reads the options whose value is
/// not the inherited default, rather than restating a catalogue of a hundred
/// options in every row. An option the catalogue does not define (an application
/// environment variable the caller names) is never a default, so it always
/// counts as an override, and neither is a setting that names a `ResourceName`
/// of its own.
///
/// WHICH catalogue is read is taken from the description itself: every
/// configuration set reports the platform version it is for, so a row that moves
/// a set to another platform version is filtered against THAT version's defaults
/// and needs to say nothing here. A setting written at exactly its platform's
/// default is not an override by this measure and so is invisible to it — the
/// resolution suite asserts the whole resolved set, which is where that case is
/// covered.
pub(crate) fn overridden_options(answer: &Answer) -> Vec<(String, String, String, String)> {
    let stack = answer
        .value("SolutionStackName")
        .and_then(eb::state::solution_stack_named);
    // Worker-tier options included, so an option is recognised at its default
    // whichever tier the configuration set is for.
    let catalog = eb::state::published_configuration_options(stack, true);
    let mut overrides: Vec<(String, String, String, String)> = reported_options(answer)
        .into_iter()
        .filter(|(namespace, option_name, value, resource_name)| {
            !resource_name.is_empty()
                || !catalog.iter().any(|option| {
                    option.namespace == namespace.as_str()
                        && option.name == option_name.as_str()
                        && option.default_value == value.as_str()
                })
        })
        .collect();
    overrides.sort();
    overrides
}

/// The service's own published solution stack at `index` — the catalogue
/// `ListAvailableSolutionStacks` reports (`eb::state::SOLUTION_STACKS`), so a row
/// cannot name a platform version the service does not publish.
pub(crate) fn published_stack(index: usize) -> &'static eb::state::SolutionStack {
    eb::state::SOLUTION_STACKS
        .get(index)
        .expect("the service publishes this many solution stacks")
}

/// The service's own published solution stack whose `platform_category` is
/// `category` — the same catalogue [`published_stack`] indexes into, looked up
/// by the key a suite that cares about the RUNTIME needs (the
/// `configuration_options` suite, whose rows are platform categories), so
/// neither suite keeps a catalogue lookup of its own.
pub(crate) fn published_stack_of_category(category: &str) -> &'static eb::state::SolutionStack {
    eb::state::SOLUTION_STACKS
        .iter()
        .find(|stack| stack.platform_category == category)
        .unwrap_or_else(|| panic!("the service publishes a {category} platform version"))
}

/// One row of the create table: the request's configuration source and option
/// settings, and the configuration the template must then hold.
struct TemplateCase {
    case: &'static str,
    template_name: &'static str,
    /// The request members beyond `ApplicationName` and `TemplateName`.
    params: Vec<(String, String)>,
    /// The option settings the request sends.
    settings: Vec<Option4>,
    /// What the stored template must report, which for a row whose source
    /// carries settings of its own is NOT the list the request sent.
    expected_settings: Vec<Option4>,
    expected_solution_stack: Option<String>,
    expected_platform_arn: Option<String>,
    expected_description: Option<&'static str>,
}

#[tokio::test]
async fn created_configuration_templates_are_derived_from_the_request_and_persisted() {
    let emulator = Emulator::new();
    created(&emulator, "CreateApplication", &[("ApplicationName", APPLICATION)]).await;

    // An environment with option settings of its own, so a row can base a
    // template on "an environment whose settings you want to use".
    let environment_stack = published_stack(1);
    let environment_settings: Vec<Option4> = vec![
        ("aws:autoscaling:asg", "MinSize", "2", ""),
        ("aws:elasticbeanstalk:application:environment", "STAGE", "qa", ""),
    ];
    let mut environment_params: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), "template-env".to_string()),
        ("SolutionStackName".to_string(), environment_stack.name()),
    ];
    environment_params.extend(option_params(&environment_settings));
    let environment = created(
        &emulator,
        "CreateEnvironment",
        &as_params(&environment_params),
    )
    .await;
    let environment_id = environment
        .value("EnvironmentId")
        .expect("CreateEnvironment answers with an EnvironmentId")
        .to_string();

    // Rows differ along what the rule generalises over: which of the four
    // configuration sources the request names, how many option settings it
    // sends, whether one of them overrides an inherited value, whether the same
    // option is sent twice, and whether a description was supplied.
    let base_stack = published_stack(0);
    let arn_stack = published_stack(2);
    let cases = vec![
        TemplateCase {
            case: "a solution stack and nothing else",
            template_name: "stack-only",
            params: vec![(
                "SolutionStackName".to_string(),
                base_stack.name(),
            )],
            settings: Vec::new(),
            expected_settings: Vec::new(),
            expected_solution_stack: Some(base_stack.name()),
            expected_platform_arn: Some(base_stack.platform_arn(TEST_REGION)),
            expected_description: None,
        },
        TemplateCase {
            case: "a solution stack with three option settings, one resource-scoped",
            template_name: "stack-with-options",
            params: vec![
                ("SolutionStackName".to_string(), base_stack.name()),
                ("Description".to_string(), "the base settings".to_string()),
            ],
            settings: vec![
                ("aws:autoscaling:asg", "MinSize", "3", ""),
                ("aws:autoscaling:asg", "MaxSize", "6", ""),
                ("aws:autoscaling:scheduledaction", "MinSize", "3", "nightly"),
            ],
            expected_settings: vec![
                ("aws:autoscaling:asg", "MinSize", "3", ""),
                ("aws:autoscaling:asg", "MaxSize", "6", ""),
                ("aws:autoscaling:scheduledaction", "MinSize", "3", "nightly"),
            ],
            expected_solution_stack: Some(base_stack.name()),
            expected_platform_arn: Some(base_stack.platform_arn(TEST_REGION)),
            expected_description: Some("the base settings"),
        },
        TemplateCase {
            // The platform version named by its ARN: the solution stack name is
            // resolved from the service's catalogue rather than left absent.
            case: "a managed platform version by ARN",
            template_name: "by-platform-arn",
            params: vec![(
                "PlatformArn".to_string(),
                arn_stack.platform_arn(TEST_REGION),
            )],
            settings: vec![("aws:elasticbeanstalk:environment", "EnvironmentType", "SingleInstance", "")],
            expected_settings: vec![("aws:elasticbeanstalk:environment", "EnvironmentType", "SingleInstance", "")],
            expected_solution_stack: Some(arn_stack.name()),
            expected_platform_arn: Some(arn_stack.platform_arn(TEST_REGION)),
            expected_description: None,
        },
        TemplateCase {
            // "Elastic Beanstalk uses the configuration values from the
            // specified configuration template to create a new configuration.
            // Values specified in OptionSettings override any values obtained
            // from the SourceConfiguration."
            case: "based on another template, overriding one option and adding one",
            template_name: "from-source",
            params: vec![
                (
                    "SourceConfiguration.ApplicationName".to_string(),
                    APPLICATION.to_string(),
                ),
                (
                    "SourceConfiguration.TemplateName".to_string(),
                    "stack-with-options".to_string(),
                ),
            ],
            settings: vec![
                ("aws:autoscaling:asg", "MinSize", "2", ""),
                ("aws:elasticbeanstalk:application:environment", "STAGE", "prod", ""),
            ],
            expected_settings: vec![
                // The inherited settings, with the overridden value replaced
                // and the new option added.
                ("aws:autoscaling:asg", "MinSize", "2", ""),
                ("aws:autoscaling:asg", "MaxSize", "6", ""),
                ("aws:autoscaling:scheduledaction", "MinSize", "3", "nightly"),
                ("aws:elasticbeanstalk:application:environment", "STAGE", "prod", ""),
            ],
            expected_solution_stack: Some(base_stack.name()),
            expected_platform_arn: Some(base_stack.platform_arn(TEST_REGION)),
            expected_description: None,
        },
        TemplateCase {
            // "The ID of an environment whose settings you want to use to create
            // the configuration template."
            case: "based on a running environment",
            template_name: "from-environment",
            params: vec![("EnvironmentId".to_string(), environment_id.clone())],
            settings: vec![("aws:autoscaling:asg", "MinSize", "5", "")],
            expected_settings: vec![
                ("aws:autoscaling:asg", "MinSize", "5", ""),
                ("aws:elasticbeanstalk:application:environment", "STAGE", "qa", ""),
            ],
            expected_solution_stack: Some(environment_stack.name()),
            expected_platform_arn: Some(environment_stack.platform_arn(TEST_REGION)),
            expected_description: None,
        },
        TemplateCase {
            // One option sent twice is ONE option: the later value is the one
            // the configuration holds.
            case: "the same option sent twice",
            template_name: "repeated-option",
            params: vec![("SolutionStackName".to_string(), base_stack.name())],
            settings: vec![
                ("aws:autoscaling:asg", "MinSize", "1", ""),
                ("aws:autoscaling:asg", "MinSize", "7", ""),
            ],
            expected_settings: vec![("aws:autoscaling:asg", "MinSize", "7", "")],
            expected_solution_stack: Some(base_stack.name()),
            expected_platform_arn: Some(base_stack.platform_arn(TEST_REGION)),
            expected_description: Some(""),
        },
    ];

    for case in &cases {
        let mut params: Vec<(String, String)> = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            ("TemplateName".to_string(), case.template_name.to_string()),
        ];
        if let Some(description) = case.expected_description {
            // An empty description is sent as an empty value, which is what the
            // "repeated-option" row carries.
            params.push(("Description".to_string(), description.to_string()));
        }
        params.extend(case.params.clone());
        params.extend(option_params(&case.settings));
        let answer = created(&emulator, "CreateConfigurationTemplate", &as_params(&params)).await;

        assert_template(&answer, case, "CreateConfigurationTemplate");

        // And the store answers the same way on a later read, through the
        // operation that reports a configuration set.
        let described = created(
            &emulator,
            "DescribeConfigurationSettings",
            &[
                ("ApplicationName", APPLICATION),
                ("TemplateName", case.template_name),
            ],
        )
        .await;
        let settings = described.elements_at("ConfigurationSettings/member");
        assert_eq!(
            settings.len(),
            1,
            "case {:?}: a template is ONE configuration set, got {}",
            case.case,
            described.body
        );
        assert_template(&settings[0], case, "DescribeConfigurationSettings");
    }

    // Every template is reported as its application's, in one list.
    let applications = emulator
        .call("DescribeApplications", &[("ApplicationNames.member.1", APPLICATION)])
        .await;
    let mut reported = applications.values_at("ConfigurationTemplates/member");
    reported.sort_unstable();
    let mut expected: Vec<&str> = cases.iter().map(|case| case.template_name).collect();
    expected.sort_unstable();
    assert_eq!(
        reported, expected,
        "case \"DescribeApplications\": the application must report every stored template, got {}",
        applications.body
    );
}

/// Assert one answer carries the configuration `case` describes, whichever
/// operation answered it.
fn assert_template(answer: &Answer, case: &TemplateCase, operation: &str) {
    let label = format!("{} via {operation}", case.case);
    assert_eq!(
        answer.value("ApplicationName"),
        Some(APPLICATION),
        "case {label:?}: unexpected ApplicationName in {}",
        answer.body
    );
    assert_eq!(
        answer.value("TemplateName"),
        Some(case.template_name),
        "case {label:?}: unexpected TemplateName in {}",
        answer.body
    );
    assert_eq!(
        answer.value("SolutionStackName").map(str::to_string),
        case.expected_solution_stack,
        "case {label:?}: unexpected SolutionStackName in {}",
        answer.body
    );
    assert_eq!(
        answer.value("PlatformArn").map(str::to_string),
        case.expected_platform_arn,
        "case {label:?}: unexpected PlatformArn in {}",
        answer.body
    );
    assert_eq!(
        answer.value("Description").map(str::to_string),
        case.expected_description
            .map(str::to_string)
            .filter(|description| !description.is_empty()),
        "case {label:?}: unexpected Description in {}",
        answer.body
    );
    // The settings the configuration PUT IN FORCE, read out of the resolved set
    // the answer reports.
    assert_eq!(
        overridden_options(answer),
        expected_options(&case.expected_settings),
        "case {label:?}: unexpected OptionSettings in {}",
        answer.body
    );
    // And each of them is in the WHOLE reported set at that value, so a setting
    // whose value happens to equal its platform default — invisible to the
    // override view above — is checked here too.
    let reported = reported_options(answer);
    for expected in expected_options(&case.expected_settings) {
        assert!(
            reported.contains(&expected),
            "case {label:?}: expected the setting {expected:?} in {}",
            answer.body
        );
    }
    // "Templates aren't associated with any environment. The EnvironmentName
    // response element is always null."
    assert_eq!(
        answer.value("EnvironmentName"),
        None,
        "case {label:?}: a template names no environment, got {}",
        answer.body
    );
    // A stored template IS the configuration set the saved configuration holds,
    // so the set a describe reports is the one in effect for it.
    assert_eq!(
        answer.value("DeploymentStatus"),
        Some(eb::state::DEPLOYMENT_STATUS_DEPLOYED),
        "case {label:?}: a stored template reports its configuration set as deployed, got {}",
        answer.body
    );
    let date_created = answer
        .value("DateCreated")
        .unwrap_or_else(|| panic!("case {label:?}: expected a DateCreated in {}", answer.body));
    assert_eq!(
        answer.value("DateUpdated"),
        Some(date_created),
        "case {label:?}: a template nothing has updated reports DateUpdated = DateCreated, got {}",
        answer.body
    );
}

/// An environment's own configuration set, which is the OTHER thing
/// `DescribeConfigurationSettings` reports: it names the environment rather than
/// a template, and reports the configuration as deployed.
#[tokio::test]
async fn an_environments_configuration_set_is_reported_as_the_deployed_one() {
    let emulator = Emulator::new();
    let case = "an environment's configuration set";
    created(&emulator, "CreateApplication", &[("ApplicationName", APPLICATION)]).await;

    let stack = published_stack(0);
    let settings: Vec<Option4> = vec![("aws:autoscaling:asg", "MaxSize", "6", "")];
    let mut params: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), "deployed-env".to_string()),
        ("SolutionStackName".to_string(), stack.name()),
        ("Description".to_string(), "the live environment".to_string()),
    ];
    params.extend(option_params(&settings));
    created(&emulator, "CreateEnvironment", &as_params(&params)).await;

    let described = created(
        &emulator,
        "DescribeConfigurationSettings",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", "deployed-env"),
        ],
    )
    .await;
    assert_eq!(
        described.value("ConfigurationSettings/member/EnvironmentName"),
        Some("deployed-env"),
        "case {case:?}: the configuration set must name the environment, got {}",
        described.body
    );
    assert_eq!(
        described.value("ConfigurationSettings/member/TemplateName"),
        None,
        "case {case:?}: an environment's configuration set is no template, got {}",
        described.body
    );
    assert_eq!(
        described.value("ConfigurationSettings/member/DeploymentStatus"),
        Some("deployed"),
        "case {case:?}: the deployed configuration must say so, got {}",
        described.body
    );
    assert_eq!(
        described.value("ConfigurationSettings/member/SolutionStackName"),
        Some(stack.name().as_str()),
        "case {case:?}: unexpected SolutionStackName, got {}",
        described.body
    );
    assert_eq!(
        overridden_options(&described),
        expected_options(&settings),
        "case {case:?}: the environment's own option settings must be reported, got {}",
        described.body
    );
}

/// A template's tags are the resource's tags: reachable by the configuration
/// template ARN the service mints for it, and writable through
/// `UpdateTagsForResource` like any other Elastic Beanstalk resource.
#[tokio::test]
async fn a_templates_tags_are_reachable_by_its_resource_arn() {
    let emulator = Emulator::new();
    let case = "a tagged configuration template";
    created(&emulator, "CreateApplication", &[("ApplicationName", APPLICATION)]).await;

    created(
        &emulator,
        "CreateConfigurationTemplate",
        &[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "tagged"),
            ("SolutionStackName", published_stack(0).name().as_str()),
            ("Tags.member.1.Key", "owner"),
            ("Tags.member.1.Value", "platform"),
        ],
    )
    .await;

    // Built through the service's OWN ARN form
    // (`eb::state::configuration_template_arn`), so the test cannot assert a
    // form the service does not mint, and through `account()` for the id the
    // emulator signs with.
    let arn = eb::state::configuration_template_arn(TEST_REGION, APPLICATION, "tagged");
    assert_eq!(
        arn,
        format!(
            "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:configurationtemplate/{APPLICATION}/tagged",
            account()
        ),
        "case {case:?}: unexpected configuration template ARN form"
    );
    let listed = created(&emulator, "ListTagsForResource", &[("ResourceArn", arn.as_str())]).await;
    assert_eq!(
        listed.value("ResourceArn"),
        Some(arn.as_str()),
        "case {case:?}: the answer must name the resource asked about, got {}",
        listed.body
    );
    assert_eq!(
        (
            listed.value("ResourceTags/member/Key"),
            listed.value("ResourceTags/member/Value")
        ),
        (Some("owner"), Some("platform")),
        "case {case:?}: the tags the create stored must come back, got {}",
        listed.body
    );

    created(
        &emulator,
        "UpdateTagsForResource",
        &[
            ("ResourceArn", arn.as_str()),
            ("TagsToAdd.member.1.Key", "tier"),
            ("TagsToAdd.member.1.Value", "web"),
            ("TagsToRemove.member.1", "owner"),
        ],
    )
    .await;
    let updated = created(&emulator, "ListTagsForResource", &[("ResourceArn", arn.as_str())]).await;
    assert_eq!(
        (
            updated.value("ResourceTags/member/Key"),
            updated.value("ResourceTags/member/Value")
        ),
        (Some("tier"), Some("web")),
        "case {case:?}: the tag update must be what the store now holds, got {}",
        updated.body
    );

    // A template ARN under an application that holds no such template still
    // resolves to nothing.
    let missing =
        eb::state::configuration_template_arn(TEST_REGION, APPLICATION, "never-created");
    let refused = emulator
        .call("ListTagsForResource", &[("ResourceArn", missing.as_str())])
        .await;
    assert_eq!(
        refused.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: an ARN naming no stored template must be refused, got {}",
        refused.body
    );
}

/// A template belongs to its application: "deletes the specified application
/// along with all associated versions and configurations".
#[tokio::test]
async fn templates_go_with_the_application_they_belong_to() {
    let emulator = Emulator::new();
    let case = "delete the application";
    created(&emulator, "CreateApplication", &[("ApplicationName", APPLICATION)]).await;
    created(
        &emulator,
        "CreateConfigurationTemplate",
        &[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "doomed"),
            ("SolutionStackName", published_stack(0).name().as_str()),
        ],
    )
    .await;

    // The create recorded its activity, which `DescribeEvents` reports under the
    // template the call was about.
    let events = created(
        &emulator,
        "DescribeEvents",
        &[("ApplicationName", APPLICATION), ("TemplateName", "doomed")],
    )
    .await;
    let messages = events.values_at("Events/member/Message");
    assert!(
        messages.contains(&"Created new Configuration Template: doomed"),
        "case {case:?}: the create must record the template it created, got {messages:?}"
    );

    created(
        &emulator,
        "DeleteApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let refused = emulator
        .call(
            "DescribeConfigurationSettings",
            &[("ApplicationName", APPLICATION), ("TemplateName", "doomed")],
        )
        .await;
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case {case:?}: the application's configurations go with it, got {}",
        refused.body
    );
}

/// One request the service must refuse, with the code it refuses it with.
struct RefusalCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    code: &'static str,
}

#[tokio::test]
async fn bad_configuration_requests_are_refused_with_the_services_own_codes() {
    let emulator = Emulator::new();
    created(&emulator, "CreateApplication", &[("ApplicationName", APPLICATION)]).await;
    let stack = published_stack(0);
    let other_stack = published_stack(1);
    created(
        &emulator,
        "CreateConfigurationTemplate",
        &[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "taken"),
            ("SolutionStackName", stack.name().as_str()),
        ],
    )
    .await;

    // Rows span the classes of refusal: a name already used, an identifier that
    // names nothing, an absent either/or parameter, two members that cannot be
    // used together, and two values that contradict each other.
    let cases = vec![
        RefusalCase {
            case: "a template name the application already holds",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "taken".to_string()),
                ("SolutionStackName".to_string(), stack.name()),
            ],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a template under an application that does not exist",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), "no-such-app".to_string()),
                ("TemplateName".to_string(), "orphan".to_string()),
                ("SolutionStackName".to_string(), stack.name()),
            ],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "no configuration source at all",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "sourceless".to_string()),
            ],
            code: "MissingParameter",
        },
        RefusalCase {
            case: "both a solution stack and a platform ARN",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "two-platforms".to_string()),
                ("SolutionStackName".to_string(), stack.name()),
                ("PlatformArn".to_string(), stack.platform_arn(TEST_REGION)),
            ],
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a source configuration naming no template",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "nameless-source".to_string()),
                (
                    "SourceConfiguration.ApplicationName".to_string(),
                    APPLICATION.to_string(),
                ),
            ],
            code: "MissingRequiredParameter",
        },
        RefusalCase {
            case: "a source template the store does not hold",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "no-source".to_string()),
                (
                    "SourceConfiguration.TemplateName".to_string(),
                    "never-created".to_string(),
                ),
            ],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a source template on a different solution stack",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "stack-mismatch".to_string()),
                (
                    "SourceConfiguration.TemplateName".to_string(),
                    "taken".to_string(),
                ),
                ("SolutionStackName".to_string(), other_stack.name()),
            ],
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "an environment id that names nothing",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "no-environment".to_string()),
                ("EnvironmentId".to_string(), "e-0000000000000".to_string()),
            ],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a platform ARN the catalogue does not publish",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "no-platform".to_string()),
                (
                    "PlatformArn".to_string(),
                    format!("arn:aws:elasticbeanstalk:{TEST_REGION}::platform/Fictional/1.0.0"),
                ),
            ],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a solution stack the catalogue does not publish",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "unpublished-stack".to_string()),
                (
                    "SolutionStackName".to_string(),
                    "64bit Fictional Linux v0.0.1 running Nothing".to_string(),
                ),
            ],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a configuration set named twice",
            action: "DescribeConfigurationSettings",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "taken".to_string()),
                ("EnvironmentName".to_string(), "some-env".to_string()),
            ],
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a configuration set not named at all",
            action: "DescribeConfigurationSettings",
            params: vec![("ApplicationName".to_string(), APPLICATION.to_string())],
            code: "MissingRequiredParameter",
        },
        RefusalCase {
            case: "a template the store does not hold",
            action: "DescribeConfigurationSettings",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "never-created".to_string()),
            ],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a template under the wrong application",
            action: "DescribeConfigurationSettings",
            params: vec![
                ("ApplicationName".to_string(), "no-such-app".to_string()),
                ("TemplateName".to_string(), "taken".to_string()),
            ],
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "an environment that was never launched",
            action: "DescribeConfigurationSettings",
            params: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), "never-env".to_string()),
            ],
            code: "InvalidParameterValue",
        },
    ];

    for case in &cases {
        let refused = emulator.call(case.action, &as_params(&case.params)).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400, got {} and body {}",
            case.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            case.code,
            "case {:?}: unexpected error code in {}",
            case.case,
            refused.body
        );
        assert!(
            !refused.error_message().is_empty(),
            "case {:?}: a refusal must say what to fix, got {}",
            case.case,
            refused.body
        );
    }

    // None of the refused creates may have been stored.
    let applications = emulator
        .call("DescribeApplications", &[("ApplicationNames.member.1", APPLICATION)])
        .await;
    assert_eq!(
        applications.values_at("ConfigurationTemplates/member"),
        vec!["taken"],
        "case \"refused creates\": a refused create must store nothing, got {}",
        applications.body
    );
}

/// The published configuration-template quota is what bounds the store, and
/// reaching it is what makes the modelled error reachable.
#[tokio::test]
async fn the_configuration_template_quota_is_enforced_with_the_modelled_error() {
    let emulator = Emulator::new();
    let case = "configuration template quota";
    created(&emulator, "CreateApplication", &[("ApplicationName", APPLICATION)]).await;
    let stack_name = published_stack(0).name();

    for index in 0..eb::state::MAX_CONFIGURATION_TEMPLATES {
        let name = format!("template-{index:04}");
        let params = [
            ("ApplicationName", APPLICATION),
            ("TemplateName", name.as_str()),
            ("SolutionStackName", stack_name.as_str()),
        ];
        let answer = emulator.call("CreateConfigurationTemplate", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 while filling the quota at {name}, got {} and body {}",
            answer.status, answer.body
        );
    }

    let over = emulator
        .call(
            "CreateConfigurationTemplate",
            &[
                ("ApplicationName", APPLICATION),
                ("TemplateName", "one-too-many"),
                ("SolutionStackName", stack_name.as_str()),
            ],
        )
        .await;
    assert_eq!(
        over.status, 400,
        "case {case:?}: expected HTTP 400, got {} and body {}",
        over.status, over.body
    );
    assert_eq!(
        over.error_code(),
        "TooManyConfigurationTemplatesException",
        "case {case:?}: unexpected error code in {}",
        over.body
    );
    let listed = emulator
        .call("DescribeApplications", &[("ApplicationNames.member.1", APPLICATION)])
        .await;
    assert_eq!(
        listed.values_at("ConfigurationTemplates/member").len(),
        eb::state::MAX_CONFIGURATION_TEMPLATES,
        "case {case:?}: the refused create must not have been stored, got {}",
        listed.body
    );
}

/// The option-setting bound — the published-catalogue-sized cap
/// `eb::state::MAX_CONFIGURATION_OPTION_SETTINGS` — asserted at the bound and one
/// past it, on the request's own list and on the list a request inherits plus
/// its own.
#[tokio::test]
async fn the_option_setting_bound_is_enforced_on_the_request_and_on_the_merged_result() {
    let emulator = Emulator::new();
    created(&emulator, "CreateApplication", &[("ApplicationName", APPLICATION)]).await;
    let stack_name = published_stack(0).name();
    let bound = eb::state::MAX_CONFIGURATION_OPTION_SETTINGS;

    // `bound` distinct options — distinct by the key an option is identified by,
    // so none of them merges into another.
    let names: Vec<String> = (0..=bound).map(|index| format!("OPT_{index:05}")).collect();
    let settings = |count: usize| -> Vec<(String, String)> {
        let mut params = Vec::new();
        for (index, name) in names.iter().take(count).enumerate() {
            let position = index + 1;
            params.push((
                format!("OptionSettings.member.{position}.Namespace"),
                "aws:elasticbeanstalk:application:environment".to_string(),
            ));
            params.push((
                format!("OptionSettings.member.{position}.OptionName"),
                name.clone(),
            ));
            params.push((
                format!("OptionSettings.member.{position}.Value"),
                "set".to_string(),
            ));
        }
        params
    };

    // The namespace the generated settings are written in: the one holding an
    // application's environment variables, which the caller names and the
    // published catalogue therefore defines no option of. Counting the reported
    // settings in THIS namespace is independent of what the catalogue's
    // defaults are.
    let generated_namespace = "aws:elasticbeanstalk:application:environment";
    let generated_count = |answer: &Answer| -> usize {
        reported_options(answer)
            .into_iter()
            .filter(|(namespace, _, _, _)| namespace == generated_namespace)
            .count()
    };

    // At the bound: accepted, and every setting stored.
    let mut at_bound: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), "at-the-bound".to_string()),
        ("SolutionStackName".to_string(), stack_name.clone()),
    ];
    at_bound.extend(settings(bound));
    let accepted = created(
        &emulator,
        "CreateConfigurationTemplate",
        &as_params(&at_bound),
    )
    .await;
    assert_eq!(
        generated_count(&accepted),
        bound,
        "case \"at the bound\": every option setting must be stored, got {}",
        accepted.body
    );

    // One past it: refused, and nothing stored.
    let mut over_bound: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), "over-the-bound".to_string()),
        ("SolutionStackName".to_string(), stack_name.clone()),
    ];
    over_bound.extend(settings(bound + 1));
    let refused = emulator
        .call("CreateConfigurationTemplate", &as_params(&over_bound))
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidParameterValue"),
        "case \"one past the bound\": expected a 400 InvalidParameterValue, got {}",
        refused.body
    );

    // Inheriting a configuration that is AT the bound and overriding one of its
    // options stays at the bound, so it is accepted …
    let overridden = created(
        &emulator,
        "CreateConfigurationTemplate",
        &[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "inherits-and-overrides"),
            ("SourceConfiguration.TemplateName", "at-the-bound"),
            (
                "OptionSettings.member.1.Namespace",
                "aws:elasticbeanstalk:application:environment",
            ),
            ("OptionSettings.member.1.OptionName", names[0].as_str()),
            ("OptionSettings.member.1.Value", "replaced"),
        ],
    )
    .await;
    assert_eq!(
        generated_count(&overridden),
        bound,
        "case \"inherit and override\": an overridden option is the same option, got {}",
        overridden.body
    );

    // … while adding one the source does not carry would take the merged
    // configuration past the bound, which is refused on the MERGED result.
    let merged_over = emulator
        .call(
            "CreateConfigurationTemplate",
            &[
                ("ApplicationName", APPLICATION),
                ("TemplateName", "inherits-and-adds"),
                ("SourceConfiguration.TemplateName", "at-the-bound"),
                (
                    "OptionSettings.member.1.Namespace",
                    "aws:elasticbeanstalk:application:environment",
                ),
                ("OptionSettings.member.1.OptionName", names[bound].as_str()),
                ("OptionSettings.member.1.Value", "one-too-many"),
            ],
        )
        .await;
    assert_eq!(
        (merged_over.status, merged_over.error_code()),
        (400, "InvalidParameterValue"),
        "case \"merged past the bound\": expected a 400 InvalidParameterValue, got {}",
        merged_over.body
    );

    let applications = emulator
        .call("DescribeApplications", &[("ApplicationNames.member.1", APPLICATION)])
        .await;
    let mut stored = applications.values_at("ConfigurationTemplates/member");
    stored.sort_unstable();
    assert_eq!(
        stored,
        vec!["at-the-bound", "inherits-and-overrides"],
        "case \"the option-setting bound\": a refused create must store nothing, got {}",
        applications.body
    );
}

/// Call `action` and require a 200, returning the answer: a precondition that
/// did not hold fails the test where it was set up rather than later.
///
/// `pub(crate)` beside [`published_stack`] and the option-setting helpers above:
/// the `configuration_options` suite drives its own preconditions through this
/// one rather than keeping a second copy of it.
pub(crate) async fn created(
    emulator: &Emulator,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}
