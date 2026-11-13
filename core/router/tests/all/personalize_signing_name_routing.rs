// SPDX-License-Identifier: CC0-1.0

//! Amazon Personalize, Amazon Personalize Events and Amazon Personalize Runtime are
//! THREE services behind ONE SigV4 signing name (`personalize`), which no Smithy trait
//! expresses. With all three mounted, the default signing-name `can_handle` makes all
//! three claim every Personalize-signed request and the router answers
//! `500 AmbiguousService` for all of it — so none of them is reachable.
//!
//! Each family member therefore claims only its own traffic: the two restJson1
//! data-plane services by the URI path space their models declare, and the awsJson1_1
//! control plane by its `X-Amz-Target` prefix.
//!
//! This suite needs the WHOLE router: the ambiguity only exists when all three
//! services are mounted, so a single-service test cannot see it.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state as runtime_state;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming `personalize` — what all THREE services' requests
/// are signed with, and therefore the whole problem.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/personalize/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The campaign the runtime row gets its recommendations from.
const CAMPAIGN_ARN: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/routing";

/// One request, and the answer its OWNING service gives.
struct Case {
    /// How the row reads in an assertion message.
    name: &'static str,
    /// The URI path the request addresses.
    path: &'static str,
    /// The `X-Amz-Target` an awsJson1_1 request carries, if any.
    target: Option<&'static str>,
    /// The request body.
    body: &'static [u8],
    /// The status the owning service answers, where pinning it does not depend on an
    /// operation that is still scaffolded.
    status: Option<u16>,
}

#[test]
fn the_three_personalize_services_route_apart_under_one_signing_name() {
    std::thread::Builder::new()
        .name("personalize-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            // The runtime service's registration seam, on the SAME engine the router
            // hands every service: this is how a campaign comes to exist for an
            // inference call until the control plane serves `CreateCampaign`.
            let items = vec![
                runtime_state::Item::new("item-a", 3.0),
                runtime_state::Item::new("item-b", 2.0),
                runtime_state::Item::new("item-c", 1.0),
            ];
            assert!(
                runtime_state::register_campaign(
                    &*engine,
                    runtime_state::Campaign::new(
                        CAMPAIGN_ARN,
                        "routing",
                        runtime_state::RecipeType::UserPersonalization,
                        items,
                    ),
                ),
                "the runtime registration seam must register the campaign"
            );

            let mut router = build_router(Arc::clone(&engine));
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Each row: the request one family member owns, and the status that
            // service answers. What this suite is about is that no request is answered
            // `AmbiguousService` or `UnknownService` — every one must reach exactly one
            // service — so the two rows whose operations are still scaffolded carry
            // `None`: pinning their 501 would fail the day they are implemented, for a
            // reason that has nothing to do with routing.
            let cases: [Case; 3] = [
                Case {
                    name: "the runtime inference surface",
                    path: "/recommendations",
                    target: None,
                    body: br#"{"campaignArn":"arn:aws:personalize:us-east-1:000000000000:campaign/routing","userId":"user-1","numResults":2}"#,
                    status: Some(200),
                },
                Case {
                    name: "the events ingestion surface",
                    path: "/events",
                    target: None,
                    body: br#"{"trackingId":"t","sessionId":"s","eventList":[]}"#,
                    status: None,
                },
                Case {
                    name: "a control-plane operation",
                    path: "/",
                    target: Some("AmazonPersonalize.ListCampaigns"),
                    body: b"{}",
                    status: None,
                },
            ];

            for case in cases {
                let Case {
                    name,
                    path,
                    target,
                    body,
                    status: expected_status,
                } = case;
                let mut request = ::http::Request::builder()
                    .method("POST")
                    .uri(format!("http://localhost{path}"))
                    .header("authorization", AUTHORIZATION);
                request = match target {
                    Some(target) => request
                        .header("x-amz-target", target)
                        .header("content-type", "application/x-amz-json-1.1"),
                    None => request.header("content-type", "application/json"),
                };
                let request = request
                    .body(::bytes::Bytes::from_static(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert_ne!(
                    status, 500,
                    "case {name} (POST {path}): the request was claimed by more than one \
                     Personalize service (AmbiguousService), got {status} with {rendered}"
                );
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {name} (POST {path}): routing failed, got {status} with {rendered}"
                );
                if let Some(expected_status) = expected_status {
                    assert_eq!(
                        status, expected_status,
                        "case {name} (POST {path}): the owning service must answer HTTP \
                         {expected_status}, got {status} with {rendered}"
                    );
                }
            }

            // The runtime row's response is computed from the seeded catalog, so the
            // request reached the runtime service and nothing else.
            let request = ::http::Request::builder()
                .method("POST")
                .uri("http://localhost/recommendations")
                .header("authorization", AUTHORIZATION)
                .header("content-type", "application/json")
                .body(::bytes::Bytes::from(
                    format!(r#"{{"campaignArn":"{CAMPAIGN_ARN}","userId":"user-1","numResults":2}}"#),
                ))
                .expect("a well-formed request");
            let response = runtime.block_on(router.dispatch(request));
            let rendered = String::from_utf8_lossy(response.body()).to_string();
            let parsed: serde_json::Value =
                serde_json::from_str(&rendered).unwrap_or(serde_json::Value::Null);
            let returned: Vec<String> = parsed
                .get("itemList")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("no itemList in {rendered}"))
                .iter()
                .map(|entry| {
                    entry
                        .get("itemId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            assert_eq!(
                returned.len(),
                2,
                "the runtime service must answer with the 2 requested items, got {rendered}"
            );
            for item_id in &returned {
                assert!(
                    ["item-a", "item-b", "item-c"].contains(&item_id.as_str()),
                    "every recommended item must come from the seeded catalog, got {item_id}"
                );
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("the three Personalize services must route apart");
}
