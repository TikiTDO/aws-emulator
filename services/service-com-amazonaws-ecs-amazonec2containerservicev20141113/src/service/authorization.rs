// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it names — the decision
//! taken ahead of every mutating operation, before any handler resolves the cluster, the
//! service or any other resource the request addresses.
//!
//! `AccessDeniedException` (HTTP 403) is what AWS refuses a caller whose policies do not
//! cover the action it asked for with ("You don't have authorization to perform the
//! requested action", the shape's own doc), and the MESSAGE a refusal carries is IAM's own
//! clause, built from the request and the stored policies by the shared
//! [`::service_common::iam_authorization`]. AWS takes that decision at the IAM layer, AHEAD
//! of the service looking up what the request named, so the refusal is the same whether or
//! not the cluster in the request exists and a refused write stores nothing — which is why
//! it is taken once, before dispatch ([`super::request_pipeline`]), rather than inside each
//! handler.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one the
//!    caller reads back for the credentials it signed with), else the IAM user that holds
//!    the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a document `iam:PutRolePolicy` wrote takes effect on
//!    the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal ARN
//! does (that is [`::service_common::iam_role`]). The lookups stay in this crate because
//! hoisting them into `service-common` would make every service depending on that crate link
//! the IAM and STS service crates.
//!
//! ## The action and the resource
//!
//! The action is `ecs:<OperationName>` ("Actions defined by Amazon Elastic Container
//! Service", IAM Service Authorization Reference), whose prefix is this service's
//! `arnNamespace` and its signing name ([`ACTION_PREFIX`]) — the same string every ARN this
//! service mints carries, so one statement scoped to this service's actions covers every
//! operation of it and nothing outside it.
//!
//! The resource is the ARN of the resource the REQUEST addresses ([`judged_resource`]),
//! built by this crate's own ARN builders from the names the request carried: the cluster
//! for a call addressed to a cluster, the service for one addressed to a service or to one
//! of its task sets, the container instance, the task, the capacity provider, the ARN the
//! caller supplied for a tagging call or a service-deployment call, and
//! [`EVERY_RESOURCE`] for an action IAM publishes no resource type for. Taking the
//! identifiers from the REQUEST rather than from a stored record is also what keeps a
//! refused call from telling an unauthorized caller whether what it named exists.
//!
//! ## Which operations this decides, and why only those
//!
//! The operations that MUTATE this account's Amazon ECS resources, and whose addressed
//! resource the request itself determines. An operation addressed to a LIST of resources
//! (the container-instance state update, the task-protection update, the task-definition
//! delete), or to a task-definition reference that need not name a revision (the two
//! launches and the deregistration, where "family" alone means whichever revision is latest
//! at the time), is NOT decided here ([`judged_resource`] answers `None`): the request does
//! not determine the ONE ARN IAM would judge such a call against, and withholding a grant
//! from it would refuse calls AWS permits — a policy scoped to that resource grants the
//! action on it. Those are left to a later change that can name every resource they address,
//! and are served exactly as before, as are this service's reporting operations and the
//! agent-plane state submissions.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the account's store may hold no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses.
//!
//! An expired session credential speaks for nobody: it carries no document, so every action
//! it asks for falls to the default deny, and the refusal names that state rather than
//! quoting the key it presented back.

use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::ServerHttpProtocol;

use crate::ops::types::*;
use crate::service::read_input;

/// This service's IAM action prefix: an action is `ecs:<OperationName>`, which is this
/// service's `arnNamespace` and its signing name
/// (`smithy_query(query="service_info")`).
pub const ACTION_PREFIX: &str = "ecs";

/// The resource an action IAM publishes no resource type for is judged against, as IAM
/// writes it for such a request: every resource.
pub const EVERY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// What stands in a task-definition ARN for the revision a REGISTRATION does not name:
/// every revision of the family.
///
/// A revision ARN ends in the number the family's counter hands out
/// ([`crate::state::task_definition_arn`]), and a registration is judged before any number
/// is allocated — so what IAM is asked about is the family whichever revision the call would
/// mint, which is also how a policy statement scoped to a family is written.
const EVERY_REVISION: &str = "*";

