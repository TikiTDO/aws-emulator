// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of this service's operations, over the restJson1 wire.
//!
//! The rule under test: a request to an operation whose model declares
//! `AccessDeniedException` is served only to a caller whose identity-based IAM policies
//! allow THAT operation's own action (`guardduty:<OperationName>`) on the resource the
//! request addresses — the detector's sub-resource path, the custom detection rule, the
//! Malware Protection plan, and the ARN a tagging call binds as its label. Every other
//! caller is refused with the modelled `AccessDeniedException` (HTTP 403) BEFORE the
//! operation reads or writes anything, so a refused caller learns nothing about what the
//! account holds — not even whether what it named exists — and a refused mutation changes
//! nothing. An operation whose model declares no 403 is not judged at all, because
//! answering a caller an error its operation does not model would be a different
//! divergence.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine (`CreateUser` + `PutUserPolicy` + `CreateAccessKey`,
//! `CreateRole` + `PutRolePolicy` + `AssumeRole`), so the documents this service reads are
//! the ones those services wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! It is asserted over TABLES that differ along what the rule generalises over: the
//! OPERATION asked for (a create under a detector, an update of one sub-resource, a read
//! of one resource, a read of a collection, a read of a resource the account does not
//! hold, a delete, a tagging read and two tagging writes — each with its own action, its
//! own HTTP method and its own kind of resource ARN), what the caller's policies say
//! (nothing at all, an `Allow` for this action on this resource, a wildcard `Allow`, an
//! `Allow` for another action, an `Allow` on another resource, an explicit `Deny` beside a
//! wildcard `Allow`), and the KIND of principal (the account's own published credential,
//! an IAM user, an assumed-role session).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::service::authorization::{
    ACTION_PREFIX, ANY_RESOURCE, GATED_OPERATIONS,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The ARN builders the service's own reads report, so a test ARN cannot drift from the one
// the gate judges against, and the non-greedy `{ResourceArn}` label spelling.
use crate::resource_tag_removal::{arn_of, uri_label};

/// The region every call below is signed for. It is one of the Regions the investigation
/// preview is served through, so the create row is judged rather than refused for the
/// endpoint it arrived at.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies and is allowed — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// The wire name and status of the modelled refusal
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// A plan id of the minted form that no create ever minted. Its form — and its length —
/// is the one the Malware Protection plan suite pins (`tests/all/malware_protection_plan.rs`),
/// so a value outside it would be the parameter failure rather than an absence.
const ABSENT_PLAN_ID: &str = "abad1deaabad1deaabad";

/// An `InvestigationId` of the modelled form (`pattern ^[a-fA-F0-9\-]+$`) that no create
/// ever minted.
const ABSENT_INVESTIGATION_ID: &str = "7f1d2c3b-4a59-4e6f-8a0b-1c2d3e4f5a6b";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: String,
    body: Value,
}

