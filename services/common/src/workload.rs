// SPDX-License-Identifier: CC0-1.0

//! WORKLOADS: what a service asks for when it runs something real on the host — an ECS task, an
//! RDS engine — stated as desired state and observed state, not as a runtime's own verbs.
//!
//! A service says what should exist ([`WorkloadSpec`]) and [`Workloads::ensure`] makes it exist:
//! it ADOPTS the one workload already carrying the spec's identity, or starts it. The service
//! then reads what is true ([`Workloads::observe`]): pending, running at an address, exited, or
//! gone. Readiness in a protocol's own sense (a database accepting a login, an HTTP health
//! check) stays the service's to judge from that observation; this layer reports only what the
//! runtime itself knows. [`Workloads::release`] ends a workload and keeps or deletes its data.
//!
//! WHERE a workload runs is a placement, chosen per workload kind ([`Placement`]): which
//! runtime, and which network on it. The only backend today is the Docker Engine of
//! [`crate::host_runtime`]; a cluster backend (Kubernetes) is the planned second, behind the
//! same three calls, so no service ever calls a runtime directly.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use crate::host_runtime::{ContainerSpec, HostRuntime, RuntimeError};

/// The variable naming a placement per workload kind:
/// `kind=socket[#network][,kind=socket[#network]…]`, e.g.
/// `rds=/run/aurora-docker.sock#cdk-local-net,ecs-arm64=/run/mac-docker.sock`. A kind it does not
/// name runs on the default runtime ([`HostRuntime::default`]).
pub const PLACEMENT_VARIABLE: &str = "EMULATOR_HOST_PLACEMENT";

/// The label carrying a workload's kind.
pub const KIND_LABEL: &str = "aws-emulator.workload.kind";
/// The label carrying a workload's identity: what a retried `ensure` adopts by.
pub const IDENTITY_LABEL: &str = "aws-emulator.workload.identity";

/// How long a workload is given to stop before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// One workload a service wants to exist.
#[derive(Debug, Clone, Default)]
pub struct WorkloadSpec {
    /// What kind of workload this is (`rds`, `ecs`, `ecs-arm64`, …): chooses its placement.
    pub kind: String,
    /// Who it is, unique within its kind: an RDS `DbiResourceId`, an ECS task ARN. A workload
    /// carrying this identity is adopted rather than started a second time.
    pub identity: String,
    /// The runtime's name for it; unique on its host.
    pub name: String,
    pub image: String,
    pub platform: Option<String>,
    pub env: Vec<(String, String)>,
    pub labels: BTreeMap<String, String>,
    pub command: Vec<String>,
    pub entrypoint: Vec<String>,
    /// Named volumes, `(volume, path in the workload)`: its data, which outlives it unless
    /// [`Workloads::release`] is told not to keep it.
    pub volumes: Vec<(String, String)>,
    pub aliases: Vec<String>,
}

/// A workload [`Workloads::ensure`] made exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadHandle {
    pub kind: String,
    /// The runtime's own id for it.
    pub id: String,
    /// Whether `ensure` adopted an existing workload rather than starting one.
    pub adopted: bool,
}

/// What the runtime reports about a workload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// It exists and has not started running yet.
    Pending,
    /// It runs, reachable at `address` on its placement's network once it has one.
    Running { address: Option<IpAddr> },
    /// It ran and stopped with `code`.
    Exited { code: i64 },
    /// The runtime no longer has it.
    Missing,
}

impl Phase {
    /// The address a port of this workload is reached at, once it runs and has one.
    pub fn endpoint(&self, port: u16) -> Option<SocketAddr> {
        match self {
            Phase::Running {
                address: Some(address),
            } => Some(SocketAddr::new(*address, port)),
            _ => None,
        }
    }
}

/// Where a kind of workload runs.
#[derive(Debug, Clone)]
pub struct Placement {
    runtime: HostRuntime,
}

/// The workloads of this emulator, placed per kind.
#[derive(Debug, Clone)]
pub struct Workloads {
    placements: BTreeMap<String, Placement>,
    fallback: Placement,
}

impl Default for Workloads {
    /// The placements [`PLACEMENT_VARIABLE`] names, on top of the default runtime.
    fn default() -> Self {
        Self::from_placement(&std::env::var(PLACEMENT_VARIABLE).unwrap_or_default())
    }
}

impl Workloads {
    /// Parse a placement value (the form [`PLACEMENT_VARIABLE`] documents). An entry that does
    /// not parse is skipped, so its kind runs on the default runtime.
    pub fn from_placement(value: &str) -> Self {
        let fallback = Placement {
            runtime: HostRuntime::default(),
        };
        let placements = value
            .split(',')
            .filter_map(|entry| {
                let (kind, target) = entry.trim().split_once('=')?;
                let (socket, network) = match target.split_once('#') {
                    Some((socket, network)) => (socket, Some(network)),
                    None => (target, None),
                };
                (!kind.is_empty() && !socket.is_empty()).then(|| {
                    let network = network
                        .filter(|network| !network.is_empty())
                        .unwrap_or(fallback.runtime.network());
                    (
                        kind.to_string(),
                        Placement {
                            runtime: HostRuntime::at(socket, network),
                        },
                    )
                })
            })
            .collect();
        Self {
            placements,
            fallback,
        }
    }