/// The ARN of the resource the request `operation` names addresses, which is what IAM judges
/// the call against — or `None` for an operation this gate does not decide (see the module
/// docs).
///
/// Every identifier is read out of the request through the SAME protocol and generated input
/// schema the dispatch sequence will hand the handler ([`read_input`]), so this gate cannot
/// judge a different value from the one the operation acts on; a body that cannot be read at
/// all is no decision here, and the framework's own walk refuses it.
pub fn judged_resource(
    operation: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    region: &str,
) -> Option<String> {
    let cluster = |reference: Option<&str>| {
        crate::state::cluster_arn(region, &crate::state::cluster_name_from_reference(reference))
    };
    // A service is named under the cluster that hosts it, and a full service ARN carries
    // that cluster itself — so the ARN the caller supplied wins over the `cluster` member,
    // exactly as every handler of this crate resolves the pair.
    let service = |cluster_reference: Option<&str>, service_reference: &str| {
        let (from_arn, service_name) =
            crate::state::cluster_and_service_from_reference(service_reference);
        let cluster_name = match from_arn {
            Some(name) => name,
            None => crate::state::cluster_name_from_reference(cluster_reference),
        };
        crate::state::service_arn(region, &cluster_name, &service_name)
    };
    match operation {
        // The task-definition FAMILY the registration addresses: the revision an accepted
        // registration would mint does not exist yet, so the resource IAM is asked about
        // names the family with every revision of it
        // ([`EVERY_REVISION`]).
        "RegisterTaskDefinition" => read_input::<RegisterTaskDefinitionInput>(
            protocol,
            REGISTER_TASK_DEFINITION_INPUT_SCHEMA,
            req,
        )
        .map(|input| {
            ::engine::emulator_arn(
                crate::state::ARN_NAMESPACE,
                region,
                &format!(
                    "{}/{}:{EVERY_REVISION}",
                    crate::state::TASK_DEFINITION_RESOURCE_TYPE,
                    input.family
                ),
            )
        }),
        // Every resource: an action IAM publishes no resource type for, so a statement
        // about it names every resource.
        "PutAccountSetting" | "PutAccountSettingDefault" | "DeleteAccountSetting" => {
            Some(EVERY_RESOURCE.to_string())
        }
        // The cluster the request addresses.
        "CreateCluster" => {
            read_input::<CreateClusterInput>(protocol, CREATE_CLUSTER_INPUT_SCHEMA, req)
                .map(|input| cluster(input.cluster_name.as_deref()))
        }
        "DeleteCluster" => {
            read_input::<DeleteClusterInput>(protocol, DELETE_CLUSTER_INPUT_SCHEMA, req)
                .map(|input| cluster(Some(&input.cluster)))
        }
        "UpdateCluster" => {
            read_input::<UpdateClusterInput>(protocol, UPDATE_CLUSTER_INPUT_SCHEMA, req)
                .map(|input| cluster(Some(&input.cluster)))
        }
        "UpdateClusterSettings" => read_input::<UpdateClusterSettingsInput>(
            protocol,
            UPDATE_CLUSTER_SETTINGS_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(Some(&input.cluster))),
        "PutClusterCapacityProviders" => read_input::<PutClusterCapacityProvidersInput>(
            protocol,
            PUT_CLUSTER_CAPACITY_PROVIDERS_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(Some(&input.cluster))),
        // An attribute write and a container-instance registration are addressed to the
        // CLUSTER they are scoped to, which is the resource of this service the request
        // names.
        "PutAttributes" => {
            read_input::<PutAttributesInput>(protocol, PUT_ATTRIBUTES_INPUT_SCHEMA, req)
                .map(|input| cluster(input.cluster.as_deref()))
        }
        "DeleteAttributes" => {
            read_input::<DeleteAttributesInput>(protocol, DELETE_ATTRIBUTES_INPUT_SCHEMA, req)
                .map(|input| cluster(input.cluster.as_deref()))
        }
        "RegisterContainerInstance" => read_input::<RegisterContainerInstanceInput>(
            protocol,
            REGISTER_CONTAINER_INSTANCE_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(input.cluster.as_deref())),
        // The service the request addresses — including the calls addressed to one of its
        // task sets, which IAM judges on the service that holds them.
        "CreateService" => {
            read_input::<CreateServiceInput>(protocol, CREATE_SERVICE_INPUT_SCHEMA, req)
                .map(|input| service(input.cluster.as_deref(), &input.service_name))
        }
        "UpdateService" => {
            read_input::<UpdateServiceInput>(protocol, UPDATE_SERVICE_INPUT_SCHEMA, req)
                .map(|input| service(input.cluster.as_deref(), &input.service))
        }
        "DeleteService" => {
            read_input::<DeleteServiceInput>(protocol, DELETE_SERVICE_INPUT_SCHEMA, req)
                .map(|input| service(input.cluster.as_deref(), &input.service))
        }
        "CreateTaskSet" => {
            read_input::<CreateTaskSetInput>(protocol, CREATE_TASK_SET_INPUT_SCHEMA, req)
                .map(|input| service(Some(&input.cluster), &input.service))
        }
        "UpdateTaskSet" => {
            read_input::<UpdateTaskSetInput>(protocol, UPDATE_TASK_SET_INPUT_SCHEMA, req)
                .map(|input| service(Some(&input.cluster), &input.service))
        }
        "DeleteTaskSet" => {
            read_input::<DeleteTaskSetInput>(protocol, DELETE_TASK_SET_INPUT_SCHEMA, req)
                .map(|input| service(Some(&input.cluster), &input.service))
        }
        "UpdateServicePrimaryTaskSet" => read_input::<UpdateServicePrimaryTaskSetInput>(
            protocol,
            UPDATE_SERVICE_PRIMARY_TASK_SET_INPUT_SCHEMA,
            req,
        )
        .map(|input| service(Some(&input.cluster), &input.service)),
        // ONE container instance of one cluster, addressed by the id or the ARN the request
        // carries.
        "DeregisterContainerInstance" => read_input::<DeregisterContainerInstanceInput>(
            protocol,
            DEREGISTER_CONTAINER_INSTANCE_INPUT_SCHEMA,
            req,
        )
        .map(|input| {
            crate::state::container_instance_arn_from_reference(
                region,
                &crate::state::cluster_name_from_reference(input.cluster.as_deref()),
                &input.container_instance,
            )
        }),
        "UpdateContainerAgent" => read_input::<UpdateContainerAgentInput>(
            protocol,
            UPDATE_CONTAINER_AGENT_INPUT_SCHEMA,
            req,
        )
        .map(|input| {
            crate::state::container_instance_arn_from_reference(
                region,
                &crate::state::cluster_name_from_reference(input.cluster.as_deref()),
                &input.container_instance,
            )
        }),
        // ONE task of one cluster, addressed by the id or the ARN the request carries.
        "StopTask" => read_input::<StopTaskInput>(protocol, STOP_TASK_INPUT_SCHEMA, req).map(
            |input| {
                crate::state::task_arn_from_reference(
                    region,
                    &crate::state::cluster_name_from_reference(input.cluster.as_deref()),
                    &input.task,
                )
            },
        ),
        "ExecuteCommand" => {
            read_input::<ExecuteCommandInput>(protocol, EXECUTE_COMMAND_INPUT_SCHEMA, req).map(
                |input| {
                    crate::state::task_arn_from_reference(
                        region,
                        &crate::state::cluster_name_from_reference(input.cluster.as_deref()),
                        &input.task,
                    )
                },
            )
        }
        // ONE capacity provider of the account, addressed by the name the request carries.
        "CreateCapacityProvider" => read_input::<CreateCapacityProviderInput>(
            protocol,
            CREATE_CAPACITY_PROVIDER_INPUT_SCHEMA,
            req,
        )
        .map(|input| crate::state::capacity_provider_arn(region, &input.name)),
        "UpdateCapacityProvider" => read_input::<UpdateCapacityProviderInput>(
            protocol,
            UPDATE_CAPACITY_PROVIDER_INPUT_SCHEMA,
            req,
        )
        .map(|input| crate::state::capacity_provider_arn(region, &input.name)),
        "DeleteCapacityProvider" => read_input::<DeleteCapacityProviderInput>(
            protocol,
            DELETE_CAPACITY_PROVIDER_INPUT_SCHEMA,
            req,
        )
        .map(|input| {
            crate::state::capacity_provider_arn(
                region,
                &crate::state::capacity_provider_name_from_reference(&input.capacity_provider),
            )
        }),
        // The resource the caller named, which these calls carry as an ARN already.
        "TagResource" => read_input::<TagResourceInput>(protocol, TAG_RESOURCE_INPUT_SCHEMA, req)
            .map(|input| input.resource_arn),
        "UntagResource" => {
            read_input::<UntagResourceInput>(protocol, UNTAG_RESOURCE_INPUT_SCHEMA, req)
                .map(|input| input.resource_arn)
        }
        "StopServiceDeployment" => read_input::<StopServiceDeploymentInput>(
            protocol,
            STOP_SERVICE_DEPLOYMENT_INPUT_SCHEMA,
            req,
        )
        .map(|input| input.service_deployment_arn),
        "ContinueServiceDeployment" => read_input::<ContinueServiceDeploymentInput>(
            protocol,
            CONTINUE_SERVICE_DEPLOYMENT_INPUT_SCHEMA,
            req,
        )
        .map(|input| input.service_deployment_arn),
        // Everything else is reported, agent-plane, or addressed to a resource the request
        // does not determine the ARN of (see the module docs).
        _ => None,
    }
}

