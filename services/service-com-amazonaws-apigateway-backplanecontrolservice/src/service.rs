// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-apigateway-backplanecontrolservice` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name. EDITED HERE: it
//!   claims the control plane as the framework's own rule does, and beside it a request
//!   addressed to a DEPLOYED STAGE's invoke URL — the `execute-api` data plane, which no
//!   route key names (see [`invoke`]).
//! - `override_operation` — which operation serves a request. EDITED HERE: the key and
//!   the handler stay the generated table's, but each operation is served through
//!   [`StatedPerOperation`], which states a refused request the way this service states
//!   it and, ahead of that, answers a query value that cannot be converted to its
//!   member's modelled type through [`query_coercion`] — a deserialization failure, not
//!   a constraint violation. A request addressed to a stage invoke URL is served by
//!   [`StageDataPlane`] instead, which the model's routing key cannot
//!   name.
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
//! ## This service's edited seam: how a broken constraint is stated
//!
//! API Gateway names the offending member of a refused request by its path from the
//! OPERATION's input shape — `updateDocumentationVersionInput.patchOperations.1.member.op`,
//! `putGatewayResponseInput.statusCode` — so the sentence identifies the operation as
//! well as the field. The framework refuses every modelled constraint before a handler
//! runs and words that refusal through [`Service::override_refusal`], which is handed
//! the violations ALONE — no operation, no request — so it cannot name the input shape
//! the path hangs off, and the root is per operation (the recordings show the same
//! member path under three different roots). [`StatedPerOperation`] is where that root
//! comes from: each modelled operation is served wrapped in it, the wrapper collects
//! the framework's own violations against THAT operation's input schema
//! ([`violations_of`]) and states them through [`validation`], which also owns the two
//! renderings this service does not share with the framework (an unanchored
//! `@pattern`, an enum value set in the service's declared order).
//!
//! ## This service's other edited seam: the `execute-api` data plane
//!
//! API Gateway publishes TWO kinds of hostname. The CONTROL plane is
//! `apigateway.{region}.amazonaws.com`, whose first label names the service, and that is
//! what the framework's route key resolves. The DATA plane is the per-api invoke endpoint
//! `{rest-api-id}.execute-api.{region}.amazonaws.com`, whose first label is the api's own
//! id: it names no service, so the front door can resolve no target service from such a
//! request and answers `404 UnknownService` for a URL this service told the caller to use.
//! A client on the machine this emulator runs on resolves that host nowhere at all, so it
//! points at the emulator's own endpoint and names the api in its PATH — behind the
//! reserved `/_aws/execute-api/{api-id}` prefix, as its first segment, or by the STAGE
//! segment alone; [`invoke`] resolves every one of those addressings.
//!
//! Two seams carry it, and nothing else changes:
//!
//! - [`Service::can_handle`] also claims a request addressed to a deployed stage of an api
//!   this service HOLDS ([`invoke::claims_parts`]), which is what routes it here at all;
//! - [`Service::override_operation`] serves EVERY invoke, as an [`ErasedOperation`]
//!   ([`StageDataPlane`]) so the handler is handed the in-process transport an `AWS`
//!   integration's call to another emulated service is made over. An invoke names no
//!   modelled operation by construction (`/{stage}/{path}` is not in the model), so that
//!   operation is named only for a request the data plane claims, and every modelled
//!   request keeps resolving through the generated table.
//!
//! Why that operation declares NO auth scheme, which is what the router reads as "served
//! unsigned": the router verifies a named
//! operation's SigV4 credential against the signing name of the service that CLAIMED the
//! request, and this service's is its control plane's (`apigateway`). An invoke of a
//! deployed stage is signed under the data plane's own signing name instead — the
//! `execute-api` service AWS publishes the invoke endpoint and the `execute-api:Invoke`
//! action under — so a correctly signed invoke never matches that name and would be refused
//! before the stage is reached, whatever the api's records say. The front door cannot be
//! pointed at a second signing name from here, so the invoke reaches the stage unchecked and
//! the stage asks the emulator's OWN verifier about the data plane's signing name instead
//! ([`invoke::sigv4_authentication`]) — one reading of SigV4 in this process, asked by
//! whichever plane the request addressed. What a method's `authorizationType` requires of the
//! request is unchanged — `AWS_IAM` admits only a request that CARRIES a SigV4 credential
//! ([`invoke::carries_sigv4_credential`]) and whose credential AUTHENTICATES: an access key
//! id this emulator holds no credential for, and one signed with the wrong secret, are both
//! refused `403` with no integration performed.
//! WHAT that credential may then do is a second question, answered from the account's own
//! records ([`invoke::execute_api_authorization`]): a verified credential is not a pass, so
//! such a method is served only where the identity-based policies in force on the principal
//! it speaks for, or the api's own document, allow the invoke.
//!
//! What the data plane resolves, and what it answers for an integration it cannot perform,
//! is [`invoke`]'s own documentation; the api's own resource policy, which every invoke is
//! decided against, is [`resource_policy`]'s.

