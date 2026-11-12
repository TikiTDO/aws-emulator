// SPDX-License-Identifier: CC0-1.0

//! Resolving a request's `InstanceId` to an Amazon Connect instance — the stage every
//! request to this service passes BEFORE its body is validated against the model's own
//! constraints.
//!
//! # The rule
//!
//! Contact Lens is reached through Amazon Connect's own front door, which resolves the
//! `InstanceId` the request names to an Amazon Connect instance before anything else looks
//! at the request. An identifier that is not one an Amazon Connect instance can be
//! addressed by resolves to no instance, and such a request is refused `UnauthorizedException`
//! carrying the fixed text `Unauthorized` — not as a statement about the body's `@length` or
//! `@pattern` clauses, which are never reached, and not as a statement about whether the
//! contact has a live analysis session, which is the stage behind this one.
//!
//! An Amazon Connect instance is addressed by the identifier Amazon Connect's OWN model
//! declares for it: a GUID, optionally prefixed by the instance's ARN
//! (`^(arn:(aws|aws-us-gov):connect:[a-z]{2}-[a-z]+-[0-9]{1}:[0-9]{1,20}:instance/)?[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`,
//! the `InstanceId` member of `com.amazonaws.connect#DescribeContactFlowAliasRequest` and
//! of every other operation of that service that declares the member's form —
//! `smithy_query(query="operation", operation="DescribeContactFlowAlias", model_path="services/service-com-amazonaws-connect-amazonconnectservice/smithy-model/com-amazonaws-connect-amazonconnectservice.json")`).
//! THIS service's model states `InstanceId` only as `@length 1..256` with `@pattern "\S"`,
//! which is why the form has to be read off the service that mints the instance.
//!
//! Resolution is about the identifier's FORM and not about whether the account holds that
//! instance: a well-formed identifier the account never created clears this stage and is
//! answered by the stage behind it, which is what keeps this refusal from reporting whether
//! an instance exists.

use ::aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::regex::Regex;

/// The wire error code of the refusal a request whose `InstanceId` resolves to no instance
/// is answered with. It is not a shape of this service's model — the front door refuses the
/// call before the modelled service is reached — so it is named here rather than built from
/// a modelled error.
pub(super) const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The whole message that refusal carries: fixed text, interpolating nothing from the
/// request and nothing from stored state, so no identifier a caller submitted is echoed
/// back by it.
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The status that refusal is answered on.
///
/// The recorded evidence pins the CODE and the MESSAGE of this refusal and not its status,
/// so the status is read off the code's own class: `UnauthorizedException` carrying the
/// fixed `Unauthorized` is the refusal an AWS REST front door's authorizer answers, which is
/// HTTP 401 — the caller was refused, rather than the request's body (400), its permissions
/// (403) or the resource it named (404). It is deliberately none of the five statuses this
/// model's own errors declare, because this refusal is not one of them.
pub(super) const UNAUTHORIZED_STATUS: u16 = 401;

/// The media type this restJson1 service renders an error in
/// (`smithy_query(query="service_info")` names `restJson1` as its only protocol).
const JSON_CONTENT_TYPE: &str = "application/json";

/// The member of this operation's input that names the instance to resolve
/// (`smithy_query(query="operation", operation="ListRealtimeContactAnalysisSegments")`:
/// `InstanceId`, `@required`), spelled as the restJson1 body carries it.
const INSTANCE_ID_MEMBER: &str = "InstanceId";

/// The `InstanceId` the request names, or `None` for a request that names none — a body
/// that is not a JSON object, or one carrying no string under that member.
///
/// Read from the REQUEST's own body (through [`super::front_door::body_members`], this
/// layer's one reader of it), because this stage runs ahead of the walk that builds the
/// operation's input: a request refused here was never deserialized. A request that names
/// none has nothing to resolve, so it is not this stage's to refuse: the member it left out
/// (or gave a value of the wrong type) is a fact about the BODY, which the stage behind this
/// one states.
pub(super) fn named_instance(
    members: Option<&::serde_json::Map<String, ::serde_json::Value>>,
) -> Option<&str> {
    super::front_door::named(members, INSTANCE_ID_MEMBER)
}

/// Whether `id` addresses an Amazon Connect instance: the GUID an instance is identified
/// by, on its own or as the resource of that instance's ARN (see the module docs).
#[must_use]
pub(super) fn resolves_to_instance(id: &str) -> bool {
    CONNECT_INSTANCE_ID.is_match(id)
}

/// Amazon Connect's own `InstanceId` expression, verbatim as that service's model declares
/// it (see the module docs): a GUID, optionally prefixed by the instance's ARN, anchored at
/// both ends so the WHOLE identifier has to satisfy it.
const CONNECT_INSTANCE_ID_PATTERN: &str = concat!(
    r"^(arn:(aws|aws-us-gov):connect:[a-z]{2}-[a-z]+-[0-9]{1}:[0-9]{1,20}:instance/)?",
    r"[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$"
);

