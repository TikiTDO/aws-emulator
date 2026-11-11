// SPDX-License-Identifier: CC0-1.0

//! What `GetRevocationStatus` reports, end to end over HTTP.
//!
//! Every case goes through the router, so it exercises routing, query-string
//! deserialization, the handler, the records the control plane persisted and
//! serialization — the path a client takes. The rules under test:
//!
//! * the answer lists, of the entities the request names, exactly those this
//!   endpoint holds a revocation for that leaves a signature made at the
//!   request's `signatureTimestamp` untrusted — each reported by the identifier
//!   the REQUEST carried for it, in the order the input names them — and the
//!   list is present and empty when none of them is revoked;
//! * a signing profile's revocation takes effect at the `effectiveTime` the
//!   revoking call named: a signature made at or after that instant is revoked,
//!   one made before it is not, and the revocation covers every version the
//!   profile has had;
//! * a signing job's signature, once `RevokeSignature` has revoked it, is
//!   untrusted whenever it was made — its record names no effective instant;
//! * no certificate hash is ever reported: this service revokes profiles and
//!   signatures and models no revocation of a certificate;
//! * an entity this endpoint holds no record for — another region's resource,
//!   another account's ARN, an id nothing was created under, a profile version
//!   the profile does not hold — is not revoked, while a value that is not a
//!   well-formed ARN of the resource its own member names, or a `platformId`
//!   outside the published catalog, is the modelled 400;
//! * asking changes nothing: a second call answers the same list, and the
//!   profile and job the question named read back unchanged;
//! * a request that leaves a required query parameter out is the modelled 400
//!   naming that member, not a 501.
//!
//! The cases are tables: a row per input class the rules generalise over (which
//! entity is revoked, how many of them, where the signature's timestamp falls
//! relative to the effective instant — including on it and one millisecond
//! either side, how many certificate hashes the request carries, which region
//! answers, and how a value misses).

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

/// The region these requests are signed for unless a case names another one.
const REGION: &str = "us-west-2";

/// Another region's endpoint, for the cases about which region holds a resource.
const OTHER_REGION: &str = "eu-west-1";

/// The status a revoking call answers with: its modelled output is
/// `smithy.api#Unit`, so the reply has nothing to report and is `204 No Content`.
const NO_CONTENT_STATUS: u16 = 204;

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handlers
/// fails.
const ACCOUNT_ID: &str = "000000000000";

/// An account id that is not [`ACCOUNT_ID`], for the ARN that names another
/// account's resource.
const OTHER_ACCOUNT_ID: &str = "210987654321";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// every published platform, including the ones that require it.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The epoch second every table hangs its offsets off.
const BASE_EPOCH_SECS: i64 = 1_700_000_000;

/// A SigV4 credential scope naming this service for `region`. It is what routes a
/// request to signer, and its region is the one whose resources are addressed.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
///
/// `GET /revocations` is claimed by this service's own `can_handle` (it is signed
/// for the `signer` signing name, which this service serves whole), so every case
/// below reaches the handler the way a client does — through the same `dispatch`
/// the production front door uses.
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

    /// The modelled `code` member, or the empty string when the body carries none.
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

async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{region}.amazonaws.com"))
        .header("authorization", signed_for(region));
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

/// `BASE_EPOCH_SECS` shifted by `millis`, which may be negative.
fn at_offset_millis(millis: i64) -> DateTime {
    let total_millis = BASE_EPOCH_SECS * 1_000 + millis;
    let subsec_millis =
        u32::try_from(total_millis.rem_euclid(1_000)).expect("a millisecond of a second fits");
    DateTime::from_secs_and_nanos(total_millis.div_euclid(1_000), subsec_millis * 1_000_000)
}

/// A `signatureTimestamp` query value: the ISO 8601 (`date-time`) rendering a
/// `@httpQuery`-bound timestamp carries by default.
fn timestamp_param(at: DateTime) -> String {
    at.fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time")
}