pub mod custom_domain_socket;
pub mod invoke;
pub mod mapping_template;
pub mod query_coercion;
pub mod rendered_templates;
pub mod resource_policy;
mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints::ViolationKind;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, Service, Violation,
};
use std::sync::Arc;

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable owner
/// of this rule, and it is CALLED here rather than re-implemented: the request's route key
/// must match this service's signing name AND its first URI path segment must be one the
/// generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment equality. It
/// is the same rule `service_common::claims_request` applies for a REST service; this
/// crate reaches the framework helper directly, as
/// `service-com-amazonaws-transcribe-transcribe`, `-appconfig-amazonappconfig` and
/// `-ivs-*` do.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-apigateway-backplanecontrolservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the `execute-api` data plane ([`invoke`]) is CLAIMED in
    /// [`Service::can_handle`], which is handed no operation of its own; it must read the
    /// one store the control-plane operations write, not a second copy.
    engine: Arc<Eng>,
    /// The `execute-api` data plane, as an operation, so the router hands it an
    /// [`OperationContext`] — the in-process transport an `AWS` integration's call to
    /// another service is made over.
    data_plane: StageDataPlane<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request names the violated member by its path
    /// from THAT operation's input shape. Keyed by the routing key the protocol resolves
    /// — the key the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store.
    stated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
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
        let mut stated = OperationRoutingTable::new();
        // EVERY modelled operation (124 of them, one per file in `src/ops/`), because the
        // rule is the service's and not one operation's: an operation whose input
        // declares no constraint simply never has a violation to state, which the wrapper
        // reads off that operation's own input schema. An operation added later is added
        // here too, or it answers a refusal in the framework's wording rather than this
        // service's.
        wrap_operations!(
            stated,
            engine,
            create_api_key::CreateApiKey,
            create_authorizer::CreateAuthorizer,
            create_base_path_mapping::CreateBasePathMapping,
            create_deployment::CreateDeployment,
            create_documentation_part::CreateDocumentationPart,
            create_documentation_version::CreateDocumentationVersion,
            create_domain_name::CreateDomainName,
            create_domain_name_access_association::CreateDomainNameAccessAssociation,
            create_model::CreateModel,
            create_request_validator::CreateRequestValidator,
            create_resource::CreateResource,
            create_rest_api::CreateRestApi,
            create_stage::CreateStage,
            create_usage_plan::CreateUsagePlan,
            create_usage_plan_key::CreateUsagePlanKey,
            create_vpc_link::CreateVpcLink,
            delete_api_key::DeleteApiKey,
            delete_authorizer::DeleteAuthorizer,
            delete_base_path_mapping::DeleteBasePathMapping,
            delete_client_certificate::DeleteClientCertificate,
            delete_deployment::DeleteDeployment,
            delete_documentation_part::DeleteDocumentationPart,
            delete_documentation_version::DeleteDocumentationVersion,
            delete_domain_name::DeleteDomainName,
            delete_domain_name_access_association::DeleteDomainNameAccessAssociation,
            delete_gateway_response::DeleteGatewayResponse,
            delete_integration::DeleteIntegration,
            delete_integration_response::DeleteIntegrationResponse,
            delete_method::DeleteMethod,
            delete_method_response::DeleteMethodResponse,
            delete_model::DeleteModel,
            delete_request_validator::DeleteRequestValidator,
            delete_resource::DeleteResource,
            delete_rest_api::DeleteRestApi,
            delete_stage::DeleteStage,
            delete_usage_plan::DeleteUsagePlan,
            delete_usage_plan_key::DeleteUsagePlanKey,
            delete_vpc_link::DeleteVpcLink,
            flush_stage_authorizers_cache::FlushStageAuthorizersCache,
            flush_stage_cache::FlushStageCache,
            generate_client_certificate::GenerateClientCertificate,
            get_account::GetAccount,
            get_api_key::GetApiKey,
            get_api_keys::GetApiKeys,
            get_authorizer::GetAuthorizer,
            get_authorizers::GetAuthorizers,
            get_base_path_mapping::GetBasePathMapping,
            get_base_path_mappings::GetBasePathMappings,
            get_client_certificate::GetClientCertificate,
            get_client_certificates::GetClientCertificates,
            get_deployment::GetDeployment,
            get_deployments::GetDeployments,
            get_documentation_part::GetDocumentationPart,
            get_documentation_parts::GetDocumentationParts,
            get_documentation_version::GetDocumentationVersion,
            get_documentation_versions::GetDocumentationVersions,
            get_domain_name::GetDomainName,
            get_domain_name_access_associations::GetDomainNameAccessAssociations,
            get_domain_names::GetDomainNames,
            get_export::GetExport,
            get_gateway_response::GetGatewayResponse,
            get_gateway_responses::GetGatewayResponses,
            get_integration::GetIntegration,
            get_integration_response::GetIntegrationResponse,
            get_method::GetMethod,
            get_method_response::GetMethodResponse,
            get_model::GetModel,
            get_model_template::GetModelTemplate,
            get_models::GetModels,
            get_request_validator::GetRequestValidator,
            get_request_validators::GetRequestValidators,
            get_resource::GetResource,
            get_resources::GetResources,
            get_rest_api::GetRestApi,
            get_rest_apis::GetRestApis,
            get_sdk::GetSdk,
            get_sdk_type::GetSdkType,
            get_sdk_types::GetSdkTypes,
            get_stage::GetStage,
            get_stages::GetStages,
            get_tags::GetTags,
            get_usage::GetUsage,
            get_usage_plan::GetUsagePlan,
            get_usage_plan_key::GetUsagePlanKey,
            get_usage_plan_keys::GetUsagePlanKeys,
            get_usage_plans::GetUsagePlans,
            get_vpc_link::GetVpcLink,
            get_vpc_links::GetVpcLinks,
            import_api_keys::ImportApiKeys,
            import_documentation_parts::ImportDocumentationParts,
            import_rest_api::ImportRestApi,
            put_gateway_response::PutGatewayResponse,
            put_integration::PutIntegration,
            put_integration_response::PutIntegrationResponse,
            put_method::PutMethod,
            put_method_response::PutMethodResponse,
            put_rest_api::PutRestApi,
            reject_domain_name_access_association::RejectDomainNameAccessAssociation,
            tag_resource::TagResource,
            test_invoke_authorizer::TestInvokeAuthorizer,
            test_invoke_method::TestInvokeMethod,
            untag_resource::UntagResource,
            update_account::UpdateAccount,
            update_api_key::UpdateApiKey,
            update_authorizer::UpdateAuthorizer,
            update_base_path_mapping::UpdateBasePathMapping,
            update_client_certificate::UpdateClientCertificate,
            update_deployment::UpdateDeployment,
            update_documentation_part::UpdateDocumentationPart,
            update_documentation_version::UpdateDocumentationVersion,
            update_domain_name::UpdateDomainName,
            update_gateway_response::UpdateGatewayResponse,
            update_integration::UpdateIntegration,
            update_integration_response::UpdateIntegrationResponse,
            update_method::UpdateMethod,
            update_method_response::UpdateMethodResponse,
            update_model::UpdateModel,
            update_request_validator::UpdateRequestValidator,
            update_resource::UpdateResource,
            update_rest_api::UpdateRestApi,
            update_stage::UpdateStage,
            update_usage::UpdateUsage,
            update_usage_plan::UpdateUsagePlan,
            update_vpc_link::UpdateVpcLink,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            data_plane: StageDataPlane {
                engine: Arc::clone(&engine),
            },
            engine,
            stated,
        }
    }
}

