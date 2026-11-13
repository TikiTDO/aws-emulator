// SPDX-License-Identifier: CC0-1.0

//! The resource policies attached to this account's Systems Manager resources: what
//! `PutResourcePolicy` writes, `GetResourcePolicies` reports and `DeleteResourcePolicy`
//! removes.
//!
//! A resource policy is attached to ONE resource, named by that resource's Amazon
//! Resource Name, so the store below is keyed by the RESOURCE that ARN names
//! ([`PolicyResourceKey`]: the Region whose endpoint the request reached, the resource
//! type and the resource's own id) and holds the policies attached to it in the order
//! they were attached. Keying by the resource rather than by the ARN text is what makes
//! two spellings of one resource address the one set of policies. Only the resources
//! Systems Manager documents as supporting a resource policy can carry one
//! ([`PolicyResourceKind`]), which is why an ARN is read before anything is stored:
//! a value naming no such resource is a bad parameter rather than a resource that
//! happens to hold no policy.
//!
//! Each attached policy carries the two values every one of the three operations speaks
//! in:
//!
//! * `PolicyId`, minted when the policy is first attached and kept for as long as the
//!   attachment lives, so an update addresses the policy it last read; and
//! * `PolicyHash`, the digest of the document as stored ([`policy_hash`]) — "the hash
//!   helps to prevent a situation where multiple users attempt to overwrite a policy. You
//!   must provide this hash when updating or deleting a policy"
//!   (`PutResourcePolicyRequest$PolicyHash`). It is therefore computed from the
//!   document's CONTENT: the same document always hashes to the same value, a document
//!   changed in any way hashes to a different one, and a caller holding an obsolete hash
//!   is refused.
//!
//! The document is stored in the one layout an attached policy is reported in
//! (`service_common::iam_policy::canonical_document`, applied by the handlers), so a read
//! reports what the write persisted rather than the caller's own bytes, and the hash a
//! write answered is the hash a later read reports.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::GetResourcePoliciesResponseEntry;

/// The resource type an OpsItemGroup ARN names
/// (`arn:aws:ssm:<region>:<account>:opsitemgroup/<id>`), the resource whose "policy
/// enables Amazon Web Services accounts to view and interact with OpsCenter operational
/// work items" (`smithy_query(query="doc",shape="GetResourcePoliciesResponseEntry")`).
pub const OPS_ITEM_GROUP_RESOURCE_TYPE: &str = "opsitemgroup";

/// The largest policy document a resource may be given: "a policy can't be greater than
/// 1024 bytes in size" (`ResourcePolicyLimitExceededException`), measured over the
/// document as it was submitted.
pub const MAX_POLICY_SIZE: usize = 1024;

/// How many policies one OpsItemGroup holds: "only one policy can be attached to
/// `OpsItemGroup`" (`ResourcePolicyLimitExceededException`).
pub const MAX_OPS_ITEM_GROUP_POLICIES: usize = 1;

/// Ceiling on the resource policies this emulator will hold at once. `PutResourcePolicy`
/// writes a record any caller can reach, keyed by an ARN the caller chose and carrying a
/// policy document, so the number of records one process will hold is bounded here rather
/// than by the caller. A write that would add one past the ceiling is refused with the
/// operation's own modelled `ResourcePolicyLimitExceededException` rather than stored.
pub const MAX_RESOURCE_POLICIES: usize = 1_000;

/// How many policies one `GetResourcePolicies` call reports: the modelled
/// `ResourcePolicyMaxResults` upper bound
/// (`smithy_query(query="shape",shape="ResourcePolicyMaxResults")`: range 1..50), which is
/// also the page a call that asks for no number gets.
pub const MAX_PAGE_SIZE: usize = 50;

/// The input member that names the resource a policy is attached to, reported in
/// `ResourcePolicyInvalidParameterException$ParameterNames` when the value it carries
/// names no such resource.
pub const RESOURCE_ARN_MEMBER: &str = "ResourceArn";

/// The input member that names which policy a call addresses. See
/// [`RESOURCE_ARN_MEMBER`].
pub const POLICY_ID_MEMBER: &str = "PolicyId";

/// The input member that carries the policy version a call proceeds against. See
/// [`RESOURCE_ARN_MEMBER`].
pub const POLICY_HASH_MEMBER: &str = "PolicyHash";

/// The input member that resumes a page of a resource's policies. See
/// [`RESOURCE_ARN_MEMBER`].
pub const NEXT_TOKEN_MEMBER: &str = "NextToken";

