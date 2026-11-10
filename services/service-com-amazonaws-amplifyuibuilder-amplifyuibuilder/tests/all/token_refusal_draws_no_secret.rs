// SPDX-License-Identifier: CC0-1.0

//! The two token handlers of this service — `ExchangeCodeForToken` and `RefreshToken` —
//! and the order in which each settles the session-store bound against drawing the secret
//! material it hands back.
//!
//! The rules under test:
//!
//! 1. A token request is settled against the bound on the session store BEFORE any secret
//!    material is drawn for it, so a request the bound refuses draws none: the mint is a
//!    `create` followed by a `delete` on the emulator's shared credential store (`state::
//!    mint_provider_token`), which is work a refused request must not do. The rule holds
//!    for every form of request either handler serves — one addressing a session the store
//!    already holds, one naming a session it does not, and one that starts a session it
//!    cannot already hold — and it is independent of what the credential store would then
//!    have drawn.
//! 2. On the admitted path the draw still happens, exactly once, and a draw that yields no
//!    secret is the server-side condition rather than a refusal of the request.
//! 3. The order leaves every response alone: a token request past the bound is still
//!    refused with HTTP 400 naming no error type, a refusal is answered identically however
//!    often it is repeated, it leaves the store exactly as it was, and a session the store
//!    already holds still refreshes at the bound and takes a freshly drawn access token.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    service_state, ComAmazonawsAmplifyuibuilderAmplifyuibuilderState, ProviderTokenKey,
    ProviderTokenRefusal, PROVIDER_TOKEN_SESSION_QUOTA,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The only `TokenProviders` value the model declares
/// (`smithy_query(query="shape",shape="TokenProviders")`).
const PROVIDER: &str = "figma";

/// The response header restJson1 resolves an error shape from. It must be absent from
/// every refusal this service answers.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// What the credential store behind the seam reports as drawn, when the table says it
/// draws: the test stands in for the credential store, so no secret material is involved.
const DRAWN: &str = "drawn";

/// Which session a token request addresses, as the store is asked about it.
#[derive(Clone, Copy, Debug)]
enum Addressed {
    /// A refresh of a session the store already holds.
    Held,
    /// A refresh naming a session the store holds none of.
    Fresh,
    /// A refresh naming a held session's refresh token for ANOTHER client, which is a
    /// session of its own.
    HeldTokenAnotherClient,
    /// An exchange: it starts a session named by a refresh token it has not drawn yet, so
    /// it has no key to ask about.
    StartsOne,
}

/// The key of the session preloaded first, which [`Addressed::Held`] addresses.
fn held_key() -> ProviderTokenKey {
    preloaded_key(0)
}

/// The key of the `index`th preloaded session.
fn preloaded_key(index: usize) -> ProviderTokenKey {
    (
        PROVIDER.to_string(),
        Some("amplify-studio".to_string()),
        format!("figd_preloaded_{index}"),
    )
}

/// One row of the table below: a store state, the request addressed against it, what the
/// credential store behind the seam would draw, and what the store must then settle.
struct Row {
    case: &'static str,
    /// How many live sessions the store holds when the request arrives.
    live: usize,
    addressed: Addressed,
    /// Whether the credential store behind the seam draws a secret when asked.
    draws: bool,
    settled: Result<&'static str, ProviderTokenRefusal>,
    /// How many times the store may ask the credential store to draw.
    draws_asked_for: usize,
}

/// A store holding `live` sessions, each with a live access token, written through the one
/// write both token handlers use.
fn store_holding(live: usize) -> ComAmazonawsAmplifyuibuilderAmplifyuibuilderState {
    let mut store = ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::default();
    let now = SystemTime::now();
    for index in 0..live {
        store.refresh_provider_token_session(
            preloaded_key(index),
            format!("preloaded-access-token-{index}"),
            now,
        );
    }
    store
}