/// The `execute-api` data plane as an [`ErasedOperation`], so the router hands it the
/// [`OperationContext`] carrying the in-process transport an `AWS` integration's call to
/// another emulated service goes over ([`invoke::aws_integration`]).
///
/// It holds the SAME shared engine the generated entry threads into every operation, so the
/// data plane reads the one store the control-plane operations write.
struct StageDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for StageDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if invoke::claims_request(&*self.engine, req) {
                return invoke::serve(&*self.engine, req, ctx).await;
            }
            // Unreachable: `override_operation` names this only for a request the data
            // plane claims.
            svc.definition().routing_error_response(req)
        })
    }

    /// NO auth scheme, which is what the router reads as "served unsigned".
    ///
    /// An invoke of a deployed stage is not a modelled operation of this service, and the
    /// credential it carries is not this service's to verify: the router verifies a named
    /// operation's SigV4 credential against the signing name of the service that CLAIMED
    /// the request, and this service's is its control plane's (`apigateway`), while an
    /// invoke is signed under the data plane's own (`execute-api`, the service AWS
    /// publishes the invoke endpoint and the `execute-api:Invoke` action under). Declaring
    /// no scheme is what lets the stage answer a correctly signed invoke at all, and it
    /// leaves the modelled operations untouched: the router reads the schemes off the
    /// MODELLED operation first, and every control-plane request resolves one.
    ///
    /// What a method's `authorizationType` requires of the request is unchanged — `AWS_IAM`
    /// admits only a request that CARRIES a SigV4 credential
    /// ([`invoke::carries_sigv4_credential`]) and whose credential the emulator's own
    /// verifier authenticates against the data plane's signing name
    /// ([`invoke::sigv4_authentication`]), so declaring no scheme here moves that check to
    /// the stage rather than dropping it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

