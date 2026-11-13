// SPDX-License-Identifier: CC0-1.0

//! The S3 Control account-level `PublicAccessBlock` configuration, driven end to
//! end over HTTP.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it
//! per the Smithy model, runs the handler and serializes the restXml response.
//! Nothing calls a handler directly, so a passing row here is a claim about the
//! wire.
//!
//! The rule under test: an account has AT MOST ONE `PublicAccessBlock`
//! configuration; `PutPublicAccessBlock` creates or replaces it,
//! `GetPublicAccessBlock` reports exactly the settings last put (a member the
//! request omits reading `false`, the model's `@default`), and
//! `DeletePublicAccessBlock` returns the account to having none — which
//! `GetPublicAccessBlock` answers `NoSuchPublicAccessBlockConfiguration` (404)
//! for, a different answer from a configuration whose four settings are all
//! `false`. The table below spans that class: every one of the sixteen
//! combinations of the four settings, the subsets of them a request may carry at
//! all, and the accounts a caller may name — its own, which is served, and
//! others, which are refused.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::state::BucketPublicAccessBlock;
use smithy_server_runtime::MultiServiceRouter;

// The status a call of this surface whose response carries no document — the
// put and the delete, whose modelled output is `smithy.api#Unit` — answers a
// success with: the service's own constant, read through the shared test
// harness so every suite asserts the one value.
use crate::access_point_lifecycle::NO_CONTENT;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The account whose configuration most rows below write and read: the caller's
/// own, which is the only one S3 Control authorizes a request for.
const ACCOUNT: &str = ::emulator_server::EMULATOR_ACCOUNT_ID;

/// A second account — one the caller holds no credentials for — used to pin that
/// a configuration request addressed in another account is refused.
const OTHER_ACCOUNT: &str = "210987654321";

/// A third account, so the refusal rows differ in more than one foreign account.
const THIRD_ACCOUNT: &str = "210987654322";

/// The XML namespace this service's model declares.
const XMLNS: &str = "http://awss3control.amazonaws.com/doc/2018-08-20/";

/// The URI both the account-level reads and writes are bound to
/// (`smithy_query(query="operation",operation="GetPublicAccessBlock")`).
const URI: &str = "/v20180820/configuration/publicAccessBlock";

/// The four members of `PublicAccessBlockConfiguration`, in the order the shape
/// declares them. SOURCE: the model's
/// `com.amazonaws.s3control#PublicAccessBlockConfiguration` — each member
/// targets `Setting` (boolean) and carries `@default: false`.
const SETTINGS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// The wire error code `GetPublicAccessBlock` answers for an account that has no
/// configuration, with HTTP 404 (the model's
/// `NoSuchPublicAccessBlockConfiguration`, `@httpError(404)`).
const NO_SUCH_CONFIGURATION: &str = "NoSuchPublicAccessBlockConfiguration";

/// One response: the status and the body, with readers for its XML elements.
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    /// The text of the first `<element>…</element>` in the body, or `None` when
    /// the body carries none.
    ///
    /// The emulator writes a flat document: no attributes on these elements, no
    /// CDATA, no layout whitespace between tags, and the values read here
    /// (`true`, `false`, an error code) carry no character needing escaping.
    fn text(&self, element: &str) -> Option<String> {
        let open = format!("<{element}>");
        let close = format!("</{element}>");
        let start = self.body.find(&open)? + open.len();
        let end = self.body[start..].find(&close)?;
        Some(self.body[start..start + end].to_string())
    }

    /// The S3-style error code (`<Code>`) of a failure response.
    fn code(&self) -> String {
        self.text("Code")
            .unwrap_or_else(|| format!("<no Code element> in {}", self.body))
    }

    /// The four settings this reply reports, as the text of each element, in the
    /// order [`SETTINGS`] declares them. An element the reply omits reads
    /// `None`.
    fn settings(&self) -> [Option<String>; 4] {
        [
            self.text(SETTINGS[0]),
            self.text(SETTINGS[1]),
            self.text(SETTINGS[2]),
            self.text(SETTINGS[3]),
        ]
    }
}

