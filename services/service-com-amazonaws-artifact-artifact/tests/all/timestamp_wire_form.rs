// SPDX-License-Identifier: CC0-1.0

//! The wire form of every timestamp this service reports, and the terminal state a
//! compliance inquiry reaches.
//!
//! Two rules are under test, both read off the Smithy model rather than off any one
//! recorded response:
//!
//! 1. A member whose target carries `@timestampFormat("date-time")` is serialized on
//!    restJson1 as an RFC 3339 / ISO-8601 date-time STRING — never as restJson1's default
//!    epoch-seconds number. In this service that is EVERY timestamp member: all of them
//!    target `com.amazonaws.artifact#TimestampAttribute`, which the model marks
//!    `@timestampFormat("date-time")`
//!    (`smithy_query(query="shape",shape="TimestampAttribute")`). The catalog of members
//!    below comes from `smithy_query(query="shape",shape="InquirySummary,InquiryDetail,
//!    QuerySummary,ResponseVersion,ReportSummary,ReportDetail,CustomerAgreementSummary,
//!    PutComplianceInquiryFeedbackResponse")` — the shapes of this service that own one.
//! 2. A compliance inquiry's terminal `status`/`statusMessage` are DERIVED from its
//!    queries: an inquiry whose queries all reached their terminal state without failing
//!    completes with `InquiryStatusMessage`'s "Compliance inquiry processing is complete.",
//!    the "One or more queries encountered errors during processing." value is reported
//!    only when a query failed, and an inquiry whose submitted content carried nothing to
//!    process — so it owns no query at all — ends `FAILED`. A query reports the answer it
//!    holds — its `response` and the `citations` supporting it, always together — once its
//!    processing is over, and reports neither while no source of the published catalog
//!    bears on it, which is not a query-level error.
//!
//! Rule 1 is checked end to end over HTTP across the operations below that report a
//! timestamp (so the rows differ along operation, shape, member, whether the member is
//! `@required`, and whether the instant has a sub-second part: a created inquiry's
//! `createdAt` comes from the clock, a catalog period is a whole second), and directly on
//! the member schemas, which is the only reach a shape no operation of this service
//! reports (`ResponseVersion`) has. `PutComplianceInquiryFeedbackResponse$submittedAt` is
//! reported over HTTP by `PutComplianceInquiryFeedback`, and its end-to-end row lives in
//! `implemented_inquiry_operations.rs`, beside that operation's other rows.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::ops::types::{
    CustomerAgreementSummary, InquiryDetail, InquirySummary, PutComplianceInquiryFeedbackOutput,
    QuerySummary, ReportDetail, ReportSummary, ResponseVersion,
};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// The span after which an inquiry's processing is due to be over, plus slack. The
/// handlers store the due instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio without
/// its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// `InquiryStatusMessage`'s value for an inquiry that completed with nothing failing.
const INQUIRY_COMPLETE: &str = "Compliance inquiry processing is complete.";

/// `InquiryStatusMessage`'s value for an inquiry that completed with a failed query.
const INQUIRY_COMPLETE_WITH_QUERY_ERRORS: &str = "Compliance inquiry processing is complete. \
     One or more queries encountered errors during processing.";

/// `InquiryStatusMessage`'s value for an inquiry whose processing itself failed.
const INQUIRY_INTERNAL_ERROR: &str =
    "An internal error occurred while processing the inquiry. Try again at a later time.";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn get(router: &MultiServiceRouter, uri: &str) -> Value {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED)
        .body(Bytes::new())
        .expect("the test request is well formed");
    send(router, uri, request).await
}

async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", SIGNED)
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    send(router, uri, request).await
}

async fn send(
    router: &MultiServiceRouter,
    uri: &str,
    request: ::http::Request<Bytes>,
) -> Value {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "{uri} answered a body that is not JSON ({error}): {:?}",
            String::from_utf8_lossy(&bytes)
        )
    });
    assert!(
        (200..300).contains(&status),
        "{uri} must succeed for this suite to say anything, got {status} with {body:?}"
    );
    body
}

/// A `CreateComplianceInquiry` body carrying one submitted query.
fn text_body(name: &str, query: &str) -> Value {
    json!({ "name": name, "inquiryContent": { "query": query }, "supportMode": "AI_ONLY" })
}