impl Reply {
    /// The `message` the error body carries.
    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One call to this service, with `access_key_id` in the SigV4 credential scope.
    async fn call(
        &self,
        access_key_id: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://guardduty.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     guardduty/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
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

    /// One awsQuery call to IAM or STS, signed with the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {action:?}: the setup call must succeed, got {status} with {text}"
        );
        text
    }

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint
    /// a long-term access key for it, and answer that key id with the user's own ARN.
    async fn user(&self, user_name: &str, policy: Option<&str>) -> Principal {
        let created = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutUserPolicy",
                &[
                    ("UserName", user_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let minted = self
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        Principal {
            access_key_id: xml_element(&minted, "AccessKeyId")
                .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
            arn,
        }
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given),
    /// assume it through STS, and answer the session's access key id with the session ARN.
    async fn assumed_role(&self, role_name: &str, policy: Option<&str>) -> Principal {
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}",
            ::engine::emulator_account_id()
        );
        self.query(
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!(
                            "arn:aws:iam::{}:role/{role_name}",
                            ::engine::emulator_account_id()
                        ),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        Principal {
            access_key_id: xml_element(&assumed, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
            arn: xml_element(&assumed, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
        }
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
fn form_encode(value: &str) -> String {
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
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A policy document allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A policy document allowing every action of this service on everything, and explicitly
/// denying `denied` — the shape of the least-privilege policy the recorded repro assumes.
fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"{ACTION_PREFIX}:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":[\"{denied}\"],\"Resource\":\"*\"}}]}}"
    )
}

/// A policy document that denies `denied` and allows nothing.
fn deny_only(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Deny\",\"Action\":[\"{denied}\"],\"Resource\":\"*\"}}]}}"
    )
}

/// An ARN of this service in the region the suite signs for.
fn arn(resource: &str) -> String {
    arn_of(REGION, resource)
}

/// One request a decision is taken about: the call itself, the action and resource IAM
/// judges it by, and the status an AUTHORIZED caller gets — the operation's own answer,
/// which is how a served call is told from a refused one.
struct Gated {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    operation: &'static str,
    resource: String,
    served_status: u16,
}

/// The resources the table is written about, created by the account's own credential.
struct Fixture {
    detector_id: String,
    detector_arn: String,
    ip_set_id: String,
    rule_id: String,
    association_id: String,
    association_arn: String,
}

/// Create the detector, the IPSet under it, and read the published rule id the table
/// addresses — each through this service's own operations, signed as the account.
async fn fixture(emulator: &Emulator) -> Fixture {
    let created = emulator
        .call(ACCOUNT_KEY, "POST", "/detector", Some(&json!({"enable": true})))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"fixture\": expected CreateDetector 200, got {created:?}"
    );
    let detector_id = created.body["detectorId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let ip_set = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            &format!("/detector/{detector_id}/ipset"),
            Some(&json!({
                "name": "fixture list",
                "format": "TXT",
                "location": "s3://my-bucket/fixture.txt",
                "activate": false,
            })),
        )
        .await;
    assert_eq!(
        ip_set.status, 200,
        "case \"fixture\": expected CreateIPSet 200, got {ip_set:?}"
    );
    let rules = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/custom-detection-rule/rule",
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        rules.status, 200,
        "case \"fixture\": expected ListCustomDetectionRules 200, got {rules:?}"
    );
    let rule_id = rules.body["rules"]
        .as_array()
        .and_then(|rules| rules.first())
        .and_then(|rule| rule["ruleId"].as_str())
        .unwrap_or_else(|| panic!("the catalogue publishes at least one rule, got {rules:?}"))
        .to_string();
    let associated = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/custom-detection-rule/association",
            Some(&json!({"ruleId": rule_id, "mode": "DRY_RUN"})),
        )
        .await;
    assert_eq!(
        associated.status, 200,
        "case \"fixture\": expected CreateCustomDetectionRuleAssociation 200, got {associated:?}"
    );
    let association = &associated.body["ruleAssociation"];
    Fixture {
        detector_arn: arn(&format!("detector/{detector_id}")),
        ip_set_id: ip_set.body["ipSetId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        association_id: association["associationId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        // The ARN the create itself reports, so the resource the gate judges the
        // association against is the one a read of it reports.
        association_arn: association["arn"].as_str().unwrap_or_default().to_string(),
        detector_id,
        rule_id,
    }
}

/// The table of gated requests, one per class of resource and method the rule spans.
///
/// `suffix` keeps the rows that CREATE a resource distinct per principal, so the same
/// table can be replayed for each one.
fn gated_requests(fixture: &Fixture, suffix: &str) -> Vec<Gated> {
    let detector = &fixture.detector_id;
    let ip_set = &fixture.ip_set_id;
    let rule = &fixture.rule_id;
    vec![
        Gated {
            case: "a create of an investigation under one detector",
            method: "POST",
            path: format!("/detector/{detector}/investigation"),
            body: Some(json!({"triggerPrompt": "Analyze findings in my organization"})),
            operation: "CreateInvestigation",
            // The create names the sub-resource space it mints into, which is the path
            // the request arrived on.
            resource: arn(&format!("detector/{detector}/investigation")),
            // `CreateInvestigation` answers the recorded 200 (see that operation's own
            // `SUCCESS_STATUS`).
            served_status: 200,
        },
        Gated {
            case: "a read of one investigation the detector does not hold",
            method: "GET",
            path: format!("/detector/{detector}/investigation/{ABSENT_INVESTIGATION_ID}"),
            body: None,
            operation: "GetInvestigation",
            resource: arn(&format!(
                "detector/{detector}/investigation/{ABSENT_INVESTIGATION_ID}"
            )),
            // The gate fires BEFORE the lookup, so an authorized caller is the only one
            // that learns the investigation is absent.
            served_status: 404,
        },
        Gated {
            case: "an update of one IPSet",
            method: "POST",
            path: format!("/detector/{detector}/ipset/{ip_set}"),
            body: Some(json!({"name": format!("renamed {suffix}")})),
            operation: "UpdateIPSet",
            resource: arn(&format!("detector/{detector}/ipset/{ip_set}")),
            served_status: 200,
        },
        Gated {
            case: "a create in the IPSet collection",
            method: "POST",
            path: format!("/detector/{detector}/ipset"),
            body: Some(json!({
                "name": format!("list {suffix}"),
                "format": "TXT",
                "location": format!("s3://my-bucket/{suffix}.txt"),
                "activate": false,
            })),
            operation: "CreateIPSet",
            resource: arn(&format!("detector/{detector}/ipset")),
            served_status: 200,
        },
        Gated {
            case: "a create in the threat intel set collection",
            method: "POST",
            path: format!("/detector/{detector}/threatintelset"),
            body: Some(json!({
                "name": format!("intel {suffix}"),
                "format": "TXT",
                "location": format!("s3://my-bucket/intel-{suffix}.txt"),
                "activate": false,
            })),
            operation: "CreateThreatIntelSet",
            resource: arn(&format!("detector/{detector}/threatintelset")),
            served_status: 200,
        },
        Gated {
            case: "a read of one published custom detection rule",
            method: "GET",
            path: format!("/custom-detection-rule/rule/{rule}"),
            body: None,
            operation: "GetCustomDetectionRule",
            // A published rule is AWS-managed rather than a resource of this account, so
            // IAM judges the action against every resource of this service in the account
            // and Region addressed.
            resource: arn(ANY_RESOURCE),
            served_status: 200,
        },
        Gated {
            case: "a read of the custom detection rule catalogue",
            method: "POST",
            path: "/custom-detection-rule/rule".to_string(),
            body: Some(json!({})),
            operation: "ListCustomDetectionRules",
            resource: arn(ANY_RESOURCE),
            served_status: 200,
        },
        Gated {
            case: "a read of one association of a published rule",
            method: "GET",
            path: format!(
                "/custom-detection-rule/rule/{rule}/association/{}",
                fixture.association_id
            ),
            body: None,
            operation: "GetCustomDetectionRuleAssociation",
            // An association IS this account's own resource, and its ARN resource path is
            // not its URI path.
            resource: fixture.association_arn.clone(),
            served_status: 200,
        },
        Gated {
            case: "a read of the Malware Protection plan collection",
            method: "GET",
            path: "/malware-protection-plan".to_string(),
            body: None,
            operation: "ListMalwareProtectionPlans",
            // A listing names no one plan, so it is judged against the any-resource
            // wildcard too.
            resource: arn(ANY_RESOURCE),
            served_status: 200,
        },
        Gated {
            case: "a delete of one Malware Protection plan the account does not hold",
            method: "DELETE",
            path: format!("/malware-protection-plan/{ABSENT_PLAN_ID}"),
            body: None,
            operation: "DeleteMalwareProtectionPlan",
            resource: arn(&format!("malware-protection-plan/{ABSENT_PLAN_ID}")),
            served_status: 404,
        },
        Gated {
            case: "a read of a resource's tags",
            method: "GET",
            path: format!("/tags/{}", uri_label(&fixture.detector_arn)),
            body: None,
            operation: "ListTagsForResource",
            // The tagging surface binds its target's whole ARN as its one label, so that
            // ARN is already the resource the action is judged against.
            resource: fixture.detector_arn.clone(),
            served_status: 200,
        },
        Gated {
            case: "a write of a resource's tags",
            method: "POST",
            path: format!("/tags/{}", uri_label(&fixture.detector_arn)),
            body: Some(json!({"tags": {"stage": "beta"}})),
            operation: "TagResource",
            resource: fixture.detector_arn.clone(),
            // `TagResourceResponse` carries no members at `@httpResponseCode` 204
            // (`smithy_query(operation="TagResource")`).
            served_status: 204,
        },
        Gated {
            case: "a removal of a resource's tags",
            method: "DELETE",
            path: format!("/tags/{}?tagKeys=stage", uri_label(&fixture.detector_arn)),
            body: None,
            operation: "UntagResource",
            resource: fixture.detector_arn.clone(),
            served_status: 204,
        },
    ]
}

/// The refusal IAM words for a request no identity-based document allows.
fn default_deny_message(principal: &str, operation: &str, resource: &str) -> String {
    let action = format!("{ACTION_PREFIX}:{operation}");
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// The refusal IAM words for a request an explicit `Deny` in the principal's own
/// identity-based policy refuses — the message the recorded least-privilege probe expects.
fn explicit_deny_message(principal: &str, operation: &str, resource: &str) -> String {
    let action = format!("{ACTION_PREFIX}:{operation}");
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in an identity-based policy"
    )
}

/// Assert `reply` is the modelled refusal, with `expected` as its message.
fn assert_refused(case: &str, request: &Gated, reply: &Reply, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case {case:?} / {:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got {} {:?} \
         with {:?}",
        request.case,
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.message(),
        expected,
        "case {case:?} / {:?}: expected the refusal to name the principal, the action and \
         the resource, got {:?}",
        request.case,
        reply.message()
    );
}

/// RULE: every gated operation refuses a principal whose identity-based policies allow
/// nothing, with the modelled `AccessDeniedException` (403) naming the principal, the
/// action and the resource the request addressed — and changes nothing.
#[tokio::test]
async fn a_principal_no_policy_allows_is_refused_every_gated_operation() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let principal = emulator.user("zero_policy", None).await;

    for request in gated_requests(&fixture, "zero") {
        let reply = emulator
            .call(
                &principal.access_key_id,
                request.method,
                &request.path,
                request.body.as_ref(),
            )
            .await;
        let expected =
            default_deny_message(&principal.arn, request.operation, &request.resource);
        assert_refused("no policy at all", &request, &reply, &expected);
    }

    // The refused mutations changed nothing: no investigation was initiated, the IPSet the
    // update addressed still carries the name its create stored, no second IPSet exists,
    // and the detector carries no tag.
    let investigations = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            &format!("/detector/{}/investigation/list", fixture.detector_id),
            Some(&json!({})),
        )
        .await;
    let count = investigations.body["investigations"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        count, 0,
        "case \"a refused mutation writes nothing\": expected no investigation, got {:?}",
        investigations.body
    );
    let listed = emulator
        .call(
            ACCOUNT_KEY,
            "GET",
            &format!("/detector/{}/ipset", fixture.detector_id),
            None,
        )
        .await;
    let ids: Vec<&str> = listed.body["ipSetIds"]
        .as_array()
        .map(|entries| entries.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![fixture.ip_set_id.as_str()],
        "case \"a refused mutation writes nothing\": expected only the fixture's IPSet, got \
         {ids:?}"
    );
    let read = emulator
        .call(
            ACCOUNT_KEY,
            "GET",
            &format!("/detector/{}/ipset/{}", fixture.detector_id, fixture.ip_set_id),
            None,
        )
        .await;
    assert_eq!(
        read.body["name"].as_str().unwrap_or_default(),
        "fixture list",
        "case \"a refused mutation writes nothing\": expected the name untouched, got {read:?}"
    );
    let tags = emulator
        .call(
            ACCOUNT_KEY,
            "GET",
            &format!("/tags/{}", uri_label(&fixture.detector_arn)),
            None,
        )
        .await;
    assert_eq!(
        tags.body["tags"],
        json!({}),
        "case \"a refused mutation writes nothing\": expected no tag, got {tags:?}"
    );
}