/// A certificate hash as `GetRevocationStatusRequest$certificateHashes`
/// documents it (`smithy_query(query="doc",shape="GetRevocationStatusRequest",
/// member="certificateHashes")`): the hex SHA-384 of a subject certificate's TBS
/// bytes followed by the hex SHA-384 of its parent CA's.
fn certificate_hash(child: &str, parent: &str) -> String {
    use sha2::Digest;
    format!(
        "{}{}",
        hex::encode(sha2::Sha384::digest(child.as_bytes())),
        hex::encode(sha2::Sha384::digest(parent.as_bytes()))
    )
}

/// The signing-job ARN of `job_id` in `region`, held by `account`.
///
/// Signer names a signing job by `jobId` everywhere else, so the ARN form is
/// written out here — `arn:aws:signer:<region>:<account>:/signing-jobs/<jobId>`,
/// the resource form its profile ARNs carry — rather than read back off a
/// response.
fn job_arn_for(region: &str, account: &str, job_id: &str) -> String {
    format!("arn:aws:signer:{region}:{account}:/signing-jobs/{job_id}")
}

async fn put_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    platform_id: &str,
) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": platform_id,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await
}

async fn start_job(router: &MultiServiceRouter, region: &str, profile_name: &str) -> Answer {
    call(
        router,
        region,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst" } },
            "profileName": profile_name,
            "clientRequestToken": format!("tok-{region}-{profile_name}"),
        })),
    )
    .await
}

async fn describe_job(router: &MultiServiceRouter, region: &str, job_id: &str) -> Answer {
    call(router, region, "GET", &format!("/signing-jobs/{job_id}"), None).await
}

async fn get_profile(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await
}

async fn revoke_signature(router: &MultiServiceRouter, region: &str, job_id: &str) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-jobs/{job_id}/revoke"),
        Some(json!({ "reason": "the signing key was compromised" })),
    )
    .await
}

async fn revoke_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    version: &str,
    effective_epoch_secs: i64,
) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": version,
            "reason": "the profile's material is no longer trusted",
            "effectiveTime": effective_epoch_secs,
        })),
    )
    .await
}

/// One `GetRevocationStatus` request, as the query string the model binds.
async fn get_revocation_status(
    router: &MultiServiceRouter,
    region: &str,
    signature_timestamp: DateTime,
    platform_id: &str,
    profile_version_arn: &str,
    job_arn: &str,
    certificate_hashes: &[String],
) -> Answer {
    // Values go onto the query string as written: a query carries `:` and `/`
    // unescaped, which is how an SDK sends an ARN and an ISO 8601 instant.
    let mut params = vec![
        (
            "signatureTimestamp",
            timestamp_param(signature_timestamp),
        ),
        ("platformId", platform_id.to_string()),
        ("profileVersionArn", profile_version_arn.to_string()),
        ("jobArn", job_arn.to_string()),
    ];
    for hash in certificate_hashes {
        params.push(("certificateHashes", hash.clone()));
    }
    get_revocations(router, region, &params).await
}

/// `GET /revocations` with exactly the query parameters given, so a case can leave
/// one out.
async fn get_revocations(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(&str, String)],
) -> Answer {
    let query = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let uri = if query.is_empty() {
        "/revocations".to_string()
    } else {
        format!("/revocations?{query}")
    };
    call(router, region, "GET", &uri, None).await
}

/// The `revokedEntities` of a 200 answer.
fn revoked_entities(answer: &Answer, case: &str) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    let list = answer.body["revokedEntities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a revokedEntities list, got {:?}",
                answer.body
            )
        });
    list.iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected every revoked entity to be a string, got {:?}",
                        answer.body
                    )
                })
                .to_string()
        })
        .collect()
}

