// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a not-found refusal, driven end to end over its
//! awsQuery HTTP surface with the shared harness in `crate::harness`.
//!
//! The rule under test: when a request names a resource that does not exist, the
//! refusal's `Error.Message` is the template the real service words THAT
//! operation's refusal in, built from the request alone — and no operation is
//! given another operation's template. Elastic Beanstalk uses more than one, and
//! which one an operation answers in is what the recorded transcripts settle:
//!
//! * most environment lookups: `No Environment found for <Member> = '<value>'.`,
//!   naming the member the caller used and the value it carried
//!   (`RestartAppServer`, `RebuildEnvironment`, `AbortEnvironmentUpdate`, the two
//!   operations-role calls, `ApplyEnvironmentManagedAction` and the two health
//!   reads);
//! * the environment-lifecycle pair `TerminateEnvironment` and
//!   `SwapEnvironmentCNAMEs`: `Environment not found with <Id|Name>: <value> in
//!   account '<account>'`, naming the KIND of identifier, its value and the
//!   account the lookup was made in — and, for the swap, leading with the role
//!   the request gave that environment (`source` / `destination`) instead of
//!   `Environment`;
//! * `DescribePlatformVersion`: `Platform '<PlatformArn>' does not exist.`
//! * every configuration-template lookup: `No Configuration Template named
//!   '<ApplicationName>/<TemplateName>' found.`, naming the template by the PAIR
//!   that identifies one (a template name "must be unique per application").
//!
//! The templates are written out HERE rather than imported from the service, so
//! a change to the service's builders that changes the wire wording fails these
//! tests instead of moving with them.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over:
//! which operation performed the lookup, which member named the resource, and
//! the form of the value it carried (shortest and longest the model allows, an
//! id-shaped value and one that is not, an ARN of each ownership form and a
//! value that is no ARN at all).

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::harness::{Emulator, PAST_DUE, SOLUTION_STACK};
use crate::lifecycle::{account, TEST_REGION};

/// The template most of this service's environment lookups word a missing
/// environment with: the input member the caller used and its value.
fn no_environment_found(member: &str, value: &str) -> String {
    format!("No Environment found for {member} = '{value}'.")
}

/// The template the environment-lifecycle pair words the same refusal with.
/// `role` is the part the request gave that environment when it named more than
/// one, and `kind` is `Id` or `Name` — whichever member the request used.
fn missing_environment_in_account(role: Option<&str>, kind: &str, value: &str) -> String {
    let subject = match role {
        Some(role) => format!("{role} environment"),
        None => "Environment".to_string(),
    };
    format!(
        "{subject} not found with {kind}: {value} in account '{}'",
        account()
    )
}

/// The template `DescribePlatformVersion` words a missing platform version with.
fn missing_platform(platform_arn: &str) -> String {
    format!("Platform '{platform_arn}' does not exist.")
}

/// The template every configuration-template lookup words a missing saved
/// configuration with: the `(ApplicationName, TemplateName)` pair that names
/// one, joined by `/`.
fn missing_configuration_template(application_name: &str, template_name: &str) -> String {
    format!("No Configuration Template named '{application_name}/{template_name}' found.")
}

/// The modelled minimum length of `EnvironmentName` (`len 4..40`), from the
/// member's own constraint in this service's Smithy model.
const ENVIRONMENT_NAME_MIN: usize = 4;
/// The modelled maximum length of `EnvironmentName` (`len 4..40`).
const ENVIRONMENT_NAME_MAX: usize = 40;

/// An operations role ARN, for the one operation that requires one beside the
/// environment it names.
const OPERATIONS_ROLE: &str = "arn:aws:iam::123456789012:role/beanstalk-ops";

