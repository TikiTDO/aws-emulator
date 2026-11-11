// SPDX-License-Identifier: CC0-1.0

//! The values a form's sectional elements admit, over both form write paths.
//!
//! The rule under test: a member whose valid values the model spells out in prose while
//! targeting a plain `String`/`Integer` shape admits only those values, so a write
//! carrying anything else is refused with HTTP 400 and stores nothing, while every
//! documented value is accepted and reported back unchanged.
//!
//! The members the rule covers here, from the model
//! (`smithy_query(query="shape",shape="SectionalElement")`):
//!
//! * `SectionalElement$type` — "Valid values are `Heading`, `Text`, and `Divider`";
//! * `SectionalElement$level` — "Valid values are `1 | 2 | 3 | 4 | 5 | 6`";
//! * `SectionalElement$orientation` — "Valid values are `horizontal` or `vertical`".
//!
//! Both write paths carry the map — `CreateForm`'s `formToCreate.sectionalElements` and
//! `UpdateForm`'s `updatedForm.sectionalElements` — so every case below is driven through
//! each of them, and each acceptance is read back through `GetForm`. The fixed rows span
//! the published catalogue and each published bound of `level` plus one step past it; the
//! generated suite then puts an out-of-set value at an arbitrary key of an arbitrary map,
//! so no map size, key spelling or element position is left to a hand-written row.
//!
//! Each suite in this crate carries its own request harness (see `form_update.rs`), so
//! this one does too.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// Every value `SectionalElement$type` admits, from
/// `smithy_query(query="shape",shape="SectionalElement")`.
const SECTIONAL_ELEMENT_TYPES: [&str; 3] = ["Heading", "Text", "Divider"];

/// Every value `SectionalElement$orientation` admits, from the same shape.
const SECTIONAL_ELEMENT_ORIENTATIONS: [&str; 2] = ["horizontal", "vertical"];

/// The published bounds of `SectionalElement$level` ("Valid values are
/// `1 | 2 | 3 | 4 | 5 | 6`"), from the same shape.
const LEVEL_MIN: i64 = 1;
const LEVEL_MAX: i64 = 6;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The form id the answer carries, or the empty string. `CreateForm`, `UpdateForm` and
    /// `GetForm` all bind their form to `@httpPayload`, so the body IS the `Form`.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The `message` the refusal carries, or the empty string.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
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

    let response = router.dispatch(request).await;
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

fn forms_uri() -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/forms")
}

async fn create(router: &MultiServiceRouter, data: Value) -> Answer {
    call(router, "POST", &forms_uri(), Some(data)).await
}

async fn update(router: &MultiServiceRouter, id: &str, patch: Value) -> Answer {
    let uri = format!("{}/{id}", forms_uri());
    call(router, "PATCH", &uri, Some(patch)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", forms_uri());
    call(router, "GET", &uri, None).await
}

async fn list(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", &forms_uri(), None).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A one-entry JSON object, for a case whose key is computed rather than written out.
fn one_entry(key: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), value);
    Value::Object(map)
}

/// A create payload under `name`, carrying every `@required` member of `CreateFormData` and
/// the sectional elements the case is about.
fn create_data(name: &str, elements: Value) -> Value {
    json!({
        "name": name,
        "formActionType": "create",
        "dataType": {"dataSourceType": "Custom", "dataTypeName": "Order"},
        "fields": {},
        "style": {},
        "sectionalElements": elements,
        "schemaVersion": "1",
    })
}

/// The sectional-element maps the rule accepts: every documented `type`, every documented
/// `orientation`, and `level` at each published value — the rows differ along the value,
/// the member carrying it and how many entries the map holds.
fn accepted_maps() -> Vec<(String, Value)> {
    let mut cases: Vec<(String, Value)> = Vec::new();

    for element_type in SECTIONAL_ELEMENT_TYPES {
        cases.push((
            format!("the documented type {element_type:?}"),
            one_entry("element", json!({"type": element_type})),
        ));
    }
    for level in LEVEL_MIN..=LEVEL_MAX {
        cases.push((
            format!("a Heading at the documented level {level}"),
            one_entry("heading", json!({"type": "Heading", "level": level})),
        ));
    }
    for orientation in SECTIONAL_ELEMENT_ORIENTATIONS {
        cases.push((
            format!("a Divider with the documented orientation {orientation:?}"),
            one_entry("divider", json!({"type": "Divider", "orientation": orientation})),
        ));
    }
    cases.push((
        "an empty map, which carries no element to judge".to_string(),
        json!({}),
    ));
    cases.push((
        "an element carrying no level and no orientation at all".to_string(),
        one_entry("note", json!({"type": "Text", "text": "note"})),
    ));
    cases.push((
        "every documented value at once, across several entries".to_string(),
        json!({
            "heading": {"type": "Heading", "level": LEVEL_MAX, "text": "Order"},
            "note": {"type": "Text", "text": "note", "position": {"rightOf": "heading"}},
            "divider": {"type": "Divider", "orientation": "vertical", "excluded": false},
        }),
    ));

    cases
}

