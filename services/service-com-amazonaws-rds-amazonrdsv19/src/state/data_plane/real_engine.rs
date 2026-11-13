// SPDX-License-Identifier: CC0-1.0

//! REAL ENGINE mode for a PostgreSQL DB instance: the instance's engine is a real PostgreSQL
//! server running as a container on the host, and the instance's own endpoint relays to it.
//!
//! The in-process engine ([`super::sql_engine`]) answers SQL the way SQLite does under a
//! PostgreSQL wire, which is what this emulator can run in process. An application whose schema
//! needs PostgreSQL itself — extensions such as PostGIS, `jsonb` operators, PostgreSQL locking —
//! cannot run against it. With host workloads turned on
//! ([`::service_common::host_workloads::enabled`]), `CreateDBInstance` for a PostgreSQL engine
//! instead starts ONE container of the official PostgreSQL image at the requested major
//! version, with PostGIS installed the way RDS makes it available (installed, not created),
//! through the shared host runtime ([`::service_common::host_runtime`]):
//!
//! * the container is created with the instance's master user, master password and `DBName`,
//!   so authentication — SCRAM included — is the real server's own;
//! * it carries the label [`RESOURCE_LABEL`] with the instance's `DbiResourceId`, so a retried
//!   or restarted create ADOPTS the container it started rather than starting a second one, and
//!   never adopts one it did not start;
//! * its data lives in the named volume [`volume_name`], which outlives the container.
//!
//! The instance's endpoint is still the listener the core opens and reports; that listener
//! serves [`PgRelay`], which answers a client's `SSLRequest` with the core's own TLS (the
//! certificate every client of this emulator already trusts), declines GSSAPI encryption, and
//! then relays every byte of the session to the container unchanged.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use ::service_common::host_runtime::{ContainerSpec, HostRuntime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The label naming the DB instance a container is the engine of: its `DbiResourceId`.
pub const RESOURCE_LABEL: &str = "aws-emulator.rds.dbi-resource-id";
/// The port PostgreSQL listens on inside its container.
const POSTGRES_PORT: u16 = 5432;
/// The PostgreSQL major version an instance gets when its request names none: RDS's default.
const DEFAULT_MAJOR: u32 = 16;
/// The PostGIS release installed beside the server: the newest one published for every major
/// version from 14 on, and the last one published for 13 and earlier.
fn postgis_release(major: u32) -> &'static str {
    if major >= 14 { "3.6" } else { "3.5" }
}
/// How long a starting server has to accept connections.
const READY_TIMEOUT: Duration = Duration::from_secs(180);

const SSL_REQUEST_CODE: i32 = 80_877_103;
const GSSENC_REQUEST_CODE: i32 = 80_877_104;
/// The largest startup message relayed: a startup packet is a few hundred bytes.
const MAX_STARTUP_BYTES: usize = 10_000;

/// What a real engine is started from.
#[derive(Debug, Clone)]
pub struct RealInstance {
    pub dbi_resource_id: String,
    pub engine_version: Option<String>,
    pub master_username: String,
    pub master_password: String,
    pub db_name: Option<String>,
}

/// Whether `CreateDBInstance` runs `engine` as a real engine.
pub fn applies(engine: &str) -> bool {
    ::service_common::host_workloads::enabled()
        && matches!(super::engine_wire(engine), Some(super::EngineWire::Postgres))
}

/// The image a PostgreSQL instance of `engine_version` runs.
pub fn postgres_image(engine_version: Option<&str>) -> String {
    let major = engine_version
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAJOR);
    format!("postgis/postgis:{major}-{}", postgis_release(major))
}

/// The named volume an instance's data lives in.
pub fn volume_name(dbi_resource_id: &str) -> String {
    format!("aws-emulator-rds-{}", dbi_resource_id.to_ascii_lowercase())
}

