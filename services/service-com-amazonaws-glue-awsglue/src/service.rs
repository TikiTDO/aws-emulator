// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-glue-awsglue` service: how it claims requests and how it words the
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
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. Glue states it in AWS's own constraint envelope at the
//!   member paths Glue's own API spells; see [`glue_constraint_refusal`].
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
//! Below the seams, this file also holds the service-wide WORDING of the Data Catalog's
//! own refusal of a member value outside what the model admits ([`database_name_refusal`]
//! and the functions beside it): those members are judged by the operation that reads
//! them rather than by the framework boundary, and several operations declare the same
//! member, so the sentences live here once instead of once per operation. The OTHER
//! layer's refusal — the boundary's constraint envelope — is rendered here too
//! ([`glue_constraint_refusal`]), once for both paths that answer one.
//!
//! # The request a response names
//!
//! Every response AWS sends names the request it answered, and an SDK surfaces that name as
//! the call's response metadata — the value a caller correlates its own logs with the
//! service's by. This service's protocol (`awsJson1_1`) carries it in the `x-amzn-requestid`
//! response HEADER alone, which the shared report
//! (`service_common::query_error_envelope::with_request_id_header`) writes, wired at the two
//! layers every response of this service passes through:
//!
//! * the operation [`Service::override_operation`] hands the router — the catalog wrapper
//!   below under [`RequestIdReportingOperation::header`], and [`RequestIdHeaderOperation`]
//!   for every other modelled operation — which covers a success, a modelled error, a
//!   refusal this file words itself and a constraint refusal raised while an operation was
//!   being served;
//! * [`RequestIdEnvelopeDefinition::header_only`], the definition wrapper
//!   [`Service::definition`] hands back, which covers a response NO operation produced: a
//!   routing failure, a transport failure detected up front, and a pre-dispatch
//!   authentication refusal.
//!
//! Nothing else about a response changes: the status, the other headers and every byte of
//! the body stay as the renderer wrote them.

use service_common::query_error_envelope::{
    RequestIdEnvelopeDefinition, RequestIdHeaderOperation, RequestIdReportingOperation,
};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, Operation, OperationContext,
    Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// The PARTITION INDEX records of the Data Catalog's tables: state this service keeps beside
/// the catalog itself, whose source file lives with the rest of this service's state
/// (`src/state/partition_indexes.rs`, beside [`crate::state::catalog_encryption`]).
///
/// It is DECLARED from this file because this is the file the service owns and which is never
/// regenerated: `src/ops/` holds one file per operation, so a module declared by a file there
/// is read as an operation this service's model does not have, and `src/state.rs` — the state
/// module root — is already at the length this project keeps a source file to.
#[path = "state/partition_indexes.rs"]
pub mod partition_indexes;

/// The rules the zero-ETL integration PROPERTY operations share: how each of them reads its
/// `ResourceArn`, and the sentences they refuse one with. Six operations address a record by
/// that member, so the reading and the wording live in one file rather than in six.
///
/// Declared from here for the same reason [`partition_indexes`] is, and its source file sits
/// with the rest of this service's state.
#[path = "state/integration_properties.rs"]
pub mod integration_properties;

