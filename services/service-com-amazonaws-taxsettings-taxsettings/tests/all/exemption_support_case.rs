// SPDX-License-Identifier: CC0-1.0

//! The AWS Support case a tax-exemption submission opens, end to end over the
//! wire — with the AWS Support emulator mounted beside this service on the one
//! shared engine.
//!
//! RULE: a `PutTaxExemption` submission opens ONE AWS Support case in the control
//! plane that owns support cases, with the submitted certificate attached to that
//! case's first communication, and the `caseId` it answers is that case's own id
//! — "The customer support case ID" (`smithy_query(query="doc",
//! shape="PutTaxExemptionResponse", member="caseId")`), the handle a client takes
//! to `support:DescribeCases` and `support:DescribeCommunications` to follow the
//! review of the document it submitted. A submission that is refused opens no
//! case and uploads nothing.
//!
//! The rows span what the rule generalises over: how many accounts a submission
//! addresses (one, several, the same id twice), how it names its exemption type (a
//! published description, a published display name, a token the collection does
//! not publish), whether the jurisdiction carries a state, and the certificate
//! itself — each extension the modelled `documentName` pattern admits, at sizes
//! from the one byte the modelled `documentFile` blob's `@length 1..4194304` lower
//! bound admits up to tens of kilobytes.
//!
//! What a correct case id looks like is NOT written down here: the row's id is
//! compared against one the Support emulator mints through its own `CreateCase`,
//! so the assertion is that both ids are of one form rather than of a form this
//! suite invented.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_support_awssupport_20130415::state::{
    ComAmazonawsSupportAwssupport20130415State, STATUS_OPENED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_registration_lifecycle::{call, field_list, mount_on, Answer};

/// The `X-Amz-Target` prefix the AWS Support emulator routes on (that crate's
/// `OPERATIONS.md`, "Wire protocol").
const SUPPORT_TARGET_PREFIX: &str = "AWSSupport_20130415";

/// A Support plan that may call the Support API, so these suites can READ the
/// case a submission opened: the Support API is served only to an account whose
/// plan qualifies, and that crate's `QUALIFYING_SUPPORT_PLANS` names this one.
const SUBSCRIBED_PLAN: &str = "enterprise";

/// The id form `PutTaxExemption` answered before it opened a case at all: a token
/// of this service's own making, which names no case anywhere. Held here so a row
/// can show the difference the rule is about.
const AN_INVENTED_CASE_ID: &str = "tax-exemption-case-00000001";

/// This service and the AWS Support control plane on one engine.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// Both services mounted on one fresh engine, with a Support plan recorded
    /// for the account the way an operator records one — before any request runs.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = mount_on(&engine);
        router.register_service(
            service_com_amazonaws_support_awssupport_20130415::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        let emulator = Self { engine, router };
        emulator.support_state().lock().support_plan = Some(SUBSCRIBED_PLAN.to_string());
        emulator
    }

    /// The AWS Support store on the shared engine — the one place a support case
    /// lives.
    fn support_state(&self) -> engine::ServiceState<ComAmazonawsSupportAwssupport20130415State> {
        service_com_amazonaws_support_awssupport_20130415::state::service_state(&*self.engine)
            .expect("the support store is reachable from the shared in-memory engine")
    }

    /// Call one TaxSettings operation.
    async fn tax(&self, operation: &str, body: &Value) -> Answer {
        call(&self.router, operation, body).await
    }

    /// Call one AWS Support operation, over THAT service's wire protocol
    /// (`awsJson1_1`, routed by `X-Amz-Target`).
    async fn support(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "support.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("{SUPPORT_TARGET_PREFIX}.{operation}"),
            )
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/support/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=00",
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the test body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call one AWS Support operation and require a 200.
    async fn support_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self.support(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected support:{operation} to answer 200, got {status} with \
             {answered}"
        );
        answered
    }
}

/// One submission of the table: what it names, and the certificate it submits.
struct Submission {
    case: &'static str,
    accounts: &'static [&'static str],
    exemption_type: &'static str,
    country: &'static str,
    state: Option<&'static str>,
    /// `ExemptionCertificate$documentName`, whose modelled pattern admits a
    /// `.pdf`, `.jpg` or `.png` suffix in either case
    /// (`smithy_query(query="shape", shape="ExemptionCertificate")`).
    document_name: &'static str,
    /// How many bytes the certificate carries. `documentFile` is a blob of
    /// `@length 1..4194304`, so one byte is the smallest document the model
    /// admits.
    document_bytes: usize,
}