/// One row of an environment table: the request sent, which identifier member
/// named the environment, and the value that member carried.
struct EnvironmentCase {
    case: &'static str,
    action: &'static str,
    /// Every member the request carries, the identifier among them.
    params: Vec<(&'static str, String)>,
    /// The identifier member the message has to report the lookup by.
    member: &'static str,
    /// The value that member carried.
    value: String,
}

/// Create an application with one environment, so a row can name a LIVE
/// environment beside an identifier that resolves to nothing.
async fn fixture(emulator: &Emulator, application: &str, environment: &str) {
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", application)])
            .await
            .status,
        200,
        "fixture {application:?}: expected 200 from CreateApplication"
    );
    assert_eq!(
        emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", application),
                    ("EnvironmentName", environment),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                ],
            )
            .await
            .status,
        200,
        "fixture {environment:?}: expected 200 from CreateEnvironment"
    );
}

/// Send every row and assert the refusal is 400 `InvalidParameterValue` with
/// `expected` as its message.
async fn assert_refusals(
    emulator: &Emulator,
    cases: &[EnvironmentCase],
    expected: impl Fn(&EnvironmentCase) -> String,
) {
    for row in cases {
        let case = row.case;
        let params: Vec<(&str, &str)> = row
            .params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let refused = emulator.call(row.action, &params).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            refused.body
        );
        let expected = expected(row);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }
}

#[tokio::test]
async fn an_environment_lookup_names_the_member_and_the_value_the_request_used() {
    let emulator = Emulator::new();
    fixture(&emulator, "wording-app", "live-env").await;

    let shortest_name = "g".repeat(ENVIRONMENT_NAME_MIN);
    let longest_name = "g".repeat(ENVIRONMENT_NAME_MAX);
    let cases = [
        EnvironmentCase {
            case: "RestartAppServer by a name nothing carries",
            action: "RestartAppServer",
            params: vec![("EnvironmentName", "ghost-env".to_string())],
            member: "EnvironmentName",
            value: "ghost-env".to_string(),
        },
        EnvironmentCase {
            case: "RestartAppServer by an id the service never issued",
            action: "RestartAppServer",
            params: vec![("EnvironmentId", "e-0000000000000".to_string())],
            member: "EnvironmentId",
            value: "e-0000000000000".to_string(),
        },
        EnvironmentCase {
            case: "RebuildEnvironment by a name at the modelled minimum length",
            action: "RebuildEnvironment",
            params: vec![("EnvironmentName", shortest_name.clone())],
            member: "EnvironmentName",
            value: shortest_name.clone(),
        },
        EnvironmentCase {
            case: "RebuildEnvironment by an id that is not id-shaped at all",
            action: "RebuildEnvironment",
            params: vec![("EnvironmentId", "not-an-id".to_string())],
            member: "EnvironmentId",
            value: "not-an-id".to_string(),
        },
        EnvironmentCase {
            case: "AbortEnvironmentUpdate by a name at the modelled maximum length",
            action: "AbortEnvironmentUpdate",
            params: vec![("EnvironmentName", longest_name.clone())],
            member: "EnvironmentName",
            value: longest_name.clone(),
        },
        EnvironmentCase {
            case: "DescribeEnvironmentHealth by a name nothing carries",
            action: "DescribeEnvironmentHealth",
            params: vec![
                ("EnvironmentName", "ghost-env".to_string()),
                ("AttributeNames.member.1", "All".to_string()),
            ],
            member: "EnvironmentName",
            value: "ghost-env".to_string(),
        },
        EnvironmentCase {
            case: "DescribeInstancesHealth by an id the service never issued",
            action: "DescribeInstancesHealth",
            params: vec![
                ("EnvironmentId", "e-fffffffffffff".to_string()),
                ("AttributeNames.member.1", "HealthStatus".to_string()),
            ],
            member: "EnvironmentId",
            value: "e-fffffffffffff".to_string(),
        },
        EnvironmentCase {
            case: "ApplyEnvironmentManagedAction by a name nothing carries",
            action: "ApplyEnvironmentManagedAction",
            params: vec![
                ("EnvironmentName", "ghost-env".to_string()),
                ("ActionId", "action-1".to_string()),
            ],
            member: "EnvironmentName",
            value: "ghost-env".to_string(),
        },
        EnvironmentCase {
            case: "AssociateEnvironmentOperationsRole by a name nothing carries",
            action: "AssociateEnvironmentOperationsRole",
            params: vec![
                ("EnvironmentName", "ghost-env".to_string()),
                ("OperationsRole", OPERATIONS_ROLE.to_string()),
            ],
            member: "EnvironmentName",
            value: "ghost-env".to_string(),
        },
        EnvironmentCase {
            case: "DisassociateEnvironmentOperationsRole by a name nothing carries",
            action: "DisassociateEnvironmentOperationsRole",
            params: vec![("EnvironmentName", "ghost-env".to_string())],
            member: "EnvironmentName",
            value: "ghost-env".to_string(),
        },
        EnvironmentCase {
            // Both members supplied: the id selects the environment, so the id is
            // what the message reports even though the name resolves.
            case: "RestartAppServer by an unknown id beside a live name",
            action: "RestartAppServer",
            params: vec![
                ("EnvironmentId", "e-0000000000000".to_string()),
                ("EnvironmentName", "live-env".to_string()),
            ],
            member: "EnvironmentId",
            value: "e-0000000000000".to_string(),
        },
    ];

    assert_refusals(&emulator, &cases, |row| {
        no_environment_found(row.member, &row.value)
    })
    .await;
}

