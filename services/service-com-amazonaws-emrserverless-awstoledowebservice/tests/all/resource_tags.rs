// SPDX-License-Identifier: CC0-1.0

//! The resource-tagging surface of EMR Serverless: `TagResource`, `UntagResource` and
//! `ListTagsForResource`, end to end over the real router.
//!
//! The rule under test, one level above any single request: the tags of a resource are
//! a persisted set living on that resource's OWN record, keyed by the ARN the service
//! minted for it. `TagResource` adds or overwrites exactly the keys it names and keeps
//! the rest; `UntagResource` removes exactly the keys it names and keeps the rest;
//! `ListTagsForResource` reports the stored set — the same set the resource's own
//! `Get` reports — and changes nothing. An ARN that names no stored resource of this
//! service is refused with the modelled `ResourceNotFoundException` (HTTP 404) by all
//! three.
//!
//! Every assertion spans a SET of inputs: all three resource classes this emulator
//! stores (an application, a job run and an interactive session), tag sets differing in
//! count, in value and in the characters `TagKey`/`TagValue` admit, and the published
//! `TagMap` bound with one past it.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// `urlencode` percent-encodes one path segment or query value, so an ARN carrying `:`
// and `/` stays the single non-greedy `{resourceArn}` label of `/tags/{resourceArn}`
// and a `TagKey` carrying ` `, `+`, `/`, `:` or `=` reaches the handler as the caller
// wrote it. Shared with the lifecycle suite rather than written twice.
use super::application_lifecycle::{
    call, create_application, emulator, get_application, get_job_run, minimal_create,
    minimal_job_run, start_job_run, started_application, urlencode, wait_for_transition,
    Answer,
};
use super::session_lifecycle::{get_session, minimal_session, session_application, start_session};

/// The largest tag set a resource keeps.
///
/// Written out here rather than read from `crate::state`, so a wrong bound in `src/`
/// cannot make this test pass. Source: `TagMap` is modelled `@length(min: 0, max: 200)`
/// (`smithy_query(query="shape",shape="TagMap")`), so 200 is the published bound and
/// 201 is one past it.
const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The status a successful TAG MUTATION answers with: 204 No Content, because
/// `TagResourceResponse` and `UntagResourceResponse` declare no members
/// (`smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse")`) and
/// so leave the caller nothing to parse. Written out here rather than read from
/// `crate::state` for the same reason as the bound above; the suite that OWNS this rule,
/// with the body bytes and the framing a no-content answer carries, is
/// `tag_mutation_status.rs`.
const TAG_MUTATION_SUCCESS_STATUS: u16 = 204;

