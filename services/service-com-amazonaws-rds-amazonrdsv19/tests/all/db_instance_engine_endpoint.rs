// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB instance ENGINE ENDPOINT: the `Endpoint{Address,Port}` a
//! DB instance publishes, and the PostgreSQL and MySQL sessions a client opens on it.
//!
//! The rule under test: for every engine whose native wire this emulator speaks, the
//! endpoint a DB instance publishes is a listener that SERVES that engine — a client
//! completes the engine's own handshake on it, authenticates as `MasterUsername` against
//! `DBName`, and runs SQL whose answers come from the instance's own database; TLS is
//! negotiated IN BAND on the same port; and whether a session in the clear is admitted at
//! all follows the instance's parameter group. An engine whose wire this emulator does not
//! speak publishes no listener and keeps the DNS-form endpoint it always had.
//!
//! Every client here is written against the wire itself (the startup and handshake
//! messages, the simple-query protocol, the MySQL command phase), so what these tests
//! drive is the protocol a stock driver speaks and not an in-process shortcut — and
//! everything runs in ONE process: the emulator's listeners are opened by the control
//! plane calls below, on the core's own loopback interface.
//!
//! The tables span what the rules generalise over: both wires, encrypted and plaintext
//! sessions, the parameter that decides whether a plaintext session is admitted in both
//! its states, the credential cases (right password, wrong password, another user), the
//! database cases (the created one, the engine's own default, one that does not exist),
//! and the lifecycle operations that end a session (reboot, stop, delete).

use std::sync::Arc;

use sha1::{Digest, Sha1};
use service_com_amazonaws_rds_amazonrdsv19::state::data_plane::wire_session::MAX_SESSION_SETTINGS;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::db_instances::{call, emulator, expect_ok, subtrees, text, wait_for_settle};

/// The master user every instance below is created with.
///
/// `pub(crate)` here and below so the sibling suites that drive this endpoint's own
/// surface — the statement classes it serves ([`crate::engine_statement_classes`]) and the
/// rows a snapshot restore brings back ([`crate::db_snapshot_restore`]) — open their
/// sessions with the same client rather than standing up a second one.
pub(crate) const MASTER_USER: &str = "emuadmin";

/// Its password. A test credential, and the only place in this suite it appears.
pub(crate) const MASTER_PASSWORD: &str = "emu-Secret-99";

/// The `DBName` every instance below is created with.
pub(crate) const DATABASE: &str = "appdb";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// Create a DB instance, wait for it to settle, and answer with the endpoint it
/// publishes.
pub(crate) async fn available_instance(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, String)],
) -> (String, u16) {
    let mut request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => identifier,
        "Engine" => engine,
        "DBInstanceClass" => "db.t3.micro",
        "AllocatedStorage" => 20,
        "MasterUsername" => MASTER_USER,
        "MasterUserPassword" => MASTER_PASSWORD,
        "DBName" => DATABASE,
    ]
    .to_vec();
    request.extend(extra.iter().cloned());
    let created = expect_ok(identifier, call(router, "CreateDBInstance", &request).await);
    let endpoint = endpoint_of(&created);
    wait_for_settle();
    let described = expect_ok(
        identifier,
        call(
            router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => identifier],
        )
        .await,
    );
    assert_eq!(
        text(&described, "DBInstanceStatus"),
        "available",
        "case {identifier:?}: the instance must settle to available before a client \
         connects, got {described}"
    );
    assert_eq!(
        endpoint_of(&described),
        endpoint,
        "case {identifier:?}: the endpoint the create published must be the endpoint the \
         describe reports"
    );
    endpoint
}

/// The `Endpoint` an envelope publishes, as a client reads it.
pub(crate) fn endpoint_of(envelope: &str) -> (String, u16) {
    let endpoint = subtrees(envelope, "Endpoint")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the envelope must carry an Endpoint: {envelope}"));
    let port = text(&endpoint, "Port");
    (
        text(&endpoint, "Address"),
        port.parse()
            .unwrap_or_else(|_| panic!("the endpoint must publish a port, got {port:?}")),
    )
}

/// A DB parameter group of `family` carrying one setting, as a client creates it.
pub(crate) async fn parameter_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    family: &str,
    parameter: &str,
    value: &str,
) {
    expect_ok(
        name,
        call(
            router,
            "CreateDBParameterGroup",
            &params![
                "DBParameterGroupName" => name,
                "DBParameterGroupFamily" => family,
                "Description" => "the engine endpoint tests' parameter group",
            ],
        )
        .await,
    );
    expect_ok(
        name,
        call(
            router,
            "ModifyDBParameterGroup",
            &params![
                "DBParameterGroupName" => name,
                "Parameters.Parameter.1.ParameterName" => parameter,
                "Parameters.Parameter.1.ParameterValue" => value,
                "Parameters.Parameter.1.ApplyMethod" => "immediate",
            ],
        )
        .await,
    );
}

// ---------------------------------------------------------------------------
// The PostgreSQL wire, as a client speaks it.
// ---------------------------------------------------------------------------

/// A connection to an engine endpoint, in the clear or upgraded to TLS.
pub(crate) trait Connection: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Connection for T {}

/// What a client holds: the stream, whatever it is now.
pub(crate) type Client = Box<dyn Connection>;

/// How a session is opened: in the clear, or with the wire's own in-band TLS upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    Plaintext,
    Tls,
}

/// A refusal a server sent: the code a client branches on and the sentence it reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) code: String,
    pub(crate) message: String,
}

/// One result set a client read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Rows {
    pub(crate) columns: Vec<String>,
    /// The type each column was described under, as the `RowDescription` carries it — the
    /// `pg_type` OID a driver picks its decoder by.
    pub(crate) type_oids: Vec<i32>,
    pub(crate) rows: Vec<Vec<Option<String>>>,
    /// What the server said about the statement without refusing it: the PostgreSQL
    /// `NoticeResponse` messages it sent before the `CommandComplete`.
    pub(crate) notices: Vec<Refusal>,
}

impl Rows {
    /// The one value a single-row, single-column answer carries.
    pub(crate) fn only(&self) -> Option<&str> {
        self.rows
            .first()
            .and_then(|row| row.first())
            .and_then(Option::as_deref)
    }
}

/// Open a TCP connection to an engine endpoint of THIS emulator.
///
/// The endpoint the control plane published is the core's own listener, which the core
/// binds on loopback (`EMULATOR_BIND_ADDRESS`, `127.0.0.1` by default), so the dial below
/// is to this machine — and the assertion is what pins it there: the address the control
/// plane published has to be that loopback address, and that is the address dialled.
async fn connect(address: &str, port: u16) -> Client {
    assert_eq!(
        address, "127.0.0.1",
        "the published endpoint must be this machine's loopback address, got {address:?}"
    );
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|error| {
            panic!("the published endpoint {address}:{port} must accept a connection, got {error}")
        });
    Box::new(stream)
}

