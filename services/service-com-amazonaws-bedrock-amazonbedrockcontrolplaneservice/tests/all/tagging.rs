// SPDX-License-Identifier: CC0-1.0

//! Tagging end-to-end: `TagResource` associates tags with a resource and
//! `ListTagsForResource` reports them back, over the same `MultiServiceRouter` the
//! emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **`TagResource` associates the request's tags with the resource its `resourceARN`
//!    names, keyed by `key`** — a key the resource already carries takes the new value and
//!    a key it does not carry is added — **so `ListTagsForResource` reports exactly the
//!    one value per key last associated**, for every resource family this service holds
//!    and whichever of the two ARN spellings a family admits addressed it. Tags a CREATE
//!    carried are that same list, and two resources never share one.
//! 2. **A `resourceARN` naming no resource this account holds is refused 404
//!    `ResourceNotFoundException`** — by `TagResource` and `ListTagsForResource` alike —
//!    whether it names a family this service holds records of or one it does not.
//! 3. **A resource carries at most the 50 tags `TooManyTagsException` documents**, counting
//!    the tags it already holds together with the request's, and a request past that bound
//!    is refused in full so the resource's tags are left exactly as they were.
//! 4. **A tag the model's own `TagKey`/`TagValue` constraints admit round-trips verbatim,
//!    and one outside them is refused 400 `ValidationException` with nothing persisted.**
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="TagResource,ListTagsForResource")` /
//! `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// The most tags one resource of this service may carry, as `TooManyTagsException`
/// documents it ("50 tags per resource") and the handler enforces it.
pub(crate) const MAX_TAGS: usize =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_TAGS_PER_RESOURCE;

/// How long after a create a model import job's move to `Completed` is owed, as the
/// service states it — the imported model exists only once that move is due.
const IMPORT_SPAN: std::time::Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::IMPORT_JOB_COMPLETE_AFTER;

/// `TagKey` is `@length(1..128)` and `TagValue` `@length(0..256)`, both over the pattern
/// `^[a-zA-Z0-9\s._:/=+@-]*$` (`smithy_query(query="shape",shape="Tag,TagKey,TagValue")`).
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MAX: usize = 256;

/// Every punctuation character that pattern admits beside the alphanumerics, so a row
/// carrying them all is a row over the whole admitted alphabet.
const TAG_PUNCTUATION: &str = " ._:/=+@-";

/// A base model this account's catalog offers WITH Provisioned Throughput
/// (`service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS`,
/// which `ListFoundationModels` reports).
const PROVISIONABLE_MODEL: &str = "amazon.titan-text-lite-v1";

/// A `RoleArn` the model admits (pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/bedrock-tagging";

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// `TagResource` is bound to `POST /tagResource`
/// (`smithy_query(query="operation",operation="TagResource")`).
pub(crate) async fn tag(emu: &Emulator, resource_arn: &str, tags: Value) -> Reply {
    emu.call(
        "POST",
        "/tagResource",
        Some(json!({ "resourceARN": resource_arn, "tags": tags })),
    )
    .await
}

/// `ListTagsForResource` is bound to `POST /listTagsForResource`.
pub(crate) async fn list_tags(emu: &Emulator, resource_arn: &str) -> Reply {
    emu.call(
        "POST",
        "/listTagsForResource",
        Some(json!({ "resourceARN": resource_arn })),
    )
    .await
}

/// A `TagList` out of `(key, value)` pairs.
pub(crate) fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect(),
    )
}

/// `n` distinct tags, each key and value inside `TagKey`/`TagValue`'s own pattern.
fn many_tags(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "key": format!("key-{index}"), "value": format!("value-{index}") }))
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------
// Response helpers.
// ---------------------------------------------------------------------------------------