#[tokio::test]
async fn terminate_environment_names_the_identifier_and_the_account() {
    let emulator = Emulator::new();
    fixture(&emulator, "terminate-wording", "live-env").await;

    let longest_name = "g".repeat(ENVIRONMENT_NAME_MAX);
    let cases = [
        EnvironmentCase {
            case: "TerminateEnvironment by a name nothing carries",
            action: "TerminateEnvironment",
            params: vec![("EnvironmentName", "ghost-env".to_string())],
            member: "Name",
            value: "ghost-env".to_string(),
        },
        EnvironmentCase {
            case: "TerminateEnvironment by a name at the modelled maximum length",
            action: "TerminateEnvironment",
            params: vec![("EnvironmentName", longest_name.clone())],
            member: "Name",
            value: longest_name.clone(),
        },
        EnvironmentCase {
            case: "TerminateEnvironment by an id the service never issued",
            action: "TerminateEnvironment",
            params: vec![("EnvironmentId", "e-0123456789abc".to_string())],
            member: "Id",
            value: "e-0123456789abc".to_string(),
        },
        EnvironmentCase {
            case: "TerminateEnvironment by an id that is not id-shaped at all",
            action: "TerminateEnvironment",
            params: vec![("EnvironmentId", "not-an-id".to_string())],
            member: "Id",
            value: "not-an-id".to_string(),
        },
        EnvironmentCase {
            // Both members supplied: the id selects the environment, so the id is
            // what the message reports even though the name resolves.
            case: "TerminateEnvironment by an unknown id beside a live name",
            action: "TerminateEnvironment",
            params: vec![
                ("EnvironmentId", "e-0000000000000".to_string()),
                ("EnvironmentName", "live-env".to_string()),
            ],
            member: "Id",
            value: "e-0000000000000".to_string(),
        },
        EnvironmentCase {
            // `ForceTerminate` and `TerminateResources` change nothing about the
            // lookup, so the same template answers a request that carries them.
            case: "TerminateEnvironment with ForceTerminate and TerminateResources",
            action: "TerminateEnvironment",
            params: vec![
                ("EnvironmentName", "ghost-env".to_string()),
                ("ForceTerminate", "true".to_string()),
                ("TerminateResources", "false".to_string()),
            ],
            member: "Name",
            value: "ghost-env".to_string(),
        },
    ];

    assert_refusals(&emulator, &cases, |row| {
        missing_environment_in_account(None, row.member, &row.value)
    })
    .await;
}

