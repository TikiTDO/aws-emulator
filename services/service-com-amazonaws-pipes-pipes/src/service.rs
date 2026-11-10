// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-pipes-pipes` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — EDITED HERE: the key and the handler stay the generated
//!   table's, but each modelled operation is served through [`FrontDoor`], which is where
//!   a refusal of the request BODY against the modelled input is worded — see "How a
//!   refused pipes request is worded".
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE for the refusals the resource-tagging layer owns;
//!   see the inline note on that method.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! # How a refused pipes request is worded
//!
//! Three layers answer a refused pipes request, and they do not answer alike.
//!
//! The FRONT DOOR validates the request's JSON BODY against the operation's modelled
//! input before the service sees it, and reports a value outside a modelled constraint as
//! the clause its body-schema validator states for that constraint class, in brackets:
//! `[ECMA 262 regex "<pattern>" does not match input string "<value>"]` for a `@pattern`,
//! `[string "<value>" is too long (length: <n>, maximum allowed: <max>)]` for a `@length`
//! maximum on a string, `[object has too many properties (found <n> but schema requires at
//! most <max>)]` for one on a map. The clause quotes the value the CALLER submitted, which
//! is why the wording happens in [`FrontDoor`], per operation, where the request can be
//! re-read: the `override_refusal` seam is handed the violations alone, and the framework's
//! own violation carries an elided rendering of a long value.
//!
//! A value the request carries OUTSIDE that body — a `@httpLabel` in the URI, a
//! `@httpQuery` parameter — is in no body schema, so the front door has no clause for it
//! and names the member alone (`[tagKeys]`) when the value is EMPTY where the model
//! requires at least one character or element: an empty query value never becomes a
//! parameter at all. Everything else about such a member is the SERVICE's own refusal, with
//! one exception the service validates itself: the pipe NAME, whose `@pattern` is reported
//! as this service's own `ValidationErrorFields` document naming the member as the control
//! plane spells it ([`PIPE_NAME_FIELD`]) — the same document `crate::state` builds for the
//! refusals its handlers state.
//!
//! The resource-tagging layer in FRONT of the three ARN-addressed tagging operations owns
//! what a TAG may contain, and refuses a tag whose content breaks a published tag rule with
//! its own code — [`TAG_RULE_REFUSAL_CODE`], which no operation of this model declares,
//! because the layer is not the service. `crate::state::tag_refusal` states the same
//! sentences for the rules the handlers enforce themselves, so one tag rule is worded one
//! way whichever layer catches it. Its message is not a bare sentence: a rule the model
//! states as a `@length` is reported as the request-validation sentence wrapped in the JSON
//! document that layer's message IS, while a rule the model states nowhere is the
//! documented sentence itself — see `crate::state::tag_refusal` for both.
//!
//! # How an answer that carries no content is framed
//!
//! An operation whose modelled output declares no member leaves its caller nothing to
//! parse, so it answers the no-content status (see `SUCCESS_STATUS` in
//! `src/ops/tag_resource.rs` and `src/ops/untag_resource.rs` — the only two outputs of this
//! model with no member). Both are a STRUCTURE rather than `smithy.api#Unit`
//! (`smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse")`), and
//! the protocol's serializer renders such a structure as the empty JSON object: a body on
//! an answer that has none.
//!
//! [`Service::override_protocol`] below therefore serves a protocol that is the model's own
//! in every respect except one: a no-content answer this service renders carries no body at
//! all and states the length it does have, through
//! `service_common::no_content_responses::without_content`, which owns that framing for
//! every service. An answer that carries content — every reader's, every lifecycle writer's,
//! and every modelled failure's — is left exactly as the model's protocol renders it. It
//! happens at the one place every response of this service passes through rather than per
//! operation.
//!
//! An answer served over HTTP/1.1 reaches the client without that framing header: the HTTP
//! writer the vendored listener is built on drops `content-length` from a status that
//! cannot carry a body, which is below anything this crate decides.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use service_common::front_door::binds_to_body;
// The framing of an answer that carries no content, shared with every other service whose
// success is the no-content status. See "How an answer that carries no content is framed".
use service_common::no_content_responses::without_content;
use service_common::validation::{refusing_violations_with_echoes, SubmittedEcho};
use service_common::validation_message::{
    map_entry_count, pattern_mismatch, string_too_long, too_many_properties,
};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code the layer that fronts this service's tagging operations refuses a
/// broken tag rule with. No operation of this model declares it — the layer is not the
/// service — so a refusal worded with it cannot come from the modelled error enums; see
/// `crate::scaffolded::Scaffolded::BadRequest` for the handler-side channel.
const TAG_RULE_REFUSAL_CODE: &str = "BadRequestException";

