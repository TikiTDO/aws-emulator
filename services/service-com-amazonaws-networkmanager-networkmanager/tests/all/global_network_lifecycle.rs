// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the global-network / site lifecycle, driven over HTTP.
//!
//! Every call here goes through the real `MultiServiceRouter` — the same dispatch the
//! production router runs — so the restJson1 routes, the query/label bindings and the
//! error envelopes are exercised exactly as a client would hit them. Nothing reaches
//! into `crate::state`: a resource is only ever read back through an operation.
//!
//! The harness below (`Emulator` and the helpers under it) is shared with the other
//! suites in this target, which reach it as `crate::global_network_lifecycle::*`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host, which is what routes the request to this service (the router's
/// `can_handle` resolves the signing name `networkmanager` from it).
const HOST: &str = "networkmanager.us-west-2.amazonaws.com";

/// The settle span `crate::state::SETTLE_DELAY` fixes, plus slack, so a test that waits
/// for the `PENDING` -> `AVAILABLE` transition the clock owes is not racing it.
pub(crate) const PAST_SETTLE: Duration = Duration::from_millis(900);

/// The emulator under test: one shared engine, this service, the production dispatch.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::with_engine().0
    }

    /// The emulator plus the engine its store lives in.
    ///
    /// Only a suite that has to SEED a record no implemented operation writes yet needs
    /// the engine (`crate::network_telemetry` does: the operations that register a
    /// telemetry-bearing connection through a GATEWAY, and for a transit gateway Connect
    /// peer, are still scaffold stubs with their own issues). Every
    /// other suite takes [`Emulator::new`] and reads a resource back through an operation.
    pub(crate) fn with_engine() -> (Self, Arc<engine::InMemoryEngine>) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise operation behaviour, not SigV4; signing is covered by the
        // framework's own tests.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_networkmanager_networkmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        (Self { router }, engine)
    }

    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("the request body must serialize")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", HOST)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("the request must build");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
            })
        };
        (status, parsed)
    }

    pub(crate) async fn get(&self, uri: &str) -> (u16, Value) {
        self.call("GET", uri, None).await
    }

    pub(crate) async fn post(&self, uri: &str, body: Value) -> (u16, Value) {
        self.call("POST", uri, Some(body)).await
    }

    pub(crate) async fn patch(&self, uri: &str, body: Value) -> (u16, Value) {
        self.call("PATCH", uri, Some(body)).await
    }

    pub(crate) async fn delete(&self, uri: &str) -> (u16, Value) {
        self.call("DELETE", uri, None).await
    }

    /// Create a global network and return its minted id.
    pub(crate) async fn create_global_network(&self, description: &str) -> String {
        let (status, body) = self
            .post("/global-networks", json!({ "Description": description }))
            .await;
        assert_eq!(
            status, 200,
            "CreateGlobalNetwork({description:?}): expected 200, got {status} with {body}"
        );
        body["GlobalNetwork"]["GlobalNetworkId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateGlobalNetwork({description:?}): no id in {body}"))
            .to_string()
    }

    /// Create a site in `global_network_id` and return its minted id.
    pub(crate) async fn create_site(&self, global_network_id: &str, description: &str) -> String {
        let (status, body) = self
            .post(
                &format!("/global-networks/{global_network_id}/sites"),
                json!({ "Description": description }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateSite({description:?}): expected 200, got {status} with {body}"
        );
        body["Site"]["SiteId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateSite({description:?}): no id in {body}"))
            .to_string()
    }
}

