// SPDX-License-Identifier: CC0-1.0

//! WHERE the ingest route is served: the address the control plane announces for the
//! write plane, and nowhere else on this emulator.
//!
//! The rule under test: a Kinesis Video Streams media API belongs to ONE access mode and
//! is bound on the address announced for that mode — Kinesis Video Media's
//! `InvalidEndpointException` is "Caller used wrong endpoint to write data to a stream. On
//! receiving such an exception, the user must call `GetDataEndpoint` with `AccessMode` set
//! to \"READ\"" — so `POST /putMedia` is served at the origin
//! `GetDataEndpoint(APIName=PUT_MEDIA)` hands a producer. Every OTHER address of this
//! emulator — the control-plane front door the discovery call itself arrived on, and the
//! read plane's own origin — hosts no API at that path at all, so it answers the 404 an
//! endpoint gives a path it binds no ROUTE for, whose whole body is the bare element
//! naming the unknown-operation class and which carries no error-type header, and ingests
//! nothing: the stream's archive stays empty, while the identical bytes sent to the
//! announced write origin persist exactly one fragment.
//!
//! The table's rows differ in the ADDRESS the producer sent the identical request to: the
//! announced write origin, the announced read origin, and three forms of this emulator's
//! own front-door address (an IPv4 loopback authority, the `localhost` name and a
//! bracketed IPv6 literal) — plus the service's published regional host, which this
//! emulator announced no plane at and cannot place on one, where the route's own claim
//! stands. Every row sends the same container to its own stream, so what was ingested is
//! read back per row.

use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;
use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice as media;
use smithy_server_runtime::MultiServiceRouter;

