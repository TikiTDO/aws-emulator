// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the principal that SIGNED a request perform the action it asked for —
//! the authorization step taken in FRONT of every operation, before anything is read or
//! written.
//!
//! Every operation of this API is signed with IAM credentials (`sigv4` is the only auth
//! scheme the model declares, `smithy_query(query="digest")`), and Step Functions decides
//! such a call at the IAM layer before it is dispatched: a caller whose identity-based
//! policies do not allow the action on the resource the request names is told it is not
//! authorized rather than told anything about what the account holds, and nothing of the
//! account changes on the way to that refusal. The decision is taken per request, so a
//! statement `iam:PutRolePolicy` wrote refuses the next call and one `iam:DeleteRolePolicy`
//! removed stops granting it — which is what makes a client's least-privilege test, and a
//! revocation drill, mean anything here.
//!
//! No error shape of this model carries that refusal — no operation of this service lists
//! an access error (`smithy_query(query="digest")`) — because it belongs to the
//! authorization layer ahead of the service's own code rather than to any one operation:
//! it is the AWS-common `AccessDeniedException`, rendered here in this service's own
//! `awsJson1_0` error envelope ([`super::wire`]) with HTTP 400, the status this endpoint
//! answers every refusal of a caller under.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who it is, what it may do, and the decision** — [`super::authorization`], the one
//!    place this service resolves the principal an access key id speaks for, reads the
//!    identity-based documents and the permissions boundary in force on it, and asks
//!    [`::service_common::iam_authorization`] about an `(action, resource)` pair
//!    ([`super::authorization::caller_action_denial`]); the callers it does not bound — the
//!    account's own published credential, which every in-process call another emulated
//!    service makes to this one presents, a principal carrying a grant this emulator cannot
//!    read, and an engine with no STS or IAM store — are named there. Nothing is cached, so
//!    a policy `iam:PutRolePolicy` wrote refuses the next call and one it replaced stops
//!    granting on the next call;
//! 2. **what the request asks for** — the action and the resource, read off the request
//!    alone here ([`super::authorization::operation_action`], [`addressed_resource`])
//!    before anything is looked up, so the decision cannot depend on whether that resource
//!    exists and a refused caller learns nothing about what the account holds;
//! 3. **where the decision is taken** — this module's [`IamAuthorized`], in front of the
//!    routing table. Wrapping is what makes it ONE decision: every modelled operation is
//!    judged by the same rule before its handler, so no handler can forget it, none of them
//!    takes it twice, and no handler of this service carries an authorization step of its
//!    own.
//!
//! ## The action and the resource
//!
//! The action is `states:<OperationName>` — this service's own ARN namespace and the
//! operation's shape name, spelled by [`super::authorization::operation_action`] — so one
//! statement scoped to `states:*` covers every operation of it and nothing outside it.
//!
//! The resource is what the request ADDRESSES: the ARN it carries in the member THAT
//! OPERATION names its resource by ([`ADDRESSED_BY`], keyed by operation so a member
//! another operation names its resource by cannot decide this one), the ARN a create would
//! MINT for the name it carries ([`minted_resource`]), and otherwise [`ANY_RESOURCE`] — a
//! call that names no resource of this service in a member of its own, which a statement
//! has to grant on every resource to cover. A request that names one only INSIDE another
//! structure (`CreateStateMachineAlias`, whose routing configuration carries the version
//! ARNs the alias is to point at) is in that last group: the ARN it would be judged against
//! is not a member of the request, so such a call is decided against every resource rather
//! than against an ARN read out of a nested shape.

use ::std::sync::Arc;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::aws_smithy_schema_server::protocol::ServerProtocol;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use super::authorization::{caller_action_denial, operation_action};

/// The wire error code of an identity-based authorization refusal, as AWS's "Common
/// Errors" table names it — what a client branches on for "not permitted".
pub const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The resource a request that names no resource of this service in a member of its own is
/// judged against, as IAM writes it for such a request: a listing, a call addressed by a
/// task token, a definition validated without a state machine, and a create whose
/// resource is named only inside a nested shape (see the module docs).
pub const ANY_RESOURCE: &str = "*";

