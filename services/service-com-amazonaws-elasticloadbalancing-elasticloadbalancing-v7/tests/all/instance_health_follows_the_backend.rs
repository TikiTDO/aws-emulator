// SPDX-License-Identifier: CC0-1.0

//! What `DescribeInstanceHealth` reports for a registered instance, driven over the wire
//! against the BACKEND the registration names.
//!
//! The rule asserted here: **the state of a registered instance is the outcome of the load
//! balancer's health checks of it — `InService` with `ReasonCode` and `Description` of
//! `N/A` once `HealthyThreshold` consecutive checks were answered, `OutOfService` with
//! `ReasonCode` `Instance` and the condition the checks ran into once `UnhealthyThreshold`
//! consecutive checks were not, and the registration's own checks still in progress until
//! one of those thresholds is reached.** A check is answered while something stands behind
//! the registration, which for an instance means EC2 — the service that owns instances —
//! reporting it running.
//!
//! The rows differ along what the rule generalises over: what the backend is doing
//! (running, stopped, terminated, an id the account holds no instance for) and what the
//! load balancer's two thresholds are configured to, including both bounds
//! `HealthyThreshold`/`UnhealthyThreshold` are modelled with (`@range(min: 2, max: 10)`).
//! The instance is launched, stopped and terminated through EC2's OWN API on the same
//! engine, so what a row asserts rests on what EC2's own reader reports rather than on a
//! value this suite made up.
//!
//! Where a row stands on its load balancer's timeline is placed by moving the stored
//! registration back ([`crate::state::InstanceSet::restamp`], the seam the sibling
//! timeline suite uses), never by sleeping for the checks.

use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::{
    DESCRIPTION_INSTANCE_STOPPED, DESCRIPTION_INSTANCE_TERMINATED, HEALTH_CHECK_SPAN,
    HEALTH_FAILED_HEALTH_CHECKS, HEALTH_IN_SERVICE, HEALTH_REGISTRATION_IN_PROGRESS,
    InstanceHealth, REASON_CODE_INSTANCE, STATE_OUT_OF_SERVICE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::wire::{
    age_registration, call, emulator_with_ec2, kid_text, launch_instance, members, param, parse,
    settle_ec2_transition,
};

/// The region every call here is signed for.
const REGION: &str = "us-east-1";

/// Drive one EC2 instance-state action over `instance_id`: the shared seam, in this suite's
/// own region.
async fn move_instance(router: &MultiServiceRouter, case: &str, action: &str, instance_id: &str) {
    super::wire::move_instance(router, REGION, case, action, instance_id).await;
}

/// The health triple of one `InstanceState` member: `(State, ReasonCode, Description)`.
type Triple = (Option<String>, Option<String>, Option<String>);

/// The triple a stored `InstanceHealth` is reported as on the wire, so every expectation
/// is the service's own constant rather than a sentence retyped here.
fn triple(health: InstanceHealth) -> Triple {
    (
        Some(health.state.to_string()),
        Some(health.reason_code.to_string()),
        Some(health.description.to_string()),
    )
}

/// The health of an instance whose checks the load balancer decided against, with the
/// condition those checks ran into: `OutOfService` for the INSTANCE's own reason, which is
/// what `InstanceState$ReasonCode` words as `Instance`.
fn out_of_service(description: &str) -> Triple {
    (
        Some(STATE_OUT_OF_SERVICE.to_string()),
        Some(REASON_CODE_INSTANCE.to_string()),
        Some(description.to_string()),
    )
}

/// What the backend behind a registration is doing — what one health check of it observes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// An instance EC2 launched and nothing has moved since: EC2 reports it running.
    Running,
    /// A launched instance that was then stopped through EC2.
    Stopped,
    /// A launched instance that was then terminated through EC2.
    Terminated,
    /// An id the account holds no instance for at all.
    Absent,
}

/// One row: a load balancer, the thresholds its health check is configured with, and the
/// backend its one registered instance names.
#[derive(Debug, Clone, Copy)]
struct Case {
    /// Also the load balancer's name.
    name: &'static str,
    healthy_threshold: i32,
    unhealthy_threshold: i32,
    backend: Backend,
}

impl Case {
    /// Whether a check of this row's backend is answered.
    fn answered(self) -> bool {
        self.backend == Backend::Running
    }

    /// The threshold this row's verdict is reached on: the healthy one for a backend that
    /// answers its checks, the unhealthy one for a backend that answers none.
    fn deciding_threshold(self) -> u32 {
        let configured = if self.answered() {
            self.healthy_threshold
        } else {
            self.unhealthy_threshold
        };
        u32::try_from(configured).expect("a configured threshold")
    }

    /// The health this row's instance ends up reporting.
    fn expected(self) -> Triple {
        match self.backend {
            Backend::Running => triple(HEALTH_IN_SERVICE),
            Backend::Stopped => out_of_service(DESCRIPTION_INSTANCE_STOPPED),
            Backend::Terminated => out_of_service(DESCRIPTION_INSTANCE_TERMINATED),
            // Nothing stands behind the registration, so its checks fail as the
            // instance's own doing.
            Backend::Absent => triple(HEALTH_FAILED_HEALTH_CHECKS),
        }
    }
}