use crate::put_media_ingest::{cluster, ingest_headers, Container, IngestHeaders};

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The service's published regional host — what a client that resolves AWS's own
/// hostnames addresses, rather than an address this emulator handed out.
const PUBLISHED_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// One router over one engine, carrying the three services the ingest round trip needs:
/// the control plane that creates the stream and announces the planes, this ingest/media
/// service, and the archive read plane that reports what was ingested.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(control_plane::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(media::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(archived_media::EmulatedService::new(engine));
        Self { router }
    }

    /// The SigV4 credential scope every request carries: its service field is the routing
    /// key the five Kinesis Video services share.
    fn authorization() -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261107/{REGION}/kinesisvideo/\
             aws4_request, SignedHeaders=host, Signature=0000"
        )
    }

    /// POST one restJson1 call at `authority`, the address the request arrives on.
    async fn call_at(
        &self,
        authority: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{authority}{path}"))
            .header("host", authority)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization())
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.router.dispatch(request).await
    }

    /// POST one ingest session to `authority`, with the path a producer appends to the
    /// endpoint it was handed.
    async fn put_media_at(
        &self,
        authority: &str,
        headers: &IngestHeaders,
        body: Vec<u8>,
    ) -> http::Response<Bytes> {
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{authority}/putMedia"))
            .header("host", authority)
            .header("authorization", Self::authorization());
        for (name, value) in headers {
            request = request.header(*name, value.clone());
        }
        self.router
            .dispatch(
                request
                    .body(Bytes::from(body))
                    .expect("a well-formed request"),
            )
            .await
    }

    /// Create `name` through the control plane, on the address the discovery call is made
    /// at, and answer the `StreamARN` it minted.
    async fn create_stream(&self, control: &str, name: &str) -> String {
        let response = self
            .call_at(
                control,
                "/createStream",
                &serde_json::json!({ "StreamName": name, "DataRetentionInHours": 1 }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case {name:?}: expected HTTP 200 from CreateStream, got {} {}",
            status(&response),
            text(&response)
        );
        json(&response)
            .get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: no StreamARN in the answer"))
            .to_string()
    }

    /// The authority `GetDataEndpoint` announces for `api_name`, with the scheme stripped
    /// off: the address a producer or reader then dials.
    async fn announced_authority(&self, control: &str, name: &str, api_name: &str) -> String {
        let response = self
            .call_at(
                control,
                "/getDataEndpoint",
                &serde_json::json!({ "StreamName": name, "APIName": api_name }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case {api_name:?}: expected HTTP 200 from GetDataEndpoint, got {} {}",
            status(&response),
            text(&response)
        );
        let endpoint = json(&response)
            .get("DataEndpoint")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let scheme = format!("{}://", control_plane::state::DATA_PLANE_SCHEME);
        endpoint
            .strip_prefix(&scheme)
            .unwrap_or_else(|| {
                panic!("case {api_name:?}: expected a {scheme} origin, got {endpoint:?}")
            })
            .to_string()
    }

    /// How many fragments the archive read plane reports for the stream `name`, over a
    /// window wide enough to hold anything this suite could ingest.
    async fn archived_fragments(&self, control: &str, name: &str) -> usize {
        let response = self
            .call_at(
                control,
                "/listFragments",
                &serde_json::json!({
                    "StreamName": name,
                    "FragmentSelector": {
                        "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                        "TimestampRange": {
                            "StartTimestamp": 0,
                            "EndTimestamp": 4_000_000_000i64,
                        },
                    },
                }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case {name:?}: expected HTTP 200 from ListFragments, got {} {}",
            status(&response),
            text(&response)
        );
        json(&response)
            .get("Fragments")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .unwrap_or_default()
    }
}

fn status(response: &http::Response<Bytes>) -> u16 {
    response.status().as_u16()
}

fn text(response: &http::Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

fn json(response: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
}

/// The `EventType` values an ingest answer's acknowledgement stream carries, in order:
/// one JSON object per line, which is what a producer reads off this wire.
fn event_types(response: &http::Response<Bytes>) -> Vec<String> {
    text(response)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|acknowledgement| {
            acknowledgement
                .get("EventType")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Which address a row sends the ingest request to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// The origin announced for `PUT_MEDIA`.
    AnnouncedWritePlane,
    /// The origin announced for the read APIs.
    AnnouncedReadPlane,
    /// A literal address of this emulator's own front door.
    FrontDoor(&'static str),
    /// The service's published regional host.
    PublishedHost,
}

/// One row: the address the identical request is sent to, and whether the ingest route is
/// served there.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    address: Address,
    served: bool,
}

/// The rows, differing along the address the producer sent the request to.
const ROWS: &[Row] = &[
    Row {
        case: "the announced write origin",
        address: Address::AnnouncedWritePlane,
        served: true,
    },
    Row {
        case: "the announced read origin",
        address: Address::AnnouncedReadPlane,
        served: false,
    },
    Row {
        case: "this emulator's front door, by its loopback address",
        address: Address::FrontDoor("127.0.0.1:9297"),
        served: false,
    },
    Row {
        case: "this emulator's front door, by name",
        address: Address::FrontDoor("localhost:9297"),
        served: false,
    },
    Row {
        case: "this emulator's front door, by its IPv6 literal",
        address: Address::FrontDoor("[::1]:9297"),
        served: false,
    },
    Row {
        case: "the service's published regional host",
        address: Address::PublishedHost,
        served: true,
    },
];

/// RULE: `POST /putMedia` is served at the origin announced for the write plane; at every
/// other address of this emulator that path is bound to no route at all, so it is answered
/// with the 404 an endpoint gives such a path — the bare unknown-operation element as the
/// whole body, and no error-type header — and nothing is ingested.
#[tokio::test]
async fn the_ingest_route_is_served_only_at_the_origin_announced_for_the_write_plane() {
    let harness = Harness::new();
    // The discovery calls are made at this emulator's own front door, as a client on this
    // machine makes them, and the planes are announced from there.
    let control = "127.0.0.1:9297";
    harness.create_stream(control, "plane-probe").await;
    let write_plane = harness
        .announced_authority(control, "plane-probe", "PUT_MEDIA")
        .await;
    let read_plane = harness
        .announced_authority(control, "plane-probe", "LIST_FRAGMENTS")
        .await;
    assert_ne!(
        write_plane, read_plane,
        "the two planes must be announced at different addresses, got {write_plane:?} for both"
    );
    assert_ne!(
        write_plane, control,
        "the write plane must not be announced at the control-plane address, \
         got {write_plane:?}"
    );

    for (index, row) in ROWS.iter().enumerate() {
        let Row {
            case,
            address,
            served,
        } = *row;
        let authority = match address {
            Address::AnnouncedWritePlane => write_plane.clone(),
            Address::AnnouncedReadPlane => read_plane.clone(),
            Address::FrontDoor(literal) => literal.to_string(),
            Address::PublishedHost => PUBLISHED_HOST.to_string(),
        };
        let name = format!("plane-probe-{index}");
        harness.create_stream(control, &name).await;

        // The identical request, differing only in the address it is sent to.
        let headers = ingest_headers(
            ("x-amzn-stream-name", name.clone()),
            "RELATIVE",
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_761_000_000_000),
        );
        let container = Container::of(vec![cluster(0, 2, 20, 0x71)]).bytes();
        let response = harness.put_media_at(&authority, &headers, container).await;

        let archived = harness.archived_fragments(control, &name).await;
        if served {
            assert_eq!(
                status(&response),
                200,
                "case {case:?} ({authority}): expected the ingest route to be served, got {} {}",
                status(&response),
                text(&response)
            );
            assert_eq!(
                event_types(&response),
                vec!["BUFFERING", "RECEIVED", "PERSISTED"],
                "case {case:?} ({authority}): expected the acknowledgement stream of one \
                 accepted Cluster, got {:?}",
                text(&response)
            );
            assert_eq!(
                archived, 1,
                "case {case:?} ({authority}): expected the submitted Cluster to be archived, \
                 got {archived} fragments"
            );
        } else {
            assert_eq!(
                status(&response),
                404,
                "case {case:?} ({authority}): expected the unrouted-path refusal, got {} {}",
                status(&response),
                text(&response)
            );
            assert_eq!(
                response
                    .headers()
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok()),
                None,
                "case {case:?} ({authority}): an unrouted path names no error type, got {:?}",
                response.headers().get("x-amzn-errortype")
            );
            assert_eq!(
                text(&response).trim(),
                // Written out as the wire carries it rather than read off the code under
                // test, so a drift away from this body must fail here.
                "<UnknownOperationException/>",
                "case {case:?} ({authority}): expected the bare unknown-operation element as \
                 the whole body, got {:?}",
                text(&response)
            );
            assert_eq!(
                archived, 0,
                "case {case:?} ({authority}): a refused ingest must archive nothing, \
                 got {archived} fragments"
            );
        }
    }
}

/// RULE: the announced write origin is one a producer DIALS as it stands — the transport
/// its scheme names, with nothing negotiated first and no trust material beyond what the
/// client already had — and the `POST /putMedia` it sends there is served and archived.
///
/// This is the whole point of the announced value: a client that cannot connect to it
/// never sends a request at all, so the case connects to the address the operation handed
/// back, over a socket, and reads the acknowledgement stream back off it.
#[tokio::test]
async fn a_producer_dials_the_announced_write_origin_as_it_stands() {
    let harness = Harness::new();
    let control = "127.0.0.1:9297";
    let name = "dialled-probe";
    harness.create_stream(control, name).await;
    let write_plane = harness.announced_authority(control, name, "PUT_MEDIA").await;

    let headers = ingest_headers(
        ("x-amzn-stream-name", name.to_string()),
        "RELATIVE",
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_761_000_000_000),
    );
    let body = Container::of(vec![cluster(0, 2, 20, 0x73)]).bytes();
    let mut head = format!(
        "POST /putMedia HTTP/1.1\r\nhost: {write_plane}\r\nauthorization: {}\r\n\
         content-length: {}\r\nconnection: close\r\n",
        Harness::authorization(),
        body.len()
    );
    for (header, value) in &headers {
        head.push_str(&format!("{header}: {value}\r\n"));
    }
    head.push_str("\r\n");

    let (host, port) = write_plane.rsplit_once(':').unwrap_or_else(|| {
        panic!("case \"dial\": expected the announced origin to carry its port, got {write_plane:?}")
    });
    let port = port
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("case \"dial\": expected a port, got {write_plane:?}"));
    let mut conn = match host {
        "127.0.0.1" => tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await,
        "::1" | "[::1]" => {
            tokio::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port)).await
        }
        other => panic!("case \"dial\": expected an address of this machine, got {other:?}"),
    }
    .unwrap_or_else(|error| {
        panic!("case \"dial\": the announced origin {write_plane:?} must accept a connection, got {error}")
    });

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    conn.write_all(head.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case \"dial\": writing the request failed: {error}"));
    conn.write_all(&body)
        .await
        .unwrap_or_else(|error| panic!("case \"dial\": writing the container failed: {error}"));
    conn.flush()
        .await
        .unwrap_or_else(|error| panic!("case \"dial\": writing the request failed: {error}"));
    let mut answer = Vec::new();
    conn.read_to_end(&mut answer)
        .await
        .unwrap_or_else(|error| panic!("case \"dial\": reading the answer failed: {error}"));
    let answer = String::from_utf8_lossy(&answer).to_string();

    let (status_line, tail) = answer.split_once("\r\n").unwrap_or_else(|| {
        panic!("case \"dial\": expected an HTTP answer from {write_plane:?}, got {answer:?}")
    });
    assert!(
        status_line.contains(" 200 "),
        "case \"dial\": expected the dialled origin to serve the ingest, got {status_line:?} \
         in {answer:?}"
    );
    assert!(
        tail.contains("\"EventType\":\"PERSISTED\""),
        "case \"dial\": expected the acknowledgement stream of the accepted Cluster, \
         got {answer:?}"
    );
    let archived = harness.archived_fragments(control, name).await;
    assert_eq!(
        archived, 1,
        "case \"dial\": expected the dialled ingest to archive one fragment, got {archived}"
    );
}

