// SPDX-License-Identifier: CC0-1.0

//! How Elastic Beanstalk REFUSES a request: the envelope every refusal is stated in, the
//! order the checks run in, and the check each rejected value gets.
//!
//! Four rules are asserted here, each over a SET of inputs rather than one recorded
//! request:
//!
//! 1. ENVELOPE — every error this service answers with is an awsQuery `<ErrorResponse>`
//!    carrying a `<RequestId>`, a sibling AFTER `<Error>`, whichever layer produced the
//!    failure: a handler, a modelled error shape, the boundary validator, the scaffold's
//!    501, or the routing table. Each response reports its OWN id, and a successful
//!    response is left alone (its id is already in `ResponseMetadata`).
//! 2. ORDER — a parameter the request did not supply is reported BEFORE any resource the
//!    request names is resolved, so the answer states the input defect rather than
//!    blaming a resource: `CreateEnvironment` naming no configuration template, no
//!    solution stack and no platform version is refused for that, whether or not its
//!    application exists.
//! 3. ORDER — the resource a request names is resolved before any precondition that
//!    depends on what that resource's children carry: `ComposeEnvironments` reports an
//!    unknown application rather than a defect in a source bundle of an application the
//!    caller never created.
//! 4. VALUE — `ApplicationResourceLifecycleConfig$ServiceRole` is "the ARN of an IAM
//!    service role"; a request that call cannot be served with that member — a value
//!    that is not a role ARN, or none at all where the call needs one and the
//!    application carries none — is refused 400 `InvalidParameterValue` with a message
//!    naming the member and the constraint its value failed, never with a sentence that
//!    only says the property is required. The rule holds on both calls the model says
//!    support it (`CreateApplication` and `UpdateApplicationResourceLifecycle`).
//! 5. MESSAGES of the tag refusals: the modelled `TooManyTagsException` states the quota
//!    a resource has, whichever check reached it, and a refusal naming a caller-supplied
//!    ARN quotes it.
//!
//! The HTTP harness (the router, the awsQuery form encoder and the XML element reader) is
//! the one `lifecycle.rs` owns in this same test binary, so this suite adds no second copy.

use crate::harness::{Emulator, SOLUTION_STACK};
use crate::lifecycle::{account, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The sentence the service answers a `CreateEnvironment` that names no platform with.
const NO_PLATFORM_MESSAGE: &str =
    "You must specify either Configuration Template name or Solution Stack name.";

/// The sentence `ComposeEnvironments` answers a name no application exists under with.
const NO_APPLICATION_MESSAGE: &str = "Application could not be found";

/// The refusal of a `ServiceRole` the call cannot be served with: the member the request
/// named and the constraint its value failed. ONE message for both ways the member can
/// fail — a value that is not an IAM role ARN, and none where the call needs one.
const INVALID_SERVICE_ROLE_MESSAGE: &str =
    "Parameter ServiceRole is invalid. Must be a valid IAM Role ARN.";

/// An IAM role ARN the fixture applications carry.
fn service_role() -> String {
    format!("arn:aws:iam::{}:role/aws-elasticbeanstalk-service-role", account())
}

/// The ARN of an Elastic Beanstalk application, whether or not one exists under the name.
fn application_arn(name: &str) -> String {
    format!(
        "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/{name}",
        account()
    )
}

/// Borrow a built parameter list for [`Emulator::call`].
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn owned(params: &[(&str, &str)]) -> Vec<(String, String)> {
    params
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// 1. The envelope every refusal is stated in
// ---------------------------------------------------------------------------

/// The `<RequestId>` of an awsQuery error envelope: the element between `</Error>` and
/// `</ErrorResponse>`, which is the only position the RESPONSE's own id can sit in (one
/// inside `<Error>` would be a member of an error shape).
fn envelope_request_id(case: &str, body: &str) -> String {
    assert!(
        body.contains("<ErrorResponse"),
        "case {case:?}: expected an <ErrorResponse> envelope, got {body:?}"
    );
    let error_end = body.rfind("</Error>").unwrap_or_else(|| {
        panic!("case {case:?}: expected a closed <Error> element, got {body:?}")
    });
    let envelope_end = body.rfind("</ErrorResponse>").unwrap_or_else(|| {
        panic!("case {case:?}: expected a closed <ErrorResponse>, got {body:?}")
    });
    let siblings = &body[error_end..envelope_end];
    let opened = siblings.find("<RequestId>").unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a <RequestId> between </Error> and </ErrorResponse>, \
             got {body:?}"
        )
    });
    let rest = &siblings[opened + "<RequestId>".len()..];
    let closed = rest.find("</RequestId>").unwrap_or_else(|| {
        panic!("case {case:?}: expected a closed <RequestId>, got {body:?}")
    });
    assert_eq!(
        body.matches("<RequestId>").count(),
        1,
        "case {case:?}: expected exactly one request id in an error document, got {body:?}"
    );
    rest[..closed].to_string()
}