/// The published platforms a SIGNING JOB can be started from whose signatures can
/// then be revoked: the catalog entries that sign through the code-signing-job
/// workflow (`StartSigningJob`, as opposed to the container-image workflow's
/// `SignPayload`) and publish `revocationSupported`.
///
/// Read off [`SIGNING_PLATFORM_CATALOG`] rather than written out, so a catalog
/// change carries the seeds with it instead of leaving a row seeding a job on a
/// platform that signs no job.
fn job_signing_platforms() -> Vec<&'static str> {
    SIGNING_PLATFORM_CATALOG
        .iter()
        .filter(|entry| {
            entry.workflow == SigningWorkflow::CodeSigningJob && entry.revocation_supported
        })
        .map(|entry| entry.platform_id)
        .collect()
}

/// The `index`th platform of [`job_signing_platforms`], cycling, so the tables
/// span as many of them as the catalog publishes.
fn job_signing_platform(index: usize) -> &'static str {
    let platforms = job_signing_platforms();
    assert!(
        !platforms.is_empty(),
        "the published catalog must carry a code-signing-job platform whose signatures can be \
         revoked"
    );
    platforms[index % platforms.len()]
}

/// One seeded signing profile and the job started from it.
struct Seeded {
    profile_name: String,
    platform_id: String,
    profile_version: String,
    profile_version_arn: String,
    job_id: String,
    job_arn: String,
}

/// Seed a profile on `platform_id` in `region` and a signing job started from
/// it, reading the version and the ARNs back off the service's own answers.
async fn seed(
    router: &MultiServiceRouter,
    region: &str,
    profile_name: &str,
    platform_id: &str,
) -> Seeded {
    let put = put_profile(router, region, profile_name, platform_id).await;
    assert_eq!(
        put.status, 200,
        "seeding profile {profile_name} on {platform_id}: got {} {:?}",
        put.status, put.body
    );
    let started = start_job(router, region, profile_name).await;
    assert_eq!(
        started.status, 200,
        "seeding a job from {profile_name}: got {} {:?}",
        started.status, started.body
    );
    let job_id = started.str_member("jobId");
    assert!(
        !job_id.is_empty(),
        "seeding a job from {profile_name}: expected a jobId, got {:?}",
        started.body
    );
    Seeded {
        profile_name: profile_name.to_string(),
        platform_id: platform_id.to_string(),
        profile_version: put.str_member("profileVersion"),
        profile_version_arn: put.str_member("profileVersionArn"),
        job_arn: job_arn_for(region, ACCOUNT_ID, &job_id),
        job_id,
    }
}

/// The certificate-hash lists the tables carry: one hash, several, and a pair
/// whose members differ in length, so a rule that reported hashes would show up
/// whatever the request carries.
fn hash_lists() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("one hash", vec![certificate_hash("child", "root")]),
        (
            "three hashes",
            vec![
                certificate_hash("child", "root"),
                certificate_hash("intermediate", "root"),
                certificate_hash("other", "other-root"),
            ],
        ),
        (
            "hashes of differing length",
            vec![
                certificate_hash("child", "root"),
                "a1b2c3".to_string(),
                "0".repeat(2048),
            ],
        ),
    ]
}

