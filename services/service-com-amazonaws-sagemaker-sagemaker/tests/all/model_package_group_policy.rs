// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the resource policy a SageMaker model package group carries:
//! `PutModelPackageGroupPolicy`, the `GetModelPackageGroupPolicy` that reads it back and
//! the `DeleteModelPackageGroupPolicy` that removes it.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call:
//!
//! * a put stores the document on the group it names and a get hands it back BYTE FOR
//!   BYTE — over documents that differ in formatting, nesting, statement count, embedded
//!   blobs and length, including the published length bound;
//! * a second put replaces the first document, so the group holds exactly one policy and
//!   the last writer wins (also under two concurrent puts);
//! * a delete removes only the policy, leaving the group itself describable, and a later
//!   read finds none;
//! * a group no record is stored under is refused, and so is a document that is not an
//!   ASCII IAM policy document — over every non-object JSON form, over a non-ASCII
//!   document and over an object that is no policy — with a 400 `ValidationException`,
//!   leaving any document already stored untouched;
//! * each group's policy is its own, and deleting the GROUP releases its policy.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The most bytes `ResourcePolicy` may carry: `PolicyString` is `len 1..20480, pattern .*`
/// (smithy_query query="operation" operation="PutModelPackageGroupPolicy").
const POLICY_MAX_LEN: usize = 20_480;

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the ARN region falls back to the
    /// emulator default.
    async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
    }

    /// Call `target` and require a 200, returning the parsed output.
    async fn ok(&self, case: &str, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {parsed}"
        );
        parsed
    }

    /// Create the group a policy is attached to, through the API that owns it, and return
    /// its ARN.
    async fn create_group(&self, case: &str, name: &str) -> String {
        let created = self
            .ok(
                case,
                "CreateModelPackageGroup",
                &json!({"ModelPackageGroupName": name}),
            )
            .await;
        str_field(&created, "ModelPackageGroupArn")
    }

    async fn put(&self, name: &str, policy: &str) -> (u16, Value) {
        self.call(
            "PutModelPackageGroupPolicy",
            &json!({"ModelPackageGroupName": name, "ResourcePolicy": policy}),
        )
        .await
    }

    async fn get(&self, name: &str) -> (u16, Value) {
        self.call(
            "GetModelPackageGroupPolicy",
            &json!({"ModelPackageGroupName": name}),
        )
        .await
    }

    async fn delete(&self, name: &str) -> (u16, Value) {
        self.call(
            "DeleteModelPackageGroupPolicy",
            &json!({"ModelPackageGroupName": name}),
        )
        .await
    }

    /// The document the group currently carries, requiring the read to succeed.
    async fn stored_policy(&self, case: &str, name: &str) -> String {
        let read = self
            .ok(
                case,
                "GetModelPackageGroupPolicy",
                &json!({"ModelPackageGroupName": name}),
            )
            .await;
        str_field(&read, "ResourcePolicy")
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

/// A policy document of exactly `len` bytes: an IAM policy whose `Sid` is padded out, so
/// the length bound can be exercised with a document that is still a policy.
fn policy_of_length(len: usize) -> String {
    let skeleton = r#"{"Version":"2012-10-17","Statement":[{"Sid":"","Effect":"Allow","Principal":{"AWS":"*"},"Action":"sagemaker:DescribeModelPackage","Resource":"*"}]}"#;
    let padding = len
        .checked_sub(skeleton.len())
        .unwrap_or_else(|| panic!("length {len} is shorter than the skeleton"));
    let document = skeleton.replace(r#""Sid":"""#, &format!(r#""Sid":"{}""#, "s".repeat(padding)));
    assert_eq!(
        document.len(),
        len,
        "the padded document should be {len} bytes, got {}",
        document.len()
    );
    document
}

/// Documents `PutModelPackageGroupPolicy` accepts, differing along what the rule
/// generalises over: whitespace and key order (so byte preservation is observable),
/// statement count and nesting depth, escaped content, an embedded base64 blob, and
/// length.
fn accepted_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "compact-single-statement",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"AllowDescribe","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":["sagemaker:DescribeModelPackage"],"Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "pretty-printed-with-newlines",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \"Effect\": \"Allow\",\n      \"Principal\": {\"AWS\": \"arn:aws:iam::123456789012:role/Reader\"},\n      \"Action\": \"sagemaker:ListModelPackages\",\n      \"Resource\": \"*\"\n    }\n  ]\n}"
                .to_string(),
        ),
        (
            "statement-before-version",
            r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"sagemaker:DeleteModelPackage","Resource":"*"}],"Version":"2012-10-17"}"#
                .to_string(),
        ),
        (
            "two-statements-with-condition",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"One","Effect":"Allow","Principal":{"AWS":["arn:aws:iam::123456789012:root","arn:aws:iam::210987654321:root"]},"Action":["sagemaker:DescribeModelPackage","sagemaker:ListModelPackages"],"Resource":"*"},{"Sid":"Two","Effect":"Deny","Principal":"*","Action":"sagemaker:UpdateModelPackage","Resource":"*","Condition":{"StringNotEquals":{"aws:PrincipalAccount":"123456789012"}}}]}"#
                .to_string(),
        ),
        (
            "escaped-content",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"Escapes\u0041nd\"Quotes","Effect":"Allow","Principal":{"AWS":"*"},"Action":"sagemaker:DescribeModelPackage","Resource":"*","Condition":{"StringEquals":{"aws:ResourceTag/team":"data\tteam"}}}]}"#
                .to_string(),
        ),
        (
            "embedded-base64-blob",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"sagemaker:DescribeModelPackage","Resource":"*","Condition":{"StringEquals":{"sagemaker:Payload":"ZXhhbXBsZSBibG9iIHdpdGggKyAvIGFuZCA9IHBhZGRpbmc="}}}]}"#
                .to_string(),
        ),
        ("at-the-length-bound", policy_of_length(POLICY_MAX_LEN)),
    ]
}

