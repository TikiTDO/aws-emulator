// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstanceHealth` over the health timeline of a registered instance, end to end
//! over the awsQuery wire.
//!
//! The rule under test: the operation answers ONE `InstanceState` member per described
//! instance — every instance registered with the named load balancer, or every instance
//! the request's `Instances` filter names — and each member's `State`, `ReasonCode` and
//! `Description` are projected together from the verdict that instance's health checks
//! have reached with this load balancer:
//!
//! * an instance the load balancer does not hold is `OutOfService` / `ELB` /
//!   "Instance is not currently registered with the LoadBalancer.";
//! * a registered instance whose checks have not reached a verdict yet is
//!   `OutOfService` / `ELB` / "Instance registration is still in progress.";
//! * once `UnhealthyThreshold` consecutive checks are due — the instances these rows
//!   register are ids the account's EC2 inventory holds no instance for, so nothing stands
//!   behind the registration and each check fails — it is `OutOfService` / `Instance` /
//!   "Instance has failed at least the UnhealthyThreshold number of health checks
//!   consecutively."
//!
//! What ONE check observes, and the verdict a run of answered checks reaches, is the
//! sibling suite `instance_health_follows_the_backend`.
//!
//! Asserted over a TABLE whose rows differ in the configured `UnhealthyThreshold`
//! (including both modelled bounds), in how many instances are registered, and in whether
//! the request filters by `Instances` — and at each point of the timeline, which is placed
//! by moving the stored registration back rather than by sleeping.

use super::wire::{call, emulator_with_engine, kid_text, members, param, parse, Wire};
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::{
    ComAmazonawsElasticloadbalancingElasticloadbalancingV7State as ElbState, HEALTH_CHECK_SPAN,
};
use std::time::{Duration, SystemTime};

/// The health triple of one `InstanceState` member: `(State, ReasonCode, Description)`.
type Triple = (Option<String>, Option<String>, Option<String>);

fn triple(state: &str, reason_code: &str, description: &str) -> Triple {
    (
        Some(state.to_string()),
        Some(reason_code.to_string()),
        Some(description.to_string()),
    )
}

/// The three documented triples this suite expects, each the one
/// `InstanceState$State`/`$ReasonCode`/`$Description` enumerate for that situation.
fn not_registered() -> Triple {
    triple(
        "OutOfService",
        "ELB",
        "Instance is not currently registered with the LoadBalancer.",
    )
}

fn registration_in_progress() -> Triple {
    triple(
        "OutOfService",
        "ELB",
        "Instance registration is still in progress.",
    )
}

fn failed_health_checks() -> Triple {
    triple(
        "OutOfService",
        "Instance",
        "Instance has failed at least the UnhealthyThreshold number of health checks \
         consecutively.",
    )
}

/// The `(InstanceId, (State, ReasonCode, Description))` of every `InstanceStates` member
/// of one answer, which is also what proves the member is a LIST element rather than the
/// element's own character data.
fn reported(answer: &Wire) -> Vec<(String, Triple)> {
    let doc = parse(&answer.body);
    members(answer.result(&doc), "InstanceStates")
        .into_iter()
        .map(|member| {
            (
                kid_text(member, "InstanceId").unwrap_or_default(),
                (
                    kid_text(member, "State"),
                    kid_text(member, "ReasonCode"),
                    kid_text(member, "Description"),
                ),
            )
        })
        .collect()
}

/// One row of the timeline table.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    region: &'static str,
    /// `None` leaves the health check `CreateLoadBalancer` seeds (whose documented
    /// `UnhealthyThreshold` default is 2); `Some(n)` configures that threshold.
    /// `HealthCheck$UnhealthyThreshold` is modelled `@range(min: 2, max: 10)`, so the
    /// rows cover both published bounds and a value between them.
    configured_threshold: Option<i32>,
    /// Registered in one call; a repeated id proves the stored set is deduplicated.
    register: Vec<&'static str>,
    /// The ids the describe's `Instances` filter names — empty means "no filter", which
    /// describes the whole registered set.
    filter: Vec<&'static str>,
}

impl Case {
    /// The `UnhealthyThreshold` the load balancer ends up with: the configured one, else
    /// the default `CreateLoadBalancer` seeds.
    fn threshold(&self) -> i32 {
        self.configured_threshold.unwrap_or(DEFAULT_UNHEALTHY_THRESHOLD)
    }

