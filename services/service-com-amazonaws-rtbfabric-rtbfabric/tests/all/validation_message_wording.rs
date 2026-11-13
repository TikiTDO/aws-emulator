// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the words this service answers a boundary constraint refusal
//! with: the `1 validation error detected: …` envelope the framework renders before any
//! handler runs.
//!
//! The rule under test is about the whole envelope, not about one recorded request: a
//! clause names the member path and the requirement broken and carries NO value the
//! request submitted, whichever member and whichever constraint class, and a `@pattern`
//! requirement quotes the expression with the whole-value anchors the model wraps it in
//! dropped. The table spans what the rule generalises over — the constraint class
//! (`@pattern`, both ends of `@range`, `@length`, an absent `@required` member), where the
//! member sits (a body member, a URI label, a list element) and which operation
//! carries it — and the recorded case (a `vpcId` outside its pattern on
//! `CreateRequesterGateway`) is one row of it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The modelled client error every input-constraint refusal of this service answers with
/// (`smithy_query(query="operation", operation="CreateRequesterGateway")`: `errors`
/// carries `ValidationException` at `httpError` 400).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The one emulated service, behind the same `MultiServiceRouter` the production router
/// assembles, so these calls take the real deserialize -> validate -> handle -> serialize
/// path and the refusal is the one the centralized walk renders.
struct Fabric {
    router: MultiServiceRouter,
}

impl Fabric {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one restJson1 request and return `(status, parsed body)`.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let bytes = match body {
            Some(body) => Bytes::from(serde_json::to_vec(&body).expect("serialize the body")),
            None => Bytes::new(),
        };
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://rtbfabric.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rtbfabric/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes)
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
}

/// A `CreateRequesterGateway` body whose members all satisfy their modelled constraints,
/// which each row then breaks in exactly one place.
fn requester_body(client_token: &str) -> Value {
    json!({
        "vpcId": "vpc-0123456789abcdef0",
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
        "clientToken": client_token,
    })
}

fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// One row of the refusal table: the call that breaks one modelled constraint, the clause
/// the message must carry, and the caller-supplied text the message must NOT carry.
struct Row {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<fn() -> Value>,
    /// The text after `1 validation error detected: `.
    clause: &'static str,
    /// What the request submitted at the offending member, which AWS does not echo.
    submitted: Option<&'static str>,
}

/// The expressions a `@pattern` clause quotes, as the member's modelled pattern reads
/// once the whole-value `^`/`$` the model wraps it in are dropped. From
/// `smithy_query(query="operation", operation="CreateRequesterGateway,GetRequesterGateway,\
/// ListTagsForResource")` and `smithy_query(query="shape", shape="SubnetIdList")`.
const VPC_ID_PATTERN: &str = r"vpc-[a-f0-9]{8,17}";
const GATEWAY_ID_PATTERN: &str = r"rtb-gw-[a-z0-9-]{1,25}";
const SUBNET_ID_PATTERN: &str = r"subnet-\w{8,17}";
const DESCRIPTION_PATTERN: &str = r"([\p{L}\p{Z}\p{N}_.:/=+\-@]*)";
const RESOURCE_ARN_PATTERN: &str = r"arn:aws:rtbfabric:[a-zA-Z0-9_-]+:[0-9]{12}:gateway/[a-zA-Z0-9-]+(/link/[a-zA-Z0-9-]+(/routing-rule/[a-zA-Z0-9-]+)?)?";

/// A `vpcId` the model's `@length` admits (12..21) and its `@pattern` does not, so the
/// refusal is about the pattern alone. Three spellings of the same class: the one the
/// recording sent, one that is uppercase where the expression wants lowercase hex, and one
/// whose suffix carries a character the expression's class excludes.
fn body_with_vpc_id(vpc_id: &str) -> Value {
    let mut body = requester_body("token-pattern");
    body["vpcId"] = json!(vpc_id);
    body
}

fn recorded_vpc_id() -> Value {
    body_with_vpc_id("not-a-valid-vpc-id")
}

fn uppercase_vpc_id() -> Value {
    body_with_vpc_id("vpc-ABCDEF123456")
}

fn non_hex_vpc_id() -> Value {
    body_with_vpc_id("vpc-0123456789abcdefg")
}

/// A `description` outside its `@pattern` (`*` is in none of the character classes the
/// expression admits), everything else well formed.
fn bad_description() -> Value {
    let mut body = requester_body("token-description");
    body["description"] = json!("not*allowed");
    body
}

/// A `description` the `@pattern` admits and the `@length` maximum of 255 does not.
fn over_long_description() -> Value {
    let mut body = requester_body("token-long-description");
    body["description"] = json!("a".repeat(256));
    body
}

