// SPDX-License-Identifier: CC0-1.0

//! The Elastic Load Balancing v2 TARGETS Amazon ECS registers for the tasks of a service
//! whose `loadBalancers` bind a target group — the registration that makes a load-balanced
//! service serve traffic, and the drain that takes a stopped task out of rotation.
//!
//! `LoadBalancer$targetGroupArn` is "the full Amazon Resource Name (ARN) of the Elastic Load
//! Balancing target group ... associated with a service or task set", and
//! `LoadBalancer$targetGroupArn`'s own note says what a task is registered AS: "If your
//! service's task definition uses the `awsvpc` network mode, you must choose `ip` as the
//! target type ... because tasks that use the `awsvpc` network mode are associated with an
//! elastic network interface, not an Amazon EC2 instance." So a RUNNING task of such a
//! service is a target of that group at the private IPv4 address of the interface it was
//! allocated, on the `containerPort` the binding names, and a task that stops leaves the
//! group — through `draining`, for the group's deregistration delay, exactly as
//! `DeregisterTargets` takes a target out (`ELBv2 state::target_health`).
//!
//! A registration also carries the ports the task's containers LISTEN on
//! ([`revision_container_ports`], bounded by [`MAX_SERVED_PORT_RANGES`]), because that is
//! what the load balancer's health check of it can get an answer on: the check connects to
//! the target group's effective health-check port, and a port no container of the task
//! listens on is one the task answers nothing on.
//!
//! # Reconciled, never appended
//!
//! The registrations Amazon ECS owns in a target group are exactly the ones its live tasks
//! need, so the set is RECOMPUTED from this store ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::load_balancer_target_registrations`])
//! and the target groups are brought in line with it ([`reconcile_target_group_registrations`]):
//! a task that started is registered, a task that is stopping or gone is deregistered, and a
//! registration a `RegisterTargets` CALLER made is never touched (it carries no
//! `RegisteredTarget::registered_for`, which is what tells the two apart).
//!
//! An `instance` target group receives nothing here: an instance target is the Amazon EC2
//! instance the task runs on, and the scheduler in this emulator places a service's task
//! without choosing one of the cluster's container instances
//! (`TaskRecord::container_instance_arn` is `None` for such a task), so there is no instance
//! to register and none is invented.
//!
//! LOCK ORDER: [`reconcile_target_group_registrations`] takes the Amazon EC2 store's lock
//! and then the Elastic Load Balancing v2 store's, and holds no guard of THIS service's
//! store while it does — the one-directional, guard-free discipline every other
//! cross-store access here follows, which is what keeps a cycle between the stores
//! unreachable.

use ::std::ops::RangeInclusive;

use ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ServedPorts;

use super::*;

/// How many declared container-port ranges the registration of ONE task carries.
///
/// A bound on the work one registration costs, not an Amazon ECS quota:
/// `PortMapping$containerPortRange` states "you can specify a maximum of 100 port ranges per
/// container", and a revision describes at most [`MAX_CONTAINER_DEFINITIONS`] containers, so
/// that product is the most a revision can legitimately declare. `portMappings` carries no
/// length of its own, so a revision past this bound has the first this many of its ranges
/// read and no more: the stored list is held to a figure this service fixes rather than to
/// one a request chose.
pub const MAX_SERVED_PORT_RANGES: usize = 100 * MAX_CONTAINER_DEFINITIONS;

