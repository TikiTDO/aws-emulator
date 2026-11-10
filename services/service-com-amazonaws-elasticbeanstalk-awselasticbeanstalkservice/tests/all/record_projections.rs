// SPDX-License-Identifier: CC0-1.0

//! What this service's stored records report: the members the model defines, the
//! defaults the service itself fills in, and the mutations a later read carries.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! 1. An application always has a resource lifecycle configuration: a setting
//!    the request leaves out is the service's own default, so every read of the
//!    application reports the whole policy.
//! 2. An application version always reports where its source bundle is: the
//!    location the request gave, or — "If neither `SourceBundle` nor
//!    `SourceBuildInformation` are provided, Elastic Beanstalk uses a sample
//!    application" — the sample application's location in the request's region.
//! 3. A version's processing status is reported in the wire form the service
//!    puts it on the wire in.
//! 4. `DeleteApplication` is asynchronous: the application is still described
//!    while its deletion runs and is gone once the clock has made the removal
//!    due, and the activity records its start and its completion under one
//!    request id.
//! 5. An environment reports the platform version it runs on however the request
//!    named it, and a tier with every member the service defaults filled in.
//! 6. An `EnvironmentId` has AWS's `e-` + 10 lowercase alphanumerics form, is
//!    unique, and does not change once minted.
//! 7. A call records its activity as a SEQUENCE of events sharing one request
//!    id, which `DescribeEvents` reports newest-first as a list.
//!
//! Driven over the same awsQuery HTTP surface as `tests/all/lifecycle.rs`,
//! through the shared [`Emulator`] harness, so nothing here reads the store
//! directly: a default that only the handler knows about, or a projection that
//! drops a member, fails these tests.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::harness::{Answer, Emulator, PAST_DUE};
use crate::lifecycle::{is_environment_id, TEST_REGION};

/// A region that is NOT the one the harness signs with by default, so a
/// region-derived value is asserted against two different regions.
const OTHER_REGION: &str = "eu-west-1";

/// A `SolutionStackName` the service itself publishes, so a `CreateEnvironment`
/// under test names a configuration source that exists: "You must specify either
/// Configuration Template name or Solution Stack name."
fn published_solution_stack() -> String {
    eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack")
        .name()
}

/// Call `action`, failing the named case unless it answered 200.
///
/// `pub(crate)` for the same reason [`ReportedLifecycle`] is: a sibling suite
/// driving the same preconditions reuses this one helper rather than keeping a
/// second copy of the status assertion.
pub(crate) async fn ok(
    emulator: &Emulator,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

// ---------------------------------------------------------------------------
// 1. The application's resource lifecycle configuration
// ---------------------------------------------------------------------------

/// The four version-lifecycle settings a read must report, as the XML carries
/// them: `(MaxAgeRule.Enabled, MaxAgeRule.MaxAgeInDays, MaxCountRule.Enabled,
/// MaxCountRule.MaxCount)` plus both `DeleteSourceFromS3` flags.
///
/// `pub(crate)` so a sibling suite asserting the SAME projection on another
/// operation's response reads it with this one reader rather than keeping a
/// second copy of the element paths.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReportedLifecycle {
    pub(crate) max_age_enabled: String,
    pub(crate) max_age_in_days: String,
    pub(crate) max_age_delete_source: String,
    pub(crate) max_count_enabled: String,
    pub(crate) max_count: String,
    pub(crate) max_count_delete_source: String,
}

impl ReportedLifecycle {
    /// The configuration reported under `prefix` in `answer` (for example
    /// `Application/ResourceLifecycleConfig`).
    pub(crate) fn of(answer: &Answer, prefix: &str, case: &str) -> Self {
        let at = |path: &str| {
            answer
                .value(&format!("{prefix}/VersionLifecycleConfig/{path}"))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a {prefix}/VersionLifecycleConfig/{path} \
                         element, got body {}",
                        answer.body
                    )
                })
                .to_string()
        };
        Self {
            max_age_enabled: at("MaxAgeRule/Enabled"),
            max_age_in_days: at("MaxAgeRule/MaxAgeInDays"),
            max_age_delete_source: at("MaxAgeRule/DeleteSourceFromS3"),
            max_count_enabled: at("MaxCountRule/Enabled"),
            max_count: at("MaxCountRule/MaxCount"),
            max_count_delete_source: at("MaxCountRule/DeleteSourceFromS3"),
        }
    }

    /// The configuration an application reports when its caller asked for none:
    /// both rules present and disabled, keeping the source bundle, at the
    /// service's own retention figures (`eb::state::DEFAULT_MAX_AGE_IN_DAYS` /
    /// `DEFAULT_MAX_COUNT`).
    pub(crate) fn service_default() -> Self {
        Self {
            max_age_enabled: "false".to_string(),
            max_age_in_days: eb::state::DEFAULT_MAX_AGE_IN_DAYS.to_string(),
            max_age_delete_source: "false".to_string(),
            max_count_enabled: "false".to_string(),
            max_count: eb::state::DEFAULT_MAX_COUNT.to_string(),
            max_count_delete_source: "false".to_string(),
        }
    }
}