/// Whether IAM lets this request perform `operation`'s action on `resource`.
///
/// `Ok(())` for a request IAM permits and for every caller this gate does not bound (see the
/// module docs); the modelled `AccessDeniedException` otherwise. It is asked BEFORE anything
/// the request names is resolved, so a denial says nothing about what the account holds.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    // An identity-based document states no `Principal` element, so the decision rests on the
    // action and the resource the request addressed.
    let said = iam::identity_decision(&caller.documents, &iam::Request::new(&action, resource, &[]));
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not a
    // permission the principal has. The account's own credential carries none.
    let Some(boundary) = caller.boundary.as_ref() else {
        return Ok(());
    };
    let reason = match iam::identity_decision(
        ::std::slice::from_ref(boundary),
        &iam::Request::new(&action, resource, &[]),
    ) {
        Decision::Allowed => return Ok(()),
        // An explicit deny is a fact about the boundary document, so the refusal names it; a
        // boundary that merely states nothing withheld the grant the identity-based policies
        // gave.
        Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
        Decision::NotStated => iam::no_permissions_boundary_allows(&action),
    };
    Err(not_authorized(&caller.arn, &action, resource, &reason))
}

/// The refusal IAM states an authorization failure with — who was refused, which action, on
/// which resource, and which of IAM's rules refused it — as the modelled error this service
/// answers it in.
///
/// An action IAM publishes no resource type for is judged against [`EVERY_RESOURCE`], and
/// the sentence states no resource clause for it: there is nothing to name. Every other one
/// carries the shared [`iam::not_authorized_by_identity_policy`], so nothing of IAM's
/// wording is spelled here; every value in the sentence comes from the request and from the
/// stored policies.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    let message = if resource == EVERY_RESOURCE {
        format!("User: {principal} is not authorized to perform: {action} {reason}")
    } else {
        iam::not_authorized_by_identity_policy(principal, action, resource, reason)
    };
    AccessDeniedException {
        message: Some(message),
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due before it answers: that write is owed at
/// the same instant whether or not anybody called.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::service_common::request_access_key_id(ctx.parts());
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which holds no identity policy.
        None => Ok(sts_store::ambient_identity()),
    };
    // An expired session credential speaks for nobody: it carries no document, so every
    // action it asks for is refused — and the refusal names the credential's STATE rather
    // than the key id that was presented, which this module never quotes back.
    let identity = match identity {
        Ok(identity) => identity,
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                ..Caller::default()
            };
        }
    };
    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        arn: identity.arn,
        ..Caller::default()
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it (the map `iam:GetUser` resolves a caller through), so a request signed
    // with one speaks for THAT user and is judged on the user's policies. A key the IAM store
    // does not hold either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about carries this service's own prefix, so a
    /// statement scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Deny","Action":"ecs:*","Resource":"*"}]}"#;
        for operation in [
            "CreateCluster",
            "DeleteCluster",
            "UpdateService",
            "RegisterTaskDefinition",
            "TagResource",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, EVERY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Denied,
                "case {operation:?}: expected ecs:* to deny {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "eks:DescribeCluster", EVERY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected ecs:* to state nothing outside this service, \
             got {outside:?}"
        );
    }

    /// RULE: an explicit `Deny` about the action is final, whatever else the principal
    /// carries, and it reaches only the action it names.
    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let document = r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"*","Resource":"*"},
            {"Effect":"Deny","Action":["ecs:CreateCluster","ecs:DeleteCluster"],
             "Resource":"*"}]}"#;
        let resource = crate::state::cluster_arn("us-west-2", "denied");
        for operation in ["CreateCluster", "DeleteCluster"] {
            let said = decision(document, &format!("{ACTION_PREFIX}:{operation}"), &resource);
            assert_eq!(
                said,
                Decision::Denied,
                "case {operation:?}: expected the deny to win, got {said:?}"
            );
        }
        let allowed = decision(
            document,
            &format!("{ACTION_PREFIX}:CreateService"),
            &resource,
        );
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }

    /// RULE: the refusal names the principal, the action, the resource the request addressed
    /// and which of IAM's rules refused it — every part taken from the request and the stored
    /// policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:sts::{}:assumed-role/ecs-deployer/session-1",
            ::engine::emulator_account_id()
        );
        let action = format!("{ACTION_PREFIX}:CreateCluster");
        let resource = crate::state::cluster_arn("us-west-2", "a-cluster");
        // (case, the clause the decision closed with)
        let rows: &[(&str, String)] = &[
            (
                "an explicit deny",
                iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string(),
            ),
            ("no statement at all", iam::no_identity_policy_allows(&action)),
            (
                "a permissions boundary that states nothing",
                iam::no_permissions_boundary_allows(&action),
            ),
        ];
        for (case, reason) in rows {
            let refusal = not_authorized(&principal, &action, &resource, reason);
            assert_eq!(
                refusal.message,
                Some(iam::not_authorized_by_identity_policy(
                    &principal, &action, &resource, reason
                )),
                "case {case:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
        // An action IAM publishes no resource type for names no resource in the sentence.
        let every = not_authorized(
            &principal,
            &action,
            EVERY_RESOURCE,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        );
        assert_eq!(
            every.message,
            Some(format!(
                "User: {principal} is not authorized to perform: {action} {}",
                iam::IDENTITY_POLICY_EXPLICIT_DENY
            )),
            "case \"every resource\": expected no resource clause, got {:?}",
            every.message
        );
    }
}
