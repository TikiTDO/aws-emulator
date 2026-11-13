// SPDX-License-Identifier: CC0-1.0

//! The UNIVERSAL target: a schedule whose `Target$Arn` names an API OPERATION rather
//! than a resource.
//!
//! "EventBridge Scheduler supports templated target that invoke common API operations,
//! as well as universal targets that you can customize to invoke over 6,000 API
//! operations across more than 270 services" (`Target`). A templated target names the
//! RESOURCE its invocation reaches — a queue, a topic, a function, a state machine —
//! and the one API that kind of resource is invoked by is a fact of its kind
//! ([`super::invocation`]). A universal target names the CALL itself, under this
//! service's own ARN namespace and with neither Region nor account:
//!
//! ```text
//! arn:<partition>:scheduler:::aws-sdk:<service>:<operation>
//! ```
//!
//! The service field is the name the AWS SDKs give the service — its SDK service id,
//! which is NOT always the endpoint prefix its API answers on (`sfn` for AWS Step
//! Functions, whose endpoint and signing name are `states`) — and the operation field is
//! that SDK's own name for the operation, in lowerCamelCase (`startExecution`). At each
//! occurrence the invocation IS that call: made under `Target$RoleArn`, with
//! `Target$Input` — "the text, or well-formed JSON, passed to the target" — as the
//! operation's own request.
//!
//! ## What this module owns, and what it does not
//!
//! WHAT a request to one of those services looks like on the wire — which service an SDK
//! service id names, its endpoint, its signing name, the protocol its model declares and
//! the spelling that model gives an operation's members — is no fact of EventBridge
//! Scheduler's. It is read from the one place this tree states it, the AWS SDK
//! integration family of AWS Step Functions
//! ([`::service_com_amazonaws_sfn_awsstepfunctions::state::integration`]), whose
//! `Resource` names a service and an operation exactly the same way; that module
//! resolves the call, builds its request and signs it, so the facts of those services
//! are stated once for both callers rather than copied here.
//!
//! What this module owns is EventBridge Scheduler's half: the universal ARN's own
//! grammar, the access request the call is subject to, and what one ANSWER means to a
//! schedule's INVOCATION — which is this service's vocabulary ([`super::dead_letter`]),
//! an error code and message to report and a status to decide a retry by, and not a
//! task's.
//!
//! ## Where the call goes
//!
//! Nowhere outside this process, and nowhere a caller chose. The ARN's service field
//! selects one of the services this emulator hosts itself, and the request is handed to
//! the in-process callback — the emulator's own router rather than a socket — in the
//! Region the SCHEDULE's own ARN names. `Target$Input` is the request's BODY and
//! nothing else: no part of it names an address, nothing is resolved and no host is
//! dialled.

use ::serde_json::Value;
use ::service_com_amazonaws_sfn_awsstepfunctions::state::integration;

use super::dead_letter::{failure_of_answer, unanswered, Attempt};

/// The ARN namespace a universal target is written under.
///
/// The ARN names EventBridge Scheduler's own target rather than a resource of the
/// service that is called, so it carries this service's namespace
/// (`awsService.arnNamespace`).
const SCHEDULER_ARN_NAMESPACE: &str = "scheduler";

/// The first field of a universal target's resource: the family the ARN names.
const AWS_SDK_SEGMENT: &str = "aws-sdk";

/// The lowest status that is not a success, which is where an answer carries a refusal
/// rather than the operation's own result.
const FIRST_ERROR_STATUS: u16 = 400;

