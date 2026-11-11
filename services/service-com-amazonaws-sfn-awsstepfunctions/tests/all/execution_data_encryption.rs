// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KMS key policy a read of KMS-encrypted Step Functions
//! data is decided against, driven over both services' own wire protocols through the
//! production `MultiServiceRouter` with Step Functions and KMS mounted on ONE engine
//! — the shape `core/router/src/lib.rs` mounts them in — so the key policy a read is
//! judged against is the document KMS's own handlers stored.
//!
//! The rule under test: data of a resource whose `encryptionConfiguration` is
//! `CUSTOMER_MANAGED_KMS_KEY` is sealed under the key the account named, so an
//! operation that REPORTS such data decrypts it as the CALLER and is refused with
//! HTTP 400 `KmsAccessDeniedException` whenever the key's own policy does not allow
//! that caller `kms:Decrypt` — and serves the data again as soon as the policy does.
//! A read that reports none of the encrypted data decrypts nothing, so it needs no
//! grant, and a resource on an Amazon Web Services owned key needs none either.
//!
//! The rows span the readers the rule covers (the execution's data, and the
//! definition of its state machine, reached by execution ARN and by state machine
//! ARN), the two encryption types, the KIND of caller (the account's own published
//! credential, and an IAM user carrying an identity policy that allows the Step
//! Functions read and nothing of KMS) and — for each reader that can ask for less
//! than the whole datum — the request that decrypts nothing.
//!
//! The principal is made with IAM's OWN operations over the same engine, so the
//! identity policy the decision reads is the document IAM's handlers wrote.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{
    error_code, string, ACCOUNT, HELLO_DEFINITION, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN,
};

/// How one service's requests are addressed on the wire, from each model's own
/// `service_info` (service shape, `sigv4Name` and the `awsJson` version its protocol
/// declares).
pub(crate) struct Wire {
    pub(crate) signing_name: &'static str,
    pub(crate) target_prefix: &'static str,
    pub(crate) json_version: &'static str,
}

pub(crate) const SFN: Wire = Wire {
    signing_name: "states",
    target_prefix: "AWSStepFunctions",
    json_version: "1.0",
};
pub(crate) const KMS: Wire = Wire {
    signing_name: "kms",
    target_prefix: "TrentService",
    json_version: "1.1",
};

/// The only key-policy name KMS holds a document under.
const POLICY_NAME: &str = "default";

/// The modelled error class a read refused for want of a grant on the key answers,
/// and the status it carries: "Either your KMS key policy or API caller does not have
/// the required permissions" (HTTP 400).
pub(crate) const KMS_ACCESS_DENIED: &str = "KmsAccessDeniedException";
pub(crate) const KMS_ACCESS_DENIED_STATUS: u16 = 400;

/// A key policy that grants the account that owns the key, which is the form the
/// default key policy takes: it delegates the decision to that account's own IAM, and
/// the account's published credential — the one these requests are signed with — is
/// the principal IAM does not bound.
pub(crate) fn granting_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [account_root_statement()]
    })
}

/// "The default key policy … gives the Amazon Web Services account that owns the KMS
/// key permission to use it in all cryptographic operations"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`): the
/// statement names the account root, so it DELEGATES the decision to that account's
/// own identity-based policies rather than granting to every principal in it.
fn account_root_statement() -> Value {
    json!({
        "Sid": "EnableIAMUserPermissions",
        "Effect": "Allow",
        "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
        "Action": "kms:*",
        "Resource": "*"
    })
}

/// A statement granting one named principal the grant a read of the key's data needs.
fn decrypt_statement(principal: &str) -> Value {
    json!({
        "Sid": "AllowOneReader",
        "Effect": "Allow",
        "Principal": { "AWS": principal },
        "Action": ["kms:Decrypt", "kms:DescribeKey"],
        "Resource": "*"
    })
}

/// A key policy carrying `statements` beside the account root's.
fn policy_with(statements: Vec<Value>) -> Value {
    let mut all = vec![account_root_statement()];
    all.extend(statements);
    json!({ "Version": "2012-10-17", "Statement": all })
}