/// The `com-amazonaws-glue-awsglue` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers. Identity, the
    /// protocol and the operations are the generated entry's untouched.
    entry: RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>,
    /// One [`CatalogNameValidation`] per operation whose name members this service words
    /// its own refusal for, keyed by the routing key the model names that operation by
    /// ([`Operation::NAME`], the key the generated routing table uses). Built once here,
    /// never per request; each entry is zero-sized, so this is a table of NAMES rather
    /// than a second set of operations — the operation that serves a request is still the
    /// one the generated table built from the shared engine.
    ///
    /// Each wrapper is composed inside [`RequestIdReportingOperation::header`], so the
    /// refusal it words names the request it answered exactly as every other response of
    /// this service does.
    name_refusals: HashMap<&'static str, Box<dyn ErasedOperation>>,
    /// Serves every OTHER modelled operation exactly as the generated routing table would,
    /// and names the request its response answers. Held, not built per request:
    /// `override_operation` hands back a borrow of it.
    request_id: RequestIdHeaderOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::header_only(crate::ServiceDefinition::new(engine)),
            name_refusals: catalog_name_validation_table::<Eng>(),
            request_id: RequestIdHeaderOperation,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what is
        // handed back is the shared `RequestIdEnvelopeDefinition`, which reads identity,
        // protocol and the routing table straight off that entry and adds the request-id
        // header to the three error renderers (see the module docs).
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
        // Serve the catalog operations whose name members this service judges itself
        // through their [`CatalogNameValidation`] wrapper, so a request that leaves one of
        // those members ABSENT is refused in the operation's own modelled
        // `InvalidInputException` instead of the framework's refusal body — the same
        // answer an EMPTY name earns inside the handler, because the catalog's sentence
        // for the check covers both. The wrapper hands every request it does not refuse to
        // the generated operation in the routing table, which remains the only thing that
        // serves one.
        //
        // Keyed by the protocol's own routing key, so a request the model names no
        // operation for resolves to nothing here and keeps the framework's routing
        // failure.
        let key = self.entry.protocol().routing_key(req).ok()?;
        if let Some(wrapper) = self.name_refusals.get(key) {
            return Some(&**wrapper);
        }
        // NOT a re-route: every other request is served by the operation this service's
        // own routing table names for it, through the shared `RequestIdHeaderOperation` —
        // the one place every other operation's response passes, so no operation can
        // answer without naming the request it answered. A request the table does not
        // answer declines here exactly as it would there, leaving the router's
        // routing-error path (which names the request itself, through the definition
        // wrapper) unchanged.
        self.entry
            .routes(req)
            .then_some(&self.request_id as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. Glue states such a refusal in AWS's own constraint envelope, which
        // differs from the framework's default in two renderings this service does not
        // choose for itself ([`glue_constraint_refusal`]): the member path is the one
        // Glue's own API spells, and a `@pattern` is quoted without the anchoring pair
        // the model wraps it in. Both the code and the message come from there, so a
        // client never sees one layer's message under the other layer's code.
        glue_constraint_refusal(refusal)
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

// ---------------------------------------------------------------------------
// How Glue states the boundary's constraint refusal
// ---------------------------------------------------------------------------

/// State `refusal` the way Glue states a constraint refusal, or `None` to keep the
/// framework's own wording.
///
/// The envelope is AWS's own and belongs to the shared renderer
/// (`service_common::validation_message::aws_validation_refusal`): HTTP 400
/// `ValidationException`, whose lowercase `message` carries
/// `N validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy
/// constraint: <requirement>`, with a `@pattern` requirement quoted WITHOUT the anchoring
/// pair the model wraps it in, which is the text AWS publishes for it. The one thing this
/// service adds is the MEMBER PATH: Glue states the path of its own request model
/// ([`api_request_path`]), which spells the member carrying a resource definition for the
/// resource it carries. Everything else — which refusals are restated, the code, the
/// envelope, the clause order and every other requirement — stays the shared renderer's
/// and the framework's, so two Glue refusal paths cannot drift apart.
///
/// Both paths that answer a boundary refusal in this service go through here: this
/// service's [`Service::override_refusal`], and the pre-handler wrapper at the end of this
/// file, which additionally restores each clause's echoed value to the bytes the caller
/// sent. Only the wrapper can do that — the refusal seam is handed the violations and not
/// the request — so a clause stated through `override_refusal` carries the framework's own
/// elision of a value past its echo ceiling.
fn glue_constraint_refusal(refusal: &Refusal) -> Option<Refused> {
    ::service_common::validation_message::aws_validation_refusal(&at_api_request_paths(refusal))
}

/// `refusal` with every violation's member path spelled the way Glue's own API spells it
/// ([`api_request_path`]).
fn at_api_request_paths(refusal: &Refusal) -> Refusal {
    match refusal {
        Refusal::Invalid(violations) => Refusal::Invalid(
            violations
                .iter()
                .map(|violation| Violation {
                    path: api_request_path(&violation.path),
                    ..violation.clone()
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The suffix the WIRE spells a member carrying a resource definition with
/// (`CreateDatabaseRequest$DatabaseInput`, `CreateTableRequest$TableInput`), which Glue's
/// own request path does not carry.
const DEFINITION_MEMBER_SUFFIX: &str = "Input";

/// `path` as Glue's own API spells the member it names: a member that carries a RESOURCE
/// DEFINITION is named for the resource it carries, so `databaseInput.name` is stated as
/// `database.name` and the member a request addresses a resource by (`name`,
/// `databaseName`) is stated exactly as the framework's walk spells it.
///
/// A constraint message is the only place the server names the member a caller got wrong,
/// so the path is part of the rule rather than prose: a client that reads it to find its
/// own field has to be given a member its request model declares. Only the FIRST segment
/// is rewritten, because that is the one naming a member of the request itself; every
/// segment under it names a member of the definition, which both models spell the same
/// way.
fn api_request_path(path: &str) -> String {
    let mut segments = path.splitn(2, '.');
    let Some(member) = segments.next() else {
        return path.to_string();
    };
    let stated = member
        .strip_suffix(DEFINITION_MEMBER_SUFFIX)
        .filter(|resource| !resource.is_empty())
        .unwrap_or(member);
    match segments.next() {
        Some(under) => format!("{stated}.{under}"),
        None => stated.to_string(),
    }
}

// ---------------------------------------------------------------------------
// How the Data Catalog refuses an input value its own validation rejects
// ---------------------------------------------------------------------------
//
// Two layers refuse a Glue request, and the recordings say which one answers what. The
// cross-service constraint validator answers a value past a `@length` MAXIMUM or outside
// a `@range` — HTTP 400 `ValidationException`, whose lowercase `message` carries the
// "N validation error(s) detected: …" envelope — which this service states in AWS's own
// rendering of it ([`glue_constraint_refusal`]). Everything else below is GLUE's own input
// validation: a value outside an enum's value set, an ARN outside `GlueResourceArn`'s
// pattern, and a required name member supplied as null or empty. Each of those is the
// operation's own modelled `InvalidInputException` (HTTP 400, prose in the `Message`
// member the model declares), carrying the sentence this service words that check with.
//
// So the members Glue judges itself are taken off the centralized boundary walk
// (`skip_validation` on their input shapes in `src/ops/types/`) and judged by the
// operation that read them — except for the one refusal no handler can reach, which the
// wrapper at the end of this file answers.
//
// The SENTENCES live here, once, because several operations declare the same member: a
// refusal `GetTable` makes has to read exactly like the one `CreateTable` makes.

/// The sentence the catalog refuses a resource's OWN name member supplied as null or
/// empty with, for a database (`DatabaseInput$Name`, and the `Name` a database write
/// addresses).
///
/// RECORDED constant, and the whole message: it states both halves of the check at once
/// and names neither the operation nor a value — there is no value to name.
const DATABASE_NAME_NULL_OR_EMPTY: &str = "Database name cannot be equal to null or empty";

/// The same for a table's own name member (`TableInput$Name`, and the `Name` a table read
/// or write addresses). RECORDED constant, and the whole message.
const TABLE_NAME_NULL_OR_EMPTY: &str = "Table name cannot be equal to null or empty";

/// The sentence the catalog refuses a `DatabaseName` — the member that addresses the
/// database a table operation works IN, rather than a database's own name — supplied as
/// null or empty with.
///
/// RECORDED constant, and a DIFFERENT sentence from [`DATABASE_NAME_NULL_OR_EMPTY`]: the
/// catalog states the parent reference as a requirement and a resource's own name as a
/// value it will not take, so the two are not one shared string.
const DATABASE_NAME_REQUIRED: &str = "Database name is required";

/// The sentence the catalog refuses a `ResourceShareType` outside the enum its member
/// targets with.
///
/// RECORDED template: fixed text plus ONE variable part, the value the request sent,
/// echoed exactly as sent — so a client reading the refusal sees which of its own values
/// the service would not take.
pub fn resource_share_type_not_valid(submitted: &str) -> String {
    format!("Resource share type {submitted} is not valid")
}

/// The sentence the catalog refuses an `AttributesToGet` entry that is no member of the
/// attribute enum its list targets with.
///
/// RECORDED constant, and the whole message: it names neither the entry the request sent
/// nor the set it had to come from, so one wording answers every such entry.
pub const ATTRIBUTE_NOT_VALID_TO_GET: &str = "Attribute name is not a valid attribute to get.";

/// Whether `attributes` carries an entry that is no member of `admitted` — the
/// generated, model-derived value set of the enum the list's member targets
/// (`crate::ops::types::TABLE_ATTRIBUTES_VALUES` and its siblings), so a value added to
/// the model reaches this check with the model.
pub fn attribute_outside_set(attributes: &[String], admitted: &[&str]) -> bool {
    attributes
        .iter()
        .any(|attribute| !admitted.contains(&attribute.as_str()))
}

// ---------------------------------------------------------------------------
// The name refusal the framework boundary would answer instead
// ---------------------------------------------------------------------------
//
// A required name member supplied as null or empty is GLUE's refusal, not the constraint
// validator's, but neither half of it can be made inside a handler: an ABSENT member is
// found while the request is being READ, so no input value ever exists to judge (the
// model makes such a member non-`Option` on the generated input struct), and an EMPTY one
// breaks the member's modelled `@length` minimum, which the boundary walk answers before
// any handler runs. Taking the member off that walk is not the answer either, because the
// SAME `@length` carries the maximum the validator does answer. So the wrapper below runs
// the framework's own boundary walk first, answers exactly those two violations in the
// catalog's own error, and leaves every other violation — the `@length` maximum included
// — to the boundary.

/// `InvalidInputException`'s wire error code and its modelled `@httpError`
/// (`smithy_query(query="shape",shape="InvalidInputException")`), restated here because
/// this file renders that error itself rather than returning it from a handler.
const INVALID_INPUT_EXCEPTION: &str = "InvalidInputException";

/// See [`INVALID_INPUT_EXCEPTION`].
const INVALID_INPUT_EXCEPTION_STATUS: u16 = 400;

/// The status the constraint validator's refusal carries, which this file renders itself
/// for a wrapped operation — the same 400 the framework's own boundary answers with
/// (`crate::scaffolded::Scaffolded::ValidationException`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The catalog operations whose name members Glue judges itself, each wrapped so a name
/// supplied as null or empty is answered in the catalog's own error.
///
/// Exactly the database and table operations whose name members the sentences above word a
/// refusal for: an operation added to that set is wrapped by adding one line here, and one
/// that judges no name member is served straight from the generated routing table.
fn catalog_name_validation_table<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    register_name_validation::<crate::ops::create_database::CreateDatabase<Eng>>(&mut refusals);
    register_name_validation::<crate::ops::update_database::UpdateDatabase<Eng>>(&mut refusals);
    register_name_validation::<crate::ops::get_database::GetDatabase<Eng>>(&mut refusals);
    register_name_validation::<crate::ops::delete_database::DeleteDatabase<Eng>>(&mut refusals);
    register_name_validation::<crate::ops::create_table::CreateTable<Eng>>(&mut refusals);
    register_name_validation::<crate::ops::update_table::UpdateTable<Eng>>(&mut refusals);
    register_name_validation::<crate::ops::get_table::GetTable<Eng>>(&mut refusals);
    register_name_validation::<crate::ops::get_tables::GetTables<Eng>>(&mut refusals);
    register_name_validation::<crate::ops::delete_table::DeleteTable<Eng>>(&mut refusals);
    refusals
}

/// Register one operation's wrapper under the key the model names that operation by, read
/// off the operation itself so the table cannot be keyed by a hand-typed name that drifts
/// from the model.
///
/// The wrapper is composed inside the shared request-id report, so the refusal it words
/// names the request it answered exactly as the generated operation's own answers do.
fn register_name_validation<Op: Operation>(
    refusals: &mut HashMap<&'static str, Box<dyn ErasedOperation>>,
) {
    refusals.insert(
        Op::NAME,
        Box::new(RequestIdReportingOperation::header(
            CatalogNameValidation::<Op>::new(),
        )),
    );
}

/// One modelled operation, served so that a request leaving one of its name members ABSENT
/// is answered in the operation's own modelled `InvalidInputException` rather than in the
/// framework's refusal body, which carries an error code and a lowercase `message` and
/// nothing else (`smithy_server_runtime::Refused`).
///
/// Zero-sized: the wrapper holds no engine and no operation instance. It carries the
/// operation TYPE, which is all it needs — the input shape to read the request into and
/// the input `Schema` to read it against — and hands a request it does not refuse to the
/// one operation instance the generated routing table built from the shared engine. There
/// is therefore no second instance of any operation and no second store.
///
/// A request this wrapper does not refuse is READ TWICE, once here and once in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation. That cost is deliberate, bounded by the body limits the framework already
/// enforces, and paid only by the few operations the table above names.
struct CatalogNameValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> CatalogNameValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for CatalogNameValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            // The protocol is read, used and dropped inside this one block:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = {
                let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
                // The framework's OWN boundary walk, collected through the shared half
                // every service that words its own pre-handler refusal uses — the
                // protocol's deserializer plus `constraints::validate` over the built
                // input — so this cannot find a different violation set from the sequence
                // that follows it. It additionally restores each clause's echoed value to
                // the bytes the caller sent, which only this side of the walk can do: the
                // framework's own rendering elides a long value, and AWS quotes it whole.
                let violations =
                    ::service_common::validation::refusing_violations::<Op>(protocol, req);
                match absent_name_exception(Op::NAME, &violations) {
                    // Glue's own refusal of a name it was given none of.
                    Some(modelled) => serialized_refusal::<Op>(
                        protocol,
                        &modelled,
                        &crate::ops::types::SCHEMA_INVALID_INPUT_EXCEPTION,
                        INVALID_INPUT_EXCEPTION,
                        INVALID_INPUT_EXCEPTION_STATUS,
                    ),
                    // Every other violation is the constraint validator's, and is stated
                    // the way AWS states one: the envelope and clauses the shared renderer
                    // owns, over the restored values above, at the member paths Glue's own
                    // API spells ([`glue_constraint_refusal`]).
                    None => glue_constraint_refusal(&Refusal::Invalid(violations)).and_then(
                        |stated| {
                            serialized_refusal::<Op>(
                                protocol,
                                &crate::scaffolded::ValidationException::new(stated.message),
                                &crate::scaffolded::VALIDATION_EXCEPTION_SCHEMA,
                                &stated.code,
                                VALIDATION_EXCEPTION_STATUS,
                            )
                        },
                    ),
                }
            };
            if let Some(response) = refused {
                // The row the framework sequence would have recorded for this request, so
                // a refused one is not a hole in the telemetry series.
                record_name_refusal::<Op>(ctx, started.elapsed());
                return response;
            }
            // Nothing to state: the request is served by the generated operation, which
            // runs the whole framework sequence (deserialize, validate, handle, serialize)
            // plus its telemetry over the same bytes. Reached through the routing table
            // rather than through `override_operation`, which would route straight back
            // here.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    /// `None` — the framework reads the schemes off the MODELLED operation the request's
    /// routing key names (the generated table's instance), which is what the model bakes
    /// them on, so wrapping an operation cannot change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// Serialize one refusal in the protocol the request is served in, or `None` when it will
/// not serialize — which leaves the request to the framework, refused in the framework's
/// own envelope. That is the very divergence this wrapper removes, so it is logged rather
/// than passed over silently.
fn serialized_refusal<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    error: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    error_schema: &::aws_smithy_schema::Schema,
    error_code: &str,
    http_status: u16,
) -> Option<aws_smithy_runtime_api::http::Response> {
    match protocol.serialize_error(error, error_schema, error_code, http_status) {
        Ok(response) => Some(response),
        Err(error) => {
            ::tracing::warn!(
                operation = Op::NAME,
                %error,
                "this service's own refusal would not serialize; it falls back to the \
                 framework's own envelope"
            );
            None
        }
    }
}

/// The modelled refusal for a request that supplied no value for one of `operation`'s name
/// members, or `None` for every other violation set — which keeps the framework's own
/// envelope.
///
/// Only a name supplied as null or empty is answered here: those are the two violations
/// Glue words itself ([`DATABASE_NAME_NULL_OR_EMPTY`] and the sentences beside it). A
/// violation of any other member, or of any other class — a name PAST the member's
/// modelled maximum, a value outside a `@range` — is left to the boundary, which answers
/// it in the constraint envelope the recordings show the validator answering.
fn absent_name_exception(
    operation: &str,
    violations: &[Violation],
) -> Option<crate::ops::types::InvalidInputException> {
    // Every violation of the set has to be one of these, so a request that ALSO broke a
    // constraint the validator answers keeps the validator's envelope rather than losing
    // that clause to this sentence. The first one states the refusal, as a service that
    // checks its members one at a time states it.
    if violations.is_empty() || !violations.iter().all(supplies_no_name) {
        return None;
    }
    let message = violations
        .iter()
        .find_map(|violation| name_refusal_message(operation, &violation.path))?;
    Some(crate::ops::types::InvalidInputException {
        message: Some(message.to_string()),
        from_federation_source: None,
    })
}

/// Whether this violation says the request supplied NO value for the member: it carried
/// none at all, or carried one shorter than the member's modelled `@length` minimum, which
/// for every name member of the catalog (`NameString`, min 1) means the empty string.
///
/// A value that merely breaks a different bound is not this refusal, so nothing else is
/// folded into it.
fn supplies_no_name(violation: &Violation) -> bool {
    match violation.kind {
        ViolationKind::MissingRequiredMember => true,
        ViolationKind::MinLength(_) => violation.value.as_deref() == Some(""),
        _ => false,
    }
}

/// The catalog's sentence for the name member at `path` of `operation`, or `None` when
/// `path` is no name member of the catalog's databases and tables.
///
/// The member paths are the ones the model declares, spelled as the framework spells a
/// violation path (lowerCamel, a nested member under its enclosing member). Which sentence
/// a member takes is a property of what it names, not of the operation that carries it: a
/// resource's OWN name member is the one the catalog will not take null or empty, the
/// `DatabaseName` that addresses the database an operation works in is the one it states as
/// a requirement, and a bare `Name` is the own name of the resource that operation
/// addresses — which is what the operation's name says it is.
fn name_refusal_message(operation: &str, path: &str) -> Option<&'static str> {
    match path {
        "databaseName" => Some(DATABASE_NAME_REQUIRED),
        "databaseInput.name" => Some(DATABASE_NAME_NULL_OR_EMPTY),
        "tableInput.name" => Some(TABLE_NAME_NULL_OR_EMPTY),
        "name" if operation.ends_with("Database") => Some(DATABASE_NAME_NULL_OR_EMPTY),
        "name" if operation.ends_with("Table") => Some(TABLE_NAME_NULL_OR_EMPTY),
        _ => None,
    }
}

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a request refused by the wrapper is not a hole in the telemetry
/// series: the same classification (from the refusal's status and error shape), the same
/// folded handle (this service's signing name and the operation's own name), and the same
/// already-sanitized request metadata. Everything is gated on a client collecting, exactly
/// as the framework gates it.
fn record_name_refusal<Op: Operation>(ctx: &OperationContext<'_>, latency: std::time::Duration) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            INVALID_INPUT_EXCEPTION_STATUS,
            INVALID_INPUT_EXCEPTION,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}
