// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteCertificate`: what it removes, and what the
//! account's own state holds it back for.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: a delete of a certificate takes effect only when nothing in the
//! account's state holds it back — the store holds one under the requested id, its status
//! is not `ACTIVE`, and no policy is attached to it — and otherwise refuses with the
//! modelled class for what held it back ("A certificate cannot be deleted if it has a
//! policy or IoT thing attached to it or if its status is set to ACTIVE",
//! `smithy_query(query="operation",operation="DeleteCertificate")`): an unheld id is
//! `ResourceNotFoundException` (404), an `ACTIVE` certificate is
//! `CertificateStateException` (406), and attached policies are `DeleteConflictException`
//! (409). `forceDelete` — "Forces the deletion of a certificate if it is inactive and is
//! not attached to an IoT thing" — relaxes that last refusal and detaches those policies,
//! and relaxes neither of the other two. A delete that takes effect is observable: the
//! removed certificate is gone from `DescribeCertificate` and from the policy-attachment
//! relation, and every other certificate is untouched.
//!
//! The tables span what the rule generalises over rather than one request: the three forms
//! `forceDelete` takes on the wire (absent — the modelled `default false` — explicitly
//! `false`, explicitly `true`), both statuses a created certificate can carry, several
//! counts of attached policies (none, one, three) under policy names of different forms,
//! and ids that satisfy the modelled `CertificateId` constraints without the store holding
//! them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    CERTIFICATE_STATUS_ACTIVE, CERTIFICATE_STATUS_INACTIVE,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// An IoT policy document every policy in this suite is created with: the document is not
/// what this suite is about, so one valid document serves every case.
const POLICY_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iot:*","Resource":"*"}]}"#;

/// Policy names of the forms the modelled `PolicyName` pattern `^[\w+=,.@-]+$` admits
/// (`smithy_query(query="operation",operation="CreatePolicy")`), so a case that attaches
/// several policies attaches names of several forms rather than one name repeated.
const POLICY_NAMES: [&str; 3] = ["DevicePolicy", "device.policy-2", "device_policy+3=4"];

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// the region: the scope is how the router picks the claiming service, and how the service
/// picks the region its ARNs are scoped to. The signature itself is never checked here —
/// the harness turns request authentication off, exactly as an unauthenticated emulator
/// runs.
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

    /// A string member of a reported `CertificateDescription`.
    fn described(&self, name: &str) -> Option<String> {
        self.body
            .get("certificateDescription")
            .and_then(|value| value.get(name))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `targets` of a `ListTargetsForPolicy` answer.
    fn targets(&self) -> Vec<String> {
        self.body
            .get("targets")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
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

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
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

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, with `setAsActive` bound
    /// as an `@httpQuery` parameter.
    async fn create_certificate(&self, set_as_active: bool) -> Reply {
        self.call(
            "POST",
            &format!("/keys-and-certificate?setAsActive={set_as_active}"),
            None,
        )
        .await
    }

    /// `DescribeCertificate`: `GET /certificates/{certificateId}`.
    async fn describe(&self, certificate_id: &str) -> Reply {
        self.call("GET", &format!("/certificates/{certificate_id}"), None)
            .await
    }

    /// `DeleteCertificate`: `DELETE /certificates/{certificateId}`, with `forceDelete`
    /// bound as an `@httpQuery` parameter — absent when the case names none.
    async fn delete(&self, certificate_id: &str, force_delete: Option<bool>) -> Reply {
        let path = match force_delete {
            None => format!("/certificates/{certificate_id}"),
            Some(force) => format!("/certificates/{certificate_id}?forceDelete={force}"),
        };
        self.call("DELETE", &path, None).await
    }

    /// `CreatePolicy`: `POST /policies/{policyName}`.
    async fn create_policy(&self, policy_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/policies/{policy_name}"),
            Some(&json!({ "policyDocument": POLICY_DOCUMENT })),
        )
        .await
    }

    /// `AttachPolicy`: `PUT /target-policies/{policyName}` carrying the target.
    async fn attach_policy(&self, policy_name: &str, target: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/target-policies/{policy_name}"),
            Some(&json!({ "target": target })),
        )
        .await
    }

    /// `ListTargetsForPolicy`: `POST /policy-targets/{policyName}`, which reads the
    /// policy-attachment relation from the policy's end.
    async fn list_targets_for_policy(&self, policy_name: &str) -> Reply {
        self.call("POST", &format!("/policy-targets/{policy_name}"), None)
            .await
    }

    /// A certificate issued for a case, as the pair every assertion needs: its id and its
    /// ARN.
    async fn issue(&self, case: &str, set_as_active: bool) -> (String, String) {
        let issued = self.create_certificate(set_as_active).await;
        assert_eq!(
            issued.status, 200,
            "case {case:?}: CreateKeysAndCertificate must succeed, got {issued:?}"
        );
        let id = issued.text("certificateId").unwrap_or_else(|| {
            panic!("case {case:?}: the issue must report a certificateId, got {issued:?}")
        });
        let arn = issued.text("certificateArn").unwrap_or_else(|| {
            panic!("case {case:?}: the issue must report a certificateArn, got {issued:?}")
        });
        (id, arn)
    }

    /// Attach `count` policies to `target`, creating each one first. `suffix` keeps the
    /// names of one case apart from another's, since a policy name is the policy's
    /// identity for the whole account.
    async fn attach_policies(
        &self,
        case: &str,
        target: &str,
        suffix: &str,
        count: usize,
    ) -> Vec<String> {
        let names: Vec<String> = POLICY_NAMES
            .iter()
            .take(count)
            .map(|name| format!("{name}-{suffix}"))
            .collect();
        for name in &names {
            let created = self.create_policy(name).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreatePolicy {name} must succeed, got {created:?}"
            );
            let attached = self.attach_policy(name, target).await;
            assert_eq!(
                attached.status, 200,
                "case {case:?}: AttachPolicy {name} must succeed, got {attached:?}"
            );
        }
        names
    }
}