/// An awsQuery parameter value, percent-encoded as that protocol takes it, so a
/// policy document's braces and quotes survive the round trip.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a sibling
/// suite that drives an awsQuery service reads this helper rather than carrying a
/// second copy of it.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
/// `pub(crate)` for the same reason as [`form_encode`].
pub(crate) fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The same document with the account's statement WITHDRAWN: the key still carries a
/// policy, and that policy grants the action to one other principal only, so nothing
/// in it allows the caller these requests are signed as.
pub(crate) fn withdrawn_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "OneOtherRoleOnly",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:role/another-reader") },
            "Action": ["kms:Decrypt", "kms:DescribeKey"],
            "Resource": "*"
        }]
    })
}

/// Step Functions and KMS over ONE engine, so the key a state machine names is the
/// key KMS's own handlers hold.
pub(crate) struct Chain {
    router: MultiServiceRouter,
}

/// The access-key id standing for the account's own published credential: neither STS
/// nor IAM holds a record of it, so it resolves to the account's ambient identity,
/// which no identity policy governs — the position AWS account root credentials are
/// in.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

impl Chain {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call one operation of one service, signed with `access_key_id` in the SigV4
    /// credential scope, and return `(status, body)`. The `Authorization` header is
    /// what routes the request, names the region its ARNs carry, and says which
    /// principal the request speaks for.
    pub(crate) async fn call_as(
        &self,
        access_key_id: &str,
        wire: &Wire,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            wire.signing_name
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{}.{REGION}.amazonaws.com", wire.signing_name))
            .header(
                "content-type",
                format!("application/x-amz-json-{}", wire.json_version),
            )
            .header("x-amz-target", format!("{}.{operation}", wire.target_prefix))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// [`Self::call_as`] for the account's own published credential.
    pub(crate) async fn call(&self, wire: &Wire, operation: &str, body: &Value) -> (u16, Value) {
        self.call_as(ACCOUNT_KEY, wire, operation, body).await
    }

    /// One awsQuery call to IAM, signed with the account's own credential.
    ///
    /// `pub(crate)` for the same reason [`Self::iam_user`] is.
    pub(crate) async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {action}: expected 200 from IAM, got {status} with {text}"
        );
        text
    }

    /// An IAM user carrying `policy` as its inline policy, with a long-term access key
    /// of its own: the access key id a request speaks for it with, and the user's ARN.
    ///
    /// `pub(crate)` for the reason [`form_encode`] is: `tests/all.rs` is this binary's
    /// crate root, so a sibling suite that has to call as a BOUNDED principal makes one
    /// here rather than carrying a second copy of this setup.
    pub(crate) async fn iam_user(&self, user_name: &str, policy: &Value) -> (String, String) {
        let created = self
            .iam("CreateUser", &[("UserName", user_name.to_string())])
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user Arn, got {created}"));
        self.iam(
            "PutUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
        let minted = self
            .iam("CreateAccessKey", &[("UserName", user_name.to_string())])
            .await;
        let key = xml_element(&minted, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}"));
        (key, arn)
    }

    /// Call one KMS operation, requiring a 200.
    pub(crate) async fn kms(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(&KMS, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200 from KMS, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Call one Step Functions operation, requiring a 200.
    pub(crate) async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(&SFN, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: expected 200 from Step Functions, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// A symmetric encryption key carrying `policy`, as its key ARN.
    ///
    /// `policy` is the document UNDER TEST — some rows withhold every action from the
    /// caller — so it need not leave the caller a path to a later `PutKeyPolicy`, which
    /// KMS's policy-lockout safety check otherwise requires. These suites are about what
    /// the document says to Step Functions, not about that check, so the key is created
    /// the way the KMS model provides for.
    pub(crate) async fn create_key(&self, policy: &Value) -> String {
        let created = self
            .kms(
                "CreateKey",
                &json!({
                    "KeySpec": "SYMMETRIC_DEFAULT",
                    "Policy": policy.to_string(),
                    "BypassPolicyLockoutSafetyCheck": true,
                }),
            )
            .await;
        string(&created["KeyMetadata"], "Arn")
    }

    /// Replace the key's policy, which is how a caller withdraws a grant and puts it
    /// back. It bypasses the policy-lockout safety check for the reason
    /// [`Self::create_key`] records.
    pub(crate) async fn put_policy(&self, key: &str, policy: &Value) {
        self.kms(
            "PutKeyPolicy",
            &json!({
                "KeyId": key,
                "PolicyName": POLICY_NAME,
                "Policy": policy.to_string(),
                "BypassPolicyLockoutSafetyCheck": true,
            }),
        )
        .await;
    }

    /// A state machine with `encryption`, one execution of it, and the moment that
    /// execution's outcome is due: the records every read below reports from.
    async fn run(&self, name: &str, encryption: Option<Value>) -> (String, String) {
        self.run_with_input(name, encryption, "{}").await
    }

    /// [`Self::run`] with the execution started on `input`, for a suite that reads
    /// that payload back out of the records.
    pub(crate) async fn run_with_input(
        &self,
        name: &str,
        encryption: Option<Value>,
        input: &str,
    ) -> (String, String) {
        let mut request = json!({
            "name": name,
            "definition": HELLO_DEFINITION,
            "roleArn": ROLE_ARN,
        });
        if let Some(encryption) = encryption {
            request["encryptionConfiguration"] = encryption;
        }
        let created = self.sfn("CreateStateMachine", &request).await;
        let machine = string(&created, "stateMachineArn");
        let started = self
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "input": input }),
            )
            .await;
        let execution = string(&started, "executionArn");
        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        (machine, execution)
    }
}