/// Rules 1 and 2: the bound is settled first, so the draw happens on the admitted path
/// alone — whatever the request addresses, however full the store is, and whatever the
/// credential store would have drawn.
#[test]
fn the_bound_is_settled_before_any_secret_material_is_drawn() {
    let quota = PROVIDER_TOKEN_SESSION_QUOTA;
    // The rows differ along what the rule generalises over: how full the store is (empty,
    // one session short of the bound, at it), which session the request addresses (one held,
    // one that is not, one differing from a held session only in its client, and an exchange
    // that has no key yet), and whether the credential store would draw a secret at all.
    let rows = [
        Row {
            case: "an empty store, a refresh naming a session it does not hold",
            live: 0,
            addressed: Addressed::Fresh,
            draws: true,
            settled: Ok(DRAWN),
            draws_asked_for: 1,
        },
        Row {
            case: "an empty store, an exchange",
            live: 0,
            addressed: Addressed::StartsOne,
            draws: true,
            settled: Ok(DRAWN),
            draws_asked_for: 1,
        },
        Row {
            case: "one session short of the bound, a refresh of a session it does not hold",
            live: quota - 1,
            addressed: Addressed::Fresh,
            draws: true,
            settled: Ok(DRAWN),
            draws_asked_for: 1,
        },
        Row {
            case: "one session short of the bound, an exchange",
            live: quota - 1,
            addressed: Addressed::StartsOne,
            draws: true,
            settled: Ok(DRAWN),
            draws_asked_for: 1,
        },
        Row {
            case: "at the bound, a refresh of a session it holds",
            live: quota,
            addressed: Addressed::Held,
            draws: true,
            settled: Ok(DRAWN),
            draws_asked_for: 1,
        },
        Row {
            case: "at the bound, a refresh naming a session it does not hold",
            live: quota,
            addressed: Addressed::Fresh,
            draws: true,
            settled: Err(ProviderTokenRefusal::OverQuota),
            draws_asked_for: 0,
        },
        Row {
            case: "at the bound, a held refresh token presented by another client",
            live: quota,
            addressed: Addressed::HeldTokenAnotherClient,
            draws: true,
            settled: Err(ProviderTokenRefusal::OverQuota),
            draws_asked_for: 0,
        },
        Row {
            case: "at the bound, an exchange",
            live: quota,
            addressed: Addressed::StartsOne,
            draws: true,
            settled: Err(ProviderTokenRefusal::OverQuota),
            draws_asked_for: 0,
        },
        Row {
            case: "at the bound, a refusal a drawing credential store would not change",
            live: quota,
            addressed: Addressed::Fresh,
            draws: false,
            settled: Err(ProviderTokenRefusal::OverQuota),
            draws_asked_for: 0,
        },
        Row {
            case: "an empty store, a credential store that draws nothing",
            live: 0,
            addressed: Addressed::Fresh,
            draws: false,
            settled: Err(ProviderTokenRefusal::NothingMinted),
            draws_asked_for: 1,
        },
        Row {
            case: "at the bound, a held session and a credential store that draws nothing",
            live: quota,
            addressed: Addressed::Held,
            draws: false,
            settled: Err(ProviderTokenRefusal::NothingMinted),
            draws_asked_for: 1,
        },
    ];

    for row in rows {
        let Row {
            case,
            live,
            addressed,
            draws,
            settled: expected,
            draws_asked_for: expected_draws,
        } = row;
        let store = store_holding(live);
        let fresh_key = (
            PROVIDER.to_string(),
            None,
            "figd_a_token_the_store_does_not_hold".to_string(),
        );
        let another_client = (held_key().0, Some("another-client".to_string()), held_key().2);
        let key = match addressed {
            Addressed::Held => Some(held_key()),
            Addressed::Fresh => Some(fresh_key),
            Addressed::HeldTokenAnotherClient => Some(another_client),
            Addressed::StartsOne => None,
        };

        // Counts the draws the store asked for: the seam the credential store sits behind,
        // so a refusal that drew nothing is told apart from one that drew and threw away.
        let draws_asked_for = Cell::new(0usize);
        let drawn = store.mint_within_token_session_bound(key.as_ref(), || {
            draws_asked_for.set(draws_asked_for.get() + 1);
            draws.then(|| DRAWN.to_string())
        });

        let drawn = drawn.as_ref().map(String::as_str).map_err(|error| *error);
        assert_eq!(
            drawn, expected,
            "case {case:?}: the store must settle the request as {expected:?}, got {drawn:?}"
        );
        assert_eq!(
            draws_asked_for.get(),
            expected_draws,
            "case {case:?}: the store must ask the credential store for {expected_draws} \
             draw(s), got {}",
            draws_asked_for.get()
        );
    }
}

/// One emulator, plus the engine its state is read back through: a router carrying only
/// this service over one shared engine, exactly as `core/router` mounts it.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// What the emulator answered: the HTTP status, the JSON body, and the error-type header.
struct Answer {
    status: u16,
    body: Value,
    error_type_header: Option<String>,
}

impl Answer {
    /// Whether the answer names an error type anywhere a client would read one.
    fn names_an_error_type(&self) -> bool {
        self.error_type_header.is_some()
            || ["__type", "code", "Code"]
                .iter()
                .any(|member| self.body[*member].is_string())
    }

