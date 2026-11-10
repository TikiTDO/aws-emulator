// SPDX-License-Identifier: CC0-1.0

//! The Elastic Beanstalk operations that act on, report on or tag an existing
//! resource: RestartAppServer, RebuildEnvironment, AssociateEnvironmentOperationsRole,
//! ApplyEnvironmentManagedAction, DescribeEnvironmentHealth, DescribeInstancesHealth,
//! ComposeEnvironments, UpdateApplicationResourceLifecycle and the two tag
//! operations — driven end to end over the service's awsQuery HTTP surface.
//!
//! The harness is the one `lifecycle.rs` owns: the same `MultiServiceRouter` the
//! production binary mounts, spoken to with a form-urlencoded `Action=…` body.
//! Nothing here reaches into the store, so an operation that persists nothing, or
//! answers from anything other than the store, fails these tests.
//!
//! Each test is a TABLE over the class its rule generalises over — which
//! identifier names the environment, which resource an ARN names, which
//! attributes a health read asks for, how many tags an update would leave — and
//! every assertion names its case and prints the actual answer.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{account, Answer, Emulator, PAST_DUE, TEST_REGION};

/// Whether `body` carries an element named `element` at all — including one with
/// no text, which [`Answer::value`] reads as absent (an empty `<Causes></Causes>`
/// is a reported-but-empty list, not an omitted member).
fn reports(body: &str, element: &str) -> bool {
    let needle = format!("<{element}");
    body.match_indices(&needle).any(|(at, _)| {
        body[at + needle.len()..]
            .chars()
            .next()
            .is_some_and(|next| next == '>' || next.is_whitespace())
    })
}

/// A platform for the environments below to run on: a `SolutionStackName` the
/// service's own catalogue publishes, since `CreateEnvironment` refuses one it
/// does not. Nothing here depends on WHICH platform it is.
fn solution_stack() -> String {
    crate::lifecycle::SOLUTION_STACK.clone()
}

/// Create `application` with one version `v1` and, when `environment` is given,
/// one environment that has reached `Ready` — the state the operations below act
/// on.
async fn fixture(emulator: &Emulator, application: &str, environment: Option<&str>) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", application)])
        .await;
    assert_eq!(
        created.status, 200,
        "fixture {application:?}: CreateApplication answered {} with body {}",
        created.status, created.body
    );
    let version = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", application),
                ("VersionLabel", "v1"),
                ("SourceBundle.S3Bucket", "bundles"),
                ("SourceBundle.S3Key", "app.zip"),
            ],
        )
        .await;
    assert_eq!(
        version.status, 200,
        "fixture {application:?}: CreateApplicationVersion answered {} with body {}",
        version.status, version.body
    );
    if let Some(environment) = environment {
        let launched = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", application),
                    ("EnvironmentName", environment),
                    ("VersionLabel", "v1"),
                    ("SolutionStackName", solution_stack().as_str()),
                ],
            )
            .await;
        assert_eq!(
            launched.status, 200,
            "fixture {environment:?}: CreateEnvironment answered {} with body {}",
            launched.status, launched.body
        );
        // The launch the clock owes: wait it out so the environment is `Ready`,
        // the one status the model documents as accepting an action.
        std::thread::sleep(PAST_DUE);
    }
}

/// The status and health DescribeEnvironments reports for `environment`.
async fn described(emulator: &Emulator, environment: &str) -> Answer {
    emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", environment)],
        )
        .await
}

// ---------------------------------------------------------------------------
// Resolving the environment an operation acts on
// ---------------------------------------------------------------------------

/// RULE: every operation that names its environment by `EnvironmentId` or
/// `EnvironmentName` refuses a value no environment in the store answers to with
/// `InvalidParameterValue` — whichever member named it.
#[tokio::test]
async fn an_environment_nobody_created_is_refused_by_every_operation_that_names_one() {
    let emulator = Emulator::new();
    fixture(&emulator, "ghost-app", None).await;

    struct Case {
        action: &'static str,
        params: Vec<(&'static str, &'static str)>,
    }
    // One row per operation that takes the pair, each row naming the
    // environment through a DIFFERENT member of it.
    let cases = [
        Case {
            action: "RestartAppServer",
            params: vec![("EnvironmentName", "ghost-env")],
        },
        Case {
            action: "RestartAppServer",
            params: vec![("EnvironmentId", "e-00000000")],
        },
        Case {
            action: "RebuildEnvironment",
            params: vec![("EnvironmentName", "ghost-env")],
        },
        Case {
            action: "RebuildEnvironment",
            params: vec![("EnvironmentId", "e-00000000")],
        },
        Case {
            action: "DescribeEnvironmentHealth",
            params: vec![("EnvironmentName", "ghost-env"), ("AttributeNames.member.1", "All")],
        },
        Case {
            action: "DescribeEnvironmentHealth",
            params: vec![("EnvironmentId", "e-00000000")],
        },
        Case {
            action: "DescribeInstancesHealth",
            params: vec![
                ("EnvironmentName", "ghost-env"),
                ("AttributeNames.member.1", "HealthStatus"),
            ],
        },
        Case {
            action: "DescribeInstancesHealth",
            params: vec![("EnvironmentId", "e-00000000")],
        },
        Case {
            action: "AssociateEnvironmentOperationsRole",
            params: vec![
                ("EnvironmentName", "ghost-env"),
                ("OperationsRole", "arn:aws:iam::123456789012:role/ops"),
            ],
        },
        Case {
            action: "ApplyEnvironmentManagedAction",
            params: vec![("EnvironmentName", "ghost-env"), ("ActionId", "action-1")],
        },
        Case {
            action: "TerminateEnvironment",
            params: vec![("EnvironmentName", "ghost-env")],
        },
    ];

    for case in cases {
        let name = format!("{} {:?}", case.action, case.params);
        let refused = emulator.call(case.action, &case.params).await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {name:?}: unexpected error code in {}",
            refused.body
        );
    }
}

