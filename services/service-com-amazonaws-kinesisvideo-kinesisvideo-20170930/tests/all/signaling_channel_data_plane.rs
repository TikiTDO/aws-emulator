// SPDX-License-Identifier: CC0-1.0

//! A created signaling channel is a channel its DATA plane serves.
//!
//! The rule these cases assert, one level above the recorded case (one channel, created
//! and then read on the signaling endpoint):
//!
//! > A signaling channel exists for the Kinesis Video Signaling data plane exactly while
//! > the control plane holds it: `CreateSignalingChannel` makes it resolvable there and
//! > `DeleteSignalingChannel` stops it resolving, so `GetIceServerConfig` and
//! > `SendAlexaOfferToMaster` answer a created channel from its record and answer a channel
//! > that was never created, or has been deleted, `ResourceNotFoundException` —
//! > `"<channelName> channel does not exist !"`. The retention window the create stated
//! > travels with it, because that is the window the data plane redelivers an undelivered
//! > message over.
//!
//! Two services, one engine, one router: the create half is served by this crate and the
//! data-plane half by the signaling crate this one seeds, which is the seam under test.
//!
//! The rows span what the rule generalises over: the channel name's form, the region and
//! the retention window the create states.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice as signaling;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying BOTH services over ONE engine, with request authentication off so a
/// case can send an unsigned-but-scoped request.
fn harness() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(kvs::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(signaling::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// One answer from the router: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: serde_json::Value,
}

impl Answer {
    /// The wire error code of a failure body, from `__type` or from the envelope header
    /// the boundary path sets.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .rsplit(['#', '/'])
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// The `Message` member a modelled failure carries.
    fn message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// POST one restJson1 call to `path`, in `region`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    path: &str,
    body: serde_json::Value,
) -> Answer {
    let host = format!("kinesisvideo.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request and names its region; the
        // signature itself is not checked here.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/kinesisvideo/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let mut body: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    if let (Some(error_type), Some(object)) = (error_type, body.as_object_mut()) {
        object
            .entry("__type".to_string())
            .or_insert(serde_json::Value::String(error_type));
    }
    Answer { status, body }
}

/// How many times a case looks for a channel to reach `ACTIVE` before failing, at
/// [`ACTIVE_POLL_INTERVAL`] apart: a budget far above the span the control plane owes the
/// transition, and bounded so a case that never activates fails instead of hanging.
const ACTIVE_POLL_ATTEMPTS: usize = 1_200;

/// How long a case waits between looks; see [`ACTIVE_POLL_ATTEMPTS`].
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Poll `DescribeSignalingChannel` until the channel reports `ACTIVE`, as a client waiting
/// on the asynchronous create does. Waits for the CONDITION, never for a fixed time.
async fn wait_until_active(router: &MultiServiceRouter, region: &str, arn: &str) {
    for _ in 0..ACTIVE_POLL_ATTEMPTS {
        let described = call(
            router,
            region,
            "/describeSignalingChannel",
            serde_json::json!({ "ChannelARN": arn }),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {arn:?}: DescribeSignalingChannel must answer 200, got {:?}",
            described.body
        );
        if described.body["ChannelInfo"]["ChannelStatus"] == "ACTIVE" {
            return;
        }
        tokio::time::sleep(ACTIVE_POLL_INTERVAL).await;
    }
    panic!("case {arn:?}: the channel must reach ACTIVE within {ACTIVE_POLL_ATTEMPTS} looks");
}

/// One row: a channel to create, and what its data plane must then serve.
struct Row {
    case: &'static str,
    region: &'static str,
    channel_name: &'static str,
    /// The `MessageTtlSeconds` the create states, when it states one.
    message_ttl_seconds: Option<i32>,
    /// The `ClientId` the ICE call names, when it names one.
    client_id: Option<&'static str>,
}

/// The rows differ along the channel's region, its name's form, the retention window the
/// create states and whether the ICE call names a viewer.
const ROWS: &[Row] = &[
    Row {
        case: "the channel of the issue report",
        region: "us-west-2",
        channel_name: "sim2real-kvsig-03156629ce13",
        message_ttl_seconds: None,
        client_id: Some("viewer-1"),
    },
    Row {
        case: "a channel in another region, with the shortest retention window",
        region: "eu-central-1",
        channel_name: "alexa.channel_2-X9",
        message_ttl_seconds: Some(5),
        client_id: None,
    },
    Row {
        case: "a one-character name and the longest retention window",
        region: "ap-northeast-1",
        channel_name: "a",
        message_ttl_seconds: Some(120),
        client_id: Some("a"),
    },
];

/// A channel created through the control plane is served by the signaling data plane, and
/// stops being served once it is deleted.
#[tokio::test]
async fn a_created_channel_is_served_by_the_signaling_data_plane() {
    let (engine, router) = harness();

    for row in ROWS {
        let case = row.case;
        let mut create = serde_json::json!({
            "ChannelName": row.channel_name,
            "ChannelType": "SINGLE_MASTER",
        });
        if let (Some(target), Some(ttl)) = (create.as_object_mut(), row.message_ttl_seconds) {
            target.insert(
                "SingleMasterConfiguration".to_string(),
                serde_json::json!({ "MessageTtlSeconds": ttl }),
            );
        }
        let created = call(&router, row.region, "/createSignalingChannel", create).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateSignalingChannel must answer 200, got {:?}",
            created.body
        );
        let arn = created.body["ChannelARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ChannelARN in {:?}", created.body))
            .to_string();
        wait_until_active(&router, row.region, &arn).await;

        // The HTTPS data-plane endpoint a client resolves before the call below.
        let resolved = call(
            &router,
            row.region,
            "/getSignalingChannelEndpoint",
            serde_json::json!({
                "ChannelARN": arn,
                "SingleMasterChannelEndpointConfiguration": {
                    "Protocols": ["HTTPS", "WSS"],
                    "Role": "VIEWER",
                }
            }),
        )
        .await;
        assert_eq!(
            resolved.status, 200,
            "case {case:?}: GetSignalingChannelEndpoint must answer 200, got {:?}",
            resolved.body
        );

        // The positive control: the ICE call against the channel that was just created.
        let mut ice = serde_json::json!({ "ChannelARN": arn });
        if let (Some(target), Some(client_id)) = (ice.as_object_mut(), row.client_id) {
            target.insert("ClientId".to_string(), serde_json::json!(client_id));
        }
        let issued = call(&router, row.region, "/v1/get-ice-server-config", ice.clone()).await;
        assert_eq!(
            issued.status, 200,
            "case {case:?}: GetIceServerConfig on a created channel must answer 200, got {:?}",
            issued.body
        );
        let servers = issued.body["IceServerList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !servers.is_empty(),
            "case {case:?}: expected a non-empty IceServerList, got {:?}",
            issued.body
        );
        let uris = servers[0]["Uris"].as_array().cloned().unwrap_or_default();
        assert!(
            uris.iter().any(|uri| uri
                .as_str()
                .is_some_and(|uri| uri.starts_with("turn:") || uri.starts_with("turns:"))),
            "case {case:?}: expected at least one TURN URI, got {uris:?}"
        );
        for member in ["Username", "Password"] {
            assert!(
                servers[0][member].as_str().is_some_and(|v| !v.is_empty()),
                "case {case:?}: expected a non-empty {member}, got {:?}",
                servers[0]
            );
        }

        // The retention window the create stated is the window the data plane holds for
        // this channel, which is what an undelivered offer is redelivered over.
        let expected_ttl = row.message_ttl_seconds.unwrap_or(60);
        let held = engine
            .state::<signaling::state::ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState>()
            .lock()
            .resolve_channel(&arn)
            .map(|channel| channel.message_ttl_secs)
            .unwrap_or_else(|error| panic!("case {case:?}: {error:?}"));
        assert_eq!(
            held, expected_ttl,
            "case {case:?}: expected the created channel's MessageTtlSeconds, got {held}"
        );

        // Deleted on the control plane, gone on the data plane: the same call now answers
        // the service's not-found sentence, naming the channel by its NAME alone.
        let deleted = call(
            &router,
            row.region,
            "/deleteSignalingChannel",
            serde_json::json!({ "ChannelARN": arn }),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteSignalingChannel must answer 200, got {:?}",
            deleted.body
        );
        let gone = call(&router, row.region, "/v1/get-ice-server-config", ice).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: GetIceServerConfig on a deleted channel must answer 404, got {:?}",
            gone.body
        );
        assert_eq!(
            gone.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            gone.body
        );
        assert_eq!(
            gone.message(),
            format!("{} channel does not exist !", row.channel_name),
            "case {case:?}: expected the service's not-found sentence, got {:?}",
            gone.body
        );
    }
}
