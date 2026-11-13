// SPDX-License-Identifier: CC0-1.0

//! The authorization refusal a resource string earns is the same in every
//! protocol this service declares.
//!
//! The rule: an authorization decision is taken from what the request STATES —
//! the identity it is signed with, the action its operation asks for and the
//! resource string it carries — and never from the encoding those arrived in, so
//! a resource string that names no automation rule of this service is refused
//! `AccessDeniedException` (403) carrying the IAM denial built from that request
//! in each of the service's wire protocols alike.
//!
//! This service declares TWO (`awsJson1_0` and `rpcv2Cbor`, from
//! `smithy_query(query="service_info")`) and a client may reach it over either,
//! so the rows vary the PROTOCOL as well as the operation refusing and the form
//! the resource string takes — the latter two being
//! [`crate::unauthorized_resource_message`]'s own tables, so the suites cannot
//! disagree about which operations and which forms the rule covers. That suite
//! asserts the sentence over the awsJson1_0 wire; this one asserts its
//! independence from the wire.
//!
//! Every row sends the operation's resource member and NOTHING else, in both
//! protocols, so the two are exactly comparable — and so each row also shows the
//! decision is taken before any other member of the request is judged, which is
//! the order [`crate::resource_arn_refusal_order`] pins.
//!
//! Every case drives the service through the same [`MultiServiceRouter`] the
//! emulator binary mounts.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::CborCodec;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{call, emulator, error_code, AMBIENT_ACCESS_KEY_ID};
use crate::error_payloads::error_message;
use crate::unauthorized_resource_message::{
    expected_message, unauthorized_resources, RESOURCE_OPERATIONS,
};

/// The protocols a request can reach this service in, as the model declares them
/// (`smithy_query(query="service_info")` -> `["awsJson1_0", "rpcv2Cbor"]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    AwsJson,
    RpcV2Cbor,
}

/// Every protocol each row is run in.
const WIRES: [Wire; 2] = [Wire::AwsJson, Wire::RpcV2Cbor];

/// The modelled input shape of each operation that names a resource by ARN, and
/// the member it names it in — `ruleArn` on the three rule operations and
/// `resourceArn` on the three tagging ones
/// (`smithy_query(query="operation",operation="GetAutomationRule,UpdateAutomationRule,DeleteAutomationRule,TagResource,UntagResource,ListTagsForResource")`).
///
/// A row's body is written against these, so the request a case sends is shaped
/// by the MODEL rather than by a literal this suite carries.
fn modelled_resource_member(operation: &str) -> (&'static Schema, &'static str) {
    match operation {
        "GetAutomationRule" => (types::GET_AUTOMATION_RULE_INPUT_SCHEMA, "ruleArn"),
        "UpdateAutomationRule" => (types::UPDATE_AUTOMATION_RULE_INPUT_SCHEMA, "ruleArn"),
        "DeleteAutomationRule" => (types::DELETE_AUTOMATION_RULE_INPUT_SCHEMA, "ruleArn"),
        "TagResource" => (types::TAG_RESOURCE_INPUT_SCHEMA, "resourceArn"),
        "UntagResource" => (types::UNTAG_RESOURCE_INPUT_SCHEMA, "resourceArn"),
        "ListTagsForResource" => (types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA, "resourceArn"),
        other => panic!("{other} names no resource by ARN"),
    }
}

/// One string member of a request, as a serializable body: what a row sends.
struct OneMember<'a> {
    member: &'a Schema,
    value: &'a str,
}

impl SerializableStruct for OneMember<'_> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        serializer.write_string(self.member, self.value)
    }
}

/// The member schema the model declares for `operation`'s resource ARN.
fn resource_member_schema(operation: &str) -> &'static Schema {
    let (input_schema, member_name) = modelled_resource_member(operation);
    input_schema
        .members()
        .iter()
        .copied()
        .find(|member| member.member_name() == Some(member_name))
        .unwrap_or_else(|| panic!("{operation}'s input models a {member_name} member"))
}

/// One request's `(status, error code, message)`, read out of whichever envelope
/// the protocol answers in.
async fn answer(
    router: &MultiServiceRouter,
    wire: Wire,
    operation: &str,
    resource: &str,
) -> (u16, String, String) {
    match wire {
        Wire::AwsJson => {
            let (_, member_name) = modelled_resource_member(operation);
            let (status, parsed) =
                call(router, operation, &json!({member_name: resource})).await;
            (status, error_code(&parsed), error_message(&parsed))
        }
        Wire::RpcV2Cbor => cbor_answer(router, operation, resource).await,
    }
}

