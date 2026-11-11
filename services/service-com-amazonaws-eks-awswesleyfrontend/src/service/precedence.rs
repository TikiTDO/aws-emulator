// SPDX-License-Identifier: CC0-1.0

//! What this service answers BEFORE a refusal of the request body: the pass-role
//! authorization of a role the request hands Amazon EKS, the capability type a
//! `CreateCapability` asks for, and the resolution of the cluster the request is
//! addressed to.
//!
//! Every handler in `crate::ops` already keeps that order — the pass-role question
//! is decided from the value alone and needs no state, and the `@httpLabel` cluster
//! names the resource being acted on, so both are answered before any member of the
//! body is judged. The framework's own walk does not: it validates the whole input
//! against the operation's generated schema and refuses a modelled constraint
//! BEFORE `handle` runs (`vendor/smithy-server-runtime/src/operation.rs`), so for a
//! request that breaks one of those constraints the body refusal reached the caller
//! first and the earlier answers were never given. A client keying on the
//! missing cluster to create it and retry, or on the pass-role denial to fix its
//! policy, saw a parameter complaint instead.
//!
//! This layer restores the order for exactly those requests. It runs in the one seam
//! a service has BEFORE dispatch (`smithy_server_runtime::Service::override_operation`),
//! the same seam the tag and resolve-conflicts contracts beside it use, and it claims
//! a request only when ALL of:
//!
//! * the operation it was sent to is one of the operations below — each takes a role
//!   it asks Amazon EKS to pass, or addresses a cluster by `@httpLabel` and answers an
//!   unknown one with [`crate::state::cluster_not_found`], or both;
//! * the framework's validating walk would refuse the body. The walk is run HERE,
//!   through the same `aws_smithy_schema_server::constraints::validate` and the same
//!   generated input schema the dispatch sequence runs, so this layer cannot disagree
//!   with the walk that would otherwise follow it. A request whose body the walk
//!   accepts is NOT claimed: its handler already answers both questions in the right
//!   order, and nothing here may change what it reports;
//! * one of the earlier answers actually fires — the value names no role this
//!   caller can pass, the request asks for a capability type this API does not define,
//!   or the store holds no such cluster — or the request is a `CreateCluster` whose
//!   refused parameter this service states its own sentence for (see
//!   [`super::parameter_contract`]), whose refusal also carries the `clusterName` the
//!   request asked for. Otherwise the request falls through to the modelled
//!   operation, which refuses the body exactly as before.
//!
//! The answers are the service's own modelled shapes, rendered through the same
//! protocol call the framework's error tail makes: `AccessDeniedException` (403) for
//! the pass-role denial, `InvalidParameterException` (400) for the capability type
//! this API does not define, and `ResourceNotFoundException` (404) for the missing
//! cluster. Each claimed operation answers those same shapes and statuses from its own
//! handler, so a refusal worded here and one worded there are indistinguishable on
//! the wire.
//!
//! WHY the capability type is answered here rather than by the handler, and why it
//! comes before the cluster: see [`crate::service::off_enum_member_contract`], which
//! states the rule and builds the refusal. `CreateCapability`'s `type` targets an
//! enum, so the walk refuses an undeclared value before `handle` runs and the handler
//! never sees it — and the value names no capability type at all, so no cluster could
//! make the request answerable.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{validate, Violation};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, OperationContext,
    OperationError, Refused, ServerHttpProtocol, Service,
};

use crate::ops::types::{
    AccessDeniedException, AssociateIdentityProviderConfigInput, CreateAccessEntryInput,
    CreateAddonInput, CreateCapabilityInput, CreateClusterInput, CreateFargateProfileInput,
    CreateNodegroupInput, CreatePodIdentityAssociationInput, InvalidParameterException,
    ListAddonsInput, ListCapabilitiesInput, ListNodegroupsInput, ListUpdatesInput,
    ResourceNotFoundException, UpdateClusterConfigInput,
    ASSOCIATE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA, CREATE_ACCESS_ENTRY_INPUT_SCHEMA,
    CREATE_ADDON_INPUT_SCHEMA, CREATE_CAPABILITY_INPUT_SCHEMA, CREATE_CLUSTER_INPUT_SCHEMA,
    CREATE_FARGATE_PROFILE_INPUT_SCHEMA, CREATE_NODEGROUP_INPUT_SCHEMA,
    CREATE_POD_IDENTITY_ASSOCIATION_INPUT_SCHEMA, LIST_ADDONS_INPUT_SCHEMA,
    LIST_CAPABILITIES_INPUT_SCHEMA, LIST_NODEGROUPS_INPUT_SCHEMA, LIST_UPDATES_INPUT_SCHEMA,
    SCHEMA_ACCESS_DENIED_EXCEPTION, SCHEMA_INVALID_PARAMETER_EXCEPTION,
    SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION, UPDATE_CLUSTER_CONFIG_INPUT_SCHEMA,
};