/// The rows: every backend state a check can observe, against thresholds at both modelled
/// bounds and between them.
fn health_cases() -> Vec<Case> {
    vec![
        Case {
            name: "backend-running-low-thresholds",
            healthy_threshold: 2,
            unhealthy_threshold: 2,
            backend: Backend::Running,
        },
        Case {
            // `HealthyThreshold` at the maximum the member is modelled with: the run of
            // answered checks the verdict waits for is the caller's configuration.
            name: "backend-running-high-healthy",
            healthy_threshold: 10,
            unhealthy_threshold: 2,
            backend: Backend::Running,
        },
        Case {
            name: "backend-stopped",
            healthy_threshold: 2,
            unhealthy_threshold: 2,
            backend: Backend::Stopped,
        },
        Case {
            name: "backend-terminated",
            healthy_threshold: 10,
            unhealthy_threshold: 3,
            backend: Backend::Terminated,
        },
        Case {
            name: "backend-absent",
            healthy_threshold: 2,
            unhealthy_threshold: 10,
            backend: Backend::Absent,
        },
    ]
}

/// Create one load balancer named `name` with an HTTP:80 listener.
async fn create_load_balancer(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        REGION,
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", name),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "80"),
            param("AvailabilityZones.member.1", &format!("{REGION}a")),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateLoadBalancer must answer 200, got {}",
        created.body
    );
}

