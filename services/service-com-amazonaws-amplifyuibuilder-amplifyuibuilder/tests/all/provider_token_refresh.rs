// SPDX-License-Identifier: CC0-1.0

//! `RefreshToken`: the third-party access token the service hands back for a refresh
//! token a caller already holds.
//!
//! The rules under test:
//!
//! 1. A refresh answers a NEWLY MINTED access token for the session the request names —
//!    the provider, the `clientId` it carries and the refresh token itself — together
//!    with the whole seconds that token stays usable, and the token it answers is the one
//!    the store then holds for that session. The access token is never anything the
//!    request carried.
//! 2. Refreshing the SAME session again replaces that session's access token: the caller
//!    gets a different token each time and the store holds one session, carrying the
//!    newest token, not one record per call.
//! 3. Two sessions are told apart by all three parts of their name, so the same refresh
//!    token presented by two clients holds two independent access tokens.
//! 4. `RefreshTokenRequestBody$token` is `@required` with no `@length`, so the model
//!    admits a token of no characters: the service refuses it with HTTP 400, names no
//!    error type (like every refusal of this service) and stores nothing.
//! 5. `provider` targets the `TokenProviders` enum, so a provider the model does not
//!    declare is refused with HTTP 400 and stores nothing.
//! 6. The session store is bounded: past the emulator's own bound a refresh that would
//!    start ANOTHER session is refused with HTTP 400, while a session the store already
//!    holds still refreshes.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    service_state, ProviderTokenKey, ProviderTokenSession, PROVIDER_ACCESS_TOKEN_CHARS,
    PROVIDER_ACCESS_TOKEN_LIFETIME, PROVIDER_TOKEN_SESSION_QUOTA,
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

/// The fixed sentence this service answers a refresh token of no characters with.
///
/// The model declares a `message` member on `InvalidParameterException` but no text for
/// it (`smithy_query(query="shape",shape="InvalidParameterException")`), and this service
/// names no error type on the wire, so the literal IS the name of the failure class. It
/// interpolates nothing: the member it rejects is `@sensitive`.
const BLANK_TOKEN_MESSAGE: &str = "The refresh token provided is not valid";

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

    /// The `accessToken` a successful refresh reports.
    fn access_token(&self) -> String {
        self.body["accessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a refresh must report an accessToken, got {:?}", self.body))
            .to_string()
    }

    /// The `expiresIn` a successful refresh reports.
    fn expires_in(&self) -> i64 {
        self.body["expiresIn"]
            .as_i64()
            .unwrap_or_else(|| panic!("a refresh must report an expiresIn, got {:?}", self.body))
    }

    /// The `message` of a refusal.
    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_else(|| panic!("a refusal must carry a message, got {:?}", self.body))
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

    /// `POST /tokens/{provider}/refresh` with the body the caller wrote.
    async fn refresh(&self, provider: &str, token: &str, client_id: Option<&str>) -> Answer {
        let mut body = json!({"token": token});
        if let Some(client_id) = client_id {
            body["clientId"] = json!(client_id);
        }
        let payload = body.to_string();
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("/tokens/{provider}/refresh"))
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
                    "a refresh answered a body that is not JSON ({error}): {:?}",
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

    /// The session the store holds for one request's (provider, clientId, token), if any.
    fn session(&self, key: &ProviderTokenKey) -> Option<ProviderTokenSession> {
        service_state(&*self.engine)
            .expect("the shared engine holds this service's state")
            .lock()
            .provider_token_session(key)
            .cloned()
    }

    /// How many token sessions the store holds at all.
    fn session_count(&self) -> usize {
        service_state(&*self.engine)
            .expect("the shared engine holds this service's state")
            .lock()
            .provider_token_sessions
            .len()
    }
}

/// The key a request addresses, as the store names it.
fn key(token: &str, client_id: Option<&str>) -> ProviderTokenKey {
    (
        PROVIDER.to_string(),
        client_id.map(str::to_string),
        token.to_string(),
    )
}

