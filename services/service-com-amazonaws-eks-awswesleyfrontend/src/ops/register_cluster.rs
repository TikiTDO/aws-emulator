// SPDX-License-Identifier: CC0-1.0

//! @op RegisterCluster
//!
//! IMPLEMENTED. `RegisterCluster` connects an EXTERNAL Kubernetes cluster: it mints one
//! [`crate::state::ClusterRecord`] in the shared engine's store, marked a connected
//! cluster by the `connectorConfig` it carries, and answers with the `Cluster` it just
//! wrote. `DescribeCluster`, `ListClusters`, `ListTagsForResource` and `TagResource` all
//! read that same record.
//!
//! A connected cluster has no Amazon EKS control plane behind it, so the record it writes
//! leaves `endpoint`, `roleArn`, `resourcesVpcConfig`, `version`, `platformVersion`,
//! `identity` and `certificateAuthority` unset: what the caller gets back is the
//! registration (`activationId`, `activationCode`, `activationExpiry`, `provider`,
//! `roleArn`), which is what the connector manifest is built from.
//!
//! It is reported `PENDING`, because the cluster only becomes visible to Amazon EKS once
//! the manifest is applied in the external cluster through its native provider — an act
//! outside this API, which nothing in this emulator performs, so no call moves it to
//! `ACTIVE`. The one move the clock does owe it is the lapse of its activation window
//! ([`crate::state::CONNECTOR_ACTIVATION_WINDOW`]): three days after the registration a
//! still-`PENDING` connected cluster is `FAILED` and has to be deregistered. The due
//! instant is stored on the record and materialised by whichever read first runs after
//! it; nothing sleeps for it.
//!
//! Nothing is written until the connector role RESOLVES: it must name a role of this
//! account ([`crate::state::pass_role_refusal`], reported as this operation's own 400
//! `InvalidParameterException`) and be one IAM holds whose trust policy lets the SSM
//! service principal assume it
//! ([`crate::state::service_principal_trust_violation`]) — a registration whose connector
//! IAM wiring could not work is refused rather than stored. That refusal comes from the
//! plane that manages external clusters, so it reaches the caller WRAPPED, with the
//! service, the status and the call behind it named in the tail
//! ([`crate::state::wrapped_peer_tail`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RegisterCluster`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Connects a Kubernetes cluster to the Amazon EKS control plane. Any Kubernetes cluster
//! can be connected to the Amazon EKS control plane to view current information about the
//! cluster and its nodes. Cluster connection requires two steps. First, send a
//! RegisterClusterRequest to add it to the Amazon EKS control plane. Second, a Manifest
//! containing the activationID and activationCode must be applied to the Kubernetes cluster
//! through it's native provider to provide visibility. After the manifest is updated and
//! applied, the connected cluster is visible to the Amazon EKS control plane. If the
//! manifest isn't applied within three days, the connected cluster will no longer be
//! visible and must be deregistered using DeregisterCluster.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `connectorConfig` (ConnectorConfigRequest) **required** — The configuration settings
//!   required to connect the Kubernetes cluster to the Amazon EKS control plane.
//! * `name` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$) **required** — A
//!   unique name for this cluster in your Amazon Web Services Region.
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources.
//!
//! ## Output
//!
//! * `cluster` (Cluster) — An object representing an Amazon EKS cluster.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have permissions to perform the requested
//!   operation. The IAM principal making the request must have at least one IAM permissions
//!   policy attached that grants the required permissions. For more information, see Access
//!   management in the IAM User Guide.
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceLimitExceededException` (HTTP 400) — You have encountered a service limit on
//!   the specified resource.
//! * `ResourcePropagationDelayException` (HTTP 428) — Required resources (such as
//!   service-linked roles) were created and are still propagating. Retry later.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.
//! * `InvalidRequestException` (HTTP 400) — widened in through `extra_errors` (see below):
//!   the model's `errors` list for this operation omits it, but the SERVICE models it and
//!   it is what a connector role Amazon EKS cannot use is refused with.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, ClusterRecord, CONNECTOR_ACTIVATION_WINDOW, CONNECTOR_ROLE_ARN_REFUSAL,
    CONNECTOR_SERVICE_PRINCIPAL, MAX_CLUSTERS, MAX_STORED_CLUSTERS, STATUS_PENDING,
};
use std::time::SystemTime;

/// The status the wrapped tail of the connector-role refusal reports: the status of
/// the refusal itself, which the model fixes at 400 for `InvalidRequestException`.
const CONNECTOR_REFUSAL_STATUS: u16 = 400;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RegisterClusterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RegisterClusterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `InvalidRequestException` — a shape this SERVICE models ("The request is invalid given
// the state of the cluster"), and other operations declare, which this one does not list —
// is widened in through `extra_errors`: it is what the recorded service answers when the
// connector role it is given cannot be used, and it carries the `clusterName` of the
// registration that was refused.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#RegisterCluster",
    extra_errors = ["InvalidRequestException"]
)]
#[derive(Debug)]
pub enum RegisterClusterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ResourcePropagationDelayException(ResourcePropagationDelayException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::register_cluster::RegisterClusterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RegisterClusterError = crate::scaffolded::Scaffolded<RegisterClusterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RegisterClusterErrorModeled> for RegisterClusterError {
    fn from(modeled: RegisterClusterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RegisterCluster` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RegisterCluster<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RegisterCluster<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RegisterCluster<Eng>
{
    type Input = RegisterClusterInput;
    type Output = RegisterClusterOutput;
    type Error = RegisterClusterError;

    const NAME: &'static str = "RegisterCluster";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_CLUSTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_CLUSTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            RegisterClusterError::from(RegisterClusterErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let arn = ::engine::emulator_arn("eks", &region, &format!("cluster/{}", input.name));

        let invalid_parameter = |message: String| {
            RegisterClusterError::from(RegisterClusterErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        // `provider` targets the `ConnectorConfigProvider` enum, whose members are the
        // only cloud providers a registration can name. An enum's member set is not a
        // constraint the boundary validator refuses on, so it is read here.
        let provider = input.connector_config.provider.clone();
        if !CONNECTOR_CONFIG_PROVIDER_VALUES.contains(&provider.as_str()) {
            return Err(invalid_parameter(format!(
                "connectorConfig.provider must be one of {}, got {provider}",
                CONNECTOR_CONFIG_PROVIDER_VALUES.join(", ")
            )));
        }

        // The connector role has to name a role of this account and partition, which is
        // the same question [`state::pass_role_refusal`] answers for every role this
        // service is handed. A registration REPORTS it differently from a create,
        // though, and that is what the recording shows: 400 `InvalidParameterException`
        // with the fixed sentence [`CONNECTOR_ROLE_ARN_REFUSAL`], not the 403 pass-role
        // refusal `CreateCluster` answers — the connector role is a parameter of the
        // registration rather than a role this call passes to a control plane it
        // provisions. One sentence for the whole class: a value that is not an ARN, an
        // ARN of another account or partition, and an IAM ARN naming something other
        // than a role are all "the provided roleArn is invalid".
        let role_arn = input.connector_config.role_arn.clone();
        if state::pass_role_refusal(&role_arn).is_some() {
            return Err(invalid_parameter(CONNECTOR_ROLE_ARN_REFUSAL.to_string()));
        }

        // And then whether it is a role Amazon EKS could actually USE for the connector:
        // the cluster is registered as a Systems Manager managed instance, so the role
        // has to exist in IAM and its trust policy has to let the SSM service principal
        // ([`CONNECTOR_SERVICE_PRINCIPAL`]) assume it. IAM is what holds roles, so IAM is
        // asked ([`state::service_principal_trust_violation`]); the registration is
        // refused with the state error the recorded service answers, naming the role at
        // fault, and nothing is written. It runs before the store is locked: it is a call
        // out to another service.
        if let Some(why) = state::service_principal_trust_violation(
            ctx,
            &region,
            &role_arn,
            &[CONNECTOR_SERVICE_PRINCIPAL],
        )
        .await
        {
            // `clusterName` is left UNSET, as the recorded refusal leaves it: no cluster
            // was registered, so there is none to name — the message names the role,
            // which is what the caller has to fix.
            //
            // The role was judged by the plane that manages EXTERNAL clusters, and a
            // refusal this service carries through from another is reported WRAPPED:
            // the sentence, then the service that answered, the status it answered and
            // the call that was made ([`state::wrapped_peer_tail`]) — the same shape
            // `CreateCluster` reports EC2's subnet refusal in.
            return Err(RegisterClusterErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    message: Some(format!(
                        "{why}{}",
                        state::wrapped_peer_tail(
                            state::EXTERNAL_CLUSTER_MANAGER,
                            CONNECTOR_REFUSAL_STATUS
                        )
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A cluster name is unique within one Region and account ("A unique name for
        // this cluster in your Amazon Web Services Region", `RegisterCluster$name`), and
        // a registration occupies that name exactly as a created cluster does — the
        // store is keyed `(region, name)` for both.
        //
        // The NAME is the whole conflict key: `clientRequestToken` is
        // `@idempotencyToken`, but a registration deduplicates on nothing — a second
        // call for a name this account already holds in this Region is refused whether
        // the token is identical, different or absent, because the name is already
        // taken and there is nothing left for the call to do. (A token is still stored
        // on the record it created, which is what `DescribeCluster` reports back.)
        if guard
            .clusters
            .contains_key(&(region.clone(), input.name.clone()))
        {
            return Err(RegisterClusterErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(format!("Cluster already exists with name: {}", input.name)),
                    ..Default::default()
                },
            )
            .into());
        }

        // The same two ceilings `CreateCluster` enforces, for the same reasons: the
        // published per-Region EKS cluster quota, and the store-wide ceiling that bounds
        // how much repeated calls can retain across caller-chosen regions. A connected
        // cluster occupies a cluster record, so it is counted by both.
        if guard.cluster_names_in(&region).count() >= MAX_CLUSTERS {
            return Err(RegisterClusterErrorModeled::ResourceLimitExceededException(
                ResourceLimitExceededException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(format!(
                        "You have reached the limit of {MAX_CLUSTERS} clusters for this \
                         account in this Region"
                    )),
                    ..Default::default()
                },
            )
            .into());
        }
        if guard.clusters.len() >= MAX_STORED_CLUSTERS {
            return Err(RegisterClusterErrorModeled::ResourceLimitExceededException(
                ResourceLimitExceededException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(format!(
                        "You have reached the limit of {MAX_STORED_CLUSTERS} clusters for \
                         this account"
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // "If the manifest isn't applied within three days, the connected cluster will
        // no longer be visible and must be deregistered using DeregisterCluster" — so
        // the activation expires one window after the registration, and that instant is
        // both what the caller is told and when the lapse falls due.
        let activation_expiry = now + CONNECTOR_ACTIVATION_WINDOW;
        let connector_config = ConnectorConfigResponse {
            activation_id: Some(state::derive_activation_id(&arn, now)),
            activation_code: Some(state::derive_activation_code(&arn, now)),
            activation_expiry: Some(::aws_smithy_types::DateTime::from(activation_expiry)),
            provider: Some(provider),
            role_arn: Some(role_arn),
        };

        let record = ClusterRecord {
            name: input.name.clone(),
            arn: arn.clone(),
            created_at: now,
            // Everything an Amazon EKS control plane would carry is unset: this cluster
            // runs outside Amazon Web Services, so none of it exists. See
            // `crate::state::ClusterRecord`.
            version: None,
            platform_version: None,
            endpoint: None,
            role_arn: None,
            resources_vpc_config: None,
            kubernetes_network_config: None,
            logging: None,
            identity: None,
            certificate_authority: None,
            // Visible to Amazon EKS only once the manifest is applied in the external
            // cluster, which is not something this API — or this emulator — does.
            status: STATUS_PENDING.to_string(),
            active_at: None,
            // There is no Amazon EKS control plane behind a connected cluster, so
            // nothing provisions one and the members that would report it stay
            // unset ([`crate::state::ClusterRecord::control_plane_provisioned`]).
            control_plane_provisioned: false,
            tags: input
                .tags
                .unwrap_or_default()
                .into_iter()
                .collect::<::std::collections::BTreeMap<_, _>>(),
            encryption_config: Vec::new(),
            access_config: None,
            // A registration sets up no cluster admin access entry: the member
            // reports what the CREATE of an Amazon EKS cluster decided, and this
            // is not one.
            bootstrap_cluster_creator_admin_permissions: None,
            deletion_protection: false,
            client_request_token: input.client_request_token.clone(),
            outpost_config: None,
            upgrade_policy: None,
            zonal_shift_config: None,
            remote_network_config: None,
            compute_config: None,
            storage_config: None,
            control_plane_scaling_config: None,
            kube_api_server_config: None,
            kube_scheduler_config: None,
            kube_controller_manager_config: None,
            connector_config: Some(connector_config),
            activation_expires_at: Some(activation_expiry),
            // No access entry, no asynchronous update and no identity provider
            // config: each is written by the operation that creates it, and a
            // connected cluster is not one those operations act on today (each is
            // still a 501 with its own gap issue, where it is not implemented).
            access_entries: ::std::collections::BTreeMap::new(),
            updates: ::std::collections::BTreeMap::new(),
            identity_provider_configs: ::std::collections::BTreeMap::new(),
            // A connected cluster has no Amazon EKS control plane, so Amazon EKS
            // provisions no certificate authority for it and none can be appended:
            // the CAs in this map are the ones that sign for an EKS-managed API
            // server (`crate::state::CertificateAuthorityRecord`).
            certificate_authorities: ::std::collections::BTreeMap::new(),
            certificate_authorities_created: 0,
            // A registration starts no insights refresh, and a connected cluster
            // is not one `StartInsightsRefresh` acts on: it never reaches `ACTIVE`
            // in this emulator.
            insights_refresh: None,
            // Cluster insights are an analysis of a control plane Amazon EKS
            // manages, and a connected cluster has none, so no analysis of one
            // ever completes.
            insights_analyzed_at: None,
        };

        let cluster = record.to_cluster();
        guard
            .arn_index
            .insert(arn, (region.clone(), input.name.clone()));
        guard.clusters.insert((region, input.name), record);

        Ok(RegisterClusterOutput {
            cluster: Some(cluster),
        })
    }
}