/// Documents the service refuses as resource policies, spanning every JSON form that is
/// not an object, text that is not JSON at all, a document carrying non-ASCII characters
/// and objects carrying elements the policy language does not define.
const REFUSED_DOCUMENTS: [(&str, &str); 12] = [
    ("not-json-at-all", "this is not a policy"),
    ("truncated-object", r#"{"Version":"2012-10-17""#),
    ("json-array", r#"[{"Effect":"Allow"}]"#),
    ("json-string", r#""just a string""#),
    ("json-number", "42"),
    ("json-null", "null"),
    ("json-boolean", "true"),
    (
        "non-ascii-statement-id",
        r#"{"Version":"2012-10-17","Statement":[{"Sid":"Modèle-日本語","Effect":"Allow","Principal":{"AWS":"*"},"Action":"sagemaker:DescribeModelPackage","Resource":"*"}]}"#,
    ),
    ("object-with-no-statement", r#"{"Version":"2012-10-17"}"#),
    (
        "an-element-the-document-does-not-define",
        r#"{"Version":"2012-10-17","NotAValidPolicyKey":1,"Statement":[]}"#,
    ),
    (
        "an-element-a-statement-does-not-define",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"sagemaker:*","Grant":"*"}]}"#,
    ),
    (
        "a-statement-that-is-not-an-object",
        r#"{"Version":"2012-10-17","Statement":["allow it"]}"#,
    ),
];

/// RULE: a put stores the document on the group it names and `GetModelPackageGroupPolicy`
/// hands back exactly those bytes — the service stores the document, it does not reformat
/// or re-serialize it.
#[tokio::test]
async fn put_then_get_returns_the_document_byte_for_byte() {
    for (case, document) in accepted_documents() {
        let emulator = Emulator::new();
        let arn = emulator.create_group(case, "policy-round-trip").await;

        let (status, body) = emulator.put("policy-round-trip", &document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from PutModelPackageGroupPolicy, got {status} \
             with {body}"
        );
        assert_eq!(
            str_field(&body, "ModelPackageGroupArn"),
            arn,
            "case {case:?}: the put answers the group's ARN, got {body:?}"
        );

        let read = emulator.stored_policy(case, "policy-round-trip").await;
        assert_eq!(
            read, document,
            "case {case:?}: the document read back should be the bytes put, got {read:?}"
        );
        // A read reports; it must not change what the next reader sees.
        let again = emulator.stored_policy(case, "policy-round-trip").await;
        assert_eq!(
            again, document,
            "case {case:?}: a second read should report the same document, got {again:?}"
        );
    }
}

/// RULE: a group carries at most ONE resource policy, so a later put REPLACES the document
/// an earlier one stored, whatever the two documents are.
#[tokio::test]
async fn a_later_put_replaces_the_stored_document() {
    let documents = accepted_documents();
    for (index, (case, first)) in documents.iter().enumerate() {
        // Pair each document with the next one in the table, so every row is both a first
        // and a second write.
        let (next_case, second) = &documents[(index + 1) % documents.len()];
        let case = format!("{case} -> {next_case}");
        let emulator = Emulator::new();
        emulator.create_group(&case, "policy-overwrite").await;

        let (status, body) = emulator.put("policy-overwrite", first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the first put, got {status} with {body}"
        );
        let (status, body) = emulator.put("policy-overwrite", second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the second put, got {status} with {body}"
        );

        let read = emulator.stored_policy(&case, "policy-overwrite").await;
        assert_eq!(
            &read, second,
            "case {case:?}: the second put's document should be the stored one, got {read:?}"
        );
    }
}

/// RULE: concurrent puts on one group serialize — both succeed and the group ends up
/// carrying exactly ONE of the documents sent, never a mix of them.
#[tokio::test]
async fn concurrent_puts_leave_exactly_one_of_the_documents() {
    let documents = accepted_documents();
    let (left_case, left) = &documents[0];
    let (right_case, right) = &documents[3];
    let case = format!("{left_case} || {right_case}");

    let emulator = Emulator::new();
    emulator.create_group(&case, "policy-concurrent").await;

    let (first, second) = tokio::join!(
        emulator.put("policy-concurrent", left),
        emulator.put("policy-concurrent", right)
    );
    assert_eq!(
        first.0, 200,
        "case {case:?}: expected HTTP 200 from the first concurrent put, got {} with {}",
        first.0, first.1
    );
    assert_eq!(
        second.0, 200,
        "case {case:?}: expected HTTP 200 from the second concurrent put, got {} with {}",
        second.0, second.1
    );

    let read = emulator.stored_policy(&case, "policy-concurrent").await;
    assert!(
        &read == left || &read == right,
        "case {case:?}: the stored document should be one of the two put, got {read:?}"
    );
}

/// RULE: a delete removes the policy and nothing else — the group itself stays, and a read
/// of the policy afterwards is refused rather than answered with an empty document.
#[tokio::test]
async fn delete_removes_only_the_policy() {
    for (case, document) in accepted_documents() {
        let emulator = Emulator::new();
        let arn = emulator.create_group(case, "policy-delete").await;
        let (status, body) = emulator.put("policy-delete", &document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the put, got {status} with {body}"
        );

        let (status, body) = emulator.delete("policy-delete").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteModelPackageGroupPolicy, got \
             {status} with {body}"
        );

        let (status, body) = emulator.get("policy-delete").await;
        assert_eq!(
            status, 400,
            "case {case:?}: a read after the delete should be refused, got {status} with \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: error code of the read after the delete, got {body:?}"
        );

        // Deleting twice deletes nothing the second time: there is no policy left.
        let (status, body) = emulator.delete("policy-delete").await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete should be refused, got {status} with {body}"
        );

        // The GROUP is untouched by a policy delete, and takes a policy again.
        let described = emulator
            .ok(
                case,
                "DescribeModelPackageGroup",
                &json!({"ModelPackageGroupName": "policy-delete"}),
            )
            .await;
        assert_eq!(
            str_field(&described, "ModelPackageGroupArn"),
            arn,
            "case {case:?}: the group should still be described, got {described:?}"
        );
        let (status, body) = emulator.put("policy-delete", &document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the group should take a policy again, got {status} with {body}"
        );
        let read = emulator.stored_policy(case, "policy-delete").await;
        assert_eq!(
            read, document,
            "case {case:?}: the re-put document should be stored, got {read:?}"
        );
    }
}