    /// The `accessToken` the answer reports, or the empty string.
    fn access_token(&self) -> String {
        self.body["accessToken"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, uri: &str, body: Value) -> Answer {
        let payload = body.to_string();
        let request = ::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
            .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER)
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request is well formed");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type_header = response
            .headers()
            .get(ERROR_TYPE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{uri} answered a body that is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer {
            status,
            body,
            error_type_header,
        }
    }

    /// `POST /tokens/{provider}/refresh` with the `RefreshTokenRequestBody` payload.
    async fn refresh(&self, token: &str, client_id: Option<&str>) -> Answer {
        let mut body = json!({"token": token});
        if let Some(client_id) = client_id {
            body["clientId"] = json!(client_id);
        }
        self.call(&format!("/tokens/{PROVIDER}/refresh"), body).await
    }

    /// `POST /tokens/{provider}` with the `ExchangeCodeForTokenRequestBody` payload.
    async fn exchange(&self, code: &str) -> Answer {
        self.call(
            &format!("/tokens/{PROVIDER}"),
            json!({"code": code, "redirectUri": "https://app.example.com/cb"}),
        )
        .await
    }

    /// Fill the shared store with `live` sessions, so the bound is reached without
    /// driving thousands of requests through the wire.
    fn preload(&self, live: usize) {
        let state = service_state(&*self.engine).expect("the shared engine holds this state");
        let mut store = state.lock();
        let now = SystemTime::now();
        for index in 0..live {
            store.refresh_provider_token_session(
                preloaded_key(index),
                format!("preloaded-access-token-{index}"),
                now,
            );
        }
    }

    /// Every session the store holds, by key, with the access token it carries: the
    /// snapshot a refusal must leave untouched.
    fn snapshot(&self) -> BTreeMap<ProviderTokenKey, String> {
        service_state(&*self.engine)
            .expect("the shared engine holds this state")
            .lock()
            .provider_token_sessions
            .iter()
            .map(|(key, session)| (key.clone(), session.access_token.clone()))
            .collect()
    }
}

/// Rule 3: the order of the two steps changes no response. Past the bound both handlers
/// still answer HTTP 400 with no error type, repeat that answer unchanged, leave the store
/// exactly as it was, and still refresh a session the store already holds.
#[tokio::test]
async fn a_token_request_past_the_bound_is_refused_without_changing_the_store() {
    let emulator = Emulator::new();
    emulator.preload(PROVIDER_TOKEN_SESSION_QUOTA);
    let before = emulator.snapshot();
    assert_eq!(
        before.len(),
        PROVIDER_TOKEN_SESSION_QUOTA,
        "the store must start at the bound, got {} sessions",
        before.len()
    );

    // The rows differ in which handler serves the request and in what it names: a refresh
    // token the store does not hold, a held one presented by another client, and an
    // exchange of a code no session was started from.
    let cases: [(&str, bool, &str, Option<&str>); 3] = [
        ("a refresh of a token the store does not hold", true, "figd_over_the_bound", None),
        (
            "a refresh of a held token by another client",
            true,
            "figd_preloaded_0",
            Some("another-client"),
        ),
        ("an exchange of a fresh code", false, "code-over-the-bound", None),
    ];

    for (case, is_refresh, value, client_id) in cases {
        let mut answers: Vec<Value> = Vec::new();
        for round in 1..=2 {
            let refused = if is_refresh {
                emulator.refresh(value, client_id).await
            } else {
                emulator.exchange(value).await
            };
            assert_eq!(
                refused.status, 400,
                "case {case:?} round {round}: a token request past the bound must be refused \
                 with HTTP 400, got {} with {:?}",
                refused.status, refused.body
            );
            assert!(
                !refused.names_an_error_type(),
                "case {case:?} round {round}: a refusal must name no error type, got header {:?} \
                 and body {:?}",
                refused.error_type_header,
                refused.body
            );
            assert_eq!(
                emulator.snapshot(),
                before,
                "case {case:?} round {round}: a refused token request must leave the store \
                 exactly as it was"
            );
            answers.push(refused.body);
        }
        assert_eq!(
            answers[0], answers[1],
            "case {case:?}: repeating a refused token request must answer the same, got {:?} \
             then {:?}",
            answers[0], answers[1]
        );
    }

    // The admitted path still draws: a session the store already holds refreshes at the
    // bound and takes an access token it did not carry before.
    let held = held_key();
    let granted = emulator.refresh(&held.2, held.1.as_deref()).await;
    assert_eq!(
        granted.status, 200,
        "a session the store holds must still refresh at the bound, got {} with {:?}",
        granted.status, granted.body
    );
    let after = emulator.snapshot();
    assert_eq!(
        after.len(),
        PROVIDER_TOKEN_SESSION_QUOTA,
        "refreshing a held session must not grow the store, got {} sessions",
        after.len()
    );
    assert_eq!(
        after.get(&held).map(String::as_str),
        Some(granted.access_token().as_str()),
        "the held session must carry the access token the caller was handed, got {:?}",
        after.get(&held)
    );
    assert_ne!(
        after.get(&held),
        before.get(&held),
        "a refresh at the bound must still draw a new access token"
    );
}
