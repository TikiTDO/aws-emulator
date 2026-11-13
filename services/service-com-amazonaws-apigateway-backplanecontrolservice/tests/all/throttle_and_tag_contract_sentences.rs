// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the sentences API Gateway refuses a THROTTLE limit and a TAG
//! outside its contract with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `GetStage` / `CreateUsagePlan` / `GetUsagePlan` /
//! `GetTags` helpers are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `stage_lifecycle`, `usage_plan_lifecycle` and `tag_reads_by_arn`.
//!
//! The RULES under test, each over a TABLE of inputs rather than one recorded call:
//!
//! * where a SETTING publishes a refusal sentence of its own, that sentence is the answer
//!   for every value the setting cannot take — a text that is no number as much as a number
//!   outside the setting's rule — and it carries neither the JSON Pointer the value arrived
//!   on nor the value itself: a method's throttle limits answer the stage's throttling
//!   sentence, and a usage plan's answer the one that shape publishes;
//! * every tag outside the tag key/value contract — an empty key as much as a key or a
//!   value past its bound — earns the one fixed sentence stating that contract, whichever
//!   operation the tag rode on;
//! * each refusal leaves the record exactly as it was, and the values the setting and the
//!   contract DO admit — the published bounds included — still apply and read back.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::{create_deployed_stage, get_stage};
use crate::tag_reads_by_arn::{apigateway_arn, encode_label, get_tags};
use crate::usage_plan_lifecycle::{create_usage_plan, get_usage_plan};

/// The modelled refusal class every case here asserts.
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// The stage every stage row is deployed to.
const STAGE: &str = "prod";

/// The sentence API Gateway refuses a METHOD SETTING's throttle limit with: entirely fixed
/// text, naming neither the limit, the method-setting path nor the value.
const INVALID_THROTTLING_SETTING: &str = "Invalid throttling setting";

/// The sentence API Gateway refuses a USAGE PLAN's throttle limit with: entirely fixed text
/// naming both limits of the shape and the rule they break.
const INVALID_USAGE_PLAN_THROTTLE: &str =
    "Usage Plan throttle rate limit and burst limit must be numeric and greater than zero";

/// The one sentence every tag key/value violation earns: fixed text stating the contract —
/// the permitted characters and the two length bounds — and no rejected key, value or
/// length.
fn invalid_tag() -> String {
    format!(
        "Invalid tag key/value. Valid characters are letters, numbers, spaces, and the \
         following: _ . : / = + - @. Tag key can be up to {MAX_TAG_KEY_CHARS} characters. \
         Tag value can be up to {MAX_TAG_VALUE_CHARS} characters."
    )
}

/// One `replace` of `path` with `value`.
fn replace(path: &str, value: &str) -> Value {
    json!({ "op": "replace", "path": path, "value": value })
}

/// `UpdateStage` for [`STAGE`] of `api` with one operation.
async fn update_stage(gateway: &Gateway, api: &str, operation: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{STAGE}"),
            Some(json!({ "patchOperations": [operation] })),
        )
        .await
}

/// `UpdateUsagePlan` for `plan` with one operation.
async fn update_usage_plan(gateway: &Gateway, plan: &str, operation: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/usageplans/{plan}"),
            Some(json!({ "patchOperations": [operation] })),
        )
        .await
}

/// `TagResource` for `arn` with the one tag `key`/`value`.
async fn tag_resource(gateway: &Gateway, arn: &str, key: &str, value: &str) -> Answer {
    gateway
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({ "tags": { key: value } })),
        )
        .await
}

/// An API deployed to [`STAGE`], answering the API id.
async fn deployed_api(gateway: &Gateway, name: &str) -> String {
    let (api, _root) = create_api(gateway, name).await;
    create_deployed_stage(gateway, &api, STAGE).await;
    api
}