/// The code and status every refusal [`FrontDoor`] words carries: the modelled
/// `ValidationException` (`com.amazonaws.pipes#ValidationException`, `@httpError(400)`),
/// which is the code the framework's own refusal channel names too — only the BODY differs.
const VALIDATION_EXCEPTION: &str = "ValidationException";
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The violation path of the pipe NAME: `Name` is a `@httpLabel` on every operation that
/// addresses one pipe, and the walk spells a member's path in lowerCamel.
const PIPE_NAME_PATH: &str = "name";

/// The member a refused pipe NAME is reported at. The wire member is `Name`; the control
/// plane's own validation reports it as the recorded refusal names it.
const PIPE_NAME_FIELD: &str = "PipeName";

/// The input members a TAG's own content arrives on, and the name request validation
/// reports a refused tag at for each.
///
/// `tagKeys` is bound ONLY by `UntagResource`
/// (`smithy_query(query="digest")`), so a violation on it is attributable and is reported
/// at the name that operation's validation uses. `tags` is bound by `TagResource` AND by
/// `CreatePipe`, and a [`Refusal`] carries no operation id, so a violation on it is
/// reported at the member as the MODEL names it (lowerCamel, which is what both
/// operations' paths spell) rather than at either operation's own validation name.
/// Narrowing that needs an operation-aware refusal seam, which this service cannot add.
const TAG_CONTENT_MEMBERS: &[&str] = &["tags", "tagKeys"];

/// The input member a violation is about: a `Violation::path` is DOTTED
/// (`tagKeys.1.member` for a list element, `tags.<key>` for a map value), so the member
/// is the path's first segment and not the whole path.
fn member_of(violation: &Violation) -> &str {
    violation.path.split('.').next().unwrap_or_default()
}

/// How a violation on one of [`TAG_CONTENT_MEMBERS`] is worded, or `None` when the
/// violation is not about ONE tag's content.
///
/// A violation at the member itself (`tags`, `tagKeys`) is about how MANY tags the
/// request carries — the collection's own `@length` — which is not a tag rule and keeps
/// the modelled wording. A violation one step inside it (a map VALUE, whose path segment
/// is the entry's key, or a list ELEMENT) is a tag whose content breaks the published
/// `TagKey`/`TagValue` bound, which is the layer's refusal to word.
fn tag_content_clause(violation: &Violation) -> Option<String> {
    let (member, inside) = violation.path.split_once('.')?;
    let value = violation.value.as_deref().unwrap_or_default();
    match member {
        // `tags.<key>`: the VALUE sent for that key is outside `TagValue`'s bound. The
        // key is the rest of the path, which it contributes whole (a key may carry a
        // dot of its own). A [`Refusal`] carries only the value that broke the rule, so
        // the entry it broke is the whole of the member's value this can echo.
        "tags" => Some(crate::state::tag_refusal(
            crate::state::CREATE_TAGS_MEMBER,
            crate::state::TagViolation::ValueLength,
            &crate::state::echoed_tags(std::iter::once((inside, value))),
        )),
        // `tagKeys.<position>.member`: that key is outside `TagKey`'s bound.
        "tagKeys" => Some(crate::state::tag_key_refusal(
            crate::state::REMOVED_TAG_KEYS_MEMBER,
            value,
        )),
        _ => None,
    }
}