/// One row of the lifecycle table: what `CreateApplication` asks for, and what
/// every read of the application must then report.
struct LifecycleCase {
    /// Doubles as the application name, so each row is an independent resource.
    case: &'static str,
    requested: &'static [(&'static str, &'static str)],
    expected: fn() -> ReportedLifecycle,
}

#[tokio::test]
async fn every_application_reports_a_complete_resource_lifecycle_config() {
    // Rows differ along what the rule generalises over: no configuration at
    // all, one rule supplied and the other left out, a rule supplied with only
    // its required member, and a configuration that names every setting.
    let cases = [
        LifecycleCase {
            case: "no-configuration-at-all",
            requested: &[],
            expected: ReportedLifecycle::service_default,
        },
        LifecycleCase {
            case: "a-service-role-and-no-rules",
            requested: &[(
                "ResourceLifecycleConfig.ServiceRole",
                "arn:aws:iam::123456789012:role/aws-elasticbeanstalk-service-role",
            )],
            expected: ReportedLifecycle::service_default,
        },
        LifecycleCase {
            case: "a-max-count-rule-only",
            requested: &[
                (
                    "ResourceLifecycleConfig.ServiceRole",
                    "arn:aws:iam::123456789012:role/aws-elasticbeanstalk-service-role",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "5",
                ),
            ],
            expected: || ReportedLifecycle {
                max_count_enabled: "true".to_string(),
                max_count: "5".to_string(),
                ..ReportedLifecycle::service_default()
            },
        },
        LifecycleCase {
            // `MaxAgeRule$Enabled` is the rule's only required member, so a
            // request can supply the rule and leave its figures out.
            case: "a-max-age-rule-with-only-its-required-member",
            requested: &[
                (
                    "ResourceLifecycleConfig.ServiceRole",
                    "arn:aws:iam::123456789012:role/aws-elasticbeanstalk-service-role",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.Enabled",
                    "true",
                ),
            ],
            expected: || ReportedLifecycle {
                max_age_enabled: "true".to_string(),
                ..ReportedLifecycle::service_default()
            },
        },
        LifecycleCase {
            case: "every-setting-named",
            requested: &[
                (
                    "ResourceLifecycleConfig.ServiceRole",
                    "arn:aws:iam::123456789012:role/aws-elasticbeanstalk-service-role",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.MaxAgeInDays",
                    "7",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.DeleteSourceFromS3",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "3",
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.DeleteSourceFromS3",
                    "true",
                ),
            ],
            expected: || ReportedLifecycle {
                max_age_enabled: "true".to_string(),
                max_age_in_days: "7".to_string(),
                max_age_delete_source: "true".to_string(),
                max_count_enabled: "true".to_string(),
                max_count: "3".to_string(),
                max_count_delete_source: "true".to_string(),
            },
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let mut params: Vec<(&str, &str)> = vec![("ApplicationName", case.case)];
        params.extend_from_slice(case.requested);
        let created = ok(&emulator, case.case, "CreateApplication", &params).await;
        let expected = (case.expected)();

        // The create path reports it,
        assert_eq!(
            ReportedLifecycle::of(&created, "Application/ResourceLifecycleConfig", case.case),
            expected,
            "case {:?}: unexpected ResourceLifecycleConfig from CreateApplication, got {}",
            case.case,
            created.body
        );

        // the read path reports the same,
        let described = ok(
            &emulator,
            case.case,
            "DescribeApplications",
            &[("ApplicationNames.member.1", case.case)],
        )
        .await;
        assert_eq!(
            ReportedLifecycle::of(
                &described,
                "Applications/member/ResourceLifecycleConfig",
                case.case
            ),
            expected,
            "case {:?}: unexpected ResourceLifecycleConfig from DescribeApplications, got {}",
            case.case,
            described.body
        );

        // and so does an update that does not touch the configuration.
        let updated = ok(
            &emulator,
            case.case,
            "UpdateApplication",
            &[("ApplicationName", case.case), ("Description", "updated")],
        )
        .await;
        assert_eq!(
            ReportedLifecycle::of(&updated, "Application/ResourceLifecycleConfig", case.case),
            expected,
            "case {:?}: unexpected ResourceLifecycleConfig from UpdateApplication, got {}",
            case.case,
            updated.body
        );
    }
}

#[tokio::test]
async fn updating_the_resource_lifecycle_fills_in_the_settings_it_leaves_out() {
    let emulator = Emulator::new();
    let case = "lifecycle-update";
    let service_role = "arn:aws:iam::123456789012:role/aws-elasticbeanstalk-service-role";
    ok(
        &emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", case)],
    )
    .await;

    // The update names one rule and one of its settings; everything it leaves
    // out is the service default, exactly as on the create path.
    let updated = ok(
        &emulator,
        case,
        "UpdateApplicationResourceLifecycle",
        &[
            ("ApplicationName", case),
            ("ResourceLifecycleConfig.ServiceRole", service_role),
            (
                "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                "true",
            ),
        ],
    )
    .await;
    let expected = ReportedLifecycle {
        max_count_enabled: "true".to_string(),
        ..ReportedLifecycle::service_default()
    };
    assert_eq!(
        ReportedLifecycle::of(&updated, "ResourceLifecycleConfig", case),
        expected,
        "case {case:?}: unexpected ResourceLifecycleConfig from \
         UpdateApplicationResourceLifecycle, got {}",
        updated.body
    );
    assert_eq!(
        updated.value("ResourceLifecycleConfig/ServiceRole"),
        Some(service_role),
        "case {case:?}: the supplied ServiceRole must be reported, got {}",
        updated.body
    );

    // And the update was applied to the stored application, so the read path
    // answers with it too.
    let described = ok(
        &emulator,
        case,
        "DescribeApplications",
        &[("ApplicationNames.member.1", case)],
    )
    .await;
    assert_eq!(
        ReportedLifecycle::of(
            &described,
            "Applications/member/ResourceLifecycleConfig",
            case
        ),
        expected,
        "case {case:?}: the updated configuration must come back from the store, got {}",
        described.body
    );
}

/// One row of the service-role table: the `ServiceRole` a lifecycle
/// configuration is asked for with, and whether the service can apply it.
struct ServiceRoleCase {
    case: &'static str,
    role: Option<&'static str>,
    accepted: bool,
}

#[tokio::test]
async fn a_version_lifecycle_configuration_needs_an_iam_role_arn() {
    // Rows differ along what the rule generalises over: no role at all, an
    // empty one, text that is not an ARN, an ARN of the wrong service, an ARN
    // whose account is not an account id, and role ARNs in two partitions.
    let cases = [
        ServiceRoleCase {
            case: "no-service-role-at-all",
            role: None,
            accepted: false,
        },
        ServiceRoleCase {
            case: "an-empty-service-role",
            role: Some(""),
            accepted: false,
        },
        ServiceRoleCase {
            case: "a-role-name-rather-than-an-arn",
            role: Some("aws-elasticbeanstalk-service-role"),
            accepted: false,
        },
        ServiceRoleCase {
            case: "an-arn-of-another-service",
            role: Some("arn:aws:s3:::my-bucket"),
            accepted: false,
        },
        ServiceRoleCase {
            case: "an-iam-arn-that-is-not-a-role",
            role: Some("arn:aws:iam::123456789012:user/someone"),
            accepted: false,
        },
        ServiceRoleCase {
            case: "an-iam-role-arn",
            role: Some("arn:aws:iam::123456789012:role/aws-elasticbeanstalk-service-role"),
            accepted: true,
        },
        ServiceRoleCase {
            case: "an-iam-role-arn-in-another-partition",
            role: Some("arn:aws-us-gov:iam::123456789012:role/with/a/path"),
            accepted: true,
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        ok(
            &emulator,
            case.case,
            "CreateApplication",
            &[("ApplicationName", case.case)],
        )
        .await;
        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", case.case),
            (
                "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                "true",
            ),
        ];
        if let Some(role) = case.role {
            params.push(("ResourceLifecycleConfig.ServiceRole", role));
        }
        let answered = emulator
            .call("UpdateApplicationResourceLifecycle", &params)
            .await;
        if case.accepted {
            assert_eq!(
                answered.status, 200,
                "case {:?}: an IAM role ARN must be accepted, got {} and body {}",
                case.case, answered.status, answered.body
            );
            assert_eq!(
                answered.value("ResourceLifecycleConfig/ServiceRole"),
                case.role,
                "case {:?}: the accepted role must be echoed back, got {}",
                case.case,
                answered.body
            );
            continue;
        }
        assert_eq!(
            answered.status, 400,
            "case {:?}: expected HTTP 400, got {} and body {}",
            case.case, answered.status, answered.body
        );
        assert_eq!(
            answered.error_code(),
            "InvalidParameterValue",
            "case {:?}: unexpected error code in {}",
            case.case,
            answered.body
        );
        // The message names the member and the constraint it failed.
        assert_eq!(
            answered.error_message(),
            "Parameter ServiceRole is invalid. Must be a valid IAM Role ARN.",
            "case {:?}: unexpected error message in {}",
            case.case,
            answered.body
        );
        // A refused configuration was not applied: the application still
        // reports the service's own defaults.
        let described = ok(
            &emulator,
            case.case,
            "DescribeApplications",
            &[("ApplicationNames.member.1", case.case)],
        )
        .await;
        assert_eq!(
            ReportedLifecycle::of(
                &described,
                "Applications/member/ResourceLifecycleConfig",
                case.case
            ),
            ReportedLifecycle::service_default(),
            "case {:?}: a refused update must change nothing, got {}",
            case.case,
            described.body
        );
    }
}

// ---------------------------------------------------------------------------
// 2 and 3. The application version's source bundle and status
// ---------------------------------------------------------------------------
/// One row of the source-bundle table: what the create supplies, and the
/// `(S3Bucket, S3Key)` every read of the version must then report — `None` where
/// the version has no source bundle at all.
struct BundleCase {
    /// Doubles as the version label, so each row is an independent resource.
    case: &'static str,
    supplied: &'static [(&'static str, &'static str)],
    expected: Option<(&'static str, &'static str)>,
}

/// The sample application's location in `region`, as the model's
/// `CreateApplicationVersion$SourceBundle` rule resolves it ("If neither
/// SourceBundle nor SourceBuildInformation are provided, Elastic Beanstalk uses
/// a sample application").
fn sample_bundle(region: &str) -> (String, String) {
    (
        format!("elasticbeanstalk-{region}"),
        "GenericSampleApplication".to_string(),
    )
}

#[tokio::test]
async fn every_application_version_reports_where_its_source_bundle_is() {
    // Rows differ along what the rule generalises over: a bundle supplied in
    // full, a bundle whose key is a nested path, neither member supplied, and a
    // version whose source is a repository commit instead of a bundle.
    let cases = [
        BundleCase {
            case: "a-bundle-in-full",
            supplied: &[
                ("SourceBundle.S3Bucket", "my-bucket"),
                ("SourceBundle.S3Key", "app.zip"),
            ],
            expected: Some(("my-bucket", "app.zip")),
        },
        BundleCase {
            case: "a-nested-bundle-key",
            supplied: &[
                ("SourceBundle.S3Bucket", "other-bucket"),
                ("SourceBundle.S3Key", "builds/2026/app-1.2.3.war"),
            ],
            expected: Some(("other-bucket", "builds/2026/app-1.2.3.war")),
        },
        BundleCase {
            case: "neither-a-bundle-nor-build-information",
            supplied: &[],
            // Filled in per region below.
            expected: None,
        },
        BundleCase {
            case: "a-codecommit-commit-instead-of-a-bundle",
            supplied: &[
                ("SourceBuildInformation.SourceType", "Git"),
                ("SourceBuildInformation.SourceRepository", "CodeCommit"),
                ("SourceBuildInformation.SourceLocation", "my-repo/main"),
            ],
            expected: None,
        },
    ];

    // Both regions: the sample application's bucket is derived from the region
    // the request was signed for, so it is asserted in two of them.
    for region in [TEST_REGION, OTHER_REGION] {
        let emulator = Emulator::new();
        let application = "bundles";
        assert_eq!(
            emulator
                .call_in_region(region, "CreateApplication", &[("ApplicationName", application)])
                .await
                .status,
            200,
            "case {region:?}: expected 200 from CreateApplication"
        );

        for case in &cases {
            let name = format!("{region}/{}", case.case);
            let mut params: Vec<(&str, &str)> = vec![
                ("ApplicationName", application),
                ("VersionLabel", case.case),
            ];
            params.extend_from_slice(case.supplied);
            let created = emulator
                .call_in_region(region, "CreateApplicationVersion", &params)
                .await;
            assert_eq!(
                created.status, 200,
                "case {name:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
                created.status, created.body
            );

            // A version with no bundle and no build information carries the
            // sample application's location; one built from a repository commit
            // has no source bundle at all, and none is invented for it.
            let sample = sample_bundle(region);
            let expected: Option<(&str, &str)> = match (case.case, case.expected) {
                (_, Some(bundle)) => Some(bundle),
                ("neither-a-bundle-nor-build-information", None) => {
                    Some((sample.0.as_str(), sample.1.as_str()))
                }
                (_, None) => None,
            };

            // Every read of the version reports the same bundle and the same
            // status: the create answer, the list read, and the update answer.
            let described = emulator
                .call_in_region(
                    region,
                    "DescribeApplicationVersions",
                    &[
                        ("ApplicationName", application),
                        ("VersionLabels.member.1", case.case),
                    ],
                )
                .await;
            let updated = emulator
                .call_in_region(
                    region,
                    "UpdateApplicationVersion",
                    &[
                        ("ApplicationName", application),
                        ("VersionLabel", case.case),
                        ("Description", "touched"),
                    ],
                )
                .await;
            let reads = [
                ("CreateApplicationVersion", &created, "ApplicationVersion"),
                (
                    "DescribeApplicationVersions",
                    &described,
                    "ApplicationVersions/member",
                ),
                ("UpdateApplicationVersion", &updated, "ApplicationVersion"),
            ];
            for (operation, answer, prefix) in reads {
                assert_eq!(
                    answer.status, 200,
                    "case {name:?}: expected 200 from {operation}, got {} and body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    (
                        answer.value(&format!("{prefix}/SourceBundle/S3Bucket")),
                        answer.value(&format!("{prefix}/SourceBundle/S3Key")),
                    ),
                    (
                        expected.map(|(bucket, _)| bucket),
                        expected.map(|(_, key)| key),
                    ),
                    "case {name:?}: unexpected SourceBundle from {operation}, got {}",
                    answer.body
                );
                // The status is reported in the wire form: nothing here
                // processes a bundle, so the version is UNPROCESSED.
                let status = answer
                    .value(&format!("{prefix}/Status"))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {name:?}: expected a {prefix}/Status element from {operation}, \
                             got body {}",
                            answer.body
                        )
                    });
                assert_eq!(
                    status, "UNPROCESSED",
                    "case {name:?}: unexpected Status from {operation}, got {}",
                    answer.body
                );
                // Whatever the wire form, the value is one the model declares,
                // read case-insensitively.
                assert!(
                    eb::ops::types::APPLICATION_VERSION_STATUS_VALUES
                        .iter()
                        .any(|modelled| modelled.eq_ignore_ascii_case(status)),
                    "case {name:?}: {status:?} is not a modelled ApplicationVersionStatus"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. The asynchronous application deletion
// ---------------------------------------------------------------------------

/// One row of the deletion table: what the application holds when it is deleted.
struct DeletionCase {
    /// Doubles as the application name.
    case: &'static str,
    versions: &'static [&'static str],
}

#[tokio::test]
async fn deleting_an_application_keeps_describing_it_until_the_removal_is_due() {
    // Rows differ along what the rule generalises over: how many resources the
    // application holds when the deletion starts.
    let cases = [
        DeletionCase {
            case: "nothing-but-the-application",
            versions: &[],
        },
        DeletionCase {
            case: "one-version",
            versions: &["v1"],
        },
        DeletionCase {
            case: "several-versions",
            versions: &["v1", "v2", "v3"],
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        ok(
            &emulator,
            case.case,
            "CreateApplication",
            &[("ApplicationName", case.case), ("Description", "doomed")],
        )
        .await;
        for label in case.versions {
            ok(
                &emulator,
                case.case,
                "CreateApplicationVersion",
                &[("ApplicationName", case.case), ("VersionLabel", label)],
            )
            .await;
        }

        let deleted = ok(
            &emulator,
            case.case,
            "DeleteApplication",
            &[("ApplicationName", case.case)],
        )
        .await;
        assert_eq!(
            deleted.error_code(),
            "",
            "case {:?}: expected no error envelope, got {}",
            case.case,
            deleted.body
        );

        // While the deletion runs the application is still described, with
        // everything it reported before: a client polling the resource it owns
        // is not told it has already gone.
        let deleting = ok(
            &emulator,
            case.case,
            "DescribeApplications",
            &[("ApplicationNames.member.1", case.case)],
        )
        .await;
        assert_eq!(
            deleting.values_at("Applications/member/ApplicationName"),
            vec![case.case],
            "case {:?}: an application whose deletion is still running must be described, got {}",
            case.case,
            deleting.body
        );
        assert_eq!(
            deleting.value("Applications/member/Description"),
            Some("doomed"),
            "case {:?}: the described application must still carry its properties, got {}",
            case.case,
            deleting.body
        );
        assert_eq!(
            ReportedLifecycle::of(
                &deleting,
                "Applications/member/ResourceLifecycleConfig",
                case.case
            ),
            ReportedLifecycle::service_default(),
            "case {:?}: it must still report its lifecycle policy, got {}",
            case.case,
            deleting.body
        );

        // The deletion's start is recorded straight away; its completion is
        // recorded when the clock makes it due, under the same request id.
        let starting = ok(
            &emulator,
            case.case,
            "DescribeEvents",
            &[
                ("ApplicationName", case.case),
                ("MaxRecords", "1"),
            ],
        )
        .await;
        assert_eq!(
            starting.value("Events/member/Message"),
            Some("deleteApplication is starting."),
            "case {:?}: the newest event must be the deletion starting, got {}",
            case.case,
            starting.body
        );
        let start_request_id = starting
            .expect("Events/member/RequestId", case.case)
            .to_string();

        std::thread::sleep(PAST_DUE);

        let gone = ok(
            &emulator,
            case.case,
            "DescribeApplications",
            &[("ApplicationNames.member.1", case.case)],
        )
        .await;
        assert!(
            gone.values_at("Applications/member/ApplicationName").is_empty(),
            "case {:?}: the application must be gone once its removal is due, got {}",
            case.case,
            gone.body
        );
        let settled = ok(
            &emulator,
            case.case,
            "DescribeEvents",
            &[("ApplicationName", case.case), ("MaxRecords", "1")],
        )
        .await;
        assert_eq!(
            settled.value("Events/member/Message"),
            Some("deleteApplication completed successfully."),
            "case {:?}: the completed deletion must record its own event, got {}",
            case.case,
            settled.body
        );
        assert_eq!(
            settled.value("Events/member/RequestId"),
            Some(start_request_id.as_str()),
            "case {:?}: the completion belongs to the request that started the deletion, got {}",
            case.case,
            settled.body
        );
        // The versions went with the call that asked for the deletion.
        let versions = ok(
            &emulator,
            case.case,
            "DescribeApplicationVersions",
            &[("ApplicationName", case.case)],
        )
        .await;
        assert!(
            versions
                .values_at("ApplicationVersions/member/VersionLabel")
                .is_empty(),
            "case {:?}: the deleted application's versions must be gone, got {}",
            case.case,
            versions.body
        );
        // The name is free once the removal is due: a re-create is accepted
        // rather than refused by a record the clock has already removed.
        let recreated = ok(
            &emulator,
            case.case,
            "CreateApplication",
            &[("ApplicationName", case.case)],
        )
        .await;
        assert_eq!(
            recreated.value("Application/ApplicationName"),
            Some(case.case),
            "case {:?}: the name must be free once the deletion has completed, got {}",
            case.case,
            recreated.body
        );
    }
}

// ---------------------------------------------------------------------------
// 5 and 6. The environment's platform, tier and id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_environment_reports_the_platform_version_it_runs_on() {
    let emulator = Emulator::new();
    let application = "platforms";
    ok(
        &emulator,
        application,
        "CreateApplication",
        &[("ApplicationName", application)],
    )
    .await;

    // Every solution stack the service itself publishes — the catalog
    // `ListAvailableSolutionStacks` reports — not just the one the recorded
    // request used, named by the stack NAME and resolved to its platform ARN.
    for (index, stack) in eb::state::SOLUTION_STACKS.iter().enumerate() {
        let name = stack.name();
        let arn = stack.platform_arn(TEST_REGION);
        let environment = format!("by-name-{index:02}");
        let created = ok(
            &emulator,
            &name,
            "CreateEnvironment",
            &[
                ("ApplicationName", application),
                ("EnvironmentName", environment.as_str()),
                ("SolutionStackName", name.as_str()),
            ],
        )
        .await;
        assert_eq!(
            created.value("PlatformArn"),
            Some(arn.as_str()),
            "case {name:?}: a solution stack name must resolve to its platform ARN, got {}",
            created.body
        );
        assert_eq!(
            created.value("SolutionStackName"),
            Some(name.as_str()),
            "case {name:?}: the solution stack must be reported as named, got {}",
            created.body
        );

        // The other way round: named by its ARN, the same version reports the
        // same pair.
        let environment = format!("by-arn-{index:02}");
        let by_arn = ok(
            &emulator,
            &name,
            "CreateEnvironment",
            &[
                ("ApplicationName", application),
                ("EnvironmentName", environment.as_str()),
                ("PlatformArn", arn.as_str()),
            ],
        )
        .await;
        assert_eq!(
            (by_arn.value("PlatformArn"), by_arn.value("SolutionStackName")),
            (Some(arn.as_str()), Some(name.as_str())),
            "case {name:?}: a platform ARN must resolve to its solution stack name, got {}",
            by_arn.body
        );

        // And the store answers the same way on a later read.
        let described = ok(
            &emulator,
            &name,
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", environment.as_str())],
        )
        .await;
        assert_eq!(
            described.value("Environments/member/PlatformArn"),
            Some(arn.as_str()),
            "case {name:?}: the stored PlatformArn must come back, got {}",
            described.body
        );
    }

    // A solution stack the catalogue does not publish names no platform version
    // at all, so the launch is refused and nothing is recorded for it.
    let case = "an unpublished solution stack";
    let unpublished = "64bit Fictional Linux v0.0.1 running Nothing";
    let refused = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", application),
                ("EnvironmentName", "unknown-stack"),
                ("SolutionStackName", unpublished),
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
        "case {case:?}: unexpected error code, got body {}",
        refused.body
    );
    assert_eq!(
        refused.error_message(),
        eb::state::missing_solution_stack_message(unpublished),
        "case {case:?}: the refusal names the stack the request asked for, got body {}",
        refused.body
    );
    let described = ok(
        &emulator,
        case,
        "DescribeEnvironments",
        &[("EnvironmentNames.member.1", "unknown-stack")],
    )
    .await;
    assert!(
        described
            .values_at("Environments/member/EnvironmentName")
            .is_empty(),
        "case {case:?}: no environment may have been recorded, got body {}",
        described.body
    );
}