/// RULE: a `ResourcePolicy` that is not an ASCII IAM policy document is refused with a
/// 400 `ValidationException` — the class for a request the caller has to change — and the
/// refusal changes nothing: a document already stored is still the one a reader sees.
#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused() {
    let (good_case, good) = accepted_documents().swap_remove(0);

    for (case, document) in REFUSED_DOCUMENTS {
        let emulator = Emulator::new();
        emulator.create_group(case, "policy-refusal").await;
        let (status, body) = emulator.put("policy-refusal", &good).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the {good_case:?} document should be accepted first, got \
             {status} with {body}"
        );

        let (status, body) = emulator.put("policy-refusal", document).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from PutModelPackageGroupPolicy, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: error code for a document that is not a policy, got {body:?}"
        );

        let read = emulator.stored_policy(case, "policy-refusal").await;
        assert_eq!(
            read, good,
            "case {case:?}: a refused put should leave the stored document alone, got \
             {read:?}"
        );
    }
}

/// RULE: `ResourcePolicy` is bounded at `POLICY_MAX_LEN` bytes (and at least one), so a
/// document AT the bound is accepted and one past it — like an empty one — is refused by
/// the boundary validator.
#[tokio::test]
async fn the_document_length_bound_is_enforced() {
    let emulator = Emulator::new();
    emulator.create_group("length bounds", "policy-length").await;

    let at_bound = policy_of_length(POLICY_MAX_LEN);
    let (status, body) = emulator.put("policy-length", &at_bound).await;
    assert_eq!(
        status, 200,
        "case \"len={POLICY_MAX_LEN}\": expected HTTP 200, got {status} with {body}"
    );
    let read = emulator.stored_policy("len at bound", "policy-length").await;
    assert_eq!(
        read, at_bound,
        "case \"len={POLICY_MAX_LEN}\": the stored document should be the bytes put"
    );

    for (case, document) in [
        (
            format!("len={}", POLICY_MAX_LEN + 1),
            policy_of_length(POLICY_MAX_LEN + 1),
        ),
        ("len=0".to_string(), String::new()),
    ] {
        let (status, body) = emulator.put("policy-length", &document).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from PutModelPackageGroupPolicy, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: error code outside the modelled length, got {body:?}"
        );
        // The refusal left the accepted document in place.
        let read = emulator.stored_policy(&case, "policy-length").await;
        assert_eq!(
            read, at_bound,
            "case {case:?}: a refused put should leave the stored document alone, got \
             {read:?}"
        );
    }
}