/// Run the client side of a TLS handshake on `conn`.
///
/// The verifier accepts the certificate the EMULATOR'S OWN core generated for its
/// listeners, which is self-signed and names loopback: this client is the emulator's own
/// test, connecting to a port the same process opened, so there is no authority to check
/// it against.
async fn upgrade_to_tls(conn: Client) -> Client {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("the provider supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptTheEmulatorsCertificate(provider)))
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let server_name = rustls::pki_types::ServerName::try_from("localhost")
        .expect("localhost is a server name");
    let stream = connector
        .connect(server_name, conn)
        .await
        .expect("the server must complete the in-band TLS handshake");
    Box::new(stream)
}

/// See [`upgrade_to_tls`].
#[derive(Debug)]
struct AcceptTheEmulatorsCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptTheEmulatorsCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A PostgreSQL session a client has opened.
pub(crate) struct PostgresSession {
    conn: Client,
}

/// Open a PostgreSQL session on `address:port`, as libpq does: the optional `SSLRequest`
/// first, then the startup message, the password, and the parameters the server reports.
pub(crate) async fn postgres_login(
    address: &str,
    port: u16,
    transport: Transport,
    user: &str,
    password: &str,
    database: Option<&str>,
) -> Result<PostgresSession, Refusal> {
    let mut conn = connect(address, port).await;
    if transport == Transport::Tls {
        let mut request = Vec::new();
        request.extend_from_slice(&8i32.to_be_bytes());
        // The `SSLRequest` code, sent in the clear as the connection's first bytes.
        request.extend_from_slice(&80_877_103i32.to_be_bytes());
        conn.write_all(&request).await.expect("the request is sent");
        let mut answer = [0u8; 1];
        conn.read_exact(&mut answer)
            .await
            .expect("the server answers the SSLRequest");
        assert_eq!(
            answer[0], b'S',
            "the server must answer an SSLRequest with 'S' before any TLS"
        );
        conn = upgrade_to_tls(conn).await;
    }
    let mut body = Vec::new();
    // Protocol version 3.0.
    body.extend_from_slice(&196_608i32.to_be_bytes());
    put_cstring(&mut body, "user");
    put_cstring(&mut body, user);
    if let Some(database) = database {
        put_cstring(&mut body, "database");
        put_cstring(&mut body, database);
    }
    body.push(0);
    let mut startup = Vec::new();
    startup.extend_from_slice(
        &i32::try_from(body.len() + 4)
            .expect("the startup message fits")
            .to_be_bytes(),
    );
    startup.extend_from_slice(&body);
    conn.write_all(&startup).await.expect("the startup is sent");

    loop {
        let (kind, body) = postgres_read(&mut conn).await;
        match kind {
            // AuthenticationRequest.
            b'R' => {
                let code = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                assert_eq!(
                    code, 3,
                    "this endpoint asks for a cleartext password, got the authentication \
                     request {code}"
                );
                let mut message = Vec::new();
                put_cstring(&mut message, password);
                postgres_send(&mut conn, b'p', &message).await;
                // The next message is AuthenticationOk or the refusal.
                let (kind, body) = postgres_read(&mut conn).await;
                match kind {
                    b'R' => continue,
                    b'E' => return Err(postgres_refusal(&body)),
                    other => panic!("unexpected message '{}' after the password", other as char),
                }
            }
            b'E' => return Err(postgres_refusal(&body)),
            // ReadyForQuery: the session is open.
            b'Z' => return Ok(PostgresSession { conn }),
            // ParameterStatus, BackendKeyData, NoticeResponse.
            _ => {}
        }
    }
}

impl PostgresSession {
    /// Run one simple query and read everything the server answers for it.
    pub(crate) async fn query(&mut self, sql: &str) -> Result<Rows, Refusal> {
        let mut message = Vec::new();
        put_cstring(&mut message, sql);
        postgres_send(&mut self.conn, b'Q', &message).await;
        self.read_answer().await
    }

    /// Run one `COPY … FROM STDIN` and stream `data` for it, as a bulk-loading client
    /// does: the statement, then the `CopyInResponse` the server answers with, then the
    /// data in `CopyData` messages and a `CopyDone`.
    pub(crate) async fn copy_in(&mut self, sql: &str, data: &str) -> Result<Rows, Refusal> {
        let mut message = Vec::new();
        put_cstring(&mut message, sql);
        postgres_send(&mut self.conn, b'Q', &message).await;
        let (kind, body) = postgres_read(&mut self.conn).await;
        if kind == b'E' {
            let refusal = postgres_refusal(&body);
            // The server reported the statement, not the data: drain the cycle.
            let _ = self.read_answer().await;
            return Err(refusal);
        }
        assert_eq!(
            kind, b'G',
            "a COPY FROM STDIN must be answered with CopyInResponse, got {:?}",
            kind as char
        );
        postgres_send(&mut self.conn, b'd', data.as_bytes()).await;
        postgres_send(&mut self.conn, b'c', &[]).await;
        self.read_answer().await
    }

    /// Run one statement through the EXTENDED query protocol, with its parameter values
    /// bound — the exchange a driver uses for a parameterised statement: `Parse`, `Bind`,
    /// `Describe`, `Execute`, `Sync`.
    pub(crate) async fn query_with_params(&mut self, sql: &str, params: &[&str]) -> Result<Rows, Refusal> {
        let mut parse = Vec::new();
        put_cstring(&mut parse, "");
        put_cstring(&mut parse, sql);
        parse.extend_from_slice(&0i16.to_be_bytes());
        postgres_send(&mut self.conn, b'P', &parse).await;

        let mut bind = Vec::new();
        put_cstring(&mut bind, "");
        put_cstring(&mut bind, "");
        // No format codes: every value is text, which is what this endpoint reads.
        bind.extend_from_slice(&0i16.to_be_bytes());
        bind.extend_from_slice(
            &i16::try_from(params.len())
                .expect("the parameter count fits")
                .to_be_bytes(),
        );
        for value in params {
            bind.extend_from_slice(
                &i32::try_from(value.len())
                    .expect("the value length fits")
                    .to_be_bytes(),
            );
            bind.extend_from_slice(value.as_bytes());
        }
        bind.extend_from_slice(&0i16.to_be_bytes());
        postgres_send(&mut self.conn, b'B', &bind).await;

        let mut describe = vec![b'P'];
        put_cstring(&mut describe, "");
        postgres_send(&mut self.conn, b'D', &describe).await;

        let mut execute = Vec::new();
        put_cstring(&mut execute, "");
        execute.extend_from_slice(&0i32.to_be_bytes());
        postgres_send(&mut self.conn, b'E', &execute).await;
        postgres_send(&mut self.conn, b'S', &[]).await;
        self.read_answer().await
    }

    /// Read everything the server answers for one query cycle.
    async fn read_answer(&mut self) -> Result<Rows, Refusal> {
        let mut answer = Rows::default();
        let mut refusal = None;
        loop {
            let Some((kind, body)) = postgres_try_read(&mut self.conn).await else {
                // The server severed the session (a reboot, a stop): whatever it said
                // before closing is the answer, and a close with nothing said is itself
                // the end of the session.
                return Err(refusal.unwrap_or(Refusal {
                    code: "08006".to_string(),
                    message: "the server closed the connection".to_string(),
                }));
            };
            match kind {
                // RowDescription.
                b'T' => {
                    let count = usize::from(u16::from_be_bytes([body[0], body[1]]));
                    let mut at = 2usize;
                    answer.columns.clear();
                    answer.type_oids.clear();
                    for _ in 0..count {
                        let (name, next) = read_cstring(&body, at);
                        answer.columns.push(name);
                        // name, table oid, column number, type oid, size, modifier,
                        // format code.
                        answer.type_oids.push(i32::from_be_bytes([
                            body[next + 6],
                            body[next + 7],
                            body[next + 8],
                            body[next + 9],
                        ]));
                        at = next + 18;
                    }
                }
                // DataRow.
                b'D' => {
                    let count = usize::from(u16::from_be_bytes([body[0], body[1]]));
                    let mut at = 2usize;
                    let mut row = Vec::with_capacity(count);
                    for _ in 0..count {
                        let length =
                            i32::from_be_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
                        at += 4;
                        if length < 0 {
                            row.push(None);
                            continue;
                        }
                        let length = usize::try_from(length).expect("a value length fits");
                        row.push(Some(
                            String::from_utf8_lossy(&body[at..at + length]).into_owned(),
                        ));
                        at += length;
                    }
                    answer.rows.push(row);
                }
                b'E' => refusal = Some(postgres_refusal(&body)),
                // NoticeResponse: what the server has to say about the statement.
                b'N' => answer.notices.push(postgres_refusal(&body)),
                // ReadyForQuery ends the exchange.
                b'Z' => break,
                // CommandComplete, EmptyQueryResponse.
                _ => {}
            }
        }
        match refusal {
            Some(refusal) => Err(refusal),
            None => Ok(answer),
        }
    }
}

/// Write one PostgreSQL message.
async fn postgres_send(conn: &mut Client, kind: u8, body: &[u8]) {
    let mut message = vec![kind];
    message.extend_from_slice(
        &i32::try_from(body.len() + 4)
            .expect("the message fits")
            .to_be_bytes(),
    );
    message.extend_from_slice(body);
    conn.write_all(&message).await.expect("the message is sent");
}

/// Read one PostgreSQL message, panicking where the server should have answered.
async fn postgres_read(conn: &mut Client) -> (u8, Vec<u8>) {
    postgres_try_read(conn)
        .await
        .expect("the server answers")
}

/// Read one PostgreSQL message, or `None` where the server closed the connection.
async fn postgres_try_read(conn: &mut Client) -> Option<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    conn.read_exact(&mut header).await.ok()?;
    let length = i32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    let body_length = usize::try_from(length - 4).expect("a message length fits");
    let mut body = vec![0u8; body_length];
    conn.read_exact(&mut body).await.ok()?;
    Some((header[0], body))
}

