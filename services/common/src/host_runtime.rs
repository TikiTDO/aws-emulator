// SPDX-License-Identifier: CC0-1.0

//! The HOST RUNTIME: the one client every service uses to run a workload as a real container,
//! when host workloads are turned on ([`super::host_workloads::enabled`]).
//!
//! Amazon ECS runs a task's containers, Amazon RDS runs a DB instance's engine, and later Lambda
//! and ElastiCache run theirs. Each of them needs the same few verbs of a local container
//! runtime — create, start, inspect, stop, remove, and find again by label — so they live here
//! once rather than in every service crate, and every container any service starts is found and
//! cleaned up the same way.
//!
//! The runtime is the Docker Engine API, spoken over HTTP/1.1 on its Unix socket
//! ([`SOCKET_VARIABLE`], default [`DEFAULT_SOCKET`]). Every container this emulator starts is
//! attached to ONE network ([`NETWORK_VARIABLE`], default [`DEFAULT_NETWORK`]) on ONE subnet
//! ([`SUBNET_VARIABLE`], default [`DEFAULT_SUBNET`], gateway its first address — the address the
//! emulator binds so containers reach it, and the sandbox DNS answers on) and carries the
//! label [`MANAGED_LABEL`], so a restarted emulator can find what it started and nothing it did
//! not start is ever touched.
//!
//! Nothing here runs unless host workloads are on: a service checks
//! [`super::host_workloads::enabled`] before it reaches for a [`HostRuntime`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};

/// The environment variable naming the runtime's socket.
pub const SOCKET_VARIABLE: &str = "EMULATOR_HOST_RUNTIME_SOCKET";
/// The runtime's socket when the variable is unset.
pub const DEFAULT_SOCKET: &str = "/var/run/docker.sock";
/// The environment variable naming the network every started container joins.
pub const NETWORK_VARIABLE: &str = "EMULATOR_HOST_NETWORK";
/// The network every started container joins when the variable is unset.
pub const DEFAULT_NETWORK: &str = "aws-emulator";
/// The environment variable naming the network's IPv4 subnet, as a CIDR.
pub const SUBNET_VARIABLE: &str = "EMULATOR_HOST_SUBNET";
/// The subnet when the variable is unset: its gateway, 172.25.0.1, is the emulator's bind address
/// and the sandbox DNS server's.
pub const DEFAULT_SUBNET: &str = "172.25.0.0/16";
/// The label every container this emulator starts carries.
pub const MANAGED_LABEL: &str = "aws-emulator.managed";
/// The Docker Engine API version spoken.
const API_VERSION: &str = "v1.43";
/// How long one runtime call may take, except pulling an image.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long pulling one image may take.
const PULL_TIMEOUT: Duration = Duration::from_secs(900);

/// What went wrong talking to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    /// The HTTP status the runtime answered, or `None` when it could not be reached.
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(f, "container runtime answered {status}: {}", self.message),
            None => write!(f, "container runtime unreachable: {}", self.message),
        }
    }
}

impl std::error::Error for RuntimeError {}

fn unreachable(message: impl Into<String>) -> RuntimeError {
    RuntimeError {
        status: None,
        message: message.into(),
    }
}

/// One container to start.
#[derive(Debug, Clone, Default)]
pub struct ContainerSpec {
    /// The container's name; unique on the host.
    pub name: String,
    /// The image reference, `repository:tag` or `repository@digest`.
    pub image: String,
    /// `linux/amd64`, `linux/arm64`, or `None` for the host's own.
    pub platform: Option<String>,
    pub env: Vec<(String, String)>,
    /// Labels besides [`MANAGED_LABEL`], which is always added.
    pub labels: BTreeMap<String, String>,
    /// The command, overriding the image's; empty keeps the image's.
    pub command: Vec<String>,
    /// The entrypoint, overriding the image's; empty keeps the image's.
    pub entrypoint: Vec<String>,
    /// Named volumes, `(volume, path in the container)`.
    pub volumes: Vec<(String, String)>,
    /// Aliases the container answers to on the network, besides its name.
    pub aliases: Vec<String>,
}