/// One refused row: what the case is, the member path the refusal must name, the value it
/// must quote, the valid values it must list, and the map the request carries.
struct Refused {
    case: String,
    path: String,
    value: String,
    valid: String,
    elements: Value,
}

/// The valid-values list a refusal of `member` carries, in the order the model lists them.
fn valid_values(member: &str) -> String {
    match member {
        "type" => SECTIONAL_ELEMENT_TYPES.join(", "),
        "orientation" => SECTIONAL_ELEMENT_ORIENTATIONS.join(", "),
        "level" => (LEVEL_MIN..=LEVEL_MAX)
            .map(|level| level.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        other => panic!("no documented value set for {other:?}"),
    }
}

fn refused(case: &str, key: &str, member: &str, value: &str, elements: Value) -> Refused {
    Refused {
        case: case.to_string(),
        path: format!("sectionalElements.{key}.{member}"),
        value: value.to_string(),
        valid: valid_values(member),
        elements,
    }
}

/// The maps the rule refuses: a value just outside each documented set, each published
/// bound of `level` overstepped in both directions, a case spelling, an empty string, and
/// an out-of-set element sitting among valid ones.
fn refused_maps() -> Vec<Refused> {
    let mut cases = vec![
        refused(
            "a type the model does not document",
            "x",
            "type",
            "Banana",
            one_entry("x", json!({"type": "Banana"})),
        ),
        refused(
            "a type differing from a documented one only in case",
            "x",
            "type",
            "heading",
            one_entry("x", json!({"type": "heading"})),
        ),
        refused(
            "a type that is a documented one with a suffix",
            "x",
            "type",
            "Headings",
            one_entry("x", json!({"type": "Headings"})),
        ),
        refused(
            "an empty type",
            "x",
            "type",
            "",
            one_entry("x", json!({"type": ""})),
        ),
        refused(
            "an orientation the model does not document",
            "divider",
            "orientation",
            "sideways",
            one_entry("divider", json!({"type": "Divider", "orientation": "sideways"})),
        ),
        refused(
            "an orientation differing from a documented one only in case",
            "divider",
            "orientation",
            "Horizontal",
            one_entry(
                "divider",
                json!({"type": "Divider", "orientation": "Horizontal"}),
            ),
        ),
        refused(
            "an empty orientation",
            "divider",
            "orientation",
            "",
            one_entry("divider", json!({"type": "Divider", "orientation": ""})),
        ),
        refused(
            "an out-of-set element sitting after a valid one, by key order",
            "second",
            "type",
            "Banner",
            json!({
                "first": {"type": "Heading", "level": 2},
                "second": {"type": "Banner"},
            }),
        ),
        refused(
            "the lowest-keyed of two out-of-set elements",
            "alpha",
            "type",
            "Alpha",
            json!({
                "alpha": {"type": "Alpha"},
                "beta": {"type": "Beta"},
            }),
        ),
        refused(
            "an element whose type and level are both out of set names the type first",
            "x",
            "type",
            "Banana",
            one_entry("x", json!({"type": "Banana", "level": 99})),
        ),
    ];

    // Each published bound of `level`, overstepped by one, and further out on both sides.
    for level in [LEVEL_MIN - 1, LEVEL_MAX + 1, -1, 99, i64::from(i32::MIN)] {
        cases.push(refused(
            "a level outside the documented values",
            "heading",
            "level",
            &level.to_string(),
            one_entry("heading", json!({"type": "Heading", "level": level})),
        ));
    }

    cases
}

/// Every documented value is accepted on both write paths and reported back unchanged by
/// `GetForm`: the rule refuses values outside the documented sets, not inside them.
#[tokio::test]
async fn every_documented_sectional_element_value_round_trips_on_both_write_paths() {
    let router = emulator();

    for (row, (case, elements)) in accepted_maps().into_iter().enumerate() {
        // The create path carries the map in `formToCreate`.
        let created = create(&router, create_data(&format!("Created-{row}"), elements.clone())).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateForm, got {} with {:?}",
            created.status, created.body
        );
        let read = get(&router, &created.id()).await;
        assert_eq!(
            at(&read.body, "/sectionalElements"),
            elements,
            "case {case:?}: GetForm must report the created map, got {:?}",
            at(&read.body, "/sectionalElements")
        );

        // And the update path carries it in `updatedForm`, onto a form created without it.
        let target = create(&router, create_data(&format!("Patched-{row}"), json!({}))).await;
        let id = target.id();
        let updated = update(&router, &id, json!({"sectionalElements": elements.clone()})).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateForm, got {} with {:?}",
            updated.status, updated.body
        );
        let read = get(&router, &id).await;
        assert_eq!(
            at(&read.body, "/sectionalElements"),
            elements,
            "case {case:?}: GetForm must report the patched map, got {:?}",
            at(&read.body, "/sectionalElements")
        );
    }
}

