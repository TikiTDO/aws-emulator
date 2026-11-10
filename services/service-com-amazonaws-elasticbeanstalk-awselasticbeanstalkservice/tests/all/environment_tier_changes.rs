// SPDX-License-Identifier: CC0-1.0

//! The environment TIER an `UpdateEnvironment` may ask for, driven end to end
//! over this service's awsQuery surface.
//!
//! RULE under test: the tier an environment runs is fixed at its launch —
//! `UpdateEnvironment$Tier` is documented "at this time, if you change the tier
//! version, name, or type, Elastic Beanstalk returns `InvalidParameterValue`
//! error" — so the only tier the platform version it runs supports FOR IT is the
//! one it already has. A request stating a different `Name`, `Type` or `Version`
//! is refused with `InvalidParameterValue`, HTTP 400 and
//! `Unsupported environment tier <Name> version <Version> for solution stack '<stack>'`,
//! naming the tier the request asked for (its own members where it stated them,
//! the environment's where it did not) and the platform version the environment
//! runs; a tier restated unchanged changes nothing and is accepted. The refusal
//! is about the REQUEST against that platform version, neither of which an
//! operation in flight changes, so it is the answer whatever status the
//! environment is in — a tier diagnosis is never masked by a transient one.
//!
//! The rows span what the rule generalises over: WHICH tier member the request
//! changes (each on its own and all three together), what it changes to (the
//! other tier of the two an environment runs in, the third tier the model
//! documents, a tier name no tier list carries, another version), and the tier
//! the environment ALREADY runs (a web-server environment and a worker one), so
//! the sentence is not keyed to one tier.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns:
//! nothing here reaches into the store, so what is asserted is what a client
//! sees.

use crate::harness::{Answer, Emulator, PAST_DUE, SOLUTION_STACK};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application the environments below belong to.
const APPLICATION: &str = "tier-app";

/// `EnvironmentTier$Type` of the worker tier, from the member's own
/// documentation ("For Worker tier — `SQS/HTTP`").
const TIER_TYPE_SQS_HTTP: &str = "SQS/HTTP";
/// `EnvironmentTier$Name` of the third tier the member documents ("For
/// Cluster-mode Amazon EKS-based applications — `Cluster`"), and its type ("For
/// Cluster tier — `EKS`").
const TIER_NAME_CLUSTER: &str = "Cluster";
/// See [`TIER_NAME_CLUSTER`].
const TIER_TYPE_EKS: &str = "EKS";

/// One tier an update asks for: the `Tier` members it states.
struct TierCase {
    case: &'static str,
    name: Option<&'static str>,
    tier_type: Option<&'static str>,
    version: Option<&'static str>,
}

impl TierCase {
    /// The `Tier.*` parameters this row sends.
    fn params(&self) -> Vec<(String, String)> {
        let mut params = Vec::new();
        for (member, value) in [
            ("Tier.Name", self.name),
            ("Tier.Type", self.tier_type),
            ("Tier.Version", self.version),
        ] {
            if let Some(value) = value {
                params.push((member.to_string(), value.to_string()));
            }
        }
        params
    }

    /// The message the refusal must carry for an environment running the tier
    /// `(current_name, current_version)` on `solution_stack`: the request's own
    /// name and version where it stated them, the environment's where it did
    /// not.
    fn message(&self, current_name: &str, current_version: &str, solution_stack: &str) -> String {
        eb::state::unsupported_environment_tier_message(
            self.name.unwrap_or(current_name),
            self.version.unwrap_or(current_version),
            solution_stack,
        )
    }
}

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

/// The `(Name, Type, Version)` `DescribeEnvironments` reports for `environment`.
async fn reported_tier(emulator: &Emulator, case: &str, environment: &str) -> (String, String, String) {
    let described = given(
        emulator,
        case,
        "DescribeEnvironments",
        &[("EnvironmentNames.member.1", environment)],
    )
    .await;
    (
        described
            .expect("Environments/member/Tier/Name", case)
            .to_string(),
        described
            .expect("Environments/member/Tier/Type", case)
            .to_string(),
        described
            .expect("Environments/member/Tier/Version", case)
            .to_string(),
    )
}

/// Launch `environment` on the published fixture stack, in the tier
/// `(name, tier_type)` when one is named, and wait the launch out.
async fn launch(emulator: &Emulator, environment: &str, tier: Option<(&str, &str)>) {
    let mut params = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), environment.to_string()),
        ("SolutionStackName".to_string(), SOLUTION_STACK.to_string()),
    ];
    if let Some((name, tier_type)) = tier {
        params.push(("Tier.Name".to_string(), name.to_string()));
        params.push(("Tier.Type".to_string(), tier_type.to_string()));
    }
    given(emulator, environment, "CreateEnvironment", &as_params(&params)).await;
    std::thread::sleep(PAST_DUE);
}

