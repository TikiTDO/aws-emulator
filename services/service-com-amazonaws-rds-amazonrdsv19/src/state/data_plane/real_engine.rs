// SPDX-License-Identifier: CC0-1.0

//! REAL ENGINE mode for a PostgreSQL DB instance: the instance's engine is a real PostgreSQL
//! server running as a host workload, and the instance's own endpoint relays to it.
//!
//! The in-process engine ([`super::sql_engine`]) answers SQL the way SQLite does under a
//! PostgreSQL wire, which is what this emulator can run in process. An application whose schema
//! needs PostgreSQL itself — extensions such as PostGIS, `jsonb` operators, PostgreSQL locking —
//! cannot run against it. With host workloads turned on
//! ([`::service_common::host_workloads::enabled`]), `CreateDBInstance` for a PostgreSQL engine
//! instead ensures ONE workload of the official PostGIS-enabled PostgreSQL image at the requested
//! major version through the declarative workload layer ([`::service_common::workload`]):
//!
//! * the workload is created with the instance's master user, master password and `DBName`,
//!   so authentication — SCRAM included — is the real server's own;
//! * its workload kind is `"rds"` and its identity is the instance's `DbiResourceId`, so
//!   `EMULATOR_HOST_PLACEMENT` can place `rds` onto a dedicated host runtime and a retried or
//!   restarted create ADOPTS the existing workload rather than starting a second one;
//! * readiness probes the PostgreSQL v3 wire startup handshake until the postmaster answers
//!   with an `Authentication*` frame (`'R'`) rather than closing during `initdb` or rejecting
//!   with SQLSTATE `57P03` (`the database system is starting up`);
//! * its data lives in the named volume [`volume_name`], which outlives restarts and is removed
//!   when `DeleteDBInstance` deletes the instance with `SkipFinalSnapshot`.
//!
//! The instance's endpoint is still the listener the core opens and reports; that listener
//! serves [`PgRelay`], which answers a client's `SSLRequest` with the core's own TLS (the
//! certificate every client of this emulator already trusts), declines GSSAPI encryption, and
//! then relays every byte of the session to the engine unchanged.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use ::service_common::workload::{Phase, WorkloadHandle, WorkloadSpec, Workloads};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The workload kind `EMULATOR_HOST_PLACEMENT` routes RDS engines by.
pub const WORKLOAD_KIND: &str = "rds";
/// The label naming the DB instance a container is the engine of: its `DbiResourceId`.
pub const RESOURCE_LABEL: &str = "aws-emulator.rds.dbi-resource-id";
/// The port PostgreSQL listens on inside its container.
const POSTGRES_PORT: u16 = 5432;
/// The PostgreSQL major version an instance gets when its request names none: RDS's default.
const DEFAULT_MAJOR: u32 = 16;
/// PostgreSQL protocol version 3.0 (`(3 << 16) | 0`).
const PG_PROTOCOL_V3: i32 = 196_608;
/// SQLSTATE `cannot_connect_now` (`FATAL: the database system is starting up` / `shutting down`).
const SQLSTATE_CANNOT_CONNECT_NOW: &str = "57P03";

/// The PostGIS release installed beside the server: the newest one published for every major
/// version from 14 on, and the last one published for 13 and earlier.
fn postgis_release(major: u32) -> &'static str {
    if major >= 14 { "3.6" } else { "3.5" }
}
/// How long a starting server has to finish `initdb` and accept PostgreSQL sessions.
const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// Timeout on a single PostgreSQL startup probe attempt.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

const SSL_REQUEST_CODE: i32 = 80_877_103;
const GSSENC_REQUEST_CODE: i32 = 80_877_104;
/// The largest startup message relayed: a startup packet is a few hundred bytes.
const MAX_STARTUP_BYTES: usize = 10_000;
const MAX_PROBE_REPLY_BYTES: usize = 4_096;

/// What a real engine is started from.
#[derive(Debug, Clone)]
pub struct RealInstance {
    pub dbi_resource_id: String,
    pub engine_version: Option<String>,
    pub master_username: String,
    pub master_password: String,
    pub db_name: Option<String>,
}

