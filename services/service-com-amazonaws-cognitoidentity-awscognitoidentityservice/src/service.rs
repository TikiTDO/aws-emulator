// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cognitoidentity-awscognitoidentityservice` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name, which this
//!   service widens to the two names ONE awsJson endpoint answers on (see the method).
//!   Edit it for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
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
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header the
//! protocol reports that id in (`x-amzn-RequestId` for `awsJson1_1`) — on a success
//! exactly as on a failure. It is a property of the RESPONSE, not of any modelled shape:
//! no output and no error shape of this model declares it, so no handler can set it, and
//! the framework's `awsJson1_1` renderers do not write it. An SDK surfaces it as the
//! call's `ResponseMetadata.RequestId`, which is what a client logs and correlates with.
//!
//! The rule is not restated here: `service_common::query_error_envelope` owns it for
//! every emulated service, and this file only chooses the report this protocol takes
//! (the header alone — `awsJson1_1` has no envelope element for the id) and wires it at
//! the two layers every response of this service passes through:
//!
//! * [`validation::QuotingSubmittedValues`], the wrapper [`Service::override_operation`]
//!   hands the router in place of each modelled operation, covers everything an operation
//!   answers — a success, a modelled error, and a constraint refusal raised for that
//!   operation's request;
//! * [`RequestIdEnvelopeDefinition::header_only`], the definition wrapper
//!   [`Service::definition`] hands back, covers a response NO operation produced: a
//!   routing failure, a transport failure detected up front, and a pre-dispatch
//!   authentication refusal.
//!
//! Nothing else about a response changes: the status, the other headers and every byte
//! of the body stay as the renderer wrote them.
//!
//! ## Naming the exception a failure is refused with
//!
//! Every failure AWS answers for this service names its exception shape in the
//! `x-amzn-errortype` header as well as in the body, and the framework's `awsJson`
//! renderers write the body alone. [`error_type_header`] owns that header, at the same two
//! layers and wrapped around the same two request-id reporters, so one response names both
//! the request it answers and the exception it answers with.
//!
//! ## Stating the value a constraint refusal is about
//!
//! A clause about a COLLECTION member states the collection the request carried, which the
//! framework's constraint walk has no rendering for and `override_refusal` cannot reach:
//! [`validation`] owns that, in the per-operation wrapper this file hands the router.

pub mod error_type_header;
pub mod validation;

/// The table of the status rule [`crate::scaffolded::wire_status`] states — test-only, the
/// rule itself living beside the error channel it renders.
#[cfg(test)]
mod modeled_error_status;

use error_type_header::ErrorTypeHeaderDefinition;
use service_common::data_events::DeliveringDataEvents;
use service_common::query_error_envelope::RequestIdEnvelopeDefinition;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, OperationRoutingTable, Refusal, Refused,
    Service,
};
use std::sync::Arc;
use validation::QuotingSubmittedValues;