/// One read of data a state machine's key seals, and whether it decrypts that data at
/// all.
pub(crate) struct Read {
    pub(crate) case: &'static str,
    pub(crate) operation: &'static str,
    /// Whether the request reports the encrypted datum, which is what needs the
    /// caller's grant on the key.
    pub(crate) decrypts: bool,
    /// The request body, built from the state machine ARN and the execution ARN.
    pub(crate) request: fn(&str, &str) -> Value,
}

/// Every read the rule covers, and — beside each — the request form of the same
/// operation that reports none of the encrypted datum.
pub(crate) fn reads() -> Vec<Read> {
    vec![
        Read {
            case: "DescribeExecution reports the execution's input and output",
            operation: "DescribeExecution",
            decrypts: true,
            request: |_machine, execution| json!({ "executionArn": execution }),
        },
        Read {
            case: "GetExecutionHistory reports the data of each event",
            operation: "GetExecutionHistory",
            decrypts: true,
            request: |_machine, execution| json!({ "executionArn": execution }),
        },
        Read {
            case: "DescribeExecution asked for metadata only",
            operation: "DescribeExecution",
            // "METADATA_ONLY" reports none of the execution's sensitive data, so the
            // read opens nothing the key sealed.
            decrypts: false,
            request: |_machine, execution| {
                json!({ "executionArn": execution, "includedData": "METADATA_ONLY" })
            },
        },
        Read {
            case: "GetExecutionHistory asked for no execution data",
            operation: "GetExecutionHistory",
            // The WHOLE history of an encrypted execution is sealed under the key —
            // each event's metadata as much as the payloads this member leaves out —
            // so a read that asks for no execution data still has to open it.
            decrypts: true,
            request: |_machine, execution| {
                json!({ "executionArn": execution, "includeExecutionData": false })
            },
        },
        Read {
            case: "DescribeStateMachineForExecution reports the definition",
            operation: "DescribeStateMachineForExecution",
            decrypts: true,
            request: |_machine, execution| json!({ "executionArn": execution }),
        },
        Read {
            case: "DescribeStateMachineForExecution asked for metadata only",
            operation: "DescribeStateMachineForExecution",
            decrypts: false,
            request: |_machine, execution| {
                json!({ "executionArn": execution, "includedData": "METADATA_ONLY" })
            },
        },
        Read {
            case: "DescribeStateMachine reports the definition",
            operation: "DescribeStateMachine",
            decrypts: true,
            request: |machine, _execution| json!({ "stateMachineArn": machine }),
        },
        Read {
            case: "DescribeStateMachine asked for metadata only",
            operation: "DescribeStateMachine",
            decrypts: false,
            request: |machine, _execution| {
                json!({ "stateMachineArn": machine, "includedData": "METADATA_ONLY" })
            },
        },
    ]
}

/// The encryption configuration a state machine on a customer managed key carries.
pub(crate) fn customer_managed(key: &str) -> Value {
    json!({
        "type": "CUSTOMER_MANAGED_KMS_KEY",
        "kmsKeyId": key,
        "kmsDataKeyReusePeriodSeconds": 300
    })
}