/// The three forms `forceDelete` takes on the wire: absent (the modelled `default false`,
/// `smithy_query(query="operation",operation="DeleteCertificate")`), explicitly `false`,
/// explicitly `true`.
fn force_delete_forms() -> Vec<(&'static str, Option<bool>)> {
    vec![
        ("no forceDelete at all", None),
        ("forceDelete=false", Some(false)),
        ("forceDelete=true", Some(true)),
    ]
}

/// The message AWS IoT refuses a delete of an `ACTIVE` certificate with: fixed text, with
/// the id taken from the request.
fn active_message(certificate_id: &str) -> String {
    format!("Certificate must be deactivated (not ACTIVE) before deletion. Id: {certificate_id}")
}

/// The message AWS IoT refuses a delete of a certificate that still holds attached
/// policies with: fixed text, with the ARN taken from the stored certificate.
fn attached_message(certificate_arn: &str) -> String {
    format!("Certificate policies must be detached before deletion (arn: {certificate_arn})")
}

/// The message every reader of one certificate refuses an id the account holds none under
/// with.
fn missing_message(certificate_id: &str) -> String {
    format!("CertificateId {certificate_id} does not exist")
}

#[tokio::test]
async fn deleting_an_inactive_unattached_certificate_removes_it() {
    let emulator = Emulator::new();

    // Nothing holds an INACTIVE certificate with no attached policy back, whichever form
    // `forceDelete` takes: the delete answers 200 with an empty body and the certificate
    // stops existing for every later reader.
    for (label, force_delete) in force_delete_forms() {
        let (id, _arn) = emulator.issue(label, false).await;
        let described = emulator.describe(&id).await;
        assert_eq!(
            described.described("status").as_deref(),
            Some(CERTIFICATE_STATUS_INACTIVE),
            "case {label:?}: the certificate under test must be INACTIVE, got {described:?}"
        );

        // A second certificate exists throughout, so what the delete removed is one record
        // and not the store.
        let (bystander, _bystander_arn) = emulator.issue(label, false).await;

        let deleted = emulator.delete(&id, force_delete).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: deleting an INACTIVE unattached certificate must succeed, \
             got {deleted:?}"
        );
        assert!(
            deleted.is_empty_body(),
            "case {label:?}: DeleteCertificate models no output members, got {deleted:?}"
        );

        let read = emulator.describe(&id).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: the deleted certificate must be gone, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException for the deleted id, got {read:?}"
        );

        let survivor = emulator.describe(&bystander).await;
        assert_eq!(
            survivor.status, 200,
            "case {label:?}: every other certificate must survive the delete, got {survivor:?}"
        );

        // The delete is not idempotent: the id names nothing afterwards, so a repeat is
        // the same refusal every reader of a missing certificate gives.
        let again = emulator.delete(&id, force_delete).await;
        assert_eq!(
            again.status, 404,
            "case {label:?}: deleting the same id twice must be refused 404, got {again:?}"
        );
        assert_eq!(
            again.message(),
            missing_message(&id),
            "case {label:?}: expected {:?}, got {again:?}",
            missing_message(&id)
        );
    }
}

