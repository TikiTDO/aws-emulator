// SPDX-License-Identifier: CC0-1.0

//! A delivery source over a CloudFront distribution is created only where CloudFront HOLDS it.
//!
//! RULE: `PutDeliverySource` creates "a logical object that represents the resource that is
//! actually sending the logs", so CloudWatch Logs resolves the `resourceArn` against the
//! service that owns the resource before it stores a source — a put naming a resource that
//! service reports it does not hold is refused with `ResourceNotFoundException`, carrying that
//! service's own wording for the missing resource, and stores nothing. A refusal is GROUNDED in
//! what the owning service answered, so a distribution CloudFront does hold is stored as
//! before.
//!
//! This suite needs CloudWatch Logs and CloudFront on one engine — CloudWatch Logs reaches
//! CloudFront through the in-process callback, so a single-service router has nothing to ask —
//! which is why it lives here rather than in the CloudWatch Logs crate. Amazon S3 is mounted
//! beside them because the created distribution's origin is a bucket. The arm whose owning
//! service is Amazon S3, and the arms where nothing can be asked at all, are the CloudWatch
//! Logs crate's own `tests/all/delivery_source_resource_resolution.rs`.
//!
//! The rows differ along what the rule generalises over: whether CloudFront holds the
//! distribution the ARN names, and the form of the id it names (one CloudFront minted, and ids
//! it never minted).

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use crate::cloudfront_oac_s3_origin_edge::{
    call, create_bucket, create_distribution, create_origin_access_control, REGION,
};

/// The AWS-managed cache policy `Managed-CachingDisabled`, named on the distribution's default
/// behaviour so the create needs nothing else configured.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// The bucket the created distribution's origin names.
const BUCKET: &str = "logs-delivery-source-origin";

/// The one log type CloudFront vends ("For CloudFront, the valid value is ACCESS_LOGS",
/// `smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
const LOG_TYPE: &str = "ACCESS_LOGS";

/// CloudFront's own wording for a distribution it does not hold (`NoSuchDistribution`), as the
/// answer to the `GetDistribution` the resolution makes carries it.
const ABSENT_MESSAGE: &str = "The specified distribution does not exist.";

/// The refusal a resource the owning service does not hold is answered with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The three services this suite exercises, mounted on one engine exactly as the router mounts
/// them, with request authentication off: each call carries a credential scope (which is what
/// names the Region) without a computed signature.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One CloudWatch Logs call: its status and parsed body.
async fn logs(
    router: &MultiServiceRouter,
    operation: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let answered = call(
        router,
        "logs",
        &format!("logs.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &format!("Logs_20140328.{operation}")),
        ],
        bytes::Bytes::from(body.to_string()),
    )
    .await;
    let text = answered.text();
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (answered.status, parsed)
}

/// The wire error code of an awsJson error body (`__type`, which may carry a namespace).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body["__type"].as_str().unwrap_or_default();
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

#[tokio::test]
async fn a_delivery_source_over_a_distribution_needs_cloudfront_to_hold_it() {
    let router = emulator();

    let case = "a distribution CloudFront holds";
    create_bucket(&router, case, BUCKET).await;
    let control =
        create_origin_access_control(&router, case, "oac-delivery-source", "always").await;
    let (id, _domain, arn) = create_distribution(
        &router,
        case,
        "delivery-source-reference",
        BUCKET,
        &control,
        CACHING_DISABLED,
    )
    .await;

    // The rows: the ARN each put names, and the refusal it must answer — `None` for a put that
    // must be accepted.
    let never_minted = format!(
        "arn:aws:cloudfront::{}:distribution/E1NEVERMINTED1X",
        engine::emulator_account_id()
    );
    let another_form = format!(
        "arn:aws:cloudfront::{}:distribution/{id}X",
        engine::emulator_account_id()
    );
    let rows: [(&str, String, Option<&str>); 3] = [
        (case, arn, None),
        (
            "a distribution id CloudFront never minted",
            never_minted,
            Some(ABSENT_MESSAGE),
        ),
        (
            "the held distribution's id with a character appended",
            another_form,
            Some(ABSENT_MESSAGE),
        ),
    ];

    for (index, (case, resource_arn, refused_with)) in rows.iter().enumerate() {
        let name = format!("cloudfront-source-{index}");
        let (status, answered) = logs(
            &router,
            "PutDeliverySource",
            &serde_json::json!({
                "name": name,
                "resourceArn": resource_arn,
                "logType": LOG_TYPE,
            }),
        )
        .await;
        let (read_status, read) =
            logs(&router, "GetDeliverySource", &serde_json::json!({"name": name})).await;
        match refused_with {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the put to be accepted, got {status} {answered}"
                );
                assert_eq!(
                    read["deliverySource"]["resourceArns"],
                    serde_json::json!([resource_arn]),
                    "case {case:?}: expected the stored source to represent {resource_arn:?}, \
                     got {read}"
                );
            }
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the put to be refused, got {status} {answered}"
                );
                let code = error_code(&answered);
                assert_eq!(
                    code, NOT_FOUND,
                    "case {case:?}: expected {NOT_FOUND}, got {code} {answered}"
                );
                assert_eq!(
                    answered["message"].as_str(),
                    Some(*message),
                    "case {case:?}: expected CloudFront's own wording {message:?}, got {answered}"
                );
                assert_eq!(
                    read_status, 400,
                    "case {case:?}: a refused put stores nothing, got {read_status} {read}"
                );
            }
        }
    }
}