/// A read of data the key seals follows the KEY POLICY at the instant of the call:
/// served while the policy allows the caller `kms:Decrypt`, refused with 400
/// `KmsAccessDeniedException` while it does not, and served again once the statement
/// is put back. A request that reports none of the encrypted datum is served
/// throughout.
#[tokio::test]
async fn a_read_of_encrypted_data_follows_the_keys_own_policy() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    let (machine, execution) = chain
        .run("encrypted-reads", Some(customer_managed(&key)))
        .await;

    for Read {
        case,
        operation,
        decrypts,
        request,
    } in reads()
    {
        let body = request(&machine, &execution);

        // Granted: every read is served.
        chain.put_policy(&key, &granting_policy()).await;
        let (status, answer) = chain.call(&SFN, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the key policy allows the caller, so the read is served, got {status} \
             with {answer:?}"
        );

        // Withdrawn: a read that decrypts the datum is refused, and one that reports
        // none of it is still served.
        chain.put_policy(&key, &withdrawn_policy()).await;
        let (status, answer) = chain.call(&SFN, operation, &body).await;
        if decrypts {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
                "case {case:?}: with the caller's statement withdrawn the read is refused, got \
                 {status} with {answer:?}"
            );
            let message = answer.get("message").and_then(Value::as_str).unwrap_or("");
            assert!(
                message.contains("kms:Decrypt") && message.contains(&key),
                "case {case:?}: the refusal names the action and the key, got {answer:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a read that decrypts nothing needs no grant, got {status} with \
                 {answer:?}"
            );
        }

        // Restored: the read is served again, with no call in between.
        chain.put_policy(&key, &granting_policy()).await;
        let (status, answer) = chain.call(&SFN, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the restored statement serves the read again, got {status} with \
             {answer:?}"
        );
    }
}

/// A state machine on an Amazon Web Services owned key — what a create naming no
/// encryption configuration is on, and what `AWS_OWNED_KEY` names — needs no grant of
/// the caller's: AWS manages that key's policy for the service, so every read is
/// served whatever any customer managed key's policy says.
#[tokio::test]
async fn a_state_machine_on_an_aws_owned_key_needs_no_grant() {
    let chain = Chain::new();
    // A key whose policy grants the caller nothing, to show that a state machine not
    // on it is unaffected by it.
    let key = chain.create_key(&withdrawn_policy()).await;
    assert!(!key.is_empty(), "case the key: expected a key ARN, got {key:?}");

    // Rows differ along HOW the state machine ends up on the AWS owned key: by
    // naming no encryption configuration at all, and by naming that type.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no encryption configuration", None),
        (
            "the AWS owned key named",
            Some(json!({ "type": "AWS_OWNED_KEY" })),
        ),
    ];

    for (index, (case, encryption)) in rows.into_iter().enumerate() {
        let (machine, execution) = chain.run(&format!("owned-{index}"), encryption).await;
        for Read {
            case: read,
            operation,
            request,
            ..
        } in reads()
        {
            let (status, answer) = chain
                .call(&SFN, operation, &request(&machine, &execution))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} / {read:?}: a resource on the AWS owned key is read without a \
                 grant, got {status} with {answer:?}"
            );
        }
    }
}

/// The refusal is the KEY's, not the execution's: a read whose ARN addresses nothing
/// is still the operation's own absence error, and a key policy that allows the caller
/// nothing does not turn it into an authorization failure.
#[tokio::test]
async fn an_absent_resource_is_still_reported_as_absent() {
    let chain = Chain::new();
    // The state machine is created while the policy still grants the caller the data
    // key the create seals its definition with — a create the key refuses creates
    // nothing — and the grant is WITHDRAWN afterwards, which is what leaves the reads
    // below with a key policy that allows them nothing.
    let key = chain.create_key(&granting_policy()).await;
    let (machine, execution) = chain.run("absent-reads", Some(customer_managed(&key))).await;
    chain.put_policy(&key, &withdrawn_policy()).await;

    // Rows differ along WHICH read is made and which ARN it names: an execution of
    // the account that was never started, and a state machine that was never created.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "DescribeExecution of an execution nobody started",
            "DescribeExecution",
            json!({ "executionArn": format!("{}-gone", execution) }),
            "ExecutionDoesNotExist",
        ),
        (
            "GetExecutionHistory of an execution nobody started",
            "GetExecutionHistory",
            json!({ "executionArn": format!("{}-gone", execution) }),
            "ExecutionDoesNotExist",
        ),
        (
            "DescribeStateMachine of a state machine nobody created",
            "DescribeStateMachine",
            json!({ "stateMachineArn": format!("{}-gone", machine) }),
            "StateMachineDoesNotExist",
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, answer) = chain.call(&SFN, operation, &request).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected}, got {status} with {answer:?}"
        );
    }
}

