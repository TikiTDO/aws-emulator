// SPDX-License-Identifier: CC0-1.0

//! What a `nextToken` of this service is a position IN, driven end to end over
//! awsJson1_0 through all five of its `@paginated` operations.
//!
//! The rule under test: a pagination token is an opaque handle a service issues for ONE
//! listing — one operation, at one Region's endpoint, over the items one selection names
//! — and a caller resumes that listing by making "the call again using the returned
//! token with all other arguments unchanged to retrieve the next page" (the `nextToken`
//! doc every `@paginated` operation of this service carries,
//! `smithy_query(query="operation",operation="ListEnvironments")`). So a token this
//! service did not issue FOR the listing it is handed to names no position in it and is
//! refused with an HTTP 400 `ValidationException`, instead of being honoured as a place
//! to start that scan; and a token it really did issue for that listing resumes it
//! exactly where the previous page stopped, until it is older than the documented day
//! ("Each pagination token expires after 24 hours. Using an expired pagination token
//! will return an HTTP 400 InvalidToken error" — this service's model declares no
//! `InvalidToken` shape, so the refusal is carried by its only client-input error, at the
//! documented status).
//!
//! The rows span what the rule generalises over:
//!
//! * the LISTING a token is handed to, as every ordered pair of eleven listings — all
//!   five paginated operations, four of them over two different resources or selections,
//!   and one of them at a second Region's endpoint — so each operation sees both its own
//!   token and a token of every other listing;
//! * the FORM of a token this store never issued: text that is not hex, the empty
//!   token, hex too short to carry an authenticator, a cursor written straight onto the
//!   wire (the shape the issue report cites), a cursor behind a made-up authenticator,
//!   a position authenticated under another store's key, and the listing's own token
//!   with its authenticator stripped, a byte of its authenticator, payload or cursor
//!   changed, a byte dropped or a byte appended;
//! * the AGE of a token this service did issue for the listing, on each side of the
//!   documented 24 hours.
//!
//! The WORDING of the refusal — one fixed message for every token a listing cannot
//! resume — is `tests/all/refusal_wording.rs`'s rule; this suite asserts it through the
//! service's own constant so the two cannot drift apart.
//!
//! The cursor each forged envelope carries sorts ABOVE every item of the listing it is
//! handed to, so a cursor honoured without being checked would answer HTTP 200 with an
//! empty page rather than the refusal asserted here.
//!
//! The harness (`emulator`, `call_in_region`, `create_settled`) is the one
//! `environment_lifecycle` owns and the connector helper the one `entitlement_connector`
//! owns, so every listing is driven the way a client drives it.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::entitlement_connector::connector_in;
use crate::environment_lifecycle::{
    call_in_region, create_settled, emulator, error_type, Emulator, REGION,
};

/// A second Region's endpoint: the same operation there pages over another Region's
/// environments, so it is another listing.
const OTHER_REGION: &str = "us-east-1";

/// The `ConnectorType` values this suite gives an environment, from the catalog this
/// crate generated from the model (`smithy_query(query="shape", shape="ConnectorType")`):
/// two of them, so a connector listing holds more than one page. Only one LIVE connector
/// of a type can exist in an environment, and an entitlement is tracked through the
/// vCenter Server appliance, so `VCENTER` is one of the two.
const SDDC_MANAGER: &str = "SDDC_MANAGER";
/// See [`SDDC_MANAGER`].
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The `EnvironmentState` a settled environment has reached, from the catalog this crate
/// generated from the model (`smithy_query(query="shape", shape="EnvironmentState")`):
/// what the `state` filter of one of the two `ListEnvironments` listings names.
const CREATED: &str = "CREATED";

/// The message a refusal of a `nextToken` a listing cannot resume states — the same one
/// for a token this service never issued and for a token too old to resume, since Amazon
/// EVS reports both with one sentence and one remedy. Read from the service's own
/// constant, so this suite and the handlers cannot drift apart over the wording; that it
/// is AWS's wording is `tests/all/pagination_refusal_message.rs`'s row.
fn refusal_message() -> &'static str {
    evs::state::INVALID_NEXT_TOKEN_MESSAGE
}

