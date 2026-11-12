// SPDX-License-Identifier: CC0-1.0

//! @op UpdateAddon
//!
//! IMPLEMENTED. `UpdateAddon` changes an add-on that is already installed on a
//! cluster. The change is ASYNCHRONOUS — the operation answers an `Update`, not an
//! `Addon` — so this handler does NOT write the requested values onto the add-on:
//! it mints an [`crate::state::UpdateRecord`] on the cluster, scoped to the add-on
//! ([`crate::state::UpdateScope::Addon`]), moves the add-on to `UPDATING`, and parks
//! the change on the record as a [`crate::state::PendingAddonUpdate`]. The clock owes
//! that update its `Successful` status [`crate::state::UPDATE_COMPLETION_DELAY`]
//! later, and materialising it is what writes the new version, service account role,
//! configuration values and EKS Pod Identity associations and hands the add-on back
//! to `ACTIVE`. `DescribeUpdate` and `ListUpdates` report the update; `DescribeAddon`
//! reports the add-on before and after it completes.
//!
//! An add-on is a child of a cluster, so the layers this handler checks run from the
//! OUTSIDE IN, and each layer's refusal is the modelled error for what it found:
//!
//! 1. the cluster named by the `@httpLabel` must exist IN THE REQUEST'S REGION, and
//!    the add-on must be installed on it — otherwise `ResourceNotFoundException`
//!    (404), which names neither half of the pair it could not resolve
//!    ([`crate::state::resource_not_found`]);
//! 2. a cluster `RegisterCluster` connected has no Amazon EKS-managed plane for an
//!    add-on to live in — `ClientException` (400), the permanent refusal
//!    [`crate::state::connected_cluster_refusal`] words;
//! 3. a retry of the call that started an update of this add-on, recognised by its
//!    `clientRequestToken`, is answered with THAT update rather than starting a
//!    second one;
//! 4. the add-on takes one update at a time — otherwise `ResourceInUseException`
//!    (409) — and must be `ACTIVE` to take one at all, otherwise
//!    `InvalidRequestException` (400);
//! 5. the body's own members must hold up — otherwise `InvalidParameterException`
//!    (400), naming the member and what it must be. A JSON `configurationValues`
//!    document is held to the configuration schema of the build the add-on will run,
//!    the same schema `CreateAddon` validates against and
//!    `DescribeAddonConfiguration` publishes;
//! 6. the EKS Pod Identity associations the update asks for must fit the cluster's
//!    association ceiling and must not double-book a `(namespace, service account)`
//!    pair with an association ANOTHER owner holds — decided before anything is
//!    written, so a request that cannot be carried out in full writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateAddon`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Updates an Amazon EKS add-on.
//!
//! ## Input
//!
//! * `addonName` (String) **required** — The name of the add-on. The name must match one of
//!   the names returned by ListAddons .
//! * `addonVersion` (String) — The version of the add-on. The version must match one of the
//!   versions returned by DescribeAddonVersions .
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$)
//!   **required** — The name of your cluster.
//! * `configurationValues` (String) — The set of configuration values for the add-on that's
//!   created. The values that you provide are validated against the schema returned by
//!   DescribeAddonConfiguration.
//! * `podIdentityAssociations` (AddonPodIdentityAssociationsList) — An array of EKS Pod
//!   Identity associations to be updated. Each association maps a Kubernetes service account
//!   to an IAM role. If this value is left blank, no change. If an empty array is provided,
//!   existing associations owned by the add-on are deleted. For more information, see Attach
//!   an IAM Role to an Amazon EKS add-on using EKS Pod Identity in the Amazon EKS User Guide.
//! * `resolveConflicts` (ResolveConflicts, one of: NONE | OVERWRITE | PRESERVE) — How to
//!   resolve field value conflicts for an Amazon EKS add-on if you've changed a value from
//!   the Amazon EKS default value. Conflicts are handled based on the option you choose: None
//!   – Amazon EKS doesn't change the value. The update might fail. Overwrite – Amazon EKS
//!   overwrites the changed value back to the Amazon EKS default value. Preserve – Amazon EKS
//!   preserves the value. If you choose this option, we recommend that you test any field and
//!   value changes on a non-production cluster before updating the add-on on your production
//!   cluster.
//! * `serviceAccountRoleArn` (RoleArn, len 1..255) — The Amazon Resource Name (ARN) of an
//!   existing IAM role to bind to the add-on's service account. The role must be assigned the
//!   IAM permissions required by the add-on. If you don't specify an existing IAM role, then
//!   the add-on uses the permissions assigned to the node IAM role. For more information, see
//!   Amazon EKS node IAM role in the Amazon EKS User Guide. To specify an existing IAM role,
//!   you must have an IAM OpenID Connect (OIDC) provider created for your cluster. For more
//!   information, see Enabling IAM roles for service accounts on your cluster in the Amazon
//!   EKS User Guide.
//!
//! ## Output
//!
//! * `update` (Update) — An object representing an asynchronous update.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, PendingAddonUpdate, UpdateRecord, UpdateScope, ADDON_STATUSES_AN_UPDATE_STARTS_FROM,
    ADDON_STATUS_UPDATING, CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN, DEFAULT_KUBERNETES_VERSION,
    MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER,
    UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS, UPDATE_PARAM_ADDON_VERSION,
    UPDATE_PARAM_CONFIGURATION_VALUES, UPDATE_PARAM_POD_IDENTITY_ASSOCIATIONS,
    UPDATE_PARAM_RESOLVE_CONFLICTS, UPDATE_PARAM_SERVICE_ACCOUNT_ROLE_ARN, UPDATE_TYPE_ADDON,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateAddonError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateAddonError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#UpdateAddon")]