/// RULE: an operation that needs one of the `EnvironmentId` / `EnvironmentName`
/// pair and is given NEITHER is refused with the code its own model documents —
/// `MissingRequiredParameter` where the model says so ("If you do not specify
/// either, Elastic Beanstalk returns MissingRequiredParameter error"), and the
/// generic query `MissingParameter` where it documents nothing.
#[tokio::test]
async fn naming_no_environment_is_refused_with_the_code_that_operations_model_documents() {
    let emulator = Emulator::new();

    // The three operations whose model documents the code, and the three whose
    // model documents none (see each handler's `## Input` contract).
    struct Case {
        action: &'static str,
        params: Vec<(&'static str, &'static str)>,
        expected: &'static str,
    }
    let cases = [
        Case {
            action: "RestartAppServer",
            params: vec![],
            expected: "MissingRequiredParameter",
        },
        Case {
            action: "RebuildEnvironment",
            params: vec![],
            expected: "MissingRequiredParameter",
        },
        Case {
            action: "TerminateEnvironment",
            params: vec![],
            expected: "MissingRequiredParameter",
        },
        Case {
            action: "ApplyEnvironmentManagedAction",
            params: vec![("ActionId", "action-1")],
            expected: "MissingParameter",
        },
        Case {
            action: "DescribeEnvironmentHealth",
            params: vec![("AttributeNames.member.1", "All")],
            expected: "MissingParameter",
        },
        Case {
            action: "DescribeInstancesHealth",
            params: vec![("AttributeNames.member.1", "All")],
            expected: "MissingParameter",
        },
    ];

    for case in cases {
        let (action, expected) = (case.action, case.expected);
        let refused = emulator.call(action, &case.params).await;
        assert_eq!(
            refused.status, 400,
            "case {action:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            expected,
            "case {action:?}: expected {expected}, got {}",
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------
// Acting on an environment
// ---------------------------------------------------------------------------

/// RULE: RestartAppServer restarts the container server without redeploying, so
/// the environment keeps its status while its health checks are suspended
/// (`Grey`), and the health it reported comes back when the clock makes the
/// restart due. Asserted for each member that can name the environment.
#[tokio::test]
async fn restart_app_server_suspends_health_checks_and_gives_them_back() {
    for by_id in [false, true] {
        let case = if by_id { "by EnvironmentId" } else { "by EnvironmentName" };
        let emulator = Emulator::new();
        fixture(&emulator, "restart-app", Some("restart-env-1")).await;
        let ready = described(&emulator, "restart-env-1").await;
        assert_eq!(
            ready.values_at("Environments/member/Status"),
            vec!["Ready"],
            "case {case:?}: the fixture environment must be Ready, got {}",
            ready.body
        );
        let environment_id = ready
            .expect("Environments/member/EnvironmentId", case)
            .to_string();

        let params: Vec<(&str, &str)> = if by_id {
            vec![("EnvironmentId", environment_id.as_str())]
        } else {
            vec![("EnvironmentName", "restart-env-1")]
        };
        let restarted = emulator.call("RestartAppServer", &params).await;
        assert_eq!(
            restarted.status, 200,
            "case {case:?}: expected 200 from RestartAppServer, got {} and body {}",
            restarted.status, restarted.body
        );

        let during = described(&emulator, "restart-env-1").await;
        assert_eq!(
            during.values_at("Environments/member/Status"),
            vec!["Ready"],
            "case {case:?}: a restart does not change the environment's status, got {}",
            during.body
        );
        assert_eq!(
            during.values_at("Environments/member/Health"),
            vec!["Grey"],
            "case {case:?}: health checks are suspended during a restart, got {}",
            during.body
        );

        std::thread::sleep(PAST_DUE);
        let after = described(&emulator, "restart-env-1").await;
        assert_eq!(
            after.values_at("Environments/member/Health"),
            vec!["Green"],
            "case {case:?}: the health before the restart must come back, got {}",
            after.body
        );
        assert_eq!(
            after.values_at("Environments/member/HealthStatus"),
            vec!["Ok"],
            "case {case:?}: the health status before the restart must come back, got {}",
            after.body
        );
    }
}

/// RULE: RebuildEnvironment recreates the environment's resources and forces a
/// restart, so the environment re-enters `Launching` and the clock owes the move
/// back to `Ready`; and an environment that is not `Ready` ("Ready: Environment
/// is available to have an action performed on it") accepts neither a rebuild nor
/// a restart.
#[tokio::test]
async fn rebuild_environment_runs_the_deployment_again_and_only_from_ready() {
    let emulator = Emulator::new();
    fixture(&emulator, "rebuild-app", Some("rebuild-env-1")).await;
    let case = "rebuild";

    let rebuilt = emulator
        .call("RebuildEnvironment", &[("EnvironmentName", "rebuild-env-1")])
        .await;
    assert_eq!(
        rebuilt.status, 200,
        "case {case:?}: expected 200 from RebuildEnvironment, got {} and body {}",
        rebuilt.status, rebuilt.body
    );
    let during = described(&emulator, "rebuild-env-1").await;
    assert_eq!(
        during.values_at("Environments/member/Status"),
        vec!["Launching"],
        "case {case:?}: a rebuilt environment re-enters its deployment, got {}",
        during.body
    );
    assert_eq!(
        during.values_at("Environments/member/Health"),
        vec!["Grey"],
        "case {case:?}: a rebuilding environment reports Grey health, got {}",
        during.body
    );

    // While the rebuild is in progress the environment is not `Ready`, so
    // neither action is accepted — both rows of the same rule.
    for action in ["RebuildEnvironment", "RestartAppServer"] {
        let refused = emulator
            .call(action, &[("EnvironmentName", "rebuild-env-1")])
            .await;
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {action:?} while launching: expected InvalidParameterValue, got {}",
            refused.body
        );
    }

    std::thread::sleep(PAST_DUE);
    let after = described(&emulator, "rebuild-env-1").await;
    assert_eq!(
        after.values_at("Environments/member/Status"),
        vec!["Ready"],
        "case {case:?}: the rebuild the clock owed must complete, got {}",
        after.body
    );
    assert_eq!(
        after.values_at("Environments/member/Health"),
        vec!["Green"],
        "case {case:?}: a rebuilt environment ends healthy, got {}",
        after.body
    );
}

/// RULE: AssociateEnvironmentOperationsRole stores the role on the named
/// environment, replacing whatever it carried, so every later read of that
/// environment reports the last role associated; an environment in the shut-down
/// process has none to associate.
#[tokio::test]
async fn an_associated_operations_role_is_what_every_later_read_reports() {
    let emulator = Emulator::new();
    fixture(&emulator, "ops-role-app", Some("ops-role-env-1")).await;

    // Rows differ along the role value: a plain role, a path-qualified one, and
    // one at the modelled 256-character maximum.
    let long_role = format!(
        "arn:aws:iam::{}:role/{}",
        account(),
        "r".repeat(256 - format!("arn:aws:iam::{}:role/", account()).len())
    );
    let roles = [
        "arn:aws:iam::123456789012:role/beanstalk-ops".to_string(),
        "arn:aws:iam::123456789012:role/team/beanstalk-ops-2".to_string(),
        long_role,
    ];
    for role in &roles {
        let associated = emulator
            .call(
                "AssociateEnvironmentOperationsRole",
                &[
                    ("EnvironmentName", "ops-role-env-1"),
                    ("OperationsRole", role.as_str()),
                ],
            )
            .await;
        assert_eq!(
            associated.status, 200,
            "case {role:?}: expected 200 from AssociateEnvironmentOperationsRole, got {} and body {}",
            associated.status, associated.body
        );
        let read_back = described(&emulator, "ops-role-env-1").await;
        assert_eq!(
            read_back.values_at("Environments/member/OperationsRole"),
            vec![role.as_str()],
            "case {role:?}: the stored operations role must be what is reported, got {}",
            read_back.body
        );
    }

    // A terminating environment acts on nothing downstream.
    let terminated = emulator
        .call(
            "TerminateEnvironment",
            &[("EnvironmentName", "ops-role-env-1")],
        )
        .await;
    assert_eq!(
        terminated.status, 200,
        "case \"terminate\": expected 200, got {} and body {}",
        terminated.status, terminated.body
    );
    let refused = emulator
        .call(
            "AssociateEnvironmentOperationsRole",
            &[
                ("EnvironmentName", "ops-role-env-1"),
                ("OperationsRole", "arn:aws:iam::123456789012:role/late"),
            ],
        )
        .await;
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case \"terminating environment\": expected InvalidParameterValue, got {}",
        refused.body
    );
}

/// RULE: ApplyEnvironmentManagedAction applies a SCHEDULED managed action, so an
/// `ActionId` that names no managed action of the environment is refused with the
/// modelled `ElasticBeanstalkServiceException` (400) rather than reporting an
/// action nobody scheduled. The fixture environment asks for no managed updates
/// (`aws:elasticbeanstalk:managedactions` / `ManagedActionsEnabled` defaults to
/// `false`), so it has no managed action for any id to name.
#[tokio::test]
async fn applying_a_managed_action_nothing_scheduled_is_refused_not_invented() {
    let emulator = Emulator::new();
    fixture(&emulator, "managed-app", Some("managed-env-1")).await;

    // Rows differ along the action id, including the id form the real service
    // issues and a free-form one.
    for action_id in ["1b2c3d4e-5f67-8901-2345-6789abcdef01", "action-1"] {
        let refused = emulator
            .call(
                "ApplyEnvironmentManagedAction",
                &[
                    ("EnvironmentName", "managed-env-1"),
                    ("ActionId", action_id),
                ],
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {action_id:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ElasticBeanstalkServiceException",
            "case {action_id:?}: unexpected error code in {}",
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------
// Reporting on an environment's health
// ---------------------------------------------------------------------------

/// RULE: DescribeEnvironmentHealth carries the environment's name always and
/// every OTHER response attribute only when the request named it (or named
/// `All`): "Specify the response elements to return. To retrieve all attributes,
/// set to All. If no attribute names are specified, returns the name of the
/// environment."
///
/// The attribute names are the modelled `EnvironmentHealthAttribute` set, which
/// `crate::ops::types::ENVIRONMENT_HEALTH_ATTRIBUTE_VALUES` lists: All,
/// ApplicationMetrics, Causes, Color, HealthStatus, InstancesHealth, RefreshedAt,
/// Status.
#[tokio::test]
async fn describe_environment_health_reports_the_attributes_the_request_named() {
    let emulator = Emulator::new();
    fixture(&emulator, "health-app", Some("health-env-1")).await;

    // Every element this response can carry, so each row can assert both what
    // it asked for and what it did NOT ask for.
    let selectable = [
        ("Status", "Status"),
        ("Color", "Color"),
        ("HealthStatus", "HealthStatus"),
        ("Causes", "Causes"),
        ("ApplicationMetrics", "ApplicationMetrics"),
        ("InstancesHealth", "InstancesHealth"),
        ("RefreshedAt", "RefreshedAt"),
    ];

    struct Case {
        asked: Vec<&'static str>,
        present: Vec<&'static str>,
    }
    let cases = [
        // "If no attribute names are specified, returns the name of the
        // environment."
        Case {
            asked: vec![],
            present: vec![],
        },
        Case {
            asked: vec!["Status"],
            present: vec!["Status"],
        },
        // An entry the issue did not cite, paired with another.
        Case {
            asked: vec!["Color", "RefreshedAt"],
            present: vec!["Color", "RefreshedAt"],
        },
        Case {
            asked: vec!["InstancesHealth", "ApplicationMetrics"],
            present: vec!["InstancesHealth", "ApplicationMetrics"],
        },
        Case {
            asked: vec!["All"],
            present: selectable.iter().map(|(name, _)| *name).collect(),
        },
    ];

    for case in cases {
        let name = format!("{:?}", case.asked);
        let mut params: Vec<(String, String)> =
            vec![("EnvironmentName".to_string(), "health-env-1".to_string())];
        for (index, attribute) in case.asked.iter().enumerate() {
            params.push((
                format!("AttributeNames.member.{}", index + 1),
                (*attribute).to_string(),
            ));
        }
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let health = emulator.call("DescribeEnvironmentHealth", &params).await;
        assert_eq!(
            health.status, 200,
            "case {name}: expected 200, got {} and body {}",
            health.status, health.body
        );
        assert_eq!(
            health.value("EnvironmentName"),
            Some("health-env-1"),
            "case {name}: the environment's name is always reported, got {}",
            health.body
        );
        for (attribute, element) in selectable {
            let asked = case.present.contains(&attribute);
            let reported = reports(&health.body, element);
            assert_eq!(
                reported, asked,
                "case {name}: {element} reported={reported} but asked={asked} in {}",
                health.body
            );
        }
    }

    // What the reported values are: the environment's own stored status, health
    // colour and health status, and the one instance it is made of.
    let all = emulator
        .call(
            "DescribeEnvironmentHealth",
            &[
                ("EnvironmentName", "health-env-1"),
                ("AttributeNames.member.1", "All"),
            ],
        )
        .await;
    let case = "All";
    assert_eq!(
        all.expect("Status", case),
        "Ready",
        "case {case:?}: Status is the environment's operational status, got {}",
        all.body
    );
    assert_eq!(
        all.expect("Color", case),
        "Green",
        "case {case:?}: Color is the environment's health colour, got {}",
        all.body
    );
    assert_eq!(
        all.expect("HealthStatus", case),
        "Ok",
        "case {case:?}: HealthStatus is the environment's health status, got {}",
        all.body
    );
    assert_eq!(
        all.expect("InstancesHealth/Ok", case),
        "1",
        "case {case:?}: the one instance of a green, Ready environment is counted in the green bucket, got {}",
        all.body
    );
    assert_eq!(
        all.expect("ApplicationMetrics/RequestCount", case),
        "0",
        "case {case:?}: no application traffic is served, got {}",
        all.body
    );
    assert!(
        all.values_at("Causes/member").is_empty(),
        "case {case:?}: a healthy environment has no causes, got {}",
        all.body
    );

    // A grey environment reports WHY it is grey, from its own status.
    let launching = Emulator::new();
    launching
        .call("CreateApplication", &[("ApplicationName", "grey-app")])
        .await;
    launching
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "grey-app"),
                ("EnvironmentName", "grey-env-1"),
                ("SolutionStackName", solution_stack().as_str()),
            ],
        )
        .await;
    let grey = launching
        .call(
            "DescribeEnvironmentHealth",
            &[
                ("EnvironmentName", "grey-env-1"),
                ("AttributeNames.member.1", "All"),
            ],
        )
        .await;
    assert_eq!(
        grey.expect("Status", "launching"),
        "Launching",
        "case \"launching\": Status follows the stored status, got {}",
        grey.body
    );
    assert_eq!(
        grey.values_at("Causes/member").len(),
        1,
        "case \"launching\": a non-green environment reports its cause, got {}",
        grey.body
    );
}

/// RULE: DescribeInstancesHealth reports the health of the environment's
/// instances — one entry per Amazon EC2 instance the environment is made of,
/// always carrying that instance's id and carrying every other member only when
/// the request's `AttributeNames` selected it — and the time the information was
/// retrieved, never a continuation token for a page that does not exist, and
/// never a token it did not issue.
#[tokio::test]
async fn describe_instances_health_reports_the_instances_an_environment_has() {
    let emulator = Emulator::new();
    fixture(&emulator, "instances-app", Some("instances-env-1")).await;

    // The instances the SAME environment reports being made of, which the
    // health report has to agree with.
    let inventory = emulator
        .call(
            "DescribeEnvironmentResources",
            &[("EnvironmentName", "instances-env-1")],
        )
        .await;
    let instances = inventory.values_at("EnvironmentResources/Instances/member/Id");
    assert!(
        !instances.is_empty(),
        "fixture: the environment reports the instances it is made of, got {}",
        inventory.body
    );

    // Rows differ along the attribute selection, from the modelled
    // `InstancesHealthAttribute` set
    // (`crate::ops::types::INSTANCES_HEALTH_ATTRIBUTE_VALUES`): none, one the
    // issue did not cite, several, and All.
    let selections: [Vec<&str>; 4] = [
        vec![],
        vec!["LaunchedAt"],
        vec!["HealthStatus", "Deployment", "InstanceType"],
        vec!["All"],
    ];
    for selection in selections {
        let name = format!("{selection:?}");
        let mut params: Vec<(String, String)> = vec![(
            "EnvironmentName".to_string(),
            "instances-env-1".to_string(),
        )];
        for (index, attribute) in selection.iter().enumerate() {
            params.push((
                format!("AttributeNames.member.{}", index + 1),
                (*attribute).to_string(),
            ));
        }
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let health = emulator.call("DescribeInstancesHealth", &params).await;
        assert_eq!(
            health.status, 200,
            "case {name}: expected 200, got {} and body {}",
            health.status, health.body
        );
        assert!(
            health.value("RefreshedAt").is_some(),
            "case {name}: expected a RefreshedAt, got {}",
            health.body
        );
        // "If no attribute names are specified, returns a list of instances":
        // the instance id is reported whatever was selected.
        assert_eq!(
            health.values_at("InstanceHealthList/member/InstanceId"),
            instances,
            "case {name}: the report covers the instances the environment is made of, got {}",
            health.body
        );
        // Every other member is reported only when its own attribute was
        // selected.
        for attribute in [
            "HealthStatus",
            "Color",
            "LaunchedAt",
            "Deployment",
            "InstanceType",
            "AvailabilityZone",
            "ApplicationMetrics",
        ] {
            let asked = selection.contains(&attribute) || selection.contains(&"All");
            let reported = reports(&health.body, attribute);
            assert_eq!(
                reported, asked,
                "case {name}: {attribute} reported={reported} but asked={asked} in {}",
                health.body
            );
        }
        assert!(
            !reports(&health.body, "NextToken"),
            "case {name}: there is no further page to continue from, got {}",
            health.body
        );
    }

    let refused = emulator
        .call(
            "DescribeInstancesHealth",
            &[
                ("EnvironmentName", "instances-env-1"),
                ("NextToken", "not-a-token-this-service-issued"),
            ],
        )
        .await;
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case \"foreign NextToken\": expected InvalidParameterValue, got {}",
        refused.body
    );
}

// ---------------------------------------------------------------------------
// ComposeEnvironments
// ---------------------------------------------------------------------------

/// RULE: ComposeEnvironments names each environment from the `env.yaml` manifest
/// inside the version's source bundle; this emulator holds a source bundle only
/// as the S3 reference the caller passed and never reads its content, so no
/// environment can be named and the request is refused with the service's
/// `MissingParameter` — for every version label, whichever application it
/// belongs to, and no environment is created either way.
#[tokio::test]
async fn compose_environments_refuses_what_no_manifest_can_name() {
    let emulator = Emulator::new();
    fixture(&emulator, "compose-app", None).await;
    fixture(&emulator, "other-app", None).await;

    struct Case {
        name: &'static str,
        params: Vec<(&'static str, &'static str)>,
    }
    let cases = [
        Case {
            name: "no version labels",
            params: vec![("ApplicationName", "compose-app")],
        },
        Case {
            name: "a version of the named application",
            params: vec![
                ("ApplicationName", "compose-app"),
                ("VersionLabels.member.1", "v1"),
            ],
        },
        Case {
            name: "a version of another application",
            params: vec![
                ("ApplicationName", "other-app"),
                ("VersionLabels.member.1", "v1"),
                ("GroupName", "group-1"),
            ],
        },
        Case {
            name: "several version labels",
            params: vec![
                ("ApplicationName", "compose-app"),
                ("VersionLabels.member.1", "v1"),
                ("VersionLabels.member.2", "v2"),
            ],
        },
    ];

    for case in cases {
        let name = case.name;
        let refused = emulator.call("ComposeEnvironments", &case.params).await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "MissingParameter",
            "case {name:?}: unexpected error code in {}",
            refused.body
        );
    }

    let environments = emulator.call("DescribeEnvironments", &[]).await;
    assert!(
        environments
            .values_at("Environments/member/EnvironmentId")
            .is_empty(),
        "case \"no environment composed\": a refused compose creates nothing, got {}",
        environments.body
    );
}

