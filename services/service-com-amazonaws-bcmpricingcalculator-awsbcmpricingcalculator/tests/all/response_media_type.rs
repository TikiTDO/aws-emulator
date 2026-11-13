// SPDX-License-Identifier: CC0-1.0

//! The media type every response of this service announces — there are two announcers.
//!
//! The RULE under test: a response of the WORKLOAD-ESTIMATE and BILL-PLANNING dispatcher —
//! its successes, and the errors its handlers raise about stored state — announces
//! `application/x-amz-json-1.1`, while everything answered in FRONT of that dispatcher
//! announces the `application/x-amz-json-1.0` the model's `aws.protocols#awsJson1_0` trait
//! implies: a refusal that reports the request itself as invalid (the modelled
//! `ValidationException`), every response of a resource-tagging operation, an unroutable
//! request, a transport-level failure and the authentication refusal. A client (or a
//! recording proxy) picks its JSON-protocol decoder from that header, so which value a
//! response carries is part of the wire contract. `src/service/response_media_type.rs`
//! states which announcer owns what, and the recorded wire it is read off.
//!
//! The rows span both halves of the rule at once: the LAYER that produced the response (a
//! handler success, a handler's modelled exception about stored state, the boundary
//! validator's refusal, a refusal a handler raises because the boundary validator cannot
//! express the rule, the scaffold's 501, the routing failure rendered before any operation
//! runs, the transport failure rendered before the request is routed, and the
//! authentication refusal the router renders before this service is asked anything), and
//! the OPERATION family it belongs to. Every row states the media type it expects, so a row
//! moving from one announcer to the other fails here. The recorded cases — the tagging
//! reads and writes at 200, the tagging refusals and the constraint refusals at 400, the
//! create at 200 and the not-found reads at 404 — are rows among them.
//!
//! The REQUEST side stays awsJson1_0: every call below is routed by
//! `X-Amz-Target: AWSBCMPricingCalculator.<Operation>` and sent under
//! `content-type: application/x-amz-json-1.0`, and every one of them is understood.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

use crate::bill_scenario_lifecycle::{request, request_builder, Emulator, REQUEST_MEDIA_TYPE};

/// The media type the workload-estimate and bill-planning dispatcher announces on what it
/// returns.
///
/// `pub(crate)` with the helpers below: `tests/all/account_class_refusal_announcer.rs` reads
/// the same two values and sends through the same request builder, so the two suites cannot
/// disagree about what either announcer answers under.
pub(crate) const DISPATCHER_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The media type the model's `awsJson1_0` protocol trait implies (smithy_query
/// `query="service_info"` → `protocols: ["awsJson1_0"]`), which the framework stamps and
/// everything answered in front of the dispatcher announces. Requests are sent under it too
/// — it is the harness's own [`REQUEST_MEDIA_TYPE`].
pub(crate) const PROTOCOL_MEDIA_TYPE: &str = REQUEST_MEDIA_TYPE;

/// A well-formed `ResourceId` (the lowercase-hex v4 UUID form `^[a-f0-9]{8}-[a-f0-9]{4}-…$`
/// the shape pins) that no live resource of this emulator carries.
const ABSENT_ID: &str = "00000000-0000-0000-0000-000000000000";

/// A `ResourceId` outside that form: what the boundary validator refuses.
const MALFORMED_ID: &str = "not-a-resource-id";

/// The ARN of a workload estimate of this account, built the way a client holding one of
/// this service's ARNs sends it back.
fn estimate_arn(id: &str) -> String {
    engine::emulator_arn(
        state::ARN_NAMESPACE,
        state::ARN_REGION,
        &format!("{}/{id}", state::WORKLOAD_ESTIMATE_ARN_TYPE),
    )
}

/// Call one operation and answer the WHOLE response, headers included — what
/// [`Emulator::call`] parses the body out of, sent through the same request builder.
pub(crate) async fn send(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
) -> http::Response<Bytes> {
    emulator.router.dispatch(request(Some(operation), body)).await
}

/// The media type a response announces, as a client reads it.
pub(crate) fn media_type(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<no content-type>")
        .to_string()
}

/// Assert one response: the status it carries, the media type it announces, and that the
/// body under that media type is the JSON document the media type promises.
pub(crate) fn assert_media_type(
    case: &str,
    response: &http::Response<Bytes>,
    expected_status: u16,
    expected_media_type: &str,
) {
    let actual = media_type(response);
    let status = response.status().as_u16();
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} (content-type {actual:?}, \
         body {:?})",
        String::from_utf8_lossy(response.body())
    );
    assert_eq!(
        actual, expected_media_type,
        "case {case:?}: expected content-type {expected_media_type:?}, got {actual:?}"
    );
    let parsed: Result<Value, _> = serde_json::from_slice(response.body());
    assert!(
        parsed.is_ok(),
        "case {case:?}: the body under {actual} must be the JSON document that media type \
         promises, got {:?}",
        String::from_utf8_lossy(response.body())
    );
}

