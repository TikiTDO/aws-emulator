// SPDX-License-Identifier: CC0-1.0

//! What this service answers BEFORE a refusal of the request body: the resolution of
//! the request's own addressing, the authorization of a role it hands Amazon EKS, the
//! page size it asks for, the capability type a `CreateCapability` asks for, and the
//! resolution of the cluster the request is addressed to.
//!
//! RULE: Amazon EKS judges a request in three stages — first the request's own
//! ADDRESSING and AUTHORIZATION (the `@httpLabel` path parameters, the role it asks
//! Amazon EKS to pass, and the `@httpQuery` page size), then the EXISTENCE of the
//! cluster the request is addressed to, and only then the rest of the body and the
//! state of the resources it names. A request with several problems therefore earns
//! the refusal of the EARLIEST stage one of its problems sits in.
//!
//! Two things in this emulator break that order, and both are answered here:
//!
//! * the framework's own walk validates the whole input against the operation's
//!   generated schema and refuses a modelled constraint BEFORE `handle` runs
//!   (`vendor/smithy-server-runtime/src/operation.rs`), so for a request that breaks
//!   one of those constraints the body refusal reached the caller first and the
//!   earlier stages were never judged. A client keying on the missing cluster to
//!   create it and retry, or on the pass-role denial to fix its policy, saw a
//!   parameter complaint instead;
//! * that walk does not judge a value bound to an `@httpLabel` at all — a label
//!   outside the cluster-name grammar reached the handler, which looked it up and
//!   reported the cluster missing, inviting the caller to create a cluster under a
//!   name no cluster can carry.
//!
//! This layer restores the order. It runs in the one seam a service has BEFORE
//! dispatch (`smithy_server_runtime::Service::override_operation`), the same seam the
//! tag contract beside it uses, and it claims a request only when ALL of:
//!
//! * the operation it was sent to is one of the operations below — each addresses a
//!   cluster by `@httpLabel`, or takes a role it asks Amazon EKS to pass, or bounds a
//!   page size, or all of these;
//! * one of the earlier stages actually answers something: the label names no cluster
//!   any Region could hold, the value names no role this caller can pass, the page
//!   size is outside its published bounds, the request asks for a capability type this
//!   API does not define, or — for a request whose body the framework's walk would
//!   refuse — the store holds no such cluster, or the refused parameter is one this
//!   service states its own sentence for (see [`super::parameter_contract`]).
//!
//! The cluster is answered ahead of the body only for a request the walk WOULD refuse:
//! the walk is run HERE, through the same `aws_smithy_schema_server::constraints::validate`
//! and the same generated input schema the dispatch sequence runs, so this layer
//! cannot disagree with the walk that would otherwise follow it, and a request whose
//! body the walk accepts keeps its handler's own answer, which already resolves the
//! cluster first.
//!
//! The answers are the service's own modelled shapes, rendered through the same
//! protocol call the framework's error tail makes: `AccessDeniedException` (403) for
//! the pass-role denial, `InvalidParameterException` (400) for the request's own
//! parameters and the capability type this API does not define, and
//! `ResourceNotFoundException` (404) for the missing cluster. Each claimed operation
//! answers those same shapes and statuses from its own handler, so a refusal worded
//! here and one worded there are indistinguishable on the wire.
//!
//! WHY the capability type is answered here rather than by the handler, and why it
//! comes before the cluster: see [`crate::service::off_enum_member_contract`], which
//! states the rule and builds the refusal. `CreateCapability`'s `type` targets an
//! enum, so the walk refuses an undeclared value before `handle` runs and the handler
//! never sees it — and the value names no capability type at all, so no cluster could
//! make the request answerable. An add-on's `resolveConflicts` is judged by the same
//! membership test and answered LAST, after the cluster: it says what to do with a
//! copy of the add-on already running on the cluster, which is a question about the
//! add-on rather than about the request's addressing (see
//! [`super::resolve_conflicts_contract`]).


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
    CreateNodegroupInput, CreatePodIdentityAssociationInput, DeleteAddonInput, DescribeAddonInput,
    InvalidParameterException, ListAddonsInput, ListCapabilitiesInput, ListNodegroupsInput,
    ListUpdatesInput, ResourceNotFoundException, UpdateAddonInput, UpdateClusterConfigInput,
    ASSOCIATE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA, CREATE_ACCESS_ENTRY_INPUT_SCHEMA,
    CREATE_ADDON_INPUT_SCHEMA, CREATE_CAPABILITY_INPUT_SCHEMA, CREATE_CLUSTER_INPUT_SCHEMA,
    CREATE_FARGATE_PROFILE_INPUT_SCHEMA, CREATE_NODEGROUP_INPUT_SCHEMA,
    CREATE_POD_IDENTITY_ASSOCIATION_INPUT_SCHEMA, DELETE_ADDON_INPUT_SCHEMA,
    DESCRIBE_ADDON_INPUT_SCHEMA, LIST_ADDONS_INPUT_SCHEMA, LIST_CAPABILITIES_INPUT_SCHEMA,
    LIST_NODEGROUPS_INPUT_SCHEMA, LIST_UPDATES_INPUT_SCHEMA, SCHEMA_ACCESS_DENIED_EXCEPTION,
    SCHEMA_INVALID_PARAMETER_EXCEPTION, SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
    UPDATE_ADDON_INPUT_SCHEMA, UPDATE_CLUSTER_CONFIG_INPUT_SCHEMA,
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
/// of the body: the request's own addressing, the roles it asks Amazon EKS to pass, in
/// the order its handler judges them, the page size it asks for, a member whose value
/// this API does not define, and the cluster it is addressed to.
///
/// All of them are read from the request the dispatch sequence would have handed the
/// handler, so this layer reads exactly the values the handler would.
#[derive(Debug, Default)]
pub struct Addressed {
    passed_roles: Vec<String>,
    /// The refusal the request's own `@httpLabel` cluster name earns, when it carries
    /// a value no cluster of this API could be named: answered before anything is
    /// looked up, because such a label names no cluster in any Region (see
    /// [`cluster_label_refusal`]).
    label: Option<InvalidParameterException>,
    /// The refusal the page size this request asks for earns, when it is outside the
    /// bounds the model publishes: `maxResults` travels in the query string, so it is
    /// part of the request's own addressing and is answered before the cluster is
    /// resolved (see [`super::parameter_contract::page_size_refusal`]).
    page_size: Option<InvalidParameterException>,
    /// The refusal a member whose value this API does not define earns, when the
    /// request carries one: a question about the request alone, so it is answered
    /// before the cluster is resolved (see
    /// [`crate::service::off_enum_member_contract`]).
    undefined_value: Option<InvalidParameterException>,
    cluster: Option<String>,
    /// The refusal this service states for the parameter the walk refused, when it
    /// states one for that rule (see [`super::parameter_contract`]), or the one an
    /// add-on's `resolveConflicts` earns (see [`super::resolve_conflicts_contract`]).
    /// It carries the members the refusal payload reports beside the sentence, which
    /// the per-service message seam cannot reach; it is answered LAST, because it is a
    /// complaint about the body and everything above is about the request's own text,
    /// the authorization of it, or the resources it names.
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