/// Which of the entities a request names are reported revoked: exactly the ones
/// the control plane holds a revocation for, by the identifier the request
/// carried, in input order — and never a certificate hash.
#[tokio::test]
async fn only_the_entities_this_endpoint_revoked_are_reported() {
    let router = emulator();

    let clean = seed(&router, REGION, "Clean_Profile", job_signing_platform(0)).await;
    let job_revoked = seed(
        &router,
        REGION,
        "Job_Revoked_Profile",
        job_signing_platform(1),
    )
    .await;
    let profile_revoked = seed(
        &router,
        REGION,
        "Profile_Revoked_Profile",
        job_signing_platform(2),
    )
    .await;
    let both_revoked = seed(
        &router,
        REGION,
        "Both_Revoked_Profile",
        job_signing_platform(3),
    )
    .await;

    for seeded in [&job_revoked, &both_revoked] {
        let revoked = revoke_signature(&router, REGION, &seeded.job_id).await;
        assert_eq!(
            revoked.status, NO_CONTENT_STATUS,
            "revoking the signature of {}: got {} {:?}",
            seeded.profile_name, revoked.status, revoked.body
        );
    }
    for seeded in [&profile_revoked, &both_revoked] {
        let revoked = revoke_profile(
            &router,
            REGION,
            &seeded.profile_name,
            &seeded.profile_version,
            BASE_EPOCH_SECS,
        )
        .await;
        assert_eq!(
            revoked.status, 204,
            "revoking profile {}: got {} {:?}",
            seeded.profile_name, revoked.status, revoked.body
        );
    }

    // (case, the profile version the request names, the job it names, what is
    // expected back). The profile and the job are named INDEPENDENTLY, so a row
    // can ask about a revoked job under an unrevoked profile version and the
    // other way round.
    let rows: Vec<(&str, &Seeded, &Seeded, Vec<&String>)> = vec![
        ("nothing revoked", &clean, &clean, vec![]),
        (
            "the job's signature revoked",
            &clean,
            &job_revoked,
            vec![&job_revoked.job_arn],
        ),
        (
            "the profile version revoked",
            &profile_revoked,
            &clean,
            vec![&profile_revoked.profile_version_arn],
        ),
        (
            "both revoked, reported in input order",
            &both_revoked,
            &both_revoked,
            vec![&both_revoked.profile_version_arn, &both_revoked.job_arn],
        ),
        (
            "a revoked profile version with an unrevoked job",
            &profile_revoked,
            &clean,
            vec![&profile_revoked.profile_version_arn],
        ),
        (
            "a revoked job with an unrevoked profile version",
            &clean,
            &both_revoked,
            vec![&both_revoked.job_arn],
        ),
    ];

    for (case, profile, job, expected) in rows {
        for (hashes_case, hashes) in hash_lists() {
            let case = format!("{case}, {hashes_case}");
            let answer = get_revocation_status(
                &router,
                REGION,
                at_offset_millis(0),
                &job.platform_id,
                &profile.profile_version_arn,
                &job.job_arn,
                &hashes,
            )
            .await;
            let reported = revoked_entities(&answer, &case);
            let expected: Vec<String> = expected.iter().map(|arn| (*arn).clone()).collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
            for hash in &hashes {
                assert!(
                    !reported.contains(hash),
                    "case {case:?}: no certificate hash can be revoked, yet {hash:?} was \
                     reported in {reported:?}"
                );
            }
        }
    }
}