/// Refuse a `Target$Arn` written as a universal target that names no call.
///
/// A universal target's two fields are not free text: the service is the called
/// service's SDK SERVICE ID — the name the AWS SDKs give it, lower-cased — and the
/// operation is that SDK's own operation name in lowerCamelCase, which is what the
/// service validates when a schedule is WRITTEN rather than leaving a schedule that can
/// invoke nothing in the store. So a value naming a service by a name no SDK gives it
/// (its endpoint prefix or its ARN namespace, where those differ from its SDK service
/// id — `states` for the service whose id is `sfn`), or naming the operation in any
/// other case than lowerCamelCase, is refused.
///
/// `None` for every `Target$Arn` that is not written as a universal target — a
/// templated target names a RESOURCE, whose own form the modelled constraints and the
/// target's kind decide — and for a universal target whose service this emulator cannot
/// place at all: the SDK service ids are AWS's "more than 270 services" rather than the
/// services mounted here, so a service id this emulator makes no call for is still a
/// service id, and refusing the write for it would refuse a schedule the service
/// accepts. Such a schedule is written, and its occurrence reports that nothing was
/// attempted for it ([`universal_call`]).
///
/// The refusal names the PAIR the value states — the api, and the service under the
/// family segment it is written beneath — and takes both from the submitted value,
/// which is the only place they are reported back. A value that states no such pair at
/// all (a resource field carrying no api, or an empty field) states nothing for the
/// refusal to name, so it is left to the occurrence to report as unattempted rather
/// than refused in words about a pair the request did not write.
pub fn universal_target_violation(target_arn: &str) -> Option<String> {
    let parsed = ::service_common::arn::parse(target_arn)?;
    if !names_universal_target(&parsed) {
        return None;
    }
    let (service, api) = parsed
        .resource
        .split_once(':')
        .and_then(|(_, tail)| tail.split_once(':'))
        .filter(|(service, api)| !service.is_empty() && !api.is_empty())?;
    let names_a_call = is_sdk_service_id(service)
        && !names_a_service_by_another_name(service)
        && is_lower_camel_case(api);
    (!names_a_call).then(|| invalid_universal_target(service, api))
}

/// How this service states a universal target whose api and service do not name one
/// of the operations it invokes.
fn invalid_universal_target(service: &str, api: &str) -> String {
    format!(
        "Invalid RequestJson provided. Reason The api {api} is not valid for the service \
         {AWS_SDK_SEGMENT}:{service}."
    )
}

/// Whether `service` is written the way an SDK service id is when the AWS SDK family
/// names a service by it: that id lower-cased with its spaces dropped
/// ([`integration::Api::sdk_token`]), so lower-case letters and digits and nothing else.
fn is_sdk_service_id(service: &str) -> bool {
    !service.is_empty()
        && service
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Whether `service` names a service this emulator holds the SDK service id of by
/// ANOTHER of that service's names — its endpoint prefix or its ARN namespace — which
/// is a name no AWS SDK gives it, so it is no universal target's service.
///
/// Both names come from the same statement of those services' wire facts the call
/// itself is resolved from ([`integration::APIS`]), so the two cannot disagree about
/// which name is which.
fn names_a_service_by_another_name(service: &str) -> bool {
    integration::APIS.iter().any(|api| {
        (api.endpoint_prefix == service || api.namespace == service)
            && api.sdk_token() != service
    })
}

/// Whether `operation` is written in the lowerCamelCase an SDK names an operation in:
/// a lower-case letter and then letters and digits, so the capitalised form of the same
/// name is not one.
fn is_lower_camel_case(operation: &str) -> bool {
    let mut characters = operation.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| character.is_ascii_alphanumeric())
}

/// One universal target's call: the operation the ARN named, and the request the
/// invocation hands it.
#[derive(Debug)]
pub(crate) struct UniversalCall {
    /// The call the operation names, resolved from the one statement of those services'
    /// wire facts this tree has.
    call: integration::Call,
    /// `Target$Input`, with the schedule's context attributes already substituted, as
    /// the invoked operation's input document.
    request: Value,
}

/// Whether `parsed` names a UNIVERSAL target: this service's own ARN namespace and the
/// AWS SDK family's segment.
///
/// Read from those two fields alone, so a value of that form whose remaining fields name
/// no operation is still a universal target — one no call is made for
/// ([`universal_call`]) rather than a resource of some other kind.
pub(crate) fn names_universal_target(parsed: &::service_common::arn::Arn<'_>) -> bool {
    parsed.vendor == SCHEDULER_ARN_NAMESPACE
        && parsed.resource.split(':').next() == Some(AWS_SDK_SEGMENT)
}