/// A request id is a hyphenated v4 UUID — the 8-4-4-4-12 hexadecimal form AWS reports a
/// request id in.
fn assert_uuid_shaped(case: &str, id: &str) {
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected the 8-4-4-4-12 request-id form, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
        "case {case:?}: expected a hexadecimal request id, got {id:?}"
    );
}

/// One failing call, and the status and error code it must be refused with.
struct EnvelopeCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn every_error_envelope_names_the_request_it_answers() {
    let emulator = Emulator::new();
    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "envelope-app")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );

    // 51 tags: one past the 50 a resource can carry, which is the request the modelled
    // `TooManyTagsException` answers.
    let mut over_quota: Vec<(String, String)> = vec![(
        "ResourceArn".to_string(),
        application_arn("envelope-app"),
    )];
    for index in 1..=eb::state::MAX_TAGS_PER_RESOURCE + 1 {
        over_quota.push((format!("TagsToAdd.member.{index}.Key"), format!("k{index}")));
        over_quota.push((format!("TagsToAdd.member.{index}.Value"), format!("v{index}")));
    }

    // Rows span the LAYERS that can produce an error, because the rule is the
    // envelope's and not any one handler's: a handler refusal, a modelled error shape,
    // the boundary validator, an absent required member, the scaffold's 501, and an
    // `Action` the routing table does not name.
    let cases = vec![
        EnvelopeCase {
            case: "a handler refusal for an absent parameter",
            action: "CreateEnvironment",
            params: owned(&[
                ("ApplicationName", "envelope-app"),
                ("EnvironmentName", "envelope-env"),
            ]),
            status: 400,
            code: "MissingParameter",
        },
        EnvelopeCase {
            case: "a handler refusal for a parameter that names nothing",
            action: "CreateEnvironment",
            params: owned(&[
                ("ApplicationName", "no-such-app"),
                ("EnvironmentName", "envelope-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ]),
            status: 400,
            code: "InvalidParameterValue",
        },
        EnvelopeCase {
            case: "a modelled error shape (ResourceNotFoundException)",
            action: "ListTagsForResource",
            params: vec![("ResourceArn".to_string(), application_arn("no-such-app"))],
            status: 400,
            code: "ResourceNotFoundException",
        },
        EnvelopeCase {
            case: "a modelled error shape (TooManyTagsException)",
            action: "UpdateTagsForResource",
            params: over_quota,
            status: 400,
            code: "TooManyTagsException",
        },
        EnvelopeCase {
            case: "a boundary refusal for a value outside a modelled bound",
            action: "DescribeEnvironments",
            params: owned(&[("MaxRecords", "1001")]),
            status: 400,
            code: "ValidationError",
        },
        EnvelopeCase {
            case: "a boundary refusal for an absent required member",
            action: "CreateApplication",
            params: owned(&[("Description", "no name supplied")]),
            status: 400,
            code: "MissingRequiredParameter",
        },
        EnvelopeCase {
            // `ValidateConfigurationSettings` still carries the scaffold header,
            // so it answers the 501 — which is an error response and carries the
            // element too. Both of its input's required members are supplied, so
            // the request reaches the handler rather than the boundary validator.
            case: "the scaffold's 501",
            action: "ValidateConfigurationSettings",
            params: owned(&[
                ("ApplicationName", "envelope-app"),
                ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
                ("OptionSettings.member.1.OptionName", "MinSize"),
                ("OptionSettings.member.1.Value", "1"),
            ]),
            status: 501,
            code: "NotImplemented",
        },
        EnvelopeCase {
            case: "an Action the routing table does not name",
            action: "NotAModelledOperation",
            params: Vec::new(),
            status: 400,
            code: "InvalidAction",
        },
    ];

    let mut seen: Vec<String> = Vec::new();
    for EnvelopeCase {
        case,
        action,
        params,
        status,
        code,
    } in &cases
    {
        let refused = emulator.call(action, &borrowed(params)).await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {:?} in body {}",
            refused.error_code(),
            refused.body
        );
        let request_id = envelope_request_id(case, &refused.body);
        assert_uuid_shaped(case, &request_id);
        assert!(
            !seen.contains(&request_id),
            "case {case:?}: every response names ONE request, but {request_id:?} was \
             already reported"
        );
        seen.push(request_id);
    }

    // The other side of the rule: a SUCCESS is not an error envelope, so the writer
    // leaves it alone and its request id stays where the protocol puts it.
    let described = emulator.call("DescribeApplications", &[]).await;
    assert_eq!(
        described.status, 200,
        "case \"success\": expected 200 from DescribeApplications, got {} and body {}",
        described.status, described.body
    );
    assert!(
        !described.body.contains("<ErrorResponse"),
        "case \"success\": a successful answer must not be an error envelope, got {}",
        described.body
    );
    let metadata_id = described
        .value("ResponseMetadata/RequestId")
        .unwrap_or_else(|| {
            panic!(
                "case \"success\": expected ResponseMetadata/RequestId, got {}",
                described.body
            )
        });
    assert_uuid_shaped("success", metadata_id);
    assert_eq!(
        described.body.matches("<RequestId>").count(),
        1,
        "case \"success\": expected exactly one request id, got {}",
        described.body
    );
}

// ---------------------------------------------------------------------------
// 2. An absent parameter is reported before any resource is resolved
// ---------------------------------------------------------------------------

/// One `CreateEnvironment`: which platform member it names, whether its application
/// exists, and the refusal (or success) that must follow.
struct PlatformCase {
    case: &'static str,
    /// The `(name, value)` pairs that say where the configuration comes from — none, or
    /// exactly one of the three the operation accepts.
    platform: Vec<(String, String)>,
    application: &'static str,
    status: u16,
    code: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn an_environment_names_where_its_configuration_comes_from_before_anything_is_resolved() {
    let emulator = Emulator::new();
    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "platform-app")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );

    // The platform catalogue is the service's own (`eb::state::SOLUTION_STACKS`, what
    // `ListAvailableSolutionStacks` reports), so neither the stack name nor the ARN
    // below can drift from what the service resolves them against.
    let stack = eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack");
    let managed_arn = stack.platform_arn(TEST_REGION);

    // Rows differ along WHICH of the three members names the configuration (none, each
    // one in turn) and whether the application exists — the two inputs the order under
    // test reads.
    let cases = [
        PlatformCase {
            case: "no platform member, unknown application",
            platform: Vec::new(),
            application: "no-such-app",
            status: 400,
            code: "MissingParameter",
            message: NO_PLATFORM_MESSAGE,
        },
        PlatformCase {
            case: "no platform member, existing application",
            platform: Vec::new(),
            application: "platform-app",
            status: 400,
            code: "MissingParameter",
            message: NO_PLATFORM_MESSAGE,
        },
        PlatformCase {
            case: "a solution stack, unknown application",
            platform: owned(&[("SolutionStackName", SOLUTION_STACK.as_str())]),
            application: "no-such-app",
            status: 400,
            code: "InvalidParameterValue",
            message: "No Application named 'no-such-app' found.",
        },
        PlatformCase {
            case: "a configuration template, unknown application",
            platform: owned(&[("TemplateName", "a-template")]),
            application: "no-such-app",
            status: 400,
            code: "InvalidParameterValue",
            message: "No Application named 'no-such-app' found.",
        },
        PlatformCase {
            case: "a platform version, unknown application",
            platform: vec![("PlatformArn".to_string(), managed_arn.clone())],
            application: "no-such-app",
            status: 400,
            code: "InvalidParameterValue",
            message: "No Application named 'no-such-app' found.",
        },
        PlatformCase {
            case: "a solution stack, existing application",
            platform: owned(&[("SolutionStackName", SOLUTION_STACK.as_str())]),
            application: "platform-app",
            status: 200,
            code: "",
            message: "",
        },
        PlatformCase {
            case: "a platform version, existing application",
            platform: vec![("PlatformArn".to_string(), managed_arn.clone())],
            application: "platform-app",
            status: 200,
            code: "",
            message: "",
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        // A distinct name per row: a name a live environment already holds is a
        // different refusal, which is not what these rows are about.
        let environment_name = format!("plat-env-{index}");
        let mut params: Vec<(String, String)> = vec![
            ("ApplicationName".to_string(), case.application.to_string()),
            ("EnvironmentName".to_string(), environment_name),
        ];
        params.extend(case.platform.iter().cloned());
        let answered = emulator.call("CreateEnvironment", &borrowed(&params)).await;
        let name = case.case;
        assert_eq!(
            answered.status, case.status,
            "case {name:?}: expected HTTP {}, got {} and body {}",
            case.status, answered.status, answered.body
        );
        assert_eq!(
            answered.error_code(),
            case.code,
            "case {name:?}: expected error code {:?}, got {:?} in body {}",
            case.code,
            answered.error_code(),
            answered.body
        );
        if !case.message.is_empty() {
            assert_eq!(
                answered.error_message(),
                case.message,
                "case {name:?}: unexpected message in {}",
                answered.body
            );
        }
        if case.status == 200 {
            // The round trip: an accepted environment is persisted with the platform
            // the request named.
            let described = emulator
                .call(
                    "DescribeEnvironments",
                    &[(
                        "EnvironmentIds.member.1",
                        answered.expect("EnvironmentId", name),
                    )],
                )
                .await;
            assert_eq!(
                described.values_at("Environments/member/ApplicationName"),
                vec![case.application],
                "case {name:?}: the stored environment must belong to the application the \
                 request named, got {}",
                described.body
            );
        }
    }

    // No environment was stored for any refused row: the refusals above changed nothing.
    let listed = emulator.call("DescribeEnvironments", &[]).await;
    let stored = listed.values_at("Environments/member/EnvironmentName");
    assert_eq!(
        stored.len(),
        2,
        "expected only the two accepted environments to be stored, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. The application a request names is resolved before its children
// ---------------------------------------------------------------------------

#[tokio::test]
async fn composing_environments_resolves_the_application_before_the_source_bundles() {
    let emulator = Emulator::new();
    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "compose-app")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );
    let version = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", "compose-app"),
                ("VersionLabel", "v1"),
                ("SourceBundle.S3Bucket", "compose-bucket"),
                ("SourceBundle.S3Key", "app.zip"),
            ],
        )
        .await;
    assert_eq!(
        version.status, 200,
        "case \"setup\": expected 200 from CreateApplicationVersion, got {} and body {}",
        version.status, version.body
    );

    // Rows differ along how many version labels the request carries, whether it names a
    // group, and whether the labels name versions that exist — none of which the
    // application check depends on.
    let unknown_application: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "one version label",
            vec![("VersionLabels.member.1", "v1")],
        ),
        (
            "three version labels",
            vec![
                ("VersionLabels.member.1", "v1"),
                ("VersionLabels.member.2", "v2"),
                ("VersionLabels.member.3", "v3"),
            ],
        ),
        (
            "a group name beside the label",
            vec![
                ("VersionLabels.member.1", "v1"),
                ("GroupName", "group-one"),
            ],
        ),
        (
            "a label no version exists under",
            vec![("VersionLabels.member.1", "nothing-under-this-label")],
        ),
    ];
    for (case, labels) in &unknown_application {
        let mut params: Vec<(&str, &str)> = vec![("ApplicationName", "no-such-app")];
        params.extend(labels.iter().copied());
        let refused = emulator.call("ComposeEnvironments", &params).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_message(),
            NO_APPLICATION_MESSAGE,
            "case {case:?}: an unknown application must be reported as that, got {}",
            refused.body
        );
    }

    // The application exists, so the refusal is the one about what the source bundle
    // does not carry — the precondition that comes AFTER the application.
    let existing = emulator
        .call(
            "ComposeEnvironments",
            &[
                ("ApplicationName", "compose-app"),
                ("VersionLabels.member.1", "v1"),
            ],
        )
        .await;
    assert_eq!(
        existing.status, 400,
        "case \"existing application\": expected HTTP 400, got {} and body {}",
        existing.status, existing.body
    );
    assert!(
        existing.error_message().contains("env.yaml"),
        "case \"existing application\": expected the manifest precondition, got {}",
        existing.body
    );

    // And a request that supplies NO version label at all is refused for that absent
    // parameter, before either — the same order `CreateEnvironment` applies.
    let no_labels = emulator
        .call("ComposeEnvironments", &[("ApplicationName", "no-such-app")])
        .await;
    assert_eq!(
        no_labels.status, 400,
        "case \"no version labels\": expected HTTP 400, got {} and body {}",
        no_labels.status, no_labels.body
    );
    assert!(
        no_labels.error_message().contains("VersionLabel"),
        "case \"no version labels\": expected the absent parameter to be named, got {}",
        no_labels.body
    );
}

