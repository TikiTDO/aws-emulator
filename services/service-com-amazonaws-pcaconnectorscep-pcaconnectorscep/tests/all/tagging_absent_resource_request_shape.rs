// SPDX-License-Identifier: CC0-1.0

//! The absent-resource refusal of `/tags/{ResourceArn}` is a function of the ARN ALONE.
//!
//! The rule under test: when the `ResourceArn` of a tagging call resolves to no resource of
//! this service, the answer is the resource-tagging layer's own refusal — HTTP 400, the
//! `NotFoundException` error code, and a body whose SINGLE member is the lower-case `message`
//! carrying one fixed sentence — and nothing else about the request changes it. The same
//! refusal therefore comes back whichever of the three operations sharing the route is
//! called, whatever `TagKeys` an `UntagResource` names or `Tags` a `TagResource` carries, and
//! however often the call is repeated; because the sentence is fixed, the refusal reports no
//! value the caller sent, and because it is a refusal, it leaves every resource that DOES
//! exist carrying exactly what it carried.
//!
//! This is the axis `missing_resource_status_class` and `untag_resource` do not cover: those
//! vary the ARN and hold the rest of the request fixed, so a handler that decided the error
//! class from the request's own members (the number of keys it names, whether a key is one
//! some resource carries) would pass them. Here the ARN is held absent and the rest of the
//! request is what varies.
//!
//! `TagKeys` is a required `list<String>` bound to the `tagKeys` query parameter with no
//! modelled `@length` and no `@pattern` on the list or its member, and `Tags` is a required
//! map with none either (smithy_query operation=UntagResource,TagResource), so the key shapes
//! below are bounded only by what a tag key is: 1 to
//! [`scep::state::MAX_TAG_KEY_LENGTH`] characters of letters, numbers, spaces and
//! `_ . : / = + - @`, letters and numbers of ANY script, outside the `aws:` namespace — and a
//! resource carries at most [`scep::state::MAX_TAGS_PER_RESOURCE`] of them.
//!
//! Every case drives the service the way a client does: one HTTP request per call through the
//! same `MultiServiceRouter` the emulator boots.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// A Region that is not the emulator default, so nothing here can pass by defaulting.
const REGION: &str = "eu-central-1";

/// The account id every generated emulator mints ARNs under.
const ACCOUNT: &str = "000000000000";

/// A private-CA ARN of exactly the form `CertificateAuthorityArn`'s own `@pattern` requires,
/// built from this suite's Region and account so the three cannot drift apart.
fn ca_arn() -> String {
    format!(
        "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
         33333333-4444-5555-6666-777777777777"
    )
}

/// The tag-key bounds, named from the service's own constants so a row sitting on one cannot
/// drift off it.
const MAX_TAG_KEY_LENGTH: usize = scep::state::MAX_TAG_KEY_LENGTH;
const MAX_TAGS_PER_RESOURCE: usize = scep::state::MAX_TAGS_PER_RESOURCE;

/// The tags the live resources of a case carry, so a row naming one of these keys names a key
/// that really is a tag key of this service — just not of the absent ARN it is sent to.
fn live_tags() -> Value {
    json!({ "owner": "platform", "tier": "gold" })
}

/// Which operation of the shared `/tags/{ResourceArn}` route a call drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tagging {
    List,
    Tag,
    Untag,
}

/// One call on the tagging route, carrying `keys` as `TagKeys` (`UntagResource`) or as the
/// keys of `Tags` (`TagResource`); `ListTagsForResource` takes neither and ignores them.
async fn tagging_call(
    router: &MultiServiceRouter,
    operation: Tagging,
    arn: &str,
    keys: &[String],
) -> Answer {
    let encoded = label(arn);
    match operation {
        Tagging::List => {
            harness::call(router, REGION, "GET", &format!("/tags/{encoded}"), None).await
        }
        Tagging::Tag => {
            let tags: Value = Value::Object(
                keys.iter()
                    .map(|key| (key.clone(), Value::String("written".to_string())))
                    .collect(),
            );
            harness::call(
                router,
                REGION,
                "POST",
                &format!("/tags/{encoded}"),
                Some(json!({ "Tags": tags })),
            )
            .await
        }
        Tagging::Untag => {
            // `TagKeys` is bound to the `tagKeys` query parameter, so each key is one
            // repetition of it, percent-encoded the way an SDK encodes a query value.
            let query: Vec<String> = keys
                .iter()
                .map(|key| format!("tagKeys={}", label(key)))
                .collect();
            harness::call(
                router,
                REGION,
                "DELETE",
                &format!("/tags/{encoded}?{}", query.join("&")),
                None,
            )
            .await
        }
    }
}

