// SPDX-License-Identifier: CC0-1.0

//! WHEN a write of this service reads the clock, end to end over HTTP.
//!
//! The rule under test: a record's stamps report when the record was created or last
//! changed — "The time that the component was modified"
//! (`smithy_query(query="doc",shape="Component",member="modifiedAt",model_path="services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/smithy-model/com-amazonaws-amplifyuibuilder-amplifyuibuilder.json")`,
//! and the same sentence for a theme and for a codegen job) — so the instant a write
//! stamps into a record is the instant it APPLIED the change, the instant it observed
//! while it held the store, never an earlier one sampled on the request's way in. Two
//! writers then stamp in the order they commit, and a record's stamp cannot end up older
//! than the one an already-committed write left.
//!
//! The first test drives that ordering DETERMINISTICALLY rather than hoping for an
//! interleaving: the test takes this service's store lock itself, issues the call under
//! test while holding it, and releases it only after a span. The call therefore arrives —
//! and would sample a clock read taken on its way in — BEFORE the earliest instant it can
//! possibly own the store, so a stamp earlier than the release instant is proof the clock
//! was read outside the critical section.
//!
//! What the rule generalises over, and so what the table spans: the OPERATION that stamps
//! (a component created, a component updated, a theme created, a theme updated, a codegen
//! job started), the record KIND stamped, whether the write mints the record or merges
//! into one that already carries stamps, and which stamps it writes (`createdAt` and
//! `modifiedAt` together, `modifiedAt` alone, and the instant a started job's owed status
//! transition falls due).
//!
//! The other two tests are the consequences a client sees: with several writers racing on
//! one record, the `modifiedAt` the record keeps is the LATEST of the instants its writers
//! reported, and a page of concurrently created records is non-decreasing in `createdAt`
//! along the creation order it is served in.
//!
//! Every call goes through the router, so each case exercises routing, deserialization,
//! the handler, the persisted state and serialization — the same path a client takes — and
//! reads each record back through its own reader.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder as amplifyuibuilder;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment every case addresses. `AppId` is
/// `@length(min: 1, max: 20)` with pattern `^d[a-z0-9]+$`.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// How long a row holds this service's store lock while the call under test waits for it.
///
/// Any span would do — the call cannot be applied before the lock is released, however
/// long that takes — so this one is only long enough to be unmistakable beside the clock's
/// resolution and short enough to keep the suite quick.
const HELD_FOR: Duration = Duration::from_millis(250);

/// How many writers race, per row of the two concurrency tests. The rule is about the
/// ORDER commits take, so the rows differ in how many commits there are to order.
const RACING: [usize; 3] = [2, 4, 8];

/// One emulator: a router carrying only this service, over one shared engine the test
/// keeps a handle on, so a case can take this service's store lock itself.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(amplifyuibuilder::EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The record id the answer carries, or the empty string. Every create, update and
    /// read here binds its entity to `@httpPayload`, so the body IS the record.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }
}

async fn call(emulator: &Emulator, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The record kind a write stamps, and so the collection its operations live under.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Component,
    Theme,
    CodegenJob,
}

impl Kind {
    /// The URI this service binds the kind's collection to.
    fn collection(self) -> String {
        let suffix = match self {
            Kind::Component => "components",
            Kind::Theme => "themes",
            Kind::CodegenJob => "codegen-jobs",
        };
        format!("/app/{APP_ID}/environment/{ENVIRONMENT}/{suffix}")
    }
}

/// The one call under test of a row.
#[derive(Debug, Clone, Copy)]
enum Write {
    CreateComponent,
    UpdateComponent,
    CreateTheme,
    UpdateTheme,
    StartCodegenJob,
}

impl Write {
    fn kind(self) -> Kind {
        match self {
            Write::CreateComponent | Write::UpdateComponent => Kind::Component,
            Write::CreateTheme | Write::UpdateTheme => Kind::Theme,
            Write::StartCodegenJob => Kind::CodegenJob,
        }
    }

    /// The stamps this write sets, under the names the record's own reader reports them.
    /// A create (or a start) sets both, because creating a record is the change that first
    /// set its `modifiedAt`; a merge moves only `modifiedAt`.
    fn stamps(self) -> &'static [&'static str] {
        match self {
            Write::CreateComponent | Write::CreateTheme | Write::StartCodegenJob => {
                &["createdAt", "modifiedAt"]
            }
            Write::UpdateComponent | Write::UpdateTheme => &["modifiedAt"],
        }
    }

    /// Whether the write merges into a record that has to exist, and so be stamped,
    /// before the call under test is issued.
    fn merges(self) -> bool {
        matches!(self, Write::UpdateComponent | Write::UpdateTheme)
    }
}