use super::off_enum_member_contract::undefined_capability_type;

/// The error code a request that may not pass the role it names is refused with.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
/// The error code a request addressed to a cluster that does not exist is refused
/// with.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
/// The error code a request naming a value this API does not define is refused with.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// What the operation a claimed request was sent to answers before it judges the rest
/// of the body: the roles the request asks Amazon EKS to pass, in the order its
/// handler judges them, a member whose value this API does not define, and the cluster
/// it is addressed to.
///
/// All of them are read from the request the dispatch sequence would have handed the
/// handler, so this layer reads exactly the values the handler would.
#[derive(Debug, Default)]
pub struct Addressed {
    passed_roles: Vec<String>,
    /// The refusal a member whose value this API does not define earns, when the
    /// request carries one: a question about the request alone, so it is answered
    /// before the cluster is resolved (see
    /// [`crate::service::off_enum_member_contract`]).
    undefined_value: Option<InvalidParameterException>,
    cluster: Option<String>,
    /// The refusal this service states for the parameter the walk refused, when it
    /// states one for that rule (see [`super::parameter_contract`]). It carries the
    /// members the refusal payload reports beside the sentence, which the per-service
    /// message seam cannot reach; it is answered LAST, because it is a complaint about
    /// the body and everything above is about the request's authorization or the
    /// resources it names.
    parameter: Option<InvalidParameterException>,
}

impl Addressed {
    /// The same request, carrying the refusal its undefined value earns.
    fn with_undefined_value(mut self, refusal: Option<InvalidParameterException>) -> Self {
        self.undefined_value = refusal;
        self
    }

    /// The same request, carrying this service's own refusal of the parameter the
    /// walk refused.
    fn with_parameter_refusal(mut self, refusal: Option<InvalidParameterException>) -> Self {
        self.parameter = refusal;
        self
    }
}

/// One answer this layer gives, as the shape and status the service models for it.
#[derive(Debug)]
enum Answer {
    /// The value names no role this caller may have Amazon EKS assume.
    PassRoleDenied(AccessDeniedException),
    /// A member of the request carries a value this API does not define, so it names
    /// nothing for any cluster to hold.
    ValueNotDefined(InvalidParameterException),
    /// The cluster the request is addressed to does not exist.
    ClusterMissing(ResourceNotFoundException),
    /// A parameter of the request broke a rule this service states its own refusal
    /// for (see [`super::parameter_contract`]).
    ParameterRefused(InvalidParameterException),
}

impl OperationError for Answer {
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::PassRoleDenied(exception) => exception,
            Self::ValueNotDefined(exception) => exception,
            Self::ClusterMissing(exception) => exception,
            Self::ParameterRefused(exception) => exception,
        }
    }

    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::PassRoleDenied(_) => &SCHEMA_ACCESS_DENIED_EXCEPTION,
            Self::ValueNotDefined(_) => &SCHEMA_INVALID_PARAMETER_EXCEPTION,
            Self::ClusterMissing(_) => &SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
            Self::ParameterRefused(_) => &SCHEMA_INVALID_PARAMETER_EXCEPTION,
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::PassRoleDenied(_) => ACCESS_DENIED_EXCEPTION,
            Self::ValueNotDefined(_) => INVALID_PARAMETER_EXCEPTION,
            Self::ClusterMissing(_) => RESOURCE_NOT_FOUND_EXCEPTION,
            Self::ParameterRefused(_) => INVALID_PARAMETER_EXCEPTION,
        }
    }

    /// The statuses the model pins on the three shapes: `@httpError 403` on
    /// `AccessDeniedException`, `@httpError 400` on `InvalidParameterException` and
    /// `@httpError 404` on `ResourceNotFoundException`
    /// (`smithy_query(query="operation",operation="CreateCapability",…)` reports all
    /// three).
    fn http_status(&self) -> u16 {
        match self {
            Self::PassRoleDenied(_) => 403,
            Self::ValueNotDefined(_) => 400,
            Self::ClusterMissing(_) => 404,
            Self::ParameterRefused(_) => 400,
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        // This service speaks restJson1, which carries no `awsQueryError` code at
        // all; both shapes are reported by their own name.
        None
    }
}