/// The `com-amazonaws-pipes-pipes` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`FrontDoor`] so a request the front door refuses is worded the way that layer
    /// words one. Keyed by the routing key the protocol resolves — the key the generated
    /// table uses — and built from the SAME shared engine, so a wrapped operation reads
    /// and writes the one store.
    front_door: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`FrontDoor`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(FrontDoor::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut front_door = OperationRoutingTable::new();
        // EVERY modelled operation, so the front door's rule is applied by ONE code path
        // rather than per operation: an operation that binds nothing to the body
        // (`DescribePipe`, whose only member is its URI label) simply never has a body
        // violation to word, which the wrapper reads off its own input schema.
        wrap_operations!(
            front_door,
            engine,
            create_pipe::CreatePipe,
            delete_pipe::DeletePipe,
            describe_pipe::DescribePipe,
            list_pipes::ListPipes,
            list_tags_for_resource::ListTagsForResource,
            start_pipe::StartPipe,
            stop_pipe::StopPipe,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_pipe::UpdatePipe,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            front_door,
        }
    }
}

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing correction the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &::aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &::aws_smithy_schema::Schema,
    ) -> Result<Box<dyn ::aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn ::aws_smithy_schema::serde::SerializableStruct,
        output_schema: &::aws_smithy_schema::Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        // The status and every header are the protocol's: only an answer that carries no
        // content is emptied of the body rendered for a members-less output.
        Ok(without_content(self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?))
    }

    fn serialize_error(
        &self,
        error: &dyn ::aws_smithy_schema::serde::SerializableStruct,
        error_schema: &::aws_smithy_schema::Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        // Every modelled failure of this service carries its error envelope at a 4xx or a
        // 5xx, so nothing is emptied here; the same call is made on both paths rather than
        // leaving one of them to answer differently.
        Ok(without_content(self.entry.protocol().serialize_error(
            error,
            error_schema,
            error_code,
            http_status,
        )?))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn ::aws_smithy_schema::serde::SerializableStruct,
        event_schema: &::aws_smithy_schema::Schema,
    ) -> Result<RtResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names the
/// protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
    }
}

/// One modelled operation, served behind this service's front door (see "How a refused
/// pipes request is worded").
///
/// Nothing here re-implements a check: the violations come from the framework's own
/// machinery (the protocol's deserializer and `constraints::validate` over the built
/// input) through `service_common::validation`, which also restores each value echo to the
/// bytes the caller submitted, and the status and the error code are the modelled ones.
/// Only the SENTENCE is this layer's. A request it has no wording for is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error — including the refusals the handlers and
/// the tagging layer state.
///
/// The wrapper exists because the clause needs the REQUEST: it quotes the submitted value,
/// and it has to tell a BODY member from one bound to the URI or the query string, which
/// is read off this operation's own input schema. The `override_refusal` seam is handed the
/// violations alone, so neither is reachable there. The cost is DELIBERATE: a valid
/// request's body is read twice, once here and once in the operation's own sequence,
/// because the framework hands no already-built input to an operation.
///
/// `service_common::front_door::FrontDoor` is the same plumbing for the services whose
/// front door answers one FIXED sentence (`BadRequestException` / "Invalid request body"),
/// and this service cannot use it as it stands: the refusal here is the modelled
/// `ValidationException` whose body is BUILT from the violations. Giving that shared
/// wrapper a wording hook, and moving this service and the other copies onto it, is a
/// change to a module several services already depend on and is not this issue's to make —
/// which is why the clause builders this wrapper calls are in `service_common`
/// (`validation`, `validation_message`, `front_door::binds_to_body`) even though the
/// wrapper itself is here: the RULES are shared, only the plumbing is repeated.
struct FrontDoor<O> {
    operation: O,
}