    /// The same request, carrying the refusal the page size it asks for earns.
    fn with_page_size_refusal(mut self, refusal: Option<InvalidParameterException>) -> Self {
        self.page_size = refusal;
        self
    }

    /// Whether this layer owes an answer the REQUEST alone decides — everything but
    /// the cluster, which only the store can resolve.
    ///
    /// This is what lets the layer claim a request whose body the walk accepts: a path
    /// label the walk never judged, a role no caller may pass and a page size outside
    /// its published bounds are each wrong on their own.
    fn answers_from_the_request_alone(&self) -> bool {
        self.passed_roles
            .iter()
            .any(|role_arn| crate::state::pass_role_refusal(role_arn).is_some())
            || self.label.is_some()
            || self.page_size.is_some()
            || self.undefined_value.is_some()
            || self.parameter.is_some()
    }
}

/// The refusal a cluster name arriving in an `@httpLabel` earns when no cluster of
/// this API could carry it, or `None` for a name inside the grammar.
///
/// Every cluster name of this API obeys one grammar and one length maximum
/// ([`crate::state::is_cluster_name`]), which the model states on `ClusterName` and
/// this service enforces on the members it leaves unconstrained as well, so a label
/// outside it names no cluster in any Region rather than one this Region does not
/// hold. The sentence is the one Amazon EKS states for that parameter
/// ([`crate::state::cluster_name_refusal`]) and the payload reports the value the
/// request supplied, which is how a caller that passed a cluster ARN where a bare name
/// belongs sees WHICH value was refused.
fn cluster_label_refusal(member: &str, cluster: &str) -> Option<InvalidParameterException> {
    (!crate::state::is_cluster_name(cluster)).then(|| InvalidParameterException {
        cluster_name: Some(cluster.to_string()),
        message: Some(crate::state::cluster_name_refusal(member, cluster)),
        ..Default::default()
    })
}

