// SPDX-License-Identifier: CC0-1.0

//! The HOST WORKLOAD registry: where a workload this emulator started as a real process or
//! container on the host can be reached, for the services that send it traffic.
//!
//! The emulator's default is that no user workload runs outside the process ("containers and
//! external processes are not available to a generated emulator yet", `engine`), so a load
//! balancer forwarding to an `ip` or `instance` target answers the `502` a load balancer gives
//! for a target it cannot connect to. With HOST WORKLOADS turned on ([`enabled`]), a scheduler
//! that does start one — Amazon ECS running a task's containers in a local container runtime —
//! PUBLISHES where each container port it serves can be dialled, and a service that forwards
//! traffic to that workload (Elastic Load Balancing) RESOLVES the registration it holds to that
//! address and connects to it.
//!
//! The record is a fact the two services must agree on exactly, and the engine keys state by
//! TYPE, so it is one SHARED type, reached as `engine.state::<HostWorkloads>()`: the scheduler
//! writes it and the forwarding service reads it, and neither names the other.
//!
//! A workload is published under every key a forwarding service may hold for it:
//!
//! * the WORKLOAD id the scheduler registered the target for (an ECS task ARN), which is what a
//!   registration a scheduler made carries ([`WorkloadKey::Workload`]);
//! * the ADDRESS the control plane reports for it (an ECS task's ENI private IP, an EC2
//!   instance id), which is what a caller's own `RegisterTargets` names
//!   ([`WorkloadKey::Target`]).
//!
//! Each key pairs with the CONTAINER port the target receives traffic on — the port a target
//! group or a registration names — and resolves to the host socket address that port is
//! reachable at, which is usually a different port on the loopback interface or an address on
//! the runtime's bridge network.
//!
//! Turning host workloads on removes the containment the emulator otherwise keeps (a workload
//! is reachable, and the forwarding service makes a connection to the host), so it is OFF
//! unless [`ENABLE_VARIABLE`] says otherwise.

use std::collections::HashMap;
use std::net::SocketAddr;

/// The environment variable that turns host workloads on: `on`, `true` or `1`.
pub const ENABLE_VARIABLE: &str = "EMULATOR_HOST_WORKLOADS";

/// Whether this emulator starts workloads on the host and forwards traffic to them.
pub fn enabled() -> bool {
    std::env::var(ENABLE_VARIABLE)
        .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "on" | "true" | "1"))
        .unwrap_or(false)
}

/// What a forwarding service knows a workload by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkloadKey {
    /// The workload a scheduler registered a target for: an ECS task ARN.
    Workload(String),
    /// The target id the control plane reports for the workload: an IP address or an
    /// instance id.
    Target(String),
}

/// Where every published workload port can be dialled. `Default` is empty: nothing runs.
#[derive(Debug, Default)]
pub struct HostWorkloads {
    endpoints: HashMap<(WorkloadKey, i32), SocketAddr>,
    /// Which keys each workload published, so withdrawing the workload removes all of them.
    published_by: HashMap<String, Vec<(WorkloadKey, i32)>>,
}

impl HostWorkloads {
    /// Publish that `workload`'s `container_port` is reachable at `address`, under the
    /// workload id itself and under every target id in `targets`.
    pub fn publish(
        &mut self,
        workload: &str,
        targets: &[String],
        container_port: i32,
        address: SocketAddr,
    ) {
        let keys = std::iter::once(WorkloadKey::Workload(workload.to_string()))
            .chain(targets.iter().cloned().map(WorkloadKey::Target));
        let published = self.published_by.entry(workload.to_string()).or_default();
        for key in keys {
            let entry = (key, container_port);
            self.endpoints.insert(entry.clone(), address);
            if !published.contains(&entry) {
                published.push(entry);
            }
        }
    }

    /// Withdraw everything `workload` published: it stopped, so nothing answers there.
    pub fn withdraw(&mut self, workload: &str) {
        for entry in self.published_by.remove(workload).unwrap_or_default() {
            self.endpoints.remove(&entry);
        }
    }

    /// Where `key`'s `container_port` can be dialled, or `None` for a port nothing published.
    pub fn resolve(&self, key: &WorkloadKey, container_port: i32) -> Option<SocketAddr> {
        self.endpoints.get(&(key.clone(), container_port)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn a_published_port_resolves_under_the_workload_and_its_targets() {
        let mut workloads = HostWorkloads::default();
        workloads.publish("arn:task/1", &["10.0.1.5".to_string()], 3000, addr(49152));
        assert_eq!(
            workloads.resolve(&WorkloadKey::Workload("arn:task/1".into()), 3000),
            Some(addr(49152))
        );
        assert_eq!(
            workloads.resolve(&WorkloadKey::Target("10.0.1.5".into()), 3000),
            Some(addr(49152))
        );
        assert_eq!(workloads.resolve(&WorkloadKey::Target("10.0.1.5".into()), 80), None);
    }

    #[test]
    fn withdrawing_a_workload_removes_every_key_it_published() {
        let mut workloads = HostWorkloads::default();
        workloads.publish("arn:task/1", &["10.0.1.5".to_string()], 3000, addr(49152));
        workloads.publish("arn:task/2", &["10.0.1.6".to_string()], 3000, addr(49153));
        workloads.withdraw("arn:task/1");
        assert_eq!(workloads.resolve(&WorkloadKey::Target("10.0.1.5".into()), 3000), None);
        assert_eq!(
            workloads.resolve(&WorkloadKey::Target("10.0.1.6".into()), 3000),
            Some(addr(49153))
        );
    }
}