    /// The instance ids the describe reports on, in the order it must report them: the
    /// filter when the request carries one, else the deduplicated registered set.
    fn described(&self) -> Vec<String> {
        let source = if self.filter.is_empty() {
            &self.register
        } else {
            &self.filter
        };
        let mut unique: Vec<String> = Vec::new();
        for id in source {
            if !unique.iter().any(|held| held == id) {
                unique.push((*id).to_string());
            }
        }
        unique
    }

    fn registered(&self) -> Vec<String> {
        let mut unique: Vec<String> = Vec::new();
        for id in &self.register {
            if !unique.iter().any(|held| held == id) {
                unique.push((*id).to_string());
            }
        }
        unique
    }

    /// How long after registration this load balancer's owed checks have all fallen due.
    fn failing_after(&self) -> Duration {
        HEALTH_CHECK_SPAN.saturating_mul(u32::try_from(self.threshold()).expect("a threshold"))
    }
}

/// The `UnhealthyThreshold` of the health check a newly created load balancer carries —
/// the Classic Load Balancer default, which `DescribeLoadBalancers` reports as 2.
const DEFAULT_UNHEALTHY_THRESHOLD: i32 = 2;

fn timeline_cases() -> Vec<Case> {
    vec![
        Case {
            name: "health-default-threshold",
            region: "us-east-1",
            configured_threshold: None,
            register: vec!["i-0123456789abcdef0"],
            filter: Vec::new(),
        },
        Case {
            name: "health-threshold-at-minimum",
            region: "eu-west-1",
            configured_threshold: Some(2),
            register: vec!["i-00000000000000001", "i-00000000000000002"],
            filter: Vec::new(),
        },
        Case {
            name: "health-threshold-at-maximum",
            region: "ap-southeast-2",
            configured_threshold: Some(10),
            register: vec!["i-aaaaaaaaaaaaaaaa1", "i-aaaaaaaaaaaaaaaa2", "i-aaaaaaaaaaaaaaaa1"],
            // A filter naming a registered instance and one this load balancer never
            // held: both are reported, each with its own triple.
            filter: vec!["i-aaaaaaaaaaaaaaaa2", "i-ffffffffffffffff0"],
        },
        Case {
            name: "health-threshold-between-bounds",
            region: "us-west-2",
            configured_threshold: Some(5),
            register: vec!["i-0dead0beef0dead01"],
            filter: vec!["i-0dead0beef0dead01"],
        },
    ]
}

/// Move the registration instant of every instance of one case's load balancer `back`
/// into the past, so the health checks the clock owes are due without the test waiting for
/// them — the shared seam (`super::wire`), read against this case's own load balancer.
fn age_registrations(engine: &engine::InMemoryEngine, case: &Case, back: Duration) {
    super::wire::age_registrations(engine, case.region, case.name, back);
}

/// What this case's describe answers now.
async fn describe(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &Case,
) -> Vec<(String, Triple)> {
    let mut params = vec![param("LoadBalancerName", case.name)];
    for (index, instance) in case.filter.iter().enumerate() {
        params.push(param(
            &format!("Instances.member.{}.InstanceId", index + 1),
            instance,
        ));
    }
    let answer = call(router, case.region, "DescribeInstanceHealth", &params).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: DescribeInstanceHealth must answer 200, got {}",
        case.name, answer.body
    );
    reported(&answer)
}