/// Create a connector that is `ACTIVE` and carries [`live_tags`], and answer its ARN.
async fn live_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
) -> String {
    let created = harness::call(
        router,
        REGION,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": ca_arn(), "Tags": live_tags() })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    harness::release_pending(engine, &arn);
    arn
}

/// A connector ARN of the form `ConnectorArn`'s own pattern requires, under which nothing was
/// ever created.
fn absent_connector() -> String {
    format!(
        "arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/\
         12345678-90ab-cdef-1234-567890abcdef"
    )
}

/// A challenge ARN under a connector that was never created either.
fn absent_challenge() -> String {
    format!(
        "{}/challenge/abcdefab-cdef-abcd-efab-cdefabcdefab",
        absent_connector()
    )
}

/// The `TagKeys` / `Tags`-key shapes a row sends, each a set of strings that IS a tag key of
/// this service, differing along count, repetition, order, script, alphabet and length.
fn key_shapes() -> Vec<(&'static str, Vec<String>)> {
    let mut shapes: Vec<(&'static str, Vec<String>)> = vec![
        ("one key the live resources do carry", vec!["owner".to_string()]),
        (
            "one key nothing carries",
            vec!["never-written".to_string()],
        ),
        (
            "the same key named twice",
            vec!["owner".to_string(), "owner".to_string()],
        ),
        (
            "several keys in descending order, carried and not",
            vec![
                "tier".to_string(),
                "owner".to_string(),
                "never-written".to_string(),
            ],
        ),
        (
            // The whole tag alphabet this service states: letters, numbers, spaces and
            // `_ . : / = + - @`.
            "a key using every character of the tag alphabet",
            vec!["a_b.c:d/e=f+g-h@i 9".to_string()],
        ),
        (
            // "Letters" and "numbers" are not restricted to ASCII, so a key of another
            // script is a tag key too.
            "keys of non-Latin scripts",
            vec!["κλειδί".to_string(), "标签键".to_string()],
        ),
        (
            // The published key-length bound, and one character short of it.
            "a key of the maximum key length",
            vec!["k".repeat(MAX_TAG_KEY_LENGTH)],
        ),
        (
            "a key one character short of the maximum",
            vec!["k".repeat(MAX_TAG_KEY_LENGTH - 1)],
        ),
    ];
    // As many keys as a resource may carry tags: the largest set that could name every tag of
    // a resource at the per-resource ceiling.
    shapes.push((
        "as many keys as a resource may carry tags",
        (0..MAX_TAGS_PER_RESOURCE)
            .map(|index| format!("key-{index}"))
            .collect(),
    ));
    shapes
}

/// Assert `answer` is the tagging layer's absent-resource refusal AND that it reports nothing
/// of the request: exactly one body member, and no sign of the ARN or of any key sent.
fn assert_refusal_reports_nothing(answer: &Answer, case: &str, arn: &str, keys: &[String]) {
    harness::assert_tagging_not_found(answer, case);
    let members = answer
        .body
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object body, got {:?}", answer.body));
    // The shape has ONE member. Beside it a restJson1 body may carry the protocol's own
    // `__type` discriminator, which is the wire error code and not a member of the shape.
    let shape_members: Vec<&String> = members.keys().filter(|name| *name != "__type").collect();
    assert_eq!(
        shape_members,
        vec!["message"],
        "case {case:?}: the refusal carries the one `message` member and nothing else, got {:?}",
        answer.body
    );
    let raw = String::from_utf8_lossy(&answer.raw).to_string();
    assert!(
        !raw.contains(arn),
        "case {case:?}: the fixed sentence interpolates nothing, yet the body names {arn:?}: \
         {raw:?}"
    );
    for key in keys {
        assert!(
            !raw.contains(key.as_str()),
            "case {case:?}: the fixed sentence interpolates nothing, yet the body names the \
             sent key {key:?}: {raw:?}"
        );
    }
}

/// Whatever `TagKeys` an `UntagResource` names — or `Tags` a `TagResource` writes — an ARN
/// this service holds no record under is the tagging layer's 400 `NotFoundException`, and the
/// live resources keep what they carried.
#[tokio::test]
async fn an_absent_resource_is_refused_whatever_the_request_carries() {
    for (absent_kind, absent) in [
        ("a connector ARN nothing was created under", absent_connector()),
        ("a challenge ARN nothing was created under", absent_challenge()),
    ] {
        let (router, engine) = new_router();
        let live = live_connector(&router, &engine).await;

        // `ListTagsForResource` carries neither `TagKeys` nor `Tags`, so the key shapes are an
        // axis it has no value on: it is driven once per absent ARN, and the two operations
        // that DO carry keys are driven on every shape.
        let case = format!("{absent_kind} on {:?}", Tagging::List);
        let answer = tagging_call(&router, Tagging::List, &absent, &[]).await;
        assert_refusal_reports_nothing(&answer, &case, &absent, &[]);

        for (shape, keys) in key_shapes() {
            for operation in [Tagging::Untag, Tagging::Tag] {
                let case = format!("{absent_kind} with {shape} on {operation:?}");
                let answer = tagging_call(&router, operation, &absent, &keys).await;
                assert_refusal_reports_nothing(&answer, &case, &absent, &keys);
            }

            // Neither refusal changed anything: the live connector still reports exactly the
            // tags it was created with, through the very same route.
            let case = format!("{absent_kind} with {shape}");
            let listed = tagging_call(&router, Tagging::List, &live, &[]).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: the live connector must still be served, got {} / {:?}",
                listed.status, listed.body
            );
            assert_eq!(
                &Value::Object(listed.tags().clone()),
                &live_tags(),
                "case {case:?}: the refused calls must leave the live connector's tags at \
                 {:?}, got {:?}",
                live_tags(),
                listed.tags()
            );
        }
    }
}