/// One row of the tier table: the `Tier` members the request supplies, and the
/// `(Name, Type, Version)` the environment must then report.
struct TierCase {
    case: &'static str,
    supplied: &'static [(&'static str, &'static str)],
    expected: (&'static str, &'static str, &'static str),
}

#[tokio::test]
async fn a_created_environment_reports_a_complete_tier() {
    // Rows differ along what the rule generalises over: no tier at all, a tier
    // with its version left out, and a tier that names every member.
    let cases = [
        TierCase {
            case: "no-tier-at-all",
            supplied: &[],
            expected: ("WebServer", "Standard", "1.0"),
        },
        TierCase {
            case: "a-web-server-tier-without-a-version",
            supplied: &[("Tier.Name", "WebServer"), ("Tier.Type", "Standard")],
            expected: ("WebServer", "Standard", "1.0"),
        },
        TierCase {
            // "For Standard-mode EC2-based backend application with Amazon SQS —
            // Worker", whose type is "SQS/HTTP".
            case: "a-worker-tier-without-a-version",
            supplied: &[("Tier.Name", "Worker"), ("Tier.Type", "SQS/HTTP")],
            expected: ("Worker", "SQS/HTTP", "1.0"),
        },
        TierCase {
            case: "a-tier-that-names-its-own-version",
            supplied: &[
                ("Tier.Name", "Worker"),
                ("Tier.Type", "SQS/HTTP"),
                ("Tier.Version", "1.1"),
            ],
            expected: ("Worker", "SQS/HTTP", "1.1"),
        },
    ];

    let emulator = Emulator::new();
    let application = "tiers";
    ok(
        &emulator,
        application,
        "CreateApplication",
        &[("ApplicationName", application)],
    )
    .await;
    // An environment's configuration has to come from somewhere: a request that
    // names no template, solution stack or platform version is refused before
    // the tier is read at all.
    let solution_stack = published_solution_stack();

    for case in &cases {
        let mut params: Vec<(&str, &str)> = vec![
            ("ApplicationName", application),
            ("EnvironmentName", case.case),
            ("SolutionStackName", solution_stack.as_str()),
        ];
        params.extend_from_slice(case.supplied);
        let created = ok(&emulator, case.case, "CreateEnvironment", &params).await;
        let reported = (
            created.value("Tier/Name"),
            created.value("Tier/Type"),
            created.value("Tier/Version"),
        );
        assert_eq!(
            reported,
            (
                Some(case.expected.0),
                Some(case.expected.1),
                Some(case.expected.2),
            ),
            "case {:?}: unexpected Tier from CreateEnvironment, got {}",
            case.case,
            created.body
        );

        // The tier the create reported is the tier the store holds.
        let described = ok(
            &emulator,
            case.case,
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", case.case)],
        )
        .await;
        assert_eq!(
            (
                described.value("Environments/member/Tier/Name"),
                described.value("Environments/member/Tier/Type"),
                described.value("Environments/member/Tier/Version"),
            ),
            (
                Some(case.expected.0),
                Some(case.expected.1),
                Some(case.expected.2),
            ),
            "case {:?}: unexpected Tier from DescribeEnvironments, got {}",
            case.case,
            described.body
        );
    }
}

#[tokio::test]
async fn every_environment_id_has_the_published_form_and_is_unique() {
    let emulator = Emulator::new();
    let application = "ids";
    ok(
        &emulator,
        application,
        "CreateApplication",
        &[("ApplicationName", application)],
    )
    .await;

    // Enough environments that a repeated or sequential id shows up, and
    // several name shapes so nothing about the id is read off the request.
    let names = [
        "id-env-a".to_string(),
        "id-env-0123456789".to_string(),
        "x".repeat(40),
        "idenv".to_string(),
        "id-env-with-many-hyphens-in-it".to_string(),
    ];
    let mut minted: Vec<String> = Vec::new();
    let solution_stack = published_solution_stack();
    for name in &names {
        let created = ok(
            &emulator,
            name,
            "CreateEnvironment",
            &[
                ("ApplicationName", application),
                ("EnvironmentName", name.as_str()),
                ("SolutionStackName", solution_stack.as_str()),
            ],
        )
        .await;
        let id = created.expect("EnvironmentId", name).to_string();
        assert!(
            is_environment_id(&id),
            "case {name:?}: expected an e- + {} lowercase alphanumerics EnvironmentId, got {id:?}",
            eb::state::ENVIRONMENT_ID_BODY_LENGTH
        );
        assert!(
            !minted.contains(&id),
            "case {name:?}: {id:?} was already issued to another environment, got {minted:?}"
        );

        // An id is minted once: reading the environment back reports the same
        // one, by name and by id.
        for filter in [
            ("EnvironmentNames.member.1", name.as_str()),
            ("EnvironmentIds.member.1", id.as_str()),
        ] {
            let described = ok(&emulator, name, "DescribeEnvironments", &[filter]).await;
            assert_eq!(
                described.values_at("Environments/member/EnvironmentId"),
                vec![id.as_str()],
                "case {name:?}: the stored EnvironmentId must come back under {}, got {}",
                filter.0,
                described.body
            );
        }
        minted.push(id);
    }
}

// ---------------------------------------------------------------------------
// The account's storage location
// ---------------------------------------------------------------------------

/// `CreateStorageLocation` is implemented in this change because the recorded
/// application lifecycle calls it between the application-version steps this
/// issue is about, and an `InvalidAction` there stops every later step. Its own
/// gap is recorded in
/// `issues/.../20261022T233132Z-elasticbeanstalk-elastic-beanstalk-inverts-the-success-and-failure-outcome.md`
/// (cause 1) and
/// `20261022T151732Z-elasticbeanstalk-elastic-beanstalk-three-modeled-operations-are-absent-from.md`
/// (cause 2).
///
/// RULE: one storage location per account per region —
/// `elasticbeanstalk-<region>-<account>` — and "if the storage location already
/// exists, `CreateStorageLocation` still returns the bucket name but does not
/// create a new bucket", so the call is idempotent.
#[tokio::test]
async fn the_account_storage_location_is_one_bucket_per_region() {
    let emulator = Emulator::new();
    for region in [TEST_REGION, OTHER_REGION] {
        let expected = format!("elasticbeanstalk-{region}-{}", crate::lifecycle::account());
        // Twice: the second call must report the bucket the first one created.
        for attempt in 1..=2 {
            let answer = emulator
                .call_in_region(region, "CreateStorageLocation", &[])
                .await;
            assert_eq!(
                answer.status, 200,
                "case {region:?} attempt {attempt}: expected 200 from CreateStorageLocation, \
                 got {} and body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.value("S3Bucket"),
                Some(expected.as_str()),
                "case {region:?} attempt {attempt}: unexpected S3Bucket in {}",
                answer.body
            );
        }
    }

    // The store this call writes to is bounded by the quota the operation's own
    // `TooManyBucketsException` names: one bucket per region, up to
    // `MAX_STORAGE_LOCATIONS`, and the call past it is refused rather than
    // minting an unbounded number of names.
    let emulator = Emulator::new();
    let regions: Vec<String> = (0..eb::state::MAX_STORAGE_LOCATIONS)
        .map(|index| format!("us-west-{index}"))
        .collect();
    for region in &regions {
        let answer = emulator
            .call_in_region(region, "CreateStorageLocation", &[])
            .await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: a storage location inside the quota must be created, got {} and \
             body {}",
            answer.status, answer.body
        );
    }
    // One past the quota.
    let refused = emulator
        .call_in_region("eu-central-1", "CreateStorageLocation", &[])
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"one past the quota\": expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TooManyBucketsException",
        "case \"one past the quota\": unexpected error code in {}",
        refused.body
    );
    // A region whose bucket already exists is still reported: the refusal is on
    // creating a NEW one, and the call stays idempotent.
    let existing = emulator
        .call_in_region(&regions[0], "CreateStorageLocation", &[])
        .await;
    assert_eq!(
        existing.status, 200,
        "case \"an existing bucket at the quota\": expected 200, got {} and body {}",
        existing.status, existing.body
    );
}

