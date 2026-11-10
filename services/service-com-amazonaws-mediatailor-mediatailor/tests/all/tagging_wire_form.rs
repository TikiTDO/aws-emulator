// SPDX-License-Identifier: CC0-1.0

//! The WIRE FORM of this service's tagging surface, end to end over HTTP.
//!
//! The RULE under test is about the shape of a request rather than about any one operation:
//! a body member the model RENAMES on the wire (`@jsonName`) is read under either spelling —
//! the renamed key an AWS SDK writes, or the member's own Smithy member name — so the member
//! reaches the handler whichever one the caller sent, and only a member absent under BOTH
//! spellings is refused (with the one client-input error this service's model declares,
//! `BadRequestException`). `Tags` -> `tags` is the only rename this service's model declares,
//! on every operation that takes tags, as the `Tags` member of each of those operations'
//! input shapes states in this crate's `smithy-model/` document. The table below spans the
//! IMPLEMENTED operations that carry it, each in both spellings and with tag sets of several
//! sizes, and reads every write back through `ListTagsForResource`. The FRAMING half of the
//! wire form — what an answer that carries no content looks like — is the sibling suite's.
//!
//! Every assertion goes through the router: a request is built, dispatched to the mounted
//! service, and the status, the headers and the JSON body read back — so what is checked is
//! the emulator's wire behaviour, not a handler's return value.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite signs for, so the ARNs it addresses are the ones the request's
/// SigV4 credential scope names.
pub(crate) const REGION: &str = "us-east-1";

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// A SigV4 `Authorization` header scoped to `mediatailor` in [`REGION`]: it is what routes
/// the request to this service. The signature is never verified here (this router runs with
/// request authentication off).
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/us-east-1/mediatailor/aws4_request, SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The status a success that carries no content answers; the length it states for the body
/// it does not carry belongs to the framing suite that asserts it.
const NO_CONTENT_STATUS: u16 = 204;

/// The one client-input error this service's model declares (HTTP 400).
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// One dispatched request's answer, with the response headers kept: these suites assert on
/// the FRAMING of an answer as well as on its body.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) content_length: Option<String>,
    pub(crate) body_bytes: Vec<u8>,
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn get(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }
}

/// The emulator under test: this one service mounted on the real router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediatailor_mediatailor::EmulatedService::new(engine),
        );
        Self { router }
    }

    pub(crate) async fn request(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let payload = match body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.mediatailor.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .header("content-length", payload.len().to_string())
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let error_code = header("x-amzn-errortype").unwrap_or_default();
        let content_length = header("content-length");
        let body_bytes = response.into_body().to_vec();
        let body = if body_bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body_bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            content_length,
            body_bytes,
            body,
        }
    }

    pub(crate) async fn list_tags(&self, arn: &str) -> Reply {
        self.request("GET", &format!("/tags/{}", encode(arn)), None)
            .await
    }

    pub(crate) async fn untag(&self, arn: &str, keys: &[&str]) -> Reply {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", encode(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.request("DELETE", &format!("/tags/{}?{query}", encode(arn)), None)
            .await
    }
}

/// Percent-encode a value bound to a URI path segment or query parameter, the way an AWS SDK
/// does: everything outside the RFC 3986 unreserved set is escaped, so an ARN's `:` and `/`
/// stay inside one path segment.
pub(crate) fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// `count` tags, so a row can differ from the others in HOW MANY tags it writes rather than
/// only in their values.
fn tag_set(count: usize) -> Value {
    let mut tags = Map::new();
    for index in 0..count {
        tags.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(tags)
}

/// How a request spells the body member the model renames.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    /// The renamed key an AWS SDK writes, from the member's `@jsonName`.
    WireName,
    /// The member name the model declares.
    MemberName,
}

impl Spelling {
    fn key(self) -> &'static str {
        match self {
            Self::WireName => "tags",
            Self::MemberName => "Tags",
        }
    }
}