/// Every character `TagKey`'s pattern `^[A-Za-z0-9 /_.:=+@-]+$` admits beyond the
/// alphanumerics, in one key — and the same set is `TagValue`'s pattern
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`).
const PUNCTUATION: &str = "a Z9 /_.:=+@-";

/// The JSON object a list of pairs spells, which is how a `TagMap` travels on the wire.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/tags/{}", urlencode(resource_arn)),
        None,
    )
    .await
}

async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", urlencode(resource_arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[String],
) -> Answer {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", urlencode(key)))
        .collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", urlencode(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// An application brought to `STARTED`, with its minted ARN read back from the service.
async fn tagged_application(router: &MultiServiceRouter, token: &str) -> (String, String) {
    let id = started_application(router, token).await;
    let arn = get_application(router, &id)
        .await
        .at("/application/arn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.starts_with("arn:aws:emr-serverless:"),
        "case {token:?}: the application must report its minted ARN, got {arn:?}"
    );
    (id, arn)
}

/// A job run of `application_id`, with its minted ARN.
async fn tagged_job_run(
    router: &MultiServiceRouter,
    application_id: &str,
    token: &str,
) -> (String, String) {
    let submitted = start_job_run(router, application_id, minimal_job_run(token)).await;
    assert_eq!(
        submitted.status, 200,
        "case {token:?}: StartJobRun expected 200, got {} with {:?}",
        submitted.status, submitted.body
    );
    let id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let arn = submitted.at("/arn").as_str().unwrap_or_default().to_string();
    (id, arn)
}

/// An interactive session of `application_id`, with its minted ARN.
async fn tagged_session(
    router: &MultiServiceRouter,
    application_id: &str,
    token: &str,
) -> (String, String) {
    let started = start_session(router, application_id, minimal_session(token)).await;
    assert_eq!(
        started.status, 200,
        "case {token:?}: StartSession expected 200, got {} with {:?}",
        started.status, started.body
    );
    let id = started
        .at("/sessionId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let arn = started.at("/arn").as_str().unwrap_or_default().to_string();
    (id, arn)
}

/// The tag sets the round trip is asserted over: rows differing in count, in the
/// characters the modelled key and value admit, and in value length.
fn tag_set_cases() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        ("no pairs at all", Vec::new()),
        (
            "one pair",
            vec![("env".to_string(), "prod".to_string())],
        ),
        (
            "three pairs, written in an order the map does not keep",
            vec![
                ("team".to_string(), "data".to_string()),
                ("env".to_string(), "staging".to_string()),
                ("cost-center".to_string(), "42".to_string()),
            ],
        ),
        (
            "the punctuation TagKey and TagValue admit",
            vec![(PUNCTUATION.to_string(), PUNCTUATION.to_string())],
        ),
        (
            "an empty value, which TagValue's 0 minimum admits",
            vec![("retain".to_string(), String::new())],
        ),
        (
            "a key and a value at their modelled maximum length",
            vec![("k".repeat(128), "v".repeat(256))],
        ),
    ]
}

/// A tag write is visible to `ListTagsForResource` and to the resource's own read, for
/// every resource class this emulator stores, and removing the keys it wrote leaves the
/// resource with none.
#[tokio::test]
async fn tags_round_trip_through_the_stored_resource_for_every_resource_class() {
    let router = emulator();
    let (application_id, application_arn) = tagged_application(&router, "tok-tags").await;
    let (job_run_id, job_run_arn) = tagged_job_run(&router, &application_id, "job-tags").await;
    // A session needs an application that serves them, which is a release and an
    // interactive configuration of its own.
    let (session_application_id, _) = session_application(&router, "tok-tags-session").await;
    let (session_id, session_arn) =
        tagged_session(&router, &session_application_id, "ses-tags").await;

    for (class, resource_arn) in [
        ("application", application_arn.clone()),
        ("job run", job_run_arn.clone()),
        ("session", session_arn.clone()),
    ] {
        for (case, pairs) in tag_set_cases() {
            let wrote = tag_resource(&router, &resource_arn, tag_map(&pairs)).await;
            assert_eq!(
                wrote.status, TAG_MUTATION_SUCCESS_STATUS,
                "case {class}/{case:?}: TagResource expected {TAG_MUTATION_SUCCESS_STATUS}, \
                 got {} with {:?}",
                wrote.status, wrote.body
            );

            let listed = list_tags(&router, &resource_arn).await;
            assert_eq!(
                listed.status, 200,
                "case {class}/{case:?}: ListTagsForResource expected 200, got {} with {:?}",
                listed.status, listed.body
            );
            assert_eq!(
                listed.at("/tags"),
                tag_map(&pairs),
                "case {class}/{case:?}: expected the written set, got {:?}",
                listed.at("/tags")
            );

            // A read changes nothing a later caller sees.
            let again = list_tags(&router, &resource_arn).await;
            assert_eq!(
                again.at("/tags"),
                listed.at("/tags"),
                "case {class}/{case:?}: a second read must answer the same set, got {:?}",
                again.at("/tags")
            );

            // The tags live on the resource's own record, so the resource's own read
            // reports the same set.
            let own_read = match class {
                "application" => get_application(&router, &application_id)
                    .await
                    .at("/application/tags"),
                "job run" => get_job_run(&router, &application_id, &job_run_id)
                    .await
                    .at("/jobRun/tags"),
                _ => get_session(&router, &session_application_id, &session_id)
                    .await
                    .at("/session/tags"),
            };
            assert_eq!(
                own_read,
                tag_map(&pairs),
                "case {class}/{case:?}: the resource's own read must report the same \
                 tags, got {own_read:?}"
            );

            let keys: Vec<String> = pairs.iter().map(|(key, _)| key.clone()).collect();
            if keys.is_empty() {
                continue;
            }
            let removed = untag_resource(&router, &resource_arn, &keys).await;
            assert_eq!(
                removed.status, TAG_MUTATION_SUCCESS_STATUS,
                "case {class}/{case:?}: UntagResource expected {TAG_MUTATION_SUCCESS_STATUS}, \
                 got {} with {:?}",
                removed.status, removed.body
            );
            let emptied = list_tags(&router, &resource_arn).await;
            assert_eq!(
                emptied.at("/tags"),
                json!({}),
                "case {class}/{case:?}: removing every key leaves an empty set, got {:?}",
                emptied.at("/tags")
            );
        }
    }
}

/// One call of the tagging surface that CHANGES the stored set.
enum TagStep {
    /// `TagResource` with these pairs.
    Write(Vec<(String, String)>),
    /// `UntagResource` with these keys.
    Remove(Vec<String>),
}

impl TagStep {
    fn write(pairs: &[(&str, &str)]) -> Self {
        Self::Write(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        )
    }

    fn remove(keys: &[&str]) -> Self {
        Self::Remove(keys.iter().map(|key| (*key).to_string()).collect())
    }
}

/// A tag write touches exactly the keys it names: it overwrites those, keeps the
/// others, and a removal of a key the resource does not carry leaves it as it was.
#[tokio::test]
async fn a_tag_write_touches_exactly_the_keys_it_names() {
    let router = emulator();
    let (_, resource_arn) = tagged_application(&router, "tok-merge").await;

    // Each step: the call it makes, and the whole set the resource carries afterwards.
    let steps: Vec<(&str, TagStep, Value)> = vec![
        (
            "the first write",
            TagStep::write(&[("a", "1")]),
            json!({ "a": "1" }),
        ),
        (
            "a write overwriting one key and adding another",
            TagStep::write(&[("a", "2"), ("b", "3")]),
            json!({ "a": "2", "b": "3" }),
        ),
        (
            "a write of no pairs at all",
            TagStep::write(&[]),
            json!({ "a": "2", "b": "3" }),
        ),
        (
            "removing one of the two keys",
            TagStep::remove(&["a"]),
            json!({ "b": "3" }),
        ),
        (
            "removing that key again, and one never written",
            TagStep::remove(&["a", "never-written"]),
            json!({ "b": "3" }),
        ),
        (
            "removing the last key",
            TagStep::remove(&["b"]),
            json!({}),
        ),
    ];

    for (case, step, expected) in steps {
        let answer = match &step {
            TagStep::Write(pairs) => tag_resource(&router, &resource_arn, tag_map(pairs)).await,
            TagStep::Remove(keys) => untag_resource(&router, &resource_arn, keys).await,
        };
        assert_eq!(
            answer.status, TAG_MUTATION_SUCCESS_STATUS,
            "case {case:?}: expected {TAG_MUTATION_SUCCESS_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        let listed = list_tags(&router, &resource_arn).await;
        assert_eq!(
            listed.at("/tags"),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed.at("/tags")
        );
    }
}

/// An ARN that names no stored resource is refused by all three tag operations with the
/// modelled `ResourceNotFoundException`, and the refusal stores nothing.
#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_refused() {
    let router = emulator();
    let (application_id, application_arn) = tagged_application(&router, "tok-notfound").await;
    let (_, job_run_arn) = tagged_job_run(&router, &application_id, "job-notfound").await;
    tag_resource(&router, &application_arn, json!({ "env": "prod" })).await;

    // Every ARN below satisfies `ResourceArn`'s pattern and length, so each one reaches
    // the handler; none of them names a resource this emulator stores.
    let unknown_application = application_arn.replace(&application_id, "0000000000009999");
    let unknown_job_run = format!("{application_arn}/jobruns/0000000000009999");
    let other_account = application_arn.replace("000000000000", "123456789012");
    let other_region = application_arn.replace("eu-west-1", "us-east-1");
    let session = format!("{application_arn}/sessions/0000000000000001");
    let unknown_arns = [
        ("an application id that names none", unknown_application),
        ("a job run id that names none", unknown_job_run),
        ("another account's application", other_account),
        ("another region's application", other_region),
        ("a session id that names none", session),
    ];

    for (case, resource_arn) in unknown_arns {
        for (operation, answer) in [
            ("ListTagsForResource", list_tags(&router, &resource_arn).await),
            (
                "TagResource",
                tag_resource(&router, &resource_arn, json!({ "env": "prod" })).await,
            ),
            (
                "UntagResource",
                untag_resource(&router, &resource_arn, &["env".to_string()]).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: expected 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
        }
    }

    // The refused calls stored nothing of their own, and left the stored resources as
    // they were.
    assert_eq!(
        list_tags(&router, &application_arn).await.at("/tags"),
        json!({ "env": "prod" }),
        "case refused calls: the known application keeps the tags it had"
    );
    assert_eq!(
        list_tags(&router, &job_run_arn).await.at("/tags"),
        json!({}),
        "case refused calls: the known job run keeps the tags it had"
    );
}

/// `CreateApplication` and `StartJobRun` both take tags, and `ListTagsForResource`
/// reports whatever they stored rather than an empty set.
#[tokio::test]
async fn tags_given_at_create_time_are_what_list_tags_reports() {
    let router = emulator();

    let mut request = minimal_create("tok-create-tags");
    request["tags"] = json!({ "env": "prod", "owner": "data" });
    let created = create_application(&router, request).await;
    assert_eq!(
        created.status, 200,
        "case create with tags: expected 200, got {} with {:?}",
        created.status, created.body
    );
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let application_arn = created.at("/arn").as_str().unwrap_or_default().to_string();
    wait_for_transition();

    assert_eq!(
        list_tags(&router, &application_arn).await.at("/tags"),
        json!({ "env": "prod", "owner": "data" }),
        "case create with tags: ListTagsForResource reports the created set"
    );

    // An application created with NO tags carries an empty set, not an absent one.
    let bare = create_application(&router, minimal_create("tok-create-no-tags")).await;
    let bare_arn = bare.at("/arn").as_str().unwrap_or_default().to_string();
    assert_eq!(
        list_tags(&router, &bare_arn).await.at("/tags"),
        json!({}),
        "case create with no tags: ListTagsForResource reports an empty set"
    );

    let mut submit = minimal_job_run("job-create-tags");
    submit["tags"] = json!({ "run": "nightly" });
    let submitted = start_job_run(&router, &application_id, submit).await;
    assert_eq!(
        submitted.status, 200,
        "case submit with tags: expected 200, got {} with {:?}",
        submitted.status, submitted.body
    );
    let job_run_arn = submitted.at("/arn").as_str().unwrap_or_default().to_string();
    assert_eq!(
        list_tags(&router, &job_run_arn).await.at("/tags"),
        json!({ "run": "nightly" }),
        "case submit with tags: ListTagsForResource reports the submitted set"
    );
}

/// `TagMap`'s `@length(0..200)` bounds the set the RESOURCE carries, so a write that
/// would take it past 200 pairs is refused and stores nothing — while a write that only
/// overwrites keys it already carries is accepted at the bound.
#[tokio::test]
async fn a_write_past_the_modelled_tag_bound_is_refused() {
    let router = emulator();
    let (_, resource_arn) = tagged_application(&router, "tok-bound").await;

    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let at_bound = tag_resource(&router, &resource_arn, tag_map(&full)).await;
    assert_eq!(
        at_bound.status, TAG_MUTATION_SUCCESS_STATUS,
        "case at the bound: {MAX_TAGS_PER_RESOURCE} tags expected \
         {TAG_MUTATION_SUCCESS_STATUS}, got {} with {:?}",
        at_bound.status, at_bound.body
    );
    let listed = list_tags(&router, &resource_arn).await;
    assert_eq!(
        listed.at("/tags").as_object().map_or(0, Map::len),
        MAX_TAGS_PER_RESOURCE,
        "case at the bound: expected {MAX_TAGS_PER_RESOURCE} stored tags, got {:?}",
        listed.at("/tags")
    );

    // One key past the bound: refused, with nothing of the request stored.
    let past = tag_resource(&router, &resource_arn, json!({ "one-too-many": "x" })).await;
    assert_eq!(
        past.status, 400,
        "case one past the bound: expected 400, got {} with {:?}",
        past.status, past.body
    );
    assert_eq!(
        past.error_code(),
        "ValidationException",
        "case one past the bound: expected ValidationException, got {:?}",
        past.body
    );
    let after = list_tags(&router, &resource_arn).await;
    assert_eq!(
        after.at("/tags"),
        listed.at("/tags"),
        "case one past the bound: the refused write must store nothing"
    );

    // At the bound, a write that only overwrites keys the resource already carries does
    // not grow the set, so it is accepted.
    let overwrite = tag_resource(&router, &resource_arn, json!({ "key-0": "rewritten" })).await;
    assert_eq!(
        overwrite.status, TAG_MUTATION_SUCCESS_STATUS,
        "case overwrite at the bound: expected {TAG_MUTATION_SUCCESS_STATUS}, got {} with {:?}",
        overwrite.status, overwrite.body
    );
    let rewritten = list_tags(&router, &resource_arn).await;
    assert_eq!(
        rewritten.at("/tags/key-0"),
        json!("rewritten"),
        "case overwrite at the bound: expected the new value, got {:?}",
        rewritten.at("/tags/key-0")
    );
    assert_eq!(
        rewritten.at("/tags").as_object().map_or(0, Map::len),
        MAX_TAGS_PER_RESOURCE,
        "case overwrite at the bound: the set must not have grown, got {:?}",
        rewritten.at("/tags")
    );
}
