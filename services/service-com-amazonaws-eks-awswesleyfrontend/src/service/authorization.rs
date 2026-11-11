// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it names — the decision
//! taken ahead of every operation, before any handler resolves the cluster, the capability
//! or any other resource the request addresses.
//!
//! `AccessDeniedException` (HTTP 403) is what AWS refuses a caller whose policies do not
//! cover the action it asked for with; the shape's own model documentation states the
//! general sentence ("You don't have permissions to perform the requested operation. The
//! IAM principal making the request must have at least one IAM permissions policy attached
//! that grants the required permissions"), while the MESSAGE a refusal carries is IAM's own
//! clause, built below from the request and the stored policies
//! ([`not_authorized`]). AWS takes that decision at the IAM layer, AHEAD of the service
//! looking up what the request named, so the refusal is the same whether or not the cluster
//! in the request exists and a refused write stores nothing. That is why it is taken once,
//! before dispatch ([`super::request_pipeline`]), rather than inside each handler.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    holds the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a document `iam:PutRolePolicy` wrote
//!    takes effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal
//! ARN does (that is [`::service_common::iam_role`]). The lookups stay in this crate
//! because hoisting them into `service-common` would make every service depending on that
//! crate link the IAM and STS service crates.
//!
//! ## The action and the resource
//!
//! The action is `eks:<OperationName>` ("Actions defined by Amazon EKS", IAM Service
//! Authorization Reference), which is this service's `arnNamespace` and its signing name
//! ([`ACTION_PREFIX`]) — so one statement scoped to this service's actions covers every
//! operation of it and nothing outside it.
//!
//! The resource is the ARN of the resource the REQUEST addresses
//! ([`judged_resource`]): the cluster for an operation addressed to a cluster — including
//! the operations that create, list or register one of its children, which IAM judges on
//! the cluster — the capability for the operations addressed to one capability
//! ([`addressed_capability`]), the ARN the caller supplied for the tagging operations, the
//! subscription for the operations addressed to one subscription, and every resource
//! ([`EVERY_RESOURCE`]) for an operation that addresses no resource of this service at
//! all. Taking the identifiers from the REQUEST rather than from a stored record is also
//! what keeps a refused call from telling an unauthorized caller whether what it named
//! exists.
//!
//! ## Which operations this decides, and why only those
//!
//! An operation addressed to a CHILD resource whose ARN carries an identifier only this
//! service mints at create time — a node group, an add-on, a Fargate profile, an access
//! entry, a Pod Identity association, a certificate authority, an insight — is NOT decided
//! here ([`judged_resource`] answers `None`), because the request does not determine the
//! ARN IAM judges such a call against: it carries the child's NAME, while the ARN carries
//! the minted id as well. Withholding a grant from such a request here would refuse calls
//! AWS permits — a policy scoped to that child's ARN grants the action on it — so those
//! operations are left to a later change that can name the resource they address, and are
//! served exactly as before.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the account's store may hold no
//!   document for it; refusing on a grant this emulator cannot see would deny access AWS
//!   permits. An explicit `Deny` in a document that CAN be read still refuses, and a
//!   permissions boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which first materialises
//! the session expiries the clock has already made due; that write is owed at the same
//! instant whether or not anybody called, which is why a REPORTING operation of this
//! service may take this decision too.

use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::ServerHttpProtocol;

use crate::ops::types::*;
use crate::service::read_input;

/// This service's IAM action prefix: an action is `eks:<OperationName>`, which is this
/// service's `arnNamespace` and its signing name
/// (`smithy_query(query="service_info")`).
pub const ACTION_PREFIX: &str = "eks";

/// The resource an operation that addresses no resource of this service is judged
/// against, as IAM writes it for such a request: every resource.
pub const EVERY_RESOURCE: &str = "*";

/// What stands in an ARN for the identifier segment a request does not name: every
/// identifier.
///
/// A nested Amazon EKS resource ARN ends in an identifier the service mints when the
/// resource is created (`nodegroup/<cluster>/<name>/<id>` is the shape every one of them
/// takes). A request names the parent and the resource's own name and never that
/// identifier, and the decision below is taken before anything is resolved, so what IAM is
/// asked about is the resource of that parent carrying that name whichever identifier it
/// was minted with. Recorded behavior: that is the resource the refusal of a request
/// addressed to one capability names.
const EVERY_IDENTIFIER: &str = "*";