/// RULE: the three policy operations address an EXISTING group's policy, so a name no
/// group is stored under is refused (HTTP 400 `ValidationException`) and nothing is
/// created — whatever the name's shape.
#[tokio::test]
async fn a_group_that_does_not_exist_is_refused() {
    // Rows differ along the name shape `EntityName` admits, including both length bounds.
    let names = [
        "g",
        "absent-group",
        "Mixed-Case-0123",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let (_, document) = accepted_documents().swap_remove(0);

    for name in names {
        let emulator = Emulator::new();
        for (operation, response) in [
            ("PutModelPackageGroupPolicy", emulator.put(name, &document).await),
            ("GetModelPackageGroupPolicy", emulator.get(name).await),
            ("DeleteModelPackageGroupPolicy", emulator.delete(name).await),
        ] {
            let (status, body) = response;
            assert_eq!(
                status, 400,
                "case {name:?}: expected HTTP 400 from {operation}, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {name:?}: error code from {operation}, got {body:?}"
            );
        }

        // Nothing was created by any of the three refusals.
        let (status, body) = emulator
            .call(
                "DescribeModelPackageGroup",
                &json!({"ModelPackageGroupName": name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: the refusals must not create the group, got {status} with {body}"
        );
    }
}

/// RULE: a group that has never been put a policy carries none, so the reader refuses it
/// instead of reporting a document.
#[tokio::test]
async fn a_group_without_a_policy_has_no_document_to_report() {
    let emulator = Emulator::new();
    for name in ["fresh-group", "another-fresh-group"] {
        emulator.create_group(name, name).await;
        let (status, body) = emulator.get(name).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 from GetModelPackageGroupPolicy, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name:?}: error code for a group carrying no policy, got {body:?}"
        );
    }
}

/// RULE: a policy belongs to the group it was put on, so each group reports its own
/// document and a delete on one group leaves the others' policies in place.
#[tokio::test]
async fn each_groups_policy_is_its_own() {
    let documents = accepted_documents();
    let emulator = Emulator::new();

    // One group per document in the table, all in one emulator.
    let groups: Vec<(String, String)> = documents
        .iter()
        .enumerate()
        .map(|(index, (_, document))| (format!("isolated-group-{index}"), document.clone()))
        .collect();
    for (name, document) in &groups {
        emulator.create_group(name, name).await;
        let (status, body) = emulator.put(name, document).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from the put, got {status} with {body}"
        );
    }
    for (name, document) in &groups {
        let read = emulator.stored_policy(name, name).await;
        assert_eq!(
            &read, document,
            "case {name:?}: each group reports its own document, got {read:?}"
        );
    }

    // Deleting the FIRST group's policy touches no other group's.
    let (status, body) = emulator.delete(&groups[0].0).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from the delete, got {status} with {body}",
        groups[0].0
    );
    for (name, document) in &groups[1..] {
        let read = emulator.stored_policy(name, name).await;
        assert_eq!(
            &read, document,
            "case {name:?}: another group's delete must not disturb this policy, got \
             {read:?}"
        );
    }
}