/// Dispatch the rpcv2Cbor request a CBOR client sends — `POST
/// /service/{Service}/operation/{Operation}`, the protocol's `smithy-protocol`
/// marker, and a body encoded by the protocol's OWN codec — and decode the error
/// envelope it answers with.
async fn cbor_answer(
    router: &MultiServiceRouter,
    operation: &str,
    resource: &str,
) -> (u16, String, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://aco-automation.us-east-1.amazonaws.com/service/\
             ComputeOptimizerAutomationService/operation/{operation}"
        ))
        .header("content-type", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        // The router keys on the SigV4 credential scope's service field
        // (`aco-automation`, this service's signing name), the same way the
        // awsJson harness does; the signature itself is not checked.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={AMBIENT_ACCESS_KEY_ID}/20260101/us-east-1/\
                 aco-automation/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(cbor_body(operation, resource))
        .expect("a well-formed rpcv2Cbor request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    // A served request answers the operation's OUTPUT shape, which is no error
    // envelope: a row that expects one asserts the refusal's status first.
    let (code, message) = if status == 200 {
        (String::new(), String::new())
    } else {
        cbor_envelope(response.body())
    };
    (status, code, message)
}

/// `resource` as the rpcv2Cbor body a client sends at `operation`'s resource
/// member: written through the protocol's own codec against the member schema the
/// model declares, so nothing here encodes CBOR by hand.
fn cbor_body(operation: &str, resource: &str) -> bytes::Bytes {
    let (input_schema, _) = modelled_resource_member(operation);
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(
            input_schema,
            &OneMember {
                member: resource_member_schema(operation),
                value: resource,
            },
        )
        .expect("the resource member serializes");
    bytes::Bytes::from(serializer.finish())
}

/// The `__type` and `message` of an rpcv2Cbor error envelope, decoded as CBOR
/// rather than scanned as text, so the message asserted on is the WHOLE message.
fn cbor_envelope(body: &[u8]) -> (String, String) {
    let mut decoder = aws_smithy_cbor::Decoder::new(body);
    let declared = decoder
        .map()
        .unwrap_or_else(|error| panic!("the error envelope is not a CBOR map: {error}"));
    let mut code = String::new();
    let mut message = String::new();
    // A definite-length map states how many entries it holds; an
    // indefinite-length one — which is what this service's codec writes a
    // modelled error as — ends at a break.
    let mut remaining = declared.unwrap_or(u64::MAX);
    while remaining > 0 {
        if declared.is_none() {
            let next = decoder
                .datatype()
                .unwrap_or_else(|error| panic!("the error envelope ends mid-entry: {error}"));
            if next == aws_smithy_cbor::data::Type::Break {
                break;
            }
        }
        let key = decoder
            .str()
            .unwrap_or_else(|error| panic!("an envelope key is not text: {error}"))
            .into_owned();
        let value = decoder
            .str()
            .unwrap_or_else(|error| panic!("the value at {key:?} is not text: {error}"))
            .into_owned();
        match key.as_str() {
            "__type" => {
                code = value
                    .rsplit(['#', '/'])
                    .next()
                    .unwrap_or_default()
                    .to_string();
            }
            "message" | "Message" => message = value,
            _ => {}
        }
        remaining -= 1;
    }
    (code, message)
}

/// The refusal names the caller, the action and the resource string exactly as
/// sent, whichever protocol carried the request.
#[tokio::test]
async fn the_denial_is_the_same_in_every_declared_protocol() {
    let router = emulator();
    // The key every row presents names no session, so it speaks for the identity
    // this emulator publishes — the one `sts:GetCallerIdentity` answers with.
    let principal = sts_state::ambient_identity().arn;

    for wire in WIRES {
        for operation in RESOURCE_OPERATIONS {
            for (form, resource) in unauthorized_resources() {
                let case = format!("{wire:?}/{operation} on {form}");
                let (status, code, message) = answer(&router, wire, operation, &resource).await;
                assert_eq!(
                    (status, code.as_str()),
                    (403, "AccessDeniedException"),
                    "case {case:?}: expected 403 AccessDeniedException, got {status} {code:?}: \
                     {message:?}"
                );
                assert_eq!(
                    message,
                    expected_message(&principal, operation, &resource),
                    "case {case:?}: expected the IAM denial built from the request, got \
                     {message:?}"
                );
            }
        }
    }
}

/// The boundary, in both protocols: the gate judges the FORM of the resource
/// string, never whether the rule exists — so a well-formed `RuleArn` no rule has
/// passes it and is answered by the operation's own lookup.
#[tokio::test]
async fn a_well_formed_arn_reaches_the_operation_in_every_declared_protocol() {
    let router = emulator();
    let absent = format!(
        "arn:aws:compute-optimizer::{}:automation-rule/0123456789abcdef",
        engine::EMULATOR_ACCOUNT_ID
    );

    for wire in WIRES {
        // The two readers model the resource ARN and nothing else, so a
        // resource-member-only body is a complete request for them and the only
        // thing that can decide the answer is the ARN.
        for operation in ["GetAutomationRule", "ListTagsForResource"] {
            let case = format!("{wire:?}/{operation} on a well-formed ARN of no rule");
            let (status, code, message) = answer(&router, wire, operation, &absent).await;
            assert_eq!(
                (status, code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {status} {code:?}: \
                 {message:?}"
            );
            assert!(
                !message.contains("is not authorized to perform"),
                "case {case:?}: expected the operation's own not-found, got {message:?}"
            );
        }
    }
}

/// And the same string that names a rule IS served in both protocols: the gate
/// refuses what no permission can cover, and nothing else.
#[tokio::test]
async fn a_string_naming_a_rule_is_served_in_every_declared_protocol() {
    let router = emulator();
    let created = call(
        &router,
        "CreateAutomationRule",
        &crate::automation_rule_lifecycle::minimal_rule("served-in-both"),
    )
    .await;
    assert_eq!(
        created.0, 200,
        "setup: CreateAutomationRule must succeed, got {} and {}",
        created.0, created.1
    );
    let rule_arn = created
        .1
        .get("ruleArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: the create reports a ruleArn, got {}", created.1))
        .to_string();

    for wire in WIRES {
        let case = format!("{wire:?}/GetAutomationRule on the rule's own ARN");
        let (status, code, message) = answer(&router, wire, "GetAutomationRule", &rule_arn).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (200, "", ""),
            "case {case:?}: expected the rule to be served, got {status} {code:?}: {message:?}"
        );
    }
}