/// RULE: the same signed call flips between served and refused with what the caller's
/// identity-based policies say — a grant of that action on that resource serves it, and
/// anything short of one (a grant of another action, a grant on another resource, an
/// explicit deny beside a wildcard grant) refuses it.
#[tokio::test]
async fn what_the_policies_say_decides_the_same_signed_call() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    // The policies each row of the table is replayed under: a label, and the document, built
    // from the row's own action and resource.
    type Document = fn(&Gated) -> String;
    let served: &[(&str, Document)] = &[
        ("an allow of this action on this resource", |request| {
            allow(
                &format!("{ACTION_PREFIX}:{}", request.operation),
                &request.resource,
            )
        }),
        ("an allow of every action on everything", |_| {
            allow(&format!("{ACTION_PREFIX}:*"), "*")
        }),
    ];
    let refused: &[(&str, Document)] = &[
        ("an allow of another action", |_| {
            allow(&format!("{ACTION_PREFIX}:ListDetectors"), "*")
        }),
        ("an allow on another resource", |request| {
            allow(
                &format!("{ACTION_PREFIX}:{}", request.operation),
                &arn("detector/00000000000000000000000000000000"),
            )
        }),
        ("an explicit deny beside a wildcard allow", |request| {
            allow_all_but(&format!("{ACTION_PREFIX}:{}", request.operation))
        }),
    ];

    for (index, (case, document)) in served.iter().enumerate() {
        for (row, request) in gated_requests(&fixture, &format!("served{index}"))
            .into_iter()
            .enumerate()
        {
            let principal = emulator
                .user(&format!("served_{index}_{row}"), Some(&document(&request)))
                .await;
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    request.method,
                    &request.path,
                    request.body.as_ref(),
                )
                .await;
            assert_eq!(
                reply.status, request.served_status,
                "case {case:?} / {:?}: expected the operation's own {} answer, got {} {:?} \
                 with {:?}",
                request.case, request.served_status, reply.status, reply.error_code, reply.body
            );
        }
    }

    for (index, (case, document)) in refused.iter().enumerate() {
        for (row, request) in gated_requests(&fixture, &format!("refused{index}"))
            .into_iter()
            .enumerate()
        {
            let principal = emulator
                .user(&format!("refused_{index}_{row}"), Some(&document(&request)))
                .await;
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    request.method,
                    &request.path,
                    request.body.as_ref(),
                )
                .await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (ACCESS_DENIED_STATUS, ACCESS_DENIED),
                "case {case:?} / {:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got \
                 {} {:?} with {:?}",
                request.case,
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }
}