/// The refusal the `serviceAccountRoleArn` of an add-on request earns, or `None` when
/// it names a role this caller may have Amazon EKS pass.
///
/// `serviceAccountRoleArn` is a role the request asks Amazon EKS to pass, so it is the
/// same question [`crate::state::pass_role_refusal`] answers for every role this
/// service is handed, reported in the same class: 403 `AccessDeniedException`. It is
/// answered HERE rather than by the handler because `CreateAddon` does not model that
/// error at all, so its own channel cannot carry it — whether the value names a role
/// IAM could hold is a separate question, which the handler asks
/// (`crate::state::names_a_role_iam_could_hold`).
fn addon_role(role_arn: Option<&String>) -> Vec<String> {
    role_arn.cloned().into_iter().collect()
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
/// judges the body, or `None` when the body cannot be read at all — which is the
/// framework's refusal, not this layer's question.
///
/// The walk's own violations are handed to `members` beside the input, so an arm that
/// words the refusal of a parameter reads exactly the violations the dispatch sequence
/// would have reported.
///
/// The CLUSTER is dropped from the answer when the walk accepts the body: such a
/// request is served by its own handler, which already resolves the cluster before it
/// judges anything else, and nothing here may change what that handler reports. Every
/// other answer stands on the request alone and is given either way.
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
    let mut addressed = members(&input, &violations);
    if violations.is_empty() {
        addressed.cluster = None;
    }
    Some(addressed)
}

/// One role the request hands Amazon EKS, and no cluster to resolve.
fn passes_roles(roles: Vec<String>) -> Addressed {
    Addressed {
        passed_roles: roles,
        ..Default::default()
    }
}

/// The cluster the request is addressed to, named by the member its `@httpLabel`
/// arrives in — which is what the refusal of a label outside the cluster-name grammar
/// names (see [`cluster_label_refusal`]) — and no role to authorize.
fn addresses_cluster(member: &str, cluster: &str) -> Addressed {
    Addressed {
        label: cluster_label_refusal(member, cluster),
        cluster: Some(cluster.to_string()),
        ..Default::default()
    }
}

/// Both: a role to authorize first, then the cluster the request is addressed to —
/// the order every handler that does both keeps.
fn passes_roles_and_addresses_cluster(roles: Vec<String>, member: &str, cluster: &str) -> Addressed {
    Addressed {
        passed_roles: roles,
        ..addresses_cluster(member, cluster)
    }
}

/// The member the CHILD resources of a cluster carry their parent's name in, as the
/// model spells it (`CreateAddon$clusterName`, `ListAddons$clusterName`, …) — the
/// name a refusal of that label reports.
const CLUSTER_NAME_MEMBER: &str = "clusterName";

/// The member the CLUSTER-scope operations carry the same name in
/// (`UpdateClusterConfig$name`, `ListUpdates$name`).
const CLUSTER_SCOPE_NAME_MEMBER: &str = "name";

/// The first path segment every URI of this API that addresses a cluster begins with
/// (`/clusters/{clusterName}/addons`, `/clusters/{name}/update-config`, …).
const CLUSTERS_PATH_SEGMENT: &str = "clusters";

/// The position of the cluster label in such a URI, counted as
/// [`::service_common::nth_path_segment`] counts: `/clusters/<label>/…`.
const CLUSTER_LABEL_SEGMENT: usize = 1;

/// The member the operation `key` carries its cluster label in, or `None` for an
/// operation this layer does not judge or one that addresses no cluster by path.
///
/// The same scope as the table below, read from the request's PATH rather than from a
/// deserialized input — see [`addressed_by_path`] for why that reading is needed at
/// all.
fn cluster_label_member(key: &str) -> Option<&'static str> {
    match key {
        "UpdateClusterConfig" | "ListUpdates" => Some(CLUSTER_SCOPE_NAME_MEMBER),
        "CreateNodegroup" | "CreateCapability" | "CreateFargateProfile" | "CreateAddon"
        | "UpdateAddon" | "DescribeAddon" | "DeleteAddon" | "CreateAccessEntry"
        | "CreatePodIdentityAssociation" | "AssociateIdentityProviderConfig"
        | "ListNodegroups" | "ListAddons" | "ListCapabilities" => Some(CLUSTER_NAME_MEMBER),
        _ => None,
    }
}