/// One operation that carries the renamed member, as a request this suite can send: how to
/// address it, the body it needs beside its tags, and the ARN the resource it writes holds.
struct TaggingWrite {
    operation: &'static str,
    method: &'static str,
    /// The request target, given the name the row creates the resource under.
    uri: fn(&str) -> String,
    /// The body beside the renamed member, given that name.
    body: fn(&str) -> Value,
    /// The ARN the written resource holds, given that name.
    arn: fn(&str) -> String,
}

/// The IMPLEMENTED operations whose input declares the renamed `Tags` member. The others the
/// model declares it on (`CreateLiveSource`, `CreatePrefetchSchedule`, `CreateProgram`,
/// `CreateSourceLocation`, `CreateVodSource`, `PutFunction`) are still 501 stubs, so they
/// have no stored resource to read a tag back from; the rule is applied to them by the same
/// one code path, which reads the rename off each operation's own input schema.
fn tagging_writes() -> Vec<TaggingWrite> {
    vec![
        TaggingWrite {
            operation: "CreateChannel",
            method: "POST",
            uri: |name| format!("/channel/{}", encode(name)),
            body: |_| {
                json!({
                    "PlaybackMode": "LOOP",
                    "Outputs": [{
                        "ManifestName": "index",
                        "SourceGroup": "default",
                        "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
                    }],
                })
            },
            arn: |name| format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:channel/{name}"),
        },
        TaggingWrite {
            operation: "PutPlaybackConfiguration",
            method: "PUT",
            uri: |_| "/playbackConfiguration".to_string(),
            body: |name| json!({ "Name": name }),
            arn: |name| {
                format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:playbackConfiguration/{name}")
            },
        },
    ]
}

/// A request body carrying `tags` under `spelling`, beside the members `base` holds.
fn with_tags(base: &Value, spelling: Spelling, tags: &Value) -> Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    object.insert(spelling.key().to_string(), tags.clone());
    Value::Object(object)
}

/// RULE 1, on the CREATE/PUT half: a renamed body member is read under either spelling, so
/// the tags a create carries are stored and read back whichever key the caller wrote them
/// under — for every operation that carries them and every size of tag set.
#[tokio::test]
async fn a_create_stores_its_tags_under_either_spelling() {
    for write in tagging_writes() {
        for spelling in [Spelling::WireName, Spelling::MemberName] {
            // The sizes span the class: no tag at all, one, and several.
            for count in [0_usize, 1, 3] {
                let case = format!(
                    "{} with {count} tag(s) under {:?}",
                    write.operation, spelling
                );
                let emulator = Emulator::new();
                let name = format!(
                    "probe-{}-{count}",
                    match spelling {
                        Spelling::WireName => "wire",
                        Spelling::MemberName => "member",
                    }
                );
                let tags = tag_set(count);
                let created = emulator
                    .request(
                        write.method,
                        &(write.uri)(&name),
                        Some(with_tags(&(write.body)(&name), spelling, &tags)),
                    )
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: expected the write to succeed, got {} {}",
                    created.status, created.error_code
                );
                assert_eq!(
                    created.get("/tags"),
                    &tags,
                    "case {case:?}: the answer reports the tags it stored, got {}",
                    created.get("/tags")
                );

                let listed = emulator.list_tags(&(write.arn)(&name)).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected the tags to be readable, got {} {}",
                    listed.status, listed.error_code
                );
                assert_eq!(
                    listed.get("/tags"),
                    &tags,
                    "case {case:?}: expected every tag the write carried, got {}",
                    listed.get("/tags")
                );
            }
        }
    }
}

