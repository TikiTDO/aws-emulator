// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `connect-campaigns` SigV4 signing name.
//!
//! ConnectCampaigns (2021-01-30) and ConnectCampaignsV2 (2024-04-23) are two API
//! versions of one AWS service, and they SHARE everything the router used to resolve a
//! service by: the `aws.auth#sigv4` name `connect-campaigns`, the `arnNamespace`
//! `connect-campaigns`, and the endpoint host family
//! `connect-campaigns.{Region}.{dnsSuffix}`. A service that claims every request
//! carrying its signing name therefore claims the other version's traffic too: the
//! router finds two claimants and refuses the request before dispatch with a code no
//! AWS model declares, which made BOTH versions unreachable.
//!
//! On AWS the two are disambiguated by their restJson1 routes, which are disjoint:
//! every V2 route is `/v2/`-prefixed, every v1 route is unprefixed. Each crate's
//! `can_handle` therefore adds the framework's path-segment conjunct (the rule
//! `emulator_server::can_handle_based_on_route_key_and_path` implements) over its own
//! generated, model-derived `ROUTE_PATH_SEGMENTS`.
//!
//! This suite pins the split with both versions mounted together: every route family of
//! each version reaches exactly that version — answered from its own store when the
//! operation is implemented, or with its own scaffold `NotImplemented` when it is still
//! a stub — and a path neither version models is refused only because nobody claims it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_connect_amazonconnectservice::state as connect_state;
use smithy_server_runtime::MultiServiceRouter;

/// The Connect instance the V2 rows name. V2 onboarding configures an EXISTING Amazon
/// Connect instance, so the account has to hold one under this id.
const V2_INSTANCE: &str = "instance-1";

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// API versions share. The signature is never checked: the router here runs with
/// request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/connect-campaigns/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A well-formed ARN in this service's namespace, percent-encoded so it occupies the
/// single `{arn}` path label the tagging routes bind.
macro_rules! encoded_arn {
    () => {
        "arn%3Aaws%3Aconnect-campaigns%3Aus-east-1%3A000000000000%3Acampaign%2F000000000000"
    };
}

/// The same, for a campaign id neither version's store holds — so the answer is
/// whichever version claimed the route saying the resource is not there.
macro_rules! encoded_missing_arn {
    () => {
        "arn%3Aaws%3Aconnect-campaigns%3Aus-east-1%3A000000000000%3Acampaign%2F999999999999"
    };
}

/// A V2 `CreateCampaign` body: `channelSubtypeConfig` is a member only the V2 model
/// has, and v1's own create requires `dialerConfig`/`outboundCallConfig` instead.
const V2_CREATE: &str = r#"{"name":"v2-campaign","connectInstanceId":"instance-1",
    "channelSubtypeConfig":{"telephony":{"outboundMode":{"agentless":{}},
    "defaultOutboundConfig":{"connectContactFlowId":"flow-1"}}}}"#;

/// A v1 `CreateCampaign` body, with the two members v1 requires.
const V1_CREATE: &str = r#"{"name":"v1-campaign","connectInstanceId":"instance-1",
    "dialerConfig":{"progressiveDialerConfig":{"bandwidthAllocation":0.5}},
    "outboundCallConfig":{"connectContactFlowId":"flow-1"}}"#;

fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    register_connect_instance(&engine, V2_INSTANCE);
    router
}

/// Give the account the Connect instance the V2 rows name, in Amazon Connect's own
/// instance registry inside the shared engine — the registry V2
/// `StartInstanceOnboardingJob` resolves a `connectInstanceId` against.
///
/// Written into Connect's store rather than created over `CreateInstance`, because that
/// call mints the id it stores while these route rows name a fixed one. The write is the
/// registration seam the Connect crate exposes, the one statement of the record a
/// registration writes — the record that call writes, already past its clock-owed
/// activation instant.
fn register_connect_instance(engine: &engine::InMemoryEngine, instance: &str) {
    assert!(
        connect_state::register_instance(engine, "us-east-1", instance),
        "instance {instance:?}: Amazon Connect's registry took no record"
    );
}