/// The id shape Network Manager mints: `<kind>-<17 lowercase hex digits>`. The model's
/// own id patterns for the kinds it constrains read `^core-network-([0-9a-f]{8,17})$`
/// (`smithy_query shape=CoreNetworkId`), which is where the alphabet and width come from.
pub(crate) fn is_minted_id(id: &str, kind: &str) -> bool {
    match id.strip_prefix(&format!("{kind}-")) {
        Some(suffix) => {
            suffix.len() == 17
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        None => false,
    }
}

/// The ARN of a resource of this global (region-less) service, built the way the
/// emulator's one account/ARN source does.
pub(crate) fn expected_arn(kind: &str, id: &str) -> String {
    format!(
        "arn:aws:networkmanager::{}:{kind}/{id}",
        engine::emulator_account_id()
    )
}

/// The ARN of a resource that exists only inside a global network — a site, a link, a
/// device: the global network's id is part of the name, ahead of the resource's own id.
pub(crate) fn expected_scoped_arn(kind: &str, global_network_id: &str, id: &str) -> String {
    expected_arn(kind, &format!("{global_network_id}/{id}"))
}

/// The wire error code of a failed response (restJson1 carries it as `__type`).
pub(crate) fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("<no __type>")
}

/// The ids in a `DescribeGlobalNetworks` / `GetSites` / `GetLinks` page, in the order
/// returned.
pub(crate) fn ids_of(body: &Value, list: &str, id_member: &str) -> Vec<String> {
    body[list]
        .as_array()
        .unwrap_or_else(|| panic!("expected a {list} array in {body}"))
        .iter()
        .map(|item| {
            item[id_member]
                .as_str()
                .unwrap_or_else(|| panic!("expected {id_member} in {item}"))
                .to_string()
        })
        .collect()
}

