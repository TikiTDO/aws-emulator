// SPDX-License-Identifier: CC0-1.0

//! The rules this service follows when it REFUSES, and the negative paths of its
//! tagging, resource-policy, conditional-update, harvest-job and data-plane surfaces —
//! each asserted over a SET of inputs rather than one recorded request.
//!
//! 1. A not-found or in-use refusal carries a FIXED sentence, one per resource type and
//!    per rejection class, containing none of the caller's identifiers and no child
//!    count. The same sentence comes back whatever the names in the request are.
//! 2. The tagging operations parse `ResourceArn` BEFORE they look a resource up: a
//!    string that is not a syntactically valid ARN for one of this service's resources
//!    is `BadRequestException` (400), and a well-formed ARN naming an absent resource is
//!    `NotFoundException` (404). Never a 5xx, and never a 501.
//! 3. The resource-policy operations validate the policy document BEFORE they resolve
//!    the parent: a `Policy` that is not a policy document is `ValidationException`
//!    (400) with `INVALID_POLICY` even when the parent is also absent, and a valid
//!    policy against an absent parent is `ResourceNotFoundException` (404).
//! 4. N concurrent conditional updates that read one entity tag resolve to EXACTLY one
//!    winner, whatever N is; the losers get `ConflictException` (409) with
//!    `CONFLICTING_OPERATION`, and the winner's write is what a later read reports.
//! 5. A harvest job runs its lifecycle out of persisted state: `QUEUED` on create,
//!    `CANCELLED` after `CancelHarvestJob`, answered the same success when that
//!    `@idempotent` cancellation is repeated, and
//!    `ListHarvestJobs` reports exactly the jobs matching its filters.
//! 6. The ingest and egress URLs the control plane publishes are SERVED, and are gated:
//!    an anonymous egress fetch is 403 until the endpoint's resource policy grants
//!    `mediapackagev2:GetObject` to `*` and 404 once it does, and an ingest push with no
//!    credential is 403. This suite runs with request authentication OFF, where the
//!    emulator verifies no signature anywhere, so a push that merely PRESENTS a credential
//!    is served; `data_plane_credentials.rs` covers what verification does with one when
//!    authentication is on.

use std::sync::{Arc, Barrier};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every control-plane request in this suite is signed for. It travels in the
/// SigV4 credential scope, which is also what the service reads its region from.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

// The service's fixed rejection sentences, spelled out here so this suite pins the
// WORDING as well as the rule. Each is one sentence per resource type and per rejection
// class, and carries nothing from the request.
//
// `pub(crate)` on the not-found four because they are the crate's ONE spelling of those
// sentences: `not_found_wording_per_kind` asserts the per-KIND rule over them rather than
// writing a second copy of each, so one wording change lands in one place.
pub(crate) const CHANNEL_GROUP_NOT_FOUND: &str = "MediaPackage can't process your request \
     because we can't find your channel group. Verify your channel group name or add a \
     channel group and then try again";
pub(crate) const CHANNEL_NOT_FOUND: &str = "MediaPackage can't process your request because \
     we can't find your channel. Verify your channel name or add a channel and then try \
     again";
pub(crate) const ORIGIN_ENDPOINT_NOT_FOUND: &str = "MediaPackage can't process your request \
     because we can't find your origin endpoint. Verify your origin endpoint name or add an \
     origin endpoint and then try again";
pub(crate) const HARVEST_JOB_NOT_FOUND: &str = "MediaPackage can't process your request \
     because we can't find your harvest job. Verify your harvest job id or create a harvest \
     job and then try again";
const RESOURCE_ALREADY_EXISTS: &str = "MediaPackage can't create the resource you requested \
     because a resource with that name already exists. Verify the name of the resource and \
     then try again";
const CHANNEL_GROUP_IN_USE: &str = "The channel group you tried to delete has channels \
     attached to it. If you want to delete this channel group, you must first delete the \
     channels attached to it";
const CHANNEL_IN_USE: &str = "The channel you tried to delete has origin endpoints attached \
     to it. If you want to delete this channel, you must first delete the origin endpoints \
     attached to it";

/// A MediaPackageV2 emulator plus a tiny restJson1 client for it.
///
/// `pub(crate)` because it is the ONE client in this crate that can address a PUBLISHED
/// data-plane hostname with or without a credential (the shared control-plane client,
/// `channel_lifecycle::Emulator`, signs every request and always addresses the
/// control-plane host): `published_data_plane_urls` drives the data plane through this one
/// rather than carrying a second copy of it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

/// One HTTP answer: status, raw body, and the body parsed as JSON when it is JSON.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: String,
    pub(crate) json: Value,
}

impl Answer {
    /// The `__type` discriminator of an error envelope.
    fn code(&self) -> &str {
        self.json
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// The `Message` member of an error envelope.
    fn message(&self) -> &str {
        self.json
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    /// A named string member of the body.
    pub(crate) fn string(&self, member: &str) -> &str {
        self.json.get(member).and_then(Value::as_str).unwrap_or("")
    }
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite signs nothing: it exercises handler behaviour, not authentication.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        // A harvest job exports to a bucket Amazon S3 owns, and a create whose destination
        // the object store does not hold is refused, so the destinations [`harvest_body`]
        // names are held before any suite submits one.
        crate::channel_lifecycle::hold_harvest_destinations(&engine, REGION);
        Self { router }
    }

    /// Send one control-plane request: the service's own endpoint host plus a SigV4
    /// credential scope naming it.
    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        self.send(
            method,
            path,
            body,
            &format!("mediapackagev2.{REGION}.amazonaws.com"),
            true,
            &[],
        )
        .await
    }

    /// Send one request, choosing the `Host`, whether it carries a SigV4 credential, and
    /// any extra headers. The data-plane cases need all three.
    pub(crate) async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        host: &str,
        signed: bool,
        headers: &[(&str, &str)],
    ) -> Answer {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        self.send_raw(method, path, payload, host, signed, headers)
            .await
    }

    /// The same, with the body sent verbatim rather than as JSON — what an ingest push
    /// looks like, since the data plane takes opaque bytes.
    ///
    /// `pub(crate)` for the same reason [`Emulator`] itself is: a suite that pushes a body
    /// of a chosen SIZE to the ingest data plane drives it through this client rather than
    /// carrying a second copy of one.
    pub(crate) async fn send_raw(
        &self,
        method: &str,
        path: &str,
        payload: String,
        host: &str,
        signed: bool,
        headers: &[(&str, &str)],
    ) -> Answer {
        let mut request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host)
            .header("content-type", "application/json");
        if signed {
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/mediapackagev2/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            );
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = String::from_utf8_lossy(&bytes).to_string();
        let json = serde_json::from_str(&body).unwrap_or(Value::Null);
        Answer { status, body, json }
    }

    pub(crate) async fn create_group(&self, name: &str, body: Value) -> Answer {
        let mut body = body;
        body["ChannelGroupName"] = json!(name);
        self.call("POST", "/channelGroup", Some(body)).await
    }

    pub(crate) async fn create_channel(&self, group: &str, name: &str, body: Value) -> Answer {
        let mut body = body;
        body["ChannelName"] = json!(name);
        self.call("POST", &format!("/channelGroup/{group}/channel"), Some(body))
            .await
    }

    pub(crate) async fn create_endpoint(
        &self,
        group: &str,
        channel: &str,
        name: &str,
        body: Value,
    ) -> Answer {
        let mut body = body;
        body["OriginEndpointName"] = json!(name);
        self.call(
            "POST",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint"),
            Some(body),
        )
        .await
    }
}