/// A `CreateComplianceInquiry` body carrying an uploaded document of `content` bytes.
fn file_body(name: &str, content: &str) -> Value {
    json!({
        "name": name,
        "inquiryContent": { "fileContent": { "content": content } },
    })
}

/// The member at `pointer`, or `Value::Null` when the body carries none.
fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Assert one `TimestampAttribute` member's wire form: a JSON STRING that parses as an
/// RFC 3339 date-time and renders back to the very same text, naming a real instant.
///
/// Parsed and re-rendered rather than matched against a literal: the instant itself is
/// environment-specific (a created inquiry's `createdAt` is the clock's), so what is under
/// test is the JSON type and the lexical form, not the value.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    assert!(
        !value.is_number(),
        "case {case:?}: {member} targets TimestampAttribute (@timestampFormat date-time), so it \
         must be an RFC 3339 string and never an epoch-seconds number, got {value:?}"
    );
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {text:?}")
    });
    let rendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {member} does not re-render ({error})"));
    assert_eq!(
        rendered, text,
        "case {case:?}: {member} must already be in the canonical date-time form, got {text:?} \
         against {rendered:?}"
    );
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {text:?}"
    );
}

#[tokio::test]
async fn every_reported_date_time_member_is_an_rfc3339_string() {
    let router = emulator();

    // The compliance-inquiry projections, read BEFORE the processing span is over (the
    // inquiry is still `PROCESSING`, so `updatedAt` is its creation instant) and again
    // after it, where the clock-owed transition moved `updatedAt` on. Both instants carry
    // a sub-second part, which the catalog's whole-second periods do not.
    let created = post(
        &router,
        "/v1/compliance-inquiry/create",
        &text_body("Data residency", "Where is customer data stored?"),
    )
    .await;
    let inquiry_id = at(&created, "/complianceInquirySummary/id")
        .as_str()
        .expect("the create reports the minted inquiry id")
        .to_string();
    let pending = get(
        &router,
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
    )
    .await;
    let listed_inquiries = get(&router, "/v1/compliance-inquiry/list").await;
    let pending_queries = get(
        &router,
        &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={inquiry_id}"),
    )
    .await;

    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    let settled = get(
        &router,
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
    )
    .await;
    let settled_queries = get(
        &router,
        &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={inquiry_id}"),
    )
    .await;

    // The report catalog and the account's customer-agreements: whole-second instants,
    // optional members, and both the list and the single-record projection.
    let reports = get(&router, "/v1/report/list").await;
    let report_id = at(&reports, "/reports/0/id")
        .as_str()
        .expect("the catalog publishes at least one report")
        .to_string();
    let report_version = at(&reports, "/reports/0/version")
        .as_i64()
        .expect("a catalog entry reports its version");
    let latest_metadata = get(
        &router,
        &format!("/v1/report/getMetadata?reportId={report_id}"),
    )
    .await;
    let pinned_metadata = get(
        &router,
        &format!("/v1/report/getMetadata?reportId={report_id}&reportVersion={report_version}"),
    )
    .await;
    let versions = get(
        &router,
        &format!("/v1/report/listVersions?reportId={report_id}"),
    )
    .await;
    let agreements = get(&router, "/v1/customer-agreement/list").await;

    // (case, body, member pointer, present?) — one row per TimestampAttribute member each
    // implemented operation reports. A `@required` member must be there; an optional one is
    // only checked when the record carries it.
    let required: Vec<(&str, &Value, &str)> = vec![
        (
            "CreateComplianceInquiry/InquirySummary",
            &created,
            "/complianceInquirySummary/createdAt",
        ),
        (
            "GetComplianceInquiryMetadata/InquiryDetail, still processing",
            &pending,
            "/complianceInquiryDetail/createdAt",
        ),
        (
            "GetComplianceInquiryMetadata/InquiryDetail, still processing",
            &pending,
            "/complianceInquiryDetail/updatedAt",
        ),
        (
            "GetComplianceInquiryMetadata/InquiryDetail, terminal",
            &settled,
            "/complianceInquiryDetail/createdAt",
        ),
        (
            "GetComplianceInquiryMetadata/InquiryDetail, terminal",
            &settled,
            "/complianceInquiryDetail/updatedAt",
        ),
        (
            "ListComplianceInquiries/InquirySummary",
            &listed_inquiries,
            "/complianceInquiries/0/createdAt",
        ),
        (
            "ListComplianceInquiryQueries/QuerySummary, still processing",
            &pending_queries,
            "/queries/0/createdAt",
        ),
        (
            "ListComplianceInquiryQueries/QuerySummary, terminal",
            &settled_queries,
            "/queries/0/createdAt",
        ),
    ];
    let optional: Vec<(&str, &Value, &str)> = vec![
        ("ListReports/ReportSummary", &reports, "/reports/0/periodStart"),
        ("ListReports/ReportSummary", &reports, "/reports/0/periodEnd"),
        (
            "GetReportMetadata/ReportDetail, latest version",
            &latest_metadata,
            "/reportDetails/periodStart",
        ),
        (
            "GetReportMetadata/ReportDetail, latest version",
            &latest_metadata,
            "/reportDetails/periodEnd",
        ),
        (
            "GetReportMetadata/ReportDetail, latest version",
            &latest_metadata,
            "/reportDetails/createdAt",
        ),
        (
            "GetReportMetadata/ReportDetail, latest version",
            &latest_metadata,
            "/reportDetails/lastModifiedAt",
        ),
        (
            "GetReportMetadata/ReportDetail, pinned version",
            &pinned_metadata,
            "/reportDetails/periodStart",
        ),
        (
            "GetReportMetadata/ReportDetail, pinned version",
            &pinned_metadata,
            "/reportDetails/createdAt",
        ),
        (
            "ListReportVersions/ReportSummary",
            &versions,
            "/reports/0/periodStart",
        ),
        (
            "ListReportVersions/ReportSummary",
            &versions,
            "/reports/0/periodEnd",
        ),
        (
            "ListCustomerAgreements/CustomerAgreementSummary",
            &agreements,
            "/customerAgreements/0/effectiveStart",
        ),
    ];

    for (case, body, pointer) in required {
        let value = at(body, pointer);
        assert!(
            !value.is_null(),
            "case {case:?}: {pointer} is @required, so the answer must carry it, got {body:?}"
        );
        assert_date_time(case, pointer, &value);
    }
    for (case, body, pointer) in optional {
        let value = at(body, pointer);
        if value.is_null() {
            continue;
        }
        assert_date_time(case, pointer, &value);
    }

    // A deleted report carries `deletedAt`; the catalog holds none, so the member is
    // absent rather than an epoch-zero instant.
    assert!(
        at(&latest_metadata, "/reportDetails/deletedAt").is_null(),
        "a report in the catalog is not deleted, so it reports no deletedAt, got {:?}",
        at(&latest_metadata, "/reportDetails/deletedAt")
    );
}

