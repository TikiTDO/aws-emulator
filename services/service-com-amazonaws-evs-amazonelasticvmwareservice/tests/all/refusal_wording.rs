// SPDX-License-Identifier: CC0-1.0

//! How Amazon EVS WORDS two of its refusals, driven end to end over awsJson1_0.
//!
//! Both rules under test are about the text of a `ValidationException` the service
//! itself words (not the envelope the boundary validator states for a modelled
//! constraint — `tests/all/validation_message_rendering.rs` owns that one):
//!
//! * A `nextToken` a paginated read cannot resume is refused with ONE fixed sentence,
//!   whatever made the token unusable and whichever listing it was handed to: "The
//!   provided nextToken is malformed or has expired. Please retry with a valid token or
//!   start a new list operation." It names the member in its own words, echoes no token
//!   back, and carries no per-failure envelope in front of it. The model declares
//!   `ValidationException` (HTTP 400) as the only client-input error of every
//!   `@paginated` operation of this service
//!   (`smithy_query(query="operation", operation="ListEnvironments")`) and documents no
//!   structure for a token, so the refusal is that error, at that status, with that
//!   message.
//! * A setting outside the domain `AccountSetting$name` documents is refused by naming
//!   the SETTING — `Setting '<name>' must not exceed <maximum>.`, the name quoted as the
//!   request gave it and the maximum taken from that setting's own documentation
//!   ("`vcfPortedCoreCount` (type: numeric string) - … The maximum value is 1,000,000
//!   cores", `smithy_query(query="doc", shape="AccountSetting", member="name")`) — and
//!   never by naming the wire member path of the list element the setting arrived in,
//!   which shifts with the order a request happens to list its settings in.
//!
//! The rows differ along what each rule generalises over: for the token, the listing it
//! is handed to (all five `@paginated` operations), the FORM of the token and the REASON
//! it cannot be resumed (never issued, or issued and then expired — one message for
//! both); for the setting, which documented setting is written, how far past its maximum
//! the value is, how the value fails to be of the documented type, which POSITION in the
//! request list carries it, and how many settings fail in one call.
//!
//! The harness (`emulator`, `call`, `create_settled`) is the one `environment_lifecycle`
//! owns and the connector helper the one `entitlement_connector` owns, so every call is
//! driven the way a client drives it.

use serde_json::{json, Value};

use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::entitlement_connector::connector_in;
use crate::environment_lifecycle::{call, create_settled, emulator, error_type, Emulator, REGION};

/// The message Amazon EVS states for a `nextToken` a listing cannot resume. Fixed text in
/// full: written out here rather than read from the service, so this suite pins the
/// wording instead of agreeing with whatever the service happens to spell.
const INVALID_NEXT_TOKEN: &str = "The provided nextToken is malformed or has expired. \
     Please retry with a valid token or start a new list operation.";