/// One row of a listing table: the case name, the ids the request names, and the ids the
/// page it answers must carry. Shared with the suites that table a listing's id member
/// (`crate::describe_global_networks_id_resolution`).
pub(crate) type ListingCase = (&'static str, Vec<String>, Vec<String>);

#[tokio::test]
async fn global_network_and_site_lifecycle_round_trips_through_stored_state() {
    let emulator = Emulator::new();

    // CREATE: the id, the ARN and CreatedAt are minted here, not echoed.
    let (status, created) = emulator
        .post(
            "/global-networks",
            json!({
                "Description": "demo",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateGlobalNetwork: expected 200, got {status} with {created}"
    );
    let network = &created["GlobalNetwork"];
    let network_id = network["GlobalNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateGlobalNetwork: no GlobalNetworkId in {created}"))
        .to_string();
    assert!(
        is_minted_id(&network_id, "global-network"),
        "CreateGlobalNetwork: expected a global-network-<17 hex> id, got {network_id:?}"
    );
    assert_eq!(
        network["GlobalNetworkArn"],
        json!(expected_arn("global-network", &network_id)),
        "CreateGlobalNetwork: ARN must name the minted id, got {network}"
    );
    assert_eq!(
        network["Description"],
        json!("demo"),
        "CreateGlobalNetwork: Description must be stored, got {network}"
    );
    assert_eq!(
        network["Tags"],
        json!([{ "Key": "env", "Value": "test" }]),
        "CreateGlobalNetwork: Tags must be stored, got {network}"
    );
    assert!(
        network["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
        "CreateGlobalNetwork: CreatedAt must be a timestamp, got {network}"
    );
    let created_at = network["CreatedAt"].clone();

    // READ: the stored record, not a fresh one.
    let (status, described) = emulator.get("/global-networks").await;
    assert_eq!(
        status, 200,
        "DescribeGlobalNetworks: expected 200, got {status} with {described}"
    );
    assert_eq!(
        ids_of(&described, "GlobalNetworks", "GlobalNetworkId"),
        vec![network_id.clone()],
        "DescribeGlobalNetworks: expected exactly the created network, got {described}"
    );
    assert_eq!(
        described["GlobalNetworks"][0]["CreatedAt"], created_at,
        "DescribeGlobalNetworks: CreatedAt must be the stored one, got {described}"
    );

    // CREATE a child site, with a Location that must round-trip.
    let location = json!({ "Address": "1 Main St", "Latitude": "47.6", "Longitude": "-122.3" });
    let (status, site_created) = emulator
        .post(
            &format!("/global-networks/{network_id}/sites"),
            json!({ "Description": "hq", "Location": location }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSite: expected 200, got {status} with {site_created}"
    );
    let site = &site_created["Site"];
    let site_id = site["SiteId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSite: no SiteId in {site_created}"))
        .to_string();
    assert!(
        is_minted_id(&site_id, "site"),
        "CreateSite: expected a site-<17 hex> id, got {site_id:?}"
    );
    assert_eq!(
        site["SiteArn"],
        json!(expected_scoped_arn("site", &network_id, &site_id)),
        "CreateSite: ARN must name the global network and the minted id, got {site}"
    );
    assert_eq!(
        site["GlobalNetworkId"],
        json!(network_id.clone()),
        "CreateSite: the site must back-reference its parent, got {site}"
    );
    assert_eq!(
        site["Location"], location,
        "CreateSite: Location must be stored, got {site}"
    );

    let (status, sites) = emulator
        .get(&format!("/global-networks/{network_id}/sites"))
        .await;
    assert_eq!(
        status, 200,
        "GetSites: expected 200, got {status} with {sites}"
    );
    assert_eq!(
        ids_of(&sites, "Sites", "SiteId"),
        vec![site_id.clone()],
        "GetSites: expected exactly the created site, got {sites}"
    );

    // UPDATE mutates the persisted record, and only the supplied member.
    let (status, updated) = emulator
        .patch(
            &format!("/global-networks/{network_id}"),
            json!({ "Description": "demo2" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "UpdateGlobalNetwork: expected 200, got {status} with {updated}"
    );
    assert_eq!(
        updated["GlobalNetwork"]["Description"],
        json!("demo2"),
        "UpdateGlobalNetwork: expected the new description, got {updated}"
    );
    let (_, described) = emulator.get("/global-networks").await;
    assert_eq!(
        described["GlobalNetworks"][0]["Description"],
        json!("demo2"),
        "DescribeGlobalNetworks after update: expected the stored new description, got {described}"
    );
    assert_eq!(
        described["GlobalNetworks"][0]["CreatedAt"], created_at,
        "UpdateGlobalNetwork must not restamp CreatedAt, got {described}"
    );
    assert_eq!(
        described["GlobalNetworks"][0]["Tags"],
        json!([{ "Key": "env", "Value": "test" }]),
        "UpdateGlobalNetwork must leave unsupplied members alone, got {described}"
    );

    // DELETE is refused while the network still owns a site: a dependency refusal, which
    // this service reports as the 400 ValidationException (the children must go first, so
    // retrying this call can never succeed).
    let (status, refused) = emulator
        .delete(&format!("/global-networks/{network_id}"))
        .await;
    assert_eq!(
        status, 400,
        "DeleteGlobalNetwork with a child site: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "DeleteGlobalNetwork with a child site: wrong error code in {refused}"
    );

    // Empty it, then the delete succeeds and the record is gone.
    let (status, deleted_site) = emulator
        .delete(&format!("/global-networks/{network_id}/sites/{site_id}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteSite: expected 200, got {status} with {deleted_site}"
    );
    assert_eq!(
        deleted_site["Site"]["State"],
        json!("DELETING"),
        "DeleteSite: expected the final state DELETING, got {deleted_site}"
    );
    let (_, sites) = emulator
        .get(&format!("/global-networks/{network_id}/sites"))
        .await;
    assert_eq!(
        ids_of(&sites, "Sites", "SiteId"),
        Vec::<String>::new(),
        "GetSites after DeleteSite: expected no sites, got {sites}"
    );

    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{network_id}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {deleted}"
    );
    assert_eq!(
        deleted["GlobalNetwork"]["State"],
        json!("DELETING"),
        "DeleteGlobalNetwork: expected the final state DELETING, got {deleted}"
    );
    assert_eq!(
        deleted["GlobalNetwork"]["GlobalNetworkId"],
        json!(network_id.clone()),
        "DeleteGlobalNetwork: expected the deleted network, got {deleted}"
    );

    let (_, described) = emulator.get("/global-networks").await;
    assert_eq!(
        ids_of(&described, "GlobalNetworks", "GlobalNetworkId"),
        Vec::<String>::new(),
        "DescribeGlobalNetworks after delete: expected no networks, got {described}"
    );

    // Every operation that ACTS ON a global network 404s once it is gone. `CreateSite`
    // only REFERENCES one, so it is refused as a field-validation failure instead (see
    // `unknown_parent_reference`).
    for (case, method, uri) in [
        ("GetSites", "GET", format!("/global-networks/{network_id}/sites")),
        ("UpdateGlobalNetwork", "PATCH", format!("/global-networks/{network_id}")),
        ("DeleteGlobalNetwork", "DELETE", format!("/global-networks/{network_id}")),
        (
            "DeleteSite",
            "DELETE",
            format!("/global-networks/{network_id}/sites/{site_id}"),
        ),
    ] {
        let body = if method == "GET" || method == "DELETE" {
            None
        } else {
            Some(json!({}))
        };
        let (status, answer) = emulator.call(method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404 for a deleted global network, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: wrong error code in {answer}"
        );
    }

    // The create that references the deleted global network is refused as a field failure.
    let (status, answer) = emulator
        .post(
            &format!("/global-networks/{network_id}/sites"),
            json!({ "Description": "orphan" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case CreateSite: expected 400 for a deleted global network, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case CreateSite: wrong error code in {answer}"
    );
}

#[tokio::test]
async fn unknown_global_network_is_not_found_for_every_operation_that_addresses_one() {
    let emulator = Emulator::new();
    // A well-formed id of the right shape that was never created, and one that is not a
    // Network Manager id at all: neither names a stored record, so both are 404 for the
    // operations that ACT ON the global network. `CreateSite` only REFERENCES one, so it is
    // refused as a field-validation failure instead (see `unknown_parent_reference`).
    for unknown in ["global-network-00000000000000000", "nope"] {
        for (case, method, uri) in [
            ("GetSites", "GET", format!("/global-networks/{unknown}/sites")),
            (
                "UpdateGlobalNetwork",
                "PATCH",
                format!("/global-networks/{unknown}"),
            ),
            (
                "DeleteGlobalNetwork",
                "DELETE",
                format!("/global-networks/{unknown}"),
            ),
            (
                "DeleteSite",
                "DELETE",
                format!("/global-networks/{unknown}/sites/site-00000000000000000"),
            ),
        ] {
            let body = if method == "GET" || method == "DELETE" {
                None
            } else {
                Some(json!({}))
            };
            let (status, answer) = emulator.call(method, &uri, body).await;
            assert_eq!(
                status, 404,
                "case {case} on {unknown:?}: expected 404, got {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case} on {unknown:?}: wrong error code in {answer}"
            );
        }
    }
}

#[tokio::test]
async fn describe_global_networks_pages_every_network_exactly_once_for_each_page_size() {
    let emulator = Emulator::new();
    let mut expected = Vec::new();
    for index in 0..5 {
        expected.push(emulator.create_global_network(&format!("net-{index}")).await);
    }
    expected.sort();

    // The rule generalises over the page size, so the rows span it: no `MaxResults` at
    // all, every size below the stored count, the count itself, one past it, and the
    // modelled maximum (`MaxResults` range 1..500).
    for page_size in [None, Some(1), Some(2), Some(4), Some(5), Some(6), Some(500)] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(ref value) = token {
                query.push(format!("nextToken={value}"));
            }
            let uri = if query.is_empty() {
                "/global-networks".to_string()
            } else {
                format!("/global-networks?{}", query.join("&"))
            };
            let (status, body) = emulator.get(&uri).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size:?} page {pages}: expected 200, got {status} with {body}"
            );
            let page = ids_of(&body, "GlobalNetworks", "GlobalNetworkId");
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case MaxResults={page_size:?} page {pages}: a page must not exceed MaxResults, got {} items",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 10,
                "case MaxResults={page_size:?}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case MaxResults={page_size:?}: following NextToken must yield every network exactly once, in key order"
        );
    }
}

#[tokio::test]
async fn get_sites_pages_every_site_of_its_own_global_network() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("paged").await;
    let other = emulator.create_global_network("other").await;
    let mut expected = Vec::new();
    for index in 0..4 {
        expected.push(emulator.create_site(&network, &format!("site-{index}")).await);
    }
    expected.sort();
    // A site of another global network must never show up on these pages.
    emulator.create_site(&other, "elsewhere").await;

    for page_size in [None, Some(1), Some(3), Some(4), Some(5)] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(ref value) = token {
                query.push(format!("nextToken={value}"));
            }
            let uri = if query.is_empty() {
                format!("/global-networks/{network}/sites")
            } else {
                format!("/global-networks/{network}/sites?{}", query.join("&"))
            };
            let (status, body) = emulator.get(&uri).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size:?} page {pages}: expected 200, got {status} with {body}"
            );
            seen.extend(ids_of(&body, "Sites", "SiteId"));
            pages += 1;
            assert!(
                pages <= 10,
                "case MaxResults={page_size:?}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case MaxResults={page_size:?}: GetSites must page exactly its own sites"
        );
    }
}