/// The SQLSTATE and the message an `ErrorResponse` carries.
fn postgres_refusal(body: &[u8]) -> Refusal {
    let mut refusal = Refusal {
        code: String::new(),
        message: String::new(),
    };
    let mut at = 0usize;
    while at < body.len() && body[at] != 0 {
        let field = body[at];
        let (value, next) = read_cstring(body, at + 1);
        match field {
            b'C' => refusal.code = value,
            b'M' => refusal.message = value,
            _ => {}
        }
        at = next;
    }
    refusal
}

/// Append a NUL-terminated string.
fn put_cstring(body: &mut Vec<u8>, text: &str) {
    body.extend_from_slice(text.as_bytes());
    body.push(0);
}

/// The NUL-terminated string at `at`, and the index after it.
fn read_cstring(body: &[u8], at: usize) -> (String, usize) {
    let rest = &body[at.min(body.len())..];
    let end = rest.iter().position(|byte| *byte == 0).unwrap_or(rest.len());
    (
        String::from_utf8_lossy(&rest[..end]).into_owned(),
        at + end + 1,
    )
}

// ---------------------------------------------------------------------------
// The MySQL wire, as a client speaks it.
// ---------------------------------------------------------------------------

/// The capability flags this client sets.
const CLIENT_CONNECT_WITH_DB: u32 = 0x0000_0008;
const CLIENT_PROTOCOL_41: u32 = 0x0000_0200;
const CLIENT_SSL: u32 = 0x0000_0800;
const CLIENT_SECURE_CONNECTION: u32 = 0x0000_8000;
const CLIENT_PLUGIN_AUTH: u32 = 0x0008_0000;
const CLIENT_DEPRECATE_EOF: u32 = 0x0100_0000;

/// A MySQL session a client has opened, and the version the greeting named.
pub(crate) struct MySqlSession {
    conn: Client,
    pub(crate) server_version: String,
}