/// A `subnetIds` ELEMENT outside `SubnetId`'s `@pattern`, at a length the element's
/// `@length` admits (15..24), so the clause is about the element's pattern alone.
fn bad_subnet_element() -> Value {
    let mut body = requester_body("token-subnet");
    body["subnetIds"] = json!(["subnet-0123456789abcdef0", "subnet-ABCDEF.0"]);
    body
}

/// A body with the `@required` `vpcId` left out altogether.
fn absent_vpc_id() -> Value {
    json!({
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
        "clientToken": "token-absent",
    })
}

/// The rows differ along the constraint class, where the offending member sits and which
/// operation carries it; the first is the recorded case.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "a body member outside its pattern, as recorded",
            method: "POST",
            path: "/requester-gateway",
            body: Some(recorded_vpc_id),
            clause: concat!(
                "Value at 'vpcId' failed to satisfy constraint: Member must satisfy regular ",
                "expression pattern: vpc-[a-f0-9]{8,17}"
            ),
            submitted: Some("not-a-valid-vpc-id"),
        },
        Row {
            name: "the same member, uppercase where the expression wants lowercase hex",
            method: "POST",
            path: "/requester-gateway",
            body: Some(uppercase_vpc_id),
            clause: concat!(
                "Value at 'vpcId' failed to satisfy constraint: Member must satisfy regular ",
                "expression pattern: vpc-[a-f0-9]{8,17}"
            ),
            submitted: Some("vpc-ABCDEF123456"),
        },
        Row {
            name: "the same member with a character the expression's class excludes",
            method: "POST",
            path: "/requester-gateway",
            body: Some(non_hex_vpc_id),
            clause: concat!(
                "Value at 'vpcId' failed to satisfy constraint: Member must satisfy regular ",
                "expression pattern: vpc-[a-f0-9]{8,17}"
            ),
            submitted: Some("vpc-0123456789abcdefg"),
        },
        Row {
            name: "another body member, whose pattern carries its own group",
            method: "POST",
            path: "/requester-gateway",
            body: Some(bad_description),
            clause: concat!(
                "Value at 'description' failed to satisfy constraint: Member must satisfy ",
                r"regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*)"
            ),
            submitted: Some("not*allowed"),
        },
        Row {
            name: "a list element outside the element shape's pattern",
            method: "POST",
            path: "/requester-gateway",
            body: Some(bad_subnet_element),
            clause: concat!(
                "Value at 'subnetIds.2.member' failed to satisfy constraint: Member must ",
                r"satisfy regular expression pattern: subnet-\w{8,17}"
            ),
            submitted: Some("subnet-ABCDEF.0"),
        },
        Row {
            name: "a URI label outside its pattern",
            method: "GET",
            path: "/requester-gateway/not-a-gateway-id",
            body: None,
            clause: concat!(
                "Value at 'gatewayId' failed to satisfy constraint: Member must satisfy ",
                "regular expression pattern: rtb-gw-[a-z0-9-]{1,25}"
            ),
            submitted: Some("not-a-gateway-id"),
        },
        Row {
            name: "a URI label whose pattern spells a whole ARN",
            method: "GET",
            path: "/tags/arn:aws:rtbfabric:us-west-2:123456789012:gateway",
            body: None,
            clause: concat!(
                "Value at 'resourceArn' failed to satisfy constraint: Member must satisfy ",
                "regular expression pattern: arn:aws:rtbfabric:[a-zA-Z0-9_-]+:[0-9]{12}:",
                "gateway/[a-zA-Z0-9-]+(/link/[a-zA-Z0-9-]+(/routing-rule/[a-zA-Z0-9-]+)?)?"
            ),
            submitted: Some("arn:aws:rtbfabric:us-west-2:123456789012:gateway"),
        },
        Row {
            name: "a query parameter one past its range maximum",
            method: "GET",
            path: "/requester-gateways?maxResults=11",
            body: None,
            clause: concat!(
                "Value at 'maxResults' failed to satisfy constraint: Member must have value ",
                "less than or equal to 10"
            ),
            submitted: Some("11"),
        },
        Row {
            name: "a query parameter one below its range minimum",
            method: "GET",
            path: "/requester-gateways?maxResults=0",
            body: None,
            clause: concat!(
                "Value at 'maxResults' failed to satisfy constraint: Member must have value ",
                "greater than or equal to 1"
            ),
            submitted: Some("0"),
        },
        Row {
            name: "a body member one past its length maximum",
            method: "POST",
            path: "/requester-gateway",
            body: Some(over_long_description),
            clause: concat!(
                "Value at 'description' failed to satisfy constraint: Member must have length ",
                "less than or equal to 255"
            ),
            submitted: None,
        },
        Row {
            name: "a required body member left out altogether",
            method: "POST",
            path: "/requester-gateway",
            body: Some(absent_vpc_id),
            clause: "Value null at 'vpcId' failed to satisfy constraint: Member must not be null",
            submitted: None,
        },
    ]
}