// ---------------------------------------------------------------------------
// The application's resource lifecycle configuration
// ---------------------------------------------------------------------------

/// RULE: a version lifecycle configuration needs a service role to apply it with
/// — "The ServiceRole property is required the first time that you provide a
/// VersionLifecycleConfig for the application in one of the supporting calls
/// (CreateApplication or UpdateApplicationResourceLifecycle). After you provide
/// it once … Elastic Beanstalk persists the Service Role with the application,
/// and you don't need to specify it again" — and an accepted configuration is
/// stored, echoed back, and reported by a later read of the application.
#[tokio::test]
async fn a_version_lifecycle_configuration_needs_a_service_role_and_then_persists() {
    let emulator = Emulator::new();
    fixture(&emulator, "lifecycle-app", None).await;
    let service_role = format!("arn:aws:iam::{}:role/aws-elasticbeanstalk-service-role", account());

    // Rows differ along WHICH rule the configuration enables and whether the
    // request carries the role: both lifecycle rules the model declares, each
    // with and without one.
    struct Case {
        name: &'static str,
        rule: Vec<(&'static str, String)>,
        with_role: bool,
        expected_status: u16,
    }
    let cases = [
        Case {
            name: "max count rule without a service role",
            rule: vec![
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true".to_string(),
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "5".to_string(),
                ),
            ],
            with_role: false,
            expected_status: 400,
        },
        Case {
            name: "max age rule without a service role",
            rule: vec![
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.Enabled",
                    "true".to_string(),
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxAgeRule.MaxAgeInDays",
                    "30".to_string(),
                ),
            ],
            with_role: false,
            expected_status: 400,
        },
        Case {
            name: "max count rule with a service role",
            rule: vec![
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true".to_string(),
                ),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.MaxCount",
                    "7".to_string(),
                ),
            ],
            with_role: true,
            expected_status: 200,
        },
    ];

    for case in cases {
        let name = case.name;
        let mut params: Vec<(String, String)> = vec![(
            "ApplicationName".to_string(),
            "lifecycle-app".to_string(),
        )];
        if case.with_role {
            params.push((
                "ResourceLifecycleConfig.ServiceRole".to_string(),
                service_role.clone(),
            ));
        }
        for (key, value) in &case.rule {
            params.push(((*key).to_string(), value.clone()));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let answered = emulator
            .call("UpdateApplicationResourceLifecycle", &borrowed)
            .await;
        assert_eq!(
            answered.status, case.expected_status,
            "case {name:?}: expected HTTP {}, got {} and body {}",
            case.expected_status, answered.status, answered.body
        );
        if case.expected_status == 400 {
            assert_eq!(
                answered.error_code(),
                "InvalidParameterValue",
                "case {name:?}: unexpected error code in {}",
                answered.body
            );
        } else {
            assert_eq!(
                answered.expect("ApplicationName", name),
                "lifecycle-app",
                "case {name:?}: the answer names the application it updated, got {}",
                answered.body
            );
            assert_eq!(
                answered.expect("ResourceLifecycleConfig/ServiceRole", name),
                service_role,
                "case {name:?}: the stored service role is echoed back, got {}",
                answered.body
            );
            assert_eq!(
                answered.expect("MaxCountRule/MaxCount", name),
                "7",
                "case {name:?}: the stored rule is echoed back, got {}",
                answered.body
            );
        }
    }

    // The accepted configuration is what a later read of the application
    // reports — the round trip through the store.
    let described = emulator
        .call(
            "DescribeApplications",
            &[("ApplicationNames.member.1", "lifecycle-app")],
        )
        .await;
    let case = "round trip";
    assert_eq!(
        described.expect("ResourceLifecycleConfig/ServiceRole", case),
        service_role,
        "case {case:?}: DescribeApplications must report the stored role, got {}",
        described.body
    );
    assert_eq!(
        described.expect("MaxCountRule/MaxCount", case),
        "7",
        "case {case:?}: DescribeApplications must report the stored rule, got {}",
        described.body
    );

    // "After you provide it once … you don't need to specify it again": a later
    // update without the role keeps the one already stored.
    let again = emulator
        .call(
            "UpdateApplicationResourceLifecycle",
            &[
                ("ApplicationName", "lifecycle-app"),
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
    let case = "role already persisted";
    assert_eq!(
        again.status, 200,
        "case {case:?}: expected 200, got {} and body {}",
        again.status, again.body
    );
    assert_eq!(
        again.expect("ResourceLifecycleConfig/ServiceRole", case),
        service_role,
        "case {case:?}: the persisted role must survive an update that omits it, got {}",
        again.body
    );
    assert_eq!(
        again.expect("MaxCountRule/MaxCount", case),
        "9",
        "case {case:?}: the new rule must be stored, got {}",
        again.body
    );

    // The same rule on the other supporting call.
    let created = emulator
        .call(
            "CreateApplication",
            &[
                ("ApplicationName", "lifecycle-app-2"),
                (
                    "ResourceLifecycleConfig.VersionLifecycleConfig.MaxCountRule.Enabled",
                    "true",
                ),
            ],
        )
        .await;
    assert_eq!(
        created.error_code(),
        "InvalidParameterValue",
        "case \"CreateApplication without a service role\": expected InvalidParameterValue, got {}",
        created.body
    );

    // An application that does not exist cannot carry a configuration.
    let unknown = emulator
        .call(
            "UpdateApplicationResourceLifecycle",
            &[
                ("ApplicationName", "never-created-app"),
                ("ResourceLifecycleConfig.ServiceRole", service_role.as_str()),
            ],
        )
        .await;
    assert_eq!(
        unknown.error_code(),
        "InvalidParameterValue",
        "case \"unknown application\": expected InvalidParameterValue, got {}",
        unknown.body
    );
}

// ---------------------------------------------------------------------------
// Tagging
// ---------------------------------------------------------------------------

/// The ARN of each taggable resource the fixture below creates.
fn resource_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "application",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/tag-app",
                account()
            ),
        ),
        (
            "application version",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:applicationversion/tag-app/v1",
                account()
            ),
        ),
        (
            "environment",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:environment/tag-app/tag-env-1",
                account()
            ),
        ),
    ]
}