use crate::state::data_events::IdentityPoolDataEvents;

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`QuotingSubmittedValues`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(QuotingSubmittedValues::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// Wire one operation of the identity DATA PLANE into `table`: wrapped exactly as the line
/// above wraps every operation, and served behind the wrapper that DELIVERS the data event
/// its handler holds ([`service_common::data_events::DeliveringDataEvents`]).
///
/// The delivery is outermost, because the record names the request the CALLER was answered
/// under and that id exists only once every layer that reports it has rendered the response.
macro_rules! wrap_data_plane_operations {
    ($table:expr, $engine:expr, $deliverer:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(DeliveringDataEvents::new(
                    QuotingSubmittedValues::new(
                        crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                    ),
                    ::std::sync::Arc::clone($deliverer),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// The `com-amazonaws-cognitoidentity-awscognitoidentityservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers and the exception
    /// it is refused with.
    entry: ErrorTypeHeaderDefinition<RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`QuotingSubmittedValues`] — so a refusal states the value the request carried, and
    /// every response names the request it answers and the exception it is refused with.
    /// Keyed by the routing key the protocol resolves — the key the generated table uses —
    /// and built from the SAME shared engine, so a wrapped operation reads and writes the
    /// one store. Held on the service because [`Service::override_operation`] hands back a
    /// borrow of `self`.
    quoting_submitted_values: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut quoting_submitted_values = OperationRoutingTable::new();
        wrap_operations!(
            quoting_submitted_values,
            engine,
            create_identity_pool::CreateIdentityPool,
            delete_identities::DeleteIdentities,
            delete_identity_pool::DeleteIdentityPool,
            describe_identity::DescribeIdentity,
            describe_identity_pool::DescribeIdentityPool,
            get_identity_pool_roles::GetIdentityPoolRoles,
            get_principal_tag_attribute_map::GetPrincipalTagAttributeMap,
            list_identities::ListIdentities,
            list_identity_pools::ListIdentityPools,
            list_tags_for_resource::ListTagsForResource,
            lookup_developer_identity::LookupDeveloperIdentity,
            merge_developer_identities::MergeDeveloperIdentities,
            set_identity_pool_roles::SetIdentityPoolRoles,
            set_principal_tag_attribute_map::SetPrincipalTagAttributeMap,
            tag_resource::TagResource,
            unlink_developer_identity::UnlinkDeveloperIdentity,
            untag_resource::UntagResource,
            update_identity_pool::UpdateIdentityPool,
        );
        // The identity DATA PLANE: the five operations CloudTrail delivers as DATA events of
        // the identity pool they act on, each served behind the delivery of that event.
        let data_events = Arc::new(IdentityPoolDataEvents::new(Arc::clone(&engine)));
        wrap_data_plane_operations!(
            quoting_submitted_values,
            engine,
            &data_events,
            get_credentials_for_identity::GetCredentialsForIdentity,
            get_id::GetId,
            get_open_id_token::GetOpenIdToken,
            get_open_id_token_for_developer_identity::GetOpenIdTokenForDeveloperIdentity,
            unlink_identity::UnlinkIdentity,
        );
        Self {
            entry: ErrorTypeHeaderDefinition::new(RequestIdEnvelopeDefinition::header_only(
                crate::ServiceDefinition::new(engine),
            )),
            quoting_submitted_values,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what
        // is handed back is the shared `RequestIdEnvelopeDefinition` under this
        // service's `ErrorTypeHeaderDefinition`, which read identity, protocol and
        // the routing table straight off that entry and add the request-id and
        // error-type headers to the three error renderers (see the module docs).
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service speaks `awsJson1_1` and several of its operations are called
        // with NO SigV4 credentials — `GetId`, `GetOpenIdToken` and
        // `GetCredentialsForIdentity` are the credential exchange a client performs
        // BEFORE it holds any Amazon Web Services credential, so the model marks them
        // anonymous. The framework resolves a routing key from the SigV4 credential
        // scope FIRST and from `X-Amz-Target` only when there is no scope, and those
        // two carry different names: the scope carries the signing name
        // (`cognito-identity`), the target carries the model's service shape name
        // (`AWSCognitoIdentityService.<Operation>`). Claiming the signing name alone
        // therefore serves every SIGNED call and leaves every unsigned one unclaimed,
        // which the router answers `UnknownService` — the service is mounted, but
        // nothing claims the request.
        //
        // Both names address this one endpoint, so both are its routing keys, and the
        // framework's own resolver still decides which one a given request resolves
        // to: a request signed for ANOTHER service resolves to that service's signing
        // name and is not claimed here however it is addressed. This service's signing
        // name is its own, so the claim needs no further discriminator.
        routes_to_signing_name(parts, self.entry.identity().signing_name())
            || unsigned_target_names_shape(parts, self.entry.identity().shape_id())
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // NOT a re-route: the operation served is the one the generated routing table names
        // for this request, wrapped so that a refusal states the value the request carried
        // and so that every response names the request it answers and the exception it is
        // refused with (see [`validation::QuotingSubmittedValues`]) — the one place every
        // operation's response passes. The routing DECISION stays the model's: the key is
        // resolved by its own protocol, and a request the generated table does not answer
        // declines here, leaving the router's routing-error path (which names the request
        // itself, through the definition wrapper) unchanged. The wrapper declares the
        // wrapped operation's own auth schemes, so this service's anonymous credential
        // exchange stays anonymous.
        if !self.entry.inner().routes(req) {
            return None;
        }
        let operation = self.entry.protocol().routing_key(req).ok()?;
        self.quoting_submitted_values.get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // A value that fails a constraint the MODEL declares — an `IdentityPoolId`
        // outside `^[\w-]+:[0-9a-f-]+$`, a `MaxResults` outside its range, a member the
        // shape requires and the request omits — is refused by the constraint-validation
        // layer that runs AHEAD of the operation, with HTTP 400 and the error code
        // `ValidationException`. No operation of this service declares that error, and
        // that is the point: the refusal is the protocol's, not one of the service's own
        // modelled failures, so it is NOT `InvalidParameterException` — which this
        // service's handlers answer for a request whose VALUES are well-formed but
        // cannot be served (an ARN that names no identity pool, a pagination token this
        // service did not issue). A client branches on the code, and the two refusals
        // are different things.
        match refusal {
            // The clauses, the envelope and the code are the shared renderings, as they are
            // for a refusal the per-operation wrapper words ([`validation`]); what cannot be
            // stated here is the one half that needs the REQUEST — the collection a clause
            // is about — because this seam is handed the violations alone.
            Refusal::Invalid(_) => validation::stated_refusal(refusal),
            // Authentication is the emulator's to answer, not this service's.
            _ => None,
        }
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

/// Whether the routing key the framework resolves for `parts` is `signing_name`.
///
/// This is `emulator_server::can_handle_based_on_route_key`'s rule applied over
/// `Parts`: that helper takes a whole buffered `http::Request`, which the
/// `Service::can_handle` seam does not receive. Comparison is case-insensitive on a
/// trimmed name, as there. It answers for every SIGNED request (the credential scope
/// carries the signing name) and for one addressed by this service's endpoint host.
fn routes_to_signing_name(parts: &http::request::Parts, signing_name: &str) -> bool {
    smithy_server_runtime::resolve_route_key(parts)
        .is_some_and(|key| key.eq_ignore_ascii_case(signing_name.trim()))
}

/// Whether `parts` carries NO SigV4 credential scope, names the service whose shape id
/// is `service_shape_id` in its `X-Amz-Target`, and arrives on the path this service's
/// protocol uses.
///
/// The three conjuncts, each for its own reason:
///
/// * NO credential scope — neither an `Authorization` header nor a presigned
///   `X-Amz-Credential` query parameter. The framework resolves a routing key from the
///   scope FIRST (`emulator_server::resolve_route_key`), so a request signed for another
///   service resolves to that service and must not be claimed here however it is
///   targeted; reading the target only in the scope's absence keeps the framework's
///   precedence instead of competing with it.
/// * the target NAMES this service — read with the framework's own
///   `service_from_target`, which takes the segment before the first dot and normalises
///   it the way a routing key is normalised. This service's published target prefix is a
///   single Smithy identifier (`AWSCognitoIdentityService`), so that segment IS the
///   shape's local name; the comparison is therefore against the shape id the generated
///   identity states, with its namespace dropped, and nothing is written out here that
///   the model does not state.
/// * the ROOT path, which is the only path an `awsJson` operation posts to. A request
///   that names this service out of band while addressing some other model's REST path
///   contradicts itself, so it must reach NOBODY rather than a handler picked off the
///   target alone.
fn unsigned_target_names_shape(parts: &http::request::Parts, service_shape_id: &str) -> bool {
    if emulator_server::service_from_sigv4(&parts.headers).is_some()
        || emulator_server::service_from_sigv4_query(&parts.uri).is_some()
    {
        return false;
    }
    let shape_name = service_shape_id
        .split('#')
        .next_back()
        .unwrap_or(service_shape_id)
        .trim();
    emulator_server::service_from_target(&parts.headers)
        .is_some_and(|named| named.eq_ignore_ascii_case(shape_name))
        && parts.uri.path().trim_matches('/').is_empty()
}

#[cfg(test)]
mod tests {
    //! The two names this ONE `awsJson` endpoint answers on, over the signals a request
    //! can carry. `tests/all/anonymous_credential_exchange.rs` asserts the same rule on
    //! the wire, through the router; this table is the rule itself, row by row.

    use super::{routes_to_signing_name, unsigned_target_names_shape};

    /// This service's SigV4 signing name and Smithy shape id, as the generated identity
    /// states them (`smithy_query(query="service_info")`: `sigv4Name` `cognito-identity`,
    /// service shape `com.amazonaws.cognitoidentity#AWSCognitoIdentityService`).
    const SIGNING_NAME: &str = "cognito-identity";
    const SHAPE_ID: &str = "com.amazonaws.cognitoidentity#AWSCognitoIdentityService";

    /// A credential scope naming `signing_name`, the primary routing signal.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=abc"
        )
    }

    /// One row: how the request is addressed — its credential scope, its `X-Amz-Target`,
    /// its `Host` and its request URI, each absent as `None` — and whether this service
    /// claims it.
    struct Row {
        case: &'static str,
        scope: Option<&'static str>,
        target: Option<&'static str>,
        host: Option<&'static str>,
        uri: &'static str,
        expected: bool,
    }

    /// The path every `awsJson` operation posts to.
    const ROOT: &str = "/";

    #[test]
    fn this_endpoint_answers_on_its_signing_name_and_on_its_shape_name() {
        let rows = [
            Row {
                case: "signed for it, targeted at it",
                scope: Some(SIGNING_NAME),
                target: Some("AWSCognitoIdentityService.DescribeIdentityPool"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                // The unsigned credential-exchange call this rule exists for: no
                // credential scope, so the target is all that names the service.
                case: "unsigned, targeted at it",
                scope: None,
                target: Some("AWSCognitoIdentityService.GetId"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, targeted at it, on the emulator's own host",
                scope: None,
                target: Some("AWSCognitoIdentityService.GetCredentialsForIdentity"),
                host: Some("127.0.0.1:9297"),
                uri: ROOT,
                expected: true,
            },
            Row {
                // The target tier outranks the `Host` tier in the framework's own
                // resolver, so the peer that host names resolves the same
                // target-derived key and does not claim this request either.
                case: "unsigned, targeted at it, on another service's endpoint host",
                scope: None,
                target: Some("AWSCognitoIdentityService.GetId"),
                host: Some("lambda.us-east-1.amazonaws.com"),
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, lower-cased target prefix",
                scope: None,
                target: Some("awscognitoidentityservice.GetId"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, targeted at the separate Cognito user-pools service",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: None,
                uri: ROOT,
                expected: false,
            },
            Row {
                // The credential scope outranks the target, so a request signed for
                // another service does not reach this one.
                case: "signed for another service, targeted at it",
                scope: Some("lambda"),
                target: Some("AWSCognitoIdentityService.GetId"),
                host: None,
                uri: ROOT,
                expected: false,
            },
            Row {
                // A request that names this service out of band while addressing
                // another model's REST path contradicts itself: it reaches nobody.
                case: "unsigned, targeted at it, on a REST path",
                scope: None,
                target: Some("AWSCognitoIdentityService.GetId"),
                host: None,
                uri: "/identitypools/pool-1",
                expected: false,
            },
            Row {
                case: "unsigned, addressed by its endpoint host",
                scope: None,
                target: None,
                host: Some("cognito-identity.us-east-1.amazonaws.com"),
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "no routing signal at all",
                scope: None,
                target: None,
                host: None,
                uri: ROOT,
                expected: false,
            },
        ];

        for row in rows {
            let mut builder = super::http::Request::builder()
                .method("POST")
                .uri(row.uri);
            if let Some(scope) = row.scope {
                builder = builder.header("authorization", authorization(scope));
            }
            if let Some(target) = row.target {
                builder = builder.header("x-amz-target", target);
            }
            if let Some(host) = row.host {
                builder = builder.header("host", host);
            }
            let parts = builder
                .body(())
                .expect("a well-formed test request")
                .into_parts()
                .0;
            let actual = routes_to_signing_name(&parts, SIGNING_NAME)
                || unsigned_target_names_shape(&parts, SHAPE_ID);
            assert_eq!(
                actual, row.expected,
                "case {:?}: expected this service to claim it == {}, got {actual}",
                row.case, row.expected
            );
        }
    }
}