/// RULE: the account's own published credential is not a principal the IAM store holds
/// policies for, so it is served every gated operation — the position AWS account root
/// credentials are in.
#[tokio::test]
async fn the_accounts_own_credential_is_served_every_gated_operation() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for request in gated_requests(&fixture, "account") {
        let reply = emulator
            .call(
                ACCOUNT_KEY,
                request.method,
                &request.path,
                request.body.as_ref(),
            )
            .await;
        assert_eq!(
            reply.status, request.served_status,
            "case \"the account's own credential\" / {:?}: expected the operation's own {} \
             answer, got {} {:?} with {:?}",
            request.case, request.served_status, reply.status, reply.error_code, reply.body
        );
    }
}

/// RULE: an assumed-role session is judged on the ROLE's policies, so an explicit `Deny`
/// on ONE action refuses exactly that action's calls — naming the session ARN, the action
/// and the resource — and leaves every other gated call served.
///
/// This is the recorded least-privilege probe: a session assumed into a role carrying an
/// explicit `Deny` on a single `guardduty` action.
#[tokio::test]
async fn an_explicit_deny_on_one_action_refuses_exactly_that_action() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    // Rows differ along the DENIED action: a create, an update and a read, each with a
    // different kind of resource ARN.
    for (index, denied) in ["CreateInvestigation", "UpdateIPSet", "GetCustomDetectionRule"]
        .into_iter()
        .enumerate()
    {
        let suffix = format!("denied{index}");
        let principal = emulator
            .assumed_role(
                &format!("least_privilege_{index}"),
                Some(&allow_all_but(&format!("{ACTION_PREFIX}:{denied}"))),
            )
            .await;
        for request in gated_requests(&fixture, &suffix) {
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    request.method,
                    &request.path,
                    request.body.as_ref(),
                )
                .await;
            if request.operation == denied {
                let expected =
                    explicit_deny_message(&principal.arn, request.operation, &request.resource);
                assert_refused(denied, &request, &reply, &expected);
            } else {
                assert_eq!(
                    reply.status, request.served_status,
                    "case {denied:?} / {:?}: a deny on one action leaves the others served, \
                     got {} {:?} with {:?}",
                    request.case, reply.status, reply.error_code, reply.body
                );
            }
        }
    }
}