/// Open a MySQL session on `address:port`, as libmysqlclient does: read the greeting,
/// answer it (with the TLS upgrade in the middle when asked for), and read the server's
/// verdict.
pub(crate) async fn mysql_login(
    address: &str,
    port: u16,
    transport: Transport,
    user: &str,
    password: &str,
    database: Option<&str>,
) -> Result<MySqlSession, Refusal> {
    let mut conn = connect(address, port).await;
    let (_, greeting) = mysql_read(&mut conn).await;
    assert_eq!(
        greeting[0], 10,
        "the greeting must announce protocol version 10"
    );
    let (server_version, at) = read_cstring(&greeting, 1);
    // connection id (4), the first 8 nonce bytes, a filler, the lower capability flags,
    // the charset, the status flags, the upper capability flags, the nonce length, 10
    // reserved bytes, then the remaining nonce bytes.
    let mut nonce = greeting[at + 4..at + 12].to_vec();
    nonce.extend_from_slice(&greeting[at + 31..at + 43]);
    let server_capabilities = u32::from(u16::from_be_bytes([
        greeting[at + 14],
        greeting[at + 13],
    ])) | (u32::from(u16::from_be_bytes([
        greeting[at + 20],
        greeting[at + 19],
    ])) << 16);
    assert!(
        server_capabilities & CLIENT_SSL != 0,
        "the greeting must offer CLIENT_SSL, so a client can negotiate TLS in band"
    );

    let mut capabilities = CLIENT_PROTOCOL_41
        | CLIENT_SECURE_CONNECTION
        | CLIENT_PLUGIN_AUTH
        | CLIENT_DEPRECATE_EOF;
    if database.is_some() {
        capabilities |= CLIENT_CONNECT_WITH_DB;
    }
    if transport == Transport::Tls {
        capabilities |= CLIENT_SSL;
        // The 32-byte handshake response that asks for TLS and nothing else.
        let mut request = Vec::new();
        request.extend_from_slice(&capabilities.to_le_bytes());
        request.extend_from_slice(&(16u32 * 1024 * 1024).to_le_bytes());
        request.push(255);
        request.extend_from_slice(&[0u8; 23]);
        mysql_write(&mut conn, 1, &request).await;
        conn = upgrade_to_tls(conn).await;
    }

    let mut response = Vec::new();
    response.extend_from_slice(&capabilities.to_le_bytes());
    response.extend_from_slice(&(16u32 * 1024 * 1024).to_le_bytes());
    response.push(255);
    response.extend_from_slice(&[0u8; 23]);
    put_cstring(&mut response, user);
    let proof = mysql_native_proof(password, &nonce);
    response.push(u8::try_from(proof.len()).expect("the proof is 20 bytes"));
    response.extend_from_slice(&proof);
    if let Some(database) = database {
        put_cstring(&mut response, database);
    }
    put_cstring(&mut response, "mysql_native_password");
    let sequence = if transport == Transport::Tls { 2 } else { 1 };
    mysql_write(&mut conn, sequence, &response).await;

    let (_, verdict) = mysql_read(&mut conn).await;
    match verdict[0] {
        0x00 | 0xfe => Ok(MySqlSession {
            conn,
            server_version,
        }),
        0xff => Err(mysql_refusal(&verdict)),
        other => panic!("unexpected answer {other:#x} to the handshake response"),
    }
}

impl MySqlSession {
    /// Run one statement through `COM_QUERY` and read everything the server answers.
    pub(crate) async fn query(&mut self, sql: &str) -> Result<Rows, Refusal> {
        let mut command = vec![0x03u8];
        command.extend_from_slice(sql.as_bytes());
        mysql_write(&mut self.conn, 0, &command).await;
        let (_, first) = mysql_read(&mut self.conn).await;
        match first[0] {
            0x00 => return Ok(Rows::default()),
            0xff => return Err(mysql_refusal(&first)),
            _ => {}
        }
        let (columns, _) = read_length_encoded(&first, 0);
        let mut answer = Rows::default();
        for _ in 0..columns {
            let (_, definition) = mysql_read(&mut self.conn).await;
            // catalog, schema, table, org_table, name: the name is the fifth
            // length-encoded string.
            let mut at = 0usize;
            let mut name = String::new();
            for field in 0..5 {
                let (value, next) = read_length_encoded_string(&definition, at);
                at = next;
                if field == 4 {
                    name = value;
                }
            }
            answer.columns.push(name);
        }
        loop {
            let (_, packet) = mysql_read(&mut self.conn).await;
            // The packet that ends a result set: an OK or EOF marker, never a row, which
            // always begins with a length-encoded value.
            if packet[0] == 0xfe && packet.len() < 9 {
                break;
            }
            if packet[0] == 0xff {
                return Err(mysql_refusal(&packet));
            }
            let mut row = Vec::new();
            let mut at = 0usize;
            while at < packet.len() {
                if packet[at] == 0xfb {
                    row.push(None);
                    at += 1;
                    continue;
                }
                let (value, next) = read_length_encoded_string(&packet, at);
                row.push(Some(value));
                at = next;
            }
            answer.rows.push(row);
        }
        Ok(answer)
    }
}

/// `SHA1(password) XOR SHA1(nonce || SHA1(SHA1(password)))` — the `mysql_native_password`
/// proof a client answers the greeting's nonce with.
fn mysql_native_proof(password: &str, nonce: &[u8]) -> Vec<u8> {
    let first = Sha1::digest(password.as_bytes());
    let stored = Sha1::digest(first);
    let mut hasher = Sha1::new();
    hasher.update(nonce);
    hasher.update(stored);
    let mask = hasher.finalize();
    first
        .iter()
        .zip(mask.iter())
        .map(|(value, mask)| value ^ mask)
        .collect()
}

/// Write one MySQL packet.
async fn mysql_write(conn: &mut Client, sequence: u8, payload: &[u8]) {
    let length = u32::try_from(payload.len()).expect("the packet fits");
    let mut packet = length.to_le_bytes()[..3].to_vec();
    packet.push(sequence);
    packet.extend_from_slice(payload);
    conn.write_all(&packet).await.expect("the packet is sent");
}

/// Read one MySQL packet.
async fn mysql_read(conn: &mut Client) -> (u8, Vec<u8>) {
    let mut header = [0u8; 4];
    conn.read_exact(&mut header)
        .await
        .expect("the server answers");
    let length = usize::from(header[0])
        | (usize::from(header[1]) << 8)
        | (usize::from(header[2]) << 16);
    let mut payload = vec![0u8; length];
    conn.read_exact(&mut payload)
        .await
        .expect("the server sends the whole packet");
    (header[3], payload)
}

/// The error number and the message an `ERR` packet carries.
fn mysql_refusal(payload: &[u8]) -> Refusal {
    let number = u16::from_le_bytes([payload[1], payload[2]]);
    // `#` then the five-character SQLSTATE, then the message.
    let message = String::from_utf8_lossy(&payload[9..]).into_owned();
    Refusal {
        code: number.to_string(),
        message,
    }
}

/// A length-encoded integer and the index after it.
fn read_length_encoded(payload: &[u8], at: usize) -> (u64, usize) {
    match payload[at] {
        0xfc => (
            u64::from(u16::from_le_bytes([payload[at + 1], payload[at + 2]])),
            at + 3,
        ),
        0xfd => (
            u64::from(u32::from_le_bytes([
                payload[at + 1],
                payload[at + 2],
                payload[at + 3],
                0,
            ])),
            at + 4,
        ),
        0xfe => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&payload[at + 1..at + 9]);
            (u64::from_le_bytes(bytes), at + 9)
        }
        value => (u64::from(value), at + 1),
    }
}