/// One call: what produced the response, and what that layer answers with.
struct Case {
    /// What produced the response, and which announcer it belongs to.
    label: &'static str,
    /// The operation named in `X-Amz-Target`.
    operation: &'static str,
    body: Value,
    status: u16,
    /// The media type this response announces.
    media_type: &'static str,
}

/// Every response of a routed request announces the media type of the announcer that
/// produced it: the dispatcher's own successes, state errors and scaffold 501s on one side,
/// the validation refusals and the whole resource-tagging family on the other.
#[tokio::test]
async fn every_routed_response_announces_its_announcers_media_type() {
    let emulator = Emulator::new();
    let created = send(&emulator, "CreateWorkloadEstimate", &json!({"name": "mediatype"})).await;
    assert_media_type(
        "the fixture: a created workload estimate",
        &created,
        200,
        DISPATCHER_MEDIA_TYPE,
    );
    let created: Value = serde_json::from_slice(created.body()).expect("a JSON body");
    let estimate_id = created["id"].as_str().expect("an id").to_string();
    let live_arn = estimate_arn(&estimate_id);

    let cases = [
        Case {
            // A recorded witness: a create of a workload estimate.
            label: "the dispatcher, a handler success: a create",
            operation: "CreateWorkloadEstimate",
            body: json!({"name": "mediatypesecond"}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: the preferences read",
            operation: "GetPreferences",
            body: json!({}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: the preferences write",
            operation: "UpdatePreferences",
            body: json!({"managementAccountRateTypeSelections": ["AFTER_DISCOUNTS"]}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: a single-resource read",
            operation: "GetWorkloadEstimate",
            body: json!({"identifier": estimate_id}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: a paged listing",
            operation: "ListWorkloadEstimates",
            body: json!({"maxResults": 1}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: an empty listing",
            operation: "ListBillScenarios",
            body: json!({"maxResults": 5}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: a create on the bill-planning surface",
            operation: "CreateBillScenario",
            body: json!({"name": "mediatypescenario"}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, a handler success: a nested usage listing",
            operation: "ListWorkloadEstimateUsage",
            body: json!({"workloadEstimateId": estimate_id}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            // A recorded witness: the not-found read of a workload estimate.
            label: "the dispatcher, an error about stored state: a read of an absent estimate",
            operation: "GetWorkloadEstimate",
            body: json!({"identifier": ABSENT_ID}),
            status: 404,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, an error about stored state: a usage listing of an \
                    absent estimate",
            operation: "ListWorkloadEstimateUsage",
            body: json!({"workloadEstimateId": ABSENT_ID}),
            status: 404,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, an error about stored state: a usage write to an absent \
                    estimate",
            operation: "BatchCreateWorkloadEstimateUsage",
            body: json!({
                "workloadEstimateId": ABSENT_ID,
                "usage": [{
                    "key": "k0",
                    "serviceCode": "AmazonEC2",
                    "usageType": "BoxUsage:m5.xlarge",
                    "operation": "RunInstances",
                    "usageAccountId": "000000000000",
                    "amount": 1.0,
                }],
            }),
            status: 404,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, the scaffold's 501: a listing nobody has implemented yet",
            operation: "ListBillEstimates",
            body: json!({}),
            status: 501,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            label: "the dispatcher, the scaffold's 501: a read nobody has implemented yet",
            operation: "GetBillEstimate",
            body: json!({"identifier": ABSENT_ID}),
            status: 501,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
        Case {
            // A recorded witness: the refusal of a name outside its modelled pattern.
            label: "the validator: a name outside the modelled pattern",
            operation: "CreateWorkloadEstimate",
            body: json!({"name": "not a legal name!"}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the validator: an absent required member",
            operation: "CreateWorkloadEstimate",
            body: json!({}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the validator: an identifier outside the modelled pattern",
            operation: "GetWorkloadEstimate",
            body: json!({"identifier": MALFORMED_ID}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            // A recorded witness: the refusal of a malformed parent id on a batch write.
            label: "the validator: a parent id outside the modelled pattern",
            operation: "BatchCreateWorkloadEstimateUsage",
            body: json!({"workloadEstimateId": MALFORMED_ID, "usage": []}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            // A recorded witness: the refusal of a malformed identifier on an update.
            label: "the validator: an identifier outside the modelled pattern on an update",
            operation: "UpdateWorkloadEstimate",
            body: json!({"identifier": MALFORMED_ID, "name": "renamed"}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the validator: a page size above the modelled range",
            operation: "ListWorkloadEstimates",
            body: json!({"maxResults": 26}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            // A refusal reporting the REQUEST as invalid is the validator's wherever the
            // emulator raises it: the boundary validator refuses what the model's
            // constraints express, a handler refuses what they cannot.
            label: "the validator, raised by a handler: a reserved tag key",
            operation: "TagResource",
            body: json!({"arn": live_arn.clone(), "tags": {"aws:internal": "x"}}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            // A recorded witness: the tag listing of a live estimate.
            label: "the tagging family: a tag listing",
            operation: "ListTagsForResource",
            body: json!({"arn": live_arn.clone()}),
            status: 200,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            // A recorded witness: the tag write.
            label: "the tagging family: a tag merge",
            operation: "TagResource",
            body: json!({"arn": live_arn.clone(), "tags": {"env": "test"}}),
            status: 200,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            // A recorded witness: the tag removal.
            label: "the tagging family: a tag removal",
            operation: "UntagResource",
            body: json!({"arn": live_arn.clone(), "tagKeys": ["env"]}),
            status: 200,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            // A recorded witness: the tagging read of an ARN addressing nothing.
            label: "the tagging family: a tag listing of an ARN addressing no live resource",
            operation: "ListTagsForResource",
            body: json!({"arn": estimate_arn(ABSENT_ID)}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag write to an ARN addressing no live resource",
            operation: "TagResource",
            body: json!({"arn": estimate_arn(ABSENT_ID), "tags": {"env": "test"}}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: a tag removal naming no key the resource carries",
            operation: "UntagResource",
            body: json!({"arn": live_arn.clone(), "tagKeys": ["never-set"]}),
            status: 200,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        Case {
            label: "the tagging family: an ARN outside the modelled pattern",
            operation: "ListTagsForResource",
            body: json!({"arn": "not-an-arn"}),
            status: 400,
            media_type: PROTOCOL_MEDIA_TYPE,
        },
        // Last: it removes the fixture the rows above read.
        Case {
            label: "the dispatcher, a handler success: a delete",
            operation: "DeleteWorkloadEstimate",
            body: json!({"identifier": estimate_id}),
            status: 200,
            media_type: DISPATCHER_MEDIA_TYPE,
        },
    ];

    for case in &cases {
        let response = send(&emulator, case.operation, &case.body).await;
        assert_media_type(case.label, &response, case.status, case.media_type);
    }
}

// A request a MEMBER account makes of the bill-planning surface is refused by the
// account-class gate, which is part of the dispatcher's front door: that refusal is the
// dispatcher's and announces its media type, even though it is worded in the same modelled
// `ValidationException` the request validator's refusal is. Pinned over every gated
// operation, beside the body it must not change, in
// `tests/all/account_class_refusal_announcer.rs`.

/// A request that reaches this service but names no modelled operation is answered before
/// any operation runs — so no dispatcher produced it, and it announces the protocol's media
/// type. The framework's own rendering for the awsJson family is kept: an unknown operation
/// is a 501 `NotImplemented`, an unreadable routing key a 400 `SerializationException`.
#[tokio::test]
async fn a_response_rendered_before_routing_announces_the_protocols_media_type() {
    let emulator = Emulator::new();

    // An `X-Amz-Target` naming an operation this model does not declare: one that does not
    // exist at all, one in the wrong case, and one that extends a real name.
    for operation in ["NoSuchOperation", "getpreferences", "GetPreferencesExtra"] {
        let response = send(&emulator, operation, &json!({})).await;
        assert_media_type(operation, &response, 501, PROTOCOL_MEDIA_TYPE);
    }

    // No `X-Amz-Target` at all: the route key still resolves from the credential scope, so
    // the request reaches this service and is refused for the unreadable routing key.
    let response = emulator.router.dispatch(request(None, &json!({}))).await;
    assert_media_type("no x-amz-target", &response, 400, PROTOCOL_MEDIA_TYPE);
}

/// A transport-level failure the router detects before the request is routed at all — a
/// body that declares `aws-chunked` framing and is not framed — reaches no dispatcher
/// either, and announces the protocol's media type.
#[tokio::test]
async fn a_transport_failure_announces_the_protocols_media_type() {
    let emulator = Emulator::new();
    let request = request_builder(Some("GetPreferences"))
        .header("content-encoding", "aws-chunked")
        // Declares the framing and carries none of it.
        .body(Bytes::from_static(b"{}"))
        .expect("a well-formed request");

    let response = emulator.router.dispatch(request).await;
    assert_media_type(
        "an aws-chunked body that is not framed",
        &response,
        400,
        PROTOCOL_MEDIA_TYPE,
    );
}

/// The authentication refusal, which the router renders through
/// `definition().refusal_response(..)` before this service is asked anything
/// (`vendor/smithy-server-runtime/src/multi_service_router.rs`): no operation of this
/// service ran, so it announces the protocol's media type.
#[tokio::test]
async fn the_authentication_refusal_announces_the_protocols_media_type() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut authenticating = MultiServiceRouter::new();
    authenticating.set_request_authentication(true);
    authenticating.register_service(EmulatedService::new(engine));

    // The harness's signature is not one this emulator vended, so the request is refused
    // before the operation it names runs.
    let response = authenticating
        .dispatch(request(Some("GetPreferences"), &json!({})))
        .await;
    assert_media_type(
        "the authentication refusal",
        &response,
        403,
        PROTOCOL_MEDIA_TYPE,
    );
}