impl Submission {
    /// The certificate's content: bytes derived from the row, so no two rows
    /// submit the same document and every byte read back can be checked.
    fn document(&self) -> Vec<u8> {
        self.document_name
            .bytes()
            .cycle()
            .take(self.document_bytes)
            .collect()
    }

    /// The request body, with the certificate base64-encoded as restJson1
    /// carries a blob.
    fn body(&self) -> Value {
        let mut authority = json!({ "country": self.country });
        if let Some(state) = self.state {
            authority["state"] = json!(state);
        }
        json!({
            "accountIds": self.accounts,
            "authority": authority,
            "exemptionType": self.exemption_type,
            "exemptionCertificate": {
                "documentName": self.document_name,
                "documentFile": ::aws_smithy_types::base64::encode(self.document().as_slice()),
            },
        })
    }
}

/// The submissions the rule is asserted over.
const SUBMISSIONS: &[Submission] = &[
    Submission {
        case: "one account, a published description, a country-wide jurisdiction",
        accounts: &["000000000001"],
        exemption_type: "Purchases for resale, held under a resale certificate",
        country: "US",
        state: None,
        document_name: "resale.pdf",
        document_bytes: 1,
    },
    Submission {
        case: "several accounts, one of them named twice, a published display name in a state",
        accounts: &["000000000002", "000000000003", "000000000002"],
        exemption_type: "Reseller",
        country: "US",
        state: Some("OH"),
        document_name: "resale-certificate.PDF",
        document_bytes: 4_096,
    },
    Submission {
        case: "a type the published collection does not carry, outside the United States",
        accounts: &["999999999999"],
        exemption_type: "Diplomatic Mission",
        country: "GB",
        state: None,
        document_name: "mission.jpg",
        document_bytes: 65_536,
    },
    Submission {
        case: "a padded, re-cased published display name and a PNG certificate",
        accounts: &["000000000004", "000000000005"],
        exemption_type: "  fEDERAL gOVERNMENT  ",
        country: "us",
        state: Some("TX"),
        document_name: "federal_exempt.PNG",
        document_bytes: 2,
    },
];

/// The segments of a case id, for comparing one id's form with another's.
fn segments(case_id: &str) -> Vec<&str> {
    case_id.split('-').collect()
}

/// Assert that `case_id` is of the same form as `minted`, an id the Support
/// emulator minted itself: the same number of segments, the same leading and
/// account and realm segments, a four-digit year and a hexadecimal tail of the
/// same width.
fn assert_same_form(case: &str, case_id: &str, minted: &str) {
    let got = segments(case_id);
    let reference = segments(minted);
    assert_eq!(
        got.len(),
        reference.len(),
        "case {case:?}: expected a case id of the form support mints ({minted:?}), got \
         {case_id:?}"
    );
    for (index, expected) in reference.iter().enumerate().take(3) {
        assert_eq!(
            got.get(index),
            Some(expected),
            "case {case:?}: expected segment {index} of {case_id:?} to match the support-minted \
             {minted:?}"
        );
    }
    let year = got.get(3).copied().unwrap_or_default();
    assert!(
        year.len() == 4 && year.chars().all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected a four-digit year segment in {case_id:?}, got {year:?}"
    );
    let tail = got.last().copied().unwrap_or_default();
    let reference_tail = reference.last().copied().unwrap_or_default();
    assert!(
        tail.len() == reference_tail.len() && tail.chars().all(|c| c.is_ascii_hexdigit()),
        "case {case:?}: expected a {}-character hexadecimal tail in {case_id:?}, got {tail:?}",
        reference_tail.len()
    );
}