/// A profile's revocation takes effect at the `effectiveTime` the revoking call
/// named: a signature made at or after that instant is revoked, one made before
/// it is not — and a signature's own revocation names no such instant, so a
/// revoked job is revoked at every timestamp.
#[tokio::test]
async fn a_profile_revocation_applies_from_its_effective_instant() {
    let router = emulator();
    let seeded = seed(
        &router,
        REGION,
        "Effective_Time_Profile",
        job_signing_platform(0),
    )
    .await;

    let revoked = revoke_profile(
        &router,
        REGION,
        &seeded.profile_name,
        &seeded.profile_version,
        BASE_EPOCH_SECS,
    )
    .await;
    assert_eq!(
        revoked.status, 204,
        "revoking the profile: got {} {:?}",
        revoked.status, revoked.body
    );

    // (case, the signature's offset from the effective instant in milliseconds,
    // whether the profile version is revoked for it).
    let rows: Vec<(&str, i64, bool)> = vec![
        ("a day before the effective instant", -86_400_000, false),
        ("a second before it", -1_000, false),
        ("a millisecond before it", -1, false),
        ("exactly on it", 0, true),
        ("a millisecond after it", 1, true),
        ("a second after it", 1_000, true),
        ("a day after it", 86_400_000, true),
    ];

    let hashes = vec![certificate_hash("child", "root")];
    for (case, offset_millis, expected_revoked) in rows {
        let answer = get_revocation_status(
            &router,
            REGION,
            at_offset_millis(offset_millis),
            &seeded.platform_id,
            &seeded.profile_version_arn,
            &seeded.job_arn,
            &hashes,
        )
        .await;
        let reported = revoked_entities(&answer, case);
        let expected: Vec<String> = if expected_revoked {
            vec![seeded.profile_version_arn.clone()]
        } else {
            vec![]
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // The job's signature, once revoked, is revoked at every one of those
    // timestamps: its record names when trust ended, not from when.
    let revoked = revoke_signature(&router, REGION, &seeded.job_id).await;
    assert_eq!(
        revoked.status, NO_CONTENT_STATUS,
        "revoking the signature: got {} {:?}",
        revoked.status, revoked.body
    );
    for offset_millis in [-86_400_000, -1, 0, 1, 86_400_000] {
        let case = format!("a revoked signature at offset {offset_millis}ms");
        let answer = get_revocation_status(
            &router,
            REGION,
            at_offset_millis(offset_millis),
            &seeded.platform_id,
            &seeded.profile_version_arn,
            &seeded.job_arn,
            &hashes,
        )
        .await;
        let reported = revoked_entities(&answer, &case);
        assert!(
            reported.contains(&seeded.job_arn),
            "case {case:?}: expected the revoked signature's job ARN in {reported:?}"
        );
    }
}

/// A profile's revocation covers the version it holds, and no version it does
/// not — and the records the answer is read from are the ones the profile and the
/// job report.
#[tokio::test]
async fn every_version_of_a_revoked_profile_is_revoked() {
    let router = emulator();
    let platform_id = job_signing_platform(0);
    let seeded = seed(&router, REGION, "Versioned_Profile", platform_id).await;

    let revoked = revoke_profile(
        &router,
        REGION,
        &seeded.profile_name,
        &seeded.profile_version,
        BASE_EPOCH_SECS,
    )
    .await;
    assert_eq!(
        revoked.status, 204,
        "revoking the profile: got {} {:?}",
        revoked.status, revoked.body
    );

    // The profile reports the revocation that the answers below are read from.
    let profile = get_profile(&router, REGION, &seeded.profile_name).await;
    assert_eq!(
        profile.str_member("status"),
        "Revoked",
        "expected the revoked profile to report Revoked, got {:?}",
        profile.body["status"]
    );
    assert!(
        !profile.body["revocationRecord"].is_null(),
        "expected a revocationRecord on the revoked profile, got {:?}",
        profile.body
    );

    // (case, the profile version ARN the request names, whether it is revoked)
    let unheld_version_arn = format!(
        "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{}/ZZZZZZZZZZ",
        seeded.profile_name
    );
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "the version the revoke named",
            &seeded.profile_version_arn,
            true,
        ),
        (
            "a version the profile does not hold",
            &unheld_version_arn,
            false,
        ),
    ];

    let hashes = vec![certificate_hash("child", "root")];
    for (case, profile_version_arn, expected_revoked) in rows {
        let answer = get_revocation_status(
            &router,
            REGION,
            at_offset_millis(0),
            platform_id,
            profile_version_arn,
            &seeded.job_arn,
            &hashes,
        )
        .await;
        let reported = revoked_entities(&answer, case);
        let expected: Vec<String> = if expected_revoked {
            vec![profile_version_arn.to_string()]
        } else {
            vec![]
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// An entity this endpoint holds no record for is not revoked — and asking is
/// answered, not refused.
#[tokio::test]
async fn entities_this_endpoint_holds_no_record_for_are_not_revoked() {
    let router = emulator();
    let here = seed(&router, REGION, "Local_Profile", job_signing_platform(0)).await;
    let revoked = revoke_signature(&router, REGION, &here.job_id).await;
    assert_eq!(
        revoked.status, NO_CONTENT_STATUS,
        "revoking the signature: {:?}",
        revoked.body
    );
    let revoked = revoke_profile(
        &router,
        REGION,
        &here.profile_name,
        &here.profile_version,
        BASE_EPOCH_SECS,
    )
    .await;
    assert_eq!(revoked.status, 204, "revoking the profile: {:?}", revoked.body);

    // Another region's endpoint holds a profile of the same name, revoked by
    // nobody, and a job of its own.
    let elsewhere = seed(
        &router,
        OTHER_REGION,
        "Local_Profile",
        job_signing_platform(0),
    )
    .await;

    // (case, the region whose endpoint answers, the profile version ARN, the job ARN)
    let other_account_profile_arn = format!(
        "arn:aws:signer:{REGION}:{OTHER_ACCOUNT_ID}:/signing-profiles/{}/{}",
        here.profile_name, here.profile_version
    );
    let unknown_profile_arn = format!(
        "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/No_Such_Profile/{}",
        here.profile_version
    );
    let unknown_job_arn = job_arn_for(REGION, ACCOUNT_ID, "00000000-0000-0000-0000-000000000000");
    let other_account_job_arn = job_arn_for(REGION, OTHER_ACCOUNT_ID, &here.job_id);
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a profile name nothing was created under",
            REGION,
            &unknown_profile_arn,
            &unknown_job_arn,
        ),
        (
            "ARNs naming another account's resources",
            REGION,
            &other_account_profile_arn,
            &other_account_job_arn,
        ),
        (
            "ARNs naming another region's resources",
            REGION,
            &elsewhere.profile_version_arn,
            &elsewhere.job_arn,
        ),
        (
            "another region's endpoint answering about this region's resources",
            OTHER_REGION,
            &here.profile_version_arn,
            &here.job_arn,
        ),
    ];

    let hashes = vec![certificate_hash("child", "root")];
    for (case, region, profile_version_arn, job_arn) in rows {
        let answer = get_revocation_status(
            &router,
            region,
            at_offset_millis(0),
            &here.platform_id,
            profile_version_arn,
            job_arn,
            &hashes,
        )
        .await;
        let reported = revoked_entities(&answer, case);
        assert!(
            reported.is_empty(),
            "case {case:?}: expected nothing revoked, got {reported:?}"
        );
    }
}

/// A value that is not a well-formed ARN of the resource ITS MEMBER names, and a
/// `platformId` outside the published catalog, are the modelled 400 — naming the
/// value that was refused.
#[tokio::test]
async fn values_naming_no_signer_resource_are_refused() {
    let router = emulator();
    let seeded = seed(&router, REGION, "Refusal_Profile", job_signing_platform(0)).await;
    let hashes = vec![certificate_hash("child", "root")];

    // (case, platformId, profileVersionArn, jobArn, the value the refusal names).
    // Every value here satisfies `Arn`'s modelled @length(20, 2048), so each one
    // reaches the handler.
    let other_service_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:not-signer");
    let regionless_job_arn =
        format!("arn:aws:signer::{ACCOUNT_ID}:/signing-jobs/{}", seeded.job_id);
    let accountless_profile_arn = format!(
        "arn:aws:signer:{REGION}::/signing-profiles/{}/{}",
        seeded.profile_name, seeded.profile_version
    );
    let partitionless_job_arn = format!(
        "arn::signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/{}",
        seeded.job_id
    );
    let not_an_arn = "x".repeat(32);
    // A Signer ARN whose resource portion names no resource of this service at
    // all, and ARNs of the WRONG KIND for the member that carries them: a
    // `profileVersionArn` is the ARN of a signing profile VERSION and a `jobArn`
    // the ARN of a signing job, so neither admits the other's form.
    let shapeless_signer_arn =
        format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:not-a-real-arn-shape");
    let profile_arn_without_version = format!(
        "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{}",
        seeded.profile_name
    );
    let empty_version_arn = format!(
        "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{}/",
        seeded.profile_name
    );
    let rows: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "a profileVersionArn of another service",
            &seeded.platform_id,
            &other_service_arn,
            &seeded.job_arn,
            &other_service_arn,
        ),
        (
            "a profileVersionArn carrying no account",
            &seeded.platform_id,
            &accountless_profile_arn,
            &seeded.job_arn,
            &accountless_profile_arn,
        ),
        (
            "a jobArn that is not an ARN at all",
            &seeded.platform_id,
            &seeded.profile_version_arn,
            &not_an_arn,
            &not_an_arn,
        ),
        (
            "a jobArn carrying no region",
            &seeded.platform_id,
            &seeded.profile_version_arn,
            &regionless_job_arn,
            &regionless_job_arn,
        ),
        (
            "a jobArn carrying no partition",
            &seeded.platform_id,
            &seeded.profile_version_arn,
            &partitionless_job_arn,
            &partitionless_job_arn,
        ),
        (
            "a platformId outside the published catalog",
            "NoSuchPlatform-SHA256-ECDSA",
            &seeded.profile_version_arn,
            &seeded.job_arn,
            "NoSuchPlatform-SHA256-ECDSA",
        ),
        (
            "a profileVersionArn whose resource names nothing of this service",
            &seeded.platform_id,
            &shapeless_signer_arn,
            &seeded.job_arn,
            &shapeless_signer_arn,
        ),
        (
            "a jobArn whose resource names nothing of this service",
            &seeded.platform_id,
            &seeded.profile_version_arn,
            &shapeless_signer_arn,
            &shapeless_signer_arn,
        ),
        (
            "a profileVersionArn carrying no version",
            &seeded.platform_id,
            &profile_arn_without_version,
            &seeded.job_arn,
            &profile_arn_without_version,
        ),
        (
            "a profileVersionArn whose version segment is empty",
            &seeded.platform_id,
            &empty_version_arn,
            &seeded.job_arn,
            &empty_version_arn,
        ),
        (
            "a jobArn carrying a signing profile's ARN",
            &seeded.platform_id,
            &seeded.profile_version_arn,
            &seeded.profile_version_arn,
            &seeded.profile_version_arn,
        ),
        (
            "a profileVersionArn carrying a signing job's ARN",
            &seeded.platform_id,
            &seeded.job_arn,
            &seeded.job_arn,
            &seeded.job_arn,
        ),
    ];

    for (case, platform_id, profile_version_arn, job_arn, named) in rows {
        let answer = get_revocation_status(
            &router,
            REGION,
            at_offset_millis(0),
            platform_id,
            profile_version_arn,
            job_arn,
            &hashes,
        )
        .await;
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
            answer.body["code"]
        );
        assert!(
            answer.message().contains(named),
            "case {case:?}: expected the refusal to name {named:?}, got {:?}",
            answer.body["message"]
        );
    }

    // Every PUBLISHED platform is accepted, whatever it publishes about
    // revocation support: the question is about records, not about capability.
    for entry in SIGNING_PLATFORM_CATALOG {
        let answer = get_revocation_status(
            &router,
            REGION,
            at_offset_millis(0),
            entry.platform_id,
            &seeded.profile_version_arn,
            &seeded.job_arn,
            &hashes,
        )
        .await;
        let reported = revoked_entities(&answer, entry.platform_id);
        assert!(
            reported.is_empty(),
            "case {:?}: nothing is revoked, got {reported:?}",
            entry.platform_id
        );
    }
}

