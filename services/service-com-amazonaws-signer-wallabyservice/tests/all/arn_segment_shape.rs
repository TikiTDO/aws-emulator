// SPDX-License-Identifier: CC0-1.0

//! What this service judges about a resource identifier it reads out of an
//! `Arn`-typed request value, driven end to end over HTTP.
//!
//! The rule under test: a value whose modelled shape is `Arn` is judged on the
//! ARN's SHAPE alone — the fields a Signer ARN carries (`arn`, a partition, the
//! `signer` service, a region, an account) plus the resource form of the kind of
//! resource the member names, each segment present and carrying no further
//! `/`-separated part — and never against the length or the character class the
//! model places on the MEMBER that carries the same identifier directly
//! (`ProfileName` is `@length(2, 64)` / `^[a-zA-Z0-9_]{2,}$` and `ProfileVersion`
//! is `@length(10, 10)` / `^[a-zA-Z0-9]{10}$`, both read off
//! `smithy_query(query="shape",shape="ProfileName,ProfileVersion,Arn")`; `Arn`
//! itself carries `@length(20, 2048)` and NO pattern). So an ARN-shaped value
//! whose segments those member constraints would refuse is ANSWERED — by
//! `GetRevocationStatus` as an entity this endpoint holds no revocation for, and
//! by the tagging operations as a profile that was never created — while a value
//! that is not ARN-shaped at all stays the refusal its operation words for it.
//!
//! The rows span what the rule generalises over: which segment of the ARN carries
//! the identifier (profile name, profile version, job id), and how it misses the
//! member constraint it is not held to (shorter than the floor, longer than the
//! ceiling, outside the character class, not the minted form) as against how it
//! misses the ARN's own shape (an empty segment, a segment too many, a missing
//! one, a value that is no ARN).

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::{
    SigningWorkflow, SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents, spelled out so a wrong
/// account in the handlers fails.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be seeded on
/// any published platform.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The instant every `signatureTimestamp` below carries.
const SIGNATURE_EPOCH_SECS: i64 = 1_700_000_000;

/// A SigV4 credential scope naming this service: what routes a request to signer.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or the empty string when there is none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body().to_vec();
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

/// `value` percent-encoded for ONE URI segment: `resourceArn` is a non-greedy
/// `@httpLabel` carrying `:` and `/`, so everything outside the unreserved set is
/// escaped, which is what keeps the ARN one segment.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// A `signatureTimestamp` query value: the ISO 8601 rendering an `@httpQuery`-bound
/// timestamp carries by default.
fn signature_timestamp_param() -> String {
    DateTime::from_secs(SIGNATURE_EPOCH_SECS)
        .fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time")
}

/// The published platform the seeds and the tables use: a catalog entry that signs
/// through the code-signing-job workflow and publishes `revocationSupported`, read
/// off [`SIGNING_PLATFORM_CATALOG`] so a catalog change carries the seeds with it.
fn job_signing_platform() -> &'static str {
    SIGNING_PLATFORM_CATALOG
        .iter()
        .find(|entry| {
            entry.workflow == SigningWorkflow::CodeSigningJob && entry.revocation_supported
        })
        .map(|entry| entry.platform_id)
        .expect("the published catalog must carry a revocable code-signing-job platform")
}

/// A signing-profile VERSION ARN of this region and account, built from the two
/// segments the resource form carries.
fn profile_version_arn(name: &str, version: &str) -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{name}/{version}")
}

/// A signing-job ARN of this region and account.
fn job_arn_for(job_id: &str) -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/{job_id}")
}

/// The certificate hash every request carries: `certificateHashes` targets a list
/// of unconstrained strings, so one entry is enough to make the request complete.
fn one_hash() -> Vec<String> {
    vec!["a".repeat(192)]
}

/// One `GetRevocationStatus` request, as the query string the model binds. Values
/// go on as written: a query carries `:` and `/` unescaped, which is how an SDK
/// sends an ARN.
async fn get_revocation_status(
    router: &MultiServiceRouter,
    platform_id: &str,
    profile_version_arn: &str,
    job_arn: &str,
) -> Answer {
    let mut query = vec![
        format!("signatureTimestamp={}", signature_timestamp_param()),
        format!("platformId={platform_id}"),
        format!("profileVersionArn={profile_version_arn}"),
        format!("jobArn={job_arn}"),
    ];
    for hash in one_hash() {
        query.push(format!("certificateHashes={hash}"));
    }
    call(router, "GET", &format!("/revocations?{}", query.join("&")), None).await
}