/// The refusal is the same answer every time: repeating any of the three calls on the absent
/// ARN answers byte for byte what the first did, so nothing about it depends on what an
/// earlier refused call left behind.
#[tokio::test]
async fn repeating_the_refused_call_answers_the_same_bytes() {
    let (router, engine) = new_router();
    let _live = live_connector(&router, &engine).await;
    let keys = vec!["owner".to_string(), "never-written".to_string()];

    for (absent_kind, absent) in [
        ("a connector ARN nothing was created under", absent_connector()),
        ("a challenge ARN nothing was created under", absent_challenge()),
    ] {
        let mut first: Option<(u16, String, Vec<u8>)> = None;
        // Three rounds over all three operations, so a later round also follows calls of the
        // other two.
        for round in 1..=3 {
            for operation in [Tagging::Untag, Tagging::Tag, Tagging::List] {
                let case = format!("{absent_kind} on {operation:?}, round {round}");
                let answer = tagging_call(&router, operation, &absent, &keys).await;
                assert_refusal_reports_nothing(&answer, &case, &absent, &keys);
                let seen = (
                    answer.status,
                    answer.error_type.clone(),
                    answer.raw.clone(),
                );
                match &first {
                    None => first = Some(seen),
                    Some(expected) => assert_eq!(
                        &seen,
                        expected,
                        "case {case:?}: expected the same refusal as the first call, {:?} / {:?} \
                         with body {:?}, got {} / {:?} with body {:?}",
                        expected.0,
                        expected.1,
                        String::from_utf8_lossy(&expected.2),
                        seen.0,
                        seen.1,
                        String::from_utf8_lossy(&seen.2),
                    ),
                }
            }
        }
    }
}