#[tokio::test]
async fn instance_health_follows_the_registration_and_the_configured_threshold() {
    let (router, engine) = emulator_with_engine();

    for case in timeline_cases() {
        let created = call(
            &router,
            case.region,
            "CreateLoadBalancer",
            &[
                param("LoadBalancerName", case.name),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "8080"),
                param("AvailabilityZones.member.1", &format!("{}a", case.region)),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateLoadBalancer must answer 200, got {}",
            case.name, created.body
        );

        if let Some(threshold) = case.configured_threshold {
            let configured = call(
                &router,
                case.region,
                "ConfigureHealthCheck",
                &[
                    param("LoadBalancerName", case.name),
                    param("HealthCheck.Target", "HTTP:8080/health"),
                    param("HealthCheck.Interval", "30"),
                    param("HealthCheck.Timeout", "5"),
                    param("HealthCheck.UnhealthyThreshold", &threshold.to_string()),
                    param("HealthCheck.HealthyThreshold", "2"),
                ],
            )
            .await;
            assert_eq!(
                configured.status, 200,
                "case {:?}: ConfigureHealthCheck must answer 200, got {}",
                case.name, configured.body
            );
        }

        // Before any instance is registered the list is genuinely empty — a filter-less
        // describe of a load balancer holding nothing reports nothing.
        if case.filter.is_empty() {
            assert!(
                describe(&router, &case).await.is_empty(),
                "case {:?}: a load balancer with no registration reports no instance state",
                case.name
            );
        }

        let mut params = vec![param("LoadBalancerName", case.name)];
        for (index, instance) in case.register.iter().enumerate() {
            params.push(param(
                &format!("Instances.member.{}.InstanceId", index + 1),
                instance,
            ));
        }
        let registered = call(
            &router,
            case.region,
            "RegisterInstancesWithLoadBalancer",
            &params,
        )
        .await;
        assert_eq!(
            registered.status, 200,
            "case {:?}: RegisterInstancesWithLoadBalancer must answer 200, got {}",
            case.name, registered.body
        );

        // ONE member per described instance, in the order the request or the registration
        // set puts them in — a list, never one empty element.
        let fresh = describe(&router, &case).await;
        assert_eq!(
            fresh.iter().map(|(id, _)| id.clone()).collect::<Vec<String>>(),
            case.described(),
            "case {:?}: one InstanceState per described instance, got {fresh:?}",
            case.name
        );
        for (instance_id, health) in &fresh {
            let expected = if case.registered().contains(instance_id) {
                registration_in_progress()
            } else {
                not_registered()
            };
            assert_eq!(
                health, &expected,
                "case {:?}: {instance_id} right after registration, got {health:?}",
                case.name
            );
        }

        // One second short of the owed checks: still the registration, not the instance.
        age_registrations(
            &engine,
            &case,
            case.failing_after().saturating_sub(Duration::from_secs(1)),
        );
        for (instance_id, health) in describe(&router, &case).await {
            let expected = if case.registered().contains(&instance_id) {
                registration_in_progress()
            } else {
                not_registered()
            };
            assert_eq!(
                health, expected,
                "case {:?}: {instance_id} just before its {} owed checks fall due, got {health:?}",
                case.name,
                case.threshold()
            );
        }

        // Once every owed check is due, a registered instance has failed them all.
        age_registrations(&engine, &case, Duration::from_secs(1));
        let failing = describe(&router, &case).await;
        for (instance_id, health) in &failing {
            let expected = if case.registered().contains(instance_id) {
                failed_health_checks()
            } else {
                not_registered()
            };
            assert_eq!(
                health, &expected,
                "case {:?}: {instance_id} after {} owed checks failed, got {health:?}",
                case.name,
                case.threshold()
            );
        }

        // The describe is a READ: polling it again answers the same thing, and the
        // registered set it was read from is untouched.
        assert_eq!(
            describe(&router, &case).await,
            failing,
            "case {:?}: a second poll must answer what the first one did",
            case.name
        );
        let described_set = call(
            &router,
            case.region,
            "DescribeLoadBalancers",
            &[param("LoadBalancerNames.member.1", case.name)],
        )
        .await;
        let described_doc = parse(&described_set.body);
        let description = members(
            described_set.result(&described_doc),
            "LoadBalancerDescriptions",
        )[0];
        assert_eq!(
            members(description, "Instances")
                .into_iter()
                .filter_map(|member| kid_text(member, "InstanceId"))
                .collect::<Vec<String>>(),
            case.registered(),
            "case {:?}: polling health must not change the registered set, got {}",
            case.name,
            described_set.body
        );

        // Deregistering takes the health with the registration: the same id is now
        // reported as not registered rather than as a failing instance.
        let first = case.registered()[0].clone();
        let deregistered = call(
            &router,
            case.region,
            "DeregisterInstancesFromLoadBalancer",
            &[
                param("LoadBalancerName", case.name),
                param("Instances.member.1.InstanceId", &first),
            ],
        )
        .await;
        assert_eq!(
            deregistered.status, 200,
            "case {:?}: DeregisterInstancesFromLoadBalancer must answer 200, got {}",
            case.name, deregistered.body
        );
        let after = call(
            &router,
            case.region,
            "DescribeInstanceHealth",
            &[
                param("LoadBalancerName", case.name),
                param("Instances.member.1.InstanceId", &first),
            ],
        )
        .await;
        assert_eq!(
            reported(&after),
            vec![(first.clone(), not_registered())],
            "case {:?}: a deregistered instance reports no registration, got {}",
            case.name,
            after.body
        );

        // Re-registering it starts a fresh timeline: it is in registration progress
        // again, however long the previous registration had been failing.
        let again = call(
            &router,
            case.region,
            "RegisterInstancesWithLoadBalancer",
            &[
                param("LoadBalancerName", case.name),
                param("Instances.member.1.InstanceId", &first),
            ],
        )
        .await;
        assert_eq!(
            again.status, 200,
            "case {:?}: re-registering must answer 200, got {}",
            case.name, again.body
        );
        let reregistered = call(
            &router,
            case.region,
            "DescribeInstanceHealth",
            &[
                param("LoadBalancerName", case.name),
                param("Instances.member.1.InstanceId", &first),
            ],
        )
        .await;
        assert_eq!(
            reported(&reregistered),
            vec![(first.clone(), registration_in_progress())],
            "case {:?}: a re-registered instance starts its checks again, got {}",
            case.name,
            reregistered.body
        );
    }
}