/// A value outside a documented set is refused with HTTP 400 on both write paths, the
/// refusal names the member the caller wrote, quotes the value and lists the documented
/// ones, and nothing is stored: no form is created, and a refused patch leaves the form
/// exactly as it was.
#[tokio::test]
async fn a_sectional_element_value_outside_the_documented_set_is_refused_and_stores_nothing() {
    for (row, refusal) in refused_maps().into_iter().enumerate() {
        let router = emulator();
        let Refused {
            case,
            path,
            value,
            valid,
            elements,
        } = refusal;

        // The create path: refused, and no form exists afterwards.
        let name = format!("Refused-{row}");
        let answer = create(&router, create_data(&name, elements.clone())).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from CreateForm, got {} with {:?}",
            answer.status, answer.body
        );
        for expected in [format!("formToCreate.{path}"), value.clone(), valid.clone()] {
            assert!(
                answer.message().contains(&expected),
                "case {case:?}: the CreateForm refusal must carry {expected:?}, got {:?}",
                answer.message()
            );
        }
        let listed = list(&router).await;
        assert_eq!(
            at(&listed.body, "/entities"),
            json!([]),
            "case {case:?}: a refused create must store no form, got {:?}",
            at(&listed.body, "/entities")
        );

        // The update path: refused on a form that already exists, which keeps what it held.
        let created = create(&router, create_data(&name, json!({"ok": {"type": "Text"}}))).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the baseline form must be created, got {} with {:?}",
            created.status, created.body
        );
        let id = created.id();
        let answer = update(&router, &id, json!({"sectionalElements": elements})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from UpdateForm, got {} with {:?}",
            answer.status, answer.body
        );
        for expected in [format!("updatedForm.{path}"), value, valid] {
            assert!(
                answer.message().contains(&expected),
                "case {case:?}: the UpdateForm refusal must carry {expected:?}, got {:?}",
                answer.message()
            );
        }
        let read = get(&router, &id).await;
        assert_eq!(
            read.body, created.body,
            "case {case:?}: a refused update must store nothing, got {:?} for {:?}",
            read.body, created.body
        );
    }
}