/// One row of the swap table: which side of the swap names nothing, by which
/// member, and the role the message has to lead with.
struct SwapCase {
    case: &'static str,
    params: Vec<(&'static str, String)>,
    /// `source` or `destination` — the role the message leads with.
    role: &'static str,
    /// `Id` or `Name` — the kind of identifier the message reports.
    kind: &'static str,
    value: String,
}

#[tokio::test]
async fn a_swap_names_the_side_of_the_missing_environment_and_the_account() {
    let emulator = Emulator::new();
    fixture(&emulator, "swap-wording", "settled-env").await;

    // Rows differ along which SIDE resolves to nothing, which member named it,
    // and whether the other side resolves — the source is resolved first, so a
    // request whose both sides are unknown reports the source.
    let cases = [
        SwapCase {
            case: "unknown source name, known destination",
            params: vec![
                ("SourceEnvironmentName", "ghost-env".to_string()),
                ("DestinationEnvironmentName", "settled-env".to_string()),
            ],
            role: "source",
            kind: "Name",
            value: "ghost-env".to_string(),
        },
        SwapCase {
            case: "known source name, unknown destination name",
            params: vec![
                ("SourceEnvironmentName", "settled-env".to_string()),
                ("DestinationEnvironmentName", "ghost-env".to_string()),
            ],
            role: "destination",
            kind: "Name",
            value: "ghost-env".to_string(),
        },
        SwapCase {
            case: "both names unknown: the source is reported",
            params: vec![
                ("SourceEnvironmentName", "ghost-one".to_string()),
                ("DestinationEnvironmentName", "ghost-two".to_string()),
            ],
            role: "source",
            kind: "Name",
            value: "ghost-one".to_string(),
        },
        SwapCase {
            case: "both sides named by unknown ids",
            params: vec![
                ("SourceEnvironmentId", "e-aaaaaaaaaaaaa".to_string()),
                ("DestinationEnvironmentId", "e-bbbbbbbbbbbbb".to_string()),
            ],
            role: "source",
            kind: "Id",
            value: "e-aaaaaaaaaaaaa".to_string(),
        },
        SwapCase {
            // The source carries an id as well as a name, and the id is what
            // selects a side, so the id is what the message reports.
            case: "a source named by both members, paired by id",
            params: vec![
                ("SourceEnvironmentName", "settled-env".to_string()),
                ("SourceEnvironmentId", "e-ccccccccccccc".to_string()),
                ("DestinationEnvironmentId", "e-ddddddddddddd".to_string()),
            ],
            role: "source",
            kind: "Id",
            value: "e-ccccccccccccc".to_string(),
        },
    ];

    for SwapCase {
        case,
        params,
        role,
        kind,
        value,
    } in &cases
    {
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let refused = emulator.call("SwapEnvironmentCNAMEs", &params).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            refused.body
        );
        let expected = missing_environment_in_account(Some(role), kind, value);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }
}

#[tokio::test]
async fn describe_platform_version_reports_the_arn_that_does_not_exist() {
    let emulator = Emulator::new();

    // Rows span the forms an unresolvable `PlatformArn` arrives in: a
    // service-owned ARN (no account id, the form a MANAGED version carries)
    // naming an unpublished platform, a published platform branch at a version
    // the catalog does not publish, a CUSTOM ARN of this account that was never
    // created (built through the service's own `platform_version_arn`, the form
    // `CreatePlatformVersion` answers with), the same ARN in another region, and
    // a value that is no ARN at all.
    let unpublished_branch = &eb::state::SOLUTION_STACKS[0].branch_name;
    let arns = [
        (
            "a service-owned ARN naming no published platform",
            format!("arn:aws:elasticbeanstalk:{TEST_REGION}::platform/Nonexistent-Platform/9.9.9"),
        ),
        (
            "a published platform branch at an unpublished version",
            format!("arn:aws:elasticbeanstalk:{TEST_REGION}::platform/{unpublished_branch}/9.9.9"),
        ),
        (
            "a custom platform ARN never created",
            eb::state::platform_version_arn(TEST_REGION, "ghost-platform", "1.0.0"),
        ),
        (
            "the same custom platform ARN in another region",
            eb::state::platform_version_arn("eu-west-1", "ghost-platform", "1.0.0"),
        ),
        ("a value that is no ARN at all", "ghost-platform".to_string()),
    ];

    for (case, arn) in &arns {
        let described = emulator
            .call("DescribePlatformVersion", &[("PlatformArn", arn)])
            .await;
        assert_eq!(
            described.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            described.body
        );
        let expected = missing_platform(arn);
        assert_eq!(
            described.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            described.error_message()
        );
    }
}