/// That expression, compiled once for the whole process.
static CONNECT_INSTANCE_ID: ::std::sync::LazyLock<Regex> = ::std::sync::LazyLock::new(|| {
    Regex::new(CONNECT_INSTANCE_ID_PATTERN).expect("the model's own expression compiles")
});

/// The wire answer to a request whose `InstanceId` resolved to no instance: the front
/// door's own refusal, in this service's JSON envelope, carrying the code a client branches
/// on in `x-amzn-errortype` and in the body's `__type` and the fixed message beside it.
///
/// Built here rather than through the framework's refusal renderer because that renderer
/// takes the status from the protocol (400 for a constraint refusal), and this refusal is
/// answered on [`UNAUTHORIZED_STATUS`].
#[must_use]
pub(super) fn unauthorized_response() -> RtResponse {
    let body = ::serde_json::json!({
        "__type": UNAUTHORIZED_CODE,
        "message": UNAUTHORIZED_MESSAGE,
    });
    let mut response = RtResponse::new(
        StatusCode::try_from(UNAUTHORIZED_STATUS).expect("the status is a constant of this module"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
        .headers_mut()
        .insert("x-amzn-errortype", UNAUTHORIZED_CODE);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an identifier resolves to an instance only when it is the GUID an Amazon
    /// Connect instance is identified by, bare or as the resource of that instance's ARN.
    ///
    /// The rows span what the rule generalises over: the form of the identifier (a GUID, a
    /// GUID's ARN, neither), its case, its length, and the ARN's own shape.
    #[test]
    fn only_a_connect_instance_identifier_resolves() {
        let guid = "4cd1aa0d-1f2e-4b3c-8d4e-5f60718293a4";
        // (case, identifier, resolves)
        let rows: [(&str, String, bool); 15] = [
            ("a bare guid", guid.to_string(), true),
            (
                "the instance's own arn",
                format!("arn:aws:connect:us-east-1:000000000000:instance/{guid}"),
                true,
            ),
            (
                "a govcloud instance arn",
                format!("arn:aws-us-gov:connect:us-gov-1:000000000000:instance/{guid}"),
                true,
            ),
            ("whitespace only", "\t\t".to_string(), false),
            ("spaces only", "   ".to_string(), false),
            ("empty", String::new(), false),
            ("a non-guid name", "inst-1".to_string(), false),
            (
                "a 256-character non-guid",
                format!("{guid}{}", "a".repeat(256))[..256].to_string(),
                false,
            ),
            ("an uppercase guid", guid.to_uppercase(), false),
            (
                "a guid with a group too long",
                format!("{guid}0"),
                false,
            ),
            (
                "a guid missing a group",
                guid.rsplit_once('-')
                    .map(|(head, _)| head.to_string())
                    .expect("the guid has five groups"),
                false,
            ),
            (
                "another service's instance arn",
                format!("arn:aws:connectcases:us-east-1:000000000000:instance/{guid}"),
                false,
            ),
            (
                "an arn naming another resource type",
                format!("arn:aws:connect:us-east-1:000000000000:queue/{guid}"),
                false,
            ),
            (
                "an arn with a trailing field",
                format!("arn:aws:connect:us-east-1:000000000000:instance/{guid}:extra"),
                false,
            ),
            (
                "an arn whose account is not digits",
                format!("arn:aws:connect:us-east-1:not-an-account:instance/{guid}"),
                false,
            ),
        ];

        for (case, id, expected) in rows {
            let resolved = resolves_to_instance(&id);
            assert_eq!(
                resolved, expected,
                "case {case:?}: expected resolves_to_instance({id:?}) to be {expected}, got \
                 {resolved}"
            );
        }
    }

    /// RULE: the refusal carries the fixed message, the code a client branches on, and the
    /// front door's status — and echoes nothing a caller submitted.
    #[test]
    fn the_refusal_is_fixed_text_on_the_front_doors_status() {
        let response = unauthorized_response();
        assert_eq!(
            response.status().as_u16(),
            UNAUTHORIZED_STATUS,
            "case \"the status\": expected {UNAUTHORIZED_STATUS}, got {}",
            response.status().as_u16()
        );
        assert_eq!(
            response
                .headers()
                .get("x-amzn-errortype")
                .unwrap_or_default(),
            UNAUTHORIZED_CODE,
            "case \"the discriminator\": expected {UNAUTHORIZED_CODE:?}, got {:?}",
            response.headers().get("x-amzn-errortype")
        );
        let body = response.body().bytes().unwrap_or_default().to_vec();
        let parsed: ::serde_json::Value =
            ::serde_json::from_slice(&body).expect("the refusal body is JSON");
        assert_eq!(
            parsed.get("message").and_then(::serde_json::Value::as_str),
            Some(UNAUTHORIZED_MESSAGE),
            "case \"the message\": expected {UNAUTHORIZED_MESSAGE:?}, got {parsed:?}"
        );
    }
}