/// Percent-encode an ARN so it fits one non-greedy `@httpLabel`, exactly as an AWS SDK
/// encodes a label value: its `:` and `/` are the reserved characters that would
/// otherwise split the path.
///
/// `pub(crate)` because it is the ONE label escaper this crate's test suites share —
/// `resource_tagging::tags_path` and `resource_tags` both address the tagging URIs
/// through it, so no suite needs a second copy and the same ARN always becomes the same
/// URI.
pub(crate) fn encoded(arn: &str) -> String {
    arn.chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The ARN of a channel group, as `state::channel_group_arn` builds it.
fn group_arn(group: &str) -> String {
    format!("arn:aws:mediapackagev2:{REGION}:{ACCOUNT}:channelGroup/{group}")
}

/// The ARN of a channel.
fn channel_arn(group: &str, channel: &str) -> String {
    format!("{}/channel/{channel}", group_arn(group))
}

/// The ARN of an origin endpoint.
fn endpoint_arn(group: &str, channel: &str, endpoint: &str) -> String {
    format!("{}/originEndpoint/{endpoint}", channel_arn(group, channel))
}

/// Assert one refusal: its status, its wire code, the discriminator member the shape
/// carries, the exact fixed sentence, and that NONE of the identifiers the request
/// carried appears anywhere in the body.
///
/// `pub(crate)` because it is the ONE fixed-refusal assertion this crate's suites share:
/// `not_found_wording_per_kind` drives the per-kind not-found rule through it rather than
/// restating these five checks.
pub(crate) fn assert_fixed_refusal(
    case: &str,
    answer: &Answer,
    status: u16,
    code: &str,
    discriminator: (&str, &str),
    message: &str,
    identifiers: &[&str],
) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        code,
        "case {case:?}: expected error code {code}, got {:?} with body {}",
        answer.code(),
        answer.body
    );
    let (member, expected_value) = discriminator;
    assert_eq!(
        answer.string(member),
        expected_value,
        "case {case:?}: expected {member} {expected_value}, got {:?}",
        answer.string(member)
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case:?}: expected the fixed sentence {message:?}, got {:?}",
        answer.message()
    );
    for identifier in identifiers {
        assert!(
            !answer.body.contains(identifier),
            "case {case:?}: the refusal must not echo the request identifier \
             {identifier:?}, got body {}",
            answer.body
        );
    }
}

/// One row of the fixed-sentence table: the three identifiers a run uses. The rule
/// generalises over the identifiers, so the rows differ in their length, their character
/// classes and whether they look like the recorded ones.
///
/// Every identifier carries a digit and is at least three characters long, because the
/// assertion that the refusal does not ECHO it is a substring search: a one-letter name
/// occurs inside ordinary English, so it could not distinguish an echo from the sentence.
fn identifier_rows() -> Vec<(String, String, String)> {
    vec![
        ("g1x".to_string(), "c1x".to_string(), "e1x".to_string()),
        (
            "sim2real-mpv2-cg-7db16ccaa90d".to_string(),
            "sim2real-mpv2-ch-dea1d83635b0".to_string(),
            "sim2real-mpv2-oe-ad323e8cbf26".to_string(),
        ),
        (
            "A_group_with_9_and-dashes".to_string(),
            "A_channel_0".to_string(),
            "An_endpoint_0".to_string(),
        ),
        // `ResourceName` is `@length 1..256`, so this row sits on the published maximum.
        (
            format!("G9{}", "g".repeat(254)),
            format!("C9{}", "c".repeat(254)),
            format!("E9{}", "e".repeat(254)),
        ),
    ]
}