/// Onboard the Connect instance the V2 create bodies name, and wait out the span the
/// clock owes the onboarding job.
///
/// V2 `CreateCampaign` mints a campaign only for an instance onboarded to Campaigns,
/// and the job it starts settles one `ONBOARDING_JOB_DURATION` later — so a V2 create
/// in this suite is preceded by this, exactly as a client would.
async fn onboard_v2_instance(router: &MultiServiceRouter) {
    let (status, _, payload) = dispatch(
        router,
        "PUT",
        "/v2/connect-instance/instance-1/onboarding",
        r#"{"encryptionConfig":{"enabled":false}}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "V2 StartInstanceOnboardingJob: expected 200, got {status} {payload}"
    );
    tokio::time::sleep(
        service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ONBOARDING_JOB_DURATION
            + std::time::Duration::from_millis(100),
    )
    .await;
}

async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &str,
) -> (u16, Option<String>, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://connect-campaigns.us-east-1.amazonaws.com{uri}"))
        .header("host", "connect-campaigns.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_owned()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    // The error code, read from the body's `__type` discriminator: a status alone
    // cannot tell the router's own refusals — `UnknownService` (nobody claimed the
    // request) and `AmbiguousService` (several did) — from a service's own modeled
    // error.
    let error_code = payload
        .get("__type")
        .or_else(|| payload.get("Code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, error_code, payload)
}

/// What a row expects of the version that claimed it: an error code only that version's
/// own answer carries, a member only its output has, or — for a batch read, whose two
/// output members both versions name alike — the per-id report the claiming version's
/// own store produces for one requested id.
#[derive(Debug)]
enum Expect {
    Error(&'static str),
    Member(&'static str),
    /// The requested campaign id comes back as the ONLY entry of `failedRequests`,
    /// carrying the not-found failure code, with `successfulRequests` empty: the answer
    /// of a store that does not hold that id.
    BatchMissing(&'static str),
    /// An onboarding job the claiming version's own store has only just started, so it
    /// reports a job that has not finished yet. The V2 version onboarded `instance-1` in
    /// ITS store before these rows ran and waited that job out, so a V2 answer to this
    /// path would report a FINISHED job — only the v1 store, which holds no record of
    /// the instance until this row's call writes one, can report one in progress.
    JobJustStarted,
}

/// Neither routing refusal may ever be the answer to a legitimately signed request.
const ROUTER_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

/// The `InstanceOnboardingJobStatusCode` the v1 version reports for an onboarding job
/// that has not finished, read from that crate rather than written out here.
const V1_ONBOARDING_IN_PROGRESS: &str =
    service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::ONBOARDING_STATUS_IN_PROGRESS;

#[tokio::test]
async fn each_connect_campaigns_api_version_serves_its_own_route_space() {
    let router = build_router();
    // The V2 create row below needs its Connect instance onboarded; the onboarding
    // route is a V2 route family in its own right, and its own row asserts that it is
    // the V2 version that answers it.
    onboard_v2_instance(&router).await;

    // One row per modelled route family of each version (sources: the `smithy_query`
    // digests of the two models — 22 v1 operations over `/campaigns`,
    // `/campaigns-state`, `/campaigns-summary`, `/connect-instance/…`, `/tags/{arn}`;
    // 37 V2 operations over the same families under `/v2/`). Asserting WHAT the
    // response carries, not just its status, is what proves exactly one version
    // claimed it: an implemented operation answers from its own store, a stub answers
    // its own `NotImplemented`, and neither router refusal can produce either.
    let rows: [(&str, &str, &str, &str, u16, Expect); 14] = [
        // ---- V2: `/v2/`-prefixed ----
        (
            "V2 CreateCampaign",
            "PUT",
            "/v2/campaigns",
            V2_CREATE,
            200,
            Expect::Member("id"),
        ),
        (
            "V2 ListCampaigns",
            "POST",
            "/v2/campaigns-summary",
            "{}",
            200,
            Expect::Member("campaignSummaryList"),
        ),
        (
            "V2 DescribeCampaign for an id no store holds",
            "GET",
            "/v2/campaigns/nosuchcampaign",
            "",
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            // V2 implements this one, so its answer is its own per-id state report
            // over ITS store, which no router refusal can produce — and the path it
            // sits at is one only the V2 version claims.
            "V2 GetCampaignStateBatch",
            "POST",
            "/v2/campaigns-state",
            r#"{"campaignIds":["000000000000"]}"#,
            200,
            Expect::Member("successfulRequests"),
        ),
        (
            "V2 StartInstanceOnboardingJob",
            "PUT",
            "/v2/connect-instance/instance-1/onboarding",
            r#"{"encryptionConfig":{"enabled":false}}"#,
            200,
            Expect::Member("connectInstanceOnboardingJobStatus"),
        ),
        (
            "V2 PutOutboundRequestBatch for a campaign no store holds",
            "PUT",
            "/v2/campaigns/nosuchcampaign/outbound-requests",
            r#"{"outboundRequests":[{"clientToken":"token-1",
                "expirationTime":"2099-01-01T00:00:00Z","channelSubtypeParameters":{"telephony":{
                    "destinationPhoneNumber":"+15550100","attributes":{}}}}]}"#,
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            "V2 ListTagsForResource for a campaign no store holds",
            "GET",
            concat!("/v2/tags/", encoded_missing_arn!()),
            "",
            404,
            // The wire code ConnectCampaignsV2 answers a tag operation with, which
            // neither the shared model shape name nor v1's stub can produce.
            Expect::Error("NotFoundException"),
        ),
        // ---- v1: unprefixed ----
        (
            "v1 CreateCampaign",
            "PUT",
            "/campaigns",
            V1_CREATE,
            200,
            Expect::Member("id"),
        ),
        (
            "v1 ListCampaigns",
            "POST",
            "/campaigns-summary",
            "{}",
            200,
            Expect::Member("campaignSummaryList"),
        ),
        (
            "v1 DescribeCampaign for an id no store holds",
            "GET",
            "/campaigns/nosuchcampaign",
            "",
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            // v1 implements this one, so its answer is its own per-id state report over
            // ITS store: the id named here is one no v1 campaign carries (the v1 create
            // row above mints its own), so it comes back as the single failed request —
            // which neither a router refusal nor the V2 version, whose every route is
            // `/v2/`-prefixed, can produce for this path.
            "v1 GetCampaignStateBatch",
            "POST",
            "/campaigns-state",
            r#"{"campaignIds":["000000000000"]}"#,
            200,
            Expect::BatchMissing("000000000000"),
        ),
        (
            // v1 implements this one too, and its answer comes from ITS own onboarding
            // store: a job it has only just started, where the V2 version — whose own
            // record for this instance settled before these rows ran — would report a
            // finished one.
            "v1 StartInstanceOnboardingJob",
            "PUT",
            "/connect-instance/instance-1/onboarding",
            r#"{"encryptionConfig":{"enabled":false}}"#,
            200,
            Expect::JobJustStarted,
        ),
        (
            // v1 implements this one now, so its answer is its own dial-plane refusal
            // over ITS store: the id named here is one no v1 campaign carries, so the
            // batch comes back as v1's own run-state not-found — which neither a router
            // refusal nor the V2 version, whose every route is `/v2/`-prefixed, can
            // produce for this path. `expirationTime` is the RFC3339 date-time string the
            // member's `@timestampFormat("date-time")` declares.
            "v1 PutDialRequestBatch",
            "PUT",
            "/campaigns/000000000000/dial-requests",
            r#"{"dialRequests":[{"clientToken":"token-1","phoneNumber":"+15550100",
                "expirationTime":"2099-01-01T00:00:00Z","attributes":{}}]}"#,
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            "v1 ListTagsForResource (still a stub)",
            "GET",
            concat!("/tags/", encoded_arn!()),
            "",
            501,
            Expect::Error("NotImplemented"),
        ),
    ];

    for (case, method, uri, body, expected_status, expected) in rows {
        let (status, error_code, payload) = dispatch(&router, method, uri, body).await;
        for refusal in ROUTER_REFUSALS {
            assert_ne!(
                error_code.as_deref(),
                Some(refusal),
                "case {case:?}: {uri} must be claimed by exactly one API version, got \
                 the router's {refusal}: {status} {payload}"
            );
        }
        match expected {
            Expect::Error(expected_code) => assert_eq!(
                (status, error_code.as_deref()),
                (expected_status, Some(expected_code)),
                "case {case:?}: expected HTTP {expected_status} {expected_code} from the \
                 one version that owns {uri}, got {status} {error_code:?}: {payload}"
            ),
            Expect::Member(member) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status} from the one version \
                     that owns {uri}, got {status} {payload}"
                );
                assert!(
                    payload.get(member).is_some(),
                    "case {case:?}: expected the claiming version's {member} in the \
                     response, got {payload}"
                );
            }
            Expect::BatchMissing(id) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status} from the one version \
                     that owns {uri}, got {status} {payload}"
                );
                let failed = payload
                    .get("failedRequests")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let reported: Vec<(&str, &str)> = failed
                    .iter()
                    .map(|entry| {
                        (
                            entry["campaignId"].as_str().unwrap_or_default(),
                            entry["failureCode"].as_str().unwrap_or_default(),
                        )
                    })
                    .collect();
                assert_eq!(
                    reported,
                    vec![(id, "ResourceNotFound")],
                    "case {case:?}: expected the claiming version's store to report the \
                     requested id as its one failed request, got {payload}"
                );
                assert_eq!(
                    payload
                        .get("successfulRequests")
                        .and_then(Value::as_array)
                        .map_or(usize::MAX, Vec::len),
                    0,
                    "case {case:?}: expected no state to be reported for an id that \
                     version's store does not hold, got {payload}"
                );
            }
            Expect::JobJustStarted => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status} from the one version \
                     that owns {uri}, got {status} {payload}"
                );
                assert_eq!(
                    payload
                        .pointer("/connectInstanceOnboardingJobStatus/status")
                        .and_then(Value::as_str),
                    Some(V1_ONBOARDING_IN_PROGRESS),
                    "case {case:?}: expected the claiming version's own store to report a \
                     job it has only just started, got {payload}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_campaign_created_through_one_version_belongs_to_that_version_alone() {
    let router = build_router();
    onboard_v2_instance(&router).await;

    // Create through V2, with a member only the V2 model carries.
    let (status, error_code, payload) = dispatch(&router, "PUT", "/v2/campaigns", V2_CREATE).await;
    assert_eq!(
        status, 200,
        "V2 CreateCampaign: expected 200, got {status} {error_code:?}: {payload}"
    );
    let v2_id = payload
        .get("id")
        .and_then(Value::as_str)
        .expect("V2 CreateCampaign returns the minted id")
        .to_string();

    // The V2 reader serves it, and reports the V2-only member back.
    let (status, error_code, payload) =
        dispatch(&router, "GET", &format!("/v2/campaigns/{v2_id}"), "").await;
    assert_eq!(
        (status, payload.pointer("/campaign/channelSubtypeConfig").is_some()),
        (200, true),
        "case {:?}: the V2 version must serve its own campaign, got {status} \
         {error_code:?}: {payload}",
        "V2 DescribeCampaign of a V2 campaign"
    );

    // The same id on v1's route reaches v1, whose own store has no such campaign:
    // v1's modeled 404, not a router refusal and not V2's answer.
    let (status, error_code, payload) =
        dispatch(&router, "GET", &format!("/campaigns/{v2_id}"), "").await;
    assert_eq!(
        (status, error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {:?}: expected v1's own 404 for a campaign it never created, got {status} \
         {error_code:?}: {payload}",
        "v1 DescribeCampaign of a V2 campaign id"
    );

    // And symmetrically: a v1 campaign is unknown to V2.
    let (status, error_code, payload) = dispatch(&router, "PUT", "/campaigns", V1_CREATE).await;
    assert_eq!(
        status, 200,
        "v1 CreateCampaign: expected 200, got {status} {error_code:?}: {payload}"
    );
    let v1_id = payload
        .get("id")
        .and_then(Value::as_str)
        .expect("v1 CreateCampaign returns the minted id")
        .to_string();
    let (status, error_code, payload) =
        dispatch(&router, "GET", &format!("/campaigns/{v1_id}"), "").await;
    assert_eq!(
        (status, payload.pointer("/campaign/dialerConfig").is_some()),
        (200, true),
        "case {:?}: the v1 version must serve its own campaign, got {status} \
         {error_code:?}: {payload}",
        "v1 DescribeCampaign of a v1 campaign"
    );
    let (status, error_code, payload) =
        dispatch(&router, "GET", &format!("/v2/campaigns/{v1_id}"), "").await;
    assert_eq!(
        (status, error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {:?}: expected V2's own 404 for a campaign it never created, got {status} \
         {error_code:?}: {payload}",
        "V2 DescribeCampaign of a v1 campaign id"
    );
}

#[tokio::test]
async fn a_path_neither_version_models_is_refused_only_for_want_of_a_claimant() {
    let router = build_router();

    // The claim is exact segment equality, so a near miss of either version's space is
    // claimed by neither — and THAT is the only reason routing may fail.
    for (case, uri) in [
        ("a third API version's prefix", "/v3/campaigns"),
        ("a longer segment the V2 prefix starts", "/v22/campaigns"),
        ("a segment that merely contains a modelled one", "/my-campaigns"),
    ] {
        let (status, error_code, payload) = dispatch(&router, "PUT", uri, V2_CREATE).await;
        assert_ne!(
            error_code.as_deref(),
            Some("AmbiguousService"),
            "case {case:?}: an unmodelled path must not be reported as contended, got \
             {status} {payload}"
        );
        assert_eq!(
            (status, error_code.as_deref()),
            (404, Some("UnknownService")),
            "case {case:?}: expected the router's no-claimant refusal for a path neither \
             version models, got {status} {error_code:?}: {payload}"
        );
    }
}

#[test]
fn the_two_path_spaces_the_claim_rests_on_are_disjoint() {
    // The premise of both `can_handle` seams, read from the generated, model-derived
    // constants: each version publishes the first segments its restJson1 operations
    // bind, and the two sets share nothing — which is what lets the path conjunct
    // separate two services that share a signing name.
    let v1 = service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ROUTE_PATH_SEGMENTS;
    let v2 = service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ROUTE_PATH_SEGMENTS;
    assert_eq!(
        service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ROUTE_KEY,
        service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ROUTE_KEY,
        "the two API versions are expected to SHARE one signing name; that is why the \
         path conjunct exists"
    );
    assert!(
        !v1.is_empty() && !v2.is_empty(),
        "each version must publish the first segments its routes bind: v1 {v1:?}, V2 {v2:?}"
    );
    for segment in v2 {
        assert!(
            !v1.contains(segment),
            "the two versions' path spaces must be disjoint, but both claim {segment:?}: \
             v1 {v1:?}, V2 {v2:?}"
        );
    }
    assert!(
        !v1.contains(&emulator_server::ROOT_PATH_SEGMENT)
            && !v2.contains(&emulator_server::ROOT_PATH_SEGMENT),
        "neither restJson1 version binds the root path, so neither may claim it: \
         v1 {v1:?}, V2 {v2:?}"
    );
}