/// Rule 1 and rule 3: every form of session named by a request gets its own freshly
/// minted access token, reported with the lifetime the store holds for it, and the token
/// the caller is handed is the one the store then holds.
#[tokio::test]
async fn a_refresh_mints_the_access_token_the_store_then_holds() {
    let long_token = "f".repeat(512);
    // The rows differ along what the rule generalises over: the refresh token's length,
    // alphabet and spacing, and whether — and which — `clientId` the request names.
    // Three rows share one refresh token and differ only in the client, which is rule 3.
    let rows: [(&str, &str, Option<&str>); 7] = [
        ("a single-character token", "t", None),
        ("an opaque provider token", "figd_AbC-123_xyz", Some("amplify-studio")),
        (
            "the same token for another client",
            "figd_AbC-123_xyz",
            Some("another-client"),
        ),
        ("the same token for no client", "figd_AbC-123_xyz", None),
        ("a long token", long_token.as_str(), Some("c")),
        ("a token outside ASCII", "réfresh-tökèn-✓", None),
        ("a token padded with spaces", " padded-token ", Some("")),
    ];

    let emulator = Emulator::new();
    let mut answered: BTreeSet<String> = BTreeSet::new();
    let mut named: BTreeSet<ProviderTokenKey> = BTreeSet::new();

    for (case, token, client_id) in rows {
        let answer = emulator.refresh(PROVIDER, token, client_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a refresh must answer HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );

        let access_token = answer.access_token();
        assert_eq!(
            access_token.chars().count(),
            PROVIDER_ACCESS_TOKEN_CHARS,
            "case {case:?}: the access token must carry {PROVIDER_ACCESS_TOKEN_CHARS} \
             characters of minted secret, got {} characters",
            access_token.chars().count()
        );
        assert_ne!(
            access_token, token,
            "case {case:?}: the access token must be minted, never the refresh token the \
             request carried"
        );
        assert_eq!(
            answer.expires_in(),
            i64::try_from(PROVIDER_ACCESS_TOKEN_LIFETIME.as_secs())
                .expect("the token lifetime fits an i64"),
            "case {case:?}: a freshly minted token must report its whole lifetime in \
             seconds, got {}",
            answer.expires_in()
        );

        let key = key(token, client_id);
        let session = emulator
            .session(&key)
            .unwrap_or_else(|| panic!("case {case:?}: the refresh must store a session"));
        assert_eq!(
            session.access_token, access_token,
            "case {case:?}: the stored session must hold the access token the caller was \
             handed, got {:?}",
            session.access_token
        );
        assert_eq!(
            session.client_id.as_deref(),
            client_id,
            "case {case:?}: the session must record the clientId the request named, got {:?}",
            session.client_id
        );
        assert_eq!(
            session
                .expires_at
                .duration_since(session.issued_at)
                .expect("a session expires after it was issued"),
            PROVIDER_ACCESS_TOKEN_LIFETIME,
            "case {case:?}: the stored expiry must be the token's lifetime after it was \
             issued"
        );

        assert!(
            answered.insert(access_token.clone()),
            "case {case:?}: every refresh must mint its own access token, got {access_token:?} \
             twice"
        );
        named.insert(key);
    }

    assert_eq!(
        emulator.session_count(),
        named.len(),
        "the store must hold one session per distinct (provider, clientId, token), got {} \
         for {} distinct requests",
        emulator.session_count(),
        named.len()
    );
}

/// Rule 2: refreshing one session again replaces that session's access token rather than
/// adding a record, and the response always reports what the store holds afterwards.
#[tokio::test]
async fn refreshing_a_session_again_replaces_its_access_token() {
    // The rows differ in whether the session is named with a client or without one: the
    // replacement rule holds for both.
    for (case, token, client_id) in [
        ("a session with a clientId", "figd_rotate_me", Some("amplify-studio")),
        ("a session without one", "figd_rotate_me_too", None),
    ] {
        let emulator = Emulator::new();
        let key = key(token, client_id);
        let mut previous: Option<String> = None;

        for round in 1..=3 {
            let answer = emulator.refresh(PROVIDER, token, client_id).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} round {round}: a refresh must answer HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let access_token = answer.access_token();
            if let Some(previous) = previous.as_deref() {
                assert_ne!(
                    access_token, previous,
                    "case {case:?} round {round}: a refresh must mint a new access token, got \
                     the previous one back"
                );
            }
            let session = emulator
                .session(&key)
                .unwrap_or_else(|| panic!("case {case:?} round {round}: the session must be held"));
            assert_eq!(
                session.access_token, access_token,
                "case {case:?} round {round}: the session must hold the newest access token, got \
                 {:?}",
                session.access_token
            );
            assert_eq!(
                emulator.session_count(),
                1,
                "case {case:?} round {round}: refreshing one session must not add another, got \
                 {} sessions",
                emulator.session_count()
            );
            previous = Some(access_token);
        }
    }
}