#[tokio::test]
async fn not_found_and_conflict_refusals_carry_the_services_fixed_sentences() {
    for (group, channel, endpoint) in identifier_rows() {
        let names: &[&str] = &[&group, &channel, &endpoint];
        let emulator = Emulator::new();
        let absent = "absent-0";

        // --- the three not-found sentences, one per resource type ------------------
        let answer = emulator
            .call("GET", &format!("/channelGroup/{group}"), None)
            .await;
        assert_fixed_refusal(
            &format!("GetChannelGroup on an absent group {group}"),
            &answer,
            404,
            "ResourceNotFoundException",
            ("ResourceTypeNotFound", "CHANNEL_GROUP"),
            CHANNEL_GROUP_NOT_FOUND,
            names,
        );

        let answer = emulator.create_channel(&group, &channel, json!({})).await;
        assert_fixed_refusal(
            &format!("CreateChannel into an absent group {group}"),
            &answer,
            404,
            "ResourceNotFoundException",
            ("ResourceTypeNotFound", "CHANNEL_GROUP"),
            CHANNEL_GROUP_NOT_FOUND,
            names,
        );

        let answer = emulator
            .call(
                "GET",
                &format!("/channelGroup/{group}/channel/{channel}/originEndpoint"),
                None,
            )
            .await;
        assert_fixed_refusal(
            &format!("ListOriginEndpoints under an absent group {group}"),
            &answer,
            404,
            "ResourceNotFoundException",
            ("ResourceTypeNotFound", "CHANNEL_GROUP"),
            CHANNEL_GROUP_NOT_FOUND,
            names,
        );

        assert_eq!(
            emulator.create_group(&group, json!({})).await.status,
            200,
            "the group must be created for the channel-scope cases"
        );

        let answer = emulator
            .call(
                "GET",
                &format!("/channelGroup/{group}/channel/{channel}/"),
                None,
            )
            .await;
        assert_fixed_refusal(
            &format!("GetChannel on an absent channel {channel}"),
            &answer,
            404,
            "ResourceNotFoundException",
            ("ResourceTypeNotFound", "CHANNEL"),
            CHANNEL_NOT_FOUND,
            names,
        );

        let answer = emulator
            .call(
                "PUT",
                &format!("/channelGroup/{group}/channel/{channel}/policy"),
                Some(json!({"Policy": grant_policy("mediapackagev2:GetObject")})),
            )
            .await;
        assert_fixed_refusal(
            &format!("PutChannelPolicy on an absent channel {channel}"),
            &answer,
            404,
            "ResourceNotFoundException",
            ("ResourceTypeNotFound", "CHANNEL"),
            CHANNEL_NOT_FOUND,
            names,
        );

        assert_eq!(
            emulator
                .create_channel(&group, &channel, json!({}))
                .await
                .status,
            200,
            "the channel must be created for the endpoint-scope cases"
        );

        let answer = emulator
            .call(
                "GET",
                &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}"),
                None,
            )
            .await;
        assert_fixed_refusal(
            &format!("GetOriginEndpoint on an absent endpoint {endpoint}"),
            &answer,
            404,
            "ResourceNotFoundException",
            ("ResourceTypeNotFound", "ORIGIN_ENDPOINT"),
            ORIGIN_ENDPOINT_NOT_FOUND,
            names,
        );

        // --- the duplicate-create sentence, at all three levels --------------------
        let answer = emulator.create_group(&group, json!({})).await;
        assert_fixed_refusal(
            &format!("CreateChannelGroup repeating {group}"),
            &answer,
            409,
            "ConflictException",
            ("ConflictExceptionType", "RESOURCE_ALREADY_EXISTS"),
            RESOURCE_ALREADY_EXISTS,
            names,
        );

        let answer = emulator.create_channel(&group, &channel, json!({})).await;
        assert_fixed_refusal(
            &format!("CreateChannel repeating {channel}"),
            &answer,
            409,
            "ConflictException",
            ("ConflictExceptionType", "RESOURCE_ALREADY_EXISTS"),
            RESOURCE_ALREADY_EXISTS,
            names,
        );

        assert_eq!(
            emulator
                .create_endpoint(
                    &group,
                    &channel,
                    &endpoint,
                    // An endpoint must carry at least one manifest
                    // (`MIN_MANIFESTS_PER_ORIGIN_ENDPOINT`).
                    json!({
                        "ContainerType": "TS",
                        "HlsManifests": [{"ManifestName": "index"}],
                    }),
                )
                .await
                .status,
            200,
            "the endpoint must be created for the duplicate and in-use cases"
        );
        let answer = emulator
            .create_endpoint(
                &group,
                &channel,
                &endpoint,
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}],
                }),
            )
            .await;
        assert_fixed_refusal(
            &format!("CreateOriginEndpoint repeating {endpoint}"),
            &answer,
            409,
            "ConflictException",
            ("ConflictExceptionType", "RESOURCE_ALREADY_EXISTS"),
            RESOURCE_ALREADY_EXISTS,
            names,
        );

        // --- the two attached-children sentences, one per parent scope -------------
        let answer = emulator
            .call(
                "DELETE",
                &format!("/channelGroup/{group}/channel/{channel}/"),
                None,
            )
            .await;
        assert_fixed_refusal(
            &format!("DeleteChannel while {channel} still has an endpoint"),
            &answer,
            409,
            "ConflictException",
            ("ConflictExceptionType", "RESOURCE_IN_USE"),
            CHANNEL_IN_USE,
            names,
        );

        let answer = emulator
            .call("DELETE", &format!("/channelGroup/{group}"), None)
            .await;
        assert_fixed_refusal(
            &format!("DeleteChannelGroup while {group} still has a channel"),
            &answer,
            409,
            "ConflictException",
            ("ConflictExceptionType", "RESOURCE_IN_USE"),
            CHANNEL_GROUP_IN_USE,
            names,
        );

        // --- the harvest-job not-found sentence ------------------------------------
        let answer = emulator
            .call(
                "GET",
                &format!(
                    "/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}\
                     /harvestJob/{absent}"
                ),
                None,
            )
            .await;
        assert_fixed_refusal(
            &format!("GetHarvestJob on an absent job under {endpoint}"),
            &answer,
            404,
            "ResourceNotFoundException",
            ("ResourceTypeNotFound", "HARVEST_JOB"),
            HARVEST_JOB_NOT_FOUND,
            names,
        );
    }
}

/// A policy granting `action` to everyone — the shape MediaPackage's own endpoint-policy
/// examples take.
///
/// `pub(crate)` for the same reason as [`Emulator`]: `published_data_plane_urls` grants the
/// egress actions through this one rather than writing the statement out again.
pub(crate) fn grant_policy(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEveryone",
            "Effect": "Allow",
            "Principal": "*",
            "Action": action,
            "Resource": "*",
        }],
    })
    .to_string()
}

