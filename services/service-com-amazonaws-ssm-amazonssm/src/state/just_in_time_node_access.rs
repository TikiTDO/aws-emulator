// SPDX-License-Identifier: CC0-1.0

//! Just-in-time node access: the access requests `StartAccessRequest` raises and the
//! credentials `GetAccessToken` vends for them.
//!
//! Just-in-time node access is the workflow in which a caller ASKS for access to a managed
//! node, an approval policy decides the request, and an approved request is exchanged for
//! the temporary security credentials a node access session is started with. The two
//! operations of that workflow in this API reach the one store below
//! ([`super::ComAmazonawsSsmAmazonssmState::access_requests`]), so what a start persisted is
//! what a later `GetAccessToken` reports, per Region.
//!
//! ## Whether the account has the workflow at all
//!
//! The workflow is OFF for an account until that account holds an access-request approval
//! policy — the document that decides who may approve a request, held as an SSM document of
//! type `AutoApprovalPolicy` or `ManualApprovalPolicy`
//! ([`APPROVAL_POLICY_DOCUMENT_TYPES`], two of the `DocumentType` values this service models,
//! `crate::ops::types::DOCUMENT_TYPE_VALUES`, and the only ones that exist for this
//! workflow). An account that holds none is reported by both operations as not having the
//! service enabled ([`NOT_ENABLED_MESSAGE`]), the refusal AWS answers before it looks at
//! anything else the request names — so a fresh account, which holds no document at all,
//! has no just-in-time node access surface, and one that creates a policy has it in the
//! Region the policy was created in. A policy is a REGIONAL document like every other, so
//! the decision is taken per Region ([`super::ComAmazonawsSsmAmazonssmState::has_access_request_approval_policy`]).
//!
//! ## What the policy decides
//!
//! An `AutoApprovalPolicy` approves the requests it covers without anybody acting, so a
//! request raised under one is `Pending` until the approval the clock owes falls due
//! ([`APPROVAL_DELAY`]) and `Approved` after. A `ManualApprovalPolicy` leaves the decision to
//! an approver, and no operation of this API records an approver's decision, so a request
//! raised under one stays `Pending`: the status is what the store holds, never a value a read
//! invents.
//!
//! ## What a read of an approved request vends
//!
//! `GetAccessToken` answers an approved request with the temporary security credentials of
//! its approved window ([`VendedCredential`]). They are created in the emulator's own
//! credential store, so the emulator accepts requests signed with them and neither the id nor
//! the secrets are chosen here; they are held against the access request for as long as they
//! are valid ([`CREDENTIAL_LIFETIME`]), so two reads of one approved request report the one
//! credential set rather than a fresh one per read, and a read past their expiry vends the
//! next set. Nothing writes them anywhere but this in-memory store.

use std::time::{Duration, SystemTime};

use super::{DocumentRecord, ARN_SERVICE};
use crate::ops::types::{Credentials, Target};

/// The `DocumentType` of an approval policy that approves the access requests it covers
/// without an approver acting, one of this service's modelled document types
/// (`crate::ops::types::DOCUMENT_TYPE_VALUES`).
pub const AUTO_APPROVAL_POLICY_DOCUMENT_TYPE: &str = "AutoApprovalPolicy";

/// The `DocumentType` of an approval policy that leaves each access request to an approver,
/// from the same modelled value set.
pub const MANUAL_APPROVAL_POLICY_DOCUMENT_TYPE: &str = "ManualApprovalPolicy";

/// The document types an access-request approval policy is held as. An account holding one
/// of these in a Region has just-in-time node access there; an account holding neither has
/// the workflow turned off ([`NOT_ENABLED_MESSAGE`]).
pub const APPROVAL_POLICY_DOCUMENT_TYPES: &[&str] = &[
    AUTO_APPROVAL_POLICY_DOCUMENT_TYPE,
    MANUAL_APPROVAL_POLICY_DOCUMENT_TYPE,
];