/// What `ResourcePolicyInvalidParameterException` says, which is the sentence the error
/// itself carries: "one or more parameters specified for the call aren't valid. Verify the
/// parameters and their values and try again". The members that aren't valid travel
/// beside it in `ParameterNames`.
pub const INVALID_PARAMETER: &str = "One or more parameters specified for the call aren't \
     valid. Verify the parameters and their values and try again.";

/// What `MalformedResourcePolicyDocumentException` says of a document that is not a
/// resource policy: "the specified policy document is malformed or invalid".
pub const MALFORMED_POLICY_DOCUMENT: &str = "The specified policy document is malformed or invalid.";

/// What `ResourcePolicyConflictException` says: "the hash provided in the call doesn't
/// match the stored hash".
pub const POLICY_HASH_MISMATCH: &str = "The hash provided in the call doesn't match the stored hash.";

/// What `ResourcePolicyNotFoundException` says: "no policies with the specified policy ID
/// and hash could be found".
pub const POLICY_NOT_FOUND: &str = "No policies with the specified policy ID and hash could be found.";

/// What `ResourcePolicyLimitExceededException` says of a resource already holding the one
/// policy an OpsItemGroup may have: "only one policy can be attached to `OpsItemGroup`".
pub const ONE_POLICY_PER_OPS_ITEM_GROUP: &str = "Only one policy can be attached to OpsItemGroup.";

/// What `ResourcePolicyLimitExceededException` says of a document over the size limit: "a
/// policy can't be greater than 1024 bytes in size", with the figure read from
/// [`MAX_POLICY_SIZE`] rather than written twice.
#[must_use]
pub fn policy_too_large() -> String {
    format!("A policy can't be greater than {MAX_POLICY_SIZE} bytes in size.")
}

/// What a write refused for [`MAX_RESOURCE_POLICIES`] says. The ceiling is this
/// emulator's own rather than a Systems Manager limit, so the sentence says whose it is;
/// the figure is read from the constant rather than written twice.
#[must_use]
pub fn too_many_policies() -> String {
    format!("This emulator holds at most {MAX_RESOURCE_POLICIES} resource policies at once.")
}

/// What `ResourceNotFoundException` says of an ARN naming a resource this account does not
/// hold, which is the sentence the modelled error itself carries: "the specified parameter
/// to be shared could not be found".
pub const RESOURCE_NOT_FOUND: &str = "The specified parameter to be shared could not be found.";

/// Which kind of Systems Manager resource a resource-policy request addresses.
///
/// The three the operation documents: "the following resources support Systems Manager
/// resource policies … `OpsItemGroup` … `Parameter` … `Document`"
/// (`smithy_query(query="doc",shape="PutResourcePolicy")`). An ARN naming anything else
/// names no resource a policy can be attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyResourceKind {
    /// The account's OpsCenter operational-work-item group.
    OpsItemGroup,
    /// One Parameter Store parameter.
    Parameter,
    /// One Systems Manager document.
    Document,
}

impl PolicyResourceKind {
    /// The resource type an ARN of this kind carries — the token the ARN's resource field
    /// begins with, and part of the key a resource's policies are held under.
    #[must_use]
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::OpsItemGroup => OPS_ITEM_GROUP_RESOURCE_TYPE,
            Self::Parameter => super::ARN_RESOURCE_PREFIX.trim_end_matches('/'),
            Self::Document => super::ARN_DOCUMENT_RESOURCE_PREFIX.trim_end_matches('/'),
        }
    }
}

/// The resource a `ResourceArn` names: its kind, the id the ARN carries for it, and the
/// account and Region the ARN names it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyResource<'a> {
    /// Which kind of resource the ARN's resource type names.
    pub kind: PolicyResourceKind,
    /// The resource's own id within that type — a parameter's or document's name, an
    /// OpsItemGroup's id.
    pub name: &'a str,
    /// The account field of the ARN, empty for an account-less spelling.
    pub account: &'a str,
    /// The Region field of the ARN, empty for a Region-less spelling.
    pub region: &'a str,
}

impl PolicyResource<'_> {
    /// Whether this is a resource THIS endpoint holds: one of the emulated account, in
    /// the Region the request was signed for.
    ///
    /// A full ARN names the account and the Region its resource lives in, and a resource
    /// is reached through the endpoint it is asked for, so an ARN naming another account
    /// or another Region names nothing reachable here — the same reading every
    /// name-taking operation of this service applies to a caller-supplied ARN. A field
    /// the ARN leaves empty names nothing to disagree with.
    #[must_use]
    pub fn is_local(&self, region: &str) -> bool {
        (self.account.is_empty() || self.account == ::engine::emulator_account_id())
            && (self.region.is_empty() || self.region == region)
    }

    /// The key this resource's policies are held under at `region`'s endpoint.
    #[must_use]
    pub fn key(&self, region: &str) -> PolicyResourceKey {
        PolicyResourceKey {
            region: region.to_string(),
            resource_type: self.kind.resource_type(),
            name: self.name.to_string(),
        }
    }
}