#[tokio::test]
async fn the_tagging_operations_judge_the_arn_before_the_resource() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("tg", json!({})).await.status, 200);
    assert_eq!(
        emulator.create_channel("tg", "tc", json!({})).await.status,
        200
    );
    assert_eq!(
        emulator
            .create_endpoint(
                "tg",
                "tc",
                "te",
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}],
                }),
            )
            .await
            .status,
        200
    );

    // Strings that cannot name a resource of this service. The rows differ in HOW they
    // fail: not an ARN at all, too few fields, another service, an unknown resource
    // type, an empty path segment, and a resource path that stops half-way.
    let malformed: &[(&str, String)] = &[
        ("not an ARN at all", "not-an-arn-at-all".to_string()),
        ("too few fields", "arn:aws:mediapackagev2".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/harvest"),
        ),
        (
            "an unknown resource type",
            format!("arn:aws:mediapackagev2:{REGION}:{ACCOUNT}:channel/tc"),
        ),
        (
            "an empty resource segment",
            format!("arn:aws:mediapackagev2:{REGION}:{ACCOUNT}:channelGroup/"),
        ),
        (
            "a resource path that stops half-way",
            format!("arn:aws:mediapackagev2:{REGION}:{ACCOUNT}:channelGroup/tg/channel"),
        ),
        (
            "an empty partition",
            format!("arn::mediapackagev2:{REGION}:{ACCOUNT}:channelGroup/tg"),
        ),
    ];
    for (label, arn) in malformed {
        for (operation, method, body) in [
            ("ListTagsForResource", "GET", None),
            ("TagResource", "POST", Some(json!({"tags": {"k": "v"}}))),
        ] {
            let answer = emulator
                .call(method, &format!("/tags/{}", encoded(arn)), body)
                .await;
            assert_eq!(
                answer.status, 400,
                "case {operation:?} with {label:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "BadRequestException",
                "case {operation:?} with {label:?}: expected BadRequestException, got {:?}",
                answer.code()
            );
        }
    }

    // A well-formed ARN naming a resource that is not there is a DIFFERENT class, at
    // every level of the hierarchy.
    let absent: &[(&str, String)] = &[
        ("an absent channel group", group_arn("no-such-group")),
        ("an absent channel", channel_arn("tg", "no-such-channel")),
        (
            "an absent origin endpoint",
            endpoint_arn("tg", "tc", "no-such-endpoint"),
        ),
    ];
    for (label, arn) in absent {
        for (operation, method, body) in [
            ("ListTagsForResource", "GET", None),
            ("TagResource", "POST", Some(json!({"tags": {"k": "v"}}))),
        ] {
            let answer = emulator
                .call(method, &format!("/tags/{}", encoded(arn)), body)
                .await;
            assert_eq!(
                answer.status, 404,
                "case {operation:?} with {label:?}: expected HTTP 404, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "NotFoundException",
                "case {operation:?} with {label:?}: expected NotFoundException, got {:?}",
                answer.code()
            );
        }
    }

    // And a tag round trip at every level: what `TagResource` assigns is what
    // `ListTagsForResource` reports, and `UntagResource` removes exactly the named keys.
    let levels: &[(&str, String)] = &[
        ("channel group", group_arn("tg")),
        ("channel", channel_arn("tg", "tc")),
        ("origin endpoint", endpoint_arn("tg", "tc", "te")),
    ];
    for (label, arn) in levels {
        let answer = emulator
            .call(
                "POST",
                &format!("/tags/{}", encoded(arn)),
                Some(json!({"tags": {"one": "1", "two": "2"}})),
            )
            .await;
        assert_eq!(
            answer.status, 204,
            "case {label:?}: TagResource must answer its modelled 204, got {} with body {}",
            answer.status, answer.body
        );

        let answer = emulator
            .call("GET", &format!("/tags/{}", encoded(arn)), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.json.get("tags"),
            Some(&json!({"one": "1", "two": "2"})),
            "case {label:?}: the tags read back must be the tags assigned, got {}",
            answer.body
        );

        let answer = emulator
            .call(
                "DELETE",
                &format!("/tags/{}?tagKeys=one", encoded(arn)),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 204,
            "case {label:?}: UntagResource must answer its modelled 204, got {} with body {}",
            answer.status, answer.body
        );
        let answer = emulator
            .call("GET", &format!("/tags/{}", encoded(arn)), None)
            .await;
        assert_eq!(
            answer.json.get("tags"),
            Some(&json!({"two": "2"})),
            "case {label:?}: untagging must remove exactly the named key, got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_policy_operations_validate_the_document_before_resolving_the_parent() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("pg", json!({})).await.status, 200);
    assert_eq!(
        emulator.create_channel("pg", "pc", json!({})).await.status,
        200
    );
    assert_eq!(
        emulator
            .create_endpoint(
                "pg",
                "pc",
                "pe",
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}],
                }),
            )
            .await
            .status,
        200
    );

    // Documents that are not policy documents. A resource policy is a JSON OBJECT
    // carrying a `Statement` (`state::is_policy_document`), so the rows differ in HOW
    // they fail that: not JSON, the wrong JSON type twice over, and no `Statement`.
    let malformed: &[(&str, &str)] = &[
        ("not JSON at all", "{not json"),
        ("a JSON array", "[]"),
        ("a JSON string", "\"Allow\""),
        ("truncated JSON", "{\"Statement\":[]"),
        ("no Statement member", "{\"Version\":\"2012-10-17\"}"),
    ];
    // Both scopes, and both an existing parent and an absent one: the validation verdict
    // must not depend on whether the parent resolves, which is what proves the order.
    let scopes: &[(&str, &str, &str)] = &[
        ("PutChannelPolicy on an existing channel", "PUT", "/channelGroup/pg/channel/pc/policy"),
        ("PutChannelPolicy on an absent channel", "PUT", "/channelGroup/pg/channel/nope/policy"),
        (
            "PutOriginEndpointPolicy on an existing endpoint",
            "POST",
            "/channelGroup/pg/channel/pc/originEndpoint/pe/policy",
        ),
        (
            "PutOriginEndpointPolicy on an absent endpoint",
            "POST",
            "/channelGroup/pg/channel/pc/originEndpoint/nope/policy",
        ),
    ];
    for (scope, method, path) in scopes {
        for (label, policy) in malformed {
            let answer = emulator
                .call(method, path, Some(json!({"Policy": policy})))
                .await;
            assert_eq!(
                answer.status, 400,
                "case {scope:?} with {label:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "ValidationException",
                "case {scope:?} with {label:?}: expected ValidationException, got {:?}",
                answer.code()
            );
            assert_eq!(
                answer.string("ValidationExceptionType"),
                "INVALID_POLICY",
                "case {scope:?} with {label:?}: expected INVALID_POLICY, got {:?}",
                answer.string("ValidationExceptionType")
            );
        }
    }

    // A VALID policy against an absent parent is the other class, with the fixed
    // per-resource sentence.
    let valid = grant_policy("mediapackagev2:GetObject");
    let answer = emulator
        .call(
            "PUT",
            "/channelGroup/pg/channel/nope/policy",
            Some(json!({"Policy": valid})),
        )
        .await;
    assert_fixed_refusal(
        "PutChannelPolicy with a valid policy on an absent channel",
        &answer,
        404,
        "ResourceNotFoundException",
        ("ResourceTypeNotFound", "CHANNEL"),
        CHANNEL_NOT_FOUND,
        &["nope"],
    );
    let answer = emulator
        .call(
            "POST",
            "/channelGroup/pg/channel/pc/originEndpoint/nope/policy",
            Some(json!({"Policy": valid})),
        )
        .await;
    assert_fixed_refusal(
        "PutOriginEndpointPolicy with a valid policy on an absent endpoint",
        &answer,
        404,
        "ResourceNotFoundException",
        ("ResourceTypeNotFound", "ORIGIN_ENDPOINT"),
        ORIGIN_ENDPOINT_NOT_FOUND,
        &["nope"],
    );

    // And a policy round trip, over documents differing in statement count, principal
    // form and action form.
    let accepted: &[(&str, String)] = &[
        ("one statement, a bare `*` principal", grant_policy("mediapackagev2:GetObject")),
        (
            "an AWS-map principal and a wildcard action",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "*"},
                    "Action": ["mediapackagev2:*"],
                }],
            })
            .to_string(),
        ),
        (
            "two statements, one of them a Deny",
            json!({
                "Statement": [
                    {"Effect": "Allow", "Principal": "*", "Action": "mediapackagev2:GetObject"},
                    {"Effect": "Deny", "Principal": "*", "Action": "mediapackagev2:PutObject"},
                ],
            })
            .to_string(),
        ),
    ];
    for (label, policy) in accepted {
        let answer = emulator
            .call(
                "PUT",
                "/channelGroup/pg/channel/pc/policy",
                Some(json!({"Policy": policy})),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: PutChannelPolicy must accept it, got {} with body {}",
            answer.status, answer.body
        );
        let answer = emulator
            .call("GET", "/channelGroup/pg/channel/pc/policy", None)
            .await;
        assert_eq!(
            answer.string("Policy"),
            policy.as_str(),
            "case {label:?}: GetChannelPolicy must report the stored policy byte for byte"
        );

        let answer = emulator
            .call(
                "POST",
                "/channelGroup/pg/channel/pc/originEndpoint/pe/policy",
                Some(json!({"Policy": policy})),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: PutOriginEndpointPolicy must accept it, got {} with body {}",
            answer.status, answer.body
        );
        let answer = emulator
            .call(
                "GET",
                "/channelGroup/pg/channel/pc/originEndpoint/pe/policy",
                None,
            )
            .await;
        assert_eq!(
            answer.string("Policy"),
            policy.as_str(),
            "case {label:?}: GetOriginEndpointPolicy must report the stored policy byte for \
             byte"
        );
    }

    // A channel that carries no policy is not the same as a channel that is absent, and
    // both are 404 on the POLICY, which is the resource this read addresses.
    assert_eq!(
        emulator.create_channel("pg", "unpolicied", json!({})).await.status,
        200
    );
    let answer = emulator
        .call("GET", "/channelGroup/pg/channel/unpolicied/policy", None)
        .await;
    assert_eq!(
        answer.status, 404,
        "case \"GetChannelPolicy with no policy attached\": expected HTTP 404, got {} with \
         body {}",
        answer.status, answer.body
    );
}