/// A caller that is NOT the account itself is judged on the documents in force on it:
/// a principal holding the Step Functions read and nothing of KMS is refused while the
/// key's policy only delegates to the account's IAM, and served as soon as the policy
/// names it — which is the grant a key policy gives directly.
///
/// The refusal states WHO was refused, WHICH action and on WHICH key, and which
/// document has to change: the key policy delegates, so the permission the principal
/// is missing is its own identity-based one.
#[tokio::test]
async fn a_principal_without_the_grant_is_refused_and_named() {
    let chain = Chain::new();
    // The identity policy of the caller: it may read the execution, and nothing it
    // says is about KMS.
    let reader_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": [
                "states:DescribeExecution",
                "states:GetExecutionHistory",
                "states:DescribeStateMachine",
                "states:DescribeStateMachineForExecution"
            ],
            "Resource": "*"
        }]
    });
    let (reader_key, reader_arn) = chain.iam_user("sfn-data-reader", &reader_policy).await;
    let key = chain.create_key(&granting_policy()).await;
    let (machine, execution) = chain.run("reader-reads", Some(customer_managed(&key))).await;

    for Read {
        case,
        operation,
        decrypts,
        request,
    } in reads()
    {
        let body = request(&machine, &execution);

        // The key policy delegates to IAM, and this principal's own documents allow
        // nothing of KMS: a read that decrypts the datum is refused.
        chain.put_policy(&key, &granting_policy()).await;
        let (status, answer) = chain.call_as(&reader_key, &SFN, operation, &body).await;
        if decrypts {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
                "case {case:?}: a principal with no grant on the key is refused, got {status} \
                 with {answer:?}"
            );
            // Every variable part of the relayed sentence comes from the request and
            // the state: who asked, what for, on which key, and which document has to
            // change — the key policy delegates, so what the principal is missing is
            // its own identity-based permission. The id of the refused inner call and
            // the attempts it took are minted per call, so the metadata that follows
            // the sentence is checked for its fixed text with those two read out of it.
            let message = answer.get("message").and_then(Value::as_str).unwrap_or("");
            let sentence = format!(
                "User: {reader_arn} is not authorized to perform: kms:Decrypt on resource: \
                 {key} because no identity-based policy allows the kms:Decrypt action"
            );
            let trailer = message.strip_prefix(&sentence).unwrap_or_else(|| {
                panic!("case {case:?}: expected the refusal to state {sentence:?}, got {answer:?}")
            });
            let (request_id, attempts) = trailer
                .strip_prefix(" (Service: Kms, Status Code: 400, Request ID: ")
                .and_then(|rest| rest.split_once(") (SDK Attempt Count: "))
                .and_then(|(id, rest)| rest.strip_suffix(')').map(|count| (id, count)))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the relayed metadata \" (Service: Kms, Status \
                         Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)\", got \
                         {trailer:?}"
                    )
                });
            assert!(
                !request_id.is_empty() && !request_id.contains(' '),
                "case {case:?}: expected the id of the refused call, got {request_id:?}"
            );
            assert!(
                attempts.parse::<u32>().is_ok_and(|made| made >= 1),
                "case {case:?}: expected how many attempts were made, got {attempts:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a read that decrypts nothing needs no grant, got {status} with \
                 {answer:?}"
            );
        }

        // A statement naming the principal ITSELF grants outright, so the same read
        // is served from the very next call.
        chain
            .put_policy(&key, &policy_with(vec![decrypt_statement(&reader_arn)]))
            .await;
        let (status, answer) = chain.call_as(&reader_key, &SFN, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the key policy now names the principal, so the read is served, got \
             {status} with {answer:?}"
        );
    }
}