/// The member of each operation's INPUT that names the ARN of the resource that request
/// addresses, keyed by the operation's shape name: the one member (or, where the model
/// declares two, the members in the order they decide) read off the body, so a value a
/// request carries under a member its own operation does not model decides nothing. An
/// empty row is an operation whose request names no resource ARN at all.
///
/// Source: every operation of this model and its own input shape
/// (`smithy_query(query="operation",operation="<Op>",model_path="services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json")`
/// for each of the 37 operations `query="digest"` lists). Public, so that catalog is
/// re-derived from the model by this crate's own tests
/// (`tests/all/identity_policy_authorization.rs`) rather than only transcribed here.
pub const ADDRESSED_BY: &[(&str, &[&str])] = &[
    ("CreateActivity", &[]),
    ("CreateStateMachine", &[]),
    ("CreateStateMachineAlias", &[]),
    ("DeleteActivity", &["activityArn"]),
    ("DeleteStateMachine", &["stateMachineArn"]),
    ("DeleteStateMachineAlias", &["stateMachineAliasArn"]),
    ("DeleteStateMachineVersion", &["stateMachineVersionArn"]),
    ("DescribeActivity", &["activityArn"]),
    ("DescribeExecution", &["executionArn"]),
    ("DescribeMapRun", &["mapRunArn"]),
    ("DescribeStateMachine", &["stateMachineArn"]),
    ("DescribeStateMachineAlias", &["stateMachineAliasArn"]),
    ("DescribeStateMachineForExecution", &["executionArn"]),
    ("GetActivityTask", &["activityArn"]),
    ("GetExecutionHistory", &["executionArn"]),
    ("ListActivities", &[]),
    // "Lists all executions of a state machine or a Map Run": the request names one or
    // the other, and is judged against whichever it carries.
    ("ListExecutions", &["stateMachineArn", "mapRunArn"]),
    ("ListMapRuns", &["executionArn"]),
    ("ListStateMachineAliases", &["stateMachineArn"]),
    ("ListStateMachineVersions", &["stateMachineArn"]),
    ("ListStateMachines", &[]),
    ("ListTagsForResource", &["resourceArn"]),
    ("PublishStateMachineVersion", &["stateMachineArn"]),
    ("RedriveExecution", &["executionArn"]),
    ("SendTaskFailure", &[]),
    ("SendTaskHeartbeat", &[]),
    ("SendTaskSuccess", &[]),
    ("StartExecution", &["stateMachineArn"]),
    ("StartSyncExecution", &["stateMachineArn"]),
    ("StopExecution", &["executionArn"]),
    ("TagResource", &["resourceArn"]),
    ("TestState", &[]),
    ("UntagResource", &["resourceArn"]),
    ("UpdateMapRun", &["mapRunArn"]),
    ("UpdateStateMachine", &["stateMachineArn"]),
    ("UpdateStateMachineAlias", &["stateMachineAliasArn"]),
    ("ValidateStateMachineDefinition", &[]),
];

/// The members `operation` names the ARN of the resource it addresses by, in the order
/// they decide — empty for an operation that names none, and for one this catalog does not
/// carry, which is judged against every resource rather than against a member read off a
/// request it does not belong to.
#[must_use]
pub fn addressed_by(operation: &str) -> &'static [&'static str] {
    ADDRESSED_BY
        .iter()
        .find(|(named, _)| *named == operation)
        .map_or(&[], |(_, members)| *members)
}

/// The member a request that MINTS a resource names it by.
const NAME_MEMBER: &str = "name";