/// What a resource's policies are held under: the Region whose endpoint the request
/// reached, the resource type and the resource's own id.
///
/// The RESOURCE rather than the ARN TEXT, so two spellings of one resource — an ARN
/// written with the `type:id` separator rather than `type/id`, or with its Region field
/// left empty — address the one set of policies instead of each holding its own. A
/// resource is reached through the endpoint it lives at, which is the Region the request
/// was signed for, so that Region is the key's first part.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct PolicyResourceKey {
    /// The Region whose endpoint the request reached.
    pub region: String,
    /// The resource type, from [`PolicyResourceKind::resource_type`].
    pub resource_type: &'static str,
    /// The resource's own id within that type.
    pub name: String,
}

impl<'de> serde::Deserialize<'de> for PolicyResourceKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct OwnedKey {
            region: String,
            resource_type: String,
            name: String,
        }
        let raw = OwnedKey::deserialize(deserializer)?;
        for kind in [
            PolicyResourceKind::OpsItemGroup,
            PolicyResourceKind::Parameter,
            PolicyResourceKind::Document,
        ] {
            if raw.resource_type == kind.resource_type() {
                return Ok(Self {
                    region: raw.region,
                    resource_type: kind.resource_type(),
                    name: raw.name,
                });
            }
        }
        Err(serde::de::Error::custom(format!(
            "unknown SSM policy resource type {:?}",
            raw.resource_type
        )))
    }
}

/// What `resource_arn` names, or `None` when it is not the ARN of a Systems Manager
/// resource that supports a resource policy.
///
/// The ARN is read through the one parse every service reads a caller-supplied ARN with,
/// so only an `ssm` ARN carrying a non-empty id of one of the supported resource types
/// resolves. Whether the resource it names is one this endpoint holds is a separate
/// question, which [`PolicyResource::is_local`] answers: an ARN that is well formed and
/// an ARN that is reachable are different refusals.
#[must_use]
pub fn parse_policy_resource(resource_arn: &str) -> Option<PolicyResource<'_>> {
    let arn = ::service_common::arn::parse_of(resource_arn, super::ARN_SERVICE)?;
    [
        PolicyResourceKind::Parameter,
        PolicyResourceKind::Document,
        PolicyResourceKind::OpsItemGroup,
    ]
    .into_iter()
    .find_map(|kind| {
        let name =
            ::service_common::arn::resource_id_of_type(resource_arn, kind.resource_type())?;
        (!name.is_empty()).then_some(PolicyResource {
            kind,
            name,
            account: arn.account,
            region: arn.region,
        })
    })
}

/// The `PolicyHash` of a policy document: the digest of the document as it is stored,
/// rendered lowercase hexadecimal.
///
/// The same content digest this service hashes a document's content with
/// ([`super::document_content_hash`]), for the same reason: it is derived from the
/// CONTENT, so the hash a write answers is reproducible from the policy a read reports
/// and an obsolete hash is recognisable as one. Not a per-run value, and not a secret.
#[must_use]
pub fn policy_hash(policy: &str) -> String {
    super::document_content_hash(policy)
}

/// One attached resource policy: the policy's id, the document as stored, and the
/// document's hash.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ResourcePolicyRecord {
    /// `PolicyId`: minted when the policy is attached, and the value an update or a
    /// delete addresses this policy by.
    pub policy_id: String,
    /// The policy document, in the layout it is reported in.
    pub policy: String,
    /// `PolicyHash`: [`policy_hash`] of [`Self::policy`].
    pub policy_hash: String,
}

impl ResourcePolicyRecord {
    /// This policy as `GetResourcePolicies` reports it, which is the stored record and
    /// nothing else.
    #[must_use]
    pub fn to_entry(&self) -> GetResourcePoliciesResponseEntry {
        GetResourcePoliciesResponseEntry {
            policy_id: Some(self.policy_id.clone()),
            policy_hash: Some(self.policy_hash.clone()),
            policy: Some(self.policy.clone()),
        }
    }
}