/// The `revokedEntities` of a 200 answer.
fn revoked_entities(answer: &Answer, case: &str) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["revokedEntities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a revokedEntities list, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect()
}

/// One seeded profile and the signing job started from it.
struct Seeded {
    profile_name: String,
    profile_version: String,
    profile_version_arn: String,
    job_id: String,
    job_arn: String,
}

/// Seed a profile and a job started from it, reading the version and the ARN back
/// off the service's own answers.
async fn seed(router: &MultiServiceRouter, profile_name: &str) -> Seeded {
    let put = call(
        router,
        "PUT",
        &format!("/signing-profiles/{profile_name}"),
        Some(json!({
            "platformId": job_signing_platform(),
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "seeding profile {profile_name}: got {} {:?}",
        put.status, put.body
    );
    let started = call(
        router,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst" } },
            "profileName": profile_name,
            "clientRequestToken": format!("tok-{profile_name}"),
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "seeding a job from {profile_name}: got {} {:?}",
        started.status, started.body
    );
    let job_id = started.str_member("jobId");
    Seeded {
        profile_name: profile_name.to_string(),
        profile_version: put.str_member("profileVersion"),
        profile_version_arn: put.str_member("profileVersionArn"),
        job_arn: job_arn_for(&job_id),
        job_id,
    }
}

/// An ARN-shaped `profileVersionArn` is ANSWERED, whatever its name and version
/// segments carry: the identifiers inside an ARN the caller was handed are not
/// held to the bounds the model places on the members that carry a profile name or
/// a version directly, and nothing this endpoint created under them is revoked.
#[tokio::test]
async fn an_arn_shaped_profile_version_is_answered_whatever_its_segments_carry() {
    let router = emulator();
    let platform_id = job_signing_platform();
    // A job ARN this endpoint holds nothing for, so only the profile version under
    // test varies between rows.
    let unknown_job_arn = job_arn_for("00000000-0000-0000-0000-000000000000");

    // (case, the profile-name segment, the profile-version segment). Each pair is
    // one the `ProfileName` / `ProfileVersion` member constraints would refuse but
    // the ARN's resource form admits.
    let name_65 = "a".repeat(65);
    let version_64 = "v".repeat(64);
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a one-character profile name",
            "n",
            "aaaaaaaaaa",
        ),
        (
            "a profile name at ProfileName's floor, nothing created under it",
            "ab",
            "aaaaaaaaaa",
        ),
        (
            "a profile name one character past ProfileName's ceiling",
            &name_65,
            "aaaaaaaaaa",
        ),
        (
            "a profile name outside ProfileName's character class",
            "has-a-hyphen",
            "aaaaaaaaaa",
        ),
        (
            "a profile name carrying a dot",
            "name.with.dots",
            "aaaaaaaaaa",
        ),
        (
            "a one-character profile version",
            "Some_Profile",
            "v",
        ),
        (
            "a profile version shorter than ProfileVersion's fixed width",
            "Some_Profile",
            "v1",
        ),
        (
            "a profile version longer than ProfileVersion's fixed width",
            "Some_Profile",
            &version_64,
        ),
        (
            "a profile version outside ProfileVersion's character class",
            "Some_Profile",
            "ver-1",
        ),
        (
            "both segments outside their members' constraints",
            "n",
            "v",
        ),
    ];

    for (case, name, version) in rows {
        let arn = profile_version_arn(name, version);
        let answer = get_revocation_status(&router, platform_id, &arn, &unknown_job_arn).await;
        let reported = revoked_entities(&answer, case);
        assert!(
            reported.is_empty(),
            "case {case:?}: expected nothing revoked for {arn:?}, got {reported:?}"
        );
    }
}

/// The same rule on the OTHER ARN member: a `jobArn` whose id segment is not the
/// UUID form this service mints is still ARN-shaped, so it is answered as a job
/// this endpoint holds no revocation for.
#[tokio::test]
async fn an_arn_shaped_job_is_answered_whatever_its_id_segment_carries() {
    let router = emulator();
    let platform_id = job_signing_platform();
    let unknown_profile_version_arn = profile_version_arn("n", "aaaaaaaaaa");

    let id_64 = "7".repeat(64);
    // (case, the job-id segment)
    let rows: Vec<(&str, &str)> = vec![
        ("a one-character job id", "a"),
        ("a job id that is not UUID-shaped", "job-00001"),
        ("a job id of 64 characters", &id_64),
        (
            "a UUID-shaped job id nothing was created under",
            "11111111-2222-3333-4444-555555555555",
        ),
    ];

    for (case, job_id) in rows {
        let arn = job_arn_for(job_id);
        let answer =
            get_revocation_status(&router, platform_id, &unknown_profile_version_arn, &arn).await;
        let reported = revoked_entities(&answer, case);
        assert!(
            reported.is_empty(),
            "case {case:?}: expected nothing revoked for {arn:?}, got {reported:?}"
        );
    }
}

/// A value that is not ARN-shaped is still the modelled 400, worded with the
/// member that carried it and the value as supplied.
#[tokio::test]
async fn a_value_that_is_not_arn_shaped_is_still_refused() {
    let router = emulator();
    let platform_id = job_signing_platform();
    let good_profile_version_arn = profile_version_arn("n", "aaaaaaaaaa");
    let good_job_arn = job_arn_for("11111111-2222-3333-4444-555555555555");

    // (case, the member the request miswrites, the value it carries)
    let not_an_arn = "a".repeat(68);
    let empty_name = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles//aaaaaaaaaa");
    let empty_version = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/Name/");
    let extra_segment =
        format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/Name/aaaaaaaaaa/more");
    let no_version = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/Name");
    let other_service = format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:not-signer");
    let no_region = format!("arn:aws:signer::{ACCOUNT_ID}:/signing-profiles/Name/aaaaaaaaaa");
    let empty_job_id = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/");
    let job_extra_segment =
        format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/one/two");
    let job_not_an_arn = "x".repeat(32);
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a profileVersionArn that is not an ARN at all",
            "profileVersionArn",
            &not_an_arn,
        ),
        (
            "a profileVersionArn whose name segment is empty",
            "profileVersionArn",
            &empty_name,
        ),
        (
            "a profileVersionArn whose version segment is empty",
            "profileVersionArn",
            &empty_version,
        ),
        (
            "a profileVersionArn carrying a segment past the version",
            "profileVersionArn",
            &extra_segment,
        ),
        (
            "a profileVersionArn carrying no version",
            "profileVersionArn",
            &no_version,
        ),
        (
            "a profileVersionArn of another service",
            "profileVersionArn",
            &other_service,
        ),
        (
            "a profileVersionArn carrying no region",
            "profileVersionArn",
            &no_region,
        ),
        (
            "a jobArn whose id segment is empty",
            "jobArn",
            &empty_job_id,
        ),
        (
            "a jobArn carrying a segment past the id",
            "jobArn",
            &job_extra_segment,
        ),
        ("a jobArn that is not an ARN at all", "jobArn", &job_not_an_arn),
    ];

    for (case, member, value) in rows {
        let (profile_version, job) = if member == "profileVersionArn" {
            (value, good_job_arn.as_str())
        } else {
            (good_profile_version_arn.as_str(), value)
        };
        let answer = get_revocation_status(&router, platform_id, profile_version, job).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.code(),
            "ValidationException",
            "case {case:?}: expected the modelled code, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            format!("Invalid {member}: {value}"),
            "case {case:?}: expected the refusal to name {member} and {value:?}, got {:?}",
            answer.body
        );
    }
}