/// The ARN IAM is asked about for a request addressed to ONE capability of one cluster:
/// this service's capability ARN ([`crate::state::capability_arn`]), built from the two
/// names the request carries, with [`EVERY_IDENTIFIER`] in the position the minted
/// identifier occupies.
///
/// This service's own capability ARN carries no identifier segment yet, so a statement
/// scoped to the ARN `DescribeCapability` REPORTS does not match this resource — a policy
/// has to name the capability by `capability/<cluster>/<name>/*` (or a wider pattern) to
/// reach it, which is also how a statement naming AWS's own capability ARN is written. The
/// recorded refusal is what settles which of the two this gate asks about; making the
/// stored ARN carry an identifier of its own is the follow-up that would make the two
/// spellings one, and it changes what every capability read reports, so it is not done
/// here.
fn addressed_capability(region: &str, cluster_name: &str, capability_name: &str) -> String {
    format!(
        "{}/{EVERY_IDENTIFIER}",
        crate::state::capability_arn(region, cluster_name, capability_name)
    )
}

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The ARN of the resource the request `operation` names addresses, which is what IAM
/// judges the call against — or `None` for an operation this gate does not decide (see the
/// module docs).
///
/// Every identifier is read out of the request through the SAME protocol and generated
/// input schema the dispatch sequence will hand the handler ([`read_input`]), so this gate
/// cannot judge a different value from the one the operation acts on; a body that cannot be
/// read at all is no decision here, and the framework's own walk refuses it.
pub fn judged_resource(
    operation: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    region: &str,
) -> Option<String> {
    let cluster = |name: &str| ::engine::emulator_arn("eks", region, &format!("cluster/{name}"));
    match operation {
        // Every resource: the operation addresses nothing of this account — a listing of
        // the account's clusters or subscriptions, or a read of a catalogue Amazon EKS
        // publishes for the whole service.
        "ListClusters"
        | "ListEksAnywhereSubscriptions"
        | "ListAccessPolicies"
        | "DescribeClusterVersions"
        | "DescribeAddonVersions"
        | "DescribeAddonConfiguration" => Some(EVERY_RESOURCE.to_string()),
        // The cluster, addressed by `name`.
        "CreateCluster" => {
            read_input::<CreateClusterInput>(protocol, CREATE_CLUSTER_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.name))
        }
        "DeleteCluster" => {
            read_input::<DeleteClusterInput>(protocol, DELETE_CLUSTER_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.name))
        }
        "DescribeCluster" => {
            read_input::<DescribeClusterInput>(protocol, DESCRIBE_CLUSTER_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.name))
        }
        "UpdateClusterConfig" => read_input::<UpdateClusterConfigInput>(
            protocol,
            UPDATE_CLUSTER_CONFIG_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.name)),
        "UpdateClusterVersion" => read_input::<UpdateClusterVersionInput>(
            protocol,
            UPDATE_CLUSTER_VERSION_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.name)),
        "DescribeUpdate" => {
            read_input::<DescribeUpdateInput>(protocol, DESCRIBE_UPDATE_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.name))
        }
        "ListUpdates" => read_input::<ListUpdatesInput>(protocol, LIST_UPDATES_INPUT_SCHEMA, req)
            .map(|input| cluster(&input.name)),
        "CancelUpdate" => {
            read_input::<CancelUpdateInput>(protocol, CANCEL_UPDATE_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.name))
        }
        // A registration occupies a cluster name exactly as a created cluster does, so
        // both calls are judged on that cluster's ARN.
        "RegisterCluster" => {
            read_input::<RegisterClusterInput>(protocol, REGISTER_CLUSTER_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.name))
        }
        "DeregisterCluster" => {
            read_input::<DeregisterClusterInput>(protocol, DEREGISTER_CLUSTER_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.name))
        }
        // The cluster, addressed by `clusterName`: the operations that create, list or
        // configure what the cluster holds are judged on the CLUSTER, which is the
        // resource the request addresses.
        "AssociateEncryptionConfig" => read_input::<AssociateEncryptionConfigInput>(
            protocol,
            ASSOCIATE_ENCRYPTION_CONFIG_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "AssociateIdentityProviderConfig" => read_input::<AssociateIdentityProviderConfigInput>(
            protocol,
            ASSOCIATE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "DisassociateIdentityProviderConfig" => {
            read_input::<DisassociateIdentityProviderConfigInput>(
                protocol,
                DISASSOCIATE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA,
                req,
            )
            .map(|input| cluster(&input.cluster_name))
        }
        "DescribeIdentityProviderConfig" => read_input::<DescribeIdentityProviderConfigInput>(
            protocol,
            DESCRIBE_IDENTITY_PROVIDER_CONFIG_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "ListIdentityProviderConfigs" => read_input::<ListIdentityProviderConfigsInput>(
            protocol,
            LIST_IDENTITY_PROVIDER_CONFIGS_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "CreateAccessEntry" => {
            read_input::<CreateAccessEntryInput>(protocol, CREATE_ACCESS_ENTRY_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.cluster_name))
        }
        "ListAccessEntries" => {
            read_input::<ListAccessEntriesInput>(protocol, LIST_ACCESS_ENTRIES_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.cluster_name))
        }
        "CreateAddon" => read_input::<CreateAddonInput>(protocol, CREATE_ADDON_INPUT_SCHEMA, req)
            .map(|input| cluster(&input.cluster_name)),
        "ListAddons" => read_input::<ListAddonsInput>(protocol, LIST_ADDONS_INPUT_SCHEMA, req)
            .map(|input| cluster(&input.cluster_name)),
        "CreateCapability" => {
            read_input::<CreateCapabilityInput>(protocol, CREATE_CAPABILITY_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.cluster_name))
        }
        "ListCapabilities" => {
            read_input::<ListCapabilitiesInput>(protocol, LIST_CAPABILITIES_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.cluster_name))
        }
        "CreateCertificateAuthority" => read_input::<CreateCertificateAuthorityInput>(
            protocol,
            CREATE_CERTIFICATE_AUTHORITY_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "ListCertificateAuthorities" => read_input::<ListCertificateAuthoritiesInput>(
            protocol,
            LIST_CERTIFICATE_AUTHORITIES_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "CreateFargateProfile" => read_input::<CreateFargateProfileInput>(
            protocol,
            CREATE_FARGATE_PROFILE_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "ListFargateProfiles" => read_input::<ListFargateProfilesInput>(
            protocol,
            LIST_FARGATE_PROFILES_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "CreateNodegroup" => {
            read_input::<CreateNodegroupInput>(protocol, CREATE_NODEGROUP_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.cluster_name))
        }
        "ListNodegroups" => {
            read_input::<ListNodegroupsInput>(protocol, LIST_NODEGROUPS_INPUT_SCHEMA, req)
                .map(|input| cluster(&input.cluster_name))
        }
        "CreatePodIdentityAssociation" => read_input::<CreatePodIdentityAssociationInput>(
            protocol,
            CREATE_POD_IDENTITY_ASSOCIATION_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "ListPodIdentityAssociations" => read_input::<ListPodIdentityAssociationsInput>(
            protocol,
            LIST_POD_IDENTITY_ASSOCIATIONS_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "ListInsights" => read_input::<ListInsightsInput>(protocol, LIST_INSIGHTS_INPUT_SCHEMA, req)
            .map(|input| cluster(&input.cluster_name)),
        "StartInsightsRefresh" => read_input::<StartInsightsRefreshInput>(
            protocol,
            START_INSIGHTS_REFRESH_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        "DescribeInsightsRefresh" => read_input::<DescribeInsightsRefreshInput>(
            protocol,
            DESCRIBE_INSIGHTS_REFRESH_INPUT_SCHEMA,
            req,
        )
        .map(|input| cluster(&input.cluster_name)),
        // ONE capability of a cluster ([`addressed_capability`]).
        "DescribeCapability" => {
            read_input::<DescribeCapabilityInput>(protocol, DESCRIBE_CAPABILITY_INPUT_SCHEMA, req)
                .map(|input| {
                    addressed_capability(region, &input.cluster_name, &input.capability_name)
                })
        }
        "DeleteCapability" => {
            read_input::<DeleteCapabilityInput>(protocol, DELETE_CAPABILITY_INPUT_SCHEMA, req).map(
                |input| addressed_capability(region, &input.cluster_name, &input.capability_name),
            )
        }
        "UpdateCapability" => {
            read_input::<UpdateCapabilityInput>(protocol, UPDATE_CAPABILITY_INPUT_SCHEMA, req).map(
                |input| addressed_capability(region, &input.cluster_name, &input.capability_name),
            )
        }
        // The resource the caller named, which a tagging call carries as an ARN already.
        "TagResource" => read_input::<TagResourceInput>(protocol, TAG_RESOURCE_INPUT_SCHEMA, req)
            .map(|input| input.resource_arn),
        "UntagResource" => {
            read_input::<UntagResourceInput>(protocol, UNTAG_RESOURCE_INPUT_SCHEMA, req)
                .map(|input| input.resource_arn)
        }
        "ListTagsForResource" => read_input::<ListTagsForResourceInput>(
            protocol,
            LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
            req,
        )
        .map(|input| input.resource_arn),
        // ONE EKS Anywhere subscription, whose ARN is derived from the id the request
        // carries ([`crate::state::eks_anywhere_subscription_arn`]).
        "DescribeEksAnywhereSubscription" => read_input::<DescribeEksAnywhereSubscriptionInput>(
            protocol,
            DESCRIBE_EKS_ANYWHERE_SUBSCRIPTION_INPUT_SCHEMA,
            req,
        )
        .map(|input| crate::state::eks_anywhere_subscription_arn(region, &input.id)),
        "DeleteEksAnywhereSubscription" => read_input::<DeleteEksAnywhereSubscriptionInput>(
            protocol,
            DELETE_EKS_ANYWHERE_SUBSCRIPTION_INPUT_SCHEMA,
            req,
        )
        .map(|input| crate::state::eks_anywhere_subscription_arn(region, &input.id)),
        "UpdateEksAnywhereSubscription" => read_input::<UpdateEksAnywhereSubscriptionInput>(
            protocol,
            UPDATE_EKS_ANYWHERE_SUBSCRIPTION_INPUT_SCHEMA,
            req,
        )
        .map(|input| crate::state::eks_anywhere_subscription_arn(region, &input.id)),
        // Everything else addresses a child resource whose ARN the request does not
        // determine (see the module docs).
        _ => None,
    }
}

/// Whether IAM lets this request perform `operation`'s action on `resource`.
///
/// `Ok(())` for a request IAM permits and for every caller this gate does not bound (see
/// the module docs); the modelled `AccessDeniedException` otherwise. It is asked BEFORE
/// anything the request names is resolved, so a denial says nothing about what the account
/// holds.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it.
///
/// Every value in it comes from the request and from the stored policies — the principal
/// from the credential the request presented, the action from the operation it named, the
/// resource from what it addressed.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action} on resource: {resource} \
             {reason}"
        )),
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
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
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
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
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"eks:*","Resource":"*"}]}"#;
        for operation in [
            "RegisterCluster",
            "DeregisterCluster",
            "CreateCapability",
            "DescribeCapability",
            "DeleteCapability",
            "ListCapabilities",
            "DescribeCluster",
            "ListClusters",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, EVERY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected eks:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "ec2:DescribeSubnets", EVERY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected eks:* to grant nothing outside this service, \
             got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused it — every part taken from the request
    /// and the stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:sts::{}:assumed-role/eks-reader/session-1",
            ::engine::emulator_account_id()
        );
        let action = format!("{ACTION_PREFIX}:DeleteCapability");
        let resource = addressed_capability("us-west-2", "a-cluster", "a-capability");
        // (case, the clause the decision closed with)
        let rows: &[(&str, String)] = &[
            (
                "an explicit deny",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "no statement at all",
                format!("because no identity-based policy allows the {action} action"),
            ),
            (
                "a permissions boundary that states nothing",
                format!("because no permissions boundary allows the {action} action"),
            ),
        ];
        for (case, reason) in rows {
            let refusal = not_authorized(&principal, &action, &resource, reason);
            assert_eq!(
                refusal.message,
                Some(format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {resource} {reason}"
                )),
                "case {case:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
    }
}
