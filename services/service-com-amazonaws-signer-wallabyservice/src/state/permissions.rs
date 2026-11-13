// SPDX-License-Identifier: CC0-1.0

//! The cross-account permission policy of a signing profile, as this emulator
//! persists it.
//!
//! A signing profile carries a resource policy of its own: `AddProfilePermission`
//! adds one statement to it, `RemoveProfilePermission` takes one out, both mint
//! the policy's next revision id, and `ListProfilePermissions` reports the
//! statements, that revision and the size of the document they form. The policy
//! lives beside [`super::SigningProfileRecord`] rather than inside it because the
//! two have different lifetimes — see [`ProfilePermissionPolicy`].
//!
//! The store itself is [`super::ComAmazonawsSignerWallabyserviceState::profile_permissions`];
//! every handler reads and writes it through [`super::service_state`].

use std::collections::BTreeMap;

use super::{SigningProfileRecord, SigningWorkflow};

/// The request asked for something the resource's current state already holds:
/// the modelled shape's own name.
pub const ERROR_CODE_CONFLICT: &str = "ConflictException";

/// The request named a `revisionId` that is not the one the profile's policy is
/// at — the condition a writer recovers from by re-reading the policy and
/// re-issuing the call against the revision it reports.
pub const ERROR_CODE_POLICY_REVISION_ID_MISMATCH: &str = "PolicyRevisionIdMismatch";

/// The profile holds no resource policy at all, so there is none to report, to
/// revise or to take a statement out of.
pub const ERROR_CODE_POLICY_NOT_FOUND: &str = "PolicyNotFound";

/// The refusal prose for a call that resolved the profile and found it holding
/// no policy.
///
/// `profileName` is bounded by its modelled shape (`ProfileName`,
/// `@length(2..64)`), so it is stated as it arrived.
pub fn no_policy_message(profile_name: &str) -> String {
    format!("No policies associated with profile {profile_name}")
}

/// The refusal prose for a write that lost the policy's optimistic-concurrency
/// check: the revision the request carried, and the one the policy is actually
/// at — which is what the writer has to re-issue the call against.
///
/// `revisionId` is a bare string in the model, so the supplied value is quoted
/// through the one ceiling this crate states for a request value a clause
/// carries ([`super::stated_value`]); the current revision is this service's own
/// minted id.
pub fn revision_mismatch_message(specified: &str, current: &str) -> String {
    format!(
        "Specified revisionId ({}) does not match the revisionId of the existing policy \
         ({current})",
        super::stated_value(specified)
    )
}

/// The refusal prose for a grant naming a `statementId` the profile's policy
/// already holds: a statement id identifies a statement WITHIN the policy, so the
/// policy cannot hold two of them.
///
/// `statementId` is a bare string in the model, so the supplied value is quoted
/// through the one ceiling this crate states for a request value a clause carries
/// ([`super::stated_value`]); `profileName` is bounded by its modelled shape and
/// is stated as it arrived.
pub fn duplicate_statement_message(statement_id: &str, profile_name: &str) -> String {
    format!(
        "statement id {} already exists in the permissions of profile {profile_name}",
        super::stated_value(statement_id)
    )
}

/// The refusal prose for a call naming a `statementId` the profile's policy does
/// not hold — the other half of the same question, so it is worded and bounded
/// beside [`duplicate_statement_message`].
pub fn statement_not_found_message(statement_id: &str, profile_name: &str) -> String {
    format!(
        "statement id {} does not exist in the permissions of profile {profile_name}",
        super::stated_value(statement_id)
    )
}

/// The request would carry a resource past a service limit: the modelled shape's
/// own name.
pub const ERROR_CODE_SERVICE_LIMIT_EXCEEDED: &str = "ServiceLimitExceededException";

/// The AWS Signer actions a cross-account permission may grant whatever the
/// profile's signing platform: the two whose operations read the profile or
/// revoke a signature rather than produce one.
///
/// Not a chosen list: `AddProfilePermissionRequest$action` documents "You can
/// designate the following actions to an account" and then names four
/// (`smithy_query(query="doc", shape="AddProfilePermissionRequest",
/// member="action")`) — these two, plus the SIGNING action of each of the two
/// signing workflows, which the same documentation splits per platform (see
/// [`grantable_actions`]).
const PLATFORM_INDEPENDENT_ACTIONS: [&str; 2] =
    ["signer:GetSigningProfile", "signer:RevokeSignature"];