/// The member schema the codec reads the wire form off: every timestamp member this
/// service models carries `@timestampFormat("date-time")`, and a member the model does not
/// annotate carries none.
///
/// This is the only reach `ResponseVersion` has, the one shape here no operation of this
/// service reports — only an edited response would carry it — and it is what fails if any
/// of these schemas goes back to the vendored codegen's output.
#[test]
fn every_modelled_timestamp_member_carries_the_date_time_format() {
    // Source: `smithy_query(query="shape",shape="InquirySummary,InquiryDetail,QuerySummary,
    // ResponseVersion,ReportSummary,ReportDetail,CustomerAgreementSummary,
    // PutComplianceInquiryFeedbackResponse")` — every member of this service targeting
    // `TimestampAttribute`.
    let date_time_members: Vec<(&str, &Schema)> = vec![
        ("InquirySummary$createdAt", &InquirySummary::M_CREATED_AT),
        ("InquiryDetail$createdAt", &InquiryDetail::M_CREATED_AT),
        ("InquiryDetail$updatedAt", &InquiryDetail::M_UPDATED_AT),
        ("QuerySummary$createdAt", &QuerySummary::M_CREATED_AT),
        ("ResponseVersion$timestamp", &ResponseVersion::M_TIMESTAMP),
        ("ReportSummary$periodStart", &ReportSummary::M_PERIOD_START),
        ("ReportSummary$periodEnd", &ReportSummary::M_PERIOD_END),
        ("ReportDetail$periodStart", &ReportDetail::M_PERIOD_START),
        ("ReportDetail$periodEnd", &ReportDetail::M_PERIOD_END),
        ("ReportDetail$createdAt", &ReportDetail::M_CREATED_AT),
        (
            "ReportDetail$lastModifiedAt",
            &ReportDetail::M_LAST_MODIFIED_AT,
        ),
        ("ReportDetail$deletedAt", &ReportDetail::M_DELETED_AT),
        (
            "CustomerAgreementSummary$effectiveStart",
            &CustomerAgreementSummary::M_EFFECTIVE_START,
        ),
        (
            "CustomerAgreementSummary$effectiveEnd",
            &CustomerAgreementSummary::M_EFFECTIVE_END,
        ),
        (
            "PutComplianceInquiryFeedbackResponse$submittedAt",
            &PutComplianceInquiryFeedbackOutput::M_SUBMITTED_AT,
        ),
    ];

    for (case, schema) in date_time_members {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // A member the model does not annotate keeps the protocol default, so the fix did not
    // put a format where the model names none.
    for (case, schema) in [
        ("InquirySummary$arn", &InquirySummary::M_ARN),
        ("ReportSummary$version", &ReportSummary::M_VERSION),
        ("QuerySummary$citations", &QuerySummary::M_CITATIONS),
    ] {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: the model annotates no format for this member, got {format:?}"
        );
    }
}