/// A `GET /revocations` that leaves a required query parameter out is the modelled
/// 400 naming the member it left out — never a 501, which would say nobody
/// implemented the operation the request addressed.
///
/// One row per `@httpQuery`-bound member omitted on its own, plus the request with
/// none of them and the complete one.
#[tokio::test]
async fn a_request_missing_a_required_query_parameter_is_refused() {
    let router = emulator();
    let seeded = seed(&router, REGION, "Query_Binding_Profile", job_signing_platform(0)).await;

    let complete: Vec<(&str, String)> = vec![
        ("signatureTimestamp", timestamp_param(at_offset_millis(0))),
        ("platformId", seeded.platform_id.clone()),
        ("profileVersionArn", seeded.profile_version_arn.clone()),
        ("jobArn", seeded.job_arn.clone()),
        ("certificateHashes", certificate_hash("child", "root")),
    ];

    // The complete request is served, so each row below differs from a served
    // request only by the member it omits.
    let served = get_revocations(&router, REGION, &complete).await;
    let reported = revoked_entities(&served, "the complete request");
    assert!(
        reported.is_empty(),
        "case \"the complete request\": nothing is revoked, got {reported:?}"
    );

    for (index, (omitted, _)) in complete.iter().enumerate() {
        let params: Vec<(&str, String)> = complete
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != index)
            .map(|(_, (name, value))| (*name, value.clone()))
            .collect();
        let answer = get_revocations(&router, REGION, &params).await;
        assert_eq!(
            answer.status, 400,
            "case {omitted:?} omitted: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains("ValidationException"),
            "case {omitted:?} omitted: expected a ValidationException, got {:?}",
            answer.body
        );
        assert!(
            answer.message().contains(omitted),
            "case {omitted:?} omitted: expected the refusal to name the absent member, got {:?}",
            answer.body["message"]
        );
    }

    let none = get_revocations(&router, REGION, &[]).await;
    assert_eq!(
        none.status, 400,
        "case \"no parameters at all\": expected HTTP 400, got {} {:?}",
        none.status, none.body
    );
    assert!(
        none.shape().contains("ValidationException"),
        "case \"no parameters at all\": expected a ValidationException, got {:?}",
        none.body
    );
}