// ---------------------------------------------------------------------------
// 7. The activity one call records
// ---------------------------------------------------------------------------

/// One row of the activity table: the call, and the events its activity must
/// record — newest first, the order `DescribeEvents` reports them in.
struct ActivityCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, String)>,
    newest_first: Vec<&'static str>,
}

#[tokio::test]
async fn a_call_records_its_whole_activity_under_one_request_id() {
    // Rows differ along what the rule generalises over: which resource the call
    // creates, and so which operation name and created-resource message the
    // sequence carries.
    let cases = vec![
        ActivityCase {
            case: "createApplication",
            action: "CreateApplication",
            params: vec![("ApplicationName", "activity-app".to_string())],
            newest_first: vec![
                "createApplication completed successfully.",
                "Created new Application: activity-app",
                "createApplication is starting.",
            ],
        },
        ActivityCase {
            case: "createApplicationVersion",
            action: "CreateApplicationVersion",
            params: vec![
                ("ApplicationName", "activity-app".to_string()),
                ("VersionLabel", "v1".to_string()),
            ],
            newest_first: vec![
                "createApplicationVersion completed successfully.",
                "Created new Application Version: v1",
                "createApplicationVersion is starting.",
            ],
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let params: Vec<(&str, &str)> = case
            .params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        ok(&emulator, case.case, case.action, &params).await;

        // The whole log, so an event this call did NOT record is visible too.
        let reported = ok(
            &emulator,
            case.case,
            "DescribeEvents",
            &[("MaxRecords", "3")],
        )
        .await;
        let messages = reported.values_at("Events/member/Message");
        assert_eq!(
            messages, case.newest_first,
            "case {:?}: unexpected activity in {}",
            case.case, reported.body
        );
        // A LIST of events, not a single structure: every one of them is its own
        // member, and they all belong to this one call's activity.
        let request_ids = reported.values_at("Events/member/RequestId");
        assert_eq!(
            request_ids.len(),
            case.newest_first.len(),
            "case {:?}: every event must carry a RequestId, got {}",
            case.case,
            reported.body
        );
        assert!(
            request_ids.windows(2).all(|pair| pair[0] == pair[1]),
            "case {:?}: one call's activity is one RequestId, got {request_ids:?}",
            case.case
        );
        let severities = reported.values_at("Events/member/Severity");
        assert!(
            severities.iter().all(|severity| *severity == "INFO"),
            "case {:?}: a successful call's events are informational, got {severities:?}",
            case.case
        );
    }
}