/// One target an ECS service's task occupies in one ELBv2 target group.
///
/// Every field is read off the stored task and the stored binding, so a registration stands
/// for a task a `DescribeTasks` of the same instant reports and never a second, invented
/// copy of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRegistration {
    /// The target group the binding named (`LoadBalancer$targetGroupArn`).
    pub target_group_arn: String,
    /// The address the load balancer reaches the task at: the private IPv4 address of the
    /// elastic network interface the task was allocated.
    pub target_id: String,
    /// The port the target receives traffic on — `LoadBalancer$containerPort`, "the port on
    /// the container to associate with the load balancer" — and `None` for a binding that
    /// names none, which takes the target group's own port.
    pub port: Option<i32>,
    /// The subnet that interface was placed in, which is what the target's Availability
    /// Zone is read from ("if the target type is `ip` and the IP address is in a subnet of
    /// the VPC for the target group, the Availability Zone is automatically detected").
    pub subnet_id: Option<String>,
    /// The task this registration stands for, which is the workload the registration is
    /// marked as owned by — so a later reconcile can take out exactly the registrations of
    /// the tasks that are gone.
    pub task_arn: String,
    /// The ports the task's containers LISTEN on, as its revision declares them
    /// ([`revision_container_ports`]).
    ///
    /// Carried into the registration because it is what a health check of that target can
    /// get an answer on: the load balancer connects to the target group's effective
    /// health-check port, and a port no container of the task listens on is a port the task
    /// answers nothing on — which is what a target group whose `HealthCheckPort` names
    /// another port finds.
    ///
    /// SHARED: one task's declared ports are read once and the registrations of that task
    /// hold the same list, so a task bound to several target groups costs one allocation.
    /// The type is the Elastic Load Balancing v2 service's own alias for the shape, which
    /// is where this value is stored ([`ServedPorts`]).
    pub served_ports: ServedPorts,
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Every ELBv2 target registration this store's services need, as of the state it holds
    /// now.
    ///
    /// One per (RUNNING task, `loadBalancers` binding) pair of every ACTIVE service that
    /// binds a target group: Amazon ECS registers a task once it is running and takes it out
    /// again when it stops, so a task that is still provisioning or starting — and one that
    /// is stopping or stopped — is not a target traffic is sent to.
    ///
    /// Only a task that HOLDS an elastic network interface is registered, because the
    /// address a target of an `awsvpc` service is reached at is that interface's: a task
    /// whose network mode gives it none is reached at its container instance's address,
    /// which this store does not hold (see this module's header).
    ///
    /// The bindings are the PLACEMENT's: a task a task set placed registers into that task
    /// set's own `loadBalancers`, which is the configuration it was placed under, rather
    /// than into the service's.
    ///
    /// At most one entry per (target group, address, port): the list is bounded by the tasks
    /// this store holds ([`MAX_TASKS`]) times the bindings one service may carry
    /// ([`MAX_LOAD_BALANCERS`]), and it is ordered so two reconciles of the same state do
    /// the same work in the same order.
    pub fn load_balancer_target_registrations(&self) -> Vec<TargetRegistration> {
        let mut registrations = Vec::new();
        let mut seen = HashSet::new();
        for service in self.services.values() {
            if service.status != SERVICE_ACTIVE {
                continue;
            }
            for task in self.tasks.values() {
                if task.cluster_name != service.cluster_name
                    || task.service_name.as_deref() != Some(service.name.as_str())
                    || !task_receives_traffic(task)
                {
                    continue;
                }
                let Some(address) = task
                    .attachments
                    .iter()
                    .find_map(AttachmentRecord::private_ipv4_address)
                else {
                    continue;
                };
                let subnet_id = task
                    .attachments
                    .iter()
                    .find_map(AttachmentRecord::subnet_id)
                    .map(str::to_string);
                // The ports this task's containers listen on, read off the revision it runs
                // once for the task rather than per binding.
                let served_ports = self.task_container_ports(task);
                for binding in self.placement_load_balancers(service, task) {
                    let Some(arn) = binding.target_group_arn.as_deref() else {
                        continue;
                    };
                    if !seen.insert((
                        arn.to_string(),
                        address.to_string(),
                        binding.container_port,
                    )) {
                        continue;
                    }
                    registrations.push(TargetRegistration {
                        target_group_arn: arn.to_string(),
                        target_id: address.to_string(),
                        port: binding.container_port,
                        subnet_id: subnet_id.clone(),
                        task_arn: task.task_arn.clone(),
                        served_ports: served_ports.clone(),
                    });
                }
            }
        }
        registrations.sort_by(|left, right| {
            (&left.target_group_arn, &left.target_id, left.port).cmp(&(
                &right.target_group_arn,
                &right.target_id,
                right.port,
            ))
        });
        registrations
    }

    /// The ports the containers of `task` listen on, read off the REVISION the task runs
    /// ([`revision_container_ports`]); empty for a task whose revision this store no longer
    /// holds, which declares no ports for a check to be read against.
    fn task_container_ports(&self, task: &TaskRecord) -> ServedPorts {
        parse_revision_reference(&task.task_definition_arn, TASK_DEFINITION_RESOURCE_TYPE)
            .and_then(|(family, revision)| Some((family, revision?)))
            .and_then(|key| self.task_definitions.get(&key))
            .map_or_else(|| ServedPorts::from(Vec::new()), revision_container_ports)
    }

    /// The `loadBalancers` bindings the placement that placed `task` registers it into: the
    /// TASK SET's own when a task set placed it (`Task$startedBy` carries that set's id),
    /// the service's otherwise.
    fn placement_load_balancers<'a>(
        &'a self,
        service: &'a ServiceRecord,
        task: &TaskRecord,
    ) -> &'a [LoadBalancer] {
        task.started_by
            .as_deref()
            .and_then(|started_by| {
                self.task_sets.get(&(
                    service.cluster_name.clone(),
                    service.name.clone(),
                    started_by.to_string(),
                ))
            })
            .map_or(service.load_balancers.as_slice(), |task_set| {
                task_set.load_balancers.as_slice()
            })
    }
}