/// What this layer answers for a request whose body the codec cannot read at all.
///
/// A request that omits a member this API requires fails DESERIALIZATION rather than
/// the validating walk (`vendor/smithy-server-runtime/src/pre_dispatch.rs` turns that
/// failure into the same refusal), so the typed reading below answers nothing about it
/// — and that is exactly the request Amazon EKS answers with its PATH LABEL: the
/// addressing is in the request line, which is readable whatever the body says, and a
/// label no cluster of this API could carry is refused before any member of the body
/// is looked at.
///
/// Only the label is read here, and the request is claimed only when that label is
/// refused: nothing else about a body the codec could not read is known, so every
/// other request keeps the framework's own refusal of it.
fn addressed_by_path(key: &str, req: &RtRequest) -> Option<Addressed> {
    let member = cluster_label_member(key)?;
    let path = ::service_common::uri_path(req.uri());
    if ::service_common::nth_path_segment(path, 0)? != CLUSTERS_PATH_SEGMENT {
        return None;
    }
    // Decoded the way the codec that deserializes the request decodes a non-greedy
    // `@httpLabel`, so this layer and the handler read the same value: a cluster ARN
    // passed where a bare name belongs arrives with its `:` and `/` escaped.
    let cluster = ::service_common::percent_decode_path_label(::service_common::nth_path_segment(
        path,
        CLUSTER_LABEL_SEGMENT,
    )?);
    Some(Addressed {
        label: Some(cluster_label_refusal(member, &cluster)?),
        ..Default::default()
    })
}

/// What the operation `key` names answers before it judges the body, for this
/// request.
///
/// The table is this layer's whole scope. An operation is in it because it addresses a
/// cluster by `@httpLabel`, or hands Amazon EKS a role, or bounds a page size — the
/// stages the rule above puts ahead of the body; `None` for every other operation,
/// which keeps the generated routing table as its only source. On this service's
/// restJson1 protocol a routing key is the operation's own shape name.
///
/// A request the codec cannot read at all falls back to its PATH, which carries the
/// addressing either way (see [`addressed_by_path`]).
fn addressed(
    key: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<Addressed> {
    addressed_by_input(key, protocol, req).or_else(|| addressed_by_path(key, req))
}

/// What the operation `key` names answers, read from the input the dispatch sequence
/// would hand its handler — `None` when that input cannot be read at all.
fn addressed_by_input(
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
                    CLUSTER_NAME_MEMBER,
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
                    CLUSTER_NAME_MEMBER,
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
                    CLUSTER_NAME_MEMBER,
                    &input.cluster_name,
                )
            },
        ),
        "UpdateClusterConfig" => addressed_with(
            protocol,
            UPDATE_CLUSTER_CONFIG_INPUT_SCHEMA,
            req,
            |input: &UpdateClusterConfigInput, _| {
                addresses_cluster(CLUSTER_SCOPE_NAME_MEMBER, &input.name)
            },
        ),
        // The two add-on operations: each hands Amazon EKS the add-on's service
        // account role, which they report an unusable value as in their own two
        // classes ([`addon_role_refusal`]), and each takes `resolveConflicts`, whose
        // enum membership is a question about the ADD-ON and so is answered after the
        // cluster (see [`super::resolve_conflicts_contract`]).
        "CreateAddon" => addressed_with(
            protocol,
            CREATE_ADDON_INPUT_SCHEMA,
            req,
            |input: &CreateAddonInput, violations| {
                passes_roles_and_addresses_cluster(
                    addon_role(input.service_account_role_arn.as_ref()),
                    CLUSTER_NAME_MEMBER,
                    &input.cluster_name,
                )
                .with_parameter_refusal(super::resolve_conflicts_contract::off_enum_refusal(
                    super::resolve_conflicts_contract::CREATE_ADDON,
                    input.resolve_conflicts.as_deref(),
                    &input.cluster_name,
                    &input.addon_name,
                    violations,
                ))
            },
        ),
        "UpdateAddon" => addressed_with(
            protocol,
            UPDATE_ADDON_INPUT_SCHEMA,
            req,
            |input: &UpdateAddonInput, violations| {
                passes_roles_and_addresses_cluster(
                    addon_role(input.service_account_role_arn.as_ref()),
                    CLUSTER_NAME_MEMBER,
                    &input.cluster_name,
                )
                .with_parameter_refusal(super::resolve_conflicts_contract::off_enum_refusal(
                    super::resolve_conflicts_contract::UPDATE_ADDON,
                    input.resolve_conflicts.as_deref(),
                    &input.cluster_name,
                    &input.addon_name,
                    violations,
                ))
            },
        ),
        // The two add-on reads addressed to a `(cluster, add-on)` pair: they carry no
        // body at all, so the only stage ahead of their own lookup is the label.
        "DescribeAddon" => addressed_with(
            protocol,
            DESCRIBE_ADDON_INPUT_SCHEMA,
            req,
            |input: &DescribeAddonInput, _| {
                addresses_cluster(CLUSTER_NAME_MEMBER, &input.cluster_name)
            },
        ),
        "DeleteAddon" => addressed_with(
            protocol,
            DELETE_ADDON_INPUT_SCHEMA,
            req,
            |input: &DeleteAddonInput, _| {
                addresses_cluster(CLUSTER_NAME_MEMBER, &input.cluster_name)
            },
        ),
        "CreateAccessEntry" => addressed_with(
            protocol,
            CREATE_ACCESS_ENTRY_INPUT_SCHEMA,
            req,
            |input: &CreateAccessEntryInput, _| {
                addresses_cluster(CLUSTER_NAME_MEMBER, &input.cluster_name)
            },
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
                    CLUSTER_NAME_MEMBER,
                    &input.cluster_name,
                )
            },
        ),
        "AssociateIdentityProviderConfig" => addressed_with(
            protocol,
            ASSOCIATE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA,
            req,
            |input: &AssociateIdentityProviderConfigInput, _| {
                addresses_cluster(CLUSTER_NAME_MEMBER, &input.cluster_name)
            },
        ),
        // The listings of a cluster's resources: each bounds `maxResults` in the query
        // string, which is part of the request's own addressing and so is answered
        // before the cluster, and each answers an unknown cluster with the same 404.
        "ListNodegroups" => addressed_with(
            protocol,
            LIST_NODEGROUPS_INPUT_SCHEMA,
            req,
            |input: &ListNodegroupsInput, violations| {
                addresses_cluster(CLUSTER_NAME_MEMBER, &input.cluster_name)
                    .with_page_size_refusal(super::parameter_contract::page_size_refusal(
                        violations,
                    ))
            },
        ),
        "ListAddons" => addressed_with(
            protocol,
            LIST_ADDONS_INPUT_SCHEMA,
            req,
            |input: &ListAddonsInput, violations| {
                addresses_cluster(CLUSTER_NAME_MEMBER, &input.cluster_name)
                    .with_page_size_refusal(super::parameter_contract::page_size_refusal(
                        violations,
                    ))
            },
        ),
        "ListCapabilities" => addressed_with(
            protocol,
            LIST_CAPABILITIES_INPUT_SCHEMA,
            req,
            |input: &ListCapabilitiesInput, violations| {
                addresses_cluster(CLUSTER_NAME_MEMBER, &input.cluster_name)
                    .with_page_size_refusal(super::parameter_contract::page_size_refusal(
                        violations,
                    ))
            },
        ),
        "ListUpdates" => addressed_with(
            protocol,
            LIST_UPDATES_INPUT_SCHEMA,
            req,
            |input: &ListUpdatesInput, violations| {
                addresses_cluster(CLUSTER_SCOPE_NAME_MEMBER, &input.name)
                    .with_page_size_refusal(super::parameter_contract::page_size_refusal(
                        violations,
                    ))
            },
        ),
        _ => None,
    }
}