/// Handle retained on the RDS store so `DeleteDBInstance` can stop and clean up the workload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RealWorkload {
    pub handle: WorkloadHandle,
    pub volume: String,
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

/// The named volume a manual or final DB snapshot's `PGDATA` copy lives in.
///
/// `DBSnapshotIdentifier` preserves caller case and may be up to 255 characters long, so the
/// volume name pairs a bounded lowercase slug with a SHA-256 digest suffix to stay collision-free
/// and within Docker volume-name limits.
pub fn snapshot_volume_name(snapshot_identifier: &str) -> String {
    use sha2::Digest as _;

    let digest = hex::encode(sha2::Sha256::digest(snapshot_identifier.as_bytes()));
    let slug: String = snapshot_identifier
        .chars()
        .take(32)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("aws-emulator-rds-snap-{}", &digest[..16])
    } else {
        format!("aws-emulator-rds-snap-{slug}-{}", &digest[..12])
    }
}

/// Copy a PostgreSQL `PGDATA` volume (`from_volume`) into `to_volume` on the RDS workload
/// placement, excluding stale postmaster lockfiles (`postmaster.pid`, `postmaster.opts`) just as
/// `pg_basebackup` does so `global/pg_control` drives clean WAL recovery on restore.
pub async fn copy_volume(from_volume: &str, to_volume: &str) -> Result<(), String> {
    Workloads::default()
        .copy_volume_excluding(
            WORKLOAD_KIND,
            from_volume,
            to_volume,
            &["postmaster.pid", "postmaster.opts"],
        )
        .await
        .map_err(|error| error.to_string())
}

/// Remove a retained DB snapshot volume on the RDS workload placement.
pub async fn remove_snapshot_volume(volume: &str) -> Result<(), String> {
    Workloads::default()
        .remove_volume(WORKLOAD_KIND, volume)
        .await
        .map_err(|error| error.to_string())
}