#[tokio::test]
async fn deleting_an_active_certificate_is_refused_certificate_state() {
    let emulator = Emulator::new();

    // "A certificate cannot be deleted ... if its status is set to ACTIVE", and
    // `forceDelete` is documented to force the deletion only of a certificate that "is
    // inactive", so it relaxes nothing here.
    for (label, force_delete) in force_delete_forms() {
        let (id, _arn) = emulator.issue(label, true).await;
        let described = emulator.describe(&id).await;
        assert_eq!(
            described.described("status").as_deref(),
            Some(CERTIFICATE_STATUS_ACTIVE),
            "case {label:?}: the certificate under test must be ACTIVE, got {described:?}"
        );

        let refused = emulator.delete(&id, force_delete).await;
        assert_eq!(
            refused.status, 406,
            "case {label:?}: deleting an ACTIVE certificate must be refused 406, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("CertificateStateException"),
            "case {label:?}: expected CertificateStateException, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            active_message(&id),
            "case {label:?}: expected {:?}, got {refused:?}",
            active_message(&id)
        );

        // The refusal wrote nothing: the certificate is still there, unchanged.
        let read = emulator.describe(&id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: a refused delete must leave the certificate readable, got {read:?}"
        );
        assert_eq!(
            read.body, described.body,
            "case {label:?}: a refused delete must change nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_certificate_with_attached_policies_is_refused_delete_conflict() {
    let emulator = Emulator::new();

    // "A certificate cannot be deleted if it has a policy ... attached to it", whatever
    // the count: one attachment holds the delete back exactly as three do, and
    // `forceDelete` absent is the modelled `false`.
    for policy_count in [1usize, 2, 3] {
        for (form, force_delete) in [("no forceDelete at all", None), ("forceDelete=false", Some(false))] {
            let label = format!("{policy_count} attached policies, {form}");
            let (id, arn) = emulator.issue(&label, false).await;
            let policies = emulator
                .attach_policies(&label, &arn, &id[..8], policy_count)
                .await;

            let refused = emulator.delete(&id, force_delete).await;
            assert_eq!(
                refused.status, 409,
                "case {label:?}: an attached policy must refuse the delete 409, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("DeleteConflictException"),
                "case {label:?}: expected DeleteConflictException, got {refused:?}"
            );
            assert_eq!(
                refused.message(),
                attached_message(&arn),
                "case {label:?}: expected {:?}, got {refused:?}",
                attached_message(&arn)
            );

            // The refusal wrote nothing: the certificate is still readable and every
            // attachment is still recorded.
            let read = emulator.describe(&id).await;
            assert_eq!(
                read.status, 200,
                "case {label:?}: a refused delete must leave the certificate readable, got {read:?}"
            );
            for policy in &policies {
                let targets = emulator.list_targets_for_policy(policy).await;
                assert!(
                    targets.targets().contains(&arn),
                    "case {label:?}: {policy} must still name {arn} as a target, got {targets:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn force_deleting_an_inactive_certificate_detaches_its_policies() {
    let emulator = Emulator::new();

    // `forceDelete` "Forces the deletion of a certificate if it is inactive and is not
    // attached to an IoT thing": the attached policies are what it relaxes, and the
    // relation stops naming the certificate for the same reason the store does.
    for policy_count in [1usize, 2, 3] {
        let label = format!("forceDelete=true with {policy_count} attached policies");
        let (id, arn) = emulator.issue(&label, false).await;
        let policies = emulator
            .attach_policies(&label, &arn, &id[..8], policy_count)
            .await;

        let deleted = emulator.delete(&id, Some(true)).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: a forced delete of an INACTIVE certificate must succeed, \
             got {deleted:?}"
        );

        let read = emulator.describe(&id).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: the force-deleted certificate must be gone, got {read:?}"
        );
        for policy in &policies {
            let targets = emulator.list_targets_for_policy(policy).await;
            assert_eq!(
                targets.status, 200,
                "case {label:?}: {policy} must still exist after the delete, got {targets:?}"
            );
            assert!(
                !targets.targets().contains(&arn),
                "case {label:?}: {policy} must no longer name {arn} as a target, got {targets:?}"
            );
        }
    }
}

#[tokio::test]
async fn force_deleting_an_active_certificate_is_still_refused() {
    let emulator = Emulator::new();

    // The force applies to a certificate that "is inactive", so an ACTIVE certificate
    // with attached policies is refused for its STATUS and not for the attachments: the
    // status rule is read first, and the attachments are still recorded afterwards.
    let label = "forceDelete=true on an ACTIVE certificate holding a policy";
    let (id, arn) = emulator.issue(label, true).await;
    let policies = emulator.attach_policies(label, &arn, &id[..8], 1).await;

    let refused = emulator.delete(&id, Some(true)).await;
    assert_eq!(
        refused.status, 406,
        "case {label:?}: an ACTIVE certificate must be refused 406 even forced, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("CertificateStateException"),
        "case {label:?}: expected CertificateStateException, got {refused:?}"
    );
    assert_eq!(
        refused.message(),
        active_message(&id),
        "case {label:?}: expected {:?}, got {refused:?}",
        active_message(&id)
    );
    for policy in &policies {
        let targets = emulator.list_targets_for_policy(policy).await;
        assert!(
            targets.targets().contains(&arn),
            "case {label:?}: a refused forced delete must detach nothing, got {targets:?}"
        );
    }
}

#[tokio::test]
async fn deleting_an_unissued_certificate_is_refused_not_found() {
    let emulator = Emulator::new();

    // One certificate exists, so the refusals below are about the id and not about an
    // empty store.
    let (existing, _arn) = emulator.issue("the certificate that exists", false).await;

    // `CertificateId` is modelled `length 64..64, pattern ^(0x)?[a-fA-F0-9]+$`
    // (`smithy_query(query="operation",operation="DeleteCertificate")`), so every id below
    // is a legal one the store simply does not hold: the lower and upper hex digits, the
    // `0x` prefix the pattern allows, and the existing id with its last character changed.
    let mut neighbour = existing.clone();
    neighbour.pop();
    neighbour.push(if existing.ends_with('0') { '1' } else { '0' });
    for id in [
        "0".repeat(64),
        "f".repeat(64),
        "ABCDEF".repeat(10) + "abcd",
        format!("0x{}", "a".repeat(62)),
        neighbour,
    ] {
        for (form, force_delete) in force_delete_forms() {
            let label = format!("{id} with {form}");
            let refused = emulator.delete(&id, force_delete).await;
            assert_eq!(
                refused.status, 404,
                "case {label:?}: deleting an id the store does not hold must be refused 404, \
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

    // The certificate that does exist is still there: the refusals wrote nothing.
    let read = emulator.describe(&existing).await;
    assert_eq!(
        read.status, 200,
        "the issued certificate must still be readable, got {read:?}"
    );
}
