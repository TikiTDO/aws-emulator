// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateCertificate`: the status a caller may put a
//! certificate in, what that changes about the stored record, and what it lets
//! `DeleteCertificate` then do.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: "Updates the status of the specified certificate. This operation is
//! idempotent" (`smithy_query(query="operation",operation="UpdateCertificate")`) — so the
//! status is the only thing the call changes, the record reports it to every later reader,
//! a call that names the status the certificate already carries changes nothing at all, and
//! a changing call moves the certificate to its next `customerVersion` and stamps it as
//! last modified. A status AWS IoT uses internally is refused `InvalidRequestException`
//! (400) — "Setting the status to PENDING_TRANSFER or PENDING_ACTIVATION will result in an
//! exception being thrown" — and an id the account holds no certificate under is refused
//! `ResourceNotFoundException` (404).
//!
//! The tables span what the rule generalises over rather than one request: every status
//! the modelled `CertificateStatus` enum carries
//! (`smithy_query(query="shape",shape="CertificateStatus")`), from each of the two
//! statuses a created certificate can start in, and ids that satisfy the modelled
//! `CertificateId` constraints without the store holding them.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use service_com_amazonaws_iot_awsiotservice::state::{
    CERTIFICATE_STATUS_ACTIVE, CERTIFICATE_STATUS_INACTIVE, CERTIFICATE_STATUS_REVOKED,
    FIRST_CERTIFICATE_CUSTOMER_VERSION, INTERNAL_CERTIFICATE_STATUSES,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// The deprecated status the modelled enum still carries: "The status value