/// How this service reports a just-in-time node access call made against an account that has
/// not turned the workflow on.
///
/// The whole sentence is FIXED text for this error class: it names the service and
/// interpolates nothing from the request, so neither the id nor the targets a caller
/// submitted appear in it. Both operations of the workflow report it, and both report it
/// before they resolve anything the request names.
pub const NOT_ENABLED_MESSAGE: &str = "Just-In-Time Node Access service is not enabled in this account.";

/// How this service reports an `AccessRequestId` that is well formed — the modelled
/// `AccessRequestId` pattern `^(oi)-[0-9a-f]{12}$` is enforced before a handler runs — but
/// names no access request the Region's endpoint issued.
///
/// Only the error CLASS of this refusal is observed (`ValidationException`), so the sentence
/// is worded the way a Systems Manager id that names no record is worded — fixed text that
/// echoes nothing of the request.
pub const ACCESS_REQUEST_NOT_FOUND_MESSAGE: &str =
    "The specified access request ID doesn't exist. Verify the ID and try again.";

/// The `AccessRequestStatus` a raised request is in before its policy decides it: "Pending",
/// one of the modelled enum's values (`crate::ops::types::ACCESS_REQUEST_STATUS_VALUES`).
pub const STATUS_PENDING: &str = "Pending";

/// The `AccessRequestStatus` of a request whose policy approved it, from the same modelled
/// enum. The one status `GetAccessToken` vends credentials for.
pub const STATUS_APPROVED: &str = "Approved";

/// How long a request raised under an auto-approval policy stays `Pending` before the clock
/// owes its move to `Approved`.
///
/// AWS publishes no span for it, so this is the emulator's CHOSEN transient window: three
/// seconds, the same span this service gives every transition nobody documents
/// ([`super::DOCUMENT_TRANSITION_DELAY`]). It is deliberately never zero: a caller that reads
/// an access request straight after raising it observes `Pending`, the way it would against
/// the real service, and the move is materialised from the stored due instant
/// ([`AccessRequestRecord::materialize_due`]) rather than by any call.
pub const APPROVAL_DELAY: Duration = Duration::from_secs(3);

/// How long the credentials vended for an approved access request are valid.
///
/// The model states no window for it — `Credentials$ExpirationTime` is only "the datetime on
/// which the current credentials expire" — so this is the emulator's CHOSEN window: one hour,
/// the lifetime temporary security credentials are minted with by default.
pub const CREDENTIAL_LIFETIME: Duration = Duration::from_secs(3_600);

/// How many access requests this store holds at once, across every Region.
///
/// This is the EMULATOR's own ceiling rather than a quota Systems Manager publishes: a start
/// is the only thing that admits a request, callers may raise as many as they like, and the
/// store is held in memory for as long as the process runs — so without a ceiling the one map
/// would grow for as long as starts keep arriving. A start past it is refused through the
/// modelled `ServiceQuotaExceededException` and writes nothing.
pub const MAX_ACCESS_REQUESTS: usize = 100;

/// What a start refused by [`MAX_ACCESS_REQUESTS`] reports: the emulator's own ceiling, named
/// as the emulator's, with the figure it stands at.
pub fn access_request_ceiling_message() -> String {
    format!(
        "This emulator holds at most {MAX_ACCESS_REQUESTS} just-in-time node access requests \
         across every Region"
    )
}

/// The `QuotaCode` the ceiling refusal carries. The bound is the EMULATOR's own rather than a
/// Service Quotas entry, so the code says so instead of quoting a code AWS publishes.
pub const ACCESS_REQUEST_QUOTA_CODE: &str = "EmulatorAccessRequestsPerAccount";