/// A length-encoded string and the index after it.
fn read_length_encoded_string(payload: &[u8], at: usize) -> (String, usize) {
    let (length, next) = read_length_encoded(payload, at);
    let length = usize::try_from(length).expect("a string length fits");
    (
        String::from_utf8_lossy(&payload[next..next + length]).into_owned(),
        next + length,
    )
}

// ---------------------------------------------------------------------------
// The tests.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_postgres_endpoint_serves_a_tls_session_its_identity_and_its_sql() {
    let router = emulator();
    let (address, port) = available_instance(&router, "emu-pg-wire", "postgres", &[]).await;
    let mut session = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted over TLS");

    // The session's own identity and shape, which a client reads from the server.
    let identity = session
        .query("SELECT current_user, current_database()")
        .await
        .expect("the identity query runs");
    assert_eq!(
        identity.rows,
        vec![vec![
            Some(MASTER_USER.to_string()),
            Some(DATABASE.to_string())
        ]],
        "the session must report the master user and the created database, got {identity:?}"
    );
    let ssl = session.query("SHOW ssl").await.expect("SHOW ssl runs");
    assert_eq!(
        ssl.only(),
        Some("on"),
        "the server must report that it accepts TLS, got {ssl:?}"
    );
    // The parameter that decides whether a session in the clear is admitted, as the
    // instance's parameter group leaves it: a namespaced setting, read back under its own
    // name.
    let force_ssl = session
        .query("SHOW rds.force_ssl")
        .await
        .expect("SHOW rds.force_ssl runs");
    assert_eq!(
        force_ssl.only(),
        Some("1"),
        "a PostgreSQL 15-or-later instance under the default parameter group forces TLS, \
         got {force_ssl:?}"
    );
    let unknown = session
        .query("SHOW not_a_setting")
        .await
        .expect_err("a setting this server does not have must be refused");
    assert_eq!(
        (unknown.code.as_str(), unknown.message.as_str()),
        (
            "42P01",
            "unrecognized configuration parameter \"not_a_setting\""
        ),
        "the refusal must be the server's own, got {unknown:?}"
    );

    // The session's own TLS, as `pg_stat_ssl` reports it for this backend.
    let session_tls = session
        .query("SELECT ssl, version FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
        .await
        .expect("the pg_stat_ssl query runs");
    assert_eq!(
        session_tls.rows.len(),
        1,
        "pg_stat_ssl must carry exactly this session's row, got {session_tls:?}"
    );
    let row = &session_tls.rows[0];
    assert_eq!(
        row[0].as_deref(),
        Some("t"),
        "pg_stat_ssl must report the session as encrypted, got {session_tls:?}"
    );
    assert!(
        matches!(row[1].as_deref(), Some("TLSv1.2" | "TLSv1.3")),
        "pg_stat_ssl must report the protocol the handshake negotiated, got {session_tls:?}"
    );

    // Arithmetic the ENGINE performs, and a round trip through the instance's own
    // database: a table created, a row written, the row read back.
    let answer = session.query("SELECT 6 * 7").await.expect("the query runs");
    assert_eq!(
        answer.only(),
        Some("42"),
        "the engine must compute the answer, got {answer:?}"
    );
    for statement in [
        "CREATE TABLE widgets (id integer PRIMARY KEY, name text NOT NULL, live boolean)",
        "INSERT INTO widgets (id, name, live) VALUES (1, 'first', true)",
        "INSERT INTO widgets (id, name, live) VALUES (2, 'second', false)",
    ] {
        session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("{statement:?} must run, got {refusal:?}"));
    }
    let widgets = session
        .query("SELECT id, name, live FROM widgets ORDER BY id")
        .await
        .expect("the read-back runs");    assert_eq!(
        widgets.rows,
        vec![
            vec![
                Some("1".to_string()),
                Some("first".to_string()),
                Some("t".to_string())
            ],
            vec![
                Some("2".to_string()),
                Some("second".to_string()),
                Some("f".to_string())
            ],
        ],
        "the rows read back must be the rows written, got {widgets:?}"
    );
    assert_eq!(
        widgets.columns,
        vec![
            "id".to_string(),
            "name".to_string(),
            "live".to_string()
        ],
        "the result set must name its columns, got {widgets:?}"
    );

    // The same database through the EXTENDED query protocol, with the values bound rather
    // than written into the statement — the exchange a driver uses for a parameterised
    // statement.
    let written = session
        .query_with_params(
            "INSERT INTO widgets (id, name, live) VALUES ($1, $2, $3)",
            &["3", "third", "1"],
        )
        .await
        .expect("the parameterised insert runs");
    assert!(
        written.rows.is_empty(),
        "an insert returns no rows of its own, got {written:?}"
    );
    let bound = session
        .query_with_params("SELECT name FROM widgets WHERE id = $1", &["3"])
        .await
        .expect("the parameterised read-back runs");
    assert_eq!(
        bound.rows,
        vec![vec![Some("third".to_string())]],
        "the bound parameter must select the row it names, got {bound:?}"
    );

    // A session setting a client sets for itself round-trips, and what one session may
    // hold is bounded: past the cap a further `SET` is refused rather than growing the
    // session's state.
    session
        .query("SET my.marker TO 'kept'")
        .await
        .expect("a namespaced session setting is one the server takes");
    let marker = session
        .query("SHOW my.marker")
        .await
        .expect("SHOW reads it back");
    assert_eq!(
        marker.only(),
        Some("kept"),
        "a session setting must read back as the session set it, got {marker:?}"
    );
    let mut refused = None;
    for index in 0..=MAX_SESSION_SETTINGS {
        if let Err(refusal) = session
            .query(&format!("SET my.setting{index} TO 'x'"))
            .await
        {
            refused = Some((index, refusal));
            break;
        }
    }
    let (index, refusal) = refused.expect("the session's settings must be bounded");
    assert_eq!(
        refusal.code, "54000",
        "the {index}th setting must be refused as past this endpoint's limit, got {refusal:?}"
    );

    // The engine is CONFINED: a statement that would reach outside the instance's own
    // in-memory database is refused by the engine itself — a second database, the engine's
    // configuration, its code loader, a host file — and the session goes on serving after
    // each refusal.
    for statement in [
        "ATTACH DATABASE 'widgets.db' AS other",
        "PRAGMA temp_store_directory = 'somewhere'",
        "SELECT load_extension('extension')",
        "SELECT readfile('a-file')",
        "SELECT writefile('a-file', 'content')",
        "CREATE VIRTUAL TABLE pages USING dbstat",
    ] {
        let refusal = session.query(statement).await;
        assert!(
            refusal.is_err(),
            "{statement:?} must be refused by the engine, got {refusal:?}"
        );
        let still_serving = session
            .query("SELECT count(*) FROM widgets")
            .await
            .unwrap_or_else(|error| {
                panic!("the session must serve after refusing {statement:?}, got {error:?}")
            });
        assert_eq!(
            still_serving.only(),
            Some("3"),
            "the instance's own rows must be untouched after refusing {statement:?}, got \
             {still_serving:?}"
        );
    }

    // The refusals the engine itself answers, each with the code a client branches on.
    let duplicate = session
        .query("INSERT INTO widgets (id, name) VALUES (1, 'again')")
        .await
        .expect_err("a duplicate key must be refused");
    assert_eq!(
        duplicate.code, "23505",
        "a unique violation must carry its SQLSTATE, got {duplicate:?}"
    );
    let missing = session
        .query("SELECT * FROM nothing_here")
        .await
        .expect_err("a table that does not exist must be refused");
    assert_eq!(
        missing.code, "42P01",
        "an undefined table must carry its SQLSTATE, got {missing:?}"
    );
    // Amazon RDS does not serve `ALTER SYSTEM` on an RDS for PostgreSQL instance at all:
    // the parameters are set through the instance's DB parameter group.
    let privileged = session
        .query("ALTER SYSTEM SET work_mem = '8MB'")
        .await
        .expect_err("ALTER SYSTEM must be refused");
    assert_eq!(
        (privileged.code.as_str(), privileged.message.as_str()),
        ("42501", "ALTER SYSTEM command is not supported"),
        "the privilege refusal must be the server's own, got {privileged:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mysql_endpoint_serves_its_version_its_sql_and_its_privilege_refusals() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-my-wire",
        "mysql",
        &params!["EngineVersion" => "8.0.39"],
    )
    .await;
    // A plaintext login is admitted by default: `require_secure_transport` is OFF unless
    // a parameter group turns it on.
    let mut session = mysql_login(
        &address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted in the clear by default");
    assert_eq!(
        session.server_version, "8.0.39",
        "the greeting must name the version the instance was created with"
    );
    let version = session
        .query("SELECT VERSION()")
        .await
        .expect("the version query runs");
    assert_eq!(
        version.only(),
        Some("8.0.39"),
        "the engine must report the instance's own engine version, got {version:?}"
    );
    let identity = session
        .query("SELECT current_user, database()")
        .await
        .expect("the identity query runs");
    assert_eq!(
        identity.rows,
        vec![vec![
            // A MySQL session's identity is the ACCOUNT whose privileges it holds, which
            // for the RDS master user is `'<master>'@'%'`.
            Some(format!("{MASTER_USER}@%")),
            Some(DATABASE.to_string())
        ]],
        "the session must report the master user's account and the created database, got {identity:?}"
    );

    // A round trip through the instance's own database.
    for statement in [
        "CREATE TABLE gauges (id int PRIMARY KEY, label varchar(32))",
        "INSERT INTO gauges (id, label) VALUES (7, 'pressure')",
    ] {
        session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("{statement:?} must run, got {refusal:?}"));
    }
    let gauges = session
        .query("SELECT id, label FROM gauges")
        .await
        .expect("the read-back runs");
    assert_eq!(
        gauges.rows,
        vec![vec![Some("7".to_string()), Some("pressure".to_string())]],
        "the row read back must be the row written, got {gauges:?}"
    );

    // A session in the clear reports no cipher, and the refusals are MySQL's own.
    let cipher = session
        .query("SHOW STATUS LIKE 'Ssl_cipher'")
        .await
        .expect("the status query runs");
    assert_eq!(
        cipher.rows,
        vec![vec![
            Some("Ssl_cipher".to_string()),
            Some(String::new())
        ]],
        "a session in the clear must report no cipher, got {cipher:?}"
    );
    // The variable that decides whether a session in the clear is admitted, as the
    // instance's parameter group leaves it.
    let secure_transport = session
        .query("SHOW VARIABLES LIKE 'require_secure_transport'")
        .await
        .expect("the variables query runs");
    assert_eq!(
        secure_transport.rows,
        vec![vec![
            Some("require_secure_transport".to_string()),
            Some("OFF".to_string())
        ]],
        "the default parameter group leaves it off, got {secure_transport:?}"
    );
    // A database the instance has, and one it does not.
    session
        .query("USE information_schema")
        .await
        .expect("a system schema is one the instance has");
    let unknown_database = session
        .query("USE not_a_database")
        .await
        .expect_err("a database the instance does not have must be refused");
    assert_eq!(
        (unknown_database.code.as_str(), unknown_database.message.as_str()),
        ("1049", "Unknown database 'not_a_database'"),
        "the refusal must be the server's own, got {unknown_database:?}"
    );
    // The RDS master user holds no SUPER privilege, so a global variable is not its to
    // set: a parameter group is how it is changed.
    let privileged = session
        .query("SET GLOBAL max_connections = 10")
        .await
        .expect_err("SET GLOBAL must be refused");
    assert_eq!(
        (privileged.code.as_str(), privileged.message.as_str()),
        (
            "1227",
            "Access denied; you need (at least one of) the SUPER or SYSTEM_VARIABLES_ADMIN \
             privilege(s) for this operation"
        ),
        "the privilege refusal must be the server's own, got {privileged:?}"
    );
    let missing = session
        .query("SELECT * FROM nothing_here")
        .await
        .expect_err("a table that does not exist must be refused");
    assert_eq!(
        missing.code, "1146",
        "an undefined table must carry its error number, got {missing:?}"
    );

    // The same instance over TLS reports the cipher the handshake negotiated.
    let mut encrypted = mysql_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted over TLS");
    let cipher = encrypted
        .query("SHOW SESSION STATUS LIKE 'Ssl_%'")
        .await
        .expect("the status query runs");
    let reported: Vec<(String, String)> = cipher
        .rows
        .iter()
        .map(|row| {
            (
                row[0].clone().unwrap_or_default(),
                row[1].clone().unwrap_or_default(),
            )
        })
        .collect();
    assert!(
        reported
            .iter()
            .any(|(name, value)| name == "Ssl_cipher" && !value.is_empty()),
        "a TLS session must report the cipher it negotiated, got {reported:?}"
    );
    assert!(
        reported.iter().any(|(name, value)| name == "Ssl_version"
            && (value == "TLSv1.2" || value == "TLSv1.3")),
        "a TLS session must report the protocol it negotiated, got {reported:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whether_a_plaintext_login_is_admitted_follows_the_instances_parameter_group() {
    let router = emulator();
    // The parameter each engine carries the requirement as, in both its states.
    parameter_group(
        &router,
        "emu-pg-no-force",
        "postgres17",
        "rds.force_ssl",
        "0",
    )
    .await;
    parameter_group(
        &router,
        "emu-my-force",
        "mysql8.0",
        "require_secure_transport",
        "ON",
    )
    .await;

    // Each row: the engine, the parameter group the instance uses, how the session is
    // opened, and what the server answers.
    struct Case {
        case: &'static str,
        identifier: &'static str,
        engine: &'static str,
        version: &'static str,
        parameter_group: Option<&'static str>,
        transport: Transport,
        /// `Some(code)` is the refusal the server must answer with.
        refused_with: Option<&'static str>,
    }
    let cases = [
        Case {
            case: "postgres under the default parameter group refuses a plaintext login",
            identifier: "emu-pg-force-default",
            engine: "postgres",
            version: "17.2",
            parameter_group: None,
            transport: Transport::Plaintext,
            refused_with: Some("28000"),
        },
        Case {
            case: "postgres with rds.force_ssl off admits one",
            identifier: "emu-pg-force-off",
            engine: "postgres",
            version: "17.2",
            parameter_group: Some("emu-pg-no-force"),
            transport: Transport::Plaintext,
            refused_with: None,
        },
        Case {
            case: "postgres 14, where the parameter's default is off, admits one",
            identifier: "emu-pg-14",
            engine: "postgres",
            version: "14.13",
            parameter_group: None,
            transport: Transport::Plaintext,
            refused_with: None,
        },
        Case {
            case: "postgres under the default parameter group admits a TLS session",
            identifier: "emu-pg-force-tls",
            engine: "postgres",
            version: "17.2",
            parameter_group: None,
            transport: Transport::Tls,
            refused_with: None,
        },
        Case {
            case: "mysql with require_secure_transport on refuses a plaintext login",
            identifier: "emu-my-force-on",
            engine: "mysql",
            version: "8.0.39",
            parameter_group: Some("emu-my-force"),
            transport: Transport::Plaintext,
            refused_with: Some("3159"),
        },
        Case {
            case: "mysql with require_secure_transport on admits a TLS session",
            identifier: "emu-my-force-tls",
            engine: "mysql",
            version: "8.0.39",
            parameter_group: Some("emu-my-force"),
            transport: Transport::Tls,
            refused_with: None,
        },
        Case {
            case: "mysql under the default parameter group admits a plaintext login",
            identifier: "emu-my-default",
            engine: "mysql",
            version: "8.0.39",
            parameter_group: None,
            transport: Transport::Plaintext,
            refused_with: None,
        },
    ];

    for row in cases {
        let mut extra: Vec<(&str, String)> = params!["EngineVersion" => row.version].to_vec();
        if let Some(group) = row.parameter_group {
            extra.push(("DBParameterGroupName", group.to_string()));
        }
        let (address, port) =
            available_instance(&router, row.identifier, row.engine, &extra).await;
        let outcome: Result<(), Refusal> = match row.engine {
            "postgres" => postgres_login(
                &address,
                port,
                row.transport,
                MASTER_USER,
                MASTER_PASSWORD,
                Some(DATABASE),
            )
            .await
            .map(|_| ()),
            _ => mysql_login(
                &address,
                port,
                row.transport,
                MASTER_USER,
                MASTER_PASSWORD,
                Some(DATABASE),
            )
            .await
            .map(|_| ()),
        };
        match (row.refused_with, &outcome) {
            (None, Ok(())) => {}
            (Some(code), Err(refusal)) => assert_eq!(
                refusal.code, code,
                "case {:?}: expected the refusal {code}, got {refusal:?}",
                row.case
            ),
            (expected, actual) => panic!(
                "case {:?}: expected {expected:?}, got {actual:?}",
                row.case
            ),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_endpoint_admits_only_the_master_user_with_its_password_and_a_database_it_has() {
    let router = emulator();
    let (pg_address, pg_port) =
        available_instance(&router, "emu-pg-auth", "postgres", &[]).await;
    let (my_address, my_port) = available_instance(
        &router,
        "emu-my-auth",
        "mysql",
        &params!["EngineVersion" => "8.0.39"],
    )
    .await;

    // Each row: who logs in, with what password, naming which database, and what the
    // server answers. The rows span the credential and database cases of both wires.
    struct Case {
        case: &'static str,
        user: &'static str,
        password: &'static str,
        database: Option<&'static str>,
        /// The PostgreSQL SQLSTATE and the MySQL error number of the refusal, or `None`
        /// where the login is admitted.
        postgres_refusal: Option<&'static str>,
        mysql_refusal: Option<&'static str>,
    }
    let cases = [
        Case {
            case: "the master user, its password, the created database",
            user: MASTER_USER,
            password: MASTER_PASSWORD,
            database: Some(DATABASE),
            postgres_refusal: None,
            mysql_refusal: None,
        },
        Case {
            case: "the master user naming no database at all",
            user: MASTER_USER,
            password: MASTER_PASSWORD,
            database: None,
            postgres_refusal: None,
            mysql_refusal: None,
        },
        Case {
            case: "the master user with the wrong password",
            user: MASTER_USER,
            password: "not-the-password",
            database: Some(DATABASE),
            postgres_refusal: Some("28P01"),
            mysql_refusal: Some("1045"),
        },
        Case {
            case: "the master user with no password",
            user: MASTER_USER,
            password: "",
            database: Some(DATABASE),
            postgres_refusal: Some("28P01"),
            mysql_refusal: Some("1045"),
        },
        Case {
            case: "another user with the master user's password",
            user: "someone-else",
            password: MASTER_PASSWORD,
            database: Some(DATABASE),
            postgres_refusal: Some("28P01"),
            mysql_refusal: Some("1045"),
        },
        Case {
            case: "the master user naming a database the instance does not have",
            user: MASTER_USER,
            password: MASTER_PASSWORD,
            database: Some("not-a-database"),
            postgres_refusal: Some("3D000"),
            mysql_refusal: Some("1049"),
        },
        Case {
            case: "the master user naming the engine's own default database",
            user: MASTER_USER,
            password: MASTER_PASSWORD,
            database: Some("postgres"),
            postgres_refusal: None,
            // A MySQL instance has no `postgres` schema.
            mysql_refusal: Some("1049"),
        },
    ];

    for row in cases {
        let postgres = postgres_login(
            &pg_address,
            pg_port,
            Transport::Tls,
            row.user,
            row.password,
            row.database,
        )
        .await
        .map(|_| ());
        match (row.postgres_refusal, &postgres) {
            (None, Ok(())) => {}
            (Some(code), Err(refusal)) => assert_eq!(
                refusal.code, code,
                "case {:?}: postgres must answer {code}, got {refusal:?}",
                row.case
            ),
            (expected, actual) => panic!(
                "case {:?}: postgres expected {expected:?}, got {actual:?}",
                row.case
            ),
        }
        let mysql = mysql_login(
            &my_address,
            my_port,
            Transport::Plaintext,
            row.user,
            row.password,
            row.database,
        )
        .await
        .map(|_| ());
        match (row.mysql_refusal, &mysql) {
            (None, Ok(())) => {}
            (Some(code), Err(refusal)) => assert_eq!(
                refusal.code, code,
                "case {:?}: mysql must answer {code}, got {refusal:?}",
                row.case
            ),
            (expected, actual) => panic!(
                "case {:?}: mysql expected {expected:?}, got {actual:?}",
                row.case
            ),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lifecycle_of_an_instance_is_the_lifecycle_of_its_sessions() {
    let router = emulator();
    let (address, port) = available_instance(&router, "emu-pg-life", "postgres", &[]).await;
    let mut session = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted");
    for statement in [
        "CREATE TABLE ledger (entry text)",
        "INSERT INTO ledger (entry) VALUES ('committed')",
        "BEGIN",
        "INSERT INTO ledger (entry) VALUES ('uncommitted')",
    ] {
        session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("{statement:?} must run, got {refusal:?}"));
    }

    // A reboot restarts the database engine: the session does not survive it, and the
    // work it had not committed is rolled back with it. The endpoint does not move.
    expect_ok(
        "reboot",
        call(
            &router,
            "RebootDBInstance",
            &params!["DBInstanceIdentifier" => "emu-pg-life"],
        )
        .await,
    );
    let severed = session.query("SELECT 1").await;
    assert!(
        severed.is_err(),
        "a reboot must end the sessions the engine held, got {severed:?}"
    );
    wait_for_settle();

    let described = expect_ok(
        "after the reboot",
        call(
            &router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => "emu-pg-life"],
        )
        .await,
    );
    assert_eq!(
        endpoint_of(&described),
        (address.clone(), port),
        "a reboot must leave the instance on the endpoint it published"
    );
    let mut session = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the endpoint must serve a session again after the reboot");
    let ledger = session
        .query("SELECT entry FROM ledger ORDER BY entry")
        .await
        .expect("the read-back runs");
    assert_eq!(
        ledger.rows,
        vec![vec![Some("committed".to_string())]],
        "the committed row must survive the reboot and the uncommitted one must not, got \
         {ledger:?}"
    );

    // A stopped instance serves no session, and starting it again keeps its rows.
    expect_ok(
        "stop",
        call(
            &router,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => "emu-pg-life"],
        )
        .await,
    );
    wait_for_settle();
    let stopped = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await;
    match stopped {
        Err(refusal) => assert_eq!(
            refusal.code, "57P03",
            "a stopped instance must refuse the login as an engine that is not running, \
             got {refusal:?}"
        ),
        Ok(_) => panic!("a stopped instance must serve no session"),
    }
    expect_ok(
        "start",
        call(
            &router,
            "StartDBInstance",
            &params!["DBInstanceIdentifier" => "emu-pg-life"],
        )
        .await,
    );
    wait_for_settle();
    let mut session = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("a started instance must serve sessions again");
    let ledger = session
        .query("SELECT entry FROM ledger")
        .await
        .expect("the read-back runs");
    assert_eq!(
        ledger.rows,
        vec![vec![Some("committed".to_string())]],
        "a stop and a start must keep the instance's rows, got {ledger:?}"
    );

    // A delete closes the port: the listener is the instance's, and it goes with it.
    expect_ok(
        "delete",
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-pg-life",
                "SkipFinalSnapshot" => true,
            ],
        )
        .await,
    );
    wait_for_settle();
    wait_for_settle();
    // A client polls the control plane until the instance is gone, which is what
    // materialises the removal the delete owes; the listener is the instance's, so the
    // port closes with the record.
    let (status, body) = call(
        &router,
        "DescribeDBInstances",
        &params!["DBInstanceIdentifier" => "emu-pg-life"],
    )
    .await;
    assert_eq!(
        status, 404,
        "the deleted instance must stop being reported, got {body}"
    );
    // The endpoint this instance published, which the core bound on loopback.
    assert_eq!(
        address, "127.0.0.1",
        "the published endpoint must be this machine's loopback address, got {address:?}"
    );
    let connected = TcpStream::connect(("127.0.0.1", port)).await;
    assert!(
        connected.is_err(),
        "a deleted instance's endpoint must stop answering, got {connected:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_requested_port_is_recorded_while_the_published_port_is_the_listener_s() {
    let router = emulator();
    // `Port` names the port the instance is CONFIGURED to listen on. The core owns the
    // port range its listeners are bound in (`vendor/server/src/listeners.rs`
    // `tcp_plaintext` takes no requested port), so what the instance PUBLISHES is the
    // port that listener was assigned — the one a client can reach — while the request's
    // own value is kept on the record and reported by `DbInstancePort` for a cluster
    // member. The create is accepted either way, and the published endpoint answers.
    let (address, port) = available_instance(
        &router,
        "emu-pg-custom-port",
        "postgres",
        &params!["Port" => 5433],
    )
    .await;
    assert_ne!(
        port, 0,
        "the instance must publish the port its engine listener is bound to"
    );
    let session = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await;
    assert!(
        session.is_ok(),
        "the published port must be the port the engine answers on, got {:?}",
        session.err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engine_this_emulator_serves_no_wire_for_publishes_no_listener() {
    let router = emulator();
    // The rows span the engine families this emulator runs no engine for, which keep the
    // DNS-form endpoint and the engine's documented default port.
    let cases = [
        ("oracle-se2", "1521"),
        ("sqlserver-ex", "1433"),
        ("db2-se", "50000"),
        ("aurora-postgresql", "5432"),
    ];
    for (engine, port) in cases {
        let identifier = format!("emu-unserved-{}", engine.replace(['-', '.'], ""));
        let created = expect_ok(
            engine,
            call(
                &router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => identifier,
                    "Engine" => engine,
                    "DBInstanceClass" => "db.m5.large",
                    "AllocatedStorage" => 100,
                ],
            )
            .await,
        );
        let (address, published) = endpoint_of(&created);
        assert!(
            address.ends_with(".rds.amazonaws.com"),
            "case {engine:?}: an engine with no listener keeps its DNS endpoint, got \
             {address:?}"
        );
        assert_eq!(
            published.to_string(),
            port,
            "case {engine:?}: it also keeps the engine's documented default port"
        );
    }
}