/// The actions a cross-account permission on a profile of `platform_id` may
/// grant, the signing action of that platform's workflow first.
///
/// A grant names an operation a designated account may call on the profile, and
/// a profile is signed through exactly one of Signer's two signing workflows
/// ([`SigningWorkflow`]) — so the signing operation of the OTHER workflow is not
/// one that profile has at all, and is not grantable on it. The model states the
/// split per action: `signer:StartSigningJob` "isn't supported for container
/// image workflows" and `signer:SignPayload` "isn't supported for AWS Lambda
/// workflows".
///
/// A `platform_id` the catalog publishes no entry for narrows nothing: there is
/// no workflow to read off it, so both signing actions stay grantable rather
/// than one being refused for a split this service cannot resolve.
pub fn grantable_actions(platform_id: &str) -> Vec<&'static str> {
    let signing_actions: Vec<&'static str> = match super::signing_platform(platform_id) {
        Some(entry) => vec![signing_action_of(entry.workflow)],
        None => vec![
            signing_action_of(SigningWorkflow::CodeSigningJob),
            signing_action_of(SigningWorkflow::ContainerImage),
        ],
    };
    signing_actions
        .into_iter()
        .chain(PLATFORM_INDEPENDENT_ACTIONS)
        .collect()
}

/// The grantable action naming `workflow`'s signing operation — `signer:`
/// followed by the operation [`SigningWorkflow::signing_operation`] names, so
/// the action and the operation cannot drift apart.
fn signing_action_of(workflow: SigningWorkflow) -> &'static str {
    match workflow {
        SigningWorkflow::CodeSigningJob => "signer:StartSigningJob",
        SigningWorkflow::ContainerImage => "signer:SignPayload",
    }
}

/// Whether `action` is one a grant on a profile of `platform_id` may carry.
pub fn is_grantable_action(platform_id: &str, action: &str) -> bool {
    grantable_actions(platform_id).contains(&action)
}

/// The refusal prose for an `action` no grant on a profile of that platform may
/// carry: the value, the platform that narrowed the set, and the actions the set
/// holds — so the caller can fix the request from the text alone.
///
/// `action` is a bare string in the model, so it is quoted through the one
/// ceiling this crate states for a request value a clause carries
/// ([`super::stated_value`]).
pub fn ungrantable_action_message(action: &str, platform_id: &str) -> String {
    format!(
        "Action {} not supported for platform {}. Action must be one of [{}]",
        super::stated_value(action),
        super::stated_value(platform_id),
        grantable_actions(platform_id).join(", ")
    )
}

/// How wide an AWS account id is, as this service's own model states it:
/// `AccountId` is `@length(12, 12)` with `^[0-9]{12}$`
/// (`smithy_query(query="shape", shape="AccountId")`) — the form every member of
/// this service that carries an account id (a job's `jobOwner` and `jobInvoker`,
/// a signing call's `profileOwner`) is held to at the boundary.
const ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is an AWS account id: exactly [`ACCOUNT_ID_DIGITS`] ASCII
/// digits, the form the modelled `AccountId` shape publishes.
fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// The `(service, resource prefix, least number of `/`-separated parts after the
/// prefix)` forms an ARN takes to name an AWS principal; a row whose count is
/// zero is one whose prefix is the WHOLE resource.
///
/// Not a chosen list: `AddProfilePermissionRequest$principal` is "The AWS
/// principal receiving cross-account permissions. This may be an IAM role or
/// another AWS account ID" (`smithy_query(query="doc",
/// shape="AddProfilePermissionRequest", member="principal")`), and a grant
/// becomes the `AWS` key of the policy statement's `Principal` — which AWS's
/// policy grammar publishes as an account id, an account root ARN, an IAM user
/// or role ARN, or a role-session ARN (an assumed role or a federated user). A
/// path before an IAM name, and the session name after a role, are part of those
/// forms, which is why the parts after the prefix are COUNTED rather than
/// spelled out: an IAM name carries no bound this grant is the place to apply.
const PRINCIPAL_ARN_FORMS: [(&str, &str, usize); 5] = [
    ("iam", "root", 0),
    ("iam", "user/", 1),
    ("iam", "role/", 1),
    ("sts", "assumed-role/", 2),
    ("sts", "federated-user/", 1),
];

/// Whether `resource` is `prefix` followed by at least `least` `/`-separated
/// parts, none of them empty — and by nothing at all when `least` is zero.
fn names_parts_after(resource: &str, prefix: &str, least: usize) -> bool {
    let Some(rest) = resource.strip_prefix(prefix) else {
        return false;
    };
    if least == 0 {
        return rest.is_empty();
    }
    let mut parts = 0usize;
    for part in rest.split('/') {
        if part.is_empty() {
            return false;
        }
        parts += 1;
    }
    parts >= least
}