/// What the runtime reports about a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerStatus {
    pub id: String,
    pub running: bool,
    /// The exit code, once the container has stopped.
    pub exit_code: Option<i64>,
    /// The container's address on the emulator's network, once it has one.
    pub address: Option<std::net::IpAddr>,
}

/// The runtime client. Cheap to construct; holds no connection.
#[derive(Debug, Clone)]
pub struct HostRuntime {
    socket: PathBuf,
    network: String,
    subnet: String,
}

impl Default for HostRuntime {
    fn default() -> Self {
        Self {
            socket: std::env::var(SOCKET_VARIABLE)
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_SOCKET)),
            network: std::env::var(NETWORK_VARIABLE).unwrap_or_else(|_| DEFAULT_NETWORK.to_string()),
            subnet: std::env::var(SUBNET_VARIABLE).unwrap_or_else(|_| DEFAULT_SUBNET.to_string()),
        }
    }
}

impl HostRuntime {
    /// The runtime behind `socket`, starting its containers on `network`, on the configured
    /// subnet.
    pub fn at(socket: impl Into<PathBuf>, network: impl Into<String>) -> Self {
        Self {
            socket: socket.into(),
            network: network.into(),
            ..Self::default()
        }
    }

    /// The network every container this runtime starts joins.
    /// The runtime behind `socket`, starting its containers on `network`.
    pub fn at(socket: impl Into<PathBuf>, network: impl Into<String>) -> Self {
        Self {
            socket: socket.into(),
            network: network.into(),
        }
    }

    pub fn network(&self) -> &str {
        &self.network
    }

    /// Create and start `spec`, pulling its image first when the host does not hold it.
    /// Returns the container id.
    pub async fn run(&self, spec: &ContainerSpec) -> Result<String, RuntimeError> {
        self.ensure_network().await?;
        let id = match self.create(spec).await {
            Err(RuntimeError {
                status: Some(404), ..
            }) => {
                self.pull(&spec.image, spec.platform.as_deref()).await?;
                self.create(spec).await?
            }
            created => created?,
        };
        self.call("POST", &format!("/containers/{id}/start"), None, CALL_TIMEOUT)
            .await?;
        Ok(id)
    }