/// How many concurrent racers each row of the conditional-update table fires. The
/// invariant is "exactly one winner" whatever N is.
const RACER_COUNTS: &[usize] = &[2, 3, 5];

#[test]
fn concurrent_conditional_updates_resolve_to_exactly_one_winner() {
    for &racers in RACER_COUNTS {
        for scope in ["channel group", "channel"] {
            let emulator = Arc::new(Emulator::new());
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            let (path, read_path) = match scope {
                "channel group" => ("/channelGroup/rg".to_string(), "/channelGroup/rg".to_string()),
                _ => (
                    "/channelGroup/rg/channel/rc/".to_string(),
                    "/channelGroup/rg/channel/rc/".to_string(),
                ),
            };
            let etag = runtime.block_on(async {
                assert_eq!(emulator.create_group("rg", json!({})).await.status, 200);
                assert_eq!(
                    emulator.create_channel("rg", "rc", json!({})).await.status,
                    200
                );
                let answer = emulator.call("GET", &read_path, None).await;
                assert_eq!(
                    answer.status, 200,
                    "case {scope:?}/{racers}: the read before the race must succeed, got {}",
                    answer.body
                );
                answer.string("ETag").to_string()
            });
            assert!(
                !etag.is_empty(),
                "case {scope:?}/{racers}: the resource must report an ETag to race on"
            );

            // Every racer presents the SAME entity tag, and they are released together,
            // so the single-winner property is a property of the serialization point and
            // not of the order the threads happened to start in.
            let barrier = Barrier::new(racers);
            let statuses: Vec<u16> = std::thread::scope(|scope_handle| {
                let handles: Vec<_> = (0..racers)
                    .map(|racer| {
                        let emulator = Arc::clone(&emulator);
                        let etag = etag.clone();
                        let path = path.clone();
                        let barrier = &barrier;
                        scope_handle.spawn(move || {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("a racer runtime");
                            barrier.wait();
                            runtime
                                .block_on(emulator.send(
                                    "PUT",
                                    &path,
                                    Some(json!({"Description": format!("racer {racer}")})),
                                    &format!("mediapackagev2.{REGION}.amazonaws.com"),
                                    true,
                                    &[("x-amzn-update-if-match", &etag)],
                                ))
                                .status
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| handle.join().expect("a racer thread"))
                    .collect()
            });

            let winners = statuses.iter().filter(|status| **status == 200).count();
            assert_eq!(
                winners, 1,
                "case {scope:?}/{racers}: expected exactly one winner, got statuses \
                 {statuses:?}"
            );
            let losers = statuses.iter().filter(|status| **status == 409).count();
            assert_eq!(
                losers,
                racers - 1,
                "case {scope:?}/{racers}: every loser must be refused 409, got statuses \
                 {statuses:?}"
            );

            // The winner's write is what a later read reports, and the entity tag moved.
            let after = runtime.block_on(emulator.call("GET", &read_path, None));
            assert!(
                after.string("Description").starts_with("racer "),
                "case {scope:?}/{racers}: the read after the race must report a racer's \
                 write, got {}",
                after.body
            );
            assert_ne!(
                after.string("ETag"),
                etag.as_str(),
                "case {scope:?}/{racers}: the committed entity tag must differ from the one \
                 the racers presented"
            );
        }
    }
}

#[tokio::test]
async fn a_stale_entity_tag_is_refused_and_a_fresh_one_commits() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("eg", json!({})).await.status, 200);
    assert_eq!(
        emulator.create_channel("eg", "ec", json!({})).await.status,
        200
    );

    for (label, path) in [
        ("channel group", "/channelGroup/eg"),
        ("channel", "/channelGroup/eg/channel/ec/"),
    ] {
        let first = emulator.call("GET", path, None).await;
        let stale = first.string("ETag").to_string();

        // An update with NO entity tag is unconditional, which is what the model's
        // optional member means.
        let answer = emulator
            .call("PUT", path, Some(json!({"Description": "unconditional"})))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: an update with no ETag must commit, got {} with body {}",
            answer.status, answer.body
        );
        let fresh = answer.string("ETag").to_string();
        assert_ne!(
            fresh, stale,
            "case {label:?}: a commit must move the entity tag"
        );

        // Now the first tag is stale, and presenting it is the conflict class.
        let answer = emulator
            .send(
                "PUT",
                path,
                Some(json!({"Description": "stale"})),
                &format!("mediapackagev2.{REGION}.amazonaws.com"),
                true,
                &[("x-amzn-update-if-match", &stale)],
            )
            .await;
        assert_eq!(
            answer.status, 409,
            "case {label:?}: a stale ETag must be refused 409, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {:?}",
            answer.code()
        );
        assert_eq!(
            answer.string("ConflictExceptionType"),
            "CONFLICTING_OPERATION",
            "case {label:?}: expected CONFLICTING_OPERATION, got {:?}",
            answer.string("ConflictExceptionType")
        );

        // And the fresh one commits, which a later read reports.
        let answer = emulator
            .send(
                "PUT",
                path,
                Some(json!({"Description": "conditional"})),
                &format!("mediapackagev2.{REGION}.amazonaws.com"),
                true,
                &[("x-amzn-update-if-match", &fresh)],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: the committed ETag must be accepted, got {} with body {}",
            answer.status, answer.body
        );
        let read_back = emulator.call("GET", path, None).await;
        assert_eq!(
            read_back.string("Description"),
            "conditional",
            "case {label:?}: the read must report the conditional write, got {}",
            read_back.body
        );
    }
}

/// A harvest job body whose schedule runs from `start` to `end` (epoch seconds) and whose
/// harvested manifests are the named HLS manifests.
///
/// `pub(crate)` with [`epoch_seconds_from_now`] below: they are the one `CreateHarvestJob`
/// body this crate's suites share, so `harvest_job_tagging` creates its jobs through them
/// rather than carrying a second copy.
pub(crate) fn harvest_body(start: f64, end: f64, manifests: &[&str]) -> Value {
    json!({
        "HarvestedManifests": {
            "HlsManifests": manifests
                .iter()
                .map(|name| json!({"ManifestName": name}))
                .collect::<Vec<_>>(),
        },
        "ScheduleConfiguration": {"StartTime": start, "EndTime": end},
        "Destination": {
            "S3Destination": {
                "BucketName": crate::channel_lifecycle::HARVEST_BUCKET,
                "DestinationPath": "out/",
            },
        },
    })
}

/// Epoch seconds, far enough ahead that a scheduled window does not end mid-test.
pub(crate) fn epoch_seconds_from_now(offset: f64) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64();
    now + offset
}