/// The ARN of the access request `access_request_id` names in `region` — the resource an
/// authorization decision about it is made against.
///
/// An access request IS an OpsItem, so it is addressed by the ARN this account mints for an
/// OpsItem of that id ([`super::ops_items::ARN_OPS_ITEM_RESOURCE_PREFIX`]) rather than by a
/// second spelling of one resource: `AccessRequestId`'s pattern `^(oi)-[0-9a-f]{12}$` IS the
/// `OpsItemId` pattern, OpsCenter's own listing narrows to these requests through the
/// `AccessRequestByRequesterArn`, `AccessRequestByApproverArn` and sibling filter keys of
/// `OpsItemFilterKey`, and `AutomationSubtype` carries `AccessRequest` beside
/// `ChangeRequest` (`crate::ops::types::OPS_ITEM_FILTER_KEY_VALUES`,
/// `crate::ops::types::AUTOMATION_SUBTYPE_VALUES`). Two resources therefore cannot collide
/// here: an id names the one OpsItem, whether a start of this workflow or an OpsCenter create
/// raised it, and a statement scoped to that id covers it under either name.
pub fn access_request_arn(region: &str, access_request_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{}{access_request_id}",
            super::ops_items::ARN_OPS_ITEM_RESOURCE_PREFIX
        ),
    )
}

/// The temporary security credentials vended for one approved access request.
///
/// The id and both secrets are the emulator credential store's own
/// (`ctx.credential_store().create(..)`): nothing here chooses them, and they are held only
/// in this in-memory store, for as long as they are valid.
#[derive(Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct VendedCredential {
    /// "The access key ID that identifies the temporary security credentials", which is also
    /// the id the credential is held in the emulator's credential store under.
    pub access_key_id: String,
    /// "The secret access key that can be used to sign requests."
    pub secret_access_key: String,
    /// "The token that users must pass to the service API to use the temporary credentials."
    pub session_token: String,
    /// "The datetime on which the current credentials expire."
    pub expires_at: SystemTime,
}

/// Prints the access key id but neither secret, so a dump of this store leaks nothing — the
/// same contract the emulator's own minted credential holds itself to.
impl ::std::fmt::Debug for VendedCredential {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("VendedCredential")
            .field("access_key_id", &self.access_key_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl VendedCredential {
    /// Whether these credentials have run out at `now`, and so are no longer the ones a read
    /// of this access request reports.
    pub fn expired(&self, now: SystemTime) -> bool {
        self.expires_at <= now
    }

    /// These credentials as the modelled `Credentials` shape a read reports them in.
    pub fn to_credentials(&self) -> Credentials {
        Credentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            session_token: self.session_token.clone(),
            expiration_time: ::aws_smithy_types::DateTime::from(self.expires_at),
        }
    }
}

/// One just-in-time node access request, as `GetAccessToken` reports it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AccessRequestRecord {
    /// The server-minted `AccessRequestId` this record is stored under.
    pub access_request_id: String,
    /// The Region whose endpoint the request was raised through, and which its ARN carries.
    pub region: String,
    /// "A brief description explaining why you are requesting access to the node"
    /// (`StartAccessRequest`'s `Reason`), as the start supplied it.
    pub reason: String,
    /// "The node you are requesting access to" (`StartAccessRequest`'s `Targets`), as the
    /// start supplied them.
    pub targets: Vec<Target>,
    /// The principal the start ran for — who is asking for access.
    pub requester_arn: String,
    /// The request's tags, keyed by tag key, as the start supplied them.
    pub tags: ::std::collections::BTreeMap<String, String>,
    /// The `AccessRequestStatus` this request is in, which is what a read reports.
    pub status: String,
    /// When the approval this request's policy owes falls due, for a request an
    /// auto-approval policy covers; `None` for one whose decision is an approver's, which
    /// nothing in this API makes.
    pub approval_due: Option<SystemTime>,
    /// The credentials vended for this request, once it is approved and something has asked
    /// for them. Held until they expire, so two reads report the one set.
    pub credential: Option<VendedCredential>,
    /// When the request was raised.
    pub created_at: SystemTime,
}

impl AccessRequestRecord {
    /// This access request's ARN ([`access_request_arn`]).
    pub fn arn(&self) -> String {
        access_request_arn(&self.region, &self.access_request_id)
    }