/// The answer as this service renders it: an `IntegrationResponse` in it reports a content
/// type whose mapping template is empty with a null template, which the schema-driven
/// serialization of a `MapOfStringToString` cannot write
/// ([`rendered_templates::with_empty_templates_as_null`]).
///
/// Anything else is returned untouched — status, headers and every byte of the body
/// exactly as the modelled walk wrote them: a failure, a streamed body, a body that is not
/// a rendering carrying such a template.
fn with_rendered_templates(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() >= 400 {
        return resp;
    }
    let rewritten = {
        // A modelled response body is built in memory by the serializer, so the bytes are
        // readable here; a streamed body is not one and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match rendered_templates::with_empty_templates_as_null(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected, and the length is
    // REMOVED rather than left describing the body before the rewrite if a decimal count
    // is somehow not a valid header value.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        if resp
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
        {
            resp.headers_mut().remove("content-length");
        }
    }
    *resp.body_mut() = aws_smithy_types::body::SdkBody::from(rewritten);
    resp
}

/// One modelled operation, served so that a refused request is stated the way this
/// service states it: every validation path rooted at THIS operation's input shape.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to
/// the generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle
/// -> serialize, telemetry and every modelled error. The violations come from the
/// framework's own machinery ([`violations_of`]), so this cannot refuse a request the
/// framework's own walk would accept, nor accept one it would refuse. The cost is that
/// a request this wrapper does not refuse is read twice, once here and once in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// Every modelled constraint `req` breaks, for the operation `O`, wherever the member it
/// names travels: the violation set the framework's own walk will find for this request.
///
/// Absence counts: a required member the request omits is found while READING the request
/// rather than while walking the value it built, but it is the same refusal to a caller —
/// the framework's own `refusal_of` folds it into exactly the violation this returns.
/// Any OTHER codec failure (a body that is not parseable at all, a member of the wrong
/// type) is not a constraint violation, so it is left to the framework's own walk, which
/// refuses it in the wording this service does not change.
///
/// This is the step `service_common::front_door::body_violations` performs before
/// NARROWING its result to body-bound members: that narrowing is the front door's rule,
/// not this service's, which roots EVERY violation's path at the operation's input shape
/// whether the member travels in the body or in the URI (`putIntegrationResponseInput.
/// statusCode` is a `@httpLabel`). One shared function is to serve both when the
/// per-operation wrapper itself is consolidated — the `TO CONSOLIDATE` note in
/// `services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/service.rs`
/// tracks that — so the collection step stays private here rather than growing a second
/// public surface on the shared crate before that lands.
fn violations_of<O: Operation>(
    protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    match read {
        Ok(input) => aws_smithy_schema_server::constraints::validate(&input, O::INPUT_SCHEMA),
        // `member_name` is already a violation path in the spelling the constraint walk
        // uses (`aws_smithy_schema_server::missing_member` applies `path::member`, and
        // each enclosing member prepends its own segment), so it places the same way a
        // walked violation does.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }],
        Err(_) => Vec::new(),
    }
}