/// One listing: an operation, the endpoint it is called at, the request that addresses
/// the collection it pages over, and the scope a token of it belongs to.
struct Listing {
    /// How a case names this listing in an assertion message.
    case: String,
    /// The operation the request targets.
    operation: &'static str,
    /// The output member the page arrives in.
    member: &'static str,
    /// The member of a reported item that identifies it, so two reports of one item
    /// compare even while the clock moves its state on.
    identity: &'static str,
    /// The Region whose endpoint the request is sent to.
    region: &'static str,
    /// Everything the request carries apart from `maxResults` and `nextToken`.
    request: Value,
    /// The request members that SELECT what this listing pages over, in the order the
    /// handler adds them to the scope a token of it is authenticated under.
    selectors: Vec<String>,
    /// A cursor of this listing's own domain that sorts above every item it holds: what
    /// a forged token carries, so honouring it unchecked empties the page.
    beyond_cursor: &'static str,
}

impl Listing {
    /// The scope a token of this listing is authenticated under — the listing's identity
    /// as the service states it. A token this suite mints under this scope is accepted,
    /// which is what ties these rows to the scope the handler itself builds.
    fn scope(&self) -> evs::state::PageScope {
        self.selectors.iter().fold(
            evs::state::PageScope::new(self.operation, self.region),
            |scope, selector| scope.selecting(selector),
        )
    }
}

/// Call one listing, with the page bound and the token the case names.
async fn page(
    router: &Emulator,
    listing: &Listing,
    max_results: Option<i64>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut request = listing.request.clone();
    if let Some(bound) = max_results {
        request["maxResults"] = json!(bound);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    call_in_region(router, listing.region, listing.operation, &request).await
}

/// Every item of a listing's collection, by its identity member, in the order the
/// listing reports them.
async fn identities(router: &Emulator, listing: &Listing) -> Vec<Value> {
    let (status, body) = page(router, listing, None, None).await;
    assert_eq!(
        status, 200,
        "case {}: listing it expected HTTP 200, got {status} ({}) {body}",
        listing.case,
        error_type(&body)
    );
    body[listing.member]
        .as_array()
        .unwrap_or_else(|| panic!("case {}: no {} in {body}", listing.case, listing.member))
        .iter()
        .map(|item| item[listing.identity].clone())
        .collect()
}

/// The `nextToken` a listing hands out for a page of one item, or `None` when its
/// collection holds no second page.
async fn one_item_page_token(router: &Emulator, listing: &Listing) -> Option<String> {
    let (status, body) = page(router, listing, Some(1), None).await;
    assert_eq!(
        status, 200,
        "case {}: a first page of one expected HTTP 200, got {status} ({}) {body}",
        listing.case,
        error_type(&body)
    );
    body["nextToken"].as_str().map(str::to_string)
}

/// The secret the emulator's Amazon EVS store authenticates its own page tokens under:
/// what this suite mints an authentic token with, through the very code a handler uses.
fn page_token_key(router: &Emulator) -> evs::state::PageTokenKey {
    evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store")
        .lock()
        .page_token_key
}

/// The authenticated payload a `nextToken` of this service carries: everything past the
/// authenticator the shared minter puts in front of it.
fn payload_of(case: &str, token: &str) -> Vec<u8> {
    let bytes = ::hex::decode(token)
        .unwrap_or_else(|_| panic!("case {case}: a token of this service is hex-encoded"));
    bytes
        .get(evs::state::AUTHENTICATOR_BYTES..)
        .unwrap_or_else(|| panic!("case {case}: a minted token carries an authenticator"))
        .to_vec()
}

/// `token`'s bytes with the byte at `index` changed: a value with exactly the shape of a
/// minted token that this store did not mint.
fn with_a_changed_byte(case: &str, token: &str, index: usize) -> String {
    let mut bytes = ::hex::decode(token)
        .unwrap_or_else(|_| panic!("case {case}: a token of this service is hex-encoded"));
    let byte = bytes
        .get_mut(index)
        .unwrap_or_else(|| panic!("case {case}: a minted token has a byte {index}"));
    *byte ^= 0x01;
    ::hex::encode(bytes)
}

/// What every refusal of a token asserts: the documented status, the modelled error, the
/// sentence the refusal states against the member it names, and no page at all.
fn assert_refused(case: &str, listing: &Listing, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} ({}) {body}",
        error_type(body)
    );
    assert!(
        error_type(body).contains("ValidationException"),
        "case {case}: expected a ValidationException, got {} in {body}",
        error_type(body)
    );
    let sentence = refusal_message();
    assert_eq!(
        body["message"].as_str(),
        Some(sentence),
        "case {case}: expected the message {sentence:?}, got {}",
        body["message"]
    );
    assert_eq!(
        body["fieldList"][0]["name"],
        json!("nextToken"),
        "case {case}: expected the refusal reported against nextToken, got {}",
        body["fieldList"]
    );
    assert_eq!(
        body[listing.member],
        Value::Null,
        "case {case}: a refused request reports no page, got {}",
        body[listing.member]
    );
}