impl<O> FrontDoor<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for FrontDoor<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one block:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = {
                let protocol = svc.definition().protocol();
                let violations = refusing_violations_with_echoes::<O>(protocol, req);
                let stated = front_door_refusal(O::INPUT_SCHEMA, &violations).map(|exception| {
                    protocol
                        .serialize_error(
                            &exception,
                            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                            VALIDATION_EXCEPTION,
                            VALIDATION_EXCEPTION_STATUS,
                        )
                        .unwrap_or_else(|error| {
                            svc.definition().framework_error_response(error, req)
                        })
                });
                // The tagging layer's own refusal, worded HERE rather than in
                // `override_refusal` because its clause echoes the tag's value and the
                // validating walk withholds the value of a `@sensitive` member — which a
                // tag's is — as it records the violation. This reading of the request is
                // what still holds it.
                stated.or_else(|| {
                    tag_rule_refusal(&violations).map(|stated| {
                        svc.definition().refusal_response(
                            ::aws_smithy_schema_server::ProtocolError::Invalid(
                                violations.iter().map(|(violation, _)| violation.clone()).collect(),
                            ),
                            Some(stated),
                            req,
                        )
                    })
                })
            };
            if let Some(response) = refused {
                return response;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The tagging layer's refusal of a tag whose CONTENT breaks a published tag rule, or
/// `None` when this refusal is not that one.
///
/// The same decision [`Service::override_refusal`] takes — every violation has to be about
/// one tag's content, and the layer states the first rule the request broke — over the
/// violations as a reading of the REQUEST restores them: the clause echoes the tag's value,
/// and the validating walk withholds the value of a `@sensitive` member (a tag's value is
/// one) as it records the violation, so the restoration
/// ([`service_common::validation_message::quoting_the_whole_submitted_value`], which puts
/// back exactly what that walk would have stored) is what lets the clause state it.
fn tag_rule_refusal(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    if refused.is_empty()
        || !refused
            .iter()
            .all(|(violation, _)| TAG_CONTENT_MEMBERS.contains(&member_of(violation)))
    {
        return None;
    }
    let message = refused
        .iter()
        .filter_map(|(violation, echo)| {
            // ONLY a withheld value is restored: everything else reaches here with the
            // value the framework's own walk stored — already elided at that walk's
            // ceiling — which is the value this layer's recorded clause quotes.
            let stated = if violation.sensitive {
                service_common::validation_message::quoting_the_whole_submitted_value(
                    violation.clone(),
                    echo,
                )
            } else {
                violation.clone()
            };
            tag_content_clause(&stated)
        })
        .next()?;
    Some(Refused {
        code: TAG_RULE_REFUSAL_CODE.to_string(),
        message,
    })
}

/// The `ValidationException` the front door answers one refused request with, or `None`
/// when it has no wording for it and the layers behind it answer instead.
///
/// The pipe NAME comes FIRST: it is read from the request URI before a body is parsed at
/// all, and the control plane validates it itself, so a request that gets its name wrong is
/// refused by that document whatever else it carries. Otherwise every violation has to be
/// one this layer states a clause for — a request carrying one it does not is left whole to
/// the framework's own wording rather than reported half in one voice and half in another.
fn front_door_refusal(
    input_schema: &::aws_smithy_schema::Schema,
    refused: &[(Violation, SubmittedEcho)],
) -> Option<crate::ops::types::ValidationException> {
    if let Some(field) = refused
        .iter()
        .find_map(|(violation, _)| pipe_name_field(violation))
    {
        return Some(crate::state::validation_error(vec![field]));
    }
    // A value that arrived EMPTY where the model requires at least one character or element
    // never became a parameter at all, so the front door reports that member and nothing
    // else about it: whatever ELSE the empty value fails (its pattern, say) is moot beside
    // the value not being there.
    let empty_members: Vec<&str> = refused
        .iter()
        .filter(|(violation, _)| {
            matches!(violation.kind, ViolationKind::MinLength(_))
                && !binds_to_body(input_schema, &violation.path)
        })
        .map(|(violation, _)| member_of(violation))
        .collect();

    let mut clauses: Vec<String> = Vec::new();
    for (violation, echo) in refused {
        let moot = empty_members.contains(&member_of(violation))
            && !matches!(violation.kind, ViolationKind::MinLength(_));
        if moot {
            continue;
        }
        let clause = clause(input_schema, violation, echo)?;
        if !clauses.contains(&clause) {
            clauses.push(clause);
        }
    }
    if clauses.is_empty() {
        return None;
    }
    Some(crate::ops::types::ValidationException {
        // The clauses of one refusal, as the schema validator lists them. No `fieldList`:
        // the member-by-member list is what the SERVICE's own validation reports (see
        // `crate::state::validation_error`), and this refusal is not the service's.
        message: Some(format!("[{}]", clauses.join(", "))),
        field_list: None,
    })
}

/// The field a refused pipe NAME is reported as, or `None` when the violation is not the
/// name's modelled `@pattern`.
///
/// The sentence states the rule and the expression the MODEL declares for `PipeName`, both
/// taken from the violation rather than written out, so it follows the model. A name
/// outside the length bound is a different rule with no recorded sentence of its own, so it
/// keeps the framework's wording, which states the bound it broke.
fn pipe_name_field(violation: &Violation) -> Option<crate::ops::types::ValidationExceptionField> {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return None;
    };
    (violation.path == PIPE_NAME_PATH).then(|| {
        crate::state::validation_field(
            PIPE_NAME_FIELD,
            format!(
                "Invalid pattern for pipe name. Pipe name should match pattern {pattern}."
            ),
        )
    })
}

