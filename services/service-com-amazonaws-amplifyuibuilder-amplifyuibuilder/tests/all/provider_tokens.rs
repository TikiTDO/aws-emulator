// SPDX-License-Identifier: CC0-1.0

//! `ExchangeCodeForToken`: the token pair this service grants for an access code, and the
//! session it starts in the one token store `RefreshToken` reads.
//!
//! The rules under test:
//!
//! 1. An exchange grants a NEWLY MINTED pair — an access token, the whole seconds it stays
//!    usable, and a refresh token — and every value is minted rather than fixed: no two
//!    exchanges report the same token, the two tokens of one exchange differ, and neither
//!    is anything the request carried.
//! 2. What it grants is PERSISTED as the session the pair belongs to, named the way a
//!    refresh names one (the provider, the `clientId` the request carried and the refresh
//!    token itself), carrying the access token the caller was handed; the lifetime reported
//!    is read off that stored session.
//! 3. The refresh token an exchange hands back resolves to THAT session: refreshing with it
//!    replaces the session's access token instead of starting another session, which is the
//!    round trip from `ExchangeCodeForToken` into `RefreshToken`.
//! 4. An access code is single-use (RFC 6749 §4.1.2): a code that already started a session
//!    the store holds is refused, however the rest of the request differs, and the refusal
//!    leaves that session alone. A code of no characters — which the model admits, since
//!    `code` carries no `@length` — is refused the same way.
//! 5. `provider` targets `TokenProviders`, an enum whose only value is `figma`
//!    (`smithy_query(query="shape",shape="TokenProviders")`), so any other provider is
//!    refused and stores nothing.
//! 6. The session store is bounded: past the emulator's own bound an exchange that would
//!    start another session is refused with HTTP 400, and nothing is stored for it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    service_state, ComAmazonawsAmplifyuibuilderAmplifyuibuilderState, ProviderTokenKey,
    ProviderTokenSession, ACCESS_CODE_NOT_VALID_MESSAGE, PROVIDER_ACCESS_TOKEN_CHARS,
    PROVIDER_ACCESS_TOKEN_LIFETIME, PROVIDER_TOKEN_SESSION_QUOTA,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The only `TokenProviders` value the model declares.
const PROVIDER: &str = "figma";