/// Relaxing what the ARN's segments may carry does not relax what the ARN has to
/// MATCH: a stored profile version and a stored job are still reported revoked
/// only through the ARN they were stored with, and an ARN-shaped neighbour of it
/// is not.
#[tokio::test]
async fn an_arn_still_has_to_match_the_stored_resource() {
    let router = emulator();
    let platform_id = job_signing_platform();
    let seeded = seed(&router, "Shape_Profile").await;

    let revoked = call(
        &router,
        "PUT",
        &format!("/signing-jobs/{}/revoke", seeded.job_id),
        Some(json!({ "reason": "the signing key was compromised" })),
    )
    .await;
    assert_eq!(
        revoked.status, 204,
        "revoking the signature: {:?}",
        revoked.body
    );
    let revoked = call(
        &router,
        "PUT",
        &format!("/signing-profiles/{}/revoke", seeded.profile_name),
        Some(json!({
            "profileVersion": seeded.profile_version,
            "reason": "the profile's material is no longer trusted",
            "effectiveTime": SIGNATURE_EPOCH_SECS - 1,
        })),
    )
    .await;
    assert_eq!(
        revoked.status, 204,
        "revoking the profile: {:?}",
        revoked.body
    );

    // The ARNs the control plane stored: both entities are reported.
    let answer = get_revocation_status(
        &router,
        platform_id,
        &seeded.profile_version_arn,
        &seeded.job_arn,
    )
    .await;
    let reported = revoked_entities(&answer, "the stored ARNs");
    assert_eq!(
        reported,
        vec![
            seeded.profile_version_arn.clone(),
            seeded.job_arn.clone()
        ],
        "case {:?}: expected both stored entities reported, got {reported:?}",
        "the stored ARNs"
    );

    // (case, the profileVersionArn, the jobArn) — ARN-shaped neighbours of the
    // stored pair, each naming a resource this endpoint holds nothing for.
    let truncated_name = seeded
        .profile_name
        .get(..1)
        .expect("the seeded name carries a first character")
        .to_string();
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the stored profile's name cut to one character",
            profile_version_arn(&truncated_name, &seeded.profile_version),
            job_arn_for("00000000-0000-0000-0000-000000000000"),
        ),
        (
            "the stored version cut to one character",
            profile_version_arn(
                &seeded.profile_name,
                seeded
                    .profile_version
                    .get(..1)
                    .expect("the minted version carries a first character"),
            ),
            job_arn_for("00000000-0000-0000-0000-000000000000"),
        ),
        (
            "the stored job's id cut to one character",
            profile_version_arn("n", "aaaaaaaaaa"),
            job_arn_for(
                seeded
                    .job_id
                    .get(..1)
                    .expect("the minted job id carries a first character"),
            ),
        ),
    ];

    for (case, profile_version, job) in rows {
        let answer = get_revocation_status(&router, platform_id, &profile_version, &job).await;
        let reported = revoked_entities(&answer, case);
        assert!(
            reported.is_empty(),
            "case {case:?}: expected nothing revoked for {profile_version:?} and {job:?}, got \
             {reported:?}"
        );
    }
}