    fn runtime(&self, kind: &str) -> &HostRuntime {
        &self
            .placements
            .get(kind)
            .unwrap_or(&self.fallback)
            .runtime
    }

    /// Make `spec` exist: adopt the workload already carrying its kind and identity — starting
    /// it again if it had stopped — or start it.
    pub async fn ensure(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle, RuntimeError> {
        let runtime = self.runtime(&spec.kind);
        let adoptable = runtime
            .find(IDENTITY_LABEL, &identity_value(&spec.kind, &spec.identity))
            .await?;
        if let Some(id) = adoptable.into_iter().next() {
            runtime.start(&id).await?;
            return Ok(WorkloadHandle {
                kind: spec.kind.clone(),
                id,
                adopted: true,
            });
        }
        let id = runtime.run(&container_spec(spec)).await?;
        Ok(WorkloadHandle {
            kind: spec.kind.clone(),
            id,
            adopted: false,
        })
    }

    /// What is true of `handle` now.
    pub async fn observe(&self, handle: &WorkloadHandle) -> Result<Phase, RuntimeError> {
        match self.runtime(&handle.kind).inspect(&handle.id).await {
            Ok(status) => Ok(match (status.running, status.exit_code) {
                (true, _) => Phase::Running {
                    address: status.address,
                },
                (false, Some(code)) => Phase::Exited { code },
                (false, None) => Phase::Pending,
            }),
            Err(RuntimeError {
                status: Some(404), ..
            }) => Ok(Phase::Missing),
            Err(error) => Err(error),
        }
    }

    /// End `handle`: stop and remove it, and remove `volumes` too unless `keep_data`. A workload
    /// already gone is not an error.
    pub async fn release(
        &self,
        handle: &WorkloadHandle,
        volumes: &[String],
        keep_data: bool,
    ) -> Result<(), RuntimeError> {
        let runtime = self.runtime(&handle.kind);
        for step in [
            runtime.stop(&handle.id, STOP_GRACE).await,
            runtime.remove(&handle.id).await,
        ] {
            match step {
                Ok(()) | Err(RuntimeError {
                    status: Some(404), ..
                }) => {}
                Err(error) => return Err(error),
            }
        }
        if !keep_data {
            for volume in volumes {
                match runtime.remove_volume(volume).await {
                    Ok(()) | Err(RuntimeError {
                        status: Some(404), ..
                    }) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }
}

/// The identity label's value: kind and identity together, so two kinds never adopt each
/// other's workloads.
fn identity_value(kind: &str, identity: &str) -> String {
    format!("{kind}/{identity}")
}

fn container_spec(spec: &WorkloadSpec) -> ContainerSpec {
    let mut labels = spec.labels.clone();
    labels.insert(KIND_LABEL.to_string(), spec.kind.clone());
    labels.insert(
        IDENTITY_LABEL.to_string(),
        identity_value(&spec.kind, &spec.identity),
    );
    ContainerSpec {
        name: spec.name.clone(),
        image: spec.image.clone(),
        platform: spec.platform.clone(),
        env: spec.env.clone(),
        labels,
        command: spec.command.clone(),
        entrypoint: spec.entrypoint.clone(),
        volumes: spec.volumes.clone(),
        aliases: spec.aliases.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placement_names_a_runtime_per_kind() {
        let workloads = Workloads::from_placement(
            "rds=/run/aurora.sock#cdk-local-net, ecs-arm64=/run/mac.sock,broken,=/x",
        );
        assert_eq!(workloads.placements.len(), 2);
        assert_eq!(workloads.runtime("rds").network(), "cdk-local-net");
        assert_eq!(
            workloads.runtime("ecs-arm64").network(),
            workloads.fallback.runtime.network()
        );
        assert_eq!(
            workloads.runtime("ecs").network(),
            workloads.fallback.runtime.network()
        );
    }

    #[test]
    fn the_workload_carries_its_kind_and_identity() {
        let spec = container_spec(&WorkloadSpec {
            kind: "rds".into(),
            identity: "db-ABC".into(),
            name: "aws-emulator-rds-db-abc".into(),
            image: "postgis/postgis:18-3.6".into(),
            ..WorkloadSpec::default()
        });
        assert_eq!(spec.labels[KIND_LABEL], "rds");
        assert_eq!(spec.labels[IDENTITY_LABEL], "rds/db-ABC");
    }

    #[test]
    fn only_a_running_workload_with_an_address_has_an_endpoint() {
        let address: IpAddr = "172.25.0.9".parse().unwrap();
        assert_eq!(
            Phase::Running {
                address: Some(address)
            }
            .endpoint(5432),
            Some(SocketAddr::new(address, 5432))
        );
        assert_eq!(Phase::Running { address: None }.endpoint(5432), None);
        assert_eq!(Phase::Pending.endpoint(5432), None);
        assert_eq!(Phase::Exited { code: 1 }.endpoint(5432), None);
    }
}