/// The call the universal target `parsed` names with `input` as its request, or why no
/// call is made for it.
///
/// The `Err` is what a target nothing was attempted for reports: it says what the ARN
/// named and what about it is not called, and never claims a call happened.
pub(crate) fn universal_call(
    parsed: &::service_common::arn::Arn<'_>,
    input: &str,
) -> Result<UniversalCall, String> {
    let named = parsed
        .resource
        .split_once(':')
        .and_then(|(_, tail)| tail.split_once(':'));
    let Some((service, operation)) = named.filter(|(service, operation)| {
        // Both fields are the ARN's own: a universal target names ONE operation of ONE
        // service, so the service may not be empty and the operation is the SDK's own
        // name for it ([`is_lower_camel_case`]) — a value carrying anything else (a
        // further field, an integration pattern of some other service) names no
        // operation this service calls, which is also what the write path refuses
        // ([`universal_target_violation`]).
        !service.is_empty() && is_lower_camel_case(operation)
    }) else {
        return Err(format!(
            "the universal target ARN {} names no operation of a service",
            parsed.resource
        ));
    };
    let call = integration::aws_sdk_call(service, operation).map_err(|why| {
        format!("this emulator does not call '{operation}' of '{service}': {why}")
    })?;
    // A universal target's `Input` IS the operation's request, so it has to be the JSON
    // object that operation takes: "the input must be a well-formed JSON"
    // (`Target$Input`). Text that is not one names no request, and nothing is sent for
    // it rather than a document guessed from the text.
    let request: Value = ::serde_json::from_str(input).map_err(|error| {
        format!(
            "the Input of the universal target {} is not the well-formed JSON the \
             '{operation}' request of '{service}' is made of: {error}",
            parsed.resource
        )
    })?;
    if !request.is_object() {
        return Err(format!(
            "the Input of the universal target {} is not a JSON object, so it is not the \
             '{operation}' request of '{service}'",
            parsed.resource
        ));
    }
    Ok(UniversalCall { call, request })
}

impl UniversalCall {
    /// The access request this call is subject to: the IAM action the invoked operation
    /// publishes, over the resource the call acts ON.
    ///
    /// A universal target's ARN names no resource — it names the call — so the resource
    /// is the one the REQUEST names, read where that is stated for each operation
    /// ([`integration::access_request`]). `None` for a call whose request names no
    /// resource of its own: there is nothing to decide an access request over, and what
    /// to answer for such a request is the invoked operation's own refusal.
    pub(crate) fn access_request(&self, region: &str) -> Option<(String, String)> {
        integration::access_request(&self.call, &self.request, region)
    }