    /// Materialise the approval the clock already owes this request at `now` — the move to
    /// `Approved` that was due whether or not anybody called.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if self.approval_due.is_some_and(|due| due <= now) {
            self.status = STATUS_APPROVED.to_string();
            self.approval_due = None;
        }
    }

    /// The credentials this request currently holds, or `None` when it holds none or the ones
    /// it holds have run out at `now`.
    pub fn live_credential(&self, now: SystemTime) -> Option<&VendedCredential> {
        self.credential.as_ref().filter(|held| !held.expired(now))
    }
}

impl super::ComAmazonawsSsmAmazonssmState {
    /// The access-request approval policies `region` holds: the documents of a policy type
    /// that account created there, in the store's own ascending order.
    ///
    /// An account holding at least one of them has just-in-time node access in that Region;
    /// an account holding none has the workflow turned off ([`NOT_ENABLED_MESSAGE`]).
    pub fn access_request_approval_policies<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a DocumentRecord> + 'a {
        self.documents
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|(_, document)| document)
            .filter(|document| {
                APPROVAL_POLICY_DOCUMENT_TYPES.contains(&document.document_type.as_str())
            })
    }

    /// Whether `region` has just-in-time node access at all: the account holds an
    /// access-request approval policy there.
    pub fn has_access_request_approval_policy(&self, region: &str) -> bool {
        self.access_request_approval_policies(region).next().is_some()
    }

    /// Whether the access requests `region` holds are decided without an approver acting.
    ///
    /// Only where every policy in force there approves on its own: a manual policy is an
    /// approver's say over the requests it covers, so an account that holds one has an
    /// approver, whatever else it holds and whatever those documents are named.
    pub fn access_requests_are_auto_approved(&self, region: &str) -> bool {
        let mut policies = self.access_request_approval_policies(region).peekable();
        policies.peek().is_some()
            && policies.all(|policy| policy.document_type == AUTO_APPROVAL_POLICY_DOCUMENT_TYPE)
    }

    /// The access request `access_request_id` names in `region`, or `None` when that Region
    /// issued none under that id — which is what the not-found refusal reports.
    pub fn access_request(
        &self,
        region: &str,
        access_request_id: &str,
    ) -> Option<&AccessRequestRecord> {
        self.access_requests
            .get(&(region.to_string(), access_request_id.to_string()))
    }

    /// The same record, to materialise what the clock owes it or to hold a vended credential
    /// against it.
    pub fn access_request_mut(
        &mut self,
        region: &str,
        access_request_id: &str,
    ) -> Option<&mut AccessRequestRecord> {
        self.access_requests
            .get_mut(&(region.to_string(), access_request_id.to_string()))
    }

    /// Whether this store already holds [`MAX_ACCESS_REQUESTS`] access requests, so a start
    /// is refused rather than growing it further.
    pub fn access_requests_at_capacity(&self) -> bool {
        self.access_requests.len() >= MAX_ACCESS_REQUESTS
    }

    /// Store `record` as an access request of its own Region, keyed by its id.
    pub fn insert_access_request(&mut self, record: AccessRequestRecord) {
        let key = (record.region.clone(), record.access_request_id.clone());
        self.access_requests.insert(key, record);
    }

    /// Mint an `AccessRequestId` no access request of this store holds.
    ///
    /// The form is the modelled `AccessRequestId` pattern `^(oi)-[0-9a-f]{12}$` — the same
    /// form an `OpsItemId` carries ([`super::ops_items::mint_ops_item_id`]), which is what an
    /// access request is identified by — minted through this store's one minter of a
    /// prefixed-hex id, from its own mint counter and `at`, so nothing of the request is in
    /// it and two starts never collide.
    pub fn next_access_request_id(&mut self, at: SystemTime) -> String {
        loop {
            self.access_request_sequence += 1;
            let candidate = super::minted_prefixed_hex_id(
                super::ops_items::OPS_ITEM_ID_PREFIX,
                super::ops_items::OPS_ITEM_ID_HEX_DIGITS,
                self.access_request_sequence,
                at,
            );
            if !self
                .access_requests
                .keys()
                .any(|(_, held)| held == &candidate)
            {
                return candidate;
            }
        }
    }

    /// Materialise the approval every access request's policy already owes at `now`.
    pub fn materialize_access_requests_due(&mut self, now: SystemTime) {
        for record in self.access_requests.values_mut() {
            record.materialize_due(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document of `document_type` in `region`, as a create persists one.
    fn document(region: &str, name: &str, document_type: &str) -> DocumentRecord {
        DocumentRecord {
            document_id: super::super::new_document_id(),
            name: name.to_string(),
            region: region.to_string(),
            document_type: document_type.to_string(),
            tags: ::std::collections::BTreeMap::new(),
            owner: ::engine::emulator_account_id().to_string(),
            versions: Vec::new(),
            default_version: super::super::FIRST_DOCUMENT_VERSION,
        }
    }

    /// An access request raised in `region` under a policy that approves it at `approval_due`.
    fn record(region: &str, id: &str, approval_due: Option<SystemTime>) -> AccessRequestRecord {
        AccessRequestRecord {
            access_request_id: id.to_string(),
            region: region.to_string(),
            reason: "a reason".to_string(),
            targets: Vec::new(),
            requester_arn: ::service_common::caller_principal::ambient_user_arn(),
            tags: ::std::collections::BTreeMap::new(),
            status: STATUS_PENDING.to_string(),
            approval_due,
            credential: None,
            created_at: SystemTime::UNIX_EPOCH,
        }
    }

    /// RULE: a Region has just-in-time node access when it holds an approval policy document,
    /// and the policy's TYPE decides whether its requests need an approver — so a Region
    /// holding documents of every other type has the workflow turned off.
    #[test]
    fn an_approval_policy_document_is_what_enables_the_workflow() {
        // (case, the document types the Region holds, enabled, auto-approved)
        let rows: &[(&str, &[&str], bool, bool)] = &[
            ("no document at all", &[], false, false),
            ("only documents of other types", &["Command", "Automation", "Session"], false, false),
            ("a manual approval policy", &[MANUAL_APPROVAL_POLICY_DOCUMENT_TYPE], true, false),
            ("an auto approval policy", &[AUTO_APPROVAL_POLICY_DOCUMENT_TYPE], true, true),
            (
                "an auto approval policy beside documents of other types",
                &["Command", AUTO_APPROVAL_POLICY_DOCUMENT_TYPE],
                true,
                true,
            ),
            (
                "an auto policy beside a manual one, whatever the names' order",
                &[
                    AUTO_APPROVAL_POLICY_DOCUMENT_TYPE,
                    MANUAL_APPROVAL_POLICY_DOCUMENT_TYPE,
                ],
                true,
                false,
            ),
            (
                "a manual policy beside an auto one, in the other order",
                &[
                    MANUAL_APPROVAL_POLICY_DOCUMENT_TYPE,
                    AUTO_APPROVAL_POLICY_DOCUMENT_TYPE,
                ],
                true,
                false,
            ),
            (
                "two auto approval policies",
                &[
                    AUTO_APPROVAL_POLICY_DOCUMENT_TYPE,
                    AUTO_APPROVAL_POLICY_DOCUMENT_TYPE,
                ],
                true,
                true,
            ),
        ];
        for (case, types, enabled, auto) in rows {
            let mut state = super::super::ComAmazonawsSsmAmazonssmState::default();
            for (index, document_type) in types.iter().enumerate() {
                let name = format!("doc-{index}");
                state.documents.insert(
                    ("us-west-2".to_string(), name.clone()),
                    document("us-west-2", &name, document_type),
                );
            }
            let held = state.has_access_request_approval_policy("us-west-2");
            assert_eq!(
                held, *enabled,
                "case {case:?}: expected enabled == {enabled}, got {held}"
            );
            let auto_approved = state.access_requests_are_auto_approved("us-west-2");
            assert_eq!(
                auto_approved, *auto,
                "case {case:?}: expected auto-approved == {auto}, got {auto_approved}"
            );
            // A policy is a regional document, so it enables the Region it was created in
            // and no other.
            assert!(
                !state.has_access_request_approval_policy("eu-central-1"),
                "case {case:?}: a policy in one Region must not enable another"
            );
        }
    }

    /// RULE: the approval an auto-approval policy owes is materialised from the stored due
    /// instant — `Pending` before it, `Approved` at and after it — and a request whose
    /// decision is an approver's is owed nothing and stays `Pending`.
    #[test]
    fn the_approval_the_clock_owes_is_materialized_from_the_due_instant() {
        let raised = SystemTime::UNIX_EPOCH;
        let due = raised + APPROVAL_DELAY;
        // (case, the due instant, when it is read, the status it must report)
        let rows: &[(&str, Option<SystemTime>, SystemTime, &str)] = &[
            ("auto-approved, read at once", Some(due), raised, STATUS_PENDING),
            (
                "auto-approved, read just before the approval falls due",
                Some(due),
                due - Duration::from_millis(1),
                STATUS_PENDING,
            ),
            ("auto-approved, read when it falls due", Some(due), due, STATUS_APPROVED),
            (
                "auto-approved, read long after",
                Some(due),
                due + Duration::from_secs(3_600),
                STATUS_APPROVED,
            ),
            ("awaiting an approver, read at once", None, raised, STATUS_PENDING),
            (
                "awaiting an approver, read long after",
                None,
                raised + Duration::from_secs(86_400),
                STATUS_PENDING,
            ),
        ];
        for (case, approval_due, read_at, expected) in rows {
            let mut held = record("us-west-2", "oi-0123456789ab", *approval_due);
            held.materialize_due(*read_at);
            assert_eq!(
                held.status, *expected,
                "case {case:?}: expected status {expected}, got {:?}",
                held.status
            );
        }
    }

    /// RULE: a minted `AccessRequestId` satisfies the modelled pattern, and two starts of one
    /// store never collide.
    #[test]
    fn a_minted_id_has_the_modelled_form_and_is_unique() {
        let mut state = super::super::ComAmazonawsSsmAmazonssmState::default();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut minted = Vec::new();
        for round in 0..8 {
            let id = state.next_access_request_id(at);
            let digits = id.strip_prefix(super::super::ops_items::OPS_ITEM_ID_PREFIX);
            let well_formed = digits.is_some_and(|digits| {
                digits.len() == super::super::ops_items::OPS_ITEM_ID_HEX_DIGITS
                    && digits
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            });
            assert!(
                well_formed,
                "case {round}: expected an id of the form oi-<12 lowercase hex>, got {id:?}"
            );
            assert!(
                !minted.contains(&id),
                "case {round}: expected a fresh id, got the already minted {id:?}"
            );
            state.insert_access_request(record("us-west-2", &id, None));
            minted.push(id);
        }
    }

    /// RULE: the credentials a read reports are the ones held against the request while they
    /// are valid, and none once they have run out.
    #[test]
    fn a_held_credential_is_reported_until_it_expires() {
        let vended_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut held = record("us-west-2", "oi-0123456789ab", None);
        held.credential = Some(VendedCredential {
            access_key_id: "AKIAEXAMPLEEXAMPLE12".to_string(),
            secret_access_key: "secret".to_string(),
            session_token: "token".to_string(),
            expires_at: vended_at + CREDENTIAL_LIFETIME,
        });
        // (case, when it is read, whether the held credential is reported)
        let rows: &[(&str, SystemTime, bool)] = &[
            ("at once", vended_at, true),
            (
                "a minute before it expires",
                vended_at + CREDENTIAL_LIFETIME - Duration::from_secs(60),
                true,
            ),
            ("when it expires", vended_at + CREDENTIAL_LIFETIME, false),
            (
                "after it expires",
                vended_at + CREDENTIAL_LIFETIME + Duration::from_secs(1),
                false,
            ),
        ];
        for (case, read_at, expected) in rows {
            let reported = held.live_credential(*read_at).is_some();
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected a reported credential == {expected}, got {reported}"
            );
        }
    }
}
