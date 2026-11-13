// SPDX-License-Identifier: CC0-1.0

//! The Region-opt-target refusal, driven over the WHOLE Region catalog this
//! service reports rather than over a sample of it, end to end over HTTP.
//!
//! The rule: a Region this service reports as turned on for every account is not
//! a Region-opt target, so BOTH opt operations (`EnableRegion`, `DisableRegion`)
//! refuse it 400 `ValidationException` whose `message` is the fixed sentence
//! ` is not a valid region for opt-in or opt-out.` with the `RegionName` the
//! request carried in front of it, whose `reason` is `fieldValidationFailed` and
//! whose `fieldList` holds one entry naming `RegionName` and carrying that same
//! sentence; the refusal writes nothing, so the Region keeps the status the
//! catalog gives it. The members are the ones the model declares for
//! `ValidationException` — `message` (required), `reason`
//! (`ValidationExceptionReason`) and `fieldList` ("The field where the invalid
//! entry was detected") — from
//! `smithy_query(query="shape", shape="ValidationException")`.
//!
//! The sibling suites pin this rule on five hand-picked Regions each. This one
//! generalises along a dimension none of them covers: it ASKS the service which
//! Regions it holds (`ListRegions`, every row in one page) and then requires the
//! rule of EVERY row of the enabled-by-default column, with the opt-in column as
//! the counter-case — so a Region added to or moved inside the catalog is held to
//! the rule too, and the refusal cannot be keyed to a particular Region code.
//! The wording is additionally pinned as a TEMPLATE: across rows, the sentence
//! with the requested Region code masked out must be one and the same string, so
//! the only thing that varies with the request is the Region code it named.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The wire error code and status of the modelled `ValidationException`, from
/// `smithy_query(query="shape", shape="ValidationException")` (`errorKind:
/// client`, `httpError: 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` a refusal detected on one input member
/// reports, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`, whose two
/// values are `invalidRegionOptTarget` and `fieldValidationFailed`.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The input member both opt operations decide on, from
/// `smithy_query(query="operation", operation="EnableRegion,DisableRegion")`
/// (`RegionName`, `@required`, `@length(min: 1, max: 50)`).
const REGION_NAME_MEMBER: &str = "RegionName";

/// The `RegionOptStatus` values this suite reads, from
/// `smithy_query(query="shape", shape="RegionOptStatus")`.
const ENABLED_BY_DEFAULT: &str = "ENABLED_BY_DEFAULT";
const DISABLED: &str = "DISABLED";

/// What a Region code is masked to when the sentence is compared across rows: a
/// string no Region code contains, so masking cannot hide a difference.
const MASK: &str = "\u{0}";

/// Regions Amazon Web Services turns on for every account. SOURCE: the AWS
/// General Reference Region table "Available Regions"
/// (<https://docs.aws.amazon.com/general/latest/gr/rande-manage.html>), the rows
/// marked "Enabled by default". Spelled out here rather than imported, so an
/// empty or wrong enabled-by-default column in the implementation cannot make
/// this suite pass; five rows from five geographies, none of them the Region the
/// issue behind this suite cited.
const DEFAULT_ENABLED: [&str; 5] = [
    "ap-southeast-1",
    "eu-west-1",
    "us-east-2",
    "ap-northeast-3",
    "eu-central-1",
];