// ---------------------------------------------------------------------------
// 4. A ServiceRole is read for its FORM, not only for its presence
// ---------------------------------------------------------------------------

/// One `ServiceRole` value and whether it is an IAM role ARN.
struct RoleCase {
    case: &'static str,
    value: String,
    accepted: bool,
}

/// The rows span the grammar the published IAM role-ARN patterns state
/// (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`, e.g.
/// `com.amazonaws.bedrockagentcorecontrol#RoleArn`): the partition, the empty Region an
/// IAM ARN carries, the account field, the `role` resource type and its non-empty name.
fn role_cases() -> Vec<RoleCase> {
    vec![
        RoleCase {
            case: "a plain role ARN",
            value: service_role(),
            accepted: true,
        },
        RoleCase {
            case: "a role ARN carrying a path",
            value: format!("arn:aws:iam::{}:role/service-role/eb-lifecycle", account()),
            accepted: true,
        },
        RoleCase {
            case: "a role ARN in another partition",
            value: "arn:aws-us-gov:iam::000000000000:role/eb-lifecycle".to_string(),
            accepted: true,
        },
        RoleCase {
            // The partition is `aws` or `aws-<suffix>`, not anything STARTING with
            // those letters.
            case: "a partition that merely starts with aws",
            value: "arn:awsfoo:iam::000000000000:role/eb-lifecycle".to_string(),
            accepted: false,
        },
        RoleCase {
            case: "a partition with an empty suffix",
            value: "arn:aws-:iam::000000000000:role/eb-lifecycle".to_string(),
            accepted: false,
        },
        RoleCase {
            case: "a role ARN with no account",
            value: "arn:aws:iam:::role/eb-lifecycle".to_string(),
            accepted: true,
        },
        RoleCase {
            case: "a name that is not an ARN at all",
            value: "aws-elasticbeanstalk-service-role".to_string(),
            accepted: false,
        },
        RoleCase {
            case: "an empty value",
            value: String::new(),
            accepted: false,
        },
        RoleCase {
            case: "another vendor's ARN",
            value: application_arn("compose-app"),
            accepted: false,
        },
        RoleCase {
            case: "an IAM ARN naming a user rather than a role",
            value: format!("arn:aws:iam::{}:user/someone", account()),
            accepted: false,
        },
        RoleCase {
            case: "a role ARN carrying a Region, which IAM ARNs do not",
            value: format!("arn:aws:iam:{TEST_REGION}:{}:role/eb-lifecycle", account()),
            accepted: false,
        },
        RoleCase {
            case: "an account that is not 12 digits",
            value: "arn:aws:iam::12345:role/eb-lifecycle".to_string(),
            accepted: false,
        },
        RoleCase {
            case: "a role ARN with no role name",
            value: format!("arn:aws:iam::{}:role/", account()),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_service_role_is_read_for_its_form_and_for_the_call_that_needs_one() {
    let emulator = Emulator::new();
    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "role-app")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );

    for RoleCase {
        case,
        value,
        accepted,
    } in role_cases()
    {
        // `UpdateApplicationResourceLifecycle`, the call that carries the member on an
        // application that already exists.
        let answered = emulator
            .call(
                "UpdateApplicationResourceLifecycle",
                &[
                    ("ApplicationName", "role-app"),
                    ("ResourceLifecycleConfig.ServiceRole", value.as_str()),
                ],
            )
            .await;
        if accepted {
            assert_eq!(
                answered.status, 200,
                "case {case:?}: expected 200 for an IAM role ARN, got {} and body {}",
                answered.status, answered.body
            );
            assert_eq!(
                answered.expect("ResourceLifecycleConfig/ServiceRole", case),
                value,
                "case {case:?}: the accepted role must be echoed back, got {}",
                answered.body
            );
            // The round trip: the accepted role is what a later read reports.
            let described = emulator
                .call(
                    "DescribeApplications",
                    &[("ApplicationNames.member.1", "role-app")],
                )
                .await;
            assert_eq!(
                described.expect("ResourceLifecycleConfig/ServiceRole", case),
                value,
                "case {case:?}: DescribeApplications must report the stored role, got {}",
                described.body
            );
        } else {
            assert_eq!(
                answered.status, 400,
                "case {case:?}: expected HTTP 400 for a value that is no role ARN, got {} \
                 and body {}",
                answered.status, answered.body
            );
            assert_eq!(
                answered.error_code(),
                "InvalidParameterValue",
                "case {case:?}: unexpected error code in {}",
                answered.body
            );
            assert_eq!(
                answered.error_message(),
                INVALID_SERVICE_ROLE_MESSAGE,
                "case {case:?}: the message must name the member and its constraint, got {}",
                answered.body
            );
        }

        // `CreateApplication`, the other call the model says supports the member, reads
        // it the same way. Each row creates its own application, so an accepted row is
        // not refused for a name already taken.
        let name = format!("role-app-{}", case.replace(' ', "-").replace(',', ""));
        let created = emulator
            .call(
                "CreateApplication",
                &[
                    ("ApplicationName", name.as_str()),
                    ("ResourceLifecycleConfig.ServiceRole", value.as_str()),
                ],
            )
            .await;
        if accepted {
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateApplication must accept an IAM role ARN, got {} and \
                 body {}",
                created.status, created.body
            );
        } else {
            assert_eq!(
                created.error_message(),
                INVALID_SERVICE_ROLE_MESSAGE,
                "case {case:?}: CreateApplication must refuse the value the same way, got {}",
                created.body
            );
        }
    }

    // NONE at all, where the call needs one: "The ServiceRole property is required the
    // first time that you provide a VersionLifecycleConfig for the application". The
    // member the request could not be served with is the same member, so the refusal is
    // the same one — which is what the recorded rejection of this step carries.
    let absent = emulator
        .call(
            "CreateApplication",
            &[
                ("ApplicationName", "role-app-absent"),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "5",
                ),
            ],
        )
        .await;
    assert_eq!(
        absent.status, 400,
        "case \"absent role\": expected HTTP 400, got {} and body {}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.error_message(),
        INVALID_SERVICE_ROLE_MESSAGE,
        "case \"absent role\": the refusal must name the member and its constraint, got {}",
        absent.body
    );
    // The same request on the other supporting call, against an application that has no
    // role persisted either.
    let second = emulator
        .call("CreateApplication", &[("ApplicationName", "role-app-absent-2")])
        .await;
    assert_eq!(
        second.status, 200,
        "case \"absent role on update\": expected 200 from CreateApplication, got {} and \
         body {}",
        second.status, second.body
    );
    let absent_update = emulator
        .call(
            "UpdateApplicationResourceLifecycle",
            &[
                ("ApplicationName", "role-app-absent-2"),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.Enabled",
                    "true",
                ),
            ],
        )
        .await;
    assert_eq!(
        absent_update.status, 400,
        "case \"absent role on update\": expected HTTP 400, got {} and body {}",
        absent_update.status, absent_update.body
    );
    assert_eq!(
        absent_update.error_message(),
        INVALID_SERVICE_ROLE_MESSAGE,
        "case \"absent role on update\": both supporting calls state the same refusal, got {}",
        absent_update.body
    );

    // A version lifecycle configuration WITH a role ARN is accepted, and the role then
    // persists: "After you provide it once … you don't need to specify it again."
    let with_role = emulator
        .call(
            "CreateApplication",
            &[
                ("ApplicationName", "role-app-with-rule"),
                ("ResourceLifecycleConfig.ServiceRole", service_role().as_str()),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "5",
                ),
            ],
        )
        .await;
    assert_eq!(
        with_role.status, 200,
        "case \"rule with a role\": expected 200, got {} and body {}",
        with_role.status, with_role.body
    );
    let later = emulator
        .call(
            "UpdateApplicationResourceLifecycle",
            &[
                ("ApplicationName", "role-app-with-rule"),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "9",
                ),
            ],
        )
        .await;
    assert_eq!(
        later.status, 200,
        "case \"role already persisted\": a stored role serves a later update, got {} and \
         body {}",
        later.status, later.body
    );
    assert_eq!(
        later.expect("ResourceLifecycleConfig/ServiceRole", "role already persisted"),
        service_role(),
        "case \"role already persisted\": the stored role must survive, got {}",
        later.body
    );
    let malformed = emulator
        .call(
            "CreateApplication",
            &[
                ("ApplicationName", "role-app-bad-rule"),
                ("ResourceLifecycleConfig.ServiceRole", "not-a-role"),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
            ],
        )
        .await;
    assert_eq!(
        malformed.error_message(),
        INVALID_SERVICE_ROLE_MESSAGE,
        "case \"rule with a malformed role\": the member's value is what is wrong, got {}",
        malformed.body
    );
}

