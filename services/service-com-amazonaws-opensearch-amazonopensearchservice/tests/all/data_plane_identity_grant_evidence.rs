// SPDX-License-Identifier: CC0-1.0

//! The FIXTURE the sibling suite [`super::data_plane_unreadable_identity_grant`] decides its
//! requests against: one domain whose access policy names a separate control role, and a set
//! of principals differing only in WHERE their grant of this service's data-plane actions
//! lives and what this emulator can show about it.
//!
//! It lives beside that suite rather than inside it for the reason
//! [`super::published_domain_address`] holds its session and address helpers: the seeding is
//! parameterised by what a principal CARRIES, which is the one axis the rule generalises
//! over, so the rows and the seeding are stated once and the suite states only the verdicts.
//!
//! Nothing here asserts anything; the rule and its rows are documented on the suite.

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

use super::domain_lifecycle::{Emulator, REGION, SETTLE_WAIT};
use super::published_domain_address::{
    given_role_policy, given_session, published_endpoint, role_arn,
};

/// The domain every row writes to.
pub(crate) const DOMAIN: &str = "grant-evidence-domain";

/// The index every probe writes into.
pub(crate) const INDEX: &str = "grants";

/// The session name every seeded credential carries.
pub(crate) const SESSION: &str = "grant-session";

/// The role the domain's access policy NAMES — a separate control role, so the domain's own
/// policy says nothing about the principals under test and every verdict is decided on the
/// identity side.
pub(crate) const CONTROL_ROLE: &str = "grant-control";

/// The access key id the control role's seeded session carries.
pub(crate) const CONTROL_KEY: &str = "ASIAEXAMPLECONTROL01";

/// An AWS-managed policy's ARN: a policy AWS owns, whose document this emulator publishes no
/// catalog entry for, so what it grants cannot be read here.
pub(crate) const AWS_MANAGED_POLICY_ARN: &str =
    "arn:aws:iam::aws:policy/AmazonOpenSearchServiceFullAccess";

/// The clause a refusal carries when a document that CAN be read explicitly denies the
/// action.
pub(crate) const EXPLICIT_IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";

/// The clause it carries when the domain holds an access policy and nothing in it allows the
/// request.
pub(crate) fn nothing_allows(action: &str) -> String {
    format!("because no resource-based policy allows the {action} action")
}

/// An ARN naming a policy of the ACCOUNT that the account does not hold: the account is the
/// authority on its own policies, so this attachment grants nothing.
fn account_policy_arn() -> String {
    format!(
        "arn:aws:iam::{}:policy/grant-absent",
        ::engine::emulator_account_id()
    )
}

/// The domain's ARN, which the documents below name as their resource.
fn domain_arn() -> String {
    format!(
        "arn:aws:es:{REGION}:{}:domain/{DOMAIN}",
        ::engine::emulator_account_id()
    )
}

/// An identity-based document with `effect` on the data-plane write actions of this service,
/// scoped to every path of the domain the way a policy for one domain is written.
fn identity_document(effect: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Action": ["es:ESHttpPost", "es:ESHttpPut"],
            "Resource": format!("{}/*", domain_arn()),
        }],
    })
}

/// The access policy the domain carries: the control role allowed on every path of the
/// domain, and nothing said about any other principal.
fn access_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": role_arn(CONTROL_ROLE) },
            "Action": "es:ESHttp*",
            "Resource": format!("{}/*", domain_arn()),
        }],
    })
    .to_string()
}

/// Attach `policy_arn` to `role` — the record `iam:AttachRolePolicy` writes, planted through
/// IAM's OWN state type and key rule rather than through a second copy of it.
fn given_attached_policy(emulator: &Emulator, role: &str, policy_arn: &str) {
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    store
        .lock()
        .role_attached_policies
        .entry(iam::name_key(role))
        .or_default()
        .insert(policy_arn.to_string());
}