/// RULE: a role that states ONLY a deny refuses that action too — an explicit deny wins
/// whatever else is absent — and the session's other gated calls fall to IAM's default
/// deny rather than being served.
#[tokio::test]
async fn a_role_stating_only_a_deny_refuses_that_action_and_grants_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let denied = "CreateInvestigation";
    let principal = emulator
        .assumed_role(
            "deny_only",
            Some(&deny_only(&format!("{ACTION_PREFIX}:{denied}"))),
        )
        .await;

    for request in gated_requests(&fixture, "denyonly") {
        let reply = emulator
            .call(
                &principal.access_key_id,
                request.method,
                &request.path,
                request.body.as_ref(),
            )
            .await;
        let expected = if request.operation == denied {
            explicit_deny_message(&principal.arn, request.operation, &request.resource)
        } else {
            default_deny_message(&principal.arn, request.operation, &request.resource)
        };
        assert_refused("a deny and nothing else", &request, &reply, &expected);
    }
}

/// RULE: an operation whose model declares no `AccessDeniedException` is not judged, since
/// there is no modelled answer for it to refuse an unauthorized caller with.
///
/// The rows are operations of three different URI spaces, each one absent from
/// [`GATED_OPERATIONS`] — asserted here rather than assumed, so a model change moves this
/// test rather than letting it pass vacuously.
#[tokio::test]
async fn an_operation_modelling_no_refusal_is_not_judged() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let principal = emulator.user("ungated_probe", None).await;

    // (the operation, its method, its path, its body, the answer it gives this caller)
    let rows: &[(&str, &str, String, Option<Value>, u16)] = &[
        ("ListDetectors", "GET", "/detector".to_string(), None, 200),
        (
            "GetDetector",
            "GET",
            format!("/detector/{}", fixture.detector_id),
            None,
            200,
        ),
        (
            "GetIPSet",
            "GET",
            format!(
                "/detector/{}/ipset/{}",
                fixture.detector_id, fixture.ip_set_id
            ),
            None,
            200,
        ),
        (
            "ListFilters",
            "GET",
            format!("/detector/{}/filter", fixture.detector_id),
            None,
            200,
        ),
        (
            "UpdateDetector",
            "POST",
            format!("/detector/{}", fixture.detector_id),
            Some(json!({"enable": true})),
            200,
        ),
    ];
    for (operation, method, path, body, expected) in rows {
        assert!(
            !GATED_OPERATIONS.contains(operation),
            "case {operation:?}: this operation models the refusal, so it belongs in the \
             gated table instead of here"
        );
        let reply = emulator
            .call(&principal.access_key_id, method, path, body.as_ref())
            .await;
        assert_eq!(
            reply.status, *expected,
            "case {operation:?}: expected the operation's own {expected} answer for an \
             unjudged caller, got {} {:?} with {:?}",
            reply.status, reply.error_code, reply.body
        );
    }
}