/// A patch carrying a valid map alongside an out-of-set one in the SAME request is refused
/// whole: the merge is all or nothing, so no other member of the patch lands either.
#[tokio::test]
async fn a_refused_patch_leaves_every_other_patched_member_alone() {
    let router = emulator();
    let created = create(&router, create_data("Whole", json!({"ok": {"type": "Text"}}))).await;
    let id = created.id();

    let answer = update(
        &router,
        &id,
        json!({
            "name": "RenamedByARefusedPatch",
            "schemaVersion": "9.9",
            "sectionalElements": {"bad": {"type": "Banana"}},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "a patch carrying an out-of-set sectional element must be refused, got {} with {:?}",
        answer.status, answer.body
    );
    let read = get(&router, &id).await;
    assert_eq!(
        read.body, created.body,
        "a refused patch must leave every member alone, got {:?} for {:?}",
        read.body, created.body
    );
}

/// How many generated payloads the generated suite judges. Bounded, so this stays a test
/// rather than a soak.
const GENERATED_CASES: usize = 32;

/// The base every generated case's seed is derived from, so the suite judges the same
/// payloads on every run and a reported failure is reproducible.
const GENERATOR_SEED: u64 = 0x5ec7_1041_b33f_0001;

/// The seeded generator the cases below draw their payloads from: SplitMix64, the mixing
/// function a 64-bit seed is expanded with.
///
/// Written here rather than taken from a generator crate for the reason
/// `component_reserved_property_name.rs` gives: a dev-dependency moves this crate's
/// `Cargo.toml` and the workspace `Cargo.lock`, the two files every other fix also moves.
/// Eight lines of arithmetic, nothing on disk, and the seed is a constant every assertion
/// message names.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value below `bound`, for `bound` above zero.
    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// An arbitrary map key, drawn from spellings the protocol admits.
fn generated_key(rng: &mut SplitMix64, index: usize) -> String {
    let shapes = ["element", "e", "address.line_1", "42", "título", "Heading"];
    format!("{}{index}", shapes[rng.below(shapes.len())])
}

/// An arbitrary element inside the documented sets.
fn generated_valid_element(rng: &mut SplitMix64) -> Value {
    let element_type = SECTIONAL_ELEMENT_TYPES[rng.below(SECTIONAL_ELEMENT_TYPES.len())];
    let mut element = json!({"type": element_type});
    if rng.below(2) == 0 {
        let level = LEVEL_MIN + rng.below((LEVEL_MAX - LEVEL_MIN + 1) as usize) as i64;
        element["level"] = json!(level);
    }
    if rng.below(2) == 0 {
        let orientation =
            SECTIONAL_ELEMENT_ORIENTATIONS[rng.below(SECTIONAL_ELEMENT_ORIENTATIONS.len())];
        element["orientation"] = json!(orientation);
    }
    element
}

/// The rule holds for a payload of any size and key spelling: a generated map of valid
/// elements is accepted and read back, and the same map with ONE member of ONE element put
/// outside its documented set is refused, on the create and on the update alike.
#[tokio::test]
async fn an_out_of_set_value_is_refused_in_a_generated_payload() {
    for index in 0..GENERATED_CASES {
        // One seed per case, from the fixed base: the payloads differ case to case and
        // repeat run to run, and every assertion message carries the seed that built the
        // case it failed on.
        let seed = GENERATOR_SEED ^ index as u64;
        let mut rng = SplitMix64(seed);

        let size = 1 + rng.below(5);
        let mut keys: Vec<String> = Vec::new();
        let mut valid = Map::new();
        for entry in 0..size {
            let key = generated_key(&mut rng, entry);
            valid.insert(key.clone(), generated_valid_element(&mut rng));
            keys.push(key);
        }
        let valid = Value::Object(valid);

        // Which element, and which of its members, is put outside its set.
        let target = keys[rng.below(keys.len())].clone();
        let (member, value) = match rng.below(3) {
            0 => ("type", json!("Banana")),
            1 => ("level", json!(LEVEL_MAX + 1 + rng.below(90) as i64)),
            _ => ("orientation", json!("sideways")),
        };
        let mut broken = valid.clone();
        broken[&target][member] = value.clone();
        // `level` and `orientation` only exist on an element that carries them, and the
        // refusal is about the value, so the type stays inside its set here.
        let quoted = match value {
            Value::String(ref text) => text.clone(),
            ref other => other.to_string(),
        };
        let case = format!(
            "seed {seed}, case {index}: {member} of element {target:?} outside its set, in a map \
             of {size}"
        );

        let router = emulator();

        // The valid map is accepted and reported back.
        let created = create(&router, create_data("Generated", valid.clone())).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the generated valid map must be accepted, got {} with {:?}",
            created.status, created.body
        );
        let id = created.id();
        let read = get(&router, &id).await;
        assert_eq!(
            at(&read.body, "/sectionalElements"),
            valid,
            "case {case:?}: GetForm must report the generated valid map, got {:?}",
            at(&read.body, "/sectionalElements")
        );

        // The same map with one value outside its set is refused on both write paths, and
        // the refusal names that element's member.
        let refused_create = create(&router, create_data("GeneratedBad", broken.clone())).await;
        assert_eq!(
            refused_create.status, 400,
            "case {case:?}: CreateForm must refuse the broken map, got {} with {:?}",
            refused_create.status, refused_create.body
        );
        let refused_update = update(&router, &id, json!({"sectionalElements": broken})).await;
        assert_eq!(
            refused_update.status, 400,
            "case {case:?}: UpdateForm must refuse the broken map, got {} with {:?}",
            refused_update.status, refused_update.body
        );
        for (payload_member, answer) in [
            ("formToCreate", &refused_create),
            ("updatedForm", &refused_update),
        ] {
            let expected = format!("{payload_member}.sectionalElements.{target}.{member}");
            assert!(
                answer.message().contains(&expected),
                "case {case:?}: the refusal must name {expected:?}, got {:?}",
                answer.message()
            );
            assert!(
                answer.message().contains(&quoted),
                "case {case:?}: the refusal must quote {quoted:?}, got {:?}",
                answer.message()
            );
        }

        // And the refused update left the stored map alone.
        let read = get(&router, &id).await;
        assert_eq!(
            at(&read.body, "/sectionalElements"),
            valid,
            "case {case:?}: a refused update must leave the stored map alone, got {:?}",
            at(&read.body, "/sectionalElements")
        );
    }
}