/// The same rule where the other reader of a profile ARN applies it: a tagging
/// `resourceArn` whose name segment the `ProfileName` member would refuse is a
/// MISSING RESOURCE (nothing was created under that name), not a bad request —
/// while a label that is not a profile ARN stays the syntax refusal.
#[tokio::test]
async fn a_tagging_arn_with_a_name_no_profile_could_carry_is_not_found() {
    let router = emulator();
    let seeded = seed(&router, "Tagged_Shape_Profile").await;

    // (case, resourceArn, status, error shape, code)
    let one_character = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/n");
    let hyphenated = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/has-a-hyphen");
    let name_65 = format!(
        "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{}",
        "a".repeat(65)
    );
    let empty_name = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/");
    let version_arn = format!(
        "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{}/{}",
        seeded.profile_name, seeded.profile_version
    );
    let rows: Vec<(&str, &str, u16, &str, &str)> = vec![
        (
            "a one-character profile name",
            &one_character,
            404,
            "NotFoundException",
            "ProfileNotFound",
        ),
        (
            "a profile name outside ProfileName's character class",
            &hyphenated,
            404,
            "NotFoundException",
            "ProfileNotFound",
        ),
        (
            "a profile name past ProfileName's ceiling",
            &name_65,
            404,
            "NotFoundException",
            "ProfileNotFound",
        ),
        (
            "an empty name segment",
            &empty_name,
            400,
            "BadRequestException",
            "BadRequestException",
        ),
        (
            "the version ARN of a profile that does exist",
            &version_arn,
            400,
            "BadRequestException",
            "BadRequestException",
        ),
    ];

    for (case, resource_arn, status, shape, code) in rows {
        let answer = call(&router, "GET", &format!("/tags/{}", encode(resource_arn)), None).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains(shape),
            "case {case:?}: expected a {shape}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.code(),
            code,
            "case {case:?}: expected the modelled code {code}, got {:?}",
            answer.body
        );
        if status == 404 {
            assert_eq!(
                answer.message(),
                format!("profile with arn {resource_arn} does not exists"),
                "case {case:?}: expected the missing-resource prose, got {:?}",
                answer.body
            );
        }
    }
}