/// The response header restJson1 resolves an error shape from. It must be absent from
/// every refusal this service answers.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

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
    /// The `accessToken` the answer reports, or the empty string.
    fn access_token(&self) -> String {
        self.body["accessToken"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The `refreshToken` the answer reports, or the empty string.
    fn refresh_token(&self) -> String {
        self.body["refreshToken"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The `expiresIn` the answer reports, or `None` when it carries none.
    fn expires_in(&self) -> Option<i64> {
        self.body["expiresIn"].as_i64()
    }

    /// The `message` the answer carries, in either spelling a JSON error body uses, or the
    /// empty string.
    fn message(&self) -> &str {
        ["message", "Message"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }

    /// Whether the answer names an error type anywhere a client would read one.
    fn names_an_error_type(&self) -> bool {
        self.error_type_header.is_some()
            || ["__type", "code", "Code"]
                .iter()
                .any(|member| self.body[*member].is_string())
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

    /// `POST /tokens/{provider}` with the `ExchangeCodeForTokenRequestBody` payload.
    async fn exchange(
        &self,
        provider: &str,
        code: &str,
        redirect_uri: &str,
        client_id: Option<&str>,
    ) -> Answer {
        let mut body = json!({"code": code, "redirectUri": redirect_uri});
        if let Some(client_id) = client_id {
            body["clientId"] = json!(client_id);
        }
        self.call(&format!("/tokens/{provider}"), body).await
    }

    /// `POST /tokens/{provider}/refresh` with the `RefreshTokenRequestBody` payload.
    async fn refresh(&self, provider: &str, token: &str, client_id: Option<&str>) -> Answer {
        let mut body = json!({"token": token});
        if let Some(client_id) = client_id {
            body["clientId"] = json!(client_id);
        }
        self.call(&format!("/tokens/{provider}/refresh"), body)
            .await
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

/// One exchange request, differing from its neighbours along what the rule generalises
/// over: the form and size of the access code, the form of the redirect URI, and whether —
/// and which — `clientId` the request names.
struct ExchangeCase {
    label: &'static str,
    code: String,
    redirect_uri: &'static str,
    client_id: Option<&'static str>,
}

fn exchange_cases() -> Vec<ExchangeCase> {
    vec![
        ExchangeCase {
            label: "a single-character code, no client named",
            code: "a".to_string(),
            redirect_uri: "https://app.example.com/callback",
            client_id: None,
        },
        ExchangeCase {
            label: "an opaque code of the length a provider hands out",
            code: "7Fq3kZ2pLx9vN4rT8sB1dG6hJ0mC5yW".to_string(),
            redirect_uri: "https://app.example.com/callback?from=figma",
            client_id: None,
        },
        ExchangeCase {
            label: "a long code, for a client on loopback",
            code: "c".repeat(512),
            redirect_uri: "http://127.0.0.1:3000/oauth",
            client_id: Some("client-on-loopback"),
        },
        ExchangeCase {
            label: "a code of punctuation a URL-safe alphabet allows",
            code: "-._~abc-123_".to_string(),
            redirect_uri: "https://app.example.com/a/deeper/path",
            client_id: Some("another-client"),
        },
        ExchangeCase {
            label: "a code outside ASCII",
            code: "código-ƒigma-✓".to_string(),
            redirect_uri: "https://app.example.com/callback",
            client_id: Some("código-client"),
        },
        ExchangeCase {
            label: "a code for a client named the empty string",
            code: "code-for-an-empty-client-id".to_string(),
            redirect_uri: "https://app.example.com/callback",
            client_id: Some(""),
        },
    ]
}

/// Rules 1, 2 and 3: every row is granted its own minted pair, the session it names is what
/// the store then holds, and the refresh token it hands back refreshes THAT session.
#[tokio::test]
async fn an_exchange_grants_a_pair_and_starts_the_session_a_refresh_reads() {
    let emulator = Emulator::new();
    // Every token any row was handed: a token that repeats is a token the handler did not
    // mint per request.
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut sessions = 0usize;

    for case in exchange_cases() {
        let label = case.label;
        let granted = emulator
            .exchange(PROVIDER, &case.code, case.redirect_uri, case.client_id)
            .await;
        assert_eq!(
            granted.status, 200,
            "case {label:?}: an exchange must answer HTTP 200, got {} with {:?}",
            granted.status, granted.body
        );

        let access_token = granted.access_token();
        let refresh_token = granted.refresh_token();
        for (member, token) in [("accessToken", &access_token), ("refreshToken", &refresh_token)] {
            assert_eq!(
                token.chars().count(),
                PROVIDER_ACCESS_TOKEN_CHARS,
                "case {label:?}: {member} must carry {PROVIDER_ACCESS_TOKEN_CHARS} characters of \
                 minted secret, got {} characters",
                token.chars().count()
            );
            assert_ne!(
                token.as_str(),
                case.code.as_str(),
                "case {label:?}: {member} must be minted, never the access code the request \
                 carried"
            );
            assert!(
                minted.insert(token.clone()),
                "case {label:?}: every minted token must be its own, got {member} {token:?} again"
            );
        }

        let lifetime = granted.expires_in().unwrap_or_else(|| {
            panic!(
                "case {label:?}: the grant must report expiresIn, got {:?}",
                granted.body
            )
        });
        assert_eq!(
            lifetime,
            i64::try_from(PROVIDER_ACCESS_TOKEN_LIFETIME.as_secs())
                .expect("the token lifetime fits an i64"),
            "case {label:?}: a freshly minted access token must report its whole lifetime in \
             seconds, got {lifetime}"
        );

        // Rule 2: the session the store holds is the one the request named, carrying the
        // token the caller was handed.
        let session_key = key(&refresh_token, case.client_id);
        let session = emulator
            .session(&session_key)
            .unwrap_or_else(|| panic!("case {label:?}: the exchange must store a session"));
        assert_eq!(
            session.access_token, access_token,
            "case {label:?}: the stored session must hold the access token the caller was \
             handed, got {:?}",
            session.access_token
        );
        assert_eq!(
            session.client_id.as_deref(),
            case.client_id,
            "case {label:?}: the session must record the clientId the request named, got {:?}",
            session.client_id
        );
        assert_eq!(
            session
                .expires_at
                .duration_since(session.issued_at)
                .expect("a session expires after it was issued"),
            PROVIDER_ACCESS_TOKEN_LIFETIME,
            "case {label:?}: the stored expiry must be the token's lifetime after it was issued"
        );
        sessions += 1;
        assert_eq!(
            emulator.session_count(),
            sessions,
            "case {label:?}: an exchange must start exactly one session, got {} for {sessions} \
             exchanges",
            emulator.session_count()
        );

        // Rule 3: the refresh token resolves to that session — refreshing replaces its
        // access token instead of starting another session.
        let refreshed = emulator
            .refresh(PROVIDER, &refresh_token, case.client_id)
            .await;
        assert_eq!(
            refreshed.status, 200,
            "case {label:?}: the refresh token an exchange handed back must be accepted, got {} \
             with {:?}",
            refreshed.status, refreshed.body
        );
        let refreshed_token = refreshed.access_token();
        assert!(
            minted.insert(refreshed_token.clone()),
            "case {label:?}: a refresh must mint its own access token, got {refreshed_token:?} \
             again"
        );
        assert_eq!(
            emulator.session(&session_key).map(|held| held.access_token),
            Some(refreshed_token),
            "case {label:?}: the refresh must replace the access token of the session the \
             exchange started"
        );
        assert_eq!(
            emulator.session_count(),
            sessions,
            "case {label:?}: refreshing the exchanged token must not start another session, got \
             {} sessions",
            emulator.session_count()
        );
    }
}

/// Rule 4: an access code grants one pair. A code that already started a live session is
/// refused however the rest of the request differs, and so is a code of no characters;
/// neither refusal touches the session already held.
#[tokio::test]
async fn an_access_code_grants_one_pair_and_a_blank_code_grants_none() {
    let emulator = Emulator::new();

    // The rows differ along the code's form, and along what the REPLAY changes beside the
    // code: nothing, the redirect URI, the client id.
    let replays = vec![
        (
            "a plain code, replayed as it was",
            "single-use-1".to_string(),
            None,
            None,
        ),
        (
            "a code replayed with another redirect URI",
            "single-use-2".to_string(),
            Some("https://elsewhere.example.com/cb"),
            None,
        ),
        (
            "a code replayed with another client id",
            "ƒ-single-use-3".to_string(),
            None,
            Some("some-other-client"),
        ),
        (
            "a long code replayed as it was",
            "s".repeat(300),
            None,
            None,
        ),
    ];

    for (label, code, other_redirect, other_client) in replays {
        let first = emulator
            .exchange(PROVIDER, &code, "https://app.example.com/cb", Some("first-client"))
            .await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first exchange of a code must be granted, got {} with {:?}",
            first.status, first.body
        );
        let refresh_token = first.refresh_token();
        let held = key(&refresh_token, Some("first-client"));
        let sessions_before = emulator.session_count();

        let replayed = emulator
            .exchange(
                PROVIDER,
                &code,
                other_redirect.unwrap_or("https://app.example.com/cb"),
                Some(other_client.unwrap_or("first-client")),
            )
            .await;
        assert_eq!(
            replayed.status, 400,
            "case {label:?}: exchanging a code twice must be refused with HTTP 400, got {} with \
             {:?}",
            replayed.status, replayed.body
        );
        assert!(
            !replayed.names_an_error_type(),
            "case {label:?}: the refusal must name no error type, got header {:?} and body {:?}",
            replayed.error_type_header,
            replayed.body
        );
        assert_eq!(
            replayed.message(),
            ACCESS_CODE_NOT_VALID_MESSAGE,
            "case {label:?}: the refusal must answer the class's sentence, got {:?}",
            replayed.body
        );
        assert!(
            replayed.access_token().is_empty() && replayed.refresh_token().is_empty(),
            "case {label:?}: a refused exchange must grant nothing, got {:?}",
            replayed.body
        );
        assert_eq!(
            emulator.session_count(),
            sessions_before,
            "case {label:?}: a refused exchange must start no session, got {} for {} held",
            emulator.session_count(),
            sessions_before
        );
        assert!(
            emulator.session(&held).is_some(),
            "case {label:?}: the session the first exchange started must stand"
        );
    }

    // The rows differ in HOW the code is blank: no characters, one space, a tab, mixed
    // whitespace — `code` is `@required` with no `@length`, so the model admits each.
    let sessions_before = emulator.session_count();
    for (label, code) in [
        ("the empty string", ""),
        ("a single space", " "),
        ("a tab", "\t"),
        ("mixed whitespace", " \t\n "),
    ] {
        let refused = emulator
            .exchange(PROVIDER, code, "https://app.example.com/cb", None)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: a blank access code must be refused with HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.message(),
            ACCESS_CODE_NOT_VALID_MESSAGE,
            "case {label:?}: the refusal must answer the class's sentence, got {:?}",
            refused.body
        );
        assert_eq!(
            emulator.session_count(),
            sessions_before,
            "case {label:?}: a refused exchange must start no session, got {} for {} held",
            emulator.session_count(),
            sessions_before
        );
    }
}

/// Rule 5: a provider outside `TokenProviders` is refused and stores nothing.
#[tokio::test]
async fn a_provider_the_model_does_not_declare_is_refused() {
    let emulator = Emulator::new();

    // The rows differ in HOW the provider misses the one declared value: another vendor,
    // case variants, a name containing it, and the value with a suffix.
    for (label, provider) in [
        ("another vendor", "github"),
        ("the uppercase form", "FIGMA"),
        ("a capitalised form", "Figma"),
        ("a name containing it", "notfigma"),
        ("the declared value with a suffix", "figma2"),
    ] {
        let refused = emulator
            .exchange(
                provider,
                "code-for-a-bad-provider",
                "https://app.example.com/cb",
                None,
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?} ({provider:?}): an exchange for an undeclared provider must be \
             refused with HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert!(
            !refused.names_an_error_type(),
            "case {label:?} ({provider:?}): the refusal must name no error type, got header {:?} \
             and body {:?}",
            refused.error_type_header,
            refused.body
        );
        assert!(
            refused.access_token().is_empty() && refused.refresh_token().is_empty(),
            "case {label:?} ({provider:?}): a refused exchange must grant nothing, got {:?}",
            refused.body
        );
        assert_eq!(
            emulator.session_count(),
            0,
            "case {label:?} ({provider:?}): a refused exchange must store nothing, got {} \
             sessions",
            emulator.session_count()
        );
    }

    // The declared value is itself accepted, so each row above is refused for its provider
    // and not for the route.
    let granted = emulator
        .exchange(
            PROVIDER,
            "code-for-the-declared-provider",
            "https://app.example.com/cb",
            None,
        )
        .await;
    assert_eq!(
        granted.status, 200,
        "the one declared provider must be accepted, got {} with {:?}",
        granted.status, granted.body
    );
}

/// Rule 6: the session store is bounded. Past the emulator's own bound an exchange is
/// refused with HTTP 400 — the `InvalidParameterException` this operation's model declares,
/// see `tests/all/emulator_store_bound_refusals.rs` for the rule — and stores nothing.
#[tokio::test]
async fn the_token_session_store_is_bounded_by_its_quota() {
    let emulator = Emulator::new();

    for index in 0..PROVIDER_TOKEN_SESSION_QUOTA {
        let granted = emulator
            .exchange(
                PROVIDER,
                &format!("code-{index}"),
                "https://app.example.com/cb",
                None,
            )
            .await;
        assert_eq!(
            granted.status, 200,
            "case session {index}: an exchange within the bound must be granted, got {} with {:?}",
            granted.status, granted.body
        );
    }

    for (label, code, client_id) in [
        ("a fresh code", "code-over-the-bound", None),
        ("another fresh code, for a client", "ƒ-code-over-the-bound", Some("a-client")),
    ] {
        let refused = emulator
            .exchange(PROVIDER, code, "https://app.example.com/cb", client_id)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: an exchange past the bound must be refused with HTTP 400, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert!(
            !refused.names_an_error_type(),
            "case {label:?}: the refusal must name no error type, got header {:?} and body {:?}",
            refused.error_type_header,
            refused.body
        );
        assert!(
            refused.access_token().is_empty() && refused.refresh_token().is_empty(),
            "case {label:?}: a refused exchange must grant nothing, got {:?}",
            refused.body
        );
    }
    assert_eq!(
        emulator.session_count(),
        PROVIDER_TOKEN_SESSION_QUOTA,
        "a refusal must not grow the store, got {} sessions",
        emulator.session_count()
    );
}

/// Rule 4, over the clock: a code is spent exactly while the session it started lives, so
/// the expiry the clock already owes releases the code with the session. Checked on the
/// published lifetime — just inside it, ON it and past it — through the store both token
/// handlers read and write.
#[test]
fn an_access_code_is_spent_exactly_while_its_session_lives() {
    let exchanged_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let lifetime = PROVIDER_ACCESS_TOKEN_LIFETIME;
    let code = "the-code-the-session-was-exchanged-from";

    for (label, elapsed, still_held) in [
        ("the instant of the exchange", Duration::ZERO, true),
        ("one second in", Duration::from_secs(1), true),
        ("one second before the lifetime ends", lifetime - Duration::from_secs(1), true),
        ("the instant the lifetime ends", lifetime, false),
        ("one second past it", lifetime + Duration::from_secs(1), false),
        ("long past it", lifetime * 2, false),
    ] {
        let mut store = ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::default();
        let session_key = (
            PROVIDER.to_string(),
            Some("a-client".to_string()),
            "the-refresh-token-it-handed-back".to_string(),
        );
        store.start_exchanged_provider_token_session(
            session_key.clone(),
            "the-access-token-it-handed-back".to_string(),
            code,
            exchanged_at,
        );

        store.expire_provider_token_sessions(exchanged_at + elapsed);
        let held = store.provider_token_session(&session_key).is_some();
        assert_eq!(
            held, still_held,
            "case {label:?} ({elapsed:?} after the exchange): the session must {}be held, got \
             held={held}",
            if still_held { "" } else { "no longer " }
        );
        assert_eq!(
            store.provider_code_already_exchanged(PROVIDER, code),
            still_held,
            "case {label:?}: the access code must be spent exactly while its session lives"
        );
    }

    // A refresh of the session does NOT un-spend the code it was exchanged from: the record
    // of what started the session survives the token replacement.
    let mut store = ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::default();
    let session_key = (PROVIDER.to_string(), None, "handed-back".to_string());
    store.start_exchanged_provider_token_session(
        session_key.clone(),
        "first-access-token".to_string(),
        code,
        exchanged_at,
    );
    store.refresh_provider_token_session(
        session_key,
        "second-access-token".to_string(),
        exchanged_at + Duration::from_secs(1),
    );
    assert!(
        store.provider_code_already_exchanged(PROVIDER, code),
        "a refresh must not un-spend the access code the session was exchanged from"
    );
}