/// RULE 1, on the TagResource half: the tagging write is served under either spelling, and
/// the tags it wrote are reported by the read that follows it — the whole write-then-read
/// round trip, for tag sets of several sizes.
#[tokio::test]
async fn tag_resource_merges_its_tags_under_either_spelling() {
    for spelling in [Spelling::WireName, Spelling::MemberName] {
        // One tag, several tags, and a set that rewrites a key the resource already holds.
        for (label, tags) in [
            ("one tag", json!({ "key-0": "value-0" })),
            ("several tags", tag_set(3)),
            ("a key the resource already holds", json!({ "Env": "rewritten" })),
        ] {
            let case = format!("{label} under {spelling:?}");
            let emulator = Emulator::new();
            let name = "tag-probe";
            let arn = format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:channel/{name}");
            let created = emulator
                .request(
                    "POST",
                    &format!("/channel/{}", encode(name)),
                    Some(json!({
                        "PlaybackMode": "LOOP",
                        "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                        "tags": { "Env": "prod" },
                    })),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the channel the tags are written onto, got {} {}",
                created.status, created.error_code
            );

            let tagged = emulator
                .request(
                    "POST",
                    &format!("/tags/{}", encode(&arn)),
                    Some(json!({ spelling.key(): tags })),
                )
                .await;
            assert_eq!(
                tagged.status, NO_CONTENT_STATUS,
                "case {case:?}: expected the modelled 204, got {} {} {}",
                tagged.status,
                tagged.error_code,
                String::from_utf8_lossy(&tagged.body_bytes)
            );

            let listed = emulator.list_tags(&arn).await;
            for (key, value) in tags.as_object().expect("an object of tags") {
                assert_eq!(
                    listed.get(&format!("/tags/{key}")),
                    value,
                    "case {case:?}: expected {key} reported as {value}, got {}",
                    listed.get("/tags")
                );
            }
            // A key the request did not mention is left alone, so the one the create wrote
            // survives unless this row rewrote it.
            let expected_env = tags
                .get("Env")
                .cloned()
                .unwrap_or_else(|| json!("prod"));
            assert_eq!(
                listed.get("/tags/Env"),
                &expected_env,
                "case {case:?}: a key the write did not mention is untouched, got {}",
                listed.get("/tags")
            );
        }
    }
}

/// RULE 1, where the two spellings MEET: a body that carries both keeps the value under the
/// wire name, which is the key the codec binds — so accepting the member name can never
/// change what a request already meant.
#[tokio::test]
async fn the_wire_spelling_wins_when_a_body_carries_both() {
    let emulator = Emulator::new();
    let name = "both-spellings";
    let arn = format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:channel/{name}");
    let created = emulator
        .request(
            "POST",
            &format!("/channel/{}", encode(name)),
            Some(json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the channel the tags are written onto, got {} {}",
        created.status, created.error_code
    );

    let tagged = emulator
        .request(
            "POST",
            &format!("/tags/{}", encode(&arn)),
            Some(json!({
                "tags": { "Source": "wire" },
                "Tags": { "Source": "member" },
            })),
        )
        .await;
    assert_eq!(
        tagged.status, NO_CONTENT_STATUS,
        "expected the modelled 204, got {} {}",
        tagged.status, tagged.error_code
    );
    let listed = emulator.list_tags(&arn).await;
    assert_eq!(
        listed.get("/tags/Source"),
        &json!("wire"),
        "the key the codec binds is the wire one, got {}",
        listed.get("/tags")
    );
}

/// RULE 1's other half: a required member absent under BOTH spellings is refused — with the
/// one client-input error this service's model declares, not with a code the model does not
/// carry.
#[tokio::test]
async fn a_member_absent_under_both_spellings_is_refused() {
    let emulator = Emulator::new();
    let name = "refusal-probe";
    let arn = format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:channel/{name}");
    emulator
        .request(
            "POST",
            &format!("/channel/{}", encode(name)),
            Some(json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
            })),
        )
        .await;

    // (case, the body the tagging write carries)
    for (case, body) in [
        ("no member at all", json!({})),
        ("a member the shape does not declare", json!({ "Other": 1 })),
    ] {
        let refused = emulator
            .request("POST", &format!("/tags/{}", encode(&arn)), Some(body))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the modelled client error's status, got {}",
            refused.status
        );
        assert_eq!(
            refused.error_code, BAD_REQUEST_CODE,
            "case {case:?}: expected the one code this service's model declares, got {:?}",
            refused.error_code
        );
        assert!(
            refused.get("/message").as_str().is_some_and(|message| {
                message.contains("validation error") && message.contains("tags")
            }),
            "case {case:?}: the refusal names the member that was null, got {}",
            refused.body
        );
        assert_eq!(
            emulator.list_tags(&arn).await.get("/tags"),
            &json!({}),
            "case {case:?}: a refused write stores nothing"
        );
    }
}