/// One row of the configuration-template table: the operation that resolves the
/// template, the request it is driven with, and the pair the message has to
/// name.
struct TemplateCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, String)>,
    application_name: String,
    template_name: String,
}

#[tokio::test]
async fn every_configuration_template_lookup_names_the_application_and_the_template() {
    let emulator = Emulator::new();
    let application = "template-messages";
    let other_application = "template-messages-other";
    for name in [application, other_application] {
        assert_eq!(
            emulator
                .call("CreateApplication", &[("ApplicationName", name)])
                .await
                .status,
            200,
            "fixture {name:?}: expected 200 from CreateApplication"
        );
    }
    // A template the OTHER application holds, so a row can name a real template
    // name under an application that does not hold it.
    assert_eq!(
        emulator
            .call(
                "CreateConfigurationTemplate",
                &[
                    ("ApplicationName", other_application),
                    ("TemplateName", "elsewhere"),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                ],
            )
            .await
            .status,
        200,
        "fixture \"elsewhere\": expected 200 from CreateConfigurationTemplate"
    );
    assert_eq!(
        emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", application),
                    ("EnvironmentName", "template-msg-env"),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                ],
            )
            .await
            .status,
        200,
        "fixture \"template-msg-env\": expected 200 from CreateEnvironment"
    );
    // The launch the clock owes, so the update below is refused by the template
    // it names rather than by a status the launch left behind.
    std::thread::sleep(PAST_DUE);

    // Rows differ along the operation that resolved the template and how the
    // pair misses: a template name no application holds, a template name its
    // owner's SIBLING application does not hold, and a source template named by
    // a create.
    let cases = vec![
        TemplateCase {
            case: "a launch from a template that does not exist",
            action: "CreateEnvironment",
            params: vec![
                ("ApplicationName", application.to_string()),
                ("EnvironmentName", "ghost-template-env".to_string()),
                ("TemplateName", "never-created".to_string()),
            ],
            application_name: application.to_string(),
            template_name: "never-created".to_string(),
        },
        TemplateCase {
            case: "an update deploying a template another application holds",
            action: "UpdateEnvironment",
            params: vec![
                ("EnvironmentName", "template-msg-env".to_string()),
                ("TemplateName", "elsewhere".to_string()),
            ],
            application_name: application.to_string(),
            template_name: "elsewhere".to_string(),
        },
        TemplateCase {
            case: "a read of a template that does not exist",
            action: "DescribeConfigurationSettings",
            params: vec![
                ("ApplicationName", application.to_string()),
                ("TemplateName", "never-created".to_string()),
            ],
            application_name: application.to_string(),
            template_name: "never-created".to_string(),
        },
        TemplateCase {
            case: "the option catalogue of a template that does not exist",
            action: "DescribeConfigurationOptions",
            params: vec![
                ("ApplicationName", application.to_string()),
                ("TemplateName", "never-created".to_string()),
            ],
            application_name: application.to_string(),
            template_name: "never-created".to_string(),
        },
        TemplateCase {
            case: "a write to a template that does not exist",
            action: "UpdateConfigurationTemplate",
            params: vec![
                ("ApplicationName", application.to_string()),
                ("TemplateName", "never-created".to_string()),
                ("Description", "no such configuration".to_string()),
            ],
            application_name: application.to_string(),
            template_name: "never-created".to_string(),
        },
        TemplateCase {
            case: "a create copying a source template another application holds",
            action: "CreateConfigurationTemplate",
            params: vec![
                ("ApplicationName", application.to_string()),
                ("TemplateName", "copied".to_string()),
                (
                    "SourceConfiguration.TemplateName",
                    "elsewhere".to_string(),
                ),
            ],
            application_name: application.to_string(),
            template_name: "elsewhere".to_string(),
        },
    ];

    for TemplateCase {
        case,
        action,
        params,
        application_name,
        template_name,
    } in &cases
    {
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let refused = emulator.call(action, &params).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            refused.body
        );
        let expected = missing_configuration_template(application_name, template_name);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }
}