/// Asking is a report: a second call answers the same list, and the profile and
/// the job the question named read back unchanged.
#[tokio::test]
async fn asking_for_the_revocation_status_changes_nothing() {
    let router = emulator();
    let seeded = seed(&router, REGION, "Stable_Profile", job_signing_platform(0)).await;
    let revoked = revoke_signature(&router, REGION, &seeded.job_id).await;
    assert_eq!(
        revoked.status, NO_CONTENT_STATUS,
        "revoking the signature: {:?}",
        revoked.body
    );

    let hashes = vec![certificate_hash("child", "root")];
    let job_before = describe_job(&router, REGION, &seeded.job_id).await;
    let profile_before = get_profile(&router, REGION, &seeded.profile_name).await;

    let first = get_revocation_status(
        &router,
        REGION,
        at_offset_millis(0),
        &seeded.platform_id,
        &seeded.profile_version_arn,
        &seeded.job_arn,
        &hashes,
    )
    .await;
    let second = get_revocation_status(
        &router,
        REGION,
        at_offset_millis(0),
        &seeded.platform_id,
        &seeded.profile_version_arn,
        &seeded.job_arn,
        &hashes,
    )
    .await;
    assert_eq!(
        revoked_entities(&first, "first read"),
        vec![seeded.job_arn.clone()],
        "expected the revoked signature to be reported, got {:?}",
        first.body
    );
    assert_eq!(
        first.body, second.body,
        "expected two reads to answer alike, got {:?} then {:?}",
        first.body, second.body
    );

    let job_after = describe_job(&router, REGION, &seeded.job_id).await;
    let profile_after = get_profile(&router, REGION, &seeded.profile_name).await;
    assert_eq!(
        job_after.body["revocationRecord"], job_before.body["revocationRecord"],
        "expected the job's revocation record to be untouched by the read, got {:?} then {:?}",
        job_before.body["revocationRecord"], job_after.body["revocationRecord"]
    );
    assert_eq!(
        profile_after.str_member("status"),
        profile_before.str_member("status"),
        "expected the profile's status to be untouched by the read, got {:?} then {:?}",
        profile_before.body["status"],
        profile_after.body["status"]
    );
}