#[tokio::test]
async fn a_tier_the_environments_platform_does_not_support_is_refused_naming_tier_and_platform() {
    let emulator = Emulator::new();
    given(
        &emulator,
        "setup",
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;

    // The two tiers an Elastic Beanstalk environment runs in, so the refusal is
    // asserted for a request that asks a web-server environment for the worker
    // tier AND for one that asks a worker environment for the web-server tier.
    let web_server = (
        eb::state::TIER_NAME_WEB_SERVER,
        eb::state::TIER_TYPE_STANDARD,
    );
    let worker = (eb::state::TIER_NAME_WORKER, TIER_TYPE_SQS_HTTP);
    let environments = [
        ("tier-web-env", web_server, worker),
        ("tier-worker-env", worker, web_server),
    ];

    for (environment, current, other) in environments {
        launch(&emulator, environment, Some(current)).await;

        // Rows differ along which member changes and what it changes to.
        let rows = [
            TierCase {
                case: "the other tier's name alone",
                name: Some(other.0),
                tier_type: None,
                version: None,
            },
            TierCase {
                case: "the other tier's type alone",
                name: None,
                tier_type: Some(other.1),
                version: None,
            },
            TierCase {
                case: "another tier version alone",
                name: None,
                tier_type: None,
                version: Some("1.1"),
            },
            TierCase {
                case: "the whole other tier, version included",
                name: Some(other.0),
                tier_type: Some(other.1),
                version: Some(eb::state::DEFAULT_TIER_VERSION),
            },
            TierCase {
                case: "the cluster tier the member documents",
                name: Some(TIER_NAME_CLUSTER),
                tier_type: Some(TIER_TYPE_EKS),
                version: None,
            },
            TierCase {
                case: "a tier name no tier list carries",
                name: Some("Nonesuch"),
                tier_type: None,
                version: Some("9.9"),
            },
        ];

        for row in &rows {
            let case = format!("{environment}: {}", row.case);
            let mut params = vec![("EnvironmentName".to_string(), environment.to_string())];
            params.extend(row.params());
            let refused = emulator
                .call("UpdateEnvironment", &as_params(&params))
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
                row.message(current.0, eb::state::DEFAULT_TIER_VERSION, &SOLUTION_STACK),
                "case {case:?}: unexpected refusal message, got body {}",
                refused.body
            );
        }

        // A refused update changed nothing: the environment still runs the tier
        // it was launched in.
        assert_eq!(
            reported_tier(&emulator, environment, environment).await,
            (
                current.0.to_string(),
                current.1.to_string(),
                eb::state::DEFAULT_TIER_VERSION.to_string(),
            ),
            "case {environment:?}: a refusal must not change the environment's tier"
        );
    }
}

#[tokio::test]
async fn a_tier_refusal_is_answered_while_an_update_is_still_in_flight() {
    let emulator = Emulator::new();
    given(
        &emulator,
        "setup",
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let environment = "tier-busy-env";
    launch(&emulator, environment, None).await;

    // An update that changes the configuration: the environment is `Updating`
    // from here until the clock completes it.
    given(
        &emulator,
        "an update in flight",
        "UpdateEnvironment",
        &[
            ("EnvironmentName", environment),
            ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
            ("OptionSettings.member.1.OptionName", "MinSize"),
            ("OptionSettings.member.1.Value", "2"),
        ],
    )
    .await;

    // The tier rule is about the request and the platform the environment runs,
    // so it is the answer even now — not the environment's transient status.
    let case = "a tier change while updating";
    let refused = emulator
        .call(
            "UpdateEnvironment",
            &[
                ("EnvironmentName", environment),
                ("Tier.Name", eb::state::TIER_NAME_WORKER),
                ("Tier.Type", TIER_TYPE_SQS_HTTP),
                ("Tier.Version", eb::state::DEFAULT_TIER_VERSION),
            ],
        )
        .await;
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case {case:?}: unexpected error code, got body {}",
        refused.body
    );
    assert_eq!(
        refused.error_message(),
        eb::state::unsupported_environment_tier_message(
            eb::state::TIER_NAME_WORKER,
            eb::state::DEFAULT_TIER_VERSION,
            &SOLUTION_STACK,
        ),
        "case {case:?}: the tier refusal must not be masked by the status, got body {}",
        refused.body
    );

    // The update the clock owed has completed, so the environment takes an
    // update again — and a tier restated unchanged is one of them.
    std::thread::sleep(PAST_DUE);
    let case = "the tier restated once the update has landed";
    let updated = given(
        &emulator,
        case,
        "UpdateEnvironment",
        &[
            ("EnvironmentName", environment),
            ("Tier.Name", eb::state::TIER_NAME_WEB_SERVER),
            ("Tier.Type", eb::state::TIER_TYPE_STANDARD),
            ("Tier.Version", eb::state::DEFAULT_TIER_VERSION),
        ],
    )
    .await;
    assert_eq!(
        updated.value("Tier/Name"),
        Some(eb::state::TIER_NAME_WEB_SERVER),
        "case {case:?}: the environment keeps the tier it runs, got body {}",
        updated.body
    );
    assert_eq!(
        reported_tier(&emulator, case, environment).await,
        (
            eb::state::TIER_NAME_WEB_SERVER.to_string(),
            eb::state::TIER_TYPE_STANDARD.to_string(),
            eb::state::DEFAULT_TIER_VERSION.to_string(),
        ),
        "case {case:?}: the stored tier must be the one the environment was launched in"
    );
}