    /// Make the call, and answer what it came to.
    ///
    /// The answer is read as EventBridge Scheduler reads a delivery: a status the
    /// operation accepted the request under is a delivery, and anything else is the
    /// failure that answer carries — which is what the schedule's retry policy and its
    /// dead-letter route then act on ([`super::dead_letter`]). An answer from the
    /// in-process transport ITSELF, rather than from the service addressed, is no answer
    /// about the request at all: nothing served the call, so nothing is reported as
    /// having failed it.
    pub(crate) async fn invoke(
        &self,
        callback: &::smithy_server_runtime::EmbeddedEmulator,
        region: &str,
    ) -> Attempt {
        let answered =
            match integration::call_answered(callback, region, &self.call, &self.request).await {
                Ok(answered) => answered,
                // The call was made and nothing came back: that establishes nothing
                // about the request, so it is reported as a failure another attempt
                // could still come to something else for.
                Err(why) => return Attempt::Failed(unanswered(&why)),
            };
        if ::service_common::in_process_refusal::is_transport_refusal(answered.payload()) {
            return Attempt::NotAttempted(format!(
                "no service of this emulator served the '{}' call the universal target names",
                self.call.operation
            ));
        }
        if answered.status() < FIRST_ERROR_STATUS {
            return Attempt::Delivered;
        }
        Attempt::Failed(failure_of_answer(
            answered.status(),
            answered.error_type(),
            answered.payload(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ARN fields of `arn`, as the shared parse reads them.
    fn parsed(arn: &str) -> ::service_common::arn::Arn<'_> {
        ::service_common::arn::parse(arn).unwrap_or_else(|| panic!("{arn} is a well-formed ARN"))
    }

    /// A universal target ARN is told from every other `Target$Arn` by its own two
    /// fields: this service's namespace and the AWS SDK family's segment. The rows
    /// differ along the vendor, along the family segment and along the resource's shape.
    #[test]
    fn a_universal_target_is_told_by_its_namespace_and_its_family() {
        // (case, the target ARN, whether it names a universal target)
        let rows: [(&str, &str, bool); 7] = [
            (
                "a universal target of Step Functions",
                "arn:aws:scheduler:::aws-sdk:sfn:startExecution",
                true,
            ),
            (
                "a universal target of another service",
                "arn:aws:scheduler:::aws-sdk:dynamodb:putItem",
                true,
            ),
            (
                "a universal target naming no operation",
                "arn:aws:scheduler:::aws-sdk:sfn",
                true,
            ),
            (
                "a queue, which is a templated target",
                "arn:aws:sqs:eu-west-2:000000000000:orders",
                false,
            ),
            (
                "a state machine, which is a templated target",
                "arn:aws:states:eu-west-2:000000000000:stateMachine:nightly",
                false,
            ),
            (
                "this service's own schedule, which names no call",
                "arn:aws:scheduler:eu-west-2:000000000000:schedule/default/nightly",
                false,
            ),
            (
                "another family under this namespace",
                "arn:aws:scheduler:::http:invoke",
                false,
            ),
        ];
        for (case, arn, expected) in rows {
            let names = names_universal_target(&parsed(arn));
            assert_eq!(
                names, expected,
                "case {case:?}: expected names_universal_target={expected}, got {names}"
            );
        }
    }

    /// The service field is read as the SDK SERVICE ID and the operation field as the
    /// SDK's own operation name: the rows differ along the service, along the operation
    /// and along the spelling of the invoked model's own members, and the endpoint
    /// prefix of a service whose id differs from it names no call at all.
    #[test]
    fn a_universal_target_names_a_service_by_its_sdk_id() {
        // (case, the target ARN, the request, the operation the call names)
        let rows: [(&str, &str, &str, Option<&str>); 8] = [
            (
                "Step Functions by its SDK service id",
                "arn:aws:scheduler:::aws-sdk:sfn:startExecution",
                r#"{"StateMachineArn":"arn:aws:states:eu-west-2:000000000000:stateMachine:n"}"#,
                Some("StartExecution"),
            ),
            (
                // The distinction the SDK service id draws: `states` is the endpoint
                // prefix and the signing name of that same service, and no SDK names it
                // so.
                "Step Functions by its endpoint prefix",
                "arn:aws:scheduler:::aws-sdk:states:startExecution",
                r#"{"StateMachineArn":"arn:aws:states:eu-west-2:000000000000:stateMachine:n"}"#,
                None,
            ),
            (
                "a queue's own API",
                "arn:aws:scheduler:::aws-sdk:sqs:sendMessage",
                r#"{"QueueUrl":"https://sqs.eu-west-2.amazonaws.com/000000000000/q","MessageBody":"hi"}"#,
                Some("SendMessage"),
            ),
            (
                "an item written to a table",
                "arn:aws:scheduler:::aws-sdk:dynamodb:putItem",
                r#"{"TableName":"orders","Item":{}}"#,
                Some("PutItem"),
            ),
            (
                // A service whose protocol binds each operation's members to a method,
                // a path and a payload rather than carrying them as the body: the call
                // is the operation's, whatever its protocol.
                "a function's own API",
                "arn:aws:scheduler:::aws-sdk:lambda:invoke",
                r#"{"FunctionName":"nightly","Payload":{}}"#,
                Some("Invoke"),
            ),
            (
                // A service whose request is a form rather than a document.
                "a topic's own API",
                "arn:aws:scheduler:::aws-sdk:sns:publish",
                r#"{"TopicArn":"arn:aws:sns:eu-west-2:000000000000:orders","Message":"hi"}"#,
                Some("Publish"),
            ),
            (
                "a service nothing here serves",
                "arn:aws:scheduler:::aws-sdk:notaservice:doSomething",
                "{}",
                None,
            ),
            (
                "no operation at all",
                "arn:aws:scheduler:::aws-sdk:sfn",
                "{}",
                None,
            ),
        ];
        for (case, arn, input, expected) in rows {
            let call = universal_call(&parsed(arn), input);
            let named = call.as_ref().ok().map(|call| call.call.operation.clone());
            assert_eq!(
                named.as_deref(),
                expected,
                "case {case:?}: expected the call {expected:?}, got {:?}",
                call.map(|call| call.call.operation.clone())
            );
        }
    }

    /// An `Input` that is not the JSON object the operation's request is made of names
    /// no request, and no call is made for it: the rows differ along what the text is.
    #[test]
    fn an_input_that_is_no_request_names_no_call() {
        let arn = "arn:aws:scheduler:::aws-sdk:sfn:startExecution";
        // (case, the Input)
        let rows: [(&str, &str); 5] = [
            ("text that is not JSON at all", "start the machine"),
            ("a JSON string", "\"start\""),
            ("a JSON array", "[{\"StateMachineArn\":\"x\"}]"),
            ("a JSON number", "7"),
            ("a truncated object", "{\"StateMachineArn\":"),
        ];
        for (case, input) in rows {
            let call = universal_call(&parsed(arn), input);
            assert!(
                call.is_err(),
                "case {case:?}: expected no call for the Input {input:?}, got {:?}",
                call.map(|call| call.call.operation.clone())
            );
        }
    }

    /// The access request a universal call is subject to is the invoked operation's own
    /// action over the resource its REQUEST names, and a request naming none is subject
    /// to no access request here.
    #[test]
    fn the_access_request_is_the_operations_action_over_the_resource_the_request_names() {
        /// One row: the case, the target ARN, the request, and the access request the
        /// call it names is subject to.
        type AccessRow = (
            &'static str,
            &'static str,
            String,
            Option<(&'static str, String)>,
        );

        let region = "eu-west-2";
        let machine = "arn:aws:states:eu-west-2:000000000000:stateMachine:nightly";
        let rows: [AccessRow; 4] = [
            (
                "a state machine named by its ARN",
                "arn:aws:scheduler:::aws-sdk:sfn:startExecution",
                format!("{{\"StateMachineArn\":\"{machine}\"}}"),
                Some(("states:StartExecution", machine.to_string())),
            ),
            (
                "the same request under the invoked model's own spelling",
                "arn:aws:scheduler:::aws-sdk:sfn:startExecution",
                format!("{{\"stateMachineArn\":\"{machine}\"}}"),
                Some(("states:StartExecution", machine.to_string())),
            ),
            (
                "a request naming no resource at all",
                "arn:aws:scheduler:::aws-sdk:sfn:startExecution",
                "{\"name\":\"run\"}".to_string(),
                None,
            ),
            (
                "a table named by its name, whose ARN the access request is over",
                "arn:aws:scheduler:::aws-sdk:dynamodb:putItem",
                "{\"TableName\":\"orders\",\"Item\":{}}".to_string(),
                Some((
                    "dynamodb:PutItem",
                    ::engine::emulator_arn("dynamodb", region, "table/orders"),
                )),
            ),
        ];
        for (case, arn, input, expected) in rows {
            let call = universal_call(&parsed(arn), &input)
                .unwrap_or_else(|why| panic!("case {case:?}: the ARN names a call: {why}"));
            let access = call.access_request(region);
            let expected = expected.map(|(action, resource)| (action.to_string(), resource));
            assert_eq!(
                access, expected,
                "case {case:?}: expected the access request {expected:?}, got {access:?}"
            );
        }
    }

    /// A `Target.Arn` written as a universal target is held to the universal target's
    /// own two fields at WRITE time: the service is the called service's SDK service id
    /// and the operation is the SDK's own name in lowerCamelCase. The rows differ along
    /// which field is at fault and along how: a service named by its endpoint prefix or
    /// its ARN namespace instead of its SDK service id, an operation in another case,
    /// and a resource that states no api/service pair at all — beside the forms the
    /// service accepts, including an SDK service id this emulator makes no call for.
    #[test]
    fn a_universal_target_is_held_to_its_own_two_fields_when_written() {
        // (case, the Target.Arn, whether the write is refused)
        let rows: [(&str, &str, bool); 14] = [
            (
                "the SDK service id and a lowerCamelCase operation",
                "arn:aws:scheduler:::aws-sdk:sfn:startExecution",
                false,
            ),
            (
                "an SDK service id of a service this emulator makes no call for",
                "arn:aws:scheduler:::aws-sdk:cloudfront:createInvalidation",
                false,
            ),
            (
                "an SDK service id carrying a digit",
                "arn:aws:scheduler:::aws-sdk:elasticloadbalancingv2:describeTargetHealth",
                false,
            ),
            (
                "an operation whose name carries a digit",
                "arn:aws:scheduler:::aws-sdk:s3:listObjectsV2",
                false,
            ),
            (
                // `states` is the endpoint prefix and the signing name of the service
                // whose SDK service id is `sfn`, and no AWS SDK names it so.
                "the endpoint prefix of a service instead of its SDK service id",
                "arn:aws:scheduler:::aws-sdk:states:startExecution",
                true,
            ),
            (
                // The same for the service whose SDK service id is `eventbridge` and
                // whose endpoint prefix and ARN namespace are `events`.
                "the ARN namespace of a service instead of its SDK service id",
                "arn:aws:scheduler:::aws-sdk:events:putEvents",
                true,
            ),
            (
                "the capitalised form of the operation",
                "arn:aws:scheduler:::aws-sdk:sqs:SendMessage",
                true,
            ),
            (
                "an operation in another case again",
                "arn:aws:scheduler:::aws-sdk:sqs:send_message",
                true,
            ),
            (
                "an upper-case service",
                "arn:aws:scheduler:::aws-sdk:SQS:sendMessage",
                true,
            ),
            (
                // A value stating no api/service pair states nothing the refusal could
                // name, so the occurrence reports it unattempted instead.
                "no operation field at all",
                "arn:aws:scheduler:::aws-sdk:sqs",
                false,
            ),
            (
                "an empty operation field",
                "arn:aws:scheduler:::aws-sdk:sqs:",
                false,
            ),
            (
                "an empty service field",
                "arn:aws:scheduler:::aws-sdk::sendMessage",
                false,
            ),
            (
                // Every other `Target.Arn` names a RESOURCE, and nothing here reads it.
                "a queue, which is a templated target",
                "arn:aws:sqs:eu-west-2:000000000000:orders",
                false,
            ),
            (
                "a state machine, which is a templated target",
                "arn:aws:states:eu-west-2:000000000000:stateMachine:nightly",
                false,
            ),
        ];
        for (case, arn, refused) in rows {
            let violation = universal_target_violation(arn);
            assert_eq!(
                violation.is_some(),
                refused,
                "case {case:?}: expected refused={refused}, got {violation:?}"
            );
            // The refusal names the pair the value states, both halves taken from it.
            if let Some(message) = violation {
                let (service, api) = arn
                    .rsplit_once(':')
                    .map(|(head, api)| (head.rsplit(':').next().unwrap_or_default(), api))
                    .unwrap_or_default();
                assert_eq!(
                    message,
                    format!(
                        "Invalid RequestJson provided. Reason The api {api} is not valid for \
                         the service aws-sdk:{service}."
                    ),
                    "case {case:?}: expected the refusal to name the pair the value states, \
                     got {message:?}"
                );
            }
        }
    }
}