/// The smallest `CreateComponentData` the model admits, under a name of its own: `name`,
/// `componentType`, `properties`, `variants`, `overrides` and `bindingProperties` are its
/// `@required` members (`smithy_query(query="shape",shape="CreateComponentData")`). A
/// component name identifies a component within its app environment, so each one names
/// itself.
fn component_data(index: usize) -> Value {
    json!({
        "name": format!("Hero-{index}"),
        "componentType": "Text",
        "properties": {},
        "variants": [],
        "overrides": {},
        "bindingProperties": {},
    })
}

/// The smallest `CreateThemeData` the model admits, under a name of its own: `name` and
/// `values` are its `@required` members
/// (`smithy_query(query="shape",shape="CreateThemeData")`).
fn theme_data(index: usize) -> Value {
    json!({
        "name": format!("Base-{index}"),
        "values": [{"key": "primaryColor", "value": {"value": "#047d95"}}],
    })
}

/// The patch one writer of a racing row sends: a component is renamed, a theme is given
/// its own values list. Each writer's patch differs, so the record keeps the one the last
/// commit wrote.
fn patch(kind: Kind, writer: usize) -> Value {
    match kind {
        Kind::Component => json!({"name": format!("Renamed-{writer}")}),
        Kind::Theme => json!({"values": [{"key": "primaryColor", "value": {"value": format!("#00000{writer}")}}]}),
        Kind::CodegenJob => json!({}),
    }
}

/// The record a merging row changes, created — and so stamped — BEFORE the lock is taken.
async fn prepare(emulator: &Emulator, case: &str, index: usize, write: Write) -> Option<String> {
    if !write.merges() {
        return None;
    }
    let kind = write.kind();
    let data = match kind {
        Kind::Component => component_data(index),
        Kind::Theme => theme_data(index),
        Kind::CodegenJob => json!({}),
    };
    let created = call(emulator, "POST", &kind.collection(), Some(data)).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the setup create expected HTTP 200, got {} with {:?}",
        created.status, created.body
    );
    Some(created.id())
}

/// The row's one call under test: the method, the URI and the body it sends.
fn request(write: Write, index: usize, target: Option<&str>) -> (&'static str, String, Value) {
    let collection = write.kind().collection();
    match write {
        Write::CreateComponent => ("POST", collection, component_data(index)),
        Write::CreateTheme => ("POST", collection, theme_data(index)),
        Write::StartCodegenJob => ("POST", collection, json!({"renderConfig": {"react": {}}})),
        Write::UpdateComponent | Write::UpdateTheme => {
            let id = target.expect("a merging row prepared the record it changes");
            (
                "PATCH",
                format!("{collection}/{id}"),
                patch(write.kind(), index),
            )
        }
    }
}

/// The stamps this service's own store holds for one record, at full precision.
#[derive(Debug)]
struct Stored {
    created_at: SystemTime,
    modified_at: SystemTime,
    /// The instant the clock owes the record's next status change at, for a record that is
    /// owed one at all — a started codegen job is the only record of this service that is.
    next_status_due: Option<SystemTime>,
}

fn stored_stamps(emulator: &Emulator, case: &str, kind: Kind, id: &str) -> Stored {
    let state = amplifyuibuilder::state::service_state(&*emulator.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    match kind {
        Kind::Component => {
            let component = store
                .component(APP_ID, ENVIRONMENT, id)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds component {id}"));
            Stored {
                created_at: component.created_at,
                modified_at: component.modified_at,
                next_status_due: None,
            }
        }
        Kind::Theme => {
            let theme = store
                .theme(APP_ID, ENVIRONMENT, id)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds theme {id}"));
            Stored {
                created_at: theme.created_at,
                modified_at: theme.modified_at,
                next_status_due: None,
            }
        }
        Kind::CodegenJob => {
            let job = store
                .codegen_job(APP_ID, ENVIRONMENT, id)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds codegen job {id}"));
            Stored {
                created_at: job.created_at,
                modified_at: job.modified_at,
                next_status_due: job.next_status_due.map(|(due, _)| due),
            }
        }
    }
}

/// A stored instant as the wire carries it. Every stamp of this service is
/// `@timestampFormat("date-time")`, so rendering both sides of a comparison the one way
/// the model asks for compares them at whatever precision that format carries.
fn as_wire(instant: SystemTime) -> String {
    DateTime::from(instant)
        .fmt(Format::DateTime)
        .expect("a stored instant renders as a date-time")
}

/// The epoch seconds and nanoseconds a date-time stamp carries, so two of them can be
/// ordered. Panics when the text is no date-time, which is itself a defect.
fn stamp_order(case: &str, what: &str, text: &str) -> (i64, u32) {
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {what} must parse as a date-time ({error}): {text:?}")
    });
    (parsed.secs(), parsed.subsec_nanos())
}