#[tokio::test]
async fn a_constraint_refusal_names_the_member_and_the_requirement_and_echoes_no_value() {
    let fabric = Fabric::new();
    for row in rows() {
        let case = row.name;
        let (status, body) = fabric
            .send(row.method, row.path, row.body.map(|build| build()))
            .await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled ValidationException status, got {status} {body:?}"
        );
        assert_eq!(
            text(&body, "__type"),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected the modelled error code, got {:?}",
            body["__type"]
        );

        let expected = format!("1 validation error detected: {}", row.clause);
        let stated = text(&body, "message");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        if let Some(submitted) = row.submitted {
            assert!(
                !stated.contains(submitted),
                "case {case:?}: expected the submitted value {submitted:?} to be withheld, got {stated:?}"
            );
        }
    }
}

/// A `@pattern` clause quotes the expression a client can apply as its own check: the
/// anchors the model wraps the published expression in are not part of what is quoted, and
/// neither is the submitted value.
#[tokio::test]
async fn a_pattern_clause_quotes_the_expression_without_the_models_anchors() {
    let fabric = Fabric::new();
    for row in rows() {
        let case = row.name;
        let (_, body) = fabric
            .send(row.method, row.path, row.body.map(|build| build()))
            .await;
        let stated = text(&body, "message");
        let Some((_, quoted)) = stated.split_once("regular expression pattern: ") else {
            continue;
        };
        assert!(
            !quoted.starts_with('^') && !quoted.ends_with('$'),
            "case {case:?}: expected the quoted expression to carry no whole-value anchors, got {quoted:?}"
        );
        let published = [
            VPC_ID_PATTERN,
            GATEWAY_ID_PATTERN,
            SUBNET_ID_PATTERN,
            DESCRIPTION_PATTERN,
            RESOURCE_ARN_PATTERN,
        ];
        assert!(
            published.contains(&quoted),
            "case {case:?}: expected one of the modelled expressions unanchored, got {quoted:?}"
        );
    }
}

/// The assertions the recorded check makes AFTER the message: the refused create persisted
/// nothing, the same call with a well-formed `vpcId` is accepted in a non-terminal status,
/// the reader answers the persisted `vpcId`, and the gateway settles ACTIVE once the clock
/// has made the transition due.
#[tokio::test]
async fn a_refused_create_persists_nothing_and_a_well_formed_one_round_trips() {
    let fabric = Fabric::new();

    let (status, refused) = fabric
        .send("POST", "/requester-gateway", Some(recorded_vpc_id()))
        .await;
    assert_eq!(
        status, 400,
        "expected the malformed vpcId to be refused, got {status} {refused:?}"
    );

    let (status, listed) = fabric.send("GET", "/requester-gateways", None).await;
    assert_eq!(
        status, 200,
        "expected ListRequesterGateways to answer 200, got {status} {listed:?}"
    );
    assert_eq!(
        listed["gatewayIds"],
        json!([]),
        "expected the refused create to have persisted no gateway, got {listed:?}"
    );

    let well_formed = "vpc-0123456789abcdef0";
    let (status, created) = fabric
        .send("POST", "/requester-gateway", Some(requester_body("token-ok")))
        .await;
    assert_eq!(
        status, 200,
        "expected the well-formed create to be accepted, got {status} {created:?}"
    );
    let gateway_id = text(&created, "gatewayId");
    assert!(
        ["PENDING_CREATION", "ACTIVE"].contains(&text(&created, "status").as_str()),
        "expected the create to report a modelled creation status, got {created:?}"
    );

    let path = format!("/requester-gateway/{gateway_id}");
    let (status, read) = fabric.send("GET", &path, None).await;
    assert_eq!(
        status, 200,
        "expected the reader to resolve {gateway_id:?}, got {status} {read:?}"
    );
    assert_eq!(
        text(&read, "vpcId"),
        well_formed,
        "expected the reader to answer the persisted vpcId, got {read:?}"
    );

    // Creation is asynchronous, so the settled status is waited for by looking again
    // until it holds rather than for any particular length of time. The deadline is
    // generous: it bounds a hang, it does not time the transition.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let settled = loop {
        let (status, read) = fabric.send("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "expected the reader to keep answering while the gateway settles, got {status} {read:?}"
        );
        let reported = text(&read, "status");
        if reported == "ACTIVE" {
            break reported;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected the gateway to settle ACTIVE, still {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        settled, "ACTIVE",
        "expected the gateway to settle ACTIVE, got {settled:?}"
    );
}