/// The tags a listing reported, in the order it reported them, as `(key, value)` pairs.
pub(crate) fn listed(case: &str, reply: &Reply) -> Vec<(String, String)> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let tags = reply.get("tags");
    let rows = tags.as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: tags must be a present array — a resource with no tags carries \
             an EMPTY one — got {:?}",
            reply.body
        )
    });
    rows.iter()
        .map(|row| {
            let key = row
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no key: {row:?}"))
                .to_string();
            let value = row
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no value: {row:?}"))
                .to_string();
            (key, value)
        })
        .collect()
}

/// `pairs` as the owned pairs a listing is compared against.
fn expected(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------------------
// Resources to tag: one of every family this service holds a record of.
// ---------------------------------------------------------------------------------------

/// Create a guardrail and hand back its `guardrailArn`.
pub(crate) async fn given_guardrail(emu: &Emulator, name: &str) -> String {
    let reply = emu
        .create(json!({
            "name": name,
            "blockedInputMessaging": "blocked in",
            "blockedOutputsMessaging": "blocked out",
        }))
        .await;
    assert_eq!(
        reply.status, 202,
        "case {name}: CreateGuardrail must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("guardrailArn")
}

/// Buy a Provisioned Throughput — with a commitment, the quota this account is granted
/// units for — and hand back its `provisionedModelArn`.
async fn given_provisioned_model(emu: &Emulator, name: &str, tags: Option<Value>) -> String {
    let mut body = json!({
        "provisionedModelName": name,
        "modelId": PROVISIONABLE_MODEL,
        "modelUnits": 1,
        "commitmentDuration": "OneMonth",
    });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let reply = emu
        .call("POST", "/provisioned-model-throughput", Some(body))
        .await;
    assert_eq!(
        reply.status, 201,
        "case {name}: CreateProvisionedModelThroughput must answer 201, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("provisionedModelArn")
}

/// Start a model import job and hand back its `jobArn`. The job and the model it imports
/// are two taggable resources, so each one's tags are passed separately.
async fn given_import_job(
    emu: &Emulator,
    job_name: &str,
    model_name: &str,
    job_tags: Option<Value>,
    model_tags: Option<Value>,
) -> String {
    let mut body = json!({
        "jobName": job_name,
        "importedModelName": model_name,
        "roleArn": ROLE_ARN,
        "modelDataSource": { "s3DataSource": { "s3Uri": "s3://imported-models/weights" } },
    });
    if let Some(tags) = job_tags {
        body["jobTags"] = tags;
    }
    if let Some(tags) = model_tags {
        body["importedModelTags"] = tags;
    }
    let reply = emu.call("POST", "/model-import-jobs", Some(body)).await;
    assert_eq!(
        reply.status, 201,
        "case {job_name}: CreateModelImportJob must answer 201, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("jobArn")
}

/// The `importedModelArn` of a completed import job, once the clock owes the completion.
async fn given_imported_model(emu: &Emulator, job_name: &str, model_name: &str) -> String {
    let job_arn = given_import_job(emu, job_name, model_name, None, None).await;
    tokio::time::sleep(IMPORT_SPAN + std::time::Duration::from_millis(150)).await;
    let read = emu
        .call(
            "GET",
            &format!("/model-import-jobs/{}", label(&job_arn)),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case {job_name}: GetModelImportJob must answer 200, got {} {:?}",
        read.status, read.body
    );
    read.text("importedModelArn")
}

/// Create an evaluation job and hand back its `jobArn`.
async fn given_evaluation_job(emu: &Emulator, name: &str) -> String {
    let body = json!({
        "jobName": name,
        "roleArn": ROLE_ARN,
        "outputDataConfig": { "s3Uri": "s3://evaluation-results/tagging/" },
        "evaluationConfig": {
            "automated": {
                "datasetMetricConfigs": [{
                    "taskType": "Summarization",
                    "dataset": { "name": "Builtin.Gigaword" },
                    "metricNames": ["Builtin.Accuracy"],
                }],
            },
        },
        "inferenceConfig": {
            "models": [{
                "bedrockModel": {
                    "modelIdentifier": "anthropic.claude-v2",
                    "inferenceParams": "{\"temperature\":0.2}",
                },
            }],
        },
    });
    let reply = emu.call("POST", "/evaluation-jobs", Some(body)).await;
    assert_eq!(
        reply.status, 202,
        "case {name}: CreateEvaluationJob must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("jobArn")
}

// ---------------------------------------------------------------------------------------
// 1. Tags round-trip on every taggable resource family.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn tags_round_trip_on_every_taggable_resource_family() {
    let emu = emulator();

    // The rows differ along what the rule generalises over: the resource FAMILY the ARN
    // names, and so the record the tags are held on. Every family this service holds a
    // record of and whose ARN `TaggableResourcesArn` admits is a row — a guardrail, a
    // Provisioned Throughput, a model import job, the model that job imports, and an
    // evaluation job.
    let guardrail = given_guardrail(&emu, "tag-round-trip").await;
    let provisioned = given_provisioned_model(&emu, "tag-round-trip", None).await;
    let import_job = given_import_job(&emu, "tag-round-trip", "TagRoundTrip", None, None).await;
    let imported_model = given_imported_model(&emu, "tag-round-trip-2", "TagRoundTripTwo").await;
    let evaluation_job = given_evaluation_job(&emu, "tag-round-trip").await;
    let rows: Vec<(&str, &str)> = vec![
        ("a guardrail", &guardrail),
        ("a Provisioned Throughput", &provisioned),
        ("a model import job", &import_job),
        ("the model an import job imports", &imported_model),
        ("an evaluation job", &evaluation_job),
    ];

    // One tag set spanning `TagKey`/`TagValue`: a plain pair, an EMPTY value (`TagValue`
    // is `@length(0..256)`), the whole punctuation alphabet the shared pattern admits in
    // both a key and a value, and each published length bound.
    let long_key = "k".repeat(TAG_KEY_MAX);
    let long_value = "v".repeat(TAG_VALUE_MAX);
    let punctuated_key = format!("key{TAG_PUNCTUATION}0");
    let punctuated_value = format!("value{TAG_PUNCTUATION}9");
    let associated: Vec<(&str, &str)> = vec![
        ("Environment", "production"),
        ("emptyValue", ""),
        (&punctuated_key, &punctuated_value),
        (&long_key, &long_value),
    ];

    for (case, arn) in rows {
        let before = list_tags(&emu, arn).await;
        assert_eq!(
            listed(case, &before),
            Vec::<(String, String)>::new(),
            "case {case:?}: a resource nothing has tagged carries no tags, got {:?}",
            before.body
        );

        let tagged = tag(&emu, arn, tag_list(&associated)).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: TagResource must answer 200, got {} {:?}",
            tagged.status, tagged.body
        );

        let read = list_tags(&emu, arn).await;
        assert_eq!(
            listed(case, &read),
            expected(&associated),
            "case {case:?}: the listing must report exactly the tags associated, got {:?}",
            read.body
        );

        // Repeating the same association changes nothing: the merge is a set of
        // assignments keyed by `key`.
        let again = tag(&emu, arn, tag_list(&associated)).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a repeated TagResource must answer 200, got {} {:?}",
            again.status, again.body
        );
        let reread = list_tags(&emu, arn).await;
        assert_eq!(
            listed(case, &reread),
            expected(&associated),
            "case {case:?}: repeating the same call must leave the same tags, got {:?}",
            reread.body
        );

        // A key the resource already carries takes the new value in place; a key it does
        // not carry is added after the keys it has.
        let changed = tag(
            &emu,
            arn,
            tag_list(&[("Environment", "staging"), ("Owner", "platform")]),
        )
        .await;
        assert_eq!(
            changed.status, 200,
            "case {case:?}: TagResource over a held key must answer 200, got {} {:?}",
            changed.status, changed.body
        );
        let mut want = expected(&associated);
        want[0].1 = "staging".to_string();
        want.push(("Owner".to_string(), "platform".to_string()));
        let merged = list_tags(&emu, arn).await;
        assert_eq!(
            listed(case, &merged),
            want,
            "case {case:?}: a held key must take the new value and a new key be added, got \
             {:?}",
            merged.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 1b. The tags a create carried are the same list tagging extends, and no two resources
//     share one list.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn tagging_extends_the_tags_a_create_carried_and_touches_only_that_resource() {
    let emu = emulator();

    // A Provisioned Throughput and a model import job both carry tags on the create
    // request, and the job carries TWO sets — its own and the imported model's — so these
    // rows cover a create-carried list being extended, and the two lists of one request
    // staying apart.
    let provisioned = given_provisioned_model(
        &emu,
        "created-with-tags",
        Some(tag_list(&[("CostCenter", "4711")])),
    )
    .await;
    let job = given_import_job(
        &emu,
        "created-with-tags",
        "CreatedWithTags",
        Some(tag_list(&[("Side", "job")])),
        Some(tag_list(&[("Side", "model")])),
    )
    .await;
    tokio::time::sleep(IMPORT_SPAN + std::time::Duration::from_millis(150)).await;
    let model = {
        let read = emu
            .call("GET", &format!("/model-import-jobs/{}", label(&job)), None)
            .await;
        read.text("importedModelArn")
    };

    for (case, arn, created) in [
        (
            "a Provisioned Throughput created with tags",
            &provisioned,
            ("CostCenter", "4711"),
        ),
        ("a job created with jobTags", &job, ("Side", "job")),
        (
            "the imported model created with importedModelTags",
            &model,
            ("Side", "model"),
        ),
    ] {
        let read = list_tags(&emu, arn).await;
        assert_eq!(
            listed(case, &read),
            expected(&[created]),
            "case {case:?}: the listing must report the tags the create carried, got {:?}",
            read.body
        );
    }

    // Tagging the job leaves the imported model's list, and every other resource's,
    // exactly as it was: the two are separate resources.
    let tagged = tag(&emu, &job, tag_list(&[("Added", "later")])).await;
    assert_eq!(
        tagged.status, 200,
        "case tag the job: TagResource must answer 200, got {} {:?}",
        tagged.status, tagged.body
    );
    let job_tags = list_tags(&emu, &job).await;
    assert_eq!(
        listed("tag the job", &job_tags),
        expected(&[("Side", "job"), ("Added", "later")]),
        "case tag the job: the job's own list must carry both tags, got {:?}",
        job_tags.body
    );
    let model_tags = list_tags(&emu, &model).await;
    assert_eq!(
        listed("tag the job", &model_tags),
        expected(&[("Side", "model")]),
        "case tag the job: the imported model's list must be untouched, got {:?}",
        model_tags.body
    );
    let other_tags = list_tags(&emu, &provisioned).await;
    assert_eq!(
        listed("tag the job", &other_tags),
        expected(&[("CostCenter", "4711")]),
        "case tag the job: another resource's list must be untouched, got {:?}",
        other_tags.body
    );
}

// ---------------------------------------------------------------------------------------
// 2. An ARN naming no resource of this account is refused, by both operations alike.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_naming_no_resource_is_refused_by_both_tagging_operations() {
    let emu = emulator();

    // One live resource of each kind of store this service keeps — the guardrail store is
    // keyed by `guardrailId` alone, as every guardrail operation resolves a
    // `guardrailIdentifier`, while an evaluation job is keyed by the ARN that carries its
    // region — so the rows that name nothing are refused for naming nothing rather than
    // for an empty store.
    let live = given_guardrail(&emu, "the-live-one").await;
    let live_in_region = given_evaluation_job(&emu, "the-live-one").await;

    // The rows differ along what the rule generalises over: the family the ARN names —
    // one this service holds records of, one it holds none of, and one whose resource
    // exists in another region — and the bare resource NAME `TaggableResourcesArn` admits
    // beside an ARN.
    let missing = "zzzzzzzzzzzz";
    let rows: Vec<(&str, String)> = vec![
        (
            "a guardrail that was never created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/{missing}"),
        ),
        (
            "a Provisioned Throughput that was never bought",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:provisioned-model/{missing}"),
        ),
        (
            "an import job that was never started",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:model-import-job/{missing}"),
        ),
        (
            "an imported model no import created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:imported-model/{missing}"),
        ),
        (
            "an imported model under its custom-model spelling",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/imported/{missing}"),
        ),
        (
            "an evaluation job that was never created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:evaluation-job/{missing}"),
        ),
        (
            "a family this service holds no record of",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:inference-profile/not-a-profile"),
        ),
        (
            "an automated reasoning policy nothing created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/Missing1"),
        ),
        (
            "a live evaluation job addressed in another region",
            live_in_region.replace(REGION, "eu-central-1"),
        ),
        (
            "a bare resource name nothing holds",
            "unknown-resource-name-0001".to_string(),
        ),
    ];

    for (case, arn) in rows {
        let tagged = tag(&emu, &arn, tag_list(&[("Environment", "production")])).await;
        assert_eq!(
            tagged.status, 404,
            "case {case:?}: TagResource must answer 404 for {arn}, got {} {:?}",
            tagged.status, tagged.body
        );
        assert_eq!(
            tagged.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: TagResource must answer ResourceNotFoundException, got {:?}",
            tagged.error
        );

        let listed_reply = list_tags(&emu, &arn).await;
        assert_eq!(
            listed_reply.status, 404,
            "case {case:?}: ListTagsForResource must answer 404 for {arn}, got {} {:?}",
            listed_reply.status, listed_reply.body
        );
        assert_eq!(
            listed_reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: ListTagsForResource must answer ResourceNotFoundException, got \
             {:?}",
            listed_reply.error
        );
    }

    // Neither live resource was tagged by any of those refusals.
    for (case, arn) in [
        ("the live guardrail", &live),
        ("the live evaluation job", &live_in_region),
    ] {
        let read = list_tags(&emu, arn).await;
        assert_eq!(
            listed(case, &read),
            Vec::<(String, String)>::new(),
            "case {case:?}: a refused call must tag nothing, got {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. The per-resource tag bound counts held and requested tags together, and a request
//    past it is refused in full.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_per_resource_tag_bound_refuses_the_whole_request() {
    let emu = emulator();

    // The rows differ along HOW the bound is reached: in one call, by a call that adds to
    // what the resource already holds, and on another family's record.
    let in_one_call = given_guardrail(&emu, "bound-in-one-call").await;
    let in_two_calls = given_guardrail(&emu, "bound-in-two-calls").await;
    let on_another_family = given_provisioned_model(&emu, "bound-on-a-throughput", None).await;

    for (case, arn, first, second) in [
        ("the bound in one call", &in_one_call, MAX_TAGS, 0),
        ("the bound over two calls", &in_two_calls, MAX_TAGS - 10, 10),
        ("the bound on a Provisioned Throughput", &on_another_family, 1, MAX_TAGS - 1),
    ] {
        let up_to = tag(&emu, arn, many_tags(first)).await;
        assert_eq!(
            up_to.status, 200,
            "case {case:?}: {first} tags are inside the bound, got {} {:?}",
            up_to.status, up_to.body
        );
        if second > 0 {
            let rest = Value::Array(
                (first..first + second)
                    .map(|index| {
                        json!({ "key": format!("key-{index}"), "value": format!("value-{index}") })
                    })
                    .collect(),
            );
            let to_bound = tag(&emu, arn, rest).await;
            assert_eq!(
                to_bound.status, 200,
                "case {case:?}: reaching exactly {MAX_TAGS} tags is inside the bound, got {} \
                 {:?}",
                to_bound.status, to_bound.body
            );
        }
        let at_bound = list_tags(&emu, arn).await;
        assert_eq!(
            listed(case, &at_bound).len(),
            MAX_TAGS,
            "case {case:?}: the resource must carry {MAX_TAGS} tags, got {:?}",
            at_bound.body
        );

        // One more KEY is past the bound: 400 `TooManyTagsException`, and nothing of the
        // request is applied.
        let past = tag(&emu, arn, tag_list(&[("one-too-many", "x")])).await;
        assert_eq!(
            past.status, 400,
            "case {case:?}: a tag past the bound must answer 400, got {} {:?}",
            past.status, past.body
        );
        assert_eq!(
            past.error.as_deref(),
            Some("TooManyTagsException"),
            "case {case:?}: a tag past the bound must answer TooManyTagsException, got {:?}",
            past.error
        );
        let after = list_tags(&emu, arn).await;
        assert_eq!(
            listed(case, &after),
            listed(case, &at_bound),
            "case {case:?}: a refused call must leave the tags it found, got {:?}",
            after.body
        );

        // A key the resource ALREADY carries does not grow it, so re-valuing one at the
        // bound is admitted.
        let revalue = tag(&emu, arn, tag_list(&[("key-0", "a new value")])).await;
        assert_eq!(
            revalue.status, 200,
            "case {case:?}: re-valuing a held key at the bound must answer 200, got {} {:?}",
            revalue.status, revalue.body
        );
        let revalued = listed(case, &list_tags(&emu, arn).await);
        assert_eq!(
            revalued.len(),
            MAX_TAGS,
            "case {case:?}: re-valuing a held key must not grow the resource, got {revalued:?}"
        );
        assert!(
            revalued.contains(&("key-0".to_string(), "a new value".to_string())),
            "case {case:?}: the held key must carry its new value, got {revalued:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. A tag the model's constraints admit round-trips verbatim; one outside them is
//    refused, and persists nothing.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_tag_outside_the_modelled_constraints_is_refused_and_persists_nothing() {
    let emu = emulator();
    let arn = given_guardrail(&emu, "constrained-tags").await;

    // `TagKey` is `@length(1..128)` and `TagValue` `@length(0..256)`, both over
    // `^[a-zA-Z0-9\s._:/=+@-]*$` — so the rows walk each published bound, one character
    // past each bound, and a value outside the pattern's alphabet (its characters are
    // ASCII, so no other script is in it).
    let key_at_bound = "k".repeat(TAG_KEY_MAX);
    let key_past_bound = "k".repeat(TAG_KEY_MAX + 1);
    let value_at_bound = "v".repeat(TAG_VALUE_MAX);
    let value_past_bound = "v".repeat(TAG_VALUE_MAX + 1);
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        ("a key at its length bound", &key_at_bound, "held", true),
        ("a key one past its length bound", &key_past_bound, "held", false),
        ("an empty key", "", "held", false),
        ("a value at its length bound", "atBound", &value_at_bound, true),
        (
            "a value one past its length bound",
            "pastBound",
            &value_past_bound,
            false,
        ),
        ("a value of only punctuation", "punctuation", " ._:/=+@-", true),
        ("a key outside the pattern", "key(parenthesised)", "held", false),
        ("a value outside the pattern", "outside", "no parentheses (here)", false),
    ];

    let mut admitted: Vec<(String, String)> = Vec::new();
    for (case, key, value, is_admitted) in rows {
        let reply = tag(&emu, &arn, tag_list(&[(key, value)])).await;
        if is_admitted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the model admits this tag, so TagResource must answer 200, got \
                 {} {:?}",
                reply.status, reply.body
            );
            admitted.push((key.to_string(), value.to_string()));
        } else {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: the model does not admit this tag, so it must be refused 400, \
                 got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ValidationException"),
                "case {case:?}: a tag outside the model's constraints is a ValidationException, \
                 got {:?}",
                reply.error
            );
        }
        let read = list_tags(&emu, &arn).await;
        assert_eq!(
            listed(case, &read),
            admitted,
            "case {case:?}: only the tags the model admits may be persisted, got {:?}",
            read.body
        );
    }
}