/// Start (or adopt) the instance's engine container and wait until it accepts connections.
/// Returns the address the engine is reached at.
pub async fn start(instance: &RealInstance) -> Result<SocketAddr, String> {
    let runtime = HostRuntime::default();
    let existing = runtime
        .find(RESOURCE_LABEL, &instance.dbi_resource_id)
        .await
        .map_err(|error| error.to_string())?;
    let id = match existing.first() {
        Some(id) => {
            runtime.start(id).await.map_err(|error| error.to_string())?;
            id.clone()
        }
        None => runtime
            .run(&container_spec(instance))
            .await
            .map_err(|error| error.to_string())?,
    };
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        let status = runtime.inspect(&id).await.map_err(|error| error.to_string())?;
        if !status.running && status.exit_code.is_some() {
            return Err(format!(
                "the DB engine container exited with code {}",
                status.exit_code.unwrap_or_default()
            ));
        }
        if let Some(address) = status.address {
            let address = SocketAddr::new(address, POSTGRES_PORT);
            // The image's first start runs its init scripts against a server that listens on
            // its Unix socket only, so a TCP accept is the restarted, real server.
            if tokio::net::TcpStream::connect(address).await.is_ok() {
                return Ok(address);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("the DB engine container did not accept connections in time".to_string());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn container_spec(instance: &RealInstance) -> ContainerSpec {
    let mut env = vec![
        ("POSTGRES_USER".to_string(), instance.master_username.clone()),
        ("POSTGRES_PASSWORD".to_string(), instance.master_password.clone()),
    ];
    if let Some(db_name) = &instance.db_name {
        env.push(("POSTGRES_DB".to_string(), db_name.clone()));
    }
    ContainerSpec {
        name: format!("aws-emulator-rds-{}", instance.dbi_resource_id.to_ascii_lowercase()),
        image: postgres_image(instance.engine_version.as_deref()),
        env,
        labels: [(RESOURCE_LABEL.to_string(), instance.dbi_resource_id.clone())].into(),
        volumes: vec![(
            volume_name(&instance.dbi_resource_id),
            "/var/lib/postgresql/data".to_string(),
        )],
        ..ContainerSpec::default()
    }
}

/// The handler a real-engine instance's listener serves: TLS by the core, bytes to the engine.
pub struct PgRelay {
    upstream: SocketAddr,
    upgrader: ::emulator_server::TlsUpgrader,
}

impl PgRelay {
    pub fn new(upstream: SocketAddr, upgrader: ::emulator_server::TlsUpgrader) -> Self {
        Self { upstream, upgrader }
    }
}

impl ::emulator_server::NativeHandler for PgRelay {
    fn serve(
        &self,
        conn: ::emulator_server::ListenerConn,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let upstream = self.upstream;
        let upgrader = self.upgrader.clone();
        Box::pin(async move {
            // An I/O error is either side's connection going away, which ends the session.
            let _ = relay(conn, upstream, upgrader).await;
        })
    }
}

async fn relay(
    mut conn: ::emulator_server::ListenerConn,
    upstream: SocketAddr,
    upgrader: ::emulator_server::TlsUpgrader,
) -> std::io::Result<()> {
    let mut encrypted = false;
    // The requests that precede the startup message are answered here; the startup message
    // itself, and everything after it, is the engine's.
    let startup = loop {
        let mut length = [0u8; 4];
        conn.read_exact(&mut length).await?;
        let body_length = usize::try_from(i32::from_be_bytes(length))
            .ok()
            .and_then(|length| length.checked_sub(4))
            .filter(|body| *body <= MAX_STARTUP_BYTES)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "bad startup length")
            })?;
        let mut body = vec![0u8; body_length];
        conn.read_exact(&mut body).await?;
        let code = (body.len() >= 4).then(|| i32::from_be_bytes([body[0], body[1], body[2], body[3]]));
        match (body.len(), code) {
            (4, Some(SSL_REQUEST_CODE)) if !encrypted => {
                conn.write_all(b"S").await?;
                conn.flush().await?;
                conn = upgrader.accept(conn, None).await?.0;
                encrypted = true;
            }
            (4, Some(GSSENC_REQUEST_CODE)) => {
                conn.write_all(b"N").await?;
                conn.flush().await?;
            }
            _ => {
                let mut message = length.to_vec();
                message.extend_from_slice(&body);
                break message;
            }
        }
    };
    let mut engine = tokio::net::TcpStream::connect(upstream).await?;
    engine.write_all(&startup).await?;
    tokio::io::copy_bidirectional(&mut conn, &mut engine).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_follows_the_requested_major_version() {
        assert_eq!(postgres_image(Some("18.4")), "postgis/postgis:18-3.6");
        assert_eq!(postgres_image(Some("13.20")), "postgis/postgis:13-3.5");
        assert_eq!(postgres_image(None), "postgis/postgis:16-3.6");
        assert_eq!(postgres_image(Some("latest")), "postgis/postgis:16-3.6");
    }

    #[test]
    fn the_container_carries_the_master_user_and_the_resource_label() {
        let spec = container_spec(&RealInstance {
            dbi_resource_id: "db-ABC".into(),
            engine_version: Some("16.4".into()),
            master_username: "postgres".into(),
            master_password: "secret".into(),
            db_name: Some("app".into()),
        });
        assert_eq!(spec.labels[RESOURCE_LABEL], "db-ABC");
        assert!(spec.env.contains(&("POSTGRES_USER".into(), "postgres".into())));
        assert!(spec.env.contains(&("POSTGRES_DB".into(), "app".into())));
        assert_eq!(spec.volumes[0].0, "aws-emulator-rds-db-abc");
    }
}