#[tokio::test]
async fn harvest_jobs_run_their_lifecycle_out_of_persisted_state() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("hg", json!({})).await.status, 200);
    assert_eq!(
        emulator.create_channel("hg", "hc", json!({})).await.status,
        200
    );
    assert_eq!(
        emulator
            .create_endpoint(
                "hg",
                "hc",
                "he",
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": "index"}, {"ManifestName": "backup"}],
                }),
            )
            .await
            .status,
        200
    );
    let endpoint_path = "/channelGroup/hg/channel/hc/originEndpoint/he";

    // Schedules the service refuses, differing in HOW the window is wrong.
    let start = epoch_seconds_from_now(60.0);
    let bad_schedules: &[(&str, f64, f64, &str)] = &[
        (
            "the end is before the start",
            start,
            start - 1.0,
            "END_TIME_EARLIER_THAN_START_TIME",
        ),
        (
            "the end is an hour before the start",
            start,
            start - 3600.0,
            "END_TIME_EARLIER_THAN_START_TIME",
        ),
        (
            "the window has zero length",
            start,
            start,
            "INVALID_HARVEST_JOB_DURATION",
        ),
    ];
    for (label, from, to, expected) in bad_schedules {
        let mut body = harvest_body(*from, *to, &["index"]);
        body["HarvestJobName"] = json!("rejected");
        let answer = emulator
            .call("POST", &format!("{endpoint_path}/harvestJob"), Some(body))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.string("ValidationExceptionType"),
            *expected,
            "case {label:?}: expected {expected}, got {:?}",
            answer.string("ValidationExceptionType")
        );
    }

    // A harvested manifest the endpoint does not publish is its own class.
    let mut body = harvest_body(start, start + 600.0, &["not-published"]);
    body["HarvestJobName"] = json!("rejected");
    let answer = emulator
        .call("POST", &format!("{endpoint_path}/harvestJob"), Some(body))
        .await;
    assert_eq!(
        answer.string("ValidationExceptionType"),
        "HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT",
        "case \"a manifest the endpoint does not publish\": got {}",
        answer.body
    );

    // Accepted jobs, differing in how many manifests they harvest and whether they name
    // themselves. Each is created QUEUED and read back from state.
    let accepted: &[(&str, &[&str], Option<&str>)] = &[
        ("one manifest, a caller-chosen name", &["index"], Some("job-a")),
        (
            "two manifests, a caller-chosen name",
            &["index", "backup"],
            Some("job_b-2"),
        ),
        ("one manifest, a service-chosen name", &["backup"], None),
    ];
    let mut created: Vec<(String, String)> = Vec::new();
    for (label, manifests, name) in accepted {
        let mut body = harvest_body(start, start + 600.0, manifests);
        if let Some(name) = name {
            body["HarvestJobName"] = json!(name);
        }
        let answer = emulator
            .call("POST", &format!("{endpoint_path}/harvestJob"), Some(body))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.string("Status"),
            "QUEUED",
            "case {label:?}: a new job must be QUEUED, got {:?}",
            answer.string("Status")
        );
        let job = answer.string("HarvestJobName").to_string();
        assert!(
            !job.is_empty(),
            "case {label:?}: the job must report the name it was given, got {}",
            answer.body
        );
        if let Some(name) = name {
            assert_eq!(
                job, *name,
                "case {label:?}: a caller-chosen name must be kept"
            );
        }

        // Read back through the implemented reader: the create's own bytes.
        let read = emulator
            .call("GET", &format!("{endpoint_path}/harvestJob/{job}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetHarvestJob must find the job, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("Arn"),
            answer.string("Arn"),
            "case {label:?}: the ARN must be the one the create derived"
        );
        created.push((label.to_string(), job));
    }

    // A duplicate name on the same endpoint is the duplicate-create class.
    let mut body = harvest_body(start, start + 600.0, &["index"]);
    body["HarvestJobName"] = json!("job-a");
    let answer = emulator
        .call("POST", &format!("{endpoint_path}/harvestJob"), Some(body))
        .await;
    assert_fixed_refusal(
        "CreateHarvestJob repeating job-a",
        &answer,
        409,
        "ConflictException",
        ("ConflictExceptionType", "RESOURCE_ALREADY_EXISTS"),
        RESOURCE_ALREADY_EXISTS,
        &["job-a"],
    );

    // Cancelling moves the status, which `GetHarvestJob` reports; cancelling again is the
    // SAME request (`@idempotent`), so it is answered the same success and the job stays
    // as the first cancellation left it.
    let (label, job) = &created[0];
    let answer = emulator
        .call("PUT", &format!("{endpoint_path}/harvestJob/{job}"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: CancelHarvestJob must succeed, got {} with body {}",
        answer.status, answer.body
    );
    let read = emulator
        .call("GET", &format!("{endpoint_path}/harvestJob/{job}"), None)
        .await;
    assert_eq!(
        read.string("Status"),
        "CANCELLED",
        "case {label:?}: the cancelled job must report CANCELLED, got {}",
        read.body
    );
    let answer = emulator
        .call("PUT", &format!("{endpoint_path}/harvestJob/{job}"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: cancelling a cancelled job again must be accepted, got {} with \
         body {}",
        answer.status, answer.body
    );
    let read_again = emulator
        .call("GET", &format!("{endpoint_path}/harvestJob/{job}"), None)
        .await;
    assert_eq!(
        read_again.json["ETag"], read.json["ETag"],
        "case {label:?}: a repeated cancellation must commit nothing, got {}",
        read_again.body
    );

    // The status filter reports exactly the jobs the store holds in that status. The
    // QUEUED -> IN_PROGRESS transition is owed by the CLOCK, so the two live statuses are
    // counted together rather than pinned to one of them: what the filter must guarantee
    // is that every job appears under its own status and under no other.
    let statuses: &[&str] = &["QUEUED", "IN_PROGRESS", "CANCELLED", "COMPLETED", "FAILED"];
    let mut counted = 0usize;
    for status in statuses {
        let answer = emulator
            .call(
                "GET",
                &format!("/channelGroup/hg/harvestJob?includeStatus={status}"),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"includeStatus={status}\": expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        let items = answer
            .json
            .get("Items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in &items {
            assert_eq!(
                item.get("Status").and_then(Value::as_str),
                Some(*status),
                "case \"includeStatus={status}\": every reported job must be in that \
                 status, got {}",
                answer.body
            );
        }
        counted += items.len();
    }
    assert_eq!(
        counted,
        created.len(),
        "every job the group holds must appear under exactly one status, got {counted} \
         across {statuses:?} for {} jobs",
        created.len()
    );
    let answer = emulator
        .call(
            "GET",
            "/channelGroup/hg/harvestJob?includeStatus=CANCELLED",
            None,
        )
        .await;
    let cancelled = answer
        .json
        .get("Items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    assert_eq!(
        cancelled, 1,
        "case \"includeStatus=CANCELLED\": exactly the cancelled job must be reported, \
         got {}",
        answer.body
    );

    // Unfiltered, the group reports every job it holds.
    let answer = emulator
        .call("GET", "/channelGroup/hg/harvestJob", None)
        .await;
    let items = answer
        .json
        .get("Items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    assert_eq!(
        items,
        created.len(),
        "case \"no filter\": expected every job, got {items} in {}",
        answer.body
    );

    // And the channel/endpoint filters narrow to one branch of the hierarchy.
    assert_eq!(
        emulator.create_channel("hg", "other", json!({})).await.status,
        200
    );
    let answer = emulator
        .call("GET", "/channelGroup/hg/harvestJob?channelName=other", None)
        .await;
    let items = answer
        .json
        .get("Items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    assert_eq!(
        items, 0,
        "case \"channelName=other\": a channel with no jobs must report none, got {}",
        answer.body
    );
}

#[tokio::test]
async fn the_tag_store_is_bounded() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("bg", json!({})).await.status, 200);
    let arn = group_arn("bg");

    // Exactly the published ceiling is accepted; one past it is refused. The ceiling is
    // `state::MAX_TAGS_PER_RESOURCE` (50), AWS's tag quota per resource.
    let at_ceiling: serde_json::Map<String, Value> = (0..50)
        .map(|index| (format!("key{index}"), json!("v")))
        .collect();
    let answer = emulator
        .call(
            "POST",
            &format!("/tags/{}", encoded(&arn)),
            Some(json!({"tags": at_ceiling})),
        )
        .await;
    assert_eq!(
        answer.status, 204,
        "case \"50 tags, the published ceiling\": expected HTTP 204, got {} with body {}",
        answer.status, answer.body
    );

    // Each row is past ONE of the three bounds, so each is refused on its own.
    let past: &[(&str, Value)] = &[
        (
            "51 tags in one request",
            json!({"tags": (0..51)
                .map(|index| (format!("fresh{index}"), json!("v")))
                .collect::<serde_json::Map<String, Value>>()}),
        ),
        (
            "one fresh tag past the ceiling the resource already holds",
            json!({"tags": {"one-too-many": "v"}}),
        ),
        (
            "a tag key of 129 characters",
            json!({"tags": {"k".repeat(129): "v"}}),
        ),
        (
            "a tag value of 257 characters",
            json!({"tags": {"key0": "v".repeat(257)}}),
        ),
    ];
    for (label, body) in past {
        let answer = emulator
            .call("POST", &format!("/tags/{}", encoded(&arn)), Some(body.clone()))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {:?}",
            answer.code()
        );
    }

    // And a refused request wrote nothing: the resource still holds exactly the tags the
    // accepted call left on it.
    let read = emulator
        .call("GET", &format!("/tags/{}", encoded(&arn)), None)
        .await;
    let held = read
        .json
        .get("tags")
        .and_then(Value::as_object)
        .map(serde_json::Map::len)
        .unwrap_or(0);
    assert_eq!(
        held, 50,
        "a refused tag request must leave the tag map alone, got {}",
        read.body
    );

    // The same bound applies to the tag map a CREATE carries, at every level that takes
    // one.
    let creates: &[(&str, &str, Value)] = &[
        ("CreateChannelGroup", "/channelGroup", json!({"ChannelGroupName": "bg2"})),
        (
            "CreateChannel",
            "/channelGroup/bg/channel",
            json!({"ChannelName": "bc"}),
        ),
    ];
    for (label, path, body) in creates {
        let mut body = body.clone();
        body["tags"] = json!((0..51)
            .map(|index| (format!("key{index}"), json!("v")))
            .collect::<serde_json::Map<String, Value>>());
        let answer = emulator.call("POST", path, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?} with 51 tags: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "ValidationException",
            "case {label:?} with 51 tags: expected ValidationException, got {:?}",
            answer.code()
        );
    }
}