/// What one row's principal carries, which is what the row varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Carried {
    /// A document embedded in the role that allows the write.
    EmbeddedAllow,
    /// An attachment naming one of AWS's own managed policies: a grant this emulator cannot
    /// read.
    AwsManagedAttachment,
    /// That same attachment, beside a document embedded in the role that explicitly denies
    /// the write.
    AwsManagedAttachmentAndEmbeddedDeny,
    /// An attachment naming a policy of the account that the account does not hold.
    AbsentAccountPolicyAttachment,
    /// Nothing of this service at all.
    Nothing,
}

/// One row: the role whose requests are decided, what it carries, and whether the write is
/// admitted.
pub(crate) struct Row {
    pub(crate) case: &'static str,
    pub(crate) role: &'static str,
    pub(crate) access_key_id: &'static str,
    pub(crate) carried: Carried,
    /// Whether the data plane must admit the write.
    pub(crate) admitted: bool,
}

/// The rows: every position a principal's grant of this service can be in.
pub(crate) const ROWS: &[Row] = &[
    Row {
        case: "a document embedded in the role allowing the write",
        role: "grant-embedded",
        access_key_id: "ASIAEXAMPLEEMBEDDED1",
        carried: Carried::EmbeddedAllow,
        admitted: true,
    },
    Row {
        case: "an attachment naming a policy AWS owns, whose document cannot be read here",
        role: "grant-aws-managed",
        access_key_id: "ASIAEXAMPLEAWSMANAGE",
        carried: Carried::AwsManagedAttachment,
        admitted: true,
    },
    Row {
        case: "that same attachment beside a readable explicit deny",
        role: "grant-aws-denied",
        access_key_id: "ASIAEXAMPLEAWSDENIED",
        carried: Carried::AwsManagedAttachmentAndEmbeddedDeny,
        admitted: false,
    },
    Row {
        case: "an attachment naming a policy of the account the account does not hold",
        role: "grant-absent-policy",
        access_key_id: "ASIAEXAMPLEABSENTPOL",
        carried: Carried::AbsentAccountPolicyAttachment,
        admitted: false,
    },
    Row {
        case: "a role carrying nothing of this service",
        role: "grant-none",
        access_key_id: "ASIAEXAMPLENOTHING01",
        carried: Carried::Nothing,
        admitted: false,
    },
];

/// The clause the refusal of one row's request ends with: the rule that refused it.
pub(crate) fn clause_of(row: &Row, action: &str) -> String {
    if row.carried == Carried::AwsManagedAttachmentAndEmbeddedDeny {
        return EXPLICIT_IDENTITY_DENY.to_string();
    }
    nothing_allows(action)
}

/// Seed every row's session and whatever its principal carries, and the control role's own
/// session beside them.
pub(crate) fn given_principals(emulator: &Emulator) {
    given_session(emulator, CONTROL_ROLE, SESSION, CONTROL_KEY);
    for row in ROWS {
        given_session(emulator, row.role, SESSION, row.access_key_id);
        match row.carried {
            Carried::EmbeddedAllow => {
                given_role_policy(emulator, row.role, identity_document("Allow"));
            }
            Carried::AwsManagedAttachment => {
                given_attached_policy(emulator, row.role, AWS_MANAGED_POLICY_ARN);
            }
            Carried::AwsManagedAttachmentAndEmbeddedDeny => {
                given_attached_policy(emulator, row.role, AWS_MANAGED_POLICY_ARN);
                given_role_policy(emulator, row.role, identity_document("Deny"));
            }
            Carried::AbsentAccountPolicyAttachment => {
                given_attached_policy(emulator, row.role, &account_policy_arn());
            }
            Carried::Nothing => {}
        }
    }
}

/// Create the domain with the access policy above and answer the address it publishes.
pub(crate) async fn given_domain(emulator: &Emulator) -> String {
    let created = emulator
        .create(json!({
            "DomainName": DOMAIN,
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
            "AccessPolicies": access_policy(),
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateDomain, got {} with {:?}",
        created.status, created.body
    );
    // A domain publishes no address until the cluster behind it is up, and the address is
    // what every request of the suite is sent to.
    tokio::time::sleep(SETTLE_WAIT).await;
    published_endpoint(emulator, DOMAIN, "setup").await
}