/// An inquiry's terminal `status`/`statusMessage` are derived from its queries, so the
/// modelled clean-completion value is what a successfully processed inquiry reports.
///
/// The rows differ along what the rule generalises over: the form of the submitted
/// content (a single submitted query versus an uploaded document), whether the content
/// carries anything to process at all, and the submitted text.
#[tokio::test]
async fn an_inquiry_completes_cleanly_when_no_query_of_it_failed() {
    let cases: Vec<(&str, Value, &str, &str, usize)> = vec![
        (
            "one submitted query",
            text_body("Data residency", "Where is customer data stored?"),
            "COMPLETED",
            INQUIRY_COMPLETE,
            1,
        ),
        (
            "one submitted query, other text",
            text_body("Subprocessors", "Which subprocessors handle customer content?"),
            "COMPLETED",
            INQUIRY_COMPLETE,
            1,
        ),
        (
            "an uploaded document processed as one unit",
            file_body("Vendor questionnaire", FILE_CONTENT),
            "COMPLETED",
            INQUIRY_COMPLETE,
            1,
        ),
        (
            "an upload carrying no bytes, so nothing to process",
            file_body("Empty questionnaire", ""),
            "FAILED",
            INQUIRY_INTERNAL_ERROR,
            0,
        ),
    ];

    for (case, body, terminal_status, terminal_message, query_count) in cases {
        let router = emulator();
        let created = post(&router, "/v1/compliance-inquiry/create", &body).await;
        let id = at(&created, "/complianceInquirySummary/id")
            .as_str()
            .expect("the create reports the minted inquiry id")
            .to_string();

        std::thread::sleep(PAST_THE_PROCESSING_SPAN);

        let detail = get(
            &router,
            &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
        )
        .await;
        assert_eq!(
            (
                at(&detail, "/complianceInquiryDetail/status"),
                at(&detail, "/complianceInquiryDetail/statusMessage"),
            ),
            (json!(terminal_status), json!(terminal_message)),
            "case {case:?}: expected the terminal pair {terminal_status:?} / \
             {terminal_message:?}, got {detail:?}"
        );
        assert_ne!(
            at(&detail, "/complianceInquiryDetail/statusMessage"),
            json!(INQUIRY_COMPLETE_WITH_QUERY_ERRORS),
            "case {case:?}: no query of this inquiry failed, so the query-error value must \
             not be reported, got {detail:?}"
        );

        let queries = get(
            &router,
            &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={id}"),
        )
        .await;
        let reported = at(&queries, "/queries");
        let reported = reported.as_array().cloned().unwrap_or_default();
        assert_eq!(
            reported.len(),
            query_count,
            "case {case:?}: expected {query_count} queries, got {queries:?}"
        );
        for (position, query) in reported.iter().enumerate() {
            assert_eq!(
                (query["status"].clone(), query["statusMessage"].clone()),
                (
                    json!("COMPLETED"),
                    json!("Query processing is complete.")
                ),
                "case {case:?}: query {position} is reviewed by the service's own model, so \
                 its processing is over without error, got {query:?}"
            );
            assert_eq!(
                query["response"].is_null(),
                query["citations"].is_null(),
                "case {case:?}: query {position} reports a response and the citations \
                 supporting it together, got {query:?}"
            );
        }
    }
}