/// Whether this layer has a question about the request `key` names — the decision
/// `crate::service::EmulatedService::override_operation` makes, with no store in
/// hand.
///
/// It is deliberately the cheap half: a question only the store can answer — whether
/// the cluster this request is addressed to exists — is claimed by the fact that the
/// request RESOLVES one, and answered once the claimed operation runs (see
/// [`PrecedenceFirst::run`]). Everything else is decided from the request alone.
pub fn has_a_question(key: &str, protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> bool {
    addressed(key, protocol, req).is_some_and(|addressed| {
        addressed.answers_from_the_request_alone() || addressed.cluster.is_some()
    })
}

/// The operation this service serves for a request whose own text, authorization or
/// cluster its operation answers before the body.
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
        // THE REQUEST'S OWN ADDRESSING NEXT: a path label no cluster of this API could
        // carry, and a page size outside its published bounds. Both are decided from
        // the request alone, and neither names a resource any lookup could find, so no
        // resolution can precede them.
        if let Some(exception) = addressed.label.clone() {
            return Some(Answer::ParameterRefused(exception));
        }
        if let Some(exception) = addressed.page_size.clone() {
            return Some(Answer::ParameterRefused(exception));
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
        // refused: everything above is about the request's own text, the authorization
        // of it or the resources it names, which the recorded service answers first. A
        // rule this service states no sentence for leaves this empty, and the modelled
        // operation answers the framework's own report exactly as before.
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