/// The communications a `DescribeCases` entry carries, newest first.
fn recent_communications(case: &Value) -> Vec<Value> {
    case.get("recentCommunications")
        .and_then(|recent| recent.get("communications"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `(attachmentId, fileName)` pairs a communication reports, read from the
/// member named by `member` — `attachments` carries every attachment and
/// `attachmentSet` those 5 MB or smaller, and an exemption certificate is at most
/// 4 MiB, so both must report it.
fn attachments(communication: &Value, member: &str) -> Vec<(String, String)> {
    communication
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|attachment| {
            (
                attachment
                    .get("attachmentId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                attachment
                    .get("fileName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// RULE: every applied submission opens its own AWS Support case, with its
/// certificate attached, and answers that case's id — which `support:DescribeCases`
/// and `support:DescribeCommunications` then resolve.
#[tokio::test]
async fn a_submission_opens_a_support_case_the_support_api_resolves() {
    let emulator = Emulator::new();

    // The reference id: one the Support emulator minted through its own
    // `CreateCase`, so the form every row is judged against is that service's
    // rather than one written out here.
    let created = emulator
        .support_ok(
            "the reference case",
            "CreateCase",
            &json!({
                "subject": "a case the account opened itself",
                "communicationBody": "the body that describes the issue",
            }),
        )
        .await;
    let minted = created
        .get("caseId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateCase must answer a caseId, got {created}"))
        .to_string();

    let mut opened: Vec<String> = Vec::new();
    for submission in SUBMISSIONS {
        let case = submission.case;
        let answer = emulator.tax("PutTaxExemption", &submission.body()).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the submission to be applied, got {answer:?}"
        );
        let case_id = answer
            .get("caseId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a caseId, got {answer:?}"))
            .to_string();

        // The id is of the form the Support emulator mints, and is its own.
        assert_same_form(case, &case_id, &minted);
        assert!(
            !opened.contains(&case_id) && case_id != minted,
            "case {case:?}: every applied submission opens its own case, got {case_id:?} twice"
        );
        opened.push(case_id.clone());

        // It resolves in AWS Support: the case the submission opened.
        let described = emulator
            .support_ok(case, "DescribeCases", &json!({ "caseIdList": [&case_id] }))
            .await;
        let cases = described
            .get("cases")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            cases.len(),
            1,
            "case {case:?}: expected support to hold the one case {case_id:?}, got {described}"
        );
        let support_case = &cases[0];
        assert_eq!(
            support_case.get("caseId").and_then(Value::as_str),
            Some(case_id.as_str()),
            "case {case:?}: expected the case the id names, got {support_case}"
        );
        assert_eq!(
            support_case.get("status").and_then(Value::as_str),
            Some(STATUS_OPENED),
            "case {case:?}: a submitted certificate awaits review, got {support_case}"
        );
        let subject = support_case
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            subject.contains(submission.exemption_type.trim()),
            "case {case:?}: expected the case subject to name the exemption submitted, got \
             {subject:?}"
        );

        // The certificate is attached to the case's first communication, under
        // the name the request gave it, and both attachment members report it.
        let communications = recent_communications(support_case);
        assert_eq!(
            communications.len(),
            1,
            "case {case:?}: expected the one communication the submission opened the case with, \
             got {communications:?}"
        );
        let body = communications[0]
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            body.contains(submission.document_name),
            "case {case:?}: expected the case body to name the submitted document, got {body:?}"
        );
        let mut attachment_id = String::new();
        for member in ["attachments", "attachmentSet"] {
            let reported = attachments(&communications[0], member);
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: expected {member} to carry the submitted certificate, got \
                 {reported:?}"
            );
            let (id, file_name) = &reported[0];
            assert_eq!(
                file_name, submission.document_name,
                "case {case:?}: expected {member} to name the submitted file, got {reported:?}"
            );
            assert!(
                !id.is_empty(),
                "case {case:?}: expected {member} to carry an attachmentId, got {reported:?}"
            );
            attachment_id = id.clone();
        }

        // `DescribeCommunications` reports the same communication and the same
        // attachment, which is the other operation a client follows the review
        // through.
        let page = emulator
            .support_ok(
                case,
                "DescribeCommunications",
                &json!({ "caseId": &case_id }),
            )
            .await;
        let listed = page
            .get("communications")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected the one communication, got {page}"
        );
        assert_eq!(
            attachments(&listed[0], "attachmentSet"),
            vec![(attachment_id.clone(), submission.document_name.to_string())],
            "case {case:?}: expected DescribeCommunications to report the same attachment, got \
             {page}"
        );

        // The document itself is RETAINED, byte for byte, under that id: the
        // content `DescribeAttachment` serves from the case.
        let store = emulator.support_state();
        let store = store.lock();
        let stored = store
            .cases
            .get(&case_id)
            .and_then(|held| held.communications.front().cloned())
            .unwrap_or_else(|| panic!("case {case:?}: the support store holds the case it opened"));
        let files: Vec<(String, usize)> = stored
            .attachments
            .iter()
            .map(|attachment| (attachment.attachment_id.clone(), attachment.data.len()))
            .collect();
        assert_eq!(
            files,
            vec![(attachment_id.clone(), submission.document_bytes)],
            "case {case:?}: expected the submitted document retained whole, got {files:?}"
        );
        assert!(
            stored
                .attachments
                .first()
                .is_some_and(|attachment| attachment.data == submission.document()),
            "case {case:?}: expected the retained content to be the bytes submitted, got {} \
             byte(s) under {attachment_id:?}",
            stored.attachments.first().map_or(0, |a| a.data.len())
        );
    }

    // Every case the submissions opened is in AWS Support beside the one the
    // account opened itself, and nothing else is.
    let all = emulator
        .support_ok("every case support holds", "DescribeCases", &json!({}))
        .await;
    let mut held: Vec<String> = all
        .get("cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|case| case.get("caseId").and_then(Value::as_str))
        .map(std::string::ToString::to_string)
        .collect();
    held.sort();
    let mut expected: Vec<String> = opened.clone();
    expected.push(minted);
    expected.sort();
    assert_eq!(
        held, expected,
        "case \"every case support holds\": expected one case per applied submission plus the \
         account's own, got {held:?}"
    );

    // And an id of the kind this service used to invent for itself names no case
    // at all — which is why the id it answers must be the store's own.
    let (status, refused) = emulator
        .support(
            "DescribeCases",
            &json!({ "caseIdList": [AN_INVENTED_CASE_ID] }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"an id support never minted\": expected the case-id refusal, got {status} with \
         {refused}"
    );
}

/// One refused submission: what it names, and the refusal it meets.
struct Refusal {
    case: &'static str,
    exemption_type: &'static str,
    authority: fn() -> Value,
}

/// The refusals the rule is asserted over: a published type claimed where the
/// collection does not publish it, and the blank `state` the model's
/// `Authority$state` pattern forbids.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a published type claimed in a state no entry names",
        exemption_type: "Reseller",
        authority: || json!({ "country": "US", "state": "AK" }),
    },
    Refusal {
        case: "a blank state the model's pattern forbids",
        exemption_type: "Diplomatic Mission",
        authority: || json!({ "country": "US", "state": "   " }),
    },
];