/// Assert `answer` is the modelled failure `expected` carrying exactly `message`.
fn assert_refusal(answer: &Answer, expected: (u16, &str), message: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// The two method scopes every method-setting row is driven under: the stage-wide wildcard
/// and one named method, so a refusal is about the SETTING and not about the scope.
const METHOD_SCOPES: [&str; 2] = ["/*/*", "/~1pets/GET"];

/// The values no throttle limit of either kind can take, each with the case it stands for:
/// a word, an empty value, a value carrying a unit, one negative, one far negative, and the
/// two texts that spell a number which is not finite.
const UNACCEPTABLE_LIMITS: [(&str, &str); 7] = [
    ("no number at all", "abc"),
    ("an empty value", ""),
    ("a number carrying a unit", "10rps"),
    ("a negative limit", "-1"),
    ("a far negative limit", "-100000"),
    ("an infinity", "Infinity"),
    ("a not-a-number", "NaN"),
];

/// The values only an INTEGER limit refuses, beside [`UNACCEPTABLE_LIMITS`]: a count of
/// requests is a whole number inside the modelled `Integer`, so a fractional text and one
/// past that type are outside it where a rate takes both.
const UNACCEPTABLE_INTEGER_LIMITS: [(&str, &str); 3] = [
    ("a fraction of a request", "10.5"),
    ("one past the modelled integer", "2147483648"),
    ("far past the modelled integer", "99999999999999"),
];

#[tokio::test]
async fn a_method_throttle_limit_the_setting_cannot_take_earns_one_fixed_sentence() {
    let gateway = Gateway::new();
    let api = deployed_api(&gateway, "throttle-setting-sentence").await;
    let before = get_stage(&gateway, &api, STAGE).await;

    // Rows differ along HOW the value breaks the rule and along WHICH limit carried it —
    // the burst, a count of requests, and the rate, a requests-per-second number — because
    // the one sentence covers both limits and every value either refuses.
    let burst: Vec<(&str, &str)> = UNACCEPTABLE_LIMITS
        .iter()
        .chain(UNACCEPTABLE_INTEGER_LIMITS.iter())
        .copied()
        .collect();
    let limits: [(&str, &[(&str, &str)]); 2] = [
        ("throttling/burstLimit", &burst),
        ("throttling/rateLimit", &UNACCEPTABLE_LIMITS),
    ];
    for scope in METHOD_SCOPES {
        for (limit, rows) in &limits {
            for (reason, value) in *rows {
                let path = format!("{scope}/{limit}");
                let case = format!("replace {path} with {value:?} ({reason})");
                let answer = update_stage(&gateway, &api, replace(&path, value)).await;
                assert_refusal(&answer, BAD_REQUEST, INVALID_THROTTLING_SETTING, &case);
                // A refused patch writes nothing: the stage is exactly as it was.
                let read = get_stage(&gateway, &api, STAGE).await;
                assert_eq!(
                    read.body, before.body,
                    "case {case}: expected the stage left at {:?}, got {:?}",
                    before.body, read.body
                );
            }
        }
    }

    // The limits the setting DOES take apply and read back, so every refusal above is about
    // the value and not about the path: the bottom of each limit's range, a limit the
    // account's documented default publishes, the top of the burst's modelled integer, and
    // a rate spelled in exponent form.
    for (value, expected) in [("0", 0), ("5000", 5000), ("2147483647", i32::MAX)] {
        let case = format!("replace /*/*/throttling/burstLimit with {value:?}");
        let answer =
            update_stage(&gateway, &api, replace("/*/*/throttling/burstLimit", value)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get_stage(&gateway, &api, STAGE).await;
        let actual = read.body["methodSettings"]["*/*"]["throttlingBurstLimit"].as_i64();
        assert_eq!(
            actual,
            Some(i64::from(expected)),
            "case {case}: expected the stage to report {expected}, got {actual:?}"
        );
    }
    for (value, expected) in [("0", 0.0), ("10000.5", 10_000.5), ("1e3", 1000.0)] {
        let case = format!("replace /*/*/throttling/rateLimit with {value:?}");
        let answer = update_stage(&gateway, &api, replace("/*/*/throttling/rateLimit", value)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get_stage(&gateway, &api, STAGE).await;
        let actual = read.body["methodSettings"]["*/*"]["throttlingRateLimit"].as_f64();
        assert_eq!(
            actual,
            Some(expected),
            "case {case}: expected the stage to report {expected}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_usage_plan_throttle_limit_the_plan_cannot_meter_earns_that_shapes_sentence() {
    let gateway = Gateway::new();
    let created = create_usage_plan(
        &gateway,
        "CreateUsagePlan(throttle sentence)",
        json!({
            "name": "throttle-sentence",
            "throttle": { "burstLimit": 10, "rateLimit": 5.0 },
        }),
    )
    .await;
    let plan = string_member(&created, "id", "CreateUsagePlan(throttle sentence)");

    // The same table of unacceptable values, on the plan's own throttle: the limits of a
    // usage plan are the same two members, so the sentence that shape publishes is the
    // answer for every value either refuses.
    let burst: Vec<(&str, &str)> = UNACCEPTABLE_LIMITS
        .iter()
        .chain(UNACCEPTABLE_INTEGER_LIMITS.iter())
        .copied()
        .collect();
    let limits: [(&str, &[(&str, &str)]); 2] = [
        ("/throttle/burstLimit", &burst),
        ("/throttle/rateLimit", &UNACCEPTABLE_LIMITS),
    ];
    for (path, rows) in &limits {
        for (reason, value) in *rows {
            let case = format!("replace {path} with {value:?} ({reason})");
            let answer = update_usage_plan(&gateway, &plan, replace(path, value)).await;
            assert_refusal(&answer, BAD_REQUEST, INVALID_USAGE_PLAN_THROTTLE, &case);
            // The refused patch wrote nothing: the plan still meters by the limits it had.
            let read = get_usage_plan(&gateway, &plan).await;
            assert_eq!(
                read.body["throttle"],
                json!({ "burstLimit": 10, "rateLimit": 5.0 }),
                "case {case}: expected the stored throttle untouched, got {:?}",
                read.body["throttle"]
            );
        }
    }

    // The limits the shape DOES take apply and read back.
    for (path, value, expected) in [
        ("/throttle/burstLimit", "0", json!(0)),
        ("/throttle/burstLimit", "2000", json!(2000)),
        ("/throttle/rateLimit", "0", json!(0.0)),
        ("/throttle/rateLimit", "12.5", json!(12.5)),
    ] {
        let case = format!("replace {path} with {value:?}");
        let answer = update_usage_plan(&gateway, &plan, replace(path, value)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let member = path.rsplit('/').next().unwrap_or(path);
        let read = get_usage_plan(&gateway, &plan).await;
        assert_eq!(
            read.body["throttle"][member], expected,
            "case {case}: expected the plan to report {expected}, got {:?}",
            read.body["throttle"]
        );
    }
}

#[tokio::test]
async fn a_tag_outside_the_contract_earns_the_one_contract_sentence() {
    let gateway = Gateway::new();
    let api = deployed_api(&gateway, "tag-contract-sentence").await;

    // Rows differ along HOW the tag leaves the contract: a key that names nothing, that key
    // beside an empty value, a key one character past its bound, a key far past it, a value
    // one character past its bound, and both at once — the sentence states the whole
    // contract, so it is the same for every one of them.
    let past_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let long_key = "k".repeat(MAX_TAG_KEY_CHARS * 4);
    let past_value = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    let rows: Vec<(&str, &str, &str)> = vec![
        ("an empty key", "", "v"),
        ("an empty key and an empty value", "", ""),
        ("a key one character past its bound", past_key.as_str(), "v"),
        ("a key far past its bound", long_key.as_str(), "v"),
        (
            "a value one character past its bound",
            "k",
            past_value.as_str(),
        ),
        ("both past their bounds", past_key.as_str(), past_value.as_str()),
    ];

    // …and along the path the tag rode in on, because the contract is the SERVICE's rather
    // than one operation's: `TagResource` of the API, `TagResource` of its stage, and the
    // `tags` a create carries.
    let expected = invalid_tag();
    let api_arn = apigateway_arn(&format!("/restapis/{api}"));
    let stage_arn = apigateway_arn(&format!("/restapis/{api}/stages/{STAGE}"));
    for (reason, key, value) in &rows {
        for arn in [&api_arn, &stage_arn] {
            let answer = tag_resource(&gateway, arn, key, value).await;
            assert_refusal(
                &answer,
                BAD_REQUEST,
                &expected,
                &format!("TagResource({arn}) / {reason}"),
            );
        }
        let on_create = gateway
            .call(
                "POST",
                "/restapis",
                Some(json!({ "name": "tagged-on-create", "tags": { (*key): value } })),
            )
            .await;
        assert_refusal(
            &on_create,
            BAD_REQUEST,
            &expected,
            &format!("CreateRestApi(tags) / {reason}"),
        );
    }

    // Where the contract STOPS: a key the contract admits is stored with the value it was
    // given — an EMPTY value included, which is a value the contract bounds rather than one
    // it refuses — and a key AT its bound is stored too, so the refusals above are the
    // contract's rules and not a blanket refusal.
    let at_key = "k".repeat(MAX_TAG_KEY_CHARS);
    let at_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    for (reason, key, value) in [
        ("a key with an empty value", "owner", ""),
        ("a key and a value at their bounds", at_key.as_str(), at_value.as_str()),
    ] {
        let case = format!("TagResource({api_arn}) / {reason}");
        let accepted = tag_resource(&gateway, &api_arn, key, value).await;
        assert_eq!(
            accepted.status, 204,
            "case {case}: expected 204, got {} with {:?}",
            accepted.status, accepted.body
        );
        let tags = get_tags(&gateway, &api_arn).await;
        assert_eq!(
            tags.body["tags"][key],
            json!(value),
            "case {case}: expected GetTags to read the stored tag back, got {:?}",
            tags.body
        );
    }
}
