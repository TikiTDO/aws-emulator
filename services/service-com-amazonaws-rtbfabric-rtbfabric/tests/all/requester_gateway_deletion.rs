// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteRequesterGateway`: releasing the requester gateway this
//! service persisted.
//!
//! The rule under test is about state, not about one recorded request: the delete releases
//! everything the store holds BECAUSE of that gateway — the gateway record, every link it
//! was an end of, and the client-token records that minted it — and refuses an id it does
//! not hold with the modelled not-found. The table spans what the rule generalises over:
//! how much the gateway carried (no links, a link to a peer, several gateways of which one
//! is deleted) and whether the id was ever stored.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The one emulated service, behind the same `MultiServiceRouter` the production router
/// assembles.
struct Fabric {
    router: MultiServiceRouter,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one restJson1 request and return `(status, parsed body)`.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let bytes = match body {
            Some(body) => Bytes::from(serde_json::to_vec(&body).expect("serialize the body")),
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://rtbfabric.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rtbfabric/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create one requester gateway and return the id the service minted.
    async fn create_requester(&self, case: &str, client_token: &str) -> String {
        let (status, body) = self
            .send("POST", "/requester-gateway", Some(requester_body(client_token)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateRequesterGateway to answer 200, got {status} {body:?}"
        );
        text(&body, "gatewayId")
    }

    /// Create one responder gateway and return the id the service minted.
    async fn create_responder(&self, case: &str, client_token: &str) -> String {
        let (status, body) = self
            .send("POST", "/responder-gateway", Some(responder_body(client_token)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateResponderGateway to answer 200, got {status} {body:?}"
        );
        text(&body, "gatewayId")
    }

    /// The ids `ListRequesterGateways` reports, sorted.
    async fn listed(&self, case: &str) -> Vec<String> {
        let (status, body) = self.send("GET", "/requester-gateways", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListRequesterGateways to answer 200, got {status} {body:?}"
        );
        let mut ids: Vec<String> = body["gatewayIds"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected gatewayIds to be a list, got {body:?}"))
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        ids.sort();
        ids
    }
}

/// A `CreateRequesterGateway` body whose members satisfy the modelled patterns.
fn requester_body(client_token: &str) -> Value {
    json!({
        "vpcId": "vpc-0123456789abcdef0",
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
        "clientToken": client_token,
    })
}

/// A `CreateResponderGateway` body, for the peer end of a link.
fn responder_body(client_token: &str) -> Value {
    json!({
        "vpcId": "vpc-0123456789abcdef0",
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
        "port": 443,
        "protocol": "HTTPS",
        "listenerConfig": { "protocols": ["HTTPS"] },
        "domainName": "responder.example.com",
        "clientToken": client_token,
    })
}

/// A `CreateLink` body joining a stored requester gateway to `peer`. `logSettings` carries
/// the members `smithy_query(query="shape", shape="LinkLogSettings,\
/// LinkApplicationLogConfiguration")` marks required.
fn link_body(peer: &str) -> Value {
    json!({
        "peerGatewayId": peer,
        "logSettings": {
            "applicationLogs": { "sampling": { "errorLog": 100.0, "filterLog": 0.0 } }
        },
    })
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// One row of the release table: how many requester gateways the store holds and whether
/// the one being deleted is an end of a link.
struct Row {
    name: &'static str,
    stored: usize,
    linked: bool,
}

const ROWS: &[Row] = &[
    Row {
        name: "the only gateway, carrying nothing",
        stored: 1,
        linked: false,
    },
    Row {
        name: "one of several gateways",
        stored: 4,
        linked: false,
    },
    Row {
        name: "a gateway that is an end of a link",
        stored: 1,
        linked: true,
    },
    Row {
        name: "one of several gateways, and the linked one",
        stored: 3,
        linked: true,
    },
];

#[tokio::test]
async fn the_delete_releases_the_gateway_and_everything_held_because_of_it() {
    for row in ROWS {
        let case = row.name;
        let fabric = Fabric::new();

        let mut created: Vec<String> = Vec::new();
        for n in 0..row.stored {
            created.push(fabric.create_requester(case, &format!("token-{n}")).await);
        }
        let deleted = created[0].clone();

        let mut link_id = String::new();
        let mut peer = String::new();
        if row.linked {
            peer = fabric.create_responder(case, "token-peer").await;
            let (status, link) = fabric
                .send(
                    "POST",
                    &format!("/gateway/{deleted}/create-link"),
                    Some(link_body(&peer)),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateLink to answer 200, got {status} {link:?}"
            );
            link_id = text(&link, "linkId");
        }

        let (status, answered) = fabric
            .send("DELETE", &format!("/requester-gateway/{deleted}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {answered:?}"
        );
        assert_eq!(
            text(&answered, "gatewayId"),
            deleted,
            "case {case:?}: expected the delete to answer the gateway it released, got {answered:?}"
        );
        assert_eq!(
            text(&answered, "status"),
            "DELETED",
            "case {case:?}: expected a modelled terminal status, got {answered:?}"
        );

        // The reader no longer resolves it, and the listing no longer reports it.
        let (status, read) = fabric
            .send("GET", &format!("/requester-gateway/{deleted}"), None)
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the released gateway to be unreadable, got {status} {read:?}"
        );
        let mut survivors: Vec<String> = created[1..].to_vec();
        survivors.sort();
        assert_eq!(
            fabric.listed(case).await,
            survivors,
            "case {case:?}: expected only the gateways that were not deleted to be listed"
        );

        if row.linked {
            // The link cannot outlive either of its two ends, so it is gone from the
            // peer's side too — while the peer gateway itself survives.
            let (status, read) = fabric
                .send("GET", &format!("/gateway/{peer}/link/{link_id}"), None)
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the link to be released with its end, got {status} {read:?}"
            );
            let (status, read) = fabric
                .send("GET", &format!("/responder-gateway/{peer}"), None)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the peer gateway to survive, got {status} {read:?}"
            );
            assert_eq!(
                read["totalLinksCount"],
                json!(0),
                "case {case:?}: expected the peer to count no links, got {read:?}"
            );
        }

        // The client token that minted the released gateway is forgotten with it: a
        // retry mints a new gateway rather than replaying an id the store no longer
        // holds.
        let reused = fabric.create_requester(case, "token-0").await;
        assert_ne!(
            reused, deleted,
            "case {case:?}: expected the retry to mint a new gateway, got the released id"
        );
        let (status, read) = fabric
            .send("GET", &format!("/requester-gateway/{reused}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the newly minted gateway to resolve, got {status} {read:?}"
        );
    }
}

#[tokio::test]
async fn an_id_the_store_does_not_hold_is_refused_with_the_modelled_not_found() {
    let fabric = Fabric::new();
    let stored = fabric.create_requester("not found", "token-0").await;
    // A responder gateway is a different resource: its id is not a requester gateway's.
    let responder = fabric.create_responder("not found", "token-peer").await;

    let (status, _) = fabric
        .send("DELETE", &format!("/requester-gateway/{stored}"), None)
        .await;
    assert_eq!(status, 200, "expected the first delete to answer 200");

    // Every id below satisfies the modelled `^rtb-gw-[a-z0-9-]{1,25}$`, so the refusal is
    // the handler's not-found rather than a constraint refusal.
    for (case, gateway_id) in [
        ("the gateway just released", stored),
        ("a responder gateway's id", responder),
        ("an id never minted", "rtb-gw-0000000000000000".to_string()),
    ] {
        let (status, body) = fabric
            .send("DELETE", &format!("/requester-gateway/{gateway_id}"), None)
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled not-found, got {status} {body:?}"
        );
        assert_eq!(
            text(&body, "__type"),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled error code, got {body:?}"
        );
        assert_eq!(
            text(&body, "message"),
            "The requested resource was not found.",
            "case {case:?}: expected the fixed not-found wording, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_gateway_still_settling_can_be_released() {
    let fabric = Fabric::new();
    let gateway_id = fabric.create_requester("still settling", "token-0").await;

    // Straight after the create the gateway is still in its transient status, and the
    // release does not wait for the transition the clock owes.
    let (status, read) = fabric
        .send("GET", &format!("/requester-gateway/{gateway_id}"), None)
        .await;
    assert_eq!(status, 200, "expected the gateway to resolve, got {read:?}");
    assert_eq!(
        text(&read, "status"),
        "PENDING_CREATION",
        "expected the transient status straight after the create, got {read:?}"
    );

    let (status, answered) = fabric
        .send("DELETE", &format!("/requester-gateway/{gateway_id}"), None)
        .await;
    assert_eq!(
        status, 200,
        "expected a pending gateway to be releasable, got {status} {answered:?}"
    );

    // The release holds once the clock has moved on: a gateway created after it settles
    // ACTIVE, so the instant at which the released gateway's own pending transition would
    // have come due has passed, and the released gateway is still gone. The deadline
    // bounds a hang, it does not time the transition.
    let other = fabric.create_requester("still settling", "token-1").await;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let (status, read) = fabric
            .send("GET", &format!("/requester-gateway/{other}"), None)
            .await;
        assert_eq!(
            status, 200,
            "expected the second gateway to keep resolving while it settles, got {read:?}"
        );
        if text(&read, "status") == "ACTIVE" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected the second gateway to settle ACTIVE, still {:?}",
            read["status"]
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let (status, read) = fabric
        .send("GET", &format!("/requester-gateway/{gateway_id}"), None)
        .await;
    assert_eq!(
        status, 404,
        "expected the released gateway to stay gone, got {status} {read:?}"
    );
}