/// Whether a task is one the load balancer sends requests to: Amazon ECS registers a task
/// as a target once it is `RUNNING`, and a task that has been asked to stop is draining
/// rather than serving, so neither it nor a task that is still coming up is a target.
fn task_receives_traffic(task: &TaskRecord) -> bool {
    task.desired_status != STATUS_STOPPED && task.last_status == STATUS_RUNNING
}

/// The ports the containers of one revision LISTEN on: every `containerPort` its
/// `containerDefinitions` declare in their `portMappings`, and every port of a
/// `containerPortRange` declared instead of one.
///
/// The ports a target of such a task answers on. A task in the `awsvpc` network mode is
/// reached at its own interface's address on those very ports — "for containers in a task
/// with the `awsvpc` network mode, the `hostPortRange` is set to the same value as the
/// `containerPortRange`" (`PortMapping$containerPortRange`), and `PortMapping$hostPort`
/// says the same of a single port — so a port the revision declares no container listening
/// on is a port nothing in the task answers on. A revision that declares no port mapping at
/// all declares nothing, which is no claim about any port.
///
/// BOUNDED by [`MAX_SERVED_PORT_RANGES`]: a revision over it keeps the first that many
/// ranges it declares and the rest are not read, so the stored list is never sized by the
/// length of a list a caller chose to send — and a check against a port outside the ones
/// kept still FAILS, rather than the whole registration falling back to claiming nothing.
fn revision_container_ports(definition: &TaskDefinitionRecord) -> ServedPorts {
    let ports: Vec<RangeInclusive<i32>> = definition
        .container_definitions
        .iter()
        .flat_map(|container| container.port_mappings.iter().flatten())
        .flat_map(|mapping| {
            mapping
                .container_port
                .map(|port| port..=port)
                .into_iter()
                .chain(container_port_range(mapping))
        })
        .take(MAX_SERVED_PORT_RANGES)
        .collect();
    ServedPorts::from(ports)
}

/// Bring the Elastic Load Balancing v2 target groups in line with the tasks this store now
/// holds: the ONE call a door — or the clock pump — makes for it.
///
/// Called with NO guard of this store held: it takes this store's lock to read the set its
/// services need ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::load_balancer_target_registrations`]),
/// releases it, and only then writes the other services' stores
/// ([`reconcile_target_group_registrations`]) — the lock order this module's header states.
///
/// Reconciling is idempotent, so a call that moved no task writes nothing: the set it reads
/// is the set already in place.
pub fn settle_load_balancer_targets<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The guard this statement takes is a TEMPORARY: it is released at the end of the
    // statement, so nothing below holds it.
    let desired = state.lock().load_balancer_target_registrations();
    reconcile_target_group_registrations(engine, &desired, SystemTime::now());
}