/// Whether `value` is the ARN of an AWS principal: one of
/// [`PRINCIPAL_ARN_FORMS`] under
/// `arn:<partition>:<service>::<account id>:<resource>`.
///
/// A principal is an identity, which is global within its account: such an ARN
/// carries no region, and its account field is an account id
/// ([`is_account_id`]). A judgement on the ARN's SHAPE alone — a grant is
/// CROSS-account, so the identity it names lives in an account this endpoint
/// holds nothing of and could not be resolved even in principle.
fn is_principal_arn(value: &str) -> bool {
    // `arn : partition : service : region : account : resource`, the resource
    // being the remainder because it carries `/`-separated parts of its own.
    let mut fields = value.splitn(6, ':');
    if fields.next() != Some("arn") {
        return false;
    }
    let (Some(partition), Some(service), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    if partition.is_empty() || !region.is_empty() || !is_account_id(account) {
        return false;
    }
    PRINCIPAL_ARN_FORMS
        .iter()
        .any(|(arn_service, prefix, least)| {
            *arn_service == service && names_parts_after(resource, prefix, *least)
        })
}

/// Whether `value` names an AWS principal — the only thing a cross-account grant
/// can be written FOR: an AWS account id, or the ARN of an identity in one
/// account ([`PRINCIPAL_ARN_FORMS`]).
///
/// `principal` is an unconstrained `String` in the model, so the boundary
/// validator admits any text; what the member carries is an AWS principal, and a
/// statement whose principal names no AWS identity grants nothing to anybody.
pub fn is_aws_principal(value: &str) -> bool {
    is_account_id(value) || is_principal_arn(value)
}

/// The refusal prose for a `principal` that names no AWS principal: the value,
/// and the forms a principal takes — so the caller can fix the request from the
/// text alone, the way [`ungrantable_action_message`] states the actions a grant
/// may carry.
///
/// `principal` is a bare string in the model, so the supplied value is quoted
/// through the one ceiling this crate states for a request value a clause
/// carries ([`super::stated_value`]).
pub fn invalid_principal_message(principal: &str) -> String {
    format!(
        "Invalid principal {}. principal must be an AWS account ID of {ACCOUNT_ID_DIGITS} digits, \
         or the ARN of an AWS account root, an IAM user, an IAM role or a role session",
        super::stated_value(principal)
    )
}

/// The IAM policy-language version every Signer profile permission policy is
/// written in — the one AWS's policy grammar publishes
/// (`"Version": "2012-10-17"`).
const POLICY_DOCUMENT_VERSION: &str = "2012-10-17";

/// The most bytes one profile's cross-account permission policy may hold.
///
/// Signer publishes no quota for it, so this is the bound AWS puts on a
/// RESOURCE-BASED policy document elsewhere — 20,480 bytes, the documented
/// maximum for a resource policy (AWS Lambda's `AddPermission` quota, the same
/// kind of document in the same policy language). It is what keeps ONE profile's
/// policy from growing without limit, one small `AddProfilePermission` at a time:
/// a grant that would carry the document past it is refused with the modelled
/// `ServiceLimitExceededException`, which this operation declares for exactly
/// that ("The client is making a request that exceeds service limits"). This one
/// constant is where a figure Signer publishes would replace it.
pub const MAX_POLICY_SIZE_BYTES: i32 = 20_480;

/// The refusal prose for a grant that would carry a policy past
/// [`MAX_POLICY_SIZE_BYTES`], built from the request's own effect on the policy
/// and saying whose bound it is, since Signer publishes none.
pub fn policy_too_large_message(profile_name: &str, resulting: i32) -> String {
    format!(
        "this emulator bounds the permissions policy of a signing profile at \
         {MAX_POLICY_SIZE_BYTES} bytes; this request would leave the policy of \
         {profile_name} holding {resulting}"
    )
}

/// The largest page `ListProfilePermissions` serves, and the page size it uses.
///
/// `ListProfilePermissionsRequest` models no `maxResults`, so the service picks
/// the page size; this is the page ceiling the service's other paginated list
/// publishes (`ListSigningProfilesRequest$maxResults` is
/// `@range(min: 1, max: 25)` — see [`super::PROFILE_PAGE_LIMIT`]), so one page of
/// permissions costs a reader no more than one page of profiles however many
/// statements the policy holds.
pub const PERMISSION_PAGE_LIMIT: usize = 25;

/// One cross-account permission statement, as `AddProfilePermission` stored it.
#[derive(Debug, Clone)]
pub struct ProfilePermissionRecord {
    /// The statement's unique id within the profile's policy.
    pub statement_id: String,
    /// One of the actions [`grantable_actions`] admits for the profile's
    /// platform.
    pub action: String,
    /// The AWS principal the action was granted to.
    pub principal: String,
    /// The profile version this statement applies to, when the grant named one;
    /// `None` for a grant that names no version and so applies to the profile.
    pub profile_version: Option<String>,
}

impl ProfilePermissionRecord {
    /// The `Permission` summary `ListProfilePermissions` reports.
    pub fn to_permission(&self) -> crate::ops::types::Permission {
        crate::ops::types::Permission {
            action: Some(self.action.clone()),
            principal: Some(self.principal.clone()),
            statement_id: Some(self.statement_id.clone()),
            profile_version: self.profile_version.clone(),
        }
    }

    /// The resource this statement grants access to: the profile version it names,
    /// or the profile itself when it names none.
    fn resource_arn(&self, profile: &SigningProfileRecord) -> String {
        match self.profile_version.as_deref() {
            Some(version) => format!("{}/{version}", profile.arn),
            None => profile.arn.clone(),
        }
    }

    /// This statement as a policy-document statement.
    fn to_statement_json(&self, profile: &SigningProfileRecord) -> ::serde_json::Value {
        ::serde_json::json!({
            "Sid": self.statement_id,
            "Effect": "Allow",
            "Principal": { "AWS": self.principal },
            "Action": self.action,
            "Resource": self.resource_arn(profile),
        })
    }
}

/// The cross-account permission policy attached to ONE signing profile: the
/// statements `AddProfilePermission` added and the revision id that identifies
/// their current state.
///
/// The policy is a resource of its own rather than a member of
/// [`SigningProfileRecord`] because it is addressed and revised on its own: a
/// grant is written and revoked against the profile's NAME, through a revision
/// chain of its own that no write to the profile record moves.
///
/// `Default` is the EMPTY policy: no statements, and a revision id no mint ever
/// produces (minted ones are UUID-shaped), so it is only ever used as the thing a
/// first grant is measured against or inserted into — every write sets the
/// revision before the policy is readable.
#[derive(Debug, Clone, Default)]
pub struct ProfilePermissionPolicy {
    /// The id of the policy's current revision — minted afresh by every
    /// `AddProfilePermission` / `RemoveProfilePermission` that changes it, which is
    /// what makes a request carrying an older id detectably stale.
    pub revision_id: String,
    /// The statements, keyed by `statementId`. A `BTreeMap` is what gives
    /// `ListProfilePermissions` the stable total order its pagination token
    /// indexes into.
    pub statements: BTreeMap<String, ProfilePermissionRecord>,
}

impl ProfilePermissionPolicy {
    /// Whether this policy holds no statement at all — the state a policy is
    /// DROPPED in rather than stored in (see
    /// [`super::ComAmazonawsSignerWallabyserviceState::drop_profile_policy`]).
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    /// The policy document these statements form, in AWS's policy language, with
    /// `extra` counted as one more statement when it is given.
    ///
    /// `extra` is appended rather than sorted into place: it is used to MEASURE a
    /// policy before a statement is added, and the document's length does not
    /// depend on the order its statements are written in.
    fn document(
        &self,
        profile: &SigningProfileRecord,
        extra: Option<&ProfilePermissionRecord>,
    ) -> String {
        let mut statements: Vec<::serde_json::Value> = self
            .statements
            .values()
            .map(|statement| statement.to_statement_json(profile))
            .collect();
        statements.extend(extra.map(|statement| statement.to_statement_json(profile)));
        ::serde_json::json!({
            "Version": POLICY_DOCUMENT_VERSION,
            "Statement": statements,
        })
        .to_string()
    }

    /// `policySizeBytes`: "Total size of the policy associated with the Signing
    /// Profile in bytes" — the byte length of the policy document these
    /// statements form, computed from the stored statements rather than tracked
    /// beside them, so it can never disagree with what the policy holds.
    pub fn policy_size_bytes(&self, profile: &SigningProfileRecord) -> i32 {
        size_in_bytes(&self.document(profile, None))
    }

    /// What [`Self::policy_size_bytes`] would report with `statement` added — the
    /// size a grant is admitted or refused on, measured BEFORE anything is
    /// written.
    pub fn policy_size_bytes_with(
        &self,
        profile: &SigningProfileRecord,
        statement: &ProfilePermissionRecord,
    ) -> i32 {
        size_in_bytes(&self.document(profile, Some(statement)))
    }
}

/// `document`'s byte length as the wire integer `policySizeBytes` reports.
fn size_in_bytes(document: &str) -> i32 {
    i32::try_from(document.len()).unwrap_or(i32::MAX)
}