#[derive(Debug)]
pub enum UpdateAddonErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_addon::UpdateAddonError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateAddonError = crate::scaffolded::Scaffolded<UpdateAddonErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateAddonErrorModeled> for UpdateAddonError {
    fn from(modeled: UpdateAddonErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The requested EKS Pod Identity associations as an `UpdateParam$value`: the
/// `podIdentityAssociations` list in the same JSON shape it arrived in, so a caller
/// reading the update back sees what it asked for rather than a rendering of this
/// service's internals.
fn pod_identity_associations_param(requested: &[AddonPodIdentityAssociations]) -> String {
    let entries: Vec<::serde_json::Value> = requested
        .iter()
        .map(|association| {
            ::serde_json::json!({
                "serviceAccount": association.service_account,
                "roleArn": association.role_arn,
            })
        })
        .collect();
    ::serde_json::Value::Array(entries).to_string()
}

/// The `UpdateAddon` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateAddon<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateAddon<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateAddon<Eng>
{
    type Input = UpdateAddonInput;
    type Output = UpdateAddonOutput;
    type Error = UpdateAddonError;

    const NAME: &'static str = "UpdateAddon";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ADDON_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ADDON_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();

        let store = state::service_state(&*self.engine).ok_or_else(|| {
            UpdateAddonError::from(UpdateAddonErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.cluster_name.clone()),
                addon_name: Some(input.addon_name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        // LAYER 0 — the request's own addressing, which needs nothing from this
        // service's own state and so is judged before the store is even locked.
        //
        // The `@httpLabel` `clusterName` must be a name a cluster of this API could
        // carry (`ClusterName` is `@length(1..100)` + `@pattern`,
        // [`crate::state::is_cluster_name`]), so a label outside it names no cluster in
        // any Region and is refused as the invalid parameter it is rather than looked
        // up. And `serviceAccountRoleArn`, the IAM role the add-on's Kubernetes service
        // account is given, must be a role this caller may hand Amazon EKS — the
        // service-wide pass-role question, answered before dispatch as the 403 this
        // operation does not model (`crate::service::precedence`), so it is not
        // re-judged here — and one IAM could hold at all
        // ([`crate::state::names_a_role_iam_could_hold`]), which is refused with the
        // sentence this API states for that parameter. The pre-dispatch layer gives
        // these same answers ahead of the framework's own refusal of a body member.
        if !state::is_cluster_name(&input.cluster_name) {
            return Err(UpdateAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(state::cluster_name_refusal(
                        "clusterName",
                        &input.cluster_name,
                    )),
                    ..Default::default()
                },
            )
            .into());
        }
        if input
            .service_account_role_arn
            .as_deref()
            .is_some_and(|role_arn| !state::names_a_role_iam_could_hold(role_arn))
        {
            // The sentence ALONE: nothing was resolved when the role was judged, so
            // there is no cluster and no add-on for the payload to report.
            return Err(UpdateAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some(state::ADDON_SERVICE_ACCOUNT_ROLE_ARN_REFUSAL.to_string()),
                    ..Default::default()
                },
            )
            .into());
        }
        // And whether IAM HOLDS the role that value names, which only IAM can answer
        // ([`crate::state::iam_holds_no_such_role`]): the same refusal, with no lock
        // held, and nothing refused where there is no IAM to ask.
        if let Some(role_arn) = input.service_account_role_arn.as_deref() {
            if state::iam_holds_no_such_role(ctx, &region, role_arn).await {
                return Err(UpdateAddonErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some(state::ADDON_SERVICE_ACCOUNT_ROLE_ARN_REFUSAL.to_string()),
                        ..Default::default()
                    },
                )
                .into());
            }
        }

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // The two refusals every layer below reports through, so the operation cannot
        // word or classify one of them two ways: "The specified parameter is invalid"
        // for a member of the body, and "The request is invalid given the state of the
        // cluster" for what the cluster or the add-on is doing.
        let invalid_parameter = |message: String| {
            UpdateAddonError::from(UpdateAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };
        let invalid_request = |message: String| {
            UpdateAddonError::from(UpdateAddonErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        // LAYER 1 — the cluster. The add-on this updates lives inside it, and a cluster
        // name identifies a cluster only within the region the request is signed for.
        // This call addresses ONE resource — the `(cluster, add-on)` pair — so neither
        // half is named and no identity member is serialized
        // ([`crate::state::resource_not_found`]).
        let cluster_key = (region.clone(), input.cluster_name.clone());
        let Some(cluster) = guard.clusters.get(&cluster_key) else {
            return Err(UpdateAddonErrorModeled::ResourceNotFoundException(
                state::resource_not_found(),
            )
            .into());
        };

        // An add-on is a piece of software Amazon EKS manages inside the cluster's
        // control plane, and a cluster `RegisterCluster` connected has none of that
        // plane: the operation does not apply to it at all, which the connected-cluster
        // gate reports permanently.
        if cluster.is_connected() {
            return Err(UpdateAddonErrorModeled::ClientException(
                state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        let addon_key = (
            region.clone(),
            input.cluster_name.clone(),
            input.addon_name.clone(),
        );
        let Some(addon) = guard.addons.get(&addon_key) else {
            return Err(UpdateAddonErrorModeled::ResourceNotFoundException(
                state::resource_not_found(),
            )
            .into());
        };
        // Everything this handler needs from the two records it has resolved, so the
        // layers below can take the store mutably when they come to write.
        let addon_arn = addon.arn.clone();
        let addon_namespace = addon.namespace.clone();
        let addon_status = addon.status.clone();
        let installed_version = addon.addon_version.clone();
        let cluster_status = cluster.status.clone();
        // The cluster's own Kubernetes version, which the builds offered for this
        // add-on are the ones compatible with. Every cluster this far into the handler
        // is an Amazon EKS cluster and reports one; the service's default Kubernetes
        // version stands in for a record that somehow carries none.
        let cluster_version = cluster
            .version
            .clone()
            .unwrap_or_else(|| DEFAULT_KUBERNETES_VERSION.to_string());
        // An update of an add-on is an update OF THAT ADD-ON: it is what selects the
        // update in a listing (`ListUpdates$addonName`) and what makes a second update
        // of the same add-on a conflict while the first is still running.
        let scope = UpdateScope::Addon(input.addon_name.clone());

        // LAYER 2 — a retry. `clientRequestToken` is `@idempotencyToken`: an SDK
        // generates it once per call and RESENDS the same value when it retries, so a
        // retry of the call that started an update of this add-on is answered with THAT
        // update rather than starting a second one. A blank token identifies nothing, so
        // it never matches, and only an update of this add-on can be a retry of this
        // call.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(update) = replayed_token
            .and_then(|token| cluster.update_for_token(token))
            .filter(|update| update.scope == scope)
        {
            return Ok(UpdateAddonOutput {
                update: Some(update.to_update()),
            });
        }

        // LAYER 3 — what is already in flight. One add-on takes one update at a time:
        // while its own update is running the add-on is in use, which is what
        // `ResourceInUseException` reports, worded by
        // [`crate::state::update_in_progress`] for the update type this call would have
        // minted.
        if let Some(in_flight) = cluster.in_flight_update_in_scope(&scope) {
            return Err(UpdateAddonErrorModeled::ResourceInUseException(
                state::update_in_progress(UPDATE_TYPE_ADDON, &input.cluster_name, in_flight),
            )
            .into());
        }

        // LAYER 4 — the state of the cluster and of the add-on. The cluster must be
        // one Amazon EKS is managing an add-on of at all
        // ([`crate::state::CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN`]: one being
        // provisioned, running, or taking a change does, one on its way out does not),
        // and an add-on can only take a change once it has SETTLED: one still being
        // created, or being deleted, cannot. An add-on whose last update failed, or
        // that is running in a degraded state, has settled — another `UpdateAddon` is
        // how such an add-on is remediated, so it is admitted here and a successful
        // update clears the issue the failure left on it.
        if !CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN.contains(&cluster_status.as_str()) {
            return Err(invalid_request(state::cluster_cannot_manage_an_addon(
                &input.cluster_name,
                &cluster_status,
                "update an add-on",
            )));
        }
        if !ADDON_STATUSES_AN_UPDATE_STARTS_FROM.contains(&addon_status.as_str()) {
            // An add-on that has not settled is IN USE by whatever has not finished
            // with it — its own install, a delete, or an update already running — so
            // this is the 409 `ResourceInUseException` the operation models rather
            // than a statement about the request: a caller that waits can send the
            // same request again and have it accepted. The sentence names the add-on
            // the request asked to update and the status it is currently in, and the
            // exception reports both the add-on and its cluster.
            return Err(UpdateAddonErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(state::addon_not_updatable_message(
                        &input.addon_name,
                        &addon_status,
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // LAYER 5 — the body's own members. Each is a rule the model cannot express, so
        // the boundary validator passed the request and this is where it is caught.
        //
        // An `addonVersion` that is blank names no build, so it is read as a request
        // that does not change the version at all — the same reading `CreateAddon`
        // gives it.
        let requested_version = input
            .addon_version
            .clone()
            .map(|version| version.trim().to_string())
            .filter(|version| !version.is_empty());
        // "The version of the add-on. The version must match one of the versions
        // returned by DescribeAddonVersions" — the same rule `CreateAddon` holds a
        // pinned build to, against the same catalog narrowed to the cluster's own
        // Kubernetes version: an add-on cannot be updated TO a build that cluster is
        // not offered, whether the catalog publishes no such build at all or publishes
        // it without that Kubernetes version among its compatibilities.
        if let Some(requested) = requested_version.as_deref() {
            if !guard.publishes_addon_build_for(
                &input.addon_name,
                requested,
                &cluster_version,
                now,
            ) {
                return Err(invalid_parameter(
                    state::ADDON_VERSION_NOT_SUPPORTED_MESSAGE.to_string(),
                ));
            }
        }
        // `serviceAccountRoleArn` is judged with the request's own addressing, at the
        // top of this handler: the authorization of a role the request asks Amazon EKS
        // to pass needs nothing from the cluster or the add-on, so it is not re-judged
        // here.
        // "The values that you provide are validated against the schema returned by
        // DescribeAddonConfiguration" — so the document is READ first
        // ([`crate::state::read_addon_configuration`], the one reading this operation
        // shares with the install) and then held to exactly that schema, the one the
        // catalog publishes for the build the add-on will be running once this update
        // completes: the version the request asks for, or the one the add-on already
        // runs when it asks for no new version. Either form of document — JSON or
        // YAML — is held to that schema and the refusal names the format it was read
        // as; one that is neither cannot be read at all and is refused for that. A
        // build this catalog does not publish publishes no schema to validate against.
        let target_version = requested_version
            .clone()
            .unwrap_or_else(|| installed_version.clone());
        let configuration = state::read_addon_configuration(input.configuration_values.as_deref());
        match &configuration {
            state::AddonConfiguration::Unreadable => {
                return Err(invalid_parameter(
                    state::ADDON_CONFIGURATION_UNREADABLE_MESSAGE.to_string(),
                ));
            }
            state::AddonConfiguration::Read { document, .. } => {
                if let Some(violation) = guard
                    .published_addon_build(&input.addon_name, &target_version, now)
                    .and_then(|(catalog_entry, build)| {
                        catalog_entry.configuration_violation(build, document)
                    })
                {
                    return Err(invalid_parameter(
                        state::addon_configuration_violation_message(
                            configuration.format(),
                            &violation,
                        ),
                    ));
                }
            }
            state::AddonConfiguration::Absent => {}
        }

        // LAYER 6 — the EKS Pod Identity associations the add-on would own afterwards.
        // "An array of EKS Pod Identity associations to be updated... If this value is
        // left blank, no change. If an empty array is provided, existing associations
        // owned by the add-on are deleted", so an absent list is not the same request as
        // an empty one, and a list that is present is the whole set the add-on owns once
        // the update completes.
        let requested_associations = input.pod_identity_associations.clone();
        if let Some(requested) = requested_associations.as_deref() {
            // Reported as `InvalidRequestException` rather than the
            // `ResourceLimitExceededException` `CreatePodIdentityAssociation` answers
            // for the same ceiling: `UpdateAddon` does not model that error, and a
            // cluster that cannot hold the associations asked for is a statement about
            // the state of the cluster.
            let too_many = |requested: usize, held: Option<usize>| {
                // The held count is stated only where it was MEASURED: the length-only
                // refusal below returns before the store is walked.
                let held = match held {
                    Some(held) => format!(" and cluster {} already holds {held}", input.cluster_name),
                    None => String::new(),
                };
                invalid_request(format!(
                    "podIdentityAssociations asks for {requested} associations{held}; a cluster \
                     holds at most {MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER} EKS Pod Identity \
                     associations."
                ))
            };
            // The list's LENGTH is refused first, before any per-entry work: the model
            // puts no `length` on `AddonPodIdentityAssociationsList`, so the boundary
            // validator accepts a list of any size, and a list longer than a cluster may
            // ever hold cannot be carried out whatever its entries say. Every loop below
            // therefore runs over at most
            // [`crate::state::MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER`] entries.
            if requested.len() > MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER {
                return Err(too_many(requested.len(), None));
            }
            // Each entry maps a Kubernetes service account to an IAM role, so the role
            // must be a role ARN and the service account must be a name a Kubernetes
            // object can have, and one service account cannot be mapped twice by the
            // same add-on. Both form checks are the ones the pod identity slice already
            // shares.
            let mut seen_service_accounts = ::std::collections::BTreeSet::new();
            for association in requested {
                if !state::is_iam_role_arn(&association.role_arn) {
                    return Err(invalid_parameter(format!(
                        "podIdentityAssociations[].roleArn {:?} is not an IAM role ARN of the \
                         form arn:<partition>:iam::<account>:role/<name>.",
                        association.role_arn
                    )));
                }
                if !state::is_kubernetes_name(&association.service_account) {
                    return Err(invalid_parameter(format!(
                        "podIdentityAssociations[].serviceAccount {:?} is not a Kubernetes \
                         object name (pattern {}).",
                        association.service_account,
                        state::KUBERNETES_NAME_PATTERN
                    )));
                }
                if !seen_service_accounts.insert(association.service_account.clone()) {
                    return Err(invalid_parameter(format!(
                        "podIdentityAssociations names serviceAccount {:?} more than once; each \
                         service account can be mapped at most once.",
                        association.service_account
                    )));
                }
            }
            // The associations THIS add-on owns are the ones the requested list
            // replaces, so they are not counted against the cluster's ceiling and they
            // cannot collide with the request: only an association another owner holds
            // can. Each side of both comparisons is bounded by that ceiling, so the work
            // is the sum of the two sides rather than their product.
            let held_by_others: Vec<(&str, &str, &str)> = guard
                .pod_identity_associations_in(&region, &input.cluster_name)
                .filter(|record| record.owner_arn.as_deref() != Some(addon_arn.as_str()))
                .map(|record| {
                    (
                        record.namespace.as_str(),
                        record.service_account.as_str(),
                        record.role_arn.as_str(),
                    )
                })
                .collect();
            if held_by_others.len() + requested.len() > MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER {
                return Err(too_many(requested.len(), Some(held_by_others.len())));
            }
            // An association for a `(namespace, service account)` pair that already
            // holds one would make the credential lookup that pair resolves ambiguous,
            // which is why the pod identity slice refuses a second association for it;
            // the same refusal belongs here, BEFORE anything is written, so a request
            // that cannot be carried out in full writes nothing. Each taken service
            // account is kept with the IAM role it is already bound to, because that
            // role is what the conflict is reported as
            // ([`crate::state::service_account_already_associated`]).
            let taken: ::std::collections::BTreeMap<&str, &str> = held_by_others
                .iter()
                .filter(|(namespace, _, _)| *namespace == addon_namespace)
                .map(|(_, service_account, role_arn)| (*service_account, *role_arn))
                .collect();
            if let Some(bound_role_arn) = requested.iter().find_map(|association| {
                taken.get(association.service_account.as_str()).copied()
            }) {
                return Err(UpdateAddonErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(input.cluster_name.clone()),
                        addon_name: Some(input.addon_name.clone()),
                        message: Some(state::service_account_already_associated(bound_role_arn)),
                        ..Default::default()
                    },
                )
                .into());
            }
        }

        // "A key-value map that contains the parameters associated with the update":
        // one entry per member of the request that asks for a change, so a caller
        // reading the update back sees what it asked for. A member the request did not
        // carry asks for nothing and is not reported.
        let mut params = Vec::new();
        if let Some(version) = requested_version.as_deref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_ADDON_VERSION.to_string()),
                value: Some(version.to_string()),
            });
        }
        if let Some(role_arn) = input.service_account_role_arn.as_deref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_SERVICE_ACCOUNT_ROLE_ARN.to_string()),
                value: Some(role_arn.to_string()),
            });
        }
        if let Some(values) = input.configuration_values.as_deref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_CONFIGURATION_VALUES.to_string()),
                value: Some(values.to_string()),
            });
        }
        if let Some(requested) = requested_associations.as_deref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_POD_IDENTITY_ASSOCIATIONS.to_string()),
                value: Some(pod_identity_associations_param(requested)),
            });
        }
        if let Some(resolve_conflicts) = input.resolve_conflicts.as_deref() {
            params.push(UpdateParam {
                r#type: Some(UPDATE_PARAM_RESOLVE_CONFLICTS.to_string()),
                value: Some(resolve_conflicts.to_string()),
            });
        }

        let sequence = guard.next_update_sequence();
        let completes_at = now + UPDATE_COMPLETION_DELAY;
        let update_id = state::new_update_id(sequence);
        let update = UpdateRecord {
            id: update_id.clone(),
            r#type: UPDATE_TYPE_ADDON.to_string(),
            status: UPDATE_IN_PROGRESS.to_string(),
            created_at: now,
            params,
            client_request_token: replayed_token.map(str::to_owned),
            succeeds_at: Some(completes_at),
            // The change this update carries is the ADD-ON's, not the cluster's: it is
            // parked on the add-on record below and applied there when the clock
            // completes this update.
            pending_config: None,
            // "This field is present only if any cancellation is attempted for the
            // update": none has been, so `CancelUpdate` is the only writer of either.
            cancellation: None,
            cancellation_request_token: None,
            scope: scope.clone(),
        };
        let answer = update.to_update();

        let server_error = |message: &str| {
            UpdateAddonError::from(UpdateAddonErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.cluster_name.clone()),
                addon_name: Some(input.addon_name.clone()),
                message: Some(message.to_string()),
                ..Default::default()
            }))
        };

        // The update joins its cluster's history through
        // [`crate::state::ClusterRecord::record_update`], which keeps that history
        // bounded; `DescribeUpdate` and `ListUpdates` report it from there.
        let cluster = guard
            .clusters
            .get_mut(&cluster_key)
            .ok_or_else(|| server_error("the cluster record disappeared while updating its add-on"))?;
        cluster.record_update(update);

        // The add-on is under the update until the clock completes it, and what it
        // reports changes only then — the update is asynchronous, so none of the
        // requested values is visible on the add-on the instant it is requested.
        let record = guard
            .addons
            .get_mut(&addon_key)
            .ok_or_else(|| server_error("the add-on record disappeared while updating it"))?;
        record.status = ADDON_STATUS_UPDATING.to_string();
        record.modified_at = now;
        record.active_at = Some(completes_at);
        record.pending_update = Some(PendingAddonUpdate {
            update_id,
            addon_version: requested_version,
            service_account_role_arn: input.service_account_role_arn.clone(),
            configuration_values: input.configuration_values.clone(),
            pod_identity_associations: requested_associations,
        });

        ::tracing::info!(
            operation = "UpdateAddon",
            cluster = %input.cluster_name,
            addon = %input.addon_name,
            from_version = %installed_version,
            to_version = %target_version,
            "started an add-on update"
        );

        Ok(UpdateAddonOutput {
            update: Some(answer),
        })
    }
}