/// RULE: UpdateTagsForResource adds or updates the tags in `TagsToAdd` and drops
/// the keys in `TagsToRemove` on the Elastic Beanstalk resource the ARN names,
/// and ListTagsForResource reports exactly what is stored — for every taggable
/// resource kind the service mints an ARN for.
#[tokio::test]
async fn tags_written_through_an_arn_are_the_tags_read_back() {
    let emulator = Emulator::new();
    fixture(&emulator, "tag-app", Some("tag-env-1")).await;

    for (kind, arn) in resource_arns() {
        let added = emulator
            .call(
                "UpdateTagsForResource",
                &[
                    ("ResourceArn", arn.as_str()),
                    ("TagsToAdd.member.1.Key", "env"),
                    ("TagsToAdd.member.1.Value", "dev"),
                    ("TagsToAdd.member.2.Key", "team"),
                    ("TagsToAdd.member.2.Value", "platform"),
                ],
            )
            .await;
        assert_eq!(
            added.status, 200,
            "case {kind:?}: expected 200 from UpdateTagsForResource, got {} and body {}",
            added.status, added.body
        );

        let listed = emulator
            .call("ListTagsForResource", &[("ResourceArn", arn.as_str())])
            .await;
        assert_eq!(
            listed.expect("ResourceArn", kind),
            arn,
            "case {kind:?}: the answer names the resource asked about, got {}",
            listed.body
        );
        assert_eq!(
            listed.values_at("ResourceTags/member/Key"),
            vec!["env", "team"],
            "case {kind:?}: the stored tag keys must come back, got {}",
            listed.body
        );
        assert_eq!(
            listed.values_at("ResourceTags/member/Value"),
            vec!["dev", "platform"],
            "case {kind:?}: the stored tag values must come back, got {}",
            listed.body
        );

        // "If a key of an existing tag is added, the tag's value is updated." /
        // "A list of tag keys to remove. If a tag key doesn't exist, it is
        // silently ignored."
        let updated = emulator
            .call(
                "UpdateTagsForResource",
                &[
                    ("ResourceArn", arn.as_str()),
                    ("TagsToAdd.member.1.Key", "env"),
                    ("TagsToAdd.member.1.Value", "prod"),
                    ("TagsToRemove.member.1", "team"),
                    ("TagsToRemove.member.2", "never-set"),
                ],
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {kind:?}: expected 200 from the second update, got {} and body {}",
            updated.status, updated.body
        );
        let listed = emulator
            .call("ListTagsForResource", &[("ResourceArn", arn.as_str())])
            .await;
        assert_eq!(
            listed.values_at("ResourceTags/member/Key"),
            vec!["env"],
            "case {kind:?}: a removed key must be gone, got {}",
            listed.body
        );
        assert_eq!(
            listed.values_at("ResourceTags/member/Value"),
            vec!["prod"],
            "case {kind:?}: an added key must update the value, got {}",
            listed.body
        );
    }

    // Neither list given: there is nothing to do and nothing to guess at.
    let (_, application_arn) = resource_arns().remove(0);
    let empty = emulator
        .call(
            "UpdateTagsForResource",
            &[("ResourceArn", application_arn.as_str())],
        )
        .await;
    assert_eq!(
        empty.error_code(),
        "MissingParameter",
        "case \"neither TagsToAdd nor TagsToRemove\": expected MissingParameter, got {}",
        empty.body
    );

    // A name that itself contains the ARN's own separator still resolves to the
    // record whose ARN the service minted.
    let slashed = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", "tag-app"),
                ("VersionLabel", "release/1.0"),
            ],
        )
        .await;
    assert_eq!(
        slashed.status, 200,
        "case \"version label with a slash\": expected 200 from CreateApplicationVersion, got {} and body {}",
        slashed.status, slashed.body
    );
    let arn = slashed.expect(
        "ApplicationVersion/ApplicationVersionArn",
        "version label with a slash",
    );
    let tagged = emulator
        .call(
            "UpdateTagsForResource",
            &[
                ("ResourceArn", arn),
                ("TagsToAdd.member.1.Key", "env"),
                ("TagsToAdd.member.1.Value", "dev"),
            ],
        )
        .await;
    assert_eq!(
        tagged.status, 200,
        "case \"version label with a slash\": expected 200 from UpdateTagsForResource, got {} and body {}",
        tagged.status, tagged.body
    );
    let listed = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn)])
        .await;
    assert_eq!(
        listed.values_at("ResourceTags/member/Key"),
        vec!["env"],
        "case \"version label with a slash\": the stored tag must come back, got {}",
        listed.body
    );
}