    /// Start container `id`, which already exists. Starting a running container is not an error.
    pub async fn start(&self, id: &str) -> Result<(), RuntimeError> {
        match self
            .call("POST", &format!("/containers/{id}/start"), None, CALL_TIMEOUT)
            .await
        {
            // 304: already started.
            Ok(_) | Err(RuntimeError { status: Some(304), .. }) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// What the runtime reports about container `id`.
    pub async fn inspect(&self, id: &str) -> Result<ContainerStatus, RuntimeError> {
        let (_, body) = self
            .call("GET", &format!("/containers/{id}/json"), None, CALL_TIMEOUT)
            .await?;
        Ok(status_of(&body, &self.network))
    }

    /// Stop container `id`, giving it `grace` to exit on its own.
    pub async fn stop(&self, id: &str, grace: Duration) -> Result<(), RuntimeError> {
        match self
            .call(
                "POST",
                &format!("/containers/{id}/stop?t={}", grace.as_secs()),
                None,
                CALL_TIMEOUT + grace,
            )
            .await
        {
            // 304: already stopped.
            Ok(_) | Err(RuntimeError { status: Some(304), .. }) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Remove container `id`, stopping it if it still runs. Its named volumes are kept.
    pub async fn remove(&self, id: &str) -> Result<(), RuntimeError> {
        match self
            .call("DELETE", &format!("/containers/{id}?force=true"), None, CALL_TIMEOUT)
            .await
        {
            Ok(_) | Err(RuntimeError { status: Some(404), .. }) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Remove named volume `name`. A volume that does not exist is already removed.
    pub async fn remove_volume(&self, name: &str) -> Result<(), RuntimeError> {
        match self
            .call("DELETE", &format!("/volumes/{name}"), None, CALL_TIMEOUT)
            .await
        {
            Ok(_) | Err(RuntimeError { status: Some(404), .. }) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The ids of every container this emulator started that carries `label=value`, running or
    /// not.
    pub async fn find(&self, label: &str, value: &str) -> Result<Vec<String>, RuntimeError> {
        let filters = json!({ "label": [MANAGED_LABEL, format!("{label}={value}")] }).to_string();
        let (_, body) = self
            .call(
                "GET",
                &format!("/containers/json?all=true&filters={}", query_escaped(&filters)),
                None,
                CALL_TIMEOUT,
            )
            .await?;
        Ok(body
            .as_array()
            .map(|containers| {
                containers
                    .iter()
                    .filter_map(|container| container["Id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn create(&self, spec: &ContainerSpec) -> Result<String, RuntimeError> {
        let mut path = format!("/containers/create?name={}", query_escaped(&spec.name));
        if let Some(platform) = &spec.platform {
            path.push_str(&format!("&platform={}", query_escaped(platform)));
        }
        let (_, body) = self
            .call("POST", &path, Some(create_body(spec, &self.network)), CALL_TIMEOUT)
            .await?;
        body["Id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| unreachable("the runtime created a container and named no id"))
    }

    async fn pull(&self, image: &str, platform: Option<&str>) -> Result<(), RuntimeError> {
        let (repository, tag) = split_reference(image);
        let mut path = format!("/images/create?fromImage={}", query_escaped(repository));
        if let Some(tag) = tag {
            path.push_str(&format!("&tag={}", query_escaped(tag)));
        }
        if let Some(platform) = platform {
            path.push_str(&format!("&platform={}", query_escaped(platform)));
        }
        let (_, body) = self.call("POST", &path, None, PULL_TIMEOUT).await?;
        // A pull reports its own failure inside a 200 stream.
        match body.get("error").and_then(Value::as_str) {
            Some(error) => Err(RuntimeError {
                status: Some(200),
                message: error.to_string(),
            }),
            None => Ok(()),
        }
    }

    /// Make sure the network exists on the configured subnet. A network of that name on another
    /// subnet is refused rather than joined: the emulator's address would not be on it.
    async fn ensure_network(&self) -> Result<(), RuntimeError> {
        let network = self.network.clone();
        match self
            .call("GET", &format!("/networks/{}", query_escaped(&network)), None, CALL_TIMEOUT)
            .await
        {
            Ok((_, existing)) => check_subnet(&existing, &network, &self.subnet),
            Err(RuntimeError { status: Some(404), .. }) => {
                let body = network_body(&network, &self.subnet)?;
                match self.call("POST", "/networks/create", Some(body), CALL_TIMEOUT).await {
                    // 409: created concurrently by another service.
                    Ok(_) | Err(RuntimeError { status: Some(409), .. }) => Ok(()),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// One call to the runtime. A streamed answer (an image pull) is read to its end, and its
    /// LAST JSON object is what is returned.
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<(u16, Value), RuntimeError> {
        let socket = self.socket.clone();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("/{API_VERSION}{path}"))
            .header("host", "docker")
            .header("content-type", "application/json")
            .body(Full::new(bytes::Bytes::from(
                body.map(|body| body.to_string()).unwrap_or_default(),
            )))
            .map_err(|error| unreachable(error.to_string()))?;
        let exchange = async move {
            let stream = tokio::net::UnixStream::connect(&socket)
                .await
                .map_err(|error| unreachable(format!("{}: {error}", socket.display())))?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .map_err(|error| unreachable(error.to_string()))?;
            let driver = tokio::spawn(async move {
                let _ = connection.await;
            });
            let response = sender
                .send_request(request)
                .await
                .map_err(|error| unreachable(error.to_string()));
            let answered = match response {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let bytes = response
                        .into_body()
                        .collect()
                        .await
                        .map_err(|error| unreachable(error.to_string()))
                        .map(|collected| collected.to_bytes());
                    bytes.map(|bytes| (status, bytes))
                }
                Err(error) => Err(error),
            };
            driver.abort();
            answered
        };
        let (status, bytes) = tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| unreachable(format!("{method} {path} timed out")))??;
        let value = last_json(&bytes);
        if (200..300).contains(&status) {
            Ok((status, value))
        } else {
            Err(RuntimeError {
                status: Some(status),
                message: value["message"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string()),
            })
        }
    }
}

/// The gateway of an IPv4 `subnet` CIDR: its first address.
fn gateway_of(subnet: &str) -> Result<std::net::Ipv4Addr, RuntimeError> {
    let invalid = || unreachable(format!("{SUBNET_VARIABLE}={subnet:?} is not an IPv4 CIDR"));
    let (address, prefix) = subnet.split_once('/').ok_or_else(invalid)?;
    let address: std::net::Ipv4Addr = address.parse().map_err(|_| invalid())?;
    let prefix: u32 = prefix.parse().ok().filter(|prefix| *prefix <= 30).ok_or_else(invalid)?;
    let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
    Ok(std::net::Ipv4Addr::from((u32::from(address) & mask) + 1))
}

/// The body of `POST /networks/create` for `network` on `subnet`.
fn network_body(network: &str, subnet: &str) -> Result<Value, RuntimeError> {
    let gateway = gateway_of(subnet)?;
    Ok(json!({
        "Name": network,
        "Driver": "bridge",
        "Labels": { MANAGED_LABEL: "true" },
        "IPAM": { "Config": [{ "Subnet": subnet, "Gateway": gateway.to_string() }] },
    }))
}

/// Whether an existing network (`GET /networks/{name}`) is on `subnet`.
fn check_subnet(existing: &Value, network: &str, subnet: &str) -> Result<(), RuntimeError> {
    let subnets: Vec<&str> = existing["IPAM"]["Config"]
        .as_array()
        .map(|configs| configs.iter().filter_map(|config| config["Subnet"].as_str()).collect())
        .unwrap_or_default();
    if subnets.contains(&subnet) {
        Ok(())
    } else {
        Err(RuntimeError {
            status: Some(409),
            message: format!(
                "network {network} exists on {subnets:?}, not {subnet}; remove it or set {SUBNET_VARIABLE}"
            ),
        })
    }
}

/// The body of `POST /containers/create` for `spec` on `network`.
fn create_body(spec: &ContainerSpec, network: &str) -> Value {
    let mut labels: BTreeMap<String, String> = spec.labels.clone();
    labels.insert(MANAGED_LABEL.to_string(), "true".to_string());
    let mut body = json!({
        "Image": spec.image,
        "Env": spec.env.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>(),
        "Labels": labels,
        "HostConfig": {
            "NetworkMode": network,
            "Mounts": spec.volumes.iter().map(|(volume, target)| json!({
                "Type": "volume", "Source": volume, "Target": target,
            })).collect::<Vec<_>>(),
        },
        "NetworkingConfig": {
            "EndpointsConfig": { network: { "Aliases": spec.aliases } }
        },
    });
    if !spec.command.is_empty() {
        body["Cmd"] = json!(spec.command);
    }
    if !spec.entrypoint.is_empty() {
        body["Entrypoint"] = json!(spec.entrypoint);
    }
    body
}

/// What `GET /containers/{id}/json` says about the container, read for `network`.
fn status_of(body: &Value, network: &str) -> ContainerStatus {
    let state = &body["State"];
    let running = state["Running"].as_bool().unwrap_or(false);
    ContainerStatus {
        id: body["Id"].as_str().unwrap_or_default().to_string(),
        running,
        exit_code: (!running).then(|| state["ExitCode"].as_i64()).flatten(),
        address: body["NetworkSettings"]["Networks"][network]["IPAddress"]
            .as_str()
            .and_then(|address| address.parse().ok()),
    }
}

/// `repository[:tag]` split at the tag; a digest reference has none.
fn split_reference(image: &str) -> (&str, Option<&str>) {
    if image.contains('@') {
        return (image, None);
    }
    match image.rsplit_once(':') {
        // A colon inside the registry host (`host:5000/name`) is a port, not a tag.
        Some((repository, tag)) if !tag.contains('/') => (repository, Some(tag)),
        _ => (image, None),
    }
}

/// The last JSON value in a body that may be a stream of them, or `Null`.
fn last_json(bytes: &[u8]) -> Value {
    serde_json::Deserializer::from_slice(bytes)
        .into_iter::<Value>()
        .filter_map(Result::ok)
        .last()
        .unwrap_or(Value::Null)
}

/// Percent-encode a query value.
fn query_escaped(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_splits_at_its_tag_but_not_at_a_registry_port() {
        assert_eq!(split_reference("postgres:16"), ("postgres", Some("16")));
        assert_eq!(split_reference("host:5000/app"), ("host:5000/app", None));
        assert_eq!(split_reference("host:5000/app:1"), ("host:5000/app", Some("1")));
        assert_eq!(split_reference("app@sha256:ab"), ("app@sha256:ab", None));
    }

    #[test]
    fn a_created_container_joins_the_network_and_carries_the_managed_label() {
        let spec = ContainerSpec {
            name: "db".into(),
            image: "postgres:16".into(),
            env: vec![("POSTGRES_USER".into(), "admin".into())],
            labels: [("aws-emulator.resource".to_string(), "arn:db".to_string())].into(),
            volumes: vec![("vol".into(), "/var/lib/postgresql/data".into())],
            aliases: vec!["db.local".into()],
            ..ContainerSpec::default()
        };
        let body = create_body(&spec, "net");
        assert_eq!(body["HostConfig"]["NetworkMode"], "net");
        assert_eq!(body["Labels"][MANAGED_LABEL], "true");
        assert_eq!(body["Labels"]["aws-emulator.resource"], "arn:db");
        assert_eq!(body["Env"][0], "POSTGRES_USER=admin");
        assert_eq!(body["NetworkingConfig"]["EndpointsConfig"]["net"]["Aliases"][0], "db.local");
        assert!(body.get("Cmd").is_none());
    }

    #[test]
    fn inspect_reads_running_state_exit_code_and_network_address() {
        let body = json!({
            "Id": "c1",
            "State": { "Running": false, "ExitCode": 3 },
            "NetworkSettings": { "Networks": { "net": { "IPAddress": "172.25.0.7" } } },
        });
        let status = status_of(&body, "net");
        assert!(!status.running);
        assert_eq!(status.exit_code, Some(3));
        assert_eq!(status.address, Some("172.25.0.7".parse().unwrap()));
    }

    #[test]
    fn a_pull_stream_answers_with_its_last_object() {
        let stream = b"{\"status\":\"Pulling\"}\r\n{\"error\":\"denied\"}\r\n";
        assert_eq!(last_json(stream)["error"], "denied");
    }

    #[test]
    fn a_label_filter_is_query_escaped() {
        assert_eq!(query_escaped("a=b c"), "a%3Db%20c");
    }

    #[test]
    fn the_network_is_created_on_the_subnet_with_its_first_address_as_gateway() {
        let body = network_body("net", "172.25.0.0/16").unwrap();
        assert_eq!(body["IPAM"]["Config"][0]["Subnet"], "172.25.0.0/16");
        assert_eq!(body["IPAM"]["Config"][0]["Gateway"], "172.25.0.1");
        assert_eq!(gateway_of("10.9.8.7/24").unwrap().to_string(), "10.9.8.1");
        assert!(gateway_of("172.25.0.0").is_err());
        assert!(gateway_of("not-an-address/16").is_err());
    }

    #[test]
    fn an_existing_network_on_another_subnet_is_refused() {
        let on = json!({ "IPAM": { "Config": [{ "Subnet": "172.25.0.0/16" }] } });
        let off = json!({ "IPAM": { "Config": [{ "Subnet": "172.18.0.0/16" }] } });
        assert!(check_subnet(&on, "net", "172.25.0.0/16").is_ok());
        let error = check_subnet(&off, "net", "172.25.0.0/16").unwrap_err();
        assert_eq!(error.status, Some(409));
        assert!(error.message.contains("172.18.0.0/16"));
    }
}