#[tokio::test]
async fn the_ingest_store_is_bounded_per_object_per_channel_and_overall() {
    let emulator = Emulator::new();
    let group = emulator.create_group("ig", json!({})).await;
    assert_eq!(group.status, 200, "the group must be created: {}", group.body);
    let channel = emulator.create_channel("ig", "ic", json!({})).await;
    assert_eq!(
        channel.status, 200,
        "the channel must be created: {}",
        channel.body
    );
    let ingest_url = channel
        .json
        .get("IngestEndpoints")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|first| first.get("Url"))
        .and_then(Value::as_str)
        .expect("a channel publishes ingest URLs")
        .to_string();
    let (host, path) = split_url(&ingest_url);

    // A push of `bytes` bytes to object `name`, as a raw body rather than JSON.
    let push = |name: String, bytes: usize| {
        let host = host.clone();
        let path = path.clone();
        let emulator = &emulator;
        async move {
            emulator
                .send_raw(
                    "PUT",
                    &format!("{path}/{name}"),
                    "x".repeat(bytes),
                    &host,
                    true,
                    &[],
                )
                .await
        }
    };

    // Per OBJECT: exactly at the published per-object ceiling is accepted, one byte past
    // it is refused. The ceiling is `state::MAX_INGESTED_OBJECT_BYTES` (8 MiB).
    let per_object = 8 * 1024 * 1024;
    let answer = push("at-ceiling".to_string(), per_object).await;
    assert_eq!(
        answer.status, 200,
        "case \"an object exactly at the per-object ceiling\": expected HTTP 200, got {} \
         with body {}",
        answer.status, answer.body
    );
    let answer = push("past-ceiling".to_string(), per_object + 1).await;
    assert_eq!(
        answer.status, 413,
        "case \"an object one byte past the per-object ceiling\": expected HTTP 413, got {} \
         with body {}",
        answer.status, answer.body
    );

    // Store-wide: the retained total is capped at `state::MAX_INGESTED_BYTES` (64 MiB),
    // so pushing 8 MiB objects stops being accepted after the eighth even though the
    // per-channel object COUNT ceiling (64) is nowhere near reached. Re-pushing an object
    // that is already there replaces it, so it does not consume more of the budget.
    let mut accepted = 1; // the at-ceiling push above
    for index in 0..16 {
        let answer = push(format!("object-{index}"), per_object).await;
        match answer.status {
            200 => accepted += 1,
            507 => break,
            other => panic!(
                "case \"filling the store with 8 MiB objects\": expected 200 or 507, got \
                 {other} with body {}",
                answer.body
            ),
        }
    }
    assert_eq!(
        accepted, 8,
        "case \"filling the store\": 64 MiB of budget must take exactly eight 8 MiB \
         objects, got {accepted}"
    );
    let answer = push("one-too-many".to_string(), per_object).await;
    assert_eq!(
        answer.status, 507,
        "case \"a push past the store-wide budget\": expected HTTP 507, got {} with body {}",
        answer.status, answer.body
    );
    // Replacing an object already in the store gives its bytes back, so it still fits.
    let answer = push("at-ceiling".to_string(), per_object).await;
    assert_eq!(
        answer.status, 200,
        "case \"replacing an object already in the store\": expected HTTP 200, got {} with \
         body {}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn the_published_data_plane_urls_are_served_and_gated_by_the_endpoint_policy() {
    let emulator = Emulator::new();
    let group = emulator.create_group("dg", json!({})).await;
    assert_eq!(group.status, 200, "the group must be created: {}", group.body);
    let egress_domain = group.string("EgressDomain").to_string();
    assert!(
        egress_domain.contains(".egress."),
        "the group must publish an egress domain, got {}",
        group.body
    );

    let channel = emulator.create_channel("dg", "dc", json!({})).await;
    assert_eq!(
        channel.status, 200,
        "the channel must be created: {}",
        channel.body
    );
    let ingest_url = channel
        .json
        .get("IngestEndpoints")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|first| first.get("Url"))
        .and_then(Value::as_str)
        .expect("a channel publishes ingest URLs")
        .to_string();

    let endpoint = emulator
        .create_endpoint(
            "dg",
            "dc",
            "de",
            json!({"ContainerType": "TS", "HlsManifests": [{"ManifestName": "index"}]}),
        )
        .await;
    assert_eq!(
        endpoint.status, 200,
        "the endpoint must be created: {}",
        endpoint.body
    );
    let manifest_url = endpoint
        .json
        .get("HlsManifests")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|first| first.get("Url"))
        .and_then(Value::as_str)
        .expect("an endpoint publishes manifest URLs")
        .to_string();

    // --- the ingest half: no credential is refused, a credentialed push is accepted ---
    let (ingest_host, ingest_path) = split_url(&ingest_url);
    let segment_path = format!("{ingest_path}/0.mp4");
    let answer = emulator
        .send(
            "PUT",
            &segment_path,
            Some(json!({"segment": "bytes"})),
            &ingest_host,
            false,
            &[],
        )
        .await;
    assert_eq!(
        answer.status, 403,
        "case \"an unauthenticated ingest PUT\": expected HTTP 403, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.body.to_lowercase().contains("access denied"),
        "case \"an unauthenticated ingest PUT\": the body must say access denied, got {}",
        answer.body
    );
    let answer = emulator
        .send(
            "PUT",
            &segment_path,
            Some(json!({"segment": "bytes"})),
            &ingest_host,
            true,
            &[],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"a credentialed ingest PUT\": expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );

    // --- the egress half: the endpoint's resource policy is the gate ---------------
    let (egress_host, egress_path) = split_url(&manifest_url);
    assert_eq!(
        egress_host, egress_domain,
        "the manifest URL must be served from the group's published egress domain"
    );

    // Policies and what each means for an ANONYMOUS fetch. 403 without a grant, 404 with
    // one (authorized, and nothing packaged for it yet).
    let policies: &[(&str, Option<String>, u16)] = &[
        ("no policy at all", None, 403),
        (
            "a policy granting a different action",
            Some(grant_policy("mediapackagev2:PutObject")),
            403,
        ),
        (
            "a policy granting GetObject to a named principal only",
            Some(
                json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
                        "Action": "mediapackagev2:GetObject",
                    }],
                })
                .to_string(),
            ),
            403,
        ),
        (
            "a grant of GetObject to everyone",
            Some(grant_policy("mediapackagev2:GetObject")),
            404,
        ),
        (
            "a wildcard grant to everyone",
            Some(grant_policy("mediapackagev2:*")),
            404,
        ),
        (
            "a grant to everyone with a Deny of the same action",
            Some(
                json!({
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": "*",
                            "Action": "mediapackagev2:GetObject",
                        },
                        {
                            "Effect": "Deny",
                            "Principal": "*",
                            "Action": "mediapackagev2:GetObject",
                        },
                    ],
                })
                .to_string(),
            ),
            403,
        ),
        // The evaluator understands Effect/Action/Principal/Resource only, so it must
        // fail CLOSED on a statement narrowed or inverted by a member it cannot judge:
        // honouring a conditional Allow as an unconditional one would authorize a fetch
        // on a condition nothing checked.
        (
            "a grant to everyone qualified by a Condition",
            Some(
                json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "mediapackagev2:GetObject",
                        "Condition": {"IpAddress": {"aws:SourceIp": "203.0.113.0/24"}},
                    }],
                })
                .to_string(),
            ),
            403,
        ),
        (
            "a grant to everyone written with NotAction",
            Some(
                json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "mediapackagev2:GetObject",
                        "NotAction": "mediapackagev2:PutObject",
                    }],
                })
                .to_string(),
            ),
            403,
        ),
        // `Resource` IS evaluated, against the ARN of the endpoint the published URL
        // names, so a grant scoped to another resource does not reach this one.
        (
            "a grant scoped to another origin endpoint",
            Some(
                json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "mediapackagev2:GetObject",
                        "Resource": endpoint_arn("dg", "dc", "someone-else"),
                    }],
                })
                .to_string(),
            ),
            403,
        ),
        (
            "a grant scoped to this endpoint by its exact ARN",
            Some(
                json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "mediapackagev2:GetObject",
                        "Resource": endpoint_arn("dg", "dc", "de"),
                    }],
                })
                .to_string(),
            ),
            404,
        ),
        (
            "a grant scoped to this endpoint by a wildcard ARN",
            Some(
                json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "mediapackagev2:GetObject",
                        "Resource": format!("{}*", channel_arn("dg", "dc")),
                    }],
                })
                .to_string(),
            ),
            404,
        ),
        (
            "a grant with no Resource at all, which means the attached one",
            Some(
                json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "mediapackagev2:GetObject",
                    }],
                })
                .to_string(),
            ),
            404,
        ),
    ];
    for (label, policy, expected) in policies {
        if let Some(policy) = policy {
            let put = emulator
                .call(
                    "POST",
                    "/channelGroup/dg/channel/dc/originEndpoint/de/policy",
                    Some(json!({"Policy": policy})),
                )
                .await;
            assert_eq!(
                put.status, 200,
                "case {label:?}: the policy must be accepted, got {} with body {}",
                put.status, put.body
            );
        }
        let answer = emulator
            .send("GET", &egress_path, None, &egress_host, false, &[])
            .await;
        assert_eq!(
            answer.status, *expected,
            "case {label:?}: expected HTTP {expected} for an anonymous manifest fetch, got {} \
             with body {}",
            answer.status, answer.body
        );
        if *expected == 403 {
            assert!(
                answer.body.to_lowercase().contains("access denied"),
                "case {label:?}: the refusal must say access denied, got {}",
                answer.body
            );
        }
    }

    // A `HEAD` of the same object is authorized against its own action, so a policy that
    // grants only the `GET` one does not reach it, and the policy that grants the `HEAD`
    // action does.
    let head_cases: &[(&str, String, u16)] = &[
        (
            "a HEAD under a GetObject-only grant",
            grant_policy("mediapackagev2:GetObject"),
            403,
        ),
        (
            "a HEAD under a GetHeadObject grant",
            grant_policy("mediapackagev2:GetHeadObject"),
            404,
        ),
    ];
    for (label, policy, expected) in head_cases {
        let put = emulator
            .call(
                "POST",
                "/channelGroup/dg/channel/dc/originEndpoint/de/policy",
                Some(json!({"Policy": policy})),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: the policy must be accepted, got {} with body {}",
            put.status, put.body
        );
        let answer = emulator
            .send("HEAD", &egress_path, None, &egress_host, false, &[])
            .await;
        assert_eq!(
            answer.status, *expected,
            "case {label:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
    }
}

/// Split a published `https://host/path` URL into its host and its path.
///
/// `pub(crate)`: the one splitter this crate's data-plane suites share.
pub(crate) fn split_url(url: &str) -> (String, String) {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    match rest.find('/') {
        Some(path_start) => (
            rest[..path_start].to_string(),
            rest[path_start..].to_string(),
        ),
        None => (rest.to_string(), "/".to_string()),
    }
}