/// Make the ELBv2 registrations Amazon ECS owns exactly `desired`, in the Elastic Load
/// Balancing v2 service's OWN store through the one shared engine — so a target this
/// registers is one that service returns from its own read (`DescribeTargetHealth`) and
/// counts against its own target quota.
///
/// A registration ECS owns that `desired` no longer holds is DEREGISTERED the way
/// `DeregisterTargets` deregisters one: it enters `draining` and leaves the group once the
/// group's `deregistration_delay.timeout_seconds` has passed, which is the contract a client
/// that drains a task before tearing it down waits on. One a `RegisterTargets` caller made
/// is left exactly as it is.
///
/// Nothing happens when the Elastic Load Balancing v2 store is unreachable (the engine is
/// not the shared in-memory one): there is then no target group to register into and none
/// Amazon ECS could have registered into, so there is nothing to add and nothing to clean
/// up — which is why this answers nothing rather than failing the request that called it.
pub fn reconcile_target_group_registrations<Eng: Send + Sync + 'static>(
    engine: &Eng,
    desired: &[TargetRegistration],
    now: SystemTime,
) {
    // The zone each address sits in, read out of Amazon EC2's own store and RELEASED before
    // the Elastic Load Balancing v2 store is locked (see this module's header).
    let zones = target_availability_zones(engine, desired);
    let Some(state) =
        ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::service_state(
            engine,
        )
    else {
        return;
    };
    let mut store = state.lock();
    // The registrations this reconcile ADDED, each as the key the target group holds it
    // under: the write that creates a registration owes it its first health check, and
    // nothing else is checked for it.
    let mut added: Vec<(
        String,
        ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::TargetKey,
    )> = Vec::new();
    for (arn, group) in store.target_groups.iter_mut() {
        // What the clock already owes this group: a registration whose drain is over has
        // left it, so it occupies neither a key nor the group's quota.
        group.settle_draining(now);
        // The registrations wanted in THIS group, each under the key the group would store
        // it at — a binding that names no port takes the group's own
        // (`RegisterTargets$Targets`: "you can override the port for a target when you
        // register it").
        let wanted: Vec<(::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::TargetKey, &TargetRegistration)> =
            desired
                .iter()
                .filter(|registration| &registration.target_group_arn == arn)
                .map(|registration| {
                    (
                        (
                            registration.target_id.clone(),
                            registration.port.or(group.port),
                        ),
                        registration,
                    )
                })
                .collect();
        // Every registration THIS service's scheduler owns that the set above no longer
        // holds: a registration another emulated service's scheduler owns is none of this
        // reconcile's business, which is read off the workload each one stands for
        // ([`names_a_task_of_this_service`]), and a `RegisterTargets` caller's own carries no
        // workload at all.
        for (key, workload) in group.scheduler_registrations() {
            if !names_a_task_of_this_service(&workload) {
                continue;
            }
            if !wanted.iter().any(|(held, _)| *held == key) {
                group.deregister(&key, now);
            }
        }
        // An address is a target of an `ip` group alone — the type the binding of an
        // `awsvpc` service's task definition is held to (`crate::state::required_target_type`).
        if group.target_type != TARGET_TYPE_IP {
            continue;
        }
        for (key, registration) in wanted {
            // Whether the group ALREADY holds this registration decides how to read the
            // answer below: a key that is already there is the steady state, while a key
            // that is not and still is not afterwards is a registration the group had no
            // room for.
            let held = group.targets.contains_key(&key);
            let registered = group.register_for(
                &registration.target_id,
                registration.port,
                zones.get(&registration.target_id).cloned(),
                &registration.task_arn,
                registration.served_ports.clone(),
                now,
            );
            // A group already holding its quota of targets takes no further registration, so
            // the task is reachable through no target: said rather than left to be read as a
            // task nothing routes to.
            if !held && !registered {
                ::tracing::warn!(
                    target_group = %arn,
                    task = %registration.task_arn,
                    "an ECS task could not be registered as a target: the target group holds \
                     its quota of targets"
                );
            }
            if registered {
                added.push((arn.clone(), key));
            }
        }
    }
    // The registrations this reconcile made are CHECKED at once, the way the Elastic Load
    // Balancing v2 write that registers a caller's own targets checks them: a task whose
    // containers do not listen on the group's health-check port answers no check, and the
    // state it is reported in must follow those checks from the moment it is registered
    // rather than wait for a clock wake-up. Only the registrations this call added, so a
    // reconcile that moved nothing checks nothing and no other registration is touched; with
    // the guard this reconcile held RELEASED, so the check takes the store's lock itself.
    drop(store);
    ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::check_registrations(
        engine,
        &added,
    );
}

/// Whether `workload` names a TASK of this service — the form
/// [`crate::state::task_arn`] mints — which is what makes a scheduler-owned registration
/// one this reconcile may add and remove.
///
/// Another emulated service's scheduler registering through the same seam names its own
/// resource, so its registrations are left exactly as they are.
fn names_a_task_of_this_service(workload: &str) -> bool {
    workload.starts_with("arn:")
        && workload.contains(&format!(":{ARN_NAMESPACE}:"))
        && workload.contains(&format!(":{TASK_RESOURCE_TYPE}/"))
}

/// The Availability Zone each address in `desired` is in, read from the subnet the task's
/// interface was placed in through Amazon EC2's OWN store — the same one-directional,
/// guard-free read every other cross-store access here makes.
///
/// An address whose subnet that store does not hold is absent from the answer, and the
/// registration then carries no zone rather than one invented: the zone is the subnet's
/// property, not the registration's.
fn target_availability_zones<Eng: Send + Sync + 'static>(
    engine: &Eng,
    desired: &[TargetRegistration],
) -> HashMap<String, String> {
    let mut zones = HashMap::new();
    if desired.is_empty() {
        return zones;
    }
    let Some(state) = ::service_com_amazonaws_ec2_amazonec2::state::service_state(engine) else {
        return zones;
    };
    let store = state.lock();
    for registration in desired {
        let Some(subnet_id) = registration.subnet_id.as_deref() else {
            continue;
        };
        if let Some(zone) = store
            .subnets
            .get(subnet_id)
            .and_then(|subnet| subnet.availability_zone.clone())
        {
            zones.insert(registration.target_id.clone(), zone);
        }
    }
    zones
}