#[tokio::test]
async fn listings_filter_by_the_ids_the_request_names() {
    let emulator = Emulator::new();
    let first = emulator.create_global_network("first").await;
    let second = emulator.create_global_network("second").await;
    let site_a = emulator.create_site(&first, "a").await;
    let site_b = emulator.create_site(&first, "b").await;
    let foreign_site = emulator.create_site(&second, "foreign").await;

    let mut both_networks = vec![first.clone(), second.clone()];
    both_networks.sort();
    let mut both_sites = vec![site_a.clone(), site_b.clone()];
    both_sites.sort();

    // Rows differ along the number of ids named and whether each names a stored record.
    // `GlobalNetworkIds` SELECTS among the stored global networks, so an id that names
    // nothing is dropped from the selection rather than failing the read (the whole rule,
    // over a wider table, is `describe_global_networks_id_resolution`).
    let unknown_network = "global-network-00000000000000000".to_string();
    let network_cases: Vec<ListingCase> = vec![
        ("no filter", vec![], both_networks.clone()),
        ("one id", vec![first.clone()], vec![first.clone()]),
        ("every id", both_networks.clone(), both_networks.clone()),
        (
            "an id that names nothing",
            vec![unknown_network.clone()],
            vec![],
        ),
        (
            "a known and an unknown id",
            vec![second.clone(), unknown_network.clone()],
            vec![second.clone()],
        ),
    ];
    for (case, filter, want) in network_cases {
        let query: Vec<String> = filter
            .iter()
            .map(|id| format!("globalNetworkIds={id}"))
            .collect();
        let uri = if query.is_empty() {
            "/global-networks".to_string()
        } else {
            format!("/global-networks?{}", query.join("&"))
        };
        let (status, body) = emulator.get(&uri).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            ids_of(&body, "GlobalNetworks", "GlobalNetworkId"),
            want,
            "case {case}: DescribeGlobalNetworks returned the wrong set, got {body}"
        );
    }

    let site_cases: Vec<ListingCase> = vec![
        ("no filter", vec![], both_sites.clone()),
        ("one id", vec![site_b.clone()], vec![site_b.clone()]),
        ("every id", both_sites.clone(), both_sites.clone()),
        (
            "an id that names nothing",
            vec!["site-00000000000000000".to_string()],
            vec![],
        ),
        (
            "a site of another global network",
            vec![foreign_site.clone()],
            vec![],
        ),
    ];
    for (case, filter, want) in site_cases {
        let query: Vec<String> = filter.iter().map(|id| format!("siteIds={id}")).collect();
        let uri = if query.is_empty() {
            format!("/global-networks/{first}/sites")
        } else {
            format!("/global-networks/{first}/sites?{}", query.join("&"))
        };
        let (status, body) = emulator.get(&uri).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            ids_of(&body, "Sites", "SiteId"),
            want,
            "case {case}: GetSites returned the wrong set"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("tokens").await;
    emulator.create_site(&network, "one").await;

    // Every row is a NON-empty value: the empty string is no token at all — it satisfies
    // the modelled `length 0..2048` and names no position, so it asks for the first page
    // (`crate::empty_page_token`) — and `hex::encode("")` is that same empty string.
    let cases: Vec<(&str, String)> = vec![
        ("not hex", "zzzz".to_string()),
        ("an odd number of hex digits", "abc".to_string()),
        (
            "hex of a token another service would mint",
            hex::encode("someone-else:global-network-0"),
        ),
        ("hex of non-utf8 bytes", hex::encode([0xff, 0xfe, 0xfd])),
        ("hex of a prefix with no key", hex::encode("networkmanager:")),
    ];

    for (case, token) in cases {
        for (operation, uri) in [
            (
                "DescribeGlobalNetworks",
                format!("/global-networks?nextToken={token}"),
            ),
            (
                "GetSites",
                format!("/global-networks/{network}/sites?nextToken={token}"),
            ),
            (
                "GetConnections",
                format!("/global-networks/{network}/connections?nextToken={token}"),
            ),
        ] {
            let (status, body) = emulator.get(&uri).await;
            assert_eq!(
                status, 400,
                "case {operation}/{case}: expected 400 for an unparseable NextToken, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {operation}/{case}: wrong error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn created_resources_report_pending_until_the_settle_span_the_clock_owes_elapses() {
    let emulator = Emulator::new();

    let (_, created) = emulator
        .post("/global-networks", json!({ "Description": "settling" }))
        .await;
    let network_id = created["GlobalNetwork"]["GlobalNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in {created}"))
        .to_string();
    assert_eq!(
        created["GlobalNetwork"]["State"],
        json!("PENDING"),
        "CreateGlobalNetwork: a caller acting immediately must still see PENDING, got {created}"
    );

    let (_, site_created) = emulator
        .post(
            &format!("/global-networks/{network_id}/sites"),
            json!({ "Description": "settling" }),
        )
        .await;
    assert_eq!(
        site_created["Site"]["State"],
        json!("PENDING"),
        "CreateSite: a caller acting immediately must still see PENDING, got {site_created}"
    );

    // The transition is owed by the clock, not by the read: nothing polls for it.
    std::thread::sleep(PAST_SETTLE);

    let (_, described) = emulator.get("/global-networks").await;
    assert_eq!(
        described["GlobalNetworks"][0]["State"],
        json!("AVAILABLE"),
        "DescribeGlobalNetworks after the settle span: expected AVAILABLE, got {described}"
    );
    let (_, sites) = emulator
        .get(&format!("/global-networks/{network_id}/sites"))
        .await;
    assert_eq!(
        sites["Sites"][0]["State"],
        json!("AVAILABLE"),
        "GetSites after the settle span: expected AVAILABLE, got {sites}"
    );
}

#[tokio::test]
async fn minted_ids_and_arns_are_unique_and_independent_of_the_request_body() {
    let emulator = Emulator::new();

    // Rows differ along the description's size and form — none of which the id/ARN rule
    // depends on. 256 is the modelled maximum for `Description` (`ConstrainedString`,
    // len 0..256); the row past it is refused below rather than minting anything.
    let cases: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("short", "hq".to_string()),
        ("multi-line", "line one\nline two".to_string()),
        ("unicode", "sïte \u{1f310}".to_string()),
        ("maximum length", "d".repeat(256)),
    ];

    let mut network_ids: Vec<String> = Vec::new();
    let mut site_ids: Vec<String> = Vec::new();
    for (case, description) in &cases {
        let (status, created) = emulator
            .post("/global-networks", json!({ "Description": description }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateGlobalNetwork expected 200, got {status} with {created}"
        );
        let network = &created["GlobalNetwork"];
        let network_id = network["GlobalNetworkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no GlobalNetworkId in {created}"))
            .to_string();
        assert!(
            is_minted_id(&network_id, "global-network"),
            "case {case}: expected a global-network-<17 hex> id, got {network_id:?}"
        );
        assert_eq!(
            network["GlobalNetworkArn"],
            json!(expected_arn("global-network", &network_id)),
            "case {case}: ARN must name the minted id, got {network}"
        );
        assert!(
            !network_ids.contains(&network_id),
            "case {case}: id {network_id:?} was already minted for another network"
        );

        let (status, site_created) = emulator
            .post(
                &format!("/global-networks/{network_id}/sites"),
                json!({ "Description": description }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateSite expected 200, got {status} with {site_created}"
        );
        let site = &site_created["Site"];
        let site_id = site["SiteId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no SiteId in {site_created}"))
            .to_string();
        assert!(
            is_minted_id(&site_id, "site"),
            "case {case}: expected a site-<17 hex> id, got {site_id:?}"
        );
        assert_eq!(
            site["SiteArn"],
            json!(expected_scoped_arn("site", &network_id, &site_id)),
            "case {case}: ARN must name the global network and the minted id, got {site}"
        );
        assert!(
            !site_ids.contains(&site_id),
            "case {case}: id {site_id:?} was already minted for another site"
        );

        // The description the caller sent is what a later read returns.
        let (_, sites) = emulator
            .get(&format!("/global-networks/{network_id}/sites"))
            .await;
        assert_eq!(
            sites["Sites"][0]["Description"],
            json!(description),
            "case {case}: GetSites must return the stored description, got {sites}"
        );

        network_ids.push(network_id);
        site_ids.push(site_id);
    }

    // One past the published bound: refused by the modelled constraint, so nothing is
    // minted and the store does not grow.
    let (status, refused) = emulator
        .post("/global-networks", json!({ "Description": "d".repeat(257) }))
        .await;
    assert_eq!(
        status, 400,
        "case one past the Description maximum: expected 400, got {status} with {refused}"
    );
    let (_, described) = emulator.get("/global-networks?maxResults=500").await;
    let mut listed = ids_of(&described, "GlobalNetworks", "GlobalNetworkId");
    listed.sort();
    let mut expected = network_ids.clone();
    expected.sort();
    assert_eq!(
        listed, expected,
        "a refused CreateGlobalNetwork must not have stored anything"
    );
}

/// The quotas that bound how much state one caller can make this service hold. The caps
/// come from the service's own constants, so this test cannot drift from them, and each
/// is asserted at the cap, one past it, and (for sites) on a sibling of a full parent.
#[tokio::test]
async fn creating_past_a_quota_is_refused_with_the_modelled_error_and_stores_nothing() {
    use service_com_amazonaws_networkmanager_networkmanager::state::{
        MAX_GLOBAL_NETWORKS, MAX_SITES_PER_GLOBAL_NETWORK,
    };

    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..MAX_GLOBAL_NETWORKS {
        created.push(emulator.create_global_network(&format!("net-{index}")).await);
    }
    let (status, refused) = emulator
        .post("/global-networks", json!({ "Description": "one too many" }))
        .await;
    assert_eq!(
        status, 402,
        "CreateGlobalNetwork at the account quota: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "CreateGlobalNetwork at the account quota: wrong error code in {refused}"
    );
    let (_, described) = emulator.get("/global-networks?maxResults=500").await;
    let mut listed = ids_of(&described, "GlobalNetworks", "GlobalNetworkId");
    listed.sort();
    created.sort();
    assert_eq!(
        listed, created,
        "a refused CreateGlobalNetwork must not have stored anything"
    );

    // The site quota is per global network, so a sibling of a full one still has room.
    let emulator = Emulator::new();
    let full = emulator.create_global_network("full").await;
    let sibling = emulator.create_global_network("sibling").await;
    let mut sites: Vec<String> = Vec::new();
    for index in 0..MAX_SITES_PER_GLOBAL_NETWORK {
        sites.push(emulator.create_site(&full, &format!("site-{index}")).await);
    }
    let (status, refused) = emulator
        .post(
            &format!("/global-networks/{full}/sites"),
            json!({ "Description": "one too many" }),
        )
        .await;
    assert_eq!(
        status, 402,
        "CreateSite at the per-global-network quota: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "CreateSite at the per-global-network quota: wrong error code in {refused}"
    );
    let (_, listed_sites) = emulator
        .get(&format!("/global-networks/{full}/sites?maxResults=500"))
        .await;
    let mut listed = ids_of(&listed_sites, "Sites", "SiteId");
    listed.sort();
    sites.sort();
    assert_eq!(
        listed, sites,
        "a refused CreateSite must not have stored anything"
    );

    let (status, accepted) = emulator
        .post(
            &format!("/global-networks/{sibling}/sites"),
            json!({ "Description": "room here" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSite in a sibling of a full global network: expected 200, got {status} with {accepted}"
    );
}