// ---------------------------------------------------------------------------
// The tag refusals the same envelope carries
// ---------------------------------------------------------------------------

/// RULE: the modelled `TooManyTagsException` states the quota a resource has, and a
/// refusal that names a caller-supplied ARN quotes it — the wording this crate uses
/// wherever it names a value the caller sent.
#[tokio::test]
async fn the_tag_refusals_state_the_quota_and_quote_the_arn() {
    let emulator = Emulator::new();
    let setup = emulator
        .call("CreateApplication", &[("ApplicationName", "tag-rules")])
        .await;
    assert_eq!(
        setup.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        setup.status, setup.body
    );
    let quota = format!(
        "The maximum number of tags per resource is exceeded. The number of allowed tags \
         is {}.",
        eb::state::MAX_TAGS_PER_RESOURCE
    );

    // Rows span the ways the quota can be exceeded: by the size of the request itself,
    // and by what the resource would carry once the request was applied (one past the
    // limit each time, which is the bound the rule is stated at).
    let arn = application_arn("tag-rules");
    let over_by_request = {
        let mut params: Vec<(String, String)> = vec![("ResourceArn".to_string(), arn.clone())];
        for index in 1..=eb::state::MAX_TAGS_PER_RESOURCE + 1 {
            params.push((format!("TagsToAdd.member.{index}.Key"), format!("k{index}")));
            params.push((format!("TagsToAdd.member.{index}.Value"), "v".to_string()));
        }
        params
    };
    let refused = emulator
        .call("UpdateTagsForResource", &borrowed(&over_by_request))
        .await;
    assert_eq!(
        refused.error_code(),
        "TooManyTagsException",
        "case \"one past the quota in one request\": unexpected code in {}",
        refused.body
    );
    assert_eq!(
        refused.error_message(),
        quota,
        "case \"one past the quota in one request\": the message must state the quota, got {}",
        refused.body
    );

    // Exactly AT the quota is accepted, so the bound itself is pinned.
    let at_quota = {
        let mut params: Vec<(String, String)> = vec![("ResourceArn".to_string(), arn.clone())];
        for index in 1..=eb::state::MAX_TAGS_PER_RESOURCE {
            params.push((format!("TagsToAdd.member.{index}.Key"), format!("k{index}")));
            params.push((format!("TagsToAdd.member.{index}.Value"), "v".to_string()));
        }
        params
    };
    let accepted = emulator
        .call("UpdateTagsForResource", &borrowed(&at_quota))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"exactly at the quota\": expected 200, got {} and body {}",
        accepted.status, accepted.body
    );

    // One more tag on a resource already at the quota: the same quota, so the same
    // sentence, reached by the other check.
    let one_more = emulator
        .call(
            "UpdateTagsForResource",
            &[
                ("ResourceArn", arn.as_str()),
                ("TagsToAdd.member.1.Key", "one-too-many"),
                ("TagsToAdd.member.1.Value", "v"),
            ],
        )
        .await;
    assert_eq!(
        one_more.error_message(),
        quota,
        "case \"one past the quota on the resource\": one quota, one sentence, got {}",
        one_more.body
    );

    // A refusal that names a caller-supplied ARN quotes it, on both tag operations and
    // for each class of ARN the reader distinguishes.
    let rows: [(&str, String, &str); 3] = [
        (
            "a taggable resource that does not exist",
            application_arn("no-such-app"),
            "ResourceNotFoundException",
        ),
        (
            "a resource type that cannot carry tags",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:solutionstack/whatever",
                account()
            ),
            "ResourceTypeNotSupportedException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:::bucket-{}", account()),
            "InvalidParameterValue",
        ),
    ];
    for (case, value, code) in rows {
        for action in ["ListTagsForResource", "UpdateTagsForResource"] {
            let params: Vec<(&str, &str)> = vec![
                ("ResourceArn", value.as_str()),
                ("TagsToAdd.member.1.Key", "k"),
                ("TagsToAdd.member.1.Value", "v"),
            ];
            let refused = emulator.call(action, &params).await;
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?} on {action}: unexpected code in {}",
                refused.body
            );
            assert!(
                refused.error_message().contains(&format!("'{value}'")),
                "case {case:?} on {action}: the message must quote the ARN it was given, \
                 got {:?}",
                refused.error_message()
            );
        }
    }
}