/// Configure the health check of `case`'s load balancer: an HTTP target on the listener's
/// instance port, with the row's own thresholds.
async fn configure_health_check(router: &MultiServiceRouter, case: &Case) {
    let configured = call(
        router,
        REGION,
        "ConfigureHealthCheck",
        &[
            param("LoadBalancerName", case.name),
            param("HealthCheck.Target", "HTTP:80/ping"),
            param("HealthCheck.Interval", "5"),
            param("HealthCheck.Timeout", "2"),
            param(
                "HealthCheck.UnhealthyThreshold",
                &case.unhealthy_threshold.to_string(),
            ),
            param(
                "HealthCheck.HealthyThreshold",
                &case.healthy_threshold.to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(
        configured.status, 200,
        "case {:?}: ConfigureHealthCheck must answer 200, got {}",
        case.name, configured.body
    );
}

/// Register `instance_id` with `name`'s load balancer.
async fn register(router: &MultiServiceRouter, name: &str, instance_id: &str) {
    let registered = call(
        router,
        REGION,
        "RegisterInstancesWithLoadBalancer",
        &[
            param("LoadBalancerName", name),
            param("Instances.member.1.InstanceId", instance_id),
        ],
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "case {name:?}: RegisterInstancesWithLoadBalancer must answer 200, got {}",
        registered.body
    );
}

/// The triple `name`'s load balancer reports for `instance_id` now.
async fn health(router: &MultiServiceRouter, name: &str, instance_id: &str) -> Triple {
    let answer = call(
        router,
        REGION,
        "DescribeInstanceHealth",
        &[
            param("LoadBalancerName", name),
            param("Instances.member.1.InstanceId", instance_id),
        ],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: DescribeInstanceHealth must answer 200, got {}",
        answer.body
    );
    let doc = parse(&answer.body);
    let reported = members(answer.result(&doc), "InstanceStates");
    assert_eq!(
        reported.len(),
        1,
        "case {name:?}: one InstanceState per described instance, got {}",
        answer.body
    );
    let member = reported[0];
    assert_eq!(
        kid_text(member, "InstanceId"),
        Some(instance_id.to_string()),
        "case {name:?}: the member must be about the instance the request named, got {}",
        answer.body
    );
    (
        kid_text(member, "State"),
        kid_text(member, "ReasonCode"),
        kid_text(member, "Description"),
    )
}

/// The reported state of a registered instance is the outcome of the checks against its
/// backend, decided by the load balancer's own thresholds.
#[tokio::test]
async fn instance_health_is_the_outcome_of_checking_the_registered_backend() {
    let (router, engine) = emulator_with_ec2();

    for case in health_cases() {
        let instance_id = match case.backend {
            // An id of the right form that EC2 holds no instance for: nothing was
            // launched for this row.
            Backend::Absent => "i-00000000000000bad".to_string(),
            _ => {
                let launched = launch_instance(&router, REGION, case.name).await;
                settle_ec2_transition().await;
                match case.backend {
                    Backend::Stopped => {
                        move_instance(&router, case.name, "StopInstances", &launched).await;
                    }
                    Backend::Terminated => {
                        move_instance(&router, case.name, "TerminateInstances", &launched).await;
                    }
                    _ => {}
                }
                launched
            }
        };

        create_load_balancer(&router, case.name).await;
        configure_health_check(&router, &case).await;
        register(&router, case.name, &instance_id).await;

        // No check has fallen due yet, whatever the backend is doing: the registration's
        // own checks are still running.
        let fresh = health(&router, case.name, &instance_id).await;
        assert_eq!(
            fresh,
            triple(HEALTH_REGISTRATION_IN_PROGRESS),
            "case {:?}: right after registration, got {fresh:?}",
            case.name
        );

        // One check short of the threshold that decides this row: no verdict yet.
        let deciding = case.deciding_threshold();
        age_registration(
            &engine,
            REGION,
            case.name,
            &instance_id,
            HEALTH_CHECK_SPAN.saturating_mul(deciding.saturating_sub(1)),
        );
        let short = health(&router, case.name, &instance_id).await;
        assert_eq!(
            short,
            triple(HEALTH_REGISTRATION_IN_PROGRESS),
            "case {:?}: after {} of {deciding} deciding checks, got {short:?}",
            case.name,
            deciding.saturating_sub(1)
        );

        // The check that reaches the threshold: the verdict follows what the checks
        // observed about the backend.
        age_registration(&engine, REGION, case.name, &instance_id, HEALTH_CHECK_SPAN);
        let settled = health(&router, case.name, &instance_id).await;
        assert_eq!(
            settled,
            case.expected(),
            "case {:?}: after {deciding} checks of a {:?} backend, got {settled:?}",
            case.name,
            case.backend
        );

        // The verdict is read, not re-decided: polling again answers the same thing, and
        // the registered set the answer was read from is untouched.
        let polled = health(&router, case.name, &instance_id).await;
        assert_eq!(
            polled, settled,
            "case {:?}: a second poll must answer what the first one did",
            case.name
        );
        let described = call(
            &router,
            REGION,
            "DescribeLoadBalancers",
            &[param("LoadBalancerNames.member.1", case.name)],
        )
        .await;
        let doc = parse(&described.body);
        let description = members(described.result(&doc), "LoadBalancerDescriptions")[0];
        assert_eq!(
            members(description, "Instances")
                .into_iter()
                .filter_map(|member| kid_text(member, "InstanceId"))
                .collect::<Vec<String>>(),
            vec![instance_id.clone()],
            "case {:?}: polling health must not change the registered set, got {}",
            case.name,
            described.body
        );
    }
}

/// The verdict FOLLOWS the backend: an instance in service whose backend stops keeps the
/// verdict its answered checks earned until `UnhealthyThreshold` consecutive checks have
/// failed, and then reports the condition they ran into.
#[tokio::test]
async fn a_verdict_moves_only_once_the_threshold_of_checks_disagrees_with_it() {
    let (router, engine) = emulator_with_ec2();
    let name = "backend-stops-serving";
    let case = Case {
        name,
        healthy_threshold: 2,
        unhealthy_threshold: 3,
        backend: Backend::Running,
    };

    let instance_id = launch_instance(&router, REGION, name).await;
    settle_ec2_transition().await;
    create_load_balancer(&router, name).await;
    configure_health_check(&router, &case).await;
    register(&router, name, &instance_id).await;

    // The answered checks put it in service.
    age_registration(
        &engine,
        REGION,
        name,
        &instance_id,
        HEALTH_CHECK_SPAN.saturating_mul(2),
    );
    let in_service = health(&router, name, &instance_id).await;
    assert_eq!(
        in_service,
        triple(HEALTH_IN_SERVICE),
        "a running backend must reach InService, got {in_service:?}"
    );

    // The backend stops. Each check from now on fails, but the verdict holds until the
    // configured run of them has.
    move_instance(&router, name, "StopInstances", &instance_id).await;
    for failed in 1..case.unhealthy_threshold {
        age_registration(&engine, REGION, name, &instance_id, HEALTH_CHECK_SPAN);
        let holding = health(&router, name, &instance_id).await;
        assert_eq!(
            holding,
            triple(HEALTH_IN_SERVICE),
            "after {failed} of {} failing checks the verdict must hold, got {holding:?}",
            case.unhealthy_threshold
        );
    }
    age_registration(&engine, REGION, name, &instance_id, HEALTH_CHECK_SPAN);
    let moved = health(&router, name, &instance_id).await;
    assert_eq!(
        moved,
        out_of_service(DESCRIPTION_INSTANCE_STOPPED),
        "after {} failing checks the verdict must follow them, got {moved:?}",
        case.unhealthy_threshold
    );

    // And back: the stopped backend is started again, and the healthy threshold's worth of
    // answered checks returns it to service.
    move_instance(&router, name, "StartInstances", &instance_id).await;
    age_registration(
        &engine,
        REGION,
        name,
        &instance_id,
        HEALTH_CHECK_SPAN.saturating_mul(u32::try_from(case.healthy_threshold).expect("a threshold")),
    );
    let recovered = health(&router, name, &instance_id).await;
    assert_eq!(
        recovered,
        triple(HEALTH_IN_SERVICE),
        "a backend that answers again must return to InService, got {recovered:?}"
    );
}