/// RULE: a `ResourceArn` that names no taggable resource of this service is
/// refused by BOTH tag operations, with the code that says which way it failed:
/// `InvalidParameterValue` for a value that is not this service's ARN at all
/// ("Must be the ARN of an Elastic Beanstalk resource"),
/// `ResourceTypeNotSupportedException` for an Elastic Beanstalk ARN naming a type
/// this service does not tag, and `ResourceNotFoundException` for a taggable type
/// whose resource does not exist.
#[tokio::test]
async fn an_arn_that_names_no_beanstalk_resource_is_refused_by_both_tag_operations() {
    let emulator = Emulator::new();
    fixture(&emulator, "tag-app", Some("tag-env-1")).await;

    // Rows differ along WHY the ARN resolves to nothing: another service's ARN,
    // a value that is no ARN, a type this service does not tag, and three
    // taggable types with no record behind them.
    let cases: [(&str, String, &str); 8] = [
        (
            "another service's ARN",
            "arn:aws:sqs:us-east-1:123456789012:a-queue".to_string(),
            "InvalidParameterValue",
        ),
        (
            "not an ARN at all",
            "tag-app".to_string(),
            "InvalidParameterValue",
        ),
        (
            "an ARN with no resource part",
            format!("arn:aws:elasticbeanstalk:{TEST_REGION}:{}:", account()),
            "InvalidParameterValue",
        ),
        (
            "an unsupported resource type",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:widget/tag-app",
                account()
            ),
            "ResourceTypeNotSupportedException",
        ),
        (
            "an application that does not exist",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/never-created",
                account()
            ),
            "ResourceNotFoundException",
        ),
        (
            "a version label that does not exist",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:applicationversion/tag-app/v99",
                account()
            ),
            "ResourceNotFoundException",
        ),
        (
            "an environment under the wrong application",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:environment/other-app/tag-env-1",
                account()
            ),
            "ResourceNotFoundException",
        ),
        (
            "a configuration template this emulator holds no record of",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:configurationtemplate/tag-app/template-1",
                account()
            ),
            "ResourceNotFoundException",
        ),
    ];

    for (name, arn, expected) in cases {
        let written = emulator
            .call(
                "UpdateTagsForResource",
                &[
                    ("ResourceArn", arn.as_str()),
                    ("TagsToAdd.member.1.Key", "env"),
                    ("TagsToAdd.member.1.Value", "dev"),
                ],
            )
            .await;
        assert_eq!(
            written.status, 400,
            "case {name:?}: expected HTTP 400 from UpdateTagsForResource, got {} and body {}",
            written.status, written.body
        );
        assert_eq!(
            written.error_code(),
            expected,
            "case {name:?}: unexpected UpdateTagsForResource error code in {}",
            written.body
        );
        let read = emulator
            .call("ListTagsForResource", &[("ResourceArn", arn.as_str())])
            .await;
        assert_eq!(
            read.error_code(),
            expected,
            "case {name:?}: unexpected ListTagsForResource error code in {}",
            read.body
        );
    }
}