/// An unknown load balancer is the modelled refusal, not an empty list — and the health
/// of an instance is only ever asked of a load balancer that exists.
#[tokio::test]
async fn describing_the_health_of_an_unknown_load_balancer_is_refused() {
    let (router, _engine) = emulator_with_engine();

    for (name, instances) in [
        ("no-such-load-balancer", Vec::new()),
        (
            "another-missing-one",
            vec!["i-0123456789abcdef0".to_string()],
        ),
    ] {
        let mut params = vec![param("LoadBalancerName", name)];
        for (index, instance) in instances.iter().enumerate() {
            params.push(param(
                &format!("Instances.member.{}.InstanceId", index + 1),
                instance,
            ));
        }
        let refused = call(&router, "us-east-1", "DescribeInstanceHealth", &params).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {name:?}: an unknown load balancer must be refused, got {}",
            refused.body
        );
    }
}

/// The registration instant is per instance, so two instances registered at different
/// times cross the threshold at different times.
#[tokio::test]
async fn each_instance_crosses_the_threshold_on_its_own_registration() {
    let (router, engine) = emulator_with_engine();
    let region = "us-east-2";
    let name = "health-per-instance-timeline";
    let early = "i-00000000000000011";
    let late = "i-00000000000000012";

    let created = call(
        &router,
        region,
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", name),
            param("Listeners.member.1.Protocol", "TCP"),
            param("Listeners.member.1.LoadBalancerPort", "9000"),
            param("Listeners.member.1.InstancePort", "9000"),
            param("AvailabilityZones.member.1", "us-east-2a"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateLoadBalancer must answer 200, got {}",
        created.body
    );

    for instance in [early, late] {
        let registered = call(
            &router,
            region,
            "RegisterInstancesWithLoadBalancer",
            &[
                param("LoadBalancerName", name),
                param("Instances.member.1.InstanceId", instance),
            ],
        )
        .await;
        assert_eq!(
            registered.status, 200,
            "registering {instance} must answer 200, got {}",
            registered.body
        );
    }

    // The default threshold is 2 owed checks: age ONLY the early instance past them.
    let span = HEALTH_CHECK_SPAN.saturating_mul(2);
    {
        let state = engine.state::<ElbState>();
        let mut guard = state.lock();
        let record = guard
            .load_balancers_in_mut(region)
            .and_then(|records| records.get_mut(name))
            .expect("the stored load balancer");
        let at = record
            .instances
            .registered_at(early)
            .expect("the early registration");
        assert!(
            record
                .instances
                .restamp(early, at.checked_sub(span).unwrap_or(SystemTime::UNIX_EPOCH)),
            "{early} must still be registered"
        );
    }

    let answer = call(
        &router,
        region,
        "DescribeInstanceHealth",
        &[param("LoadBalancerName", name)],
    )
    .await;
    assert_eq!(
        reported(&answer),
        vec![
            (early.to_string(), failed_health_checks()),
            (late.to_string(), registration_in_progress()),
        ],
        "each instance must report the health its OWN registration earns, got {}",
        answer.body
    );
}