/// Every resource policy this account holds, keyed by the resource it is attached to
/// ([`PolicyResourceKey`]).
///
/// A `BTreeMap` so a walk over the resources is a stable, total order, and each
/// resource's policies are held in the order they were attached — which is the order
/// `GetResourcePolicies` reports them in and what makes its resume cursor total.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ResourcePolicyStore {
    /// The policies attached to each resource, by that resource's key.
    #[serde(with = "::service_common::persist::entries")]
    attached: BTreeMap<PolicyResourceKey, Vec<ResourcePolicyRecord>>,
    /// How many policies this store has minted an id for, the seed that makes the next
    /// [`Self::mint_policy_id`] unrelated to the last.
    minted: u64,
}

impl ResourcePolicyStore {
    /// The policies attached to `resource`, in attachment order; empty for a resource
    /// holding none.
    #[must_use]
    pub fn policies(&self, resource: &PolicyResourceKey) -> &[ResourcePolicyRecord] {
        self.attached
            .get(resource)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The policy `policy_id` names on `resource`, or `None` when that resource holds
    /// no policy of that id — which is what "no policies with the specified policy ID …
    /// could be found" reports.
    #[must_use]
    pub fn policy(
        &self,
        resource: &PolicyResourceKey,
        policy_id: &str,
    ) -> Option<&ResourcePolicyRecord> {
        self.policies(resource)
            .iter()
            .find(|held| held.policy_id == policy_id)
    }

    /// How many policies this store holds across every resource, which is what
    /// [`MAX_RESOURCE_POLICIES`] bounds.
    #[must_use]
    pub fn policy_count(&self) -> usize {
        self.attached.values().map(Vec::len).sum()
    }

    /// Attach `policy` to `resource` as a NEW policy, answering the record that was
    /// stored — its freshly minted id and the hash of the document.
    pub fn attach(
        &mut self,
        resource: &PolicyResourceKey,
        policy: String,
    ) -> ResourcePolicyRecord {
        let record = ResourcePolicyRecord {
            policy_id: self.mint_policy_id(),
            policy_hash: policy_hash(&policy),
            policy,
        };
        self.attached
            .entry(resource.clone())
            .or_default()
            .push(record.clone());
        record
    }

    /// Replace the document of the policy `policy_id` names on `resource`, answering
    /// the record as it now stands; `None` when that resource holds no such policy.
    ///
    /// The policy keeps its id — it is the same attachment, rewritten — and its hash is
    /// recomputed from the new document, so the hash the caller held is obsolete
    /// afterwards.
    pub fn replace(
        &mut self,
        resource: &PolicyResourceKey,
        policy_id: &str,
        policy: String,
    ) -> Option<ResourcePolicyRecord> {
        let held = self
            .attached
            .get_mut(resource)?
            .iter_mut()
            .find(|held| held.policy_id == policy_id)?;
        held.policy_hash = policy_hash(&policy);
        held.policy = policy;
        Some(held.clone())
    }

    /// Detach the policy BOTH `policy_id` and `policy_hash` name from `resource`,
    /// reporting whether one was removed.
    ///
    /// The pair is one address: "no policies with the specified policy ID AND HASH could
    /// be found" (`ResourcePolicyNotFoundException`), so a caller holding an obsolete hash
    /// names no policy here rather than naming the current one with the wrong hash — and
    /// the lookup and the removal are ONE step, so the policy a delete refuses and the
    /// policy it removes cannot drift apart. A resource left holding no policy is
    /// forgotten, so it reads exactly like one that never had a policy.
    pub fn detach_addressed(
        &mut self,
        resource: &PolicyResourceKey,
        policy_id: &str,
        policy_hash: &str,
    ) -> bool {
        let Some(held) = self.attached.get_mut(resource) else {
            return false;
        };
        let before = held.len();
        held.retain(|record| {
            record.policy_id != policy_id || record.policy_hash != policy_hash
        });
        let removed = held.len() != before;
        if held.is_empty() {
            self.attached.remove(resource);
        }
        removed
    }

    /// A fresh `PolicyId`: the opaque, server-minted id of one attachment.
    ///
    /// Minted through this service's one id minter ([`super::minted_uuid`]) from this
    /// store's mint count and the clock, so it is laid out as a UUID — the form AWS mints
    /// an opaque resource id in — and consecutive ids are unrelated. Nothing about the
    /// request or the policy is in it: two attachments of the SAME document are two
    /// policies, each with its own id.
    fn mint_policy_id(&mut self) -> String {
        self.minted += 1;
        super::minted_uuid(now_nanos(), self.minted)
    }
}

/// Now, in nanoseconds since the Unix epoch — only an id-minting seed.
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}