/// The ARN the create `operation` would MINT for the resource `name` in `region`, or
/// `None` for an operation that mints none.
///
/// Source: the two operations of this model whose input carries the `name` of the resource
/// it creates and no ARN of an existing one (`CreateStateMachineInput$name`,
/// `CreateActivityInput$name`). Such a request addresses the resource it is about to mint,
/// so it is judged against that ARN — built by the one place this service spells each form
/// ([`crate::state::state_machine_arn`], [`crate::state::activity_arn`]) and from the
/// request alone, so nothing is looked up and nothing is created to decide it.
#[must_use]
pub fn minted_resource(operation: &str, region: &str, name: &str) -> Option<String> {
    match operation {
        "CreateStateMachine" => Some(crate::state::state_machine_arn(region, name)),
        "CreateActivity" => Some(crate::state::activity_arn(region, name)),
        _ => None,
    }
}

/// The resource this request is judged against: what it ADDRESSES (see the module docs).
///
/// Read off the request's own body, which on this protocol is the operation's input as
/// JSON. A body that is not a JSON document at all addresses nothing of this service, so
/// it is judged exactly as a request that named no resource.
fn addressed_resource(ctx: &OperationContext<'_>, operation: &str, req: &RtRequest) -> String {
    let submitted: Option<::serde_json::Value> = req
        .body()
        .bytes()
        .and_then(|body| ::serde_json::from_slice(body).ok());
    let region = crate::state::request_region(ctx.parts());
    addressed_in(submitted.as_ref(), operation, &region)
}