impl<O> ErasedOperation for StatedPerOperation<O>
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
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
            // await below would make this future non-`Send`.
            //
            // A query value that cannot be CONVERTED to the modelled type of its member
            // is refused FIRST, because deserialization precedes constraint validation:
            // such a value never becomes a value at all, so there are no violations to
            // state and no handler to run. The framework would otherwise report that
            // conversion failure as a constraint violation (`ValidationException`,
            // lowercase `message`), which is a different error class, a different
            // remediation for the caller and a different SDK error type — see
            // [`query_coercion`] for the whole argument and for the refusal this answers
            // instead. The operation's own `O::INPUT_SCHEMA` is what says which of its
            // members are convertible scalars bound to the query string.
            if let Some(message) = query_coercion::refusal_for(O::INPUT_SCHEMA, req) {
                return query_coercion::serialization_exception(&message);
            }
            let refused = violations_of::<O>(svc.definition().protocol(), req);
            if let Some(stated) = validation::refusal(O::NAME, O::INPUT_SCHEMA, &refused) {
                return svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(refused), Some(stated), req);
            }
            with_rendered_templates(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
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
        // This service signs as `apigateway` — a signing name it SHARES with Amazon API
        // Gateway V2 (`services/service-com-amazonaws-apigatewayv2-apigatewayv2`, `sdkId`
        // `ApiGatewayV2`, the same `apigateway` endpoint prefix and signing name), a
        // DIFFERENT service that AWS separates by URI path space: every V2 operation is
        // bound under `/v2/…`, while this control plane's are bound under `/restapis`,
        // `/apikeys`, `/account`, `/domainnames`, `/usageplans`, `/vpclinks`,
        // `/clientcertificates`, `/sdktypes`, `/tags`, … The router resolves a request's
        // routing key from the SigV4 credential scope, so both services resolve to
        // `apigateway`; claiming on the signing name ALONE makes both claim every such
        // request, which the router answers `500 AmbiguousService` — a code no AWS model
        // carries — leaving BOTH services unreachable.
        //
        // So the claim adds the framework's path conjunct over this service's own
        // model-derived path space — see [`can_handle_parts`]. `/v2` is in neither of
        // this model's segments, so the V2 endpoint alone claims its traffic and each
        // service answers only its own.
        if can_handle_parts(parts) {
            return true;
        }
        // And the DATA plane: a request addressed to a DEPLOYED STAGE of an api THIS
        // service holds a record for. Such a request names no modelled operation and
        // carries no credential scope naming this service, so the front door could resolve
        // no target service for it and answered `404 UnknownService` — for a URL this
        // service told the caller to use (`crate::service::invoke`).
        //
        // The claim is narrowed to an api this service HOLDS, read from the one shared
        // store, rather than to a host or a path shape alone, because the same shapes are
        // the invoke endpoint of an HTTP or WebSocket api of the V2 service too: each
        // service claiming only the ids it minted keeps one claimant per request.
        // `@connections` is excluded for the same reason — it is ApiGatewayManagementApi's
        // modelled path space on this host.
        invoke::claims_parts(&*self.engine, parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route for a modelled request: the key is the one the generated table's
        // own protocol resolves, and the operation it names is the one served — wrapped in
        // `StatedPerOperation` so a refused request names the violated member by its
        // path from that operation's input shape, the way this service names it, and so
        // that a query value it cannot convert is answered by `query_coercion` ahead of
        // that.
        if let Some(stated) = self
            .entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.stated.get(key))
        {
            return Some(stated);
        }
        // An invoke of a deployed stage names no modelled operation by construction
        // (`/{stage}/{path}` is not in the model), so it is served here, by the one
        // operation that is not the model's: `StageDataPlane`, which the router hands the
        // in-process transport an `AWS` integration's call to another service is made over.
        // It declares no auth scheme, so the router serves the invoke whatever credential
        // it carries — see that operation's own `auth_schemes`.
        invoke::claims_request(&*self.engine, req)
            .then_some(&self.data_plane as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. It is handed the violations ALONE — no operation — so it cannot
        // root a path at the operation's input shape, which is what this service's
        // wording needs; `StatedPerOperation` answers those refusals before this walk
        // is reached. What is left here is a refusal that reached no modelled operation
        // (a request this service could not authenticate, HTTP 403), whose wording this
        // service does not change: returning `None` keeps the framework's.
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. An invoke of a deployed stage is named by
        // `override_operation` above, so what is left here is a request this service
        // serves nothing for: it keeps the protocol-correct default for this service's
        // envelope (query -> 400 InvalidAction text/xml, awsJson -> 404, rpcv2Cbor -> 404,
        // rest -> 501) by returning `None`.
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

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// behind [`StatedPerOperation`], under the same key the generated routing table is
    /// keyed by.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an
    /// operation added to the model and not to `wrap_operations!` fails here instead of
    /// silently answering a refusal in the framework's wording — and a key spelled
    /// differently from the generated table's fails here instead of silently never being
    /// reached by `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_with_this_services_refusal_wording() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            operations.len() > 100,
            "case the model's surface: expected this service's operations, got {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is \
                 wrong"
            );
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every modelled operation is served behind \
                 StatedPerOperation; this one is missing from wrap_operations!, so it \
                 answers a refusal in the framework's wording instead of this service's"
            );
        }
    }
}