/// RULE: the policy is part of the group record, so `DeleteModelPackageGroup` releases it
/// with the group: a group re-created under the same name carries no policy.
#[tokio::test]
async fn deleting_the_group_releases_its_policy() {
    let (case, document) = accepted_documents().swap_remove(0);
    let emulator = Emulator::new();
    emulator.create_group(case, "recycled-group").await;
    let (status, body) = emulator.put("recycled-group", &document).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the put, got {status} with {body}"
    );

    emulator
        .ok(
            case,
            "DeleteModelPackageGroup",
            &json!({"ModelPackageGroupName": "recycled-group"}),
        )
        .await;
    let (status, body) = emulator.get("recycled-group").await;
    assert_eq!(
        status, 400,
        "case {case:?}: the deleted group has no policy to report, got {status} with {body}"
    );

    emulator.create_group(case, "recycled-group").await;
    let (status, body) = emulator.get("recycled-group").await;
    assert_eq!(
        status, 400,
        "case {case:?}: a re-created group carries no stale policy, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: error code for the re-created group, got {body:?}"
    );
}

/// RULE: the ARN a put answers is the group's own —
/// `arn:aws:sagemaker:<signed region>:<account>:model-package-group/<name>` — whatever
/// region the caller signed for, and the emulator default when the request is unsigned.
#[tokio::test]
async fn the_put_answers_the_groups_arn_in_the_signed_region() {
    let (_, document) = accepted_documents().swap_remove(0);
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in ["g", "policy-arn-group"] {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateModelPackageGroup",
                    &json!({"ModelPackageGroupName": name}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateModelPackageGroup, got {status} \
                 with {created}"
            );

            let (status, body) = emulator
                .call_in(
                    region,
                    "PutModelPackageGroupPolicy",
                    &json!({"ModelPackageGroupName": name, "ResourcePolicy": document}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from PutModelPackageGroupPolicy, got \
                 {status} with {body}"
            );
            assert_eq!(
                str_field(&body, "ModelPackageGroupArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:model-package-group/{name}"),
                "case {case:?}: ModelPackageGroupArn, got {body:?}"
            );
            assert_eq!(
                str_field(&body, "ModelPackageGroupArn"),
                str_field(&created, "ModelPackageGroupArn"),
                "case {case:?}: the put answers the ARN the create minted, got {body:?}"
            );
        }
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call_unsigned(
            "CreateModelPackageGroup",
            &json!({"ModelPackageGroupName": "unsigned-group"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"unsigned\": expected HTTP 200 from CreateModelPackageGroup, got {status} with \
         {created}"
    );
    let (status, body) = emulator
        .call_unsigned(
            "PutModelPackageGroupPolicy",
            &json!({"ModelPackageGroupName": "unsigned-group", "ResourcePolicy": document}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"unsigned\": expected HTTP 200 from PutModelPackageGroupPolicy, got {status} \
         with {body}"
    );
    assert_eq!(
        str_field(&body, "ModelPackageGroupArn"),
        str_field(&created, "ModelPackageGroupArn"),
        "case \"unsigned\": the put answers the ARN the create minted, got {body:?}"
    );
}