impl std::fmt::Display for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PassRoleDenied(exception) => write!(f, "{exception}"),
            Self::ValueNotDefined(exception) => write!(f, "{exception}"),
            Self::ClusterMissing(exception) => write!(f, "{exception}"),
            Self::ParameterRefused(exception) => write!(f, "{exception}"),
        }
    }
}

impl std::error::Error for Answer {}

impl Answer {
    /// This answer as a response in the service's own protocol envelope, serialized
    /// through the same protocol call the framework's error tail makes.
    ///
    /// FAIL CLOSED: a request is claimed only when this answer fires, so it must
    /// never fall through to a handler that would report something else. Where the
    /// modelled envelope cannot be rendered at all, the same code and message are
    /// answered through the service's framework-refusal envelope instead.
    fn response(self, definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> RtResponse {
        let code = self.shape_name().to_string();
        let message = match &self {
            Self::PassRoleDenied(exception) => exception.message.clone(),
            Self::ValueNotDefined(exception) => exception.message.clone(),
            Self::ClusterMissing(exception) => exception.message.clone(),
            Self::ParameterRefused(exception) => exception.message.clone(),
        }
        .unwrap_or_default();
        definition
            .protocol()
            .serialize_error(
                self.as_serializable(),
                self.variant_schema(),
                self.shape_name(),
                self.http_status(),
            )
            .ok()
            .unwrap_or_else(|| {
                definition.refusal_response(
                    ProtocolError::Invalid(Vec::new()),
                    Some(Refused { code, message }),
                    req,
                )
            })
    }
}

/// The violations the framework's own validating walk would report for this input,
/// which is the only case this layer has a question about: they are what would
/// otherwise be answered BEFORE the checks the operation performs first.
fn boundary_violations(input: &dyn SerializableStruct, schema: &'static Schema) -> Vec<Violation> {
    validate(input, schema)
}

/// Read one claimed operation's request and name what its handler answers before it
/// judges the body, or `None` when the walk accepts the body (so there is no
/// question) or the body cannot be read at all.
///
/// The walk's own violations are handed to `members` beside the input, so an arm that
/// words the refusal of a parameter reads exactly the violations the dispatch sequence
/// would have reported.
fn addressed_with<T: DeserializableStruct + SerializableStruct>(
    protocol: &dyn ServerHttpProtocol,
    schema: &'static Schema,
    req: &RtRequest,
    members: impl FnOnce(&T, &[Violation]) -> Addressed,
) -> Option<Addressed> {
    // Read through the ONE reader every pre-dispatch layer of this service shares
    // (`crate::service::read_input`), so this layer and the authorization gate ahead of
    // it cannot read one request differently.
    let input: T = super::read_input(protocol, schema, req)?;
    let violations = boundary_violations(&input, schema);
    if violations.is_empty() {
        return None;
    }
    Some(members(&input, &violations))
}

/// One role the request hands Amazon EKS, and no cluster to resolve.
fn passes_roles(roles: Vec<String>) -> Addressed {
    Addressed {
        passed_roles: roles,
        ..Default::default()
    }
}

/// The cluster the request is addressed to, and no role to authorize.
fn addresses_cluster(cluster: &str) -> Addressed {
    Addressed {
        cluster: Some(cluster.to_string()),
        ..Default::default()
    }
}

/// Both: a role to authorize first, then the cluster the request is addressed to —
/// the order every handler that does both keeps.
fn passes_roles_and_addresses_cluster(roles: Vec<String>, cluster: &str) -> Addressed {
    Addressed {
        passed_roles: roles,
        cluster: Some(cluster.to_string()),
        ..Default::default()
    }
}

/// What the operation `key` names answers before it judges the body, for this
/// request.
///
/// The table is this layer's whole scope. An operation is in it because its handler
/// makes one of those two checks first and its body carries a modelled constraint
/// the walk can refuse; `None` for every other operation, which keeps the generated
/// routing table as its only source. On this service's restJson1 protocol a routing
/// key is the operation's own shape name.
fn addressed(
    key: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<Addressed> {
    match key {
        // `roleArn` is the cluster's own role and `computeConfig.nodeRoleArn` the one
        // its Auto Mode nodes take; the handler judges them in that order. The
        // operation creates the cluster, so there is no cluster to resolve — and the
        // parameter the walk refused is worded here, because the refusal carries the
        // `clusterName` the request asked for beside the sentence.
        "CreateCluster" => addressed_with(
            protocol,
            CREATE_CLUSTER_INPUT_SCHEMA,
            req,
            |input: &CreateClusterInput, violations| {
                passes_roles(
                    [
                        Some(input.role_arn.clone()),
                        input
                            .compute_config
                            .as_ref()
                            .and_then(|compute| compute.node_role_arn.clone()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect(),
                )
                .with_parameter_refusal(super::parameter_contract::create_cluster_refusal(
                    violations,
                    &input.name,
                ))
            },
        ),
        "CreateNodegroup" => addressed_with(
            protocol,
            CREATE_NODEGROUP_INPUT_SCHEMA,
            req,
            |input: &CreateNodegroupInput, _| {
                passes_roles_and_addresses_cluster(
                    vec![input.node_role.clone()],
                    &input.cluster_name,
                )
            },
        ),
        // `type` says WHICH capability to create, and a value outside the modelled
        // `CapabilityType` names none at all, so it is answered after the pass-role
        // authorization and before the cluster — the one question of this body that
        // needs neither the cluster nor another service to decide.
        "CreateCapability" => addressed_with(
            protocol,
            CREATE_CAPABILITY_INPUT_SCHEMA,
            req,
            |input: &CreateCapabilityInput, _| {
                passes_roles_and_addresses_cluster(
                    vec![input.role_arn.clone()],
                    &input.cluster_name,
                )
                .with_undefined_value(undefined_capability_type(&input.r#type))
            },
        ),
        "CreateFargateProfile" => addressed_with(
            protocol,
            CREATE_FARGATE_PROFILE_INPUT_SCHEMA,
            req,
            |input: &CreateFargateProfileInput, _| {
                passes_roles_and_addresses_cluster(
                    vec![input.pod_execution_role_arn.clone()],
                    &input.cluster_name,
                )
            },
        ),
        "UpdateClusterConfig" => addressed_with(
            protocol,
            UPDATE_CLUSTER_CONFIG_INPUT_SCHEMA,
            req,
            |input: &UpdateClusterConfigInput, _| addresses_cluster(&input.name),
        ),
        "CreateAddon" => addressed_with(
            protocol,
            CREATE_ADDON_INPUT_SCHEMA,
            req,
            |input: &CreateAddonInput, _| addresses_cluster(&input.cluster_name),
        ),
        "CreateAccessEntry" => addressed_with(
            protocol,
            CREATE_ACCESS_ENTRY_INPUT_SCHEMA,
            req,
            |input: &CreateAccessEntryInput, _| addresses_cluster(&input.cluster_name),
        ),
        // `roleArn` is the role the EKS Pod Identity agent has Amazon EKS assume for
        // the Pods of the service account, so the handler authorizes passing it
        // before it judges the body or resolves the cluster.
        "CreatePodIdentityAssociation" => addressed_with(
            protocol,
            CREATE_POD_IDENTITY_ASSOCIATION_INPUT_SCHEMA,
            req,
            |input: &CreatePodIdentityAssociationInput, _| {
                passes_roles_and_addresses_cluster(
                    vec![input.role_arn.clone()],
                    &input.cluster_name,
                )
            },
        ),
        "AssociateIdentityProviderConfig" => addressed_with(
            protocol,
            ASSOCIATE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA,
            req,
            |input: &AssociateIdentityProviderConfigInput, _| {
                addresses_cluster(&input.cluster_name)
            },
        ),
        // The listings of a cluster's resources: each bounds `maxResults`, so the walk
        // can refuse the body of a read too, and each answers an unknown cluster with
        // the same 404.
        "ListNodegroups" => addressed_with(
            protocol,
            LIST_NODEGROUPS_INPUT_SCHEMA,
            req,
            |input: &ListNodegroupsInput, _| addresses_cluster(&input.cluster_name),
        ),
        "ListAddons" => addressed_with(
            protocol,
            LIST_ADDONS_INPUT_SCHEMA,
            req,
            |input: &ListAddonsInput, _| addresses_cluster(&input.cluster_name),
        ),
        "ListCapabilities" => addressed_with(
            protocol,
            LIST_CAPABILITIES_INPUT_SCHEMA,
            req,
            |input: &ListCapabilitiesInput, _| addresses_cluster(&input.cluster_name),
        ),
        "ListUpdates" => addressed_with(
            protocol,
            LIST_UPDATES_INPUT_SCHEMA,
            req,
            |input: &ListUpdatesInput, _| addresses_cluster(&input.name),
        ),
        _ => None,
    }
}

/// Whether this layer has a question about the request `key` names — the decision
/// `crate::service::EmulatedService::override_operation` makes, with no store in
/// hand.
///
/// It is deliberately the cheap half: the answer itself needs the store and the
/// request's signing region, which are only reachable once the claimed operation
/// runs (see [`PrecedenceFirst::run`]).
pub fn has_a_question(key: &str, protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> bool {
    addressed(key, protocol, req).is_some()
}

/// The operation this service serves for a request whose body the walk refuses and
/// whose operation answers something else first.
///
/// An [`ErasedOperation`] rather than a response built in
/// `Service::override_operation`, because that seam hands the router a BORROWED
/// operation. It holds the shared engine, because the missing-cluster half of the
/// question is answered from this service's store.
pub struct PrecedenceFirst<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> PrecedenceFirst<Eng> {
    /// Hold the shared engine, the same one every operation of this service is
    /// constructed from — never a store of its own.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> std::fmt::Debug for PrecedenceFirst<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrecedenceFirst").finish_non_exhaustive()
    }
}

impl<Eng: Send + Sync + 'static> PrecedenceFirst<Eng> {
    /// The answer this layer owes `addressed`, or `None` when none of the checks fires
    /// and the modelled operation should serve the request after all.
    fn answer(
        &self,
        addressed: &Addressed,
        ctx: &OperationContext<'_>,
    ) -> Option<Answer> {
        // AUTHORIZATION FIRST, in the order the handler judges the roles: the
        // question needs no state at all ([`crate::state::pass_role_refusal`]).
        for role_arn in &addressed.passed_roles {
            if let Some(why) = crate::state::pass_role_refusal(role_arn) {
                return Some(Answer::PassRoleDenied(AccessDeniedException {
                    message: Some(why),
                }));
            }
        }
        // THE VALUE THIS API DOES NOT DEFINE NEXT: it is decided from the request
        // alone, like the authorization above, and it names nothing any cluster could
        // hold, so no resolution can precede it (see
        // [`crate::service::off_enum_member_contract`]).
        if let Some(exception) = addressed.undefined_value.clone() {
            return Some(Answer::ValueNotDefined(exception));
        }
        // THE CLUSTER NEXT. A cluster name identifies a cluster only within one
        // region, so it is resolved in the region the request is signed for — the same
        // key every handler reads. A store this layer cannot reach answers nothing:
        // the modelled operation runs and reports what it always did.
        if let Some(cluster_name) = addressed.cluster.as_deref() {
            let store = crate::state::service_state(&*self.engine)?;
            let region = crate::state::request_region(ctx.parts());
            let now = std::time::SystemTime::now();
            let mut guard = store.lock();
            guard.materialize_due_transitions(now);
            if !guard
                .clusters
                .contains_key(&(region, cluster_name.to_string()))
            {
                return Some(Answer::ClusterMissing(crate::state::cluster_not_found(
                    cluster_name,
                )));
            }
        }
        // THE BODY LAST, in this service's own words for the parameter the walk
        // refused: everything above is about the authorization of the request or the
        // resources it names, which the recorded service answers first. A rule this
        // service states no sentence for leaves this empty, and the modelled operation
        // answers the framework's own report exactly as before.
        addressed.parameter.clone().map(Answer::ParameterRefused)
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for PrecedenceFirst<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The service's model-derived half is read inside this block and not held
            // past it: `&dyn ErasedServiceDefinition` is not `Sync`, and the future
            // returned here is `Send`. Only the modelled operation — which is
            // `Send + Sync` — crosses the await below.
            let served = {
                let definition = svc.definition();
                let protocol = definition.protocol();
                let key = protocol.routing_key(req).ok();
                // The body is read with the codec dispatch reads one with (see
                // [`crate::service::body_codec`]).
                let body = super::body_codec(svc, definition);
                // The same decision `override_operation` made, re-derived from the
                // same request, and then the answer it owes. When neither check
                // fires, the modelled operation the routing key names serves the
                // request exactly as it would have.
                match key
                    .and_then(|key| addressed(key, body, req))
                    .and_then(|addressed| self.answer(&addressed, ctx))
                {
                    Some(answer) => Ok(answer.response(definition, req)),
                    None => match key.and_then(|key| definition.routing_table().get(key)) {
                        Some(operation) => Err(operation),
                        None => Ok(definition.routing_error_response(req)),
                    },
                }
            };
            match served {
                Ok(response) => response,
                Err(operation) => operation.run(svc, req, ctx).await,
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so this
        // layer cannot weaken the schemes the model declares for it.
        None
    }
}
