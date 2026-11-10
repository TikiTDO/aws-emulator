// SPDX-License-Identifier: CC0-1.0

//! The wire harness and fixtures every suite in this target shares.
//!
//! One emulator behind the same router the binary boots, driven with awsJson1_0
//! `POST /` requests, plus the readers and request fixtures a test needs. Suites
//! `use crate::harness::…` rather than standing one up each.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in these tests is signed for. ARNs must carry it,
/// which is how a region other than the emulator default proves the ARN is built
/// from the request rather than from a constant.
pub const REGION: &str = "eu-west-1";

/// The account segment every emulator ARN carries, sourced from the vendored
/// constant so an ARN assertion cannot drift from it.
pub const ACCOUNT: &str = emulator_server::EMULATOR_ACCOUNT_ID;

/// One emulator: a single shared engine behind the router every test calls.
pub struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    /// Stand up the router with this service mounted on a fresh engine.
    pub fn new() -> Self {
        Self::mounting(false)
    }

    /// Stand up the router with this service AND Amazon S3 mounted on the SAME
    /// engine, exactly as the assembled emulator mounts them.
    ///
    /// The bucket an S3 log destination names is a resource of THAT service, and
    /// what its policy permits is a fact only it can answer, so a suite about
    /// those destinations needs one to reach; [`Harness::new`] deliberately
    /// leaves it unmounted, which is the "Amazon S3 said nothing" case.
    pub fn with_s3() -> Self {
        Self::mounting(true)
    }

    /// Stand up the router, with Amazon S3 beside this service when `s3` says so.
    fn mounting(s3: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        if s3 {
            router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self { router }
    }

    /// A handle to this emulator for driving it through an AWS SDK client: the
    /// same in-process seam a handler reaches another mounted service through.
    pub fn callback(&self) -> smithy_server_runtime::EmbeddedEmulator {
        smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone())
    }

    /// An Amazon S3 client for [`REGION`] over this emulator's own in-process
    /// callback — the very seam, and the very client construction
    /// ([`service_common::s3_client::s3_client`]), the handlers resolve a log
    /// destination through.
    pub fn s3(&self) -> ::aws_sdk_s3::Client {
        ::service_common::s3_client::s3_client(&self.callback(), REGION)
    }

    /// Send one operation and return its HTTP status and parsed JSON body.
    pub async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://network-firewall.eu-west-1.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("NetworkFirewall_20201112.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/network-firewall/\
                     aws4_request, SignedHeaders=host, Signature=abc"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Send one operation that must succeed, and return its body.
    pub async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Create a firewall policy with the minimal valid document.
    pub async fn create_policy(&self, name: &str) -> Value {
        self.ok(
            "CreateFirewallPolicy",
            &json!({ "FirewallPolicyName": name, "FirewallPolicy": minimal_policy_document() }),
        )
        .await
    }

    /// Create a firewall policy and return its ARN.
    pub async fn shared_policy(&self, name: &str) -> String {
        let created = self.create_policy(name).await;
        string_at(&created, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string()
    }

    /// Create a rule group of `group_type` carrying one rule, and return its ARN.
    pub async fn shared_rule_group(&self, name: &str, group_type: &str, capacity: i64) -> String {
        let document = if group_type == "STATELESS" {
            json!({ "RulesSource": { "StatelessRulesAndCustomActions": {
                "StatelessRules": [{
                    "Priority": 1,
                    "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": {} },
                }],
            }}})
        } else {
            json!({ "RulesSource": { "StatefulRules": [{
                "Action": "PASS",
                "Header": {
                    "Protocol": "TCP",
                    "Source": "ANY",
                    "SourcePort": "ANY",
                    "Direction": "ANY",
                    "Destination": "ANY",
                    "DestinationPort": "ANY",
                },
                "RuleOptions": [{ "Keyword": "sid", "Settings": ["1"] }],
            }]}})
        };
        let created = self
            .ok(
                "CreateRuleGroup",
                &json!({
                    "RuleGroupName": name,
                    "Type": group_type,
                    "Capacity": capacity,
                    "RuleGroup": document,
                }),
            )
            .await;
        string_at(&created, &["RuleGroupResponse", "RuleGroupArn"]).to_string()
    }
}

/// A minimal valid `FirewallPolicy` document: the two members the model marks
/// required (`StatelessDefaultActions`, `StatelessFragmentDefaultActions`).
pub fn minimal_policy_document() -> Value {
    json!({
        "StatelessDefaultActions": ["aws:forward_to_sfe"],
        "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
    })
}

/// A resource-policy document granting exactly `actions` on `resource` to this
/// account, in the form the AWS `PutResourcePolicy` documentation shows: one
/// `Allow` statement carrying all of them.
pub fn resource_policy_document(actions: &[&str], resource: &str) -> String {
    split_resource_policy_document(&[actions], resource)
}

/// The same document with its grants SPLIT across one `Allow` statement per entry
/// of `statements`, so the policy's granted set is their union.
pub fn split_resource_policy_document(statements: &[&[&str]], resource: &str) -> String {
    let statements: Vec<Value> = statements
        .iter()
        .map(|actions| {
            json!({
                "Effect": "Allow",
                "Principal": { "AWS": ACCOUNT },
                "Action": actions,
                "Resource": resource,
            })
        })
        .collect();
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// The awsJson error code (`__type`) of an error body.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` member of an error body.
pub fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// The text every `ResourceNotFoundException` this service answers carries, as
/// recorded off the service: byte-identical across operations and resource
/// kinds, naming neither the resource, nor its type, nor its ARN.
pub const NOT_FOUND_MESSAGE: &str =
    "No object with the provided name or resource identifier was found";

/// Whether `value` satisfies the `ResourceId` / `UpdateToken` pattern
/// `^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$` (length 36).
pub fn is_uuid_form(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// A resource projection with its `Tags` member defaulted to the EMPTY LIST.
///
/// A read and a delete project the stored record, so each reports a resource's
/// tag collection as a list where a create and an update report the `Tags`
/// member as the request gave it — absent when it gave none. An assertion that a
/// read reports what a create or an update minted passes the WRITE's projection
/// through this and leaves the read's own side untouched, so the read must still
/// carry the list while every other member stays an exact comparison.
pub fn with_tag_list(projection: &Value) -> Value {
    let mut projection = projection.clone();
    if let Some(object) = projection.as_object_mut() {
        object.entry("Tags").or_insert_with(|| json!([]));
    }
    projection
}

/// Read a required string out of a response body, naming the path on failure.
pub fn string_at<'a>(body: &'a Value, path: &[&str]) -> &'a str {
    let mut cursor = body;
    for step in path {
        cursor = cursor
            .get(step)
            .unwrap_or_else(|| panic!("expected {path:?} in {body:?}"));
    }
    cursor
        .as_str()
        .unwrap_or_else(|| panic!("expected {path:?} to be a string in {body:?}"))
}