/// The `ValidationExceptionReason` a refusal of one input member carries
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`: `unknownOperation`,
/// `cannotParse`, `fieldValidationFailed`, `other`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// What the top-level `message` of an aggregated refusal of this service opens with, and
/// what joins its clauses — the envelope the account-setting refusal is stated in, and the
/// one the pagination refusal is NOT.
const REFUSAL_PREFIX: &str = "Request validation failure: ";
/// See [`REFUSAL_PREFIX`].
const CLAUSE_JOINER: &str = " | ";

/// The `ConnectorType` an entitlement is tracked through, and the only `EntitlementType`
/// the model declares — what the `ListVmEntitlements` listing is addressed with.
const VCENTER: &str = "VCENTER";
/// See [`VCENTER`].
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// One paginated listing of this service: the operation, the request that addresses the
/// collection it pages over, the member its page arrives in, the request members that
/// select what it pages over (the scope a token of it is authenticated under), and a
/// cursor of its own domain.
struct Listing {
    operation: &'static str,
    member: &'static str,
    request: Value,
    selectors: Vec<String>,
    cursor: &'static str,
}

impl Listing {
    /// The scope a token of this listing is authenticated under, built the way the
    /// handler builds it.
    fn scope(&self) -> evs::state::PageScope {
        self.selectors.iter().fold(
            evs::state::PageScope::new(self.operation, REGION),
            |scope, selector| scope.selecting(selector),
        )
    }
}

/// Every `@paginated` operation of this service, each addressed to a collection this
/// account really holds.
async fn listings(router: &Emulator) -> Vec<Listing> {
    let environment = create_settled(router, Some("refusal-wording")).await;
    let connector = connector_in(router, "refusal wording", &environment, VCENTER).await;
    vec![
        Listing {
            operation: "ListEnvironments",
            member: "environmentSummaries",
            request: json!({}),
            selectors: Vec::new(),
            cursor: "env-zzzzzzzzzz",
        },
        Listing {
            operation: "ListEnvironmentHosts",
            member: "environmentHosts",
            request: json!({ "environmentId": environment }),
            selectors: vec![environment.clone()],
            cursor: "esx-zzzz",
        },
        Listing {
            operation: "ListEnvironmentConnectors",
            member: "connectors",
            request: json!({ "environmentId": environment }),
            selectors: vec![environment.clone()],
            cursor: "cnctr-zzzzzzzzzz",
        },
        Listing {
            operation: "ListEnvironmentVlans",
            member: "environmentVlans",
            request: json!({ "environmentId": environment }),
            selectors: vec![environment.clone()],
            // A VLAN cursor is the `VlanId` integer the next page starts at.
            cursor: "4094",
        },
        Listing {
            operation: "ListVmEntitlements",
            member: "entitlements",
            request: json!({
                "environmentId": environment,
                "connectorId": connector,
                "entitlementType": WINDOWS_SERVER,
            }),
            selectors: vec![
                environment.clone(),
                connector.clone(),
                WINDOWS_SERVER.to_string(),
            ],
            cursor: "cnctr-zzzzzzzzzz/vm-999999",
        },
    ]
}

/// The secret the emulator's Amazon EVS store authenticates its own page tokens under —
/// what an AUTHENTIC token of a listing has to be minted with.
fn page_token_key(router: &Emulator) -> evs::state::PageTokenKey {
    evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store")
        .lock()
        .page_token_key
}

#[tokio::test]
async fn every_paginated_read_states_one_message_for_a_token_it_cannot_resume() {
    let router = emulator();
    let listings = listings(&router).await;
    let key = page_token_key(&router);
    let ttl_ms = evs::state::PAGE_TOKEN_TTL.as_millis() as i64;

    for listing in &listings {
        // The positive control: this listing really is reachable, so each row below is
        // the refusal of a token and not of the request around it.
        let (status, body) = call(&router, listing.operation, &listing.request).await;
        assert_eq!(
            status, 200,
            "case {:?} with no token: expected HTTP 200, got {status} ({}) {body}",
            listing.operation,
            error_type(&body)
        );

        // Why a token cannot be resumed is what these rows span: a token this service
        // never issued, in every form such a value takes, and a token it DID issue for
        // this very listing and then let expire. Amazon EVS answers one message for all
        // of them.
        let rows: Vec<(String, String)> = vec![
            (
                "text that is not a token at all".to_string(),
                "sim2real-not-a-real-token".to_string(),
            ),
            ("the empty token".to_string(), String::new()),
            (
                "hex too short to carry an authenticator".to_string(),
                "abcdef".to_string(),
            ),
            (
                "a cursor written straight onto the wire".to_string(),
                ::hex::encode(listing.cursor),
            ),
            (
                "a cursor behind a made-up authenticator".to_string(),
                ::hex::encode(
                    [
                        vec![0_u8; evs::state::AUTHENTICATOR_BYTES].as_slice(),
                        listing.cursor.as_bytes(),
                    ]
                    .concat(),
                ),
            ),
            (
                "a position authenticated under another store's key".to_string(),
                evs::state::encode_page_token(
                    &evs::state::PageTokenKey::new(),
                    &listing.scope(),
                    listing.cursor,
                ),
            ),
            (
                "a token of this listing issued just past the documented day".to_string(),
                evs::state::encode_page_token_issued_at(
                    &key,
                    &listing.scope(),
                    listing.cursor,
                    evs::state::now_ms() - ttl_ms - 1,
                ),
            ),
            (
                "a token of this listing issued a week ago".to_string(),
                evs::state::encode_page_token_issued_at(
                    &key,
                    &listing.scope(),
                    listing.cursor,
                    evs::state::now_ms() - 7 * 24 * 60 * 60 * 1_000,
                ),
            ),
        ];

        for (form, token) in rows {
            let case = format!("{form}, handed to {}", listing.operation);
            let mut request = listing.request.clone();
            request["nextToken"] = json!(token);
            let (status, body) = call(&router, listing.operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(INVALID_NEXT_TOKEN),
                "case {case:?}: expected the message {INVALID_NEXT_TOKEN:?}, got {}",
                body["message"]
            );
            assert!(
                !body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(REFUSAL_PREFIX),
                "case {case:?}: this refusal states no per-member envelope in front of \
                 its message, got {}",
                body["message"]
            );
            assert_eq!(
                body["reason"].as_str(),
                Some(FIELD_VALIDATION_FAILED),
                "case {case:?}: expected the reason {FIELD_VALIDATION_FAILED:?}, got {}",
                body["reason"]
            );
            assert_eq!(
                body["fieldList"][0]["name"].as_str(),
                Some("nextToken"),
                "case {case:?}: expected the refusal reported against nextToken, got {}",
                body["fieldList"]
            );
            assert_eq!(
                body[listing.member],
                Value::Null,
                "case {case:?}: a refused request reports no page, got {}",
                body[listing.member]
            );
        }
    }
}

/// The clause Amazon EVS states for a value above the maximum a setting's own
/// documentation gives it.
fn above_maximum(name: &str, maximum: u64) -> String {
    format!("Setting '{name}' must not exceed {maximum}.")
}

/// The clause for a value that is not of the TYPE the setting is documented with.
fn not_the_documented_type(name: &str) -> String {
    format!("Setting '{name}' must be a numeric string.")
}

/// One `PutAccountSettings` call outside the documented domain: the settings it carries,
/// and the clauses its refusal must state, in request order.
struct Row {
    case: String,
    settings: Vec<Value>,
    clauses: Vec<String>,
}

/// A `PutAccountSettings` body carrying `settings`.
fn put(settings: &[Value]) -> Value {
    json!({ "settings": settings })
}

/// One setting, as the request spells it.
fn pair(name: &str, value: &str) -> Value {
    json!({ "name": name, "value": value })
}

/// What `GetAccountSettings` reports for the Region the caller addressed, as name/value
/// pairs in the order the service reported them.
async fn held(router: &Emulator, case: &str) -> Vec<(String, String)> {
    let (status, body) = call(router, "GetAccountSettings", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the read-back, got {status} {body}"
    );
    body["settings"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: GetAccountSettings reports a list, got {body}"))
        .iter()
        .map(|entry| {
            (
                entry["name"].as_str().unwrap_or_default().to_string(),
                entry["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_setting_outside_its_documented_domain_is_refused_by_the_setting_it_names() {
    let router = emulator();
    // The documented catalog, so a setting the model documents later is covered by the
    // same rows: one per setting, at its OWN maximum.
    let catalog = evs::state::ACCOUNT_SETTINGS;
    assert!(
        !catalog.is_empty(),
        "the fixture: the model documents at least one account setting"
    );

    // One value inside the domain first, so every refusal below is checked against a
    // Region that already holds a setting: a refused put must leave it exactly as it was.
    const HELD: &str = "42";
    let stored: Vec<Value> = catalog
        .iter()
        .map(|spec| pair(spec.name, HELD))
        .collect();
    let (status, body) = call(&router, "PutAccountSettings", &put(&stored)).await;
    assert_eq!(
        status, 200,
        "the accepted write: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let before = held(&router, "the accepted write").await;
    let expected_held: Vec<(String, String)> = catalog
        .iter()
        .map(|spec| (spec.name.to_string(), HELD.to_string()))
        .collect();
    assert_eq!(
        before, expected_held,
        "the accepted write: expected the Region to hold {expected_held:?}, got {before:?}"
    );

    let mut rows: Vec<Row> = Vec::new();
    for spec in catalog {
        let name = spec.name;
        let maximum = spec.maximum;
        // How far past the maximum, and how the value fails to be a numeric string.
        for (label, value) in [
            ("one past the maximum", (maximum + 1).to_string()),
            ("ten times the maximum", (maximum * 10).to_string()),
            (
                "more digits than any integer type holds",
                "9".repeat(128),
            ),
        ] {
            rows.push(Row {
                case: format!("{name} with a value {label}"),
                settings: vec![pair(name, &value)],
                clauses: vec![above_maximum(name, maximum)],
            });
        }
        for (label, value) in [
            ("a word", "manycores"),
            ("a number with a unit", "100cores"),
            ("an exponent", "1e6"),
        ] {
            rows.push(Row {
                case: format!("{name} with {label} for a value"),
                settings: vec![pair(name, value)],
                clauses: vec![not_the_documented_type(name)],
            });
        }
        // The POSITION the bad setting arrives in says nothing about how the refusal is
        // worded: the clause names the setting either way.
        let past = (maximum + 1).to_string();
        rows.push(Row {
            case: format!("{name} past its maximum in the second position"),
            settings: vec![pair(name, HELD), pair(name, &past)],
            clauses: vec![above_maximum(name, maximum)],
        });
        rows.push(Row {
            case: format!("{name} past its maximum in the fifth position"),
            settings: vec![
                pair(name, HELD),
                pair(name, HELD),
                pair(name, HELD),
                pair(name, HELD),
                pair(name, &past),
            ],
            clauses: vec![above_maximum(name, maximum)],
        });
        // Two failing settings in one call: one clause each, joined in request order.
        rows.push(Row {
            case: format!("{name} failing twice in one call"),
            settings: vec![pair(name, "notanumber"), pair(name, &past)],
            clauses: vec![
                not_the_documented_type(name),
                above_maximum(name, maximum),
            ],
        });
    }
    // The setting and the maximum the model's own documentation names, written out rather
    // than read from the catalog: the published bound, the value one past it, and the
    // refusal AWS words for it.
    rows.push(Row {
        case: "the documented setting one core past its documented maximum".to_string(),
        settings: vec![pair("vcfPortedCoreCount", "1000001")],
        clauses: vec!["Setting 'vcfPortedCoreCount' must not exceed 1000000.".to_string()],
    });

    for row in rows {
        let Row {
            case,
            settings,
            clauses,
        } = row;
        let (status, body) = call(&router, "PutAccountSettings", &put(&settings)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a setting outside its documented domain must be refused with \
             HTTP 400, got {status} ({}) {body}",
            error_type(&body)
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let expected = format!("{REFUSAL_PREFIX}{}", clauses.join(CLAUSE_JOINER));
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {}",
            body["message"]
        );
        assert_eq!(
            body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected the reason {FIELD_VALIDATION_FAILED:?}, got {}",
            body["reason"]
        );
        // Refused, so nothing was written: the Region still holds exactly what it did.
        let after = held(&router, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused put stores nothing, expected {before:?}, got {after:?}"
        );
    }

    // The other side of the rule: a value AT the documented maximum is inside the domain,
    // so it is stored and read back.
    for spec in catalog {
        let case = format!("{} at its documented maximum", spec.name);
        let maximum = spec.maximum.to_string();
        let (status, body) = call(
            &router,
            "PutAccountSettings",
            &put(&[pair(spec.name, &maximum)]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value at the maximum must be accepted, got {status} ({}) {body}",
            error_type(&body)
        );
        let after = held(&router, &case).await;
        assert!(
            after.contains(&(spec.name.to_string(), maximum.clone())),
            "case {case:?}: expected the Region to hold {maximum:?}, got {after:?}"
        );
    }
}