/// The clause the front door states for one violation, or `None` when it states none.
///
/// A member carried OUTSIDE the request body is in no body schema: the front door reads it
/// from the URI or the query string, where a value that is EMPTY where the model requires
/// at least one character or element never arrives as a parameter at all, and the refusal
/// names the member alone. Every other rule about such a member belongs to the service.
///
/// A BODY member is reported by the clause the body-schema validator states for the class
/// of constraint it broke, quoting the value the caller submitted — every one of those
/// sentences through `service_common::validation_message`, which is their home for the
/// whole tree. Only a TOP-LEVEL member is worded: one nested inside a structure, a map
/// entry or a list element is reported by the layer that owns that member — a tag's content
/// is the tagging layer's ([`tag_content_clause`]) — so its clause is not this layer's to
/// state. A class that layer has no recorded sentence for (a `@length` MINIMUM on a body
/// string, a `@range`, an enum, an absent `@required` member) keeps the framework's own
/// wording rather than being given one here.
fn clause(
    input_schema: &::aws_smithy_schema::Schema,
    violation: &Violation,
    echo: &SubmittedEcho,
) -> Option<String> {
    if !binds_to_body(input_schema, &violation.path) {
        return matches!(violation.kind, ViolationKind::MinLength(_))
            .then(|| member_of(violation).to_string());
    }
    if violation.path.contains('.') {
        return None;
    }
    match (echo, &violation.kind) {
        (SubmittedEcho::Rendered(value), ViolationKind::Pattern(pattern)) => {
            Some(pattern_mismatch(pattern, value))
        }
        (SubmittedEcho::Rendered(value), ViolationKind::MaxLength(max)) => {
            // `@length` counts CHARACTERS, which is what the constraint walk counted.
            Some(string_too_long(value, value.chars().count(), *max))
        }
        // A value with no text rendering whose `@length` is the ENTRY COUNT of a map: the
        // body schema states that bound as the object's property count, which
        // `service_common::validation_message` already states once for the whole tree
        // (together with the `<n> entries` rendering the count is read from). Every map a
        // request of this service carries at the top level of its body is a `TagMap`.
        (SubmittedEcho::Unrendered, ViolationKind::MaxLength(max)) => {
            let found = map_entry_count(violation.value.as_deref()?)?;
            Some(too_many_properties(found, *max))
        }
        _ => None,
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in [`FrontDoor`] so a
        // request the front door refuses is worded the way that layer words one. A key the
        // model does not name resolves to `None` here exactly as it does in the generated
        // table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // [`FrontDoor`] answers every refusal this service has its own wording for before
        // the framework's walk reaches here, so this seam is left with the one layer whose
        // refusal is keyed on the violation ALONE: the resource-tagging layer in front of
        // the three ARN-addressed tagging operations, which refuses a tag whose CONTENT
        // breaks a published tag rule with its own code — `BadRequestException`, which no
        // operation of this model declares, because the layer is not the service. The rule
        // the tag broke is one the model DOES declare (`TagKey`'s and `TagValue`'s
        // `@length`), so the boundary validator is what finds it.
        // `crate::state::tag_refusal` states the same sentences for the rules the handlers
        // enforce themselves, so one tag rule is worded one way whichever layer catches it.
        //
        // Everything else is the SERVICE's own refusal, and this service models
        // `ValidationException` for it — which is the code the default already names —
        // so it keeps the framework's wording.
        let violations = match refusal {
            Refusal::Invalid(violations) => violations,
            // An authentication refusal is the framework's 403 to word, not ours.
            _ => return None,
        };
        if !violations
            .iter()
            .all(|violation| TAG_CONTENT_MEMBERS.contains(&member_of(violation)))
        {
            return None;
        }
        let clauses: Vec<String> = violations.iter().filter_map(tag_content_clause).collect();
        // One refused tag at a time: the layer states the first tag rule the request
        // broke, as the handlers do, so the two cannot be told apart by how many
        // clauses they carry.
        let message = clauses.into_iter().next()?;
        Some(Refused {
            code: TAG_RULE_REFUSAL_CODE.to_string(),
            message,
        })
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by
        // the protocol returned here. See the module docs: it is the model's own protocol,
        // with one correction — an answer that carries no content carries no body, instead
        // of the empty JSON object the serializer renders for a members-less output.
        Some(self)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