/// One emulator: a router with this service mounted on a single shared engine,
/// exactly as `core/router` assembles it. The engine is kept so a test can seed
/// the store directly (see [`Emulator::hold_configurations_for`]).
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These requests carry an unsigned `Authorization` header, which is what
        // makes them route to the `s3` signing name; signature verification is
        // the transport's concern, not this suite's.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_s3control_awss3controlservicev20180820::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Put `count` accounts' configurations into the store directly.
    ///
    /// The front door cannot reach a store of more than one configuration: a
    /// configuration is keyed by the account alone, and every account but the
    /// caller's own is refused before anything is stored. The store-wide bound
    /// exists for the store, not for one caller, so it is reached here by
    /// seeding it and then asking the service to record one more.
    ///
    /// Seeded where the configurations LIVE — the object store Amazon S3
    /// enforces them from, which this service writes them in — so the count a
    /// put is judged against is the one the service really reads.
    fn hold_configurations_for(&self, count: usize) {
        let state = ::service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
            .expect("the shared engine must hold the object store");
        let mut store = state.lock();
        for index in 0..count {
            assert!(
                store.record_account_public_access_block(
                    &account_id(index),
                    BucketPublicAccessBlock::all_enabled(),
                ),
                "seeding account {index} must be recorded"
            );
        }
    }

    async fn call(&self, method: &str, account_id: &str, body: &str) -> Reply {
        let request = ::http::Request::builder()
            .method(method)
            .uri(URI)
            .header(
                "host",
                format!("{account_id}.s3-control.{REGION}.amazonaws.com"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/s3/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .header("x-amz-account-id", account_id)
            .header("content-type", "application/xml")
            .body(::bytes::Bytes::from(body.to_owned()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        Reply {
            status: response.status().as_u16(),
            body: String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body"),
        }
    }

    /// `PutPublicAccessBlock` with all four settings spelled out.
    async fn put(&self, account_id: &str, settings: [bool; 4]) -> Reply {
        let members: String = SETTINGS
            .iter()
            .zip(settings)
            .map(|(member, value)| format!("<{member}>{value}</{member}>"))
            .collect();
        self.put_body(account_id, &members).await
    }

    /// `PutPublicAccessBlock` carrying exactly `members` inside the payload
    /// element, so a row can omit a setting the way a client that leaves it
    /// unset does.
    async fn put_body(&self, account_id: &str, members: &str) -> Reply {
        let body = format!(
            "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">{members}\
             </PublicAccessBlockConfiguration>"
        );
        self.call("PUT", account_id, &body).await
    }

    async fn get(&self, account_id: &str) -> Reply {
        self.call("GET", account_id, "").await
    }

    async fn delete(&self, account_id: &str) -> Reply {
        self.call("DELETE", account_id, "").await
    }
}

/// Every combination of the four settings: sixteen rows, differing in which
/// settings are on and in how many of them are, with the all-`false` and
/// all-`true` corners among them.
fn every_combination() -> Vec<[bool; 4]> {
    (0..16u8)
        .map(|bits| {
            [
                bits & 1 != 0,
                bits & 2 != 0,
                bits & 4 != 0,
                bits & 8 != 0,
            ]
        })
        .collect()
}

/// The text a reply must carry for each setting of `settings`.
fn expected(settings: [bool; 4]) -> [Option<String>; 4] {
    [
        Some(settings[0].to_string()),
        Some(settings[1].to_string()),
        Some(settings[2].to_string()),
        Some(settings[3].to_string()),
    ]
}

#[tokio::test]
async fn put_then_get_round_trips_every_combination_of_the_four_settings() {
    let emulator = Emulator::start();

    for settings in every_combination() {
        let put = emulator.put(ACCOUNT, settings).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {settings:?}: expected PutPublicAccessBlock to answer {NO_CONTENT}, got {} with \
             body {}",
            put.status, put.body
        );

        let read = emulator.get(ACCOUNT).await;
        assert_eq!(
            read.status, 200,
            "case {settings:?}: expected GetPublicAccessBlock to answer 200 after the put, \
             got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.settings(),
            expected(settings),
            "case {settings:?}: expected the settings just put, got body {}",
            read.body
        );

        // A second read changes nothing a later caller sees: the report is the
        // same configuration.
        let again = emulator.get(ACCOUNT).await;
        assert_eq!(
            again.settings(),
            expected(settings),
            "case {settings:?}: expected a repeat read to report the same settings, got body {}",
            again.body
        );
    }

    // Sixteen puts against one account left one configuration, not sixteen: the
    // last one put is what is reported.
    let last = *every_combination().last().expect("sixteen rows");
    let read = emulator.get(ACCOUNT).await;
    assert_eq!(
        read.settings(),
        expected(last),
        "case {last:?}: expected the LAST put to win, got body {}",
        read.body
    );
}

#[tokio::test]
async fn absence_is_distinct_from_a_configuration_whose_settings_are_all_false() {
    let all_false = [false; 4];
    let emulator = Emulator::start();

    let before = emulator.get(ACCOUNT).await;
    assert_eq!(
        (before.status, before.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: expected a 404 {NO_SUCH_CONFIGURATION} before any put, got {} with body {}",
        "no configuration",
        before.status,
        before.body
    );

    assert_eq!(
        emulator.put(ACCOUNT, all_false).await.status,
        NO_CONTENT,
        "case {:?}: expected the all-false put to be accepted",
        "all four settings false"
    );
    let read = emulator.get(ACCOUNT).await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected 200 rather than {NO_SUCH_CONFIGURATION} for a stored all-false \
         configuration, got {} with body {}",
        "all four settings false", read.status, read.body
    );
    assert_eq!(
        read.settings(),
        expected(all_false),
        "case {:?}: expected the four settings reported as false, got body {}",
        "all four settings false",
        read.body
    );

    let delete = emulator.delete(ACCOUNT).await;
    assert_eq!(
        delete.status, NO_CONTENT,
        "case {:?}: expected DeletePublicAccessBlock to answer {NO_CONTENT}, got {} with body {}",
        "delete a stored configuration", delete.status, delete.body
    );
    let after = emulator.get(ACCOUNT).await;
    assert_eq!(
        (after.status, after.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: expected the account to hold no configuration after the delete, got {} \
         with body {}",
        "read after delete",
        after.status,
        after.body
    );

    // The delete is idempotent: the model declares no error for it, and an
    // account holding none ends up holding none.
    let repeat = emulator.delete(ACCOUNT).await;
    assert_eq!(
        repeat.status, NO_CONTENT,
        "case {:?}: expected a repeat delete to succeed, got {} with body {}",
        "repeat delete", repeat.status, repeat.body
    );
    let still = emulator.get(ACCOUNT).await;
    assert_eq!(
        (still.status, still.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "case {:?}: expected the account to still hold no configuration, got {} with body {}",
        "read after repeat delete",
        still.status,
        still.body
    );
}

#[tokio::test]
async fn a_setting_the_request_omits_is_stored_and_reported_as_false() {
    let emulator = Emulator::start();

    // Each row carries a DIFFERENT subset of the four members: none of them, one
    // of them, and three of them. The members a row leaves out must read
    // `false`, which is the `@default` the model puts on every one of them.
    let mut rows: Vec<(String, String, [bool; 4])> = vec![(
        "no member at all".to_string(),
        String::new(),
        [false; 4],
    )];
    for (index, member) in SETTINGS.iter().enumerate() {
        let mut settings = [false; 4];
        settings[index] = true;
        rows.push((
            format!("only {member}"),
            format!("<{member}>true</{member}>"),
            settings,
        ));
    }
    // Three of the four, leaving the last one out.
    let three: String = SETTINGS[..3]
        .iter()
        .map(|member| format!("<{member}>true</{member}>"))
        .collect();
    rows.push((
        format!("every member but {}", SETTINGS[3]),
        three,
        [true, true, true, false],
    ));

    for (case, members, settings) in rows {
        let put = emulator.put_body(ACCOUNT, &members).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {case:?}: expected PutPublicAccessBlock to accept the payload, got {} with \
             body {}",
            put.status, put.body
        );

        let read = emulator.get(ACCOUNT).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetPublicAccessBlock to answer 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.settings(),
            expected(settings),
            "case {case:?}: expected the omitted members to read false, got body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_configuration_call_addressed_in_another_account_is_refused() {
    let emulator = Emulator::start();
    let mine = [true, false, true, false];
    let theirs = [false, true, false, true];

    assert_eq!(emulator.put(ACCOUNT, mine).await.status, NO_CONTENT);

    // One row per operation of this surface, each addressed in an account the
    // caller holds no credentials for: the answer is the same refusal, before
    // any configuration is read, written or removed, and it names the account
    // the request asked for.
    for (case, reply) in [
        ("GetPublicAccessBlock", emulator.get(OTHER_ACCOUNT).await),
        (
            "PutPublicAccessBlock",
            emulator.put(OTHER_ACCOUNT, theirs).await,
        ),
        ("DeletePublicAccessBlock", emulator.delete(OTHER_ACCOUNT).await),
        // A third account, to pin that the rule is about the account not being
        // the caller's own rather than about one other account.
        ("GetPublicAccessBlock, a third account", emulator.get(THIRD_ACCOUNT).await),
    ] {
        assert_eq!(
            (reply.status, reply.code().as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected a request for another account to be refused 403 \
             AccessDenied, got {} with body {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.text("Message").as_deref(),
            Some("Access Denied"),
            "case {case:?}: expected the fixed refusal message, got body {}",
            reply.body
        );
    }

    // None of that touched the caller's own configuration, and the refused put
    // stored nothing for the account it named.
    let kept = emulator.get(ACCOUNT).await;
    assert_eq!(
        kept.settings(),
        expected(mine),
        "expected the caller's own configuration to be exactly what it put, got body {}",
        kept.body
    );
    assert_eq!(
        emulator.delete(ACCOUNT).await.status,
        NO_CONTENT,
        "expected the caller's own delete to still be served"
    );
    let gone = emulator.get(ACCOUNT).await;
    assert_eq!(
        (gone.status, gone.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "expected the deleted configuration to be gone, got {} with body {}",
        gone.status,
        gone.body
    );
}

/// A 12-digit account id, as `AccountId`'s `^\d{12}$` requires, distinct per
/// `index`.
fn account_id(index: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + index as u64)
}

#[tokio::test]
async fn the_store_cannot_be_grown_by_naming_accounts_the_caller_does_not_hold() {
    let emulator = Emulator::start();
    // Read from the crate so this test cannot drift onto a stale number.
    let quota =
        service_com_amazonaws_s3control_awss3controlservicev20180820::state::PUBLIC_ACCESS_BLOCK_QUOTA;

    // A configuration is keyed by the account alone, and the only account a
    // caller can address is its own, so no sequence of calls grows the store
    // past the one record — far below the bound the store would otherwise
    // refuse at.
    assert!(
        quota > 1,
        "expected the store's bound to be more than the one record a caller can hold, got {quota}"
    );
    for (case, settings) in [
        ("the first put", [true; 4]),
        ("a replacing put", [false; 4]),
        ("a put differing in one setting", [true, false, false, false]),
    ] {
        let stored = emulator.put(ACCOUNT, settings).await;
        assert_eq!(
            stored.status, NO_CONTENT,
            "case {case:?}: expected the caller's own put to be accepted, got {} with body {}",
            stored.status, stored.body
        );
        assert_eq!(
            emulator.get(ACCOUNT).await.settings(),
            expected(settings),
            "case {case:?}: expected the stored configuration to be the one just put"
        );
    }

    // Every other account id is refused, whatever it looks like: the ids below
    // differ in their leading digits, in being all zeroes but for one digit, and
    // in being the largest the `^\d{12}$` pattern admits.
    for (case, named) in [
        ("an id one past the caller's own", account_id(0)),
        ("an id in another block", account_id(quota)),
        ("the largest id the pattern admits", "999999999999".to_string()),
    ] {
        let refused = emulator.put(&named, [true; 4]).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected the put for {named} to be refused, got {} with body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("AccountId").as_deref(),
            Some(named.as_str()),
            "case {case:?}: expected the refusal to name the account asked for, got body {}",
            refused.body
        );
        // Nothing was stored for it: the read is refused the same way rather
        // than reporting a configuration.
        let read = emulator.get(&named).await;
        assert_eq!(
            (read.status, read.code().as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected the read of another account to be refused too, got {} \
             with body {}",
            read.status,
            read.body
        );
    }
}

#[tokio::test]
async fn the_number_of_stored_configurations_is_bounded_and_a_delete_releases_the_bound() {
    let emulator = Emulator::start();
    // Read from the crate so this test cannot drift onto a stale number.
    let quota =
        service_com_amazonaws_s3control_awss3controlservicev20180820::state::PUBLIC_ACCESS_BLOCK_QUOTA;

    // The store holds the bound already, less the one slot the caller's own
    // configuration takes.
    emulator.hold_configurations_for(quota - 1);
    let stored = emulator.put(ACCOUNT, [true; 4]).await;
    assert_eq!(
        stored.status, NO_CONTENT,
        "expected the put that fills the last slot to be accepted, got {} with body {}",
        stored.status, stored.body
    );

    // A configuration already in the store stays writable at the bound:
    // replacing it adds no record.
    let replaced = emulator.put(ACCOUNT, [false; 4]).await;
    assert_eq!(
        (replaced.status, emulator.get(ACCOUNT).await.settings()),
        (NO_CONTENT, expected([false; 4])),
        "expected a replace at the bound to be accepted and reported, got {} with body {}",
        replaced.status,
        replaced.body
    );

    // A delete releases the slot it held, so the store cannot wedge: the put
    // that follows is accepted.
    assert_eq!(
        emulator.delete(ACCOUNT).await.status,
        NO_CONTENT,
        "expected the caller's own delete to be served"
    );
    let after_the_delete = emulator.put(ACCOUNT, [true; 4]).await;
    assert_eq!(
        after_the_delete.status, NO_CONTENT,
        "expected the put after the delete to be accepted, got {} with body {}",
        after_the_delete.status, after_the_delete.body
    );

    // With the bound reached by records the caller holds none of, one more would
    // grow the store past it, so it is refused rather than stored — and the
    // refusal names the bound it is about.
    assert_eq!(emulator.delete(ACCOUNT).await.status, NO_CONTENT);
    emulator.hold_configurations_for(quota);
    let refused = emulator.put(ACCOUNT, [true; 4]).await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (403, "AccessDenied"),
        "expected a put past the store's bound to be refused, got {} with body {}",
        refused.status,
        refused.body
    );
    assert!(
        refused.body.contains(&quota.to_string()),
        "expected the refusal to name the bound {quota}, got body {}",
        refused.body
    );
    let unstored = emulator.get(ACCOUNT).await;
    assert_eq!(
        (unstored.status, unstored.code().as_str()),
        (404, NO_SUCH_CONFIGURATION),
        "expected the refused put to store nothing, got {} with body {}",
        unstored.status,
        unstored.body
    );
}