/// REGISTER_INACTIVE is deprecated and should not be used"
/// (`UpdateCertificate$newStatus`) — deprecated is not refused, so a caller that names it
/// is served.
const CERTIFICATE_STATUS_REGISTER_INACTIVE: &str = "REGISTER_INACTIVE";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// the region. The signature itself is never checked here — the harness turns request
/// authentication off, exactly as an unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `message` of an error body.
    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }

    /// A member of the reported `CertificateDescription`.
    fn member(&self, name: &str) -> Option<&Value> {
        self.body
            .get("certificateDescription")
            .and_then(|value| value.get(name))
    }

    /// A string member of the reported `CertificateDescription`.
    fn member_str(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// Whether the body carries no member at all — what an operation whose output shape is
    /// `Unit` answers with.
    fn is_empty_body(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::new())
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`.
    async fn issue(&self, case: &str, set_as_active: bool) -> String {
        let issued = self
            .call(
                "POST",
                &format!("/keys-and-certificate?setAsActive={set_as_active}"),
            )
            .await;
        assert_eq!(
            issued.status, 200,
            "case {case:?}: CreateKeysAndCertificate must succeed, got {issued:?}"
        );
        issued.text("certificateId").unwrap_or_else(|| {
            panic!("case {case:?}: the issue must report a certificateId, got {issued:?}")
        })
    }

    /// `DescribeCertificate`: `GET /certificates/{certificateId}`.
    async fn describe(&self, certificate_id: &str) -> Reply {
        self.call("GET", &format!("/certificates/{certificate_id}"))
            .await
    }

    /// `UpdateCertificate`: `PUT /certificates/{certificateId}`, with `newStatus` bound as
    /// an `@httpQuery` parameter.
    async fn update(&self, certificate_id: &str, new_status: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/certificates/{certificate_id}?newStatus={new_status}"),
        )
        .await
    }

    /// `DeleteCertificate`: `DELETE /certificates/{certificateId}`.
    async fn delete(&self, certificate_id: &str) -> Reply {
        self.call("DELETE", &format!("/certificates/{certificate_id}"))
            .await
    }
}

/// The statuses a CALLER may put a certificate in: every value of the modelled
/// `CertificateStatus` enum (`smithy_query(query="shape",shape="CertificateStatus")`) less
/// the two the same model documents as internal to AWS IoT.
fn settable_statuses() -> Vec<&'static str> {
    vec![
        CERTIFICATE_STATUS_ACTIVE,
        CERTIFICATE_STATUS_INACTIVE,
        CERTIFICATE_STATUS_REVOKED,
        CERTIFICATE_STATUS_REGISTER_INACTIVE,
    ]
}

/// The message every reader of one certificate refuses an id the account holds none under
/// with.
fn missing_message(certificate_id: &str) -> String {
    format!("CertificateId {certificate_id} does not exist")
}

/// Epoch seconds now, for bounding a reported timestamp.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

#[tokio::test]
async fn updating_a_certificate_records_the_new_status() {
    let emulator = Emulator::new();

    // Every settable status, reached from each of the two statuses a created certificate
    // can start in: the status is what the call changes, and `DescribeCertificate` reports
    // it afterwards.
    for set_as_active in [false, true] {
        let started_in = if set_as_active {
            CERTIFICATE_STATUS_ACTIVE
        } else {
            CERTIFICATE_STATUS_INACTIVE
        };
        for new_status in settable_statuses() {
            let label = format!("{started_in} -> {new_status}");
            let id = emulator.issue(&label, set_as_active).await;
            let before = emulator.describe(&id).await;
            assert_eq!(
                before.member_str("status"),
                Some(started_in),
                "case {label:?}: the certificate must start in {started_in}, got {before:?}"
            );
            let created_at = before
                .member("creationDate")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: the description reported no creationDate: {before:?}")
                });

            let updated = emulator.update(&id, new_status).await;
            assert_eq!(
                updated.status, 200,
                "case {label:?}: UpdateCertificate must succeed, got {updated:?}"
            );
            assert!(
                updated.is_empty_body(),
                "case {label:?}: UpdateCertificate models no output members, got {updated:?}"
            );

            let after = emulator.describe(&id).await;
            assert_eq!(
                after.member_str("status"),
                Some(new_status),
                "case {label:?}: the description must report {new_status}, got {after:?}"
            );
            // Only the status moves: the certificate itself, its ARN and its id are the
            // ones the issue minted.
            for unchanged in ["certificateId", "certificateArn", "certificatePem", "ownedBy"] {
                assert_eq!(
                    after.member_str(unchanged),
                    before.member_str(unchanged),
                    "case {label:?}: {unchanged} must not change, got {after:?}"
                );
            }

            // A call that MOVED the status is a customer-driven change: it takes the
            // certificate to its next customer version and stamps it as last modified. A
            // call that named the status it already had changed nothing.
            let moved = new_status != started_in;
            let expected_version = if moved {
                i64::from(FIRST_CERTIFICATE_CUSTOMER_VERSION) + 1
            } else {
                i64::from(FIRST_CERTIFICATE_CUSTOMER_VERSION)
            };
            assert_eq!(
                after.member("customerVersion").and_then(Value::as_i64),
                Some(expected_version),
                "case {label:?}: expected customerVersion {expected_version}, got {after:?}"
            );
            let modified_at = after
                .member("lastModifiedDate")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: the description reported no lastModifiedDate: {after:?}")
                });
            assert!(
                modified_at >= created_at && modified_at <= now_epoch_seconds() + 1.0,
                "case {label:?}: lastModifiedDate {modified_at} must be at or after the \
                 creation {created_at} and not in the future, got {after:?}"
            );
            if !moved {
                assert_eq!(
                    after.body, before.body,
                    "case {label:?}: naming the status the certificate already carries must \
                     change nothing, got {after:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn updating_a_certificate_is_idempotent() {
    let emulator = Emulator::new();

    // "This operation is idempotent": the second call to the same status leaves the record
    // exactly as the first one left it — the customer version and the last-modified
    // instant included.
    for new_status in settable_statuses() {
        let label = format!("twice to {new_status}");
        let id = emulator.issue(&label, true).await;

        let first = emulator.update(&id, new_status).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first update must succeed, got {first:?}"
        );
        let once = emulator.describe(&id).await;

        let second = emulator.update(&id, new_status).await;
        assert_eq!(
            second.status, 200,
            "case {label:?}: repeating the update must succeed, got {second:?}"
        );
        let twice = emulator.describe(&id).await;

        assert_eq!(
            twice.body, once.body,
            "case {label:?}: a repeated update must change nothing, got {once:?} then {twice:?}"
        );
    }
}

#[tokio::test]
async fn updating_a_certificate_to_an_internal_status_is_refused() {
    let emulator = Emulator::new();

    // "PENDING_TRANSFER and PENDING_ACTIVATION are statuses used internally by IoT. They
    // are not intended for developer use": the request is refused and the certificate is
    // left exactly as it was, whichever status it was in.
    for set_as_active in [false, true] {
        for new_status in INTERNAL_CERTIFICATE_STATUSES {
            let label = format!("setAsActive={set_as_active} -> {new_status}");
            let id = emulator.issue(&label, set_as_active).await;
            let before = emulator.describe(&id).await;

            let refused = emulator.update(&id, new_status).await;
            assert_eq!(
                refused.status, 400,
                "case {label:?}: an internal status must be refused 400, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("InvalidRequestException"),
                "case {label:?}: expected InvalidRequestException, got {refused:?}"
            );
            assert!(
                refused.message().contains(new_status),
                "case {label:?}: the refusal must name the status it refused, got {refused:?}"
            );

            let after = emulator.describe(&id).await;
            assert_eq!(
                after.body, before.body,
                "case {label:?}: a refused update must change nothing, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn updating_an_unissued_certificate_is_refused_not_found() {
    let emulator = Emulator::new();

    // One certificate exists, so the refusals below are about the id and not about an
    // empty store.
    let existing = emulator.issue("the certificate that exists", false).await;

    // `CertificateId` is modelled `length 64..64, pattern ^(0x)?[a-fA-F0-9]+$`
    // (`smithy_query(query="operation",operation="UpdateCertificate")`), so every id below
    // is a legal one the store simply does not hold.
    let mut neighbour = existing.clone();
    neighbour.pop();
    neighbour.push(if existing.ends_with('0') { '1' } else { '0' });
    for id in [
        "0".repeat(64),
        "f".repeat(64),
        format!("0x{}", "b".repeat(62)),
        neighbour,
    ] {
        for new_status in settable_statuses() {
            let label = format!("{id} -> {new_status}");
            let refused = emulator.update(&id, new_status).await;
            assert_eq!(
                refused.status, 404,
                "case {label:?}: an id the store does not hold must be refused 404, \
                 got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {label:?}: expected ResourceNotFoundException, got {refused:?}"
            );
            assert_eq!(
                refused.message(),
                missing_message(&id),
                "case {label:?}: expected {:?}, got {refused:?}",
                missing_message(&id)
            );
        }
    }

    // The certificate that does exist is untouched: the refusals wrote nothing.
    let read = emulator.describe(&existing).await;
    assert_eq!(
        read.member_str("status"),
        Some(CERTIFICATE_STATUS_INACTIVE),
        "the issued certificate must be as it was issued, got {read:?}"
    );
}

#[tokio::test]
async fn deactivating_a_certificate_is_what_lets_it_be_deleted() {
    let emulator = Emulator::new();

    // The two operations meet here: "To delete a certificate ... use the UpdateCertificate
    // action to set the certificate to the INACTIVE status" (`DeleteCertificate`). Every
    // status that is not ACTIVE admits the delete, and moving back to ACTIVE refuses it
    // again — so what decides the delete is the status the store holds, not the order the
    // calls came in.
    for new_status in settable_statuses() {
        let label = format!("ACTIVE -> {new_status} -> delete");
        let id = emulator.issue(&label, true).await;

        let refused = emulator.delete(&id).await;
        assert_eq!(
            refused.status, 406,
            "case {label:?}: an ACTIVE certificate must not be deletable, got {refused:?}"
        );

        let updated = emulator.update(&id, new_status).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: UpdateCertificate must succeed, got {updated:?}"
        );

        let deleted = emulator.delete(&id).await;
        let expected_status = if new_status == CERTIFICATE_STATUS_ACTIVE {
            406
        } else {
            200
        };
        assert_eq!(
            deleted.status, expected_status,
            "case {label:?}: expected the delete to answer {expected_status}, got {deleted:?}"
        );

        let read = emulator.describe(&id).await;
        let expected_read = if new_status == CERTIFICATE_STATUS_ACTIVE {
            200
        } else {
            404
        };
        assert_eq!(
            read.status, expected_read,
            "case {label:?}: expected DescribeCertificate to answer {expected_read} after the \
             delete, got {read:?}"
        );
    }
}