/// What a token accepted by its own listing asserts: the page it resumes starts at the
/// item the page before it stopped at.
fn assert_resumed(case: &str, listing: &Listing, status: u16, body: &Value, expected: &Value) {
    assert_eq!(
        status, 200,
        "case {case}: expected HTTP 200, got {status} ({}) {body}",
        error_type(body)
    );
    assert_eq!(
        &body[listing.member][0][listing.identity],
        expected,
        "case {case}: a resumed page starts at the item the page before it stopped at, \
         expected {expected}, got {}",
        body[listing.member][0][listing.identity]
    );
}

/// The eleven listings this suite crosses tokens between: every `@paginated` operation
/// of this service, four of them over two different resources or selections, and
/// `ListEnvironments` also at a second Region's endpoint — where this account holds no
/// environment, so a token of the first Region's listing could only be honoured there by
/// answering an empty page.
///
/// Each collection holds at least two items, so each listing really does hand out a
/// token of its own.
async fn listings(router: &Emulator) -> Vec<Listing> {
    let mut environments: Vec<String> = Vec::new();
    let mut vcenters: Vec<String> = Vec::new();
    for label in ["first", "second"] {
        // Settled, because a list addressed to an environment that is still initializing
        // is refused (`tests/all/list_while_initializing.rs` owns that rule), and because
        // a host can only join a deployed environment.
        let environment = create_settled(router, Some(&format!("token-{label}"))).await;
        for index in 1..=2 {
            // "DNS hostnames for hosts must be unique across Amazon EVS environments"
            // (`Host$hostName` doc), so each environment's hosts carry its own label.
            let host_name = format!("esx-{label}-{index}");
            let (status, body) = call_in_region(
                router,
                REGION,
                "CreateEnvironmentHost",
                &json!({
                    "environmentId": environment,
                    "host": {
                        "hostName": host_name,
                        "keyName": "evs-key",
                        "instanceType": "i4i.metal",
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "host {host_name}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
        }
        for connector_type in [SDDC_MANAGER, VCENTER] {
            let connector = connector_in(router, label, &environment, connector_type).await;
            if connector_type == VCENTER {
                let (status, body) = call_in_region(
                    router,
                    REGION,
                    "CreateEntitlement",
                    &json!({
                        "environmentId": environment,
                        "connectorId": connector,
                        "entitlementType": WINDOWS_SERVER,
                        // The modelled `VmId` pattern is `^vm-[0-9]+$`.
                        "vmIds": [format!("vm-{}1", environments.len() + 1),
                                  format!("vm-{}2", environments.len() + 1)],
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "the {label} entitlements: expected HTTP 200, got {status} ({}) {body}",
                    error_type(&body)
                );
                vcenters.push(connector);
            }
        }
        environments.push(environment);
    }

    let mut listings = vec![
        Listing {
            case: "ListEnvironments".to_string(),
            operation: "ListEnvironments",
            member: "environmentSummaries",
            identity: "environmentId",
            region: REGION,
            request: json!({}),
            selectors: Vec::new(),
            beyond_cursor: "env-zzzzzzzzzz",
        },
        Listing {
            case: "ListEnvironments filtered to CREATED".to_string(),
            operation: "ListEnvironments",
            member: "environmentSummaries",
            identity: "environmentId",
            region: REGION,
            request: json!({ "state": [CREATED] }),
            selectors: vec![CREATED.to_string()],
            beyond_cursor: "env-zzzzzzzzzz",
        },
        Listing {
            case: "ListEnvironments at another Region's endpoint".to_string(),
            operation: "ListEnvironments",
            member: "environmentSummaries",
            identity: "environmentId",
            region: OTHER_REGION,
            request: json!({}),
            selectors: Vec::new(),
            beyond_cursor: "env-zzzzzzzzzz",
        },
    ];
    for (index, environment) in environments.iter().enumerate() {
        let which = if index == 0 { "the first" } else { "the second" };
        listings.extend([
            Listing {
                case: format!("ListEnvironmentHosts of {which} environment"),
                operation: "ListEnvironmentHosts",
                member: "environmentHosts",
                identity: "hostName",
                region: REGION,
                request: json!({ "environmentId": environment }),
                selectors: vec![environment.clone()],
                beyond_cursor: "esx-zzzz",
            },
            Listing {
                case: format!("ListEnvironmentConnectors of {which} environment"),
                operation: "ListEnvironmentConnectors",
                member: "connectors",
                identity: "connectorId",
                region: REGION,
                request: json!({ "environmentId": environment }),
                selectors: vec![environment.clone()],
                beyond_cursor: "cnctr-zzzzzzzzzz",
            },
            Listing {
                case: format!("ListEnvironmentVlans of {which} environment"),
                operation: "ListEnvironmentVlans",
                member: "environmentVlans",
                identity: "vlanId",
                region: REGION,
                request: json!({ "environmentId": environment }),
                selectors: vec![environment.clone()],
                // A VLAN cursor is a VLAN id, so one above every id this service
                // provisions; the handler reads it as the `VlanId` integer it is.
                beyond_cursor: "4094",
            },
            Listing {
                case: format!("ListVmEntitlements of {which} environment"),
                operation: "ListVmEntitlements",
                member: "entitlements",
                identity: "vmId",
                region: REGION,
                request: json!({
                    "environmentId": environment,
                    "connectorId": vcenters[index],
                    "entitlementType": WINDOWS_SERVER,
                }),
                selectors: vec![
                    environment.clone(),
                    vcenters[index].clone(),
                    WINDOWS_SERVER.to_string(),
                ],
                beyond_cursor: "cnctr-zzzzzzzzzz/vm-999999",
            },
        ]);
    }
    listings
}

#[tokio::test]
async fn a_token_resumes_the_listing_that_issued_it_and_no_other() {
    let router = emulator();
    let listings = listings(&router).await;

    // What each listing reports, and the token it hands out for a page of one — absent
    // only for the listing of a Region this account holds no environment in.
    let mut items: Vec<Vec<Value>> = Vec::new();
    let mut tokens: Vec<Option<String>> = Vec::new();
    for listing in &listings {
        items.push(identities(&router, listing).await);
        tokens.push(one_item_page_token(&router, listing).await);
    }
    assert_eq!(
        tokens.iter().filter(|token| token.is_some()).count(),
        listings.len() - 1,
        "the fixture: every listing but the empty one pages, got {tokens:?}"
    );

    for (source_index, source) in listings.iter().enumerate() {
        let Some(token) = tokens[source_index].as_deref() else {
            continue;
        };
        for (target_index, target) in listings.iter().enumerate() {
            let case = format!("the {} token handed to {}", source.case, target.case);
            let (status, body) = page(&router, target, Some(1), Some(token)).await;
            if target_index == source_index {
                let expected = items[target_index]
                    .get(1)
                    .unwrap_or_else(|| panic!("case {case}: the fixture holds a second item"));
                assert_resumed(&case, target, status, &body, expected);
            } else {
                assert_refused(&case, target, status, &body);
            }
        }
    }
}

#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused_by_every_listing() {
    let router = emulator();
    let listings = listings(&router).await;

    // A key no store of this emulator holds: what another emulator process, or a
    // restarted one, would have minted its tokens under.
    let another_store = evs::state::PageTokenKey::new();

    for listing in &listings {
        let beyond = listing.beyond_cursor;
        // Every form of a token this store never issued, each carrying a cursor that
        // sorts above every item of this listing.
        let mut rows: Vec<(String, String)> = vec![
            ("text that is not hex".to_string(), "not-a-token".to_string()),
            ("the empty token".to_string(), String::new()),
            (
                "hex too short to carry an authenticator".to_string(),
                "abcdef".to_string(),
            ),
            (
                "hex of a cursor with no authenticator at all".to_string(),
                ::hex::encode(beyond),
            ),
            (
                "hex of a zero authenticator and a cursor".to_string(),
                ::hex::encode(
                    [
                        vec![0_u8; evs::state::AUTHENTICATOR_BYTES].as_slice(),
                        beyond.as_bytes(),
                    ]
                    .concat(),
                ),
            ),
            (
                "this listing's own position authenticated under another store's key"
                    .to_string(),
                evs::state::encode_page_token(&another_store, &listing.scope(), beyond),
            ),
        ];
        if let Some(token) = one_item_page_token(&router, listing).await {
            let payload = payload_of(&listing.case, &token);
            let bytes = payload.len() + evs::state::AUTHENTICATOR_BYTES;
            rows.extend([
                (
                    "this listing's own payload with its authenticator stripped".to_string(),
                    ::hex::encode(&payload),
                ),
                (
                    "this listing's own token with a byte of its authenticator changed"
                        .to_string(),
                    with_a_changed_byte(&listing.case, &token, 0),
                ),
                (
                    "this listing's own token with the first byte of its payload changed"
                        .to_string(),
                    with_a_changed_byte(&listing.case, &token, evs::state::AUTHENTICATOR_BYTES),
                ),
                (
                    "this listing's own token with the last byte of its cursor changed"
                        .to_string(),
                    with_a_changed_byte(&listing.case, &token, bytes - 1),
                ),
                (
                    "this listing's own token with its last byte dropped".to_string(),
                    token[..token.len() - 2].to_string(),
                ),
                (
                    "this listing's own token with a byte appended".to_string(),
                    format!("{token}2a"),
                ),
            ]);
        }

        for (form, token) in rows {
            let case = format!("{form}, handed to {}", listing.case);
            let (status, body) = page(&router, listing, Some(1), Some(&token)).await;
            assert_refused(&case, listing, status, &body);
        }
    }
}

#[tokio::test]
async fn a_token_resumes_its_listing_until_the_documented_day_is_up() {
    let router = emulator();
    let listings = listings(&router).await;
    let key = page_token_key(&router);
    let ttl_ms = evs::state::PAGE_TOKEN_TTL.as_millis() as i64;
    // How far inside the window the row that is still usable sits: enough that it stays
    // inside it while the suite runs, since the age is measured when the request arrives.
    let margin_ms = 5_000;
    let hour_ms = 60 * 60 * 1_000;
    // Each row: how long before now the token was issued, and whether it still resumes.
    let rows: [(&str, i64, bool); 5] = [
        ("issued now", 0, true),
        ("issued an hour ago", hour_ms, true),
        ("issued just inside the day", ttl_ms - margin_ms, true),
        ("issued just past the day", ttl_ms + 1, false),
        ("issued a week ago", 7 * 24 * hour_ms, false),
    ];

    for listing in &listings {
        let Some(token) = one_item_page_token(&router, listing).await else {
            continue;
        };
        // The position that token names, read back through the very decoder the handler
        // uses: that it decodes at all under the scope this suite builds is what ties
        // these rows to the scope the handler itself builds.
        let cursor = evs::state::decode_page_token(&key, &listing.scope(), &token)
            .unwrap_or_else(|error| {
                panic!(
                    "case {}: this listing's own token must name a position, got {error:?}",
                    listing.case
                )
            });
        let second = identities(&router, listing)
            .await
            .get(1)
            .cloned()
            .unwrap_or_else(|| panic!("case {}: the fixture holds a second item", listing.case));

        for (age, age_ms, resumes) in rows {
            let case = format!("a token of {} {age}", listing.case);
            // Authentic in every way but its age: minted under this store's key and this
            // listing's scope, at the instant the row names.
            let aged = evs::state::encode_page_token_issued_at(
                &key,
                &listing.scope(),
                &cursor,
                evs::state::now_ms() - age_ms,
            );
            let (status, body) = page(&router, listing, Some(1), Some(&aged)).await;
            if resumes {
                assert_resumed(&case, listing, status, &body, &second);
            } else {
                assert_refused(&case, listing, status, &body);
            }
        }
    }
}