/// The resource the submitted input `submitted` addresses, for `operation` in `region`
/// (see the module docs).
///
/// A member that is absent, is not a string or is empty names no resource, so such a
/// request is judged exactly as one that named none at all.
fn addressed_in(submitted: Option<&::serde_json::Value>, operation: &str, region: &str) -> String {
    let member = |name: &str| {
        submitted
            .and_then(|body| body.get(name))
            .and_then(::serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    for named in addressed_by(operation) {
        if let Some(arn) = member(named) {
            return arn;
        }
    }
    member(NAME_MEMBER)
        .and_then(|name| minted_resource(operation, region, &name))
        .unwrap_or_else(|| ANY_RESOURCE.to_string())
}

/// Why the policies in force do not let this caller perform `operation`, or `None` when
/// they do — including every caller the gate does not bound (see the module docs).
///
/// Call it BEFORE the operation runs: a refused request must read and write nothing. The
/// decision itself is [`super::authorization::caller_action_denial`], the one place this
/// service asks what the documents in force on a caller say about an `(action, resource)`
/// pair; this reads the pair off the request.
fn denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    req: &RtRequest,
) -> Option<String> {
    let action = operation_action(operation);
    let resource = addressed_resource(ctx, operation, req);
    caller_action_denial(engine, ctx, &action, &resource)
}

/// Every modelled operation of this service, served only to a caller IAM allows to perform
/// it.
///
/// It holds no operation of its own: the one it serves a permitted request through is the
/// one the service's routing table names for that request, reached through the SAME
/// wrapper every response of this service already passes
/// (`service_common::query_error_envelope::RequestIdHeaderOperation`), so the framework
/// still owns the whole deserialize -> validate -> handle -> serialize sequence and every
/// status, code and message it answers with. A refusal of its own is reported with the
/// request id too, because that is a property of every response this service sends.
pub struct IamAuthorized<Eng> {
    /// The shared engine, so the principal's policies are read from the one store every
    /// service of this emulator shares.
    engine: Arc<Eng>,
    /// Serves the request the decision admitted, and names the request each response
    /// answers.
    served: ::service_common::query_error_envelope::RequestIdHeaderOperation,
    /// This service's wire protocol, held rather than built per refusal: the envelope a
    /// refusal is rendered in is the one every other error of this endpoint is rendered in.
    protocol: super::wire::SwfJsonProtocol,
}

impl<Eng> IamAuthorized<Eng> {
    /// Put every modelled operation of this service behind the IAM decision, reading the
    /// policies in force from `engine`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            engine,
            served: ::service_common::query_error_envelope::RequestIdHeaderOperation,
            protocol: super::wire::SwfJsonProtocol::new(),
        }
    }

    /// The refusal as this endpoint puts it on the wire: the AWS-common
    /// `AccessDeniedException`, in this service's own `awsJson1_0` envelope and under the
    /// status it answers every client refusal with.
    fn denied_response(&self, message: &str) -> RtResponse {
        let refused = crate::scaffolded::AccessDeniedException::new(message);
        let rendered = self.protocol.serialize_error(
            &refused,
            &crate::scaffolded::ACCESS_DENIED_EXCEPTION_SCHEMA,
            ACCESS_DENIED_CODE,
            super::wire::CLIENT_ERROR_STATUS,
        );
        match rendered {
            Ok(response) => response,
            // The envelope is built from this crate's own schema and a message of its own,
            // so it cannot fail to render; a failure is still answered rather than leaving
            // the caller waiting, and it is answered as a REFUSAL, never as a success.
            Err(failed) => {
                ::tracing::warn!(
                    error = %failed,
                    "the authorization refusal could not be rendered in this service's envelope"
                );
                RtResponse::new(
                    ::aws_smithy_runtime_api::http::StatusCode::try_from(
                        super::wire::CLIENT_ERROR_STATUS,
                    )
                    .expect("the client-error status of this service is a status"),
                    ::aws_smithy_types::body::SdkBody::from(
                        format!("{{\"__type\":\"{ACCESS_DENIED_CODE}\"}}").as_bytes(),
                    ),
                )
            }
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for IamAuthorized<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // What the CLOCK and the KEY owe this service's records does not depend on
            // which call is being made, or on whether this caller may make it: an
            // execution of a state machine on a customer managed KMS key does its
            // remaining work under that key, so an outcome that comes due while the key
            // cannot be used for the work is a FAILURE rather than the outcome the walk
            // computed ([`crate::state::encryption::fail_due_runs_the_key_refuses`]).
            // It is materialised here, in front of every call this service serves, for
            // the same reason the management event behind this seam is recorded here:
            // it is a property of the service answering at this instant and of no one
            // operation, so no handler can forget it and none decides it twice.
            crate::state::encryption::fail_due_runs_the_key_refuses(
                &*self.engine,
                crate::state::now(),
            );
            // WHICH operation this is, read off the routing key this protocol resolves —
            // the same key the generated table is keyed by, so the action asked about
            // cannot drift from the operation served. A request the key cannot be read
            // from names no operation of this service and is left to the framework, which
            // answers it as an unroutable request.
            if let Ok(operation) =
                ::aws_smithy_schema_server::rpc::routing_key_from_x_amz_target(req)
            {
                if let Some(message) = denial(&*self.engine, ctx, operation, req) {
                    ::tracing::info!(
                        operation = %operation,
                        "iam does not allow this caller to perform this action"
                    );
                    return ::service_common::query_error_envelope::with_request_id_header(
                        self.denied_response(&message),
                    );
                }
            }
            self.served.run(svc, req, ctx).await
        })
    }

    /// `None` is "defer to the modelled operation the routing key names", which is the
    /// operation a permitted request is served through: wrapping must not change which
    /// schemes sign an operation.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use ::serde_json::json;

    use super::*;

    /// The Region the ARNs below are read in.
    const REGION: &str = "us-west-2";

    /// RULE: a request is judged against the ARN it NAMES, in the member ITS OWN
    /// operation names its resource by; one that names none of them is judged against the
    /// ARN its name mints, and otherwise against every resource of this service.
    ///
    /// The rows span every member of [`ADDRESSED_BY`] (including operations the issue this
    /// rule was written for did not cite), the precedence between the two members one
    /// operation may carry, both creates, the three ways a request names no resource at
    /// all, and a member another operation would be judged by carried on an operation that
    /// does not model it.
    #[test]
    fn a_request_is_judged_against_the_arn_it_names() {
        let machine = crate::state::state_machine_arn(REGION, "addressed");
        let execution = crate::state::execution_arn(REGION, "addressed", "one");
        let activity = crate::state::activity_arn(REGION, "worker");
        let map_run = crate::state::map_run_arn(REGION, "addressed", "label", "map-run-id");
        let alias = crate::state::state_machine_alias_arn(&machine, "PROD");
        let version = crate::state::state_machine_version_arn(&machine, 1);
        // (case, operation, the request body, the resource it addresses)
        let rows: Vec<(&str, &str, ::serde_json::Value, String)> = vec![
            (
                "a read addressed by execution ARN",
                "DescribeExecution",
                json!({ "executionArn": execution }),
                execution.clone(),
            ),
            (
                "a start addressed by state machine ARN",
                "StartExecution",
                json!({ "stateMachineArn": machine, "name": "run" }),
                machine.clone(),
            ),
            (
                "a list addressed by Map Run ARN alone",
                "ListExecutions",
                json!({ "mapRunArn": map_run }),
                map_run.clone(),
            ),
            (
                "a list naming both a state machine and a Map Run",
                "ListExecutions",
                json!({ "stateMachineArn": machine, "mapRunArn": map_run }),
                machine.clone(),
            ),
            (
                "an alias read addressed by alias ARN",
                "DescribeStateMachineAlias",
                json!({ "stateMachineAliasArn": alias }),
                alias.clone(),
            ),
            (
                "a version delete addressed by version ARN",
                "DeleteStateMachineVersion",
                json!({ "stateMachineVersionArn": version }),
                version.clone(),
            ),
            (
                "an activity poll addressed by activity ARN",
                "GetActivityTask",
                json!({ "activityArn": activity, "workerName": "worker-1" }),
                activity.clone(),
            ),
            (
                "a tag read addressed by resourceArn",
                "ListTagsForResource",
                json!({ "resourceArn": machine }),
                machine.clone(),
            ),
            (
                "a state machine create, judged against the ARN its name mints",
                "CreateStateMachine",
                json!({ "name": "minted", "definition": "{}", "roleArn": "role" }),
                crate::state::state_machine_arn(REGION, "minted"),
            ),
            (
                "an activity create, judged against the ARN its name mints",
                "CreateActivity",
                json!({ "name": "minted" }),
                crate::state::activity_arn(REGION, "minted"),
            ),
            (
                "an alias create, whose resource is named only inside its routing shape",
                "CreateStateMachineAlias",
                json!({ "name": "PROD", "routingConfiguration": [] }),
                ANY_RESOURCE.to_string(),
            ),
            (
                "a listing that names no resource",
                "ListStateMachines",
                json!({}),
                ANY_RESOURCE.to_string(),
            ),
            (
                "a call addressed by a task token",
                "SendTaskSuccess",
                json!({ "taskToken": "token", "output": "{}" }),
                ANY_RESOURCE.to_string(),
            ),
            (
                "a member carrying no text at all",
                "DescribeExecution",
                json!({ "executionArn": "" }),
                ANY_RESOURCE.to_string(),
            ),
            (
                "a member carrying a value of another type",
                "DescribeExecution",
                json!({ "executionArn": 7 }),
                ANY_RESOURCE.to_string(),
            ),
            (
                "a member the operation does not model, carried beside the one it does",
                "StartExecution",
                json!({ "stateMachineArn": machine, "executionArn": execution }),
                machine.clone(),
            ),
            (
                "a member the operation does not model, carried alone",
                "ListStateMachines",
                json!({ "stateMachineArn": machine }),
                ANY_RESOURCE.to_string(),
            ),
        ];
        for (case, operation, body, expected) in rows {
            let addressed = addressed_in(Some(&body), operation, REGION);
            assert_eq!(
                addressed, expected,
                "case {case:?}: expected the request to address {expected:?}, got {addressed:?}"
            );
        }
        // A body that is no JSON document at all names nothing either.
        let unreadable = addressed_in(None, "DescribeExecution", REGION);
        assert_eq!(
            unreadable, ANY_RESOURCE,
            "case \"a body that is not a JSON document\": expected {ANY_RESOURCE:?}, got \
             {unreadable:?}"
        );
    }
}