/// RULE: the read plane's own routes keep being served at the address announced for them —
/// scoping the ingest route to its own origin takes nothing away from the other plane.
#[tokio::test]
async fn the_read_plane_still_serves_its_own_routes_at_the_origin_announced_for_it() {
    let harness = Harness::new();
    let control = "127.0.0.1:9297";
    harness.create_stream(control, "read-probe").await;
    let write_plane = harness
        .announced_authority(control, "read-probe", "PUT_MEDIA")
        .await;
    let read_plane = harness
        .announced_authority(control, "read-probe", "LIST_FRAGMENTS")
        .await;

    let headers = ingest_headers(
        ("x-amzn-stream-name", "read-probe".to_string()),
        "RELATIVE",
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_761_000_000_000),
    );
    let ingested = harness
        .put_media_at(
            &write_plane,
            &headers,
            Container::of(vec![cluster(0, 1, 0, 0x72)]).bytes(),
        )
        .await;
    assert_eq!(
        status(&ingested),
        200,
        "case \"ingest\": expected the announced write origin to serve the ingest, got {} {}",
        status(&ingested),
        text(&ingested)
    );

    // Each read route of the archive plane, asked for at the address announced for it.
    for path in ["/listFragments", "/getMedia"] {
        let body = match path {
            "/getMedia" => serde_json::json!({
                "StreamName": "read-probe",
                "StartSelector": { "StartSelectorType": "EARLIEST" },
            }),
            _ => serde_json::json!({
                "StreamName": "read-probe",
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": 0, "EndTimestamp": 4_000_000_000i64 },
                },
            }),
        };
        let response = harness.call_at(&read_plane, path, &body).await;
        assert_eq!(
            status(&response),
            200,
            "case {path:?} ({read_plane}): expected the announced read origin to serve it, \
             got {} {}",
            status(&response),
            text(&response)
        );
    }
}