/// The two operations that decide by the opt-target rule, each as the name this
/// suite reports it by and the URI it is reached at, from each operation's
/// `@http` binding (`POST /enableRegion`, `POST /disableRegion`).
const OPT_OPERATIONS: [(&str, &str); 2] = [
    ("EnableRegion", "/enableRegion"),
    ("DisableRegion", "/disableRegion"),
];

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
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The `name`/`message` pairs of this answer's `fieldList` (empty when the
    /// member is absent).
    fn field_list(&self) -> Vec<(String, String)> {
        self.body["fieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| {
                        (
                            field["name"].as_str().unwrap_or_default().to_string(),
                            field["message"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
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
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The sentence both opt operations refuse a Region that is not an opt target
/// with: fixed text, with the Region code the request named in front of it.
fn not_an_opt_target(region_name: &str) -> String {
    format!("{region_name} is not a valid region for opt-in or opt-out.")
}

/// Every Region this service holds, with the `RegionOptStatus` it reports for the
/// caller's own account, read off one unpaged `ListRegions`.
async fn catalog(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let answer = call(router, "/listRegions", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "ListRegions must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body["NextToken"].is_null(),
        "the whole catalog must fit one unpaged ListRegions, got {:?}",
        answer.body
    );
    answer.body["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("a ListRegions success carries Regions, got {:?}", answer.body))
        .iter()
        .map(|region| {
            (
                region["RegionName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a listed Region carries a RegionName: {region:?}"))
                    .to_string(),
                region["RegionOptStatus"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("a listed Region carries a RegionOptStatus: {region:?}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The `RegionOptStatus` the emulator reports for `region_name` on the caller's
/// own account.
async fn status_of(router: &MultiServiceRouter, region_name: &str) -> String {
    let answer = call(
        router,
        "/getRegionOptStatus",
        json!({ REGION_NAME_MEMBER: region_name }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {region_name:?}: GetRegionOptStatus must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("RegionOptStatus").to_string()
}

/// Both opt calls refuse EVERY Region the service reports as turned on for every
/// account, with the whole payload the rule states, and write nothing; every
/// Region of the other column is accepted as the opt target it is.
#[tokio::test]
async fn every_enabled_by_default_region_of_the_catalog_earns_the_opt_target_refusal() {
    let router = emulator();
    let rows = catalog(&router).await;

    let default_enabled: Vec<&(String, String)> =
        rows.iter().filter(|(_, status)| status == ENABLED_BY_DEFAULT).collect();
    let opt_in: Vec<&(String, String)> =
        rows.iter().filter(|(_, status)| status == DISABLED).collect();
    assert!(
        default_enabled.len() >= DEFAULT_ENABLED.len() && !opt_in.is_empty(),
        "case \"the catalog\": both Region columns must be populated, got {} \
         {ENABLED_BY_DEFAULT} and {} {DISABLED} rows in {rows:?}",
        default_enabled.len(),
        opt_in.len()
    );

    for (region, _) in &default_enabled {
        let sentence = not_an_opt_target(region);
        let mut bodies = Vec::new();
        for (operation, uri) in OPT_OPERATIONS {
            let answer = call(&router, uri, json!({ REGION_NAME_MEMBER: region })).await;
            assert_eq!(
                answer.status, VALIDATION_STATUS,
                "case {region:?}/{operation}: a Region turned on for every account is not an \
                 opt target, so the call must answer {VALIDATION_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                VALIDATION_EXCEPTION,
                "case {region:?}/{operation}: expected {VALIDATION_EXCEPTION}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.member("message"),
                sentence,
                "case {region:?}/{operation}: expected the opt-target sentence {sentence:?}, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                answer.member("reason"),
                FIELD_VALIDATION_FAILED,
                "case {region:?}/{operation}: expected reason {FIELD_VALIDATION_FAILED}, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                answer.field_list(),
                vec![(REGION_NAME_MEMBER.to_string(), sentence.clone())],
                "case {region:?}/{operation}: fieldList must hold one entry naming \
                 {REGION_NAME_MEMBER} with that same sentence, got {:?}",
                answer.body
            );
            bodies.push((operation, answer));
        }

        // One rule, so neither direction of the opt lifecycle can answer this
        // Region differently from the other.
        let (first, second) = (&bodies[0].1, &bodies[1].1);
        assert_eq!(
            (first.status, &first.body),
            (second.status, &second.body),
            "case {region:?}: {} answered {} with {:?} where {} answered {} with {:?}",
            bodies[0].0,
            first.status,
            first.body,
            bodies[1].0,
            second.status,
            second.body
        );

        // A refused call writes nothing: the status a later caller reads is the
        // one the catalog gives the Region.
        assert_eq!(
            status_of(&router, region).await,
            ENABLED_BY_DEFAULT,
            "case {region:?}: a refused opt call must leave the reported status alone"
        );
    }

    // The counter-case over the whole other column: every opt-in Region IS an
    // opt target, so the refusal does not reach it. `DisableRegion` is the
    // direction that asks for the status such a Region already has, so it leaves
    // the catalog as the next row finds it.
    for (region, _) in &opt_in {
        let answer = call(&router, "/disableRegion", json!({ REGION_NAME_MEMBER: region })).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}/DisableRegion: an opt-in Region is an opt target, so the call must \
             answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            status_of(&router, region).await,
            DISABLED,
            "case {region:?}: disabling an already {DISABLED} Region must leave it {DISABLED}"
        );
    }
}

/// The sentence is a TEMPLATE: its fixed text is one string for every Region, and
/// the only part that varies is the Region code the request named — so it is
/// taken from the request rather than from any one recorded value.
#[tokio::test]
async fn the_opt_target_sentence_is_fixed_text_with_the_requested_region_interpolated() {
    let router = emulator();
    let mut masked: BTreeSet<String> = BTreeSet::new();

    for region in DEFAULT_ENABLED {
        assert_eq!(
            status_of(&router, region).await,
            ENABLED_BY_DEFAULT,
            "case {region:?}: a Region Amazon Web Services turns on for every account must \
             report {ENABLED_BY_DEFAULT}"
        );

        for (operation, uri) in OPT_OPERATIONS {
            let answer = call(&router, uri, json!({ REGION_NAME_MEMBER: region })).await;
            assert_eq!(
                answer.member("message"),
                not_an_opt_target(region),
                "case {region:?}/{operation}: expected the opt-target sentence, got {:?}",
                answer.body
            );
            let fields = answer.field_list();
            assert_eq!(
                fields.len(),
                1,
                "case {region:?}/{operation}: fieldList must hold exactly one entry, got {:?}",
                answer.body
            );
            assert_eq!(
                fields[0].0, REGION_NAME_MEMBER,
                "case {region:?}/{operation}: the entry must name the member the refusal is \
                 about, got {:?}",
                answer.body
            );
            assert_eq!(
                fields[0].1,
                answer.member("message"),
                "case {region:?}/{operation}: the entry must carry the message's sentence, \
                 got {:?}",
                answer.body
            );
            masked.insert(answer.member("message").replace(region, MASK));
        }
    }

    assert_eq!(
        masked.len(),
        1,
        "case \"the sentence's fixed text\": masking the requested Region code must leave one \
         and the same sentence for every Region, got {masked:?}"
    );
    let fixed_text = format!("{MASK} is not a valid region for opt-in or opt-out.");
    assert_eq!(
        masked.iter().next(),
        Some(&fixed_text),
        "case \"the sentence's fixed text\": unexpected fixed text, got {masked:?}"
    );
}