/// Rule 4: a refresh token of no characters is refused with HTTP 400, names no error
/// type, answers this service's one sentence for the class, and stores nothing.
#[tokio::test]
async fn a_blank_refresh_token_is_refused() {
    // The rows differ in HOW the token is blank: absent characters, one space, a tab, and
    // mixed whitespace.
    let blank = [
        ("the empty string", ""),
        ("a single space", " "),
        ("a tab", "\t"),
        ("mixed whitespace", " \t\n "),
    ];

    let emulator = Emulator::new();
    for (case, token) in blank {
        for client_id in [None, Some("amplify-studio")] {
            let refused = emulator.refresh(PROVIDER, token, client_id).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} (clientId {client_id:?}): a blank refresh token must be refused \
                 with HTTP 400, got {} with {:?}",
                refused.status, refused.body
            );
            assert!(
                !refused.names_an_error_type(),
                "case {case:?}: a refusal must name no error type, got header {:?} and body {:?}",
                refused.error_type_header,
                refused.body
            );
            assert_eq!(
                refused.message(),
                BLANK_TOKEN_MESSAGE,
                "case {case:?}: the refusal must answer the class's sentence, got {:?}",
                refused.message()
            );
            assert!(
                emulator.session(&key(token, client_id)).is_none(),
                "case {case:?}: a refused refresh must store nothing"
            );
        }
    }
    assert_eq!(
        emulator.session_count(),
        0,
        "no refused refresh may leave a session behind, got {} sessions",
        emulator.session_count()
    );
}

/// Rule 5: a provider outside `TokenProviders` is refused with HTTP 400 and stores
/// nothing.
#[tokio::test]
async fn a_provider_the_model_does_not_declare_is_refused() {
    // The rows differ in HOW the provider misses the enum: another vendor, case variants
    // of the declared value, and a near-miss spelling.
    let unknown = [
        ("another vendor", "github"),
        ("the uppercase form", "FIGMA"),
        ("a capitalised form", "Figma"),
        ("the declared value with a suffix", "figma2"),
    ];

    let emulator = Emulator::new();
    for (case, provider) in unknown {
        let refused = emulator.refresh(provider, "figd_token", None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} ({provider:?}): an undeclared provider must be refused with HTTP 400, \
             got {} with {:?}",
            refused.status, refused.body
        );
        assert!(
            !refused.names_an_error_type(),
            "case {case:?} ({provider:?}): a refusal must name no error type, got header {:?} and \
             body {:?}",
            refused.error_type_header,
            refused.body
        );
        assert_eq!(
            emulator.session_count(),
            0,
            "case {case:?} ({provider:?}): a refused refresh must store nothing, got {} sessions",
            emulator.session_count()
        );
    }
}

/// Rule 6: the session store is bounded. Past the bound a refresh naming a session the
/// store does not hold is refused with HTTP 400 — the `InvalidParameterException` this
/// operation's model declares, see `tests/all/emulator_store_bound_refusals.rs` for the
/// rule — while a session already held still refreshes.
#[tokio::test]
async fn the_token_session_store_is_bounded_by_its_quota() {
    let emulator = Emulator::new();

    for index in 0..PROVIDER_TOKEN_SESSION_QUOTA {
        let answer = emulator
            .refresh(PROVIDER, &format!("figd_token_{index}"), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case session {index}: a refresh within the bound must succeed, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // One more session is refused, however the request differs from the ones held: a
    // fresh token, or a token already held presented by another client.
    for (case, token, client_id) in [
        ("a fresh refresh token", "figd_token_over_the_bound", None),
        ("a held token for another client", "figd_token_0", Some("another-client")),
    ] {
        let refused = emulator.refresh(PROVIDER, token, client_id).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a refresh past the bound must be refused with HTTP 400, got {} with \
             {:?}",
            refused.status, refused.body
        );
        assert!(
            !refused.names_an_error_type(),
            "case {case:?}: a refusal must name no error type, got header {:?} and body {:?}",
            refused.error_type_header,
            refused.body
        );
        assert!(
            emulator.session(&key(token, client_id)).is_none(),
            "case {case:?}: a refused refresh must store nothing"
        );
    }
    assert_eq!(
        emulator.session_count(),
        PROVIDER_TOKEN_SESSION_QUOTA,
        "a refusal must not grow the store, got {} sessions",
        emulator.session_count()
    );

    // A session the store already holds still refreshes, and takes a new access token.
    let held = key("figd_token_0", None);
    let before = emulator
        .session(&held)
        .expect("the first session is still held");
    let answer = emulator.refresh(PROVIDER, "figd_token_0", None).await;
    assert_eq!(
        answer.status, 200,
        "a session already held must still refresh at the bound, got {} with {:?}",
        answer.status, answer.body
    );
    assert_ne!(
        answer.access_token(),
        before.access_token,
        "a refresh at the bound must still mint a new access token"
    );
    assert_eq!(
        emulator.session_count(),
        PROVIDER_TOKEN_SESSION_QUOTA,
        "refreshing a held session must not grow the store, got {} sessions",
        emulator.session_count()
    );
}