/// RULE: a submission that is refused opens NO support case and uploads nothing,
/// whatever it is refused for — so a client cannot be told a certificate is under
/// review on a case that was never created.
#[tokio::test]
async fn a_refused_submission_opens_no_support_case() {
    for refusal in REFUSALS {
        let case = refusal.case;
        let emulator = Emulator::new();
        let answer = emulator
            .tax(
                "PutTaxExemption",
                &json!({
                    "accountIds": [engine::emulator_account_id()],
                    "authority": (refusal.authority)(),
                    "exemptionType": refusal.exemption_type,
                    "exemptionCertificate": {
                        "documentName": "resale.pdf",
                        "documentFile": ::aws_smithy_types::base64::encode("%PDF-1.4 refused"),
                    },
                }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the submission to be refused, got {answer:?}"
        );
        assert!(
            !field_list(&answer).is_empty() || answer.get("message").is_string(),
            "case {case:?}: expected the refusal to state what is at fault, got {:?}",
            answer.body
        );
        assert!(
            answer.get("caseId").is_null(),
            "case {case:?}: a refused submission answers no caseId, got {:?}",
            answer.body
        );

        let described = emulator
            .support_ok(case, "DescribeCases", &json!({}))
            .await;
        let held = described
            .get("cases")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            held.is_empty(),
            "case {case:?}: a refused submission opens no support case, got {described}"
        );
        let retained = emulator.support_state().lock().retained_attachment_bytes();
        assert_eq!(
            retained, 0,
            "case {case:?}: a refused submission uploads no document, got {retained} byte(s) \
             retained"
        );
    }
}