/// Ensure (adopt or start) the instance's engine workload and wait until the PostgreSQL v3 wire
/// handshake confirms `initdb` and recovery have completed.
pub async fn start(instance: &RealInstance) -> Result<(SocketAddr, RealWorkload), String> {
    let workloads = Workloads::default();
    let spec = workload_spec(instance);
    let volume = volume_name(&instance.dbi_resource_id);
    let handle = workloads
        .ensure(&spec)
        .await
        .map_err(|error| error.to_string())?;
    let probe_user = if instance.master_username.is_empty() {
        "postgres"
    } else {
        instance.master_username.as_str()
    };
    let probe_db = instance.db_name.as_deref().unwrap_or("postgres");
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        let phase = workloads
            .observe(&handle)
            .await
            .map_err(|error| error.to_string())?;
        match phase {
            Phase::Exited { code } => {
                return Err(format!("the DB engine container exited with code {code}"));
            }
            Phase::Missing => {
                return Err("the DB engine workload disappeared before becoming ready".to_string());
            }
            Phase::Running {
                address: Some(ip),
            } => {
                let address = SocketAddr::new(ip, POSTGRES_PORT);
                if postgres_wire_ready(address, probe_user, probe_db).await {
                    return Ok((address, RealWorkload { handle, volume }));
                }
            }
            Phase::Pending | Phase::Running { address: None } => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "the DB engine container did not finish startup and accept PostgreSQL sessions in time"
                    .to_string(),
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Stop and remove a real-engine workload, removing its named volume unless `keep_data` is set.
pub async fn release(workload: &RealWorkload, keep_data: bool) -> Result<(), String> {
    Workloads::default()
        .release(
            &workload.handle,
            std::slice::from_ref(&workload.volume),
            keep_data,
        )
        .await
        .map_err(|error| error.to_string())
}

fn workload_spec(instance: &RealInstance) -> WorkloadSpec {
    let mut env = vec![
        ("POSTGRES_USER".to_string(), instance.master_username.clone()),
        ("POSTGRES_PASSWORD".to_string(), instance.master_password.clone()),
    ];
    if let Some(db_name) = &instance.db_name {
        env.push(("POSTGRES_DB".to_string(), db_name.clone()));
    }
    WorkloadSpec {
        kind: WORKLOAD_KIND.to_string(),
        identity: instance.dbi_resource_id.clone(),
        name: format!("aws-emulator-rds-{}", instance.dbi_resource_id.to_ascii_lowercase()),
        image: postgres_image(instance.engine_version.as_deref()),
        env,
        labels: [(RESOURCE_LABEL.to_string(), instance.dbi_resource_id.clone())].into(),
        volumes: vec![(
            volume_name(&instance.dbi_resource_id),
            "/var/lib/postgresql/data".to_string(),
        )],
        ..WorkloadSpec::default()
    }
}

/// Probe `address` with a PostgreSQL v3 `StartupMessage` and verify the postmaster is ready to
/// authenticate sessions (and not still in `57P03` recovery/startup).
pub async fn postgres_wire_ready(address: SocketAddr, user: &str, database: &str) -> bool {
    let packet = build_startup_packet(user, database);
    tokio::time::timeout(PROBE_TIMEOUT, async move {
        let mut stream = tokio::net::TcpStream::connect(address).await.ok()?;
        stream.write_all(&packet).await.ok()?;
        stream.flush().await.ok()?;
        let mut header = [0u8; 5];
        stream.read_exact(&mut header).await.ok()?;
        let tag = header[0];
        let length = i32::from_be_bytes([header[1], header[2], header[3], header[4]]);
        let body_len = usize::try_from(length)
            .ok()
            .and_then(|total| total.checked_sub(4))
            .filter(|len| *len <= MAX_PROBE_REPLY_BYTES)?;
        let mut body = vec![0u8; body_len];
        stream.read_exact(&mut body).await.ok()?;
        let _ = stream.shutdown().await;
        Some(startup_reply_indicates_ready(tag, &body))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

fn build_startup_packet(user: &str, database: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(32 + user.len() + database.len());
    payload.extend_from_slice(&PG_PROTOCOL_V3.to_be_bytes());
    payload.extend_from_slice(b"user\0");
    payload.extend_from_slice(user.as_bytes());
    payload.push(0);
    payload.extend_from_slice(b"database\0");
    payload.extend_from_slice(database.as_bytes());
    payload.push(0);
    payload.push(0);
    let total_len = i32::try_from(payload.len() + 4).unwrap_or(i32::MAX);
    let mut packet = Vec::with_capacity(payload.len() + 4);
    packet.extend_from_slice(&total_len.to_be_bytes());
    packet.extend_from_slice(&payload);
    packet
}

fn startup_reply_indicates_ready(tag: u8, body: &[u8]) -> bool {
    match tag {
        // `Authentication*` (`Ok`, `CleartextPassword`, `MD5Password`, `SASL`, ...): postmaster
        // has finished `initdb` and recovery and is ready for client logins.
        b'R' => true,
        // `ErrorResponse`: check SQLSTATE (`C` field). `57P03` means the postmaster is still
        // starting up or shutting down; other SQLSTATEs (such as auth/db errors on an adopted
        // data volume) still prove the postmaster is up and serving sessions.
        b'E' => error_sqlstate(body) != Some(SQLSTATE_CANNOT_CONNECT_NOW),
        _ => false,
    }
}

fn error_sqlstate(body: &[u8]) -> Option<&str> {
    let mut cursor = 0usize;
    while cursor < body.len() {
        let field_type = body[cursor];
        if field_type == 0 {
            break;
        }
        cursor += 1;
        let end = body[cursor..]
            .iter()
            .position(|&byte| byte == 0)
            .map(|offset| cursor + offset)?;
        if field_type == b'C' {
            return std::str::from_utf8(&body[cursor..end]).ok();
        }
        cursor = end + 1;
    }
    None
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
        let code =
            (body.len() >= 4).then(|| i32::from_be_bytes([body[0], body[1], body[2], body[3]]));
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
    fn the_workload_spec_carries_rds_kind_identity_and_volume() {
        let spec = workload_spec(&RealInstance {
            dbi_resource_id: "db-ABC".into(),
            engine_version: Some("18.4".into()),
            master_username: "postgres".into(),
            master_password: "secret".into(),
            db_name: Some("app".into()),
        });
        assert_eq!(spec.kind, WORKLOAD_KIND);
        assert_eq!(spec.identity, "db-ABC");
        assert_eq!(spec.labels[RESOURCE_LABEL], "db-ABC");
        assert!(spec.env.contains(&("POSTGRES_USER".into(), "postgres".into())));
        assert!(spec.env.contains(&("POSTGRES_DB".into(), "app".into())));
        assert_eq!(spec.volumes[0].0, "aws-emulator-rds-db-abc");
    }

    #[test]
    fn snapshot_volumes_stay_bounded_and_distinguish_case_and_real_workload_roundtrips_json() {
        let upper = snapshot_volume_name("Daily-Prod-Snap");
        let lower = snapshot_volume_name("daily-prod-snap");
        assert!(upper.starts_with("aws-emulator-rds-snap-daily-prod-snap-"));
        assert_ne!(upper, lower, "case-distinct snapshot ids must not share one volume");

        let max_len_id = format!("a{}", "b".repeat(254));
        let max_vol = snapshot_volume_name(&max_len_id);
        assert!(max_vol.len() <= 72, "got len {}: {max_vol}", max_vol.len());
        assert!(
            max_vol
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        );

        let workload = RealWorkload {
            handle: WorkloadHandle {
                kind: WORKLOAD_KIND.into(),
                id: "container-xyz".into(),
                adopted: false,
            },
            volume: volume_name("db-ABC"),
        };
        let encoded = serde_json::to_string(&workload).unwrap();
        let decoded: RealWorkload = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, workload);
    }

    #[tokio::test]
    async fn wire_readiness_rejects_57p03_startup_and_accepts_authentication_challenge() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral listener");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            // 1st connection: simulate postmaster still starting up (`57P03`).
            let (mut conn1, _) = listener.accept().await.unwrap();
            let mut len_buf = [0u8; 4];
            conn1.read_exact(&mut len_buf).await.unwrap();
            let rem = usize::try_from(i32::from_be_bytes(len_buf) - 4).unwrap();
            let mut body = vec![0u8; rem];
            conn1.read_exact(&mut body).await.unwrap();
            let mut err_body = Vec::new();
            err_body.extend_from_slice(b"SFATAL\0C57P03\0Mthe database system is starting up\0\0");
            let mut err_frame = vec![b'E'];
            err_frame.extend_from_slice(
                &i32::try_from(err_body.len() + 4)
                    .unwrap()
                    .to_be_bytes(),
            );
            err_frame.extend_from_slice(&err_body);
            conn1.write_all(&err_frame).await.unwrap();

            // 2nd connection: simulate postmaster ready (`AuthenticationCleartextPassword`, `'R'`).
            let (mut conn2, _) = listener.accept().await.unwrap();
            conn2.read_exact(&mut len_buf).await.unwrap();
            let rem2 = usize::try_from(i32::from_be_bytes(len_buf) - 4).unwrap();
            let mut body2 = vec![0u8; rem2];
            conn2.read_exact(&mut body2).await.unwrap();
            // 'R' + int32(8) + int32(3)
            let mut auth_frame = vec![b'R'];
            auth_frame.extend_from_slice(&8i32.to_be_bytes());
            auth_frame.extend_from_slice(&3i32.to_be_bytes());
            conn2.write_all(&auth_frame).await.unwrap();
        });

        assert!(
            !postgres_wire_ready(addr, "postgres", "app").await,
            "57P03 startup error must hold readiness false"
        );
        assert!(
            postgres_wire_ready(addr, "postgres", "app").await,
            "Authentication frame must mark PostgreSQL ready"
        );
        server.await.unwrap();
    }
}