/// RULE: a tag update is refused with `TooManyTagsException` when the tags the
/// resource would be left with exceed the per-resource quota of
/// [`eb::state::MAX_TAGS_PER_RESOURCE`] — counting "both the number of tags the
/// resource already has and the tags this operation would add if it succeeded",
/// so a removal in the same call makes room.
#[tokio::test]
async fn the_per_resource_tag_quota_bounds_what_an_update_can_leave() {
    let emulator = Emulator::new();
    fixture(&emulator, "tag-app", None).await;
    let arn = format!(
        "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/tag-app",
        account()
    );
    let quota = eb::state::MAX_TAGS_PER_RESOURCE;

    /// `count` tag parameters, keyed `k<n>`, starting at `from`.
    fn tag_params(from: usize, count: usize) -> Vec<(String, String)> {
        let mut params = Vec::new();
        for index in 0..count {
            params.push((
                format!("TagsToAdd.member.{}.Key", index + 1),
                format!("k{}", from + index),
            ));
            params.push((
                format!("TagsToAdd.member.{}.Value", index + 1),
                format!("v{}", from + index),
            ));
        }
        params
    }

    // The published bound, and one past it, in a single call.
    for (name, count, expected_status) in [
        ("one past the quota in one call", quota + 1, 400u16),
        ("exactly the quota in one call", quota, 200u16),
    ] {
        let fresh = Emulator::new();
        fixture(&fresh, "tag-app", None).await;
        let mut params: Vec<(String, String)> =
            vec![("ResourceArn".to_string(), arn.clone())];
        params.extend(tag_params(0, count));
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let answered = fresh.call("UpdateTagsForResource", &borrowed).await;
        assert_eq!(
            answered.status, expected_status,
            "case {name:?}: expected HTTP {expected_status}, got {} and body {}",
            answered.status, answered.body
        );
        if expected_status == 400 {
            assert_eq!(
                answered.error_code(),
                "TooManyTagsException",
                "case {name:?}: unexpected error code in {}",
                answered.body
            );
        }
    }

    // At the quota, one more key is refused — and the refusal changes nothing.
    let mut params: Vec<(String, String)> = vec![("ResourceArn".to_string(), arn.clone())];
    params.extend(tag_params(0, quota));
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let filled = emulator.call("UpdateTagsForResource", &borrowed).await;
    assert_eq!(
        filled.status, 200,
        "case \"fill to the quota\": expected 200, got {} and body {}",
        filled.status, filled.body
    );
    let over = emulator
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
        over.error_code(),
        "TooManyTagsException",
        "case \"one past a full resource\": expected TooManyTagsException, got {}",
        over.body
    );
    let listed = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn.as_str())])
        .await;
    assert_eq!(
        listed.values_at("ResourceTags/member/Key").len(),
        quota,
        "case \"one past a full resource\": the refusal must store nothing, got {}",
        listed.body
    );

    // A request whose own lists are longer than the quota is refused on the
    // request alone: it could not leave a legal tag set whatever the resource
    // holds.
    let mut oversized: Vec<(String, String)> = vec![("ResourceArn".to_string(), arn.clone())];
    for index in 0..=quota {
        oversized.push((
            format!("TagsToRemove.member.{}", index + 1),
            format!("k{index}"),
        ));
    }
    let borrowed: Vec<(&str, &str)> = oversized
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let too_many_removals = emulator.call("UpdateTagsForResource", &borrowed).await;
    assert_eq!(
        too_many_removals.error_code(),
        "TooManyTagsException",
        "case \"more keys to remove than a resource can hold\": expected TooManyTagsException, got {}",
        too_many_removals.body
    );

    // Updating an existing key is not a new tag, and a removal in the same call
    // makes room for an addition.
    let replaced = emulator
        .call(
            "UpdateTagsForResource",
            &[
                ("ResourceArn", arn.as_str()),
                ("TagsToAdd.member.1.Key", "k0"),
                ("TagsToAdd.member.1.Value", "changed"),
            ],
        )
        .await;
    assert_eq!(
        replaced.status, 200,
        "case \"update at the quota\": expected 200, got {} and body {}",
        replaced.status, replaced.body
    );
    let swapped = emulator
        .call(
            "UpdateTagsForResource",
            &[
                ("ResourceArn", arn.as_str()),
                ("TagsToRemove.member.1", "k1"),
                ("TagsToAdd.member.1.Key", "swapped-in"),
                ("TagsToAdd.member.1.Value", "v"),
            ],
        )
        .await;
    assert_eq!(
        swapped.status, 200,
        "case \"remove then add at the quota\": expected 200, got {} and body {}",
        swapped.status, swapped.body
    );
    let listed = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn.as_str())])
        .await;
    let keys = listed.values_at("ResourceTags/member/Key");
    assert_eq!(
        keys.len(),
        quota,
        "case \"remove then add at the quota\": the resource stays at the quota, got {}",
        listed.body
    );
    assert!(
        keys.contains(&"swapped-in") && !keys.contains(&"k1"),
        "case \"remove then add at the quota\": expected k1 gone and swapped-in present, got {keys:?}"
    );
}