/// One stamp of a record a response carries, as text.
fn stamp_text(case: &str, what: &str, value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: {what} must be an ISO-8601 date-time string, got {value}")
        })
        .to_string()
}

// Four worker threads: the call under test BLOCKS one of them on this service's store lock
// while the test holds it, so the test's own task needs another to run on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_stamps_the_instant_it_owned_the_store() {
    let rows: [(&str, Write); 5] = [
        ("a component created", Write::CreateComponent),
        ("a component updated", Write::UpdateComponent),
        ("a theme created", Write::CreateTheme),
        ("a theme updated", Write::UpdateTheme),
        ("a codegen job started", Write::StartCodegenJob),
    ];

    for (index, (case, write)) in rows.into_iter().enumerate() {
        let emulator = Arc::new(emulator());
        let target = prepare(&emulator, case, index, write).await;
        let (method, uri, body) = request(write, index, target.as_deref());

        // This service's store lock, taken and HELD on a thread of its own, so the call
        // issued below cannot be applied until it is released. The thread SIGNALS once it
        // holds the lock, so the call races a lock that is provably held rather than one
        // assumed to be held by then. It answers the instant it released at.
        let (held, taken) = mpsc::channel();
        let holding = std::thread::spawn({
            let engine = Arc::clone(&emulator.engine);
            move || {
                let state = amplifyuibuilder::state::service_state(&*engine)
                    .expect("the in-memory engine carries this service's store");
                let store = state.lock();
                held.send(()).expect("the test waits for the lock to be taken");
                std::thread::sleep(HELD_FOR);
                let released_at = SystemTime::now();
                drop(store);
                released_at
            }
        });
        taken.recv().expect("the store lock was taken");

        let waiting = tokio::spawn({
            let emulator = Arc::clone(&emulator);
            let uri = uri.clone();
            async move { call(&emulator, method, &uri, Some(body)).await }
        });
        let released_at = holding.join().expect("the lock was held and released");
        let answered = waiting.await.expect("the call under test ran to completion");
        assert_eq!(
            answered.status, 200,
            "case {case:?}: {method} {uri} expected HTTP 200, got {} with {:?}",
            answered.status, answered.body
        );

        let id = answered.id();
        assert!(
            !id.is_empty(),
            "case {case:?}: the answer must name the record it stamped, got {:?}",
            answered.body
        );
        let stored = stored_stamps(&emulator, case, write.kind(), &id);

        for member in write.stamps() {
            let stamp = if *member == "createdAt" {
                stored.created_at
            } else {
                stored.modified_at
            };
            // The stamp is the instant the write observed while it OWNED the store, so it
            // cannot be earlier than the instant the store was released to it.
            assert!(
                stamp >= released_at,
                "case {case:?}: {member} must be an instant observed while the write held \
                 the store, so not before the store was released to it ({released_at:?}), \
                 got {stamp:?}"
            );
            // The answer reports the record, stamp included.
            assert_eq!(
                answered.body[*member],
                json!(as_wire(stamp)),
                "case {case:?}: the answer must carry the {member} the store holds ({:?}), \
                 got {:?}",
                as_wire(stamp),
                answered.body[*member]
            );
        }

        // A merge leaves the identity the create stamped where it was, and dates its own
        // change after it.
        if write.merges() {
            assert!(
                stored.created_at < released_at,
                "case {case:?}: createdAt must stay at the instant the setup create \
                 stamped, which is before the store was released to the update \
                 ({released_at:?}), got {:?}",
                stored.created_at
            );
            assert!(
                stored.modified_at > stored.created_at,
                "case {case:?}: a merge dates its change after the create it merges into \
                 ({:?}), got {:?}",
                stored.created_at,
                stored.modified_at
            );
        }

        // The transition the clock owes a started job is owed FROM the instant the start
        // was applied, so it falls due a whole span after the store was released.
        if let Some(due) = stored.next_status_due {
            let earliest = released_at + amplifyuibuilder::state::CODEGEN_JOB_TRANSITION_DELAY;
            assert!(
                due >= earliest,
                "case {case:?}: the owed transition must fall due a span after the start \
                 was applied, so not before {earliest:?}, got {due:?}"
            );
        }

        // Persisted: the record's own reader reports the stamps the write left.
        let read = call(
            &emulator,
            "GET",
            &format!("{}/{id}", write.kind().collection()),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the record back expected HTTP 200, got {} with {:?}",
            read.status, read.body
        );
        for member in write.stamps() {
            assert_eq!(
                read.body[*member],
                answered.body[*member],
                "case {case:?}: the reader must report the {member} the write stamped \
                 ({:?}), got {:?}",
                answered.body[*member],
                read.body[*member]
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stamp_a_record_keeps_is_the_latest_its_writers_reported() {
    for width in RACING {
        for (kind_case, kind) in [("a component", Kind::Component), ("a theme", Kind::Theme)] {
            let case = format!("{width} writers on {kind_case}");
            let emulator = Arc::new(emulator());
            let data = match kind {
                Kind::Component => component_data(0),
                Kind::Theme => theme_data(0),
                Kind::CodegenJob => json!({}),
            };
            let created = call(&emulator, "POST", &kind.collection(), Some(data)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the setup create expected HTTP 200, got {} with {:?}",
                created.status, created.body
            );
            let id = created.id();

            // Every writer merges into the ONE record, so their commits are ordered by
            // the store lock and nothing else.
            let mut racing = Vec::new();
            for writer in 0..width {
                racing.push(tokio::spawn({
                    let emulator = Arc::clone(&emulator);
                    let uri = format!("{}/{id}", kind.collection());
                    let body = patch(kind, writer);
                    async move { call(&emulator, "PATCH", &uri, Some(body)).await }
                }));
            }

            let mut reported: Vec<String> = Vec::new();
            for (writer, task) in racing.into_iter().enumerate() {
                let answer = task.await.expect("a racing update ran to completion");
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: writer {writer} expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                reported.push(stamp_text(
                    &case,
                    "the modifiedAt a writer reported",
                    &answer.body["modifiedAt"],
                ));
            }

            let latest = reported
                .iter()
                .max_by_key(|text| stamp_order(&case, "the modifiedAt a writer reported", text))
                .expect("each row has at least two writers")
                .clone();
            let stored = stored_stamps(&emulator, &case, kind, &id);
            assert_eq!(
                as_wire(stored.modified_at),
                latest,
                "case {case:?}: the record must keep the latest modifiedAt its writers \
                 reported ({latest}), got {} (the writers reported {reported:?})",
                as_wire(stored.modified_at)
            );

            // And the reader reports that same record.
            let read = call(
                &emulator,
                "GET",
                &format!("{}/{id}", kind.collection()),
                None,
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: reading the record back expected HTTP 200, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body["modifiedAt"],
                json!(latest),
                "case {case:?}: the reader must report the modifiedAt the record keeps \
                 ({latest}), got {:?}",
                read.body["modifiedAt"]
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrently_created_records_are_stamped_in_the_order_they_were_stored() {
    for width in RACING {
        let case = format!("{width} concurrent creates");
        let emulator = Arc::new(emulator());

        let mut racing = Vec::new();
        for maker in 0..width {
            racing.push(tokio::spawn({
                let emulator = Arc::clone(&emulator);
                let uri = Kind::Component.collection();
                let body = component_data(maker);
                async move { call(&emulator, "POST", &uri, Some(body)).await }
            }));
        }
        for (maker, task) in racing.into_iter().enumerate() {
            let answer = task.await.expect("a racing create ran to completion");
            assert_eq!(
                answer.status, 200,
                "case {case:?}: create {maker} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
        }

        // `ExportComponents` serves an app environment's components in the order they were
        // stored, so the stamps along that order are the stamps in commit order.
        let exported = call(
            &emulator,
            "GET",
            &format!("/export/app/{APP_ID}/environment/{ENVIRONMENT}/components"),
            None,
        )
        .await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: ExportComponents expected HTTP 200, got {} with {:?}",
            exported.status, exported.body
        );
        let entities = exported.body["entities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no entities in {:?}", exported.body))
            .clone();
        assert_eq!(
            entities.len(),
            width,
            "case {case:?}: every create must have stored a component, got {:?}",
            entities.len()
        );

        let mut previous: Option<(i64, u32)> = None;
        for (position, entity) in entities.iter().enumerate() {
            let created_at = stamp_text(&case, "createdAt", &entity["createdAt"]);
            let stamp = stamp_order(&case, "createdAt", &created_at);
            if let Some(previous) = previous {
                assert!(
                    stamp >= previous,
                    "case {case:?}: the component at position {position} was stored after \
                     the one before it, so its createdAt ({stamp:?}) cannot be earlier \
                     than {previous:?}"
                );
            }
            previous = Some(stamp);
            // Nothing has updated these components, so the change that first set
            // `modifiedAt` is the create itself.
            assert_eq!(
                entity["modifiedAt"],
                json!(created_at),
                "case {case:?}: the component at position {position} was never updated, so \
                 its modifiedAt must be its createdAt ({created_at}), got {:?}",
                entity["modifiedAt"]
            );
        }
    }
}
