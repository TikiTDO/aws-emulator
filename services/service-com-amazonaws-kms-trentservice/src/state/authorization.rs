// SPDX-License-Identifier: CC0-1.0

//! Who a request to this service speaks for, and whether the authorities in force let it
//! do what it asks.
//!
//! KMS is one of the services whose permission model is not "the caller is the account":
//! a request's permission comes from one of THREE authorities, and a request none of them
//! allows is refused with `AccessDeniedException` (HTTP 400) before anything is read or
//! written:
//!
//! 1. the KEY's own key policy, the resource-based document the key carries
//!    ([`super::Cmk::policy`], which `CreateKey`/`PutKeyPolicy` wrote);
//! 2. the calling principal's IDENTITY-based policies, read out of the IAM store;
//! 3. the GRANTS on that key the caller holds — the ones whose grantee principal is the
//!    caller, and the ones the request presents a grant token for. "Grants are an
//!    alternative to key policies" and they are withdrawn by `RetireGrant` /
//!    `RevokeGrant`, so the authority disappears from the very next request.
//!
//! One divergence from the real service is deliberate and is the one the handlers'
//! module docs record: a request that PRESENTS a grant token is treated as drawing its
//! permission from the grants those tokens name, rather than additively beside the two
//! policies. See [`authorize`].
//!
//! An operation that names NO key has no key policy to ask, so only the second authority
//! is in force: its permission comes from the calling principal's identity-based policies
//! alone, over the whole service in this Region ([`authorize_account_action`]).
//!
//! The pieces, each held in exactly one place — the same shape every other service's own
//! `state/authorization.rs` takes, because this is one rule and not several:
//!
//! * **who** — the principal the request's access key id speaks for: the session STS
//!   minted that key for (the same place `sts:GetCallerIdentity` answers from), else the
//!   IAM user that owns the long-term key, else the account's own published credential.
//!   Resolving it materialises the session expiries the clock has already made due, a
//!   write owed at that instant whether or not anybody called.
//! * **the documents** — the key policy out of this service's own store, and the
//!   principal's inline and attached policies out of IAM's own store, read through IAM's
//!   OWN `principal_policies` accessor — where the collection rule lives — so this service
//!   sees exactly what those handlers put in force, including the default version of an
//!   attached policy whether AWS or this account owns it.
//! * **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!   evaluation of policy documents against `(action, resource, principal)`, so this
//!   service and every other one handed the same document decide alike.
//!
//! ## Why a statement naming the ACCOUNT is not a grant on its own
//!
//! The default key policy "gives the Amazon Web Services account that owns the KMS key
//! permission to use it in all cryptographic operations"
//! (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`) by naming the
//! account's root user. Such a statement DELEGATES the decision to that account's own
//! identity-based policies rather than granting to every principal in the account: a
//! principal whose identity policies say nothing is not allowed by it, while the
//! account's own credential — which no identity policy governs — is. A statement naming
//! the principal ITSELF grants outright. The two principal spellings that express that
//! split are the shared evaluator's ([`iam::account_principals`]); a DENY is decided
//! against both sets at once, because denying an account denies each of its principals.
//!
//! ## The callers that are not IAM principals
//!
//! * **The account's own published credential.** A request whose access key id neither
//!   STS nor IAM holds a record for speaks for the account itself, which holds no
//!   identity policies — the position AWS account root credentials are in.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-MANAGED
//!   policy AWS publishes but this emulator's catalog does not carry has no document in
//!   the store, so refusing on evidence this emulator does not have would deny access AWS
//!   permits. A policy the store DOES resolve a document for — an account-owned one, or a
//!   published AWS-managed one — is read like any other document and bounds the principal
//!   as it is written.
//! * **An AWS SERVICE acting for one of its own resources.** A cryptographic operation
//!   another service performs as its OWN service principal — CloudFront opening the bytes
//!   of an origin object for a fetch its origin access control signed — is made by no
//!   principal of this account at all ([`Caller::service_principal`]): it holds no
//!   identity-based policies and naming the account grants it nothing, so the key policy is
//!   the whole decision, and what it carries beside its principal is the resource it acts
//!   for.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused.
//!
//! ## The condition keys a statement is judged under
//!
//! A statement applies to a request only when the `Condition` block it carries holds for
//! that request, whichever effect it states — so a statement scoped to one principal
//! governs that principal alone, and one scoped to an encryption-context entry governs the
//! requests carrying that entry alone. The keys this gate can answer EXACTLY are the ones
//! whose value the REQUEST itself carries, and they are in the request context of every
//! call ([`request_conditions`]): the ARN of the principal that made the request and the
//! account it belongs to, and one key per entry of the request's encryption context, beside
//! the multi-valued key naming those entries. The encryption-context family is answered for
//! as a WHOLE, so a clause over an entry the request does not carry is decided as absent
//! rather than left undecided. `kms:ViaService` is answered for the requests that have a
//! value for it — the ones another service makes on the caller's behalf, which name
//! themselves through [`RequestedUse::through_service`] — and for a request a caller makes
//! directly to this service there is no such value, so it is left unpublished there. A
//! request another service makes as its own service principal answers instead for
//! `aws:SourceArn` and `aws:SourceAccount` — the keys a grant to such a principal is scoped
//! by — and for no principal ARN, since no IAM entity made it. Every
//! other key is left unpublished too, so a statement naming one stays undecided and the
//! shared evaluator reads it fail-closed.

use std::collections::HashMap;
use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The global condition keys that name the PRINCIPAL a request was signed by, which this
/// gate answers exactly: "aws:PrincipalArn — the ARN of the principal that made the
/// request" and "aws:PrincipalAccount — the account the principal belongs to" (IAM's
/// global condition keys).
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";
const PRINCIPAL_ACCOUNT_KEY: &str = "aws:PrincipalAccount";

/// The condition-key FAMILY naming one entry of the request's encryption context:
/// "kms:EncryptionContext:<context-key> … compares the encryption context pair in the
/// request with the pair specified in the policy" (KMS's own condition keys). The family is
/// published WHOLE, because this gate holds the request's entire encryption context — empty
/// for a request that carries none — and can therefore answer for any member of it,
/// including answering that the request carries no value for that member.
const ENCRYPTION_CONTEXT_KEY_FAMILY: &str = "kms:EncryptionContext:";

/// The condition key naming the KEYS of the request's encryption context, one value per
/// entry: "kms:EncryptionContextKeys … compares the encryption context keys in the request
/// with those specified in the policy" (KMS's own condition keys). A multi-valued key, so a
/// statement decides it under one of IAM's set qualifiers.
const ENCRYPTION_CONTEXT_KEYS_KEY: &str = "kms:EncryptionContextKeys";

/// The global condition keys that name the RESOURCE a request was made ON BEHALF OF, and
/// the account holding it: "aws:SourceArn — the ARN of the resource that is making a
/// service-to-service request" and "aws:SourceAccount — the account the resource making a
/// service-to-service request belongs to" (IAM's global condition keys). Only a request
/// another AWS service makes as its own service principal for one of its resources has a
/// value for either, and a key policy written for such a caller is scoped by them.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The condition key naming the service a request was made THROUGH: "kms:ViaService …
/// limits use of a KMS key to requests from particular Amazon Web Services services" (KMS's
/// own condition keys). Its value is that service's regional endpoint name, and only a
/// request another service makes on a caller's behalf has one.
const VIA_SERVICE_KEY: &str = "kms:ViaService";

/// The most entries of a request's encryption context this gate copies out of the request
/// to decide a condition over.
///
/// A policy scopes a key to a handful of named entries, so this is far above what a
/// statement is written against; it is here so that what one request makes this gate hold
/// and scan is bounded by a constant rather than by a count the caller chooses. A request
/// carrying more entries than this is judged with NO encryption-context key answered for,
/// so a statement conditioned on one stays undecided and is read fail-closed.
pub const MAX_PUBLISHED_ENCRYPTION_CONTEXT_ENTRIES: usize = 64;

/// The principal a request speaks for, with everything a decision about it is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// The spellings a policy statement names this principal ITSELF by: the session ARN
    /// STS reports, and the ARN of the IAM role behind an assumed-role session. A
    /// statement naming one of these grants to the caller outright.
    pub principals: Vec<String>,
    /// The spellings a statement names the caller's whole ACCOUNT by — see the module
    /// docs for what naming the account does and does not grant.
    pub account: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when this request presents the account's own published credential.
    pub account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does
    /// not hold — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
    /// Set for a request another AWS SERVICE made as its own service principal on behalf of
    /// one of its resources ([`Caller::service_principal`]): the resource that request was
    /// made for. `None` for a request a caller made with a credential of its own, which has
    /// no value for [`SOURCE_ARN_KEY`] at all.
    pub on_behalf_of: Option<CallingResource>,
}

/// The resource a service-to-service request was made ON BEHALF OF, as the condition keys
/// a grant to a service principal is scoped by name it.
#[derive(Debug, Clone)]
pub struct CallingResource {
    /// The resource's ARN — the value of [`SOURCE_ARN_KEY`].
    pub arn: String,
    /// The account holding it — the value of [`SOURCE_ACCOUNT_KEY`], empty for an ARN that
    /// names no account, which declares no value for that key rather than a guess.
    pub account: String,
}

impl Caller {
    /// The caller of a request an AWS SERVICE made as its own service principal on behalf of
    /// one of its own resources — the shape of a cryptographic operation CloudFront performs
    /// for an origin fetch its origin access control signed, which arrives as the
    /// `cloudfront.amazonaws.com` principal with `aws:SourceArn` naming the distribution.
    ///
    /// Such a caller is NOT a principal of the account: it holds no identity-based policies
    /// (there is no IAM entity to attach one to), and naming the account grants it nothing,
    /// so the KEY's own policy is the only document in force over it — which is why a key
    /// whose policy does not name the service principal refuses it even when the account's
    /// own credential would be served. What it carries instead is the resource it acts for,
    /// which is what a grant to that principal is scoped by ([`CallingResource`]).
    #[must_use]
    pub fn service_principal(principal: &str, source_arn: &str) -> Self {
        let account = ::service_common::arn::parse(source_arn)
            .map(|arn| arn.account.to_string())
            .unwrap_or_default();
        Self {
            arn: principal.to_string(),
            principals: vec![principal.to_string()],
            account: Vec::new(),
            documents: Vec::new(),
            account_credential: false,
            unreadable_attachment: false,
            on_behalf_of: Some(CallingResource {
                arn: source_arn.to_string(),
                account,
            }),
        }
    }

    /// Whether `grant` was issued to THIS caller, so the permissions it carries are the
    /// caller's: "the grantee principal" is the identity a grant gives permission to, and
    /// a session of a role is the role it is a session of.
    #[must_use]
    pub fn holds(&self, grant: &super::Grant) -> bool {
        grant.grantee_principal.as_deref().is_some_and(|grantee| {
            self.principals
                .iter()
                .any(|principal| principal == grantee)
        })
    }

    /// Whether this caller's identity-based half of the decision cannot refuse: the
    /// account's own credential, or a principal carrying a grant this emulator cannot
    /// read (see the module docs).
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// OWN accessor: the documents in force on it — its inline policies and the default
    /// version of each managed policy attached to it, whether that policy belongs to this
    /// account or to AWS — and whether it carries an attachment whose document the store
    /// holds none for.
    ///
    /// The collection rule itself lives in IAM
    /// ([`iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies`]),
    /// so this service sees exactly what `PutRolePolicy`/`AttachRolePolicy` put in force
    /// and reads a principal's permissions as IAM's own authorizer reads them. Only a
    /// policy the store resolves NO document for leaves part of the principal's authority
    /// unknown, which is what [`Self::unreadable_attachment`] stands for.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
    }
}

/// Whether the authorities in force let `caller` perform `kms:<operation>` on `key`, and
/// the refusal message when they do not.
///
/// `presented` is `Some` of the grants the request's own grant tokens name — the handler
/// resolves them, because a token this service never minted is a different, modelled
/// refusal — and `None` for a request that presents no grant token at all. A request that
/// DOES present one is treated as drawing its permission from the grants those tokens
/// name, which is narrower than the real service, where a grant token is additive: that
/// is this emulator's one documented divergence here, and each handler's module doc
/// records it.
///
/// `context` is the request's encryption context, which a grant's
/// `EncryptionContextEquals` / `EncryptionContextSubset` constraint is judged against, which
/// the `kms:` condition keys of a conditioned policy statement are decided against, and
/// which an operation that takes none passes as `None`.
///
/// This is the decision for a request a caller made to this service DIRECTLY;
/// [`authorize_use`] is the same decision for one another service makes on the caller's
/// behalf.
pub fn authorize(
    caller: &Caller,
    state: &super::ComAmazonawsKmsTrentserviceState,
    region: &str,
    key: &super::Cmk,
    operation: &str,
    context: Option<&HashMap<String, String>>,
    presented: Option<&[&super::Grant]>,
) -> Result<(), String> {
    authorize_use(
        caller,
        state,
        region,
        key,
        RequestedUse::direct(operation),
        context,
        presented,
    )
}

/// What a request asks of a key: the KMS operation, and the AWS service the request
/// arrived THROUGH, for one another service performs on the caller's behalf.
#[derive(Debug, Clone, Copy)]
pub struct RequestedUse<'a> {
    /// The KMS operation, without this service's `kms:` namespace.
    operation: &'a str,
    /// The regional endpoint name of the service the request arrived through, which is
    /// the value of [`VIA_SERVICE_KEY`]; `None` for a request made to this service
    /// directly, which has no value for that key.
    via_service: Option<&'a str>,
}

impl<'a> RequestedUse<'a> {
    /// `operation`, as a caller asked it of this service directly.
    #[must_use]
    pub fn direct(operation: &'a str) -> Self {
        Self {
            operation,
            via_service: None,
        }
    }

    /// `operation`, as another service performs it on the caller's behalf. `via_service`
    /// is that service's regional endpoint name.
    #[must_use]
    pub fn through_service(operation: &'a str, via_service: &'a str) -> Self {
        Self {
            operation,
            via_service: Some(via_service),
        }
    }
}

/// [`authorize`] for a use of the key that a request may have reached THROUGH another AWS
/// service, which performs the cryptographic operation on the caller's behalf.
///
/// A statement scoped by `kms:ViaService` governs exactly the requests that arrive through
/// the service it names, so the decision is made with the value
/// [`RequestedUse::through_service`] carries; a request made to this service directly has
/// no value for that key and is the whole of [`authorize`].
pub fn authorize_use(
    caller: &Caller,
    state: &super::ComAmazonawsKmsTrentserviceState,
    region: &str,
    key: &super::Cmk,
    requested: RequestedUse<'_>,
    context: Option<&HashMap<String, String>>,
    presented: Option<&[&super::Grant]>,
) -> Result<(), String> {
    let RequestedUse {
        operation,
        via_service,
    } = requested;
    let action = format!("{}:{operation}", super::ARN_SERVICE);

    // The condition keys this request answers for: the values it carries, and the names it
    // DECLARES it models. Without them a statement scoped by a condition could not be
    // decided at all, and the evaluator's fail-closed reading would apply a `Deny` to
    // requests it does not cover — which is what a conditioned statement is written to
    // exclude.
    let conditions = request_conditions(caller, context, via_service);
    let request_context = conditions.carried.as_slice();
    let published = conditions.published.as_slice();

    // An explicit `Deny` wins over every grant, whichever document states it, and
    // denying the ACCOUNT denies each of its principals — so the deny is decided against
    // the caller's own spellings and its account's together.
    let mut denying = caller.principals.clone();
    denying.extend_from_slice(&caller.account);
    let deny_request = iam::Request::new(&action, &key.arn, &denying)
        .with_context(request_context)
        .with_published_keys(published);
    if iam::resource_decision(&key.policy, &deny_request) == Decision::Denied {
        return Err(explicit_deny_message(
            &caller.arn,
            &action,
            &key.arn,
            "a resource-based policy",
        ));
    }
    let granting = iam::Request::new(&action, &key.arn, &caller.principals)
        .with_context(request_context)
        .with_published_keys(published);
    let identity_said = iam::identity_decision(&caller.documents, &granting);
    if identity_said == Decision::Denied {
        return Err(explicit_deny_message(
            &caller.arn,
            &action,
            &key.arn,
            "an identity-based policy",
        ));
    }

    // WHICH document has to change is what AWS's two implicit-deny sentences say, and the
    // key policy decides which one applies: a policy that does not allow the action for
    // this caller or its account withholds it outright, and no identity policy can widen a
    // key policy that grants nothing — while a policy that DELEGATES to the account leaves
    // the principal's own identity policies as the permission it is missing. The key
    // policy delegates only when the statement that allows the account APPLIES to this
    // request, so a delegation gated on a condition the request does not satisfy is no
    // delegation and the key policy is the document that withheld the action.
    let delegates_to_account = || {
        !caller.account.is_empty()
            && iam::resource_decision(
                &key.policy,
                &iam::Request::new(&action, &key.arn, &caller.account)
                    .with_context(request_context)
                    .with_published_keys(published),
            ) == Decision::Allowed
    };
    let no_allow_message = |delegates: bool| {
        if delegates {
            no_identity_policy_message(&caller.arn, &action, &key.arn)
        } else {
            no_resource_policy_message(&caller.arn, &action, &key.arn)
        }
    };

    // A grant permits exactly the operations it lists, on the one key it covers, under
    // its own encryption-context constraints. The grants in force are those the request's
    // tokens name — and, for a request that presents none, those ISSUED to the caller:
    // "the grantee principal" holds the grant's permissions without presenting anything,
    // which is why `RetireGrant` and `RevokeGrant` withdraw its access.
    if let Some(presented) = presented {
        return presented
            .iter()
            .any(|grant| grant.permits(&key.key_id, operation, context))
            .then_some(())
            .ok_or_else(|| no_allow_message(delegates_to_account()));
    }
    let by_grant = state
        .grants_for_key(region, &key.key_id)
        .into_iter()
        .any(|grant| caller.holds(grant) && grant.permits(&key.key_id, operation, context));
    if by_grant {
        return Ok(());
    }

    // A key-policy statement naming the principal itself grants outright; one naming the
    // account delegates to that account's identity policies (see the module docs).
    if iam::resource_decision(&key.policy, &granting) == Decision::Allowed {
        return Ok(());
    }
    let delegates = delegates_to_account();
    if delegates && (caller.identity_is_unbounded() || identity_said == Decision::Allowed) {
        return Ok(());
    }
    Err(no_allow_message(delegates))
}

/// Whether a key policy document the request is attaching to the key `key_arn` leaves
/// `caller` a path to a subsequent `kms:PutKeyPolicy` on that key — the POLICY LOCKOUT
/// SAFETY CHECK.
///
/// "The key policy must allow the calling principal to make a subsequent PutKeyPolicy
/// request on the KMS key. This reduces the risk that the KMS key becomes unmanageable"
/// (`smithy_query(query="doc",shape="PutKeyPolicyRequest",member="Policy")`), so the
/// question is the one [`authorize_use`] answers for every other request, asked of the
/// CANDIDATE document and of the one action that can replace it.
///
/// A statement naming the caller ITSELF keeps the path open outright, and one naming the
/// caller's ACCOUNT keeps it open through that account's own identity policies, which the
/// account can still write — the delegation the module docs record, which is why the
/// default key policy passes this check. An explicit `Deny` over the caller or its
/// account closes the path whatever else the document allows, and a conditioned allow
/// this gate cannot decide for a future request leaves no path either.
///
/// Identity-based policies and grants are not consulted: neither can authorize an action
/// on a key whose own policy withholds it, and a grant carries only the cryptographic
/// operations a grant may carry.
#[must_use]
pub fn permits_policy_management(caller: &Caller, policy: &str, key_arn: &str) -> bool {
    let action = format!("{}:{POLICY_MANAGEMENT_OPERATION}", super::ARN_SERVICE);
    let conditions = request_conditions(caller, None, None);
    let mut principals = caller.principals.clone();
    principals.extend_from_slice(&caller.account);
    let request = iam::Request::new(&action, key_arn, &principals)
        .with_context(&conditions.carried)
        .with_published_keys(&conditions.published);
    iam::resource_decision(policy, &request) == Decision::Allowed
}

/// The operation whose availability the policy-lockout safety check is about: the one
/// that REPLACES a key's policy, so a document that withholds it is a document nothing
/// can replace.
const POLICY_MANAGEMENT_OPERATION: &str = "PutKeyPolicy";

/// Whether the authorities in force let `caller` perform an ACCOUNT-LEVEL
/// `kms:<operation>` — one that names no KMS key — and the refusal message when they do
/// not.
///
/// Such an operation reaches no key, so no key policy is in force and the whole decision
/// is the calling principal's identity-based policies: the permission KMS documents for
/// one of these is an IAM-policy permission
/// (`smithy_query(query="doc",shape="ListAliases")`: "Required permissions:
/// kms:ListAliases (IAM policy)"), and a principal whose identity policies do not allow
/// the action holds no other authority to fall back on — a grant covers one key and a key
/// policy governs one key, so neither can carry a request that is about neither.
///
/// The resource the request is judged against is the whole service in `region`
/// ([`account_resource`]), because an action with no resource type is written in a policy
/// against the service's wildcard ARN.
pub fn authorize_account_action(
    caller: &Caller,
    region: &str,
    operation: &str,
) -> Result<(), String> {
    let action = format!("{}:{operation}", super::ARN_SERVICE);
    let resource = account_resource(region);

    // The same request context every key-scoped decision answers for, so one statement
    // scoped to a principal is read the same way whichever operation asks. An operation
    // that names no key takes no encryption context either, so the request carries no
    // entry of that family.
    let conditions = request_conditions(caller, None, None);
    let request = iam::Request::new(&action, &resource, &caller.principals)
        .with_context(&conditions.carried)
        .with_published_keys(&conditions.published);

    match iam::identity_decision(&caller.documents, &request) {
        // An explicit `Deny` wins over every `Allow`, whichever of the caller's documents
        // states it.
        Decision::Denied => Err(explicit_deny_message(
            &caller.arn,
            &action,
            &resource,
            "an identity-based policy",
        )),
        Decision::Allowed => Ok(()),
        // A caller whose identity-based half cannot refuse (see the module docs) is
        // served: refusing on evidence this emulator does not hold would deny access AWS
        // permits.
        _ if caller.identity_is_unbounded() => Ok(()),
        _ => Err(no_identity_policy_message(&caller.arn, &action, &resource)),
    }
}

/// Whether the authorities in force let `caller` ADMINISTER `key` — change the key itself
/// rather than use it — and the refusal message when they do not.
///
/// A key whose `KeyManager` is Amazon Web Services ([`super::KEY_MANAGER_AWS`]) is one
/// Amazon Web Services manages rather than one the customer created: "KMS keys in your
/// Amazon Web Services account are either customer managed or Amazon Web Services managed"
/// (`smithy_query(query="doc",shape="KeyMetadata",member="KeyManager")`), and the manager of
/// a key is who administers it. The key policy such a key carries is the owning service's
/// own, written to let that service use the key for the account's resources and to let the
/// account READ the key — so it delegates no administration of the key to the account, and a
/// call from the account that would CHANGE the key is refused for want of permission with
/// the sentence AWS words for an action no resource-based policy allows
/// ([`no_resource_policy_message`]), leaving the key as it was.
///
/// Which calls those are is what the key record holds: a call that writes the key's key
/// state, its key material, its key policy, its description, its tags or its multi-Region
/// layout administers it, while reading its metadata and using it in a cryptographic
/// operation do not and are decided by [`authorize_use`] like any other use.
///
/// A key the CUSTOMER manages is administered by the account that created it, so this
/// refuses nothing on one. One function for every operation that administers a key, so no
/// two of them decide the same key differently.
pub fn authorize_key_administration(
    caller: &Caller,
    key: &super::Cmk,
    operation: &str,
) -> Result<(), String> {
    if key.key_manager != super::KEY_MANAGER_AWS {
        return Ok(());
    }
    Err(no_resource_policy_message(
        &caller.arn,
        &format!("{}:{operation}", super::ARN_SERVICE),
        &key.arn,
    ))
}

/// [`authorize_key_administration`] on an operation's own error channel: the
/// `AccessDeniedException` (HTTP 400) that refuses a caller who may not administer the key,
/// which is what AWS answers whatever this service's model declares.
///
/// `caller` is read BEFORE this service's store is locked ([`caller`] reaches other
/// services' stores); `key` is the record the request resolved, so the decision is made
/// about the key the call would change.
pub fn check_key_administration<E>(
    caller: &Caller,
    key: &super::Cmk,
    operation: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_key_administration(caller, key, operation)
        .map_err(crate::scaffolded::Scaffolded::access_denied)
}

/// The resource an ACCOUNT-LEVEL action is judged against: every KMS resource of this
/// account in `region`, which is what an action carrying no resource type is written
/// against in a policy. Built through [`super::ARN_SERVICE`] and the engine's one ARN
/// builder, the same way [`super::key_arn`] names a key.
#[must_use]
pub fn account_resource(region: &str) -> String {
    ::engine::emulator_arn(super::ARN_SERVICE, region, "*")
}

/// The whole account-level gate an implemented operation needs, on that operation's own
/// error channel: who the request speaks for, whether its identity-based policies allow
/// `kms:<operation>` in the Region the request was signed for, and the
/// `AccessDeniedException` (HTTP 400) that refuses it when they do not.
///
/// One call per handler, so no two account-level operations gate themselves differently —
/// and it reaches no store of this service, so a handler calls it before locking its own.
pub fn check_account_action<E, Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let caller = caller(engine, ctx);
    let region = super::request_region(ctx);
    authorize_account_action(&caller, &region, operation)
        .map_err(crate::scaffolded::Scaffolded::access_denied)
}

/// The condition keys a statement of these documents is judged under: what the request
/// carries a value for, and the names it DECLARES it models.
///
/// A key the request declares is one a clause can be DECIDED over — which is what scopes a
/// conditioned statement to the principals it names and to the encryption contexts it
/// covers — while a clause over any other key stays undecided and is read fail-closed. The
/// declared names are a fixed handful whatever the request carries, so deciding one clause
/// costs the same whatever the caller sent.
#[derive(Debug)]
struct RequestConditions {
    /// The `(key, value)` pairs the request carries, one pair per value.
    carried: Vec<(String, String)>,
    /// The names the request models, each either a key or a whole family.
    published: Vec<&'static str>,
}

/// The condition keys THIS request answers for.
///
/// A caller that speaks for nobody (an expired session credential) carries no principal
/// spelling, so there is nothing to compare and no principal key is declared; the account is
/// read out of the principal's own ARN through the tree's one ARN parse, so an ARN carrying
/// none declares no account key rather than a guess.
///
/// `context` is the request's encryption context, `None` for an operation that takes none.
/// Each entry answers for its own key of the [`ENCRYPTION_CONTEXT_KEY_FAMILY`] and
/// contributes one value of the multi-valued [`ENCRYPTION_CONTEXT_KEYS_KEY`], so a statement
/// conditioned on an entry decides against the values the REQUEST carried — and the family
/// is declared as a whole, so a clause over an entry the request does not carry is decided
/// as absent (IAM: "if the key is not present … a negated condition operator returns true")
/// rather than left undecided.
///
/// Past [`MAX_PUBLISHED_ENCRYPTION_CONTEXT_ENTRIES`] entries none of them is answered for:
/// what this gate copies out of a request is bounded by that constant rather than by a
/// count the caller chooses, and a statement conditioned on an encryption context stays
/// undecided for such a request, which the shared evaluator reads fail-closed.
///
/// `via_service` is the service a request arrived THROUGH, `None` for one made to this
/// service directly: a request that reaches the key through another service carries that
/// service's endpoint name as the value of [`VIA_SERVICE_KEY`], and one made directly
/// carries no value for it at all, so the key is not declared and a clause over it stays
/// undecided rather than being decided as absent.
fn request_conditions(
    caller: &Caller,
    context: Option<&HashMap<String, String>>,
    via_service: Option<&str>,
) -> RequestConditions {
    let mut carried = Vec::new();
    let mut published = Vec::new();
    // A request another service made AS ITS OWN SERVICE PRINCIPAL answers for the resource it
    // was made on behalf of, and for no principal ARN: a service principal is not an IAM
    // entity, so there is no `aws:PrincipalArn` value to compare and a clause over one stays
    // undecided rather than being decided against a service name.
    if let Some(source) = &caller.on_behalf_of {
        carried.push((SOURCE_ARN_KEY.to_string(), source.arn.clone()));
        published.push(SOURCE_ARN_KEY);
        if !source.account.is_empty() {
            carried.push((SOURCE_ACCOUNT_KEY.to_string(), source.account.clone()));
            published.push(SOURCE_ACCOUNT_KEY);
        }
    } else if !caller.principals.is_empty() {
        carried.push((PRINCIPAL_ARN_KEY.to_string(), caller.arn.clone()));
        published.push(PRINCIPAL_ARN_KEY);
        let account = ::service_common::arn::parse(&caller.arn)
            .map(|arn| arn.account)
            .filter(|account| !account.is_empty());
        if let Some(account) = account {
            carried.push((PRINCIPAL_ACCOUNT_KEY.to_string(), account.to_string()));
            published.push(PRINCIPAL_ACCOUNT_KEY);
        }
    }
    let answerable =
        context.is_none_or(|held| held.len() <= MAX_PUBLISHED_ENCRYPTION_CONTEXT_ENTRIES);
    if answerable {
        for (name, value) in context.into_iter().flatten() {
            carried.push((
                format!("{ENCRYPTION_CONTEXT_KEY_FAMILY}{name}"),
                value.clone(),
            ));
            carried.push((ENCRYPTION_CONTEXT_KEYS_KEY.to_string(), name.clone()));
        }
        published.push(ENCRYPTION_CONTEXT_KEY_FAMILY);
        published.push(ENCRYPTION_CONTEXT_KEYS_KEY);
    }
    if let Some(via_service) = via_service {
        carried.push((VIA_SERVICE_KEY.to_string(), via_service.to_string()));
        published.push(VIA_SERVICE_KEY);
    }
    RequestConditions { carried, published }
}

/// How AWS words an authorization failure that no document allowed: who was refused,
/// which action, on which resource, and that no identity-based policy allows the action.
/// Every value in it comes from the request and the caller's credential.
///
/// This is the sentence for a key whose policy DELEGATES the action to the caller's
/// account, where the missing permission is the principal's own; a key policy that allows
/// the action to neither the caller nor its account is withholding it itself and is
/// refused with [`no_resource_policy_message`] instead.
///
/// One function for the whole service, so no two operations word the same refusal
/// differently.
#[must_use]
pub fn no_identity_policy_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// How AWS words an authorization failure the RESOURCE's own policy withheld: who was
/// refused, which action, on which resource, and that no resource-based policy allows the
/// action — the sentence for a key whose policy does not allow the action for this caller
/// or for its account at all, which is the document that has to change.
///
/// The sentence itself is the tree's one spelling of it
/// ([`iam::not_authorized_by_resource_policy`]), so this service cannot drift from the
/// other services that answer the same refusal class; this wrapper keeps the key's ARN and
/// the `kms:` action the only things a caller here has to supply.
#[must_use]
pub fn no_resource_policy_message(principal: &str, action: &str, resource: &str) -> String {
    iam::not_authorized_by_resource_policy(principal, action, resource)
}

/// How AWS words an authorization failure a document explicitly denied, naming which kind
/// of document denied it.
#[must_use]
fn explicit_deny_message(principal: &str, action: &str, resource: &str, document: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in {document}"
    )
}

/// The principal this request speaks for, and the documents in force on it.
///
/// Called BEFORE this service's own store is locked, so the STS and IAM stores it reads
/// are never reached while this service's state guard is held.
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
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
        // The session credential has expired, so it speaks for nobody.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                principals: Vec::new(),
                account: Vec::new(),
                documents: Vec::new(),
                account_credential: false,
                unreadable_attachment: false,
                on_behalf_of: None,
            }
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        account: iam::account_principals(&identity.arn),
        arn: identity.arn,
        documents: Vec::new(),
        unreadable_attachment: false,
        on_behalf_of: None,
    };
    let Some(iam_state) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam_state.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it
    // never minted is the account's ambient identity — but IAM's store maps an access key
    // id to the user that owns it, so a request signed with one speaks for THAT user and
    // is judged on the user's policies rather than served as the account.
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
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the session
        // ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            let role_arn = held.arn.clone();
            caller.read_policies_of(&store, &role_arn);
        }
    } else if iam_user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// The principal an IAM ROLE is, with the documents in force on it: the caller of a
/// request another AWS service makes AS a role one of its own resources names — a state
/// machine's execution role, which is the identity Step Functions uses that state
/// machine's key as.
///
/// Such a caller holds exactly what a session of the role holds: the role's ARN is the
/// spelling a key-policy statement names it by, the role's own identity-based policies
/// are the documents in force on it, and a statement naming the ACCOUNT delegates the
/// decision to those documents — the same two halves [`authorize_use`] weighs for a
/// session. It presents no credential of the account's own, so nothing about it is
/// unbounded: a role the IAM store holds no record for carries no documents, which
/// leaves the key's own policy as the only authority over it.
///
/// Call it BEFORE this service's store is locked: it takes IAM's own.
#[must_use]
pub fn role_caller<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: &str) -> Caller {
    let mut caller = Caller {
        arn: role_arn.to_string(),
        principals: vec![role_arn.to_string()],
        account: iam::account_principals(role_arn),
        documents: Vec::new(),
        account_credential: false,
        unreadable_attachment: false,
        on_behalf_of: None,
    };
    let Some(iam_state) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam_state.lock();
    caller.read_policies_of(&store, role_arn);
    caller
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role
/// is the first segment after `assumed-role/`.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The user name an IAM user ARN names: the LAST segment of `user/<path…>/<name>`, since
/// the path is optional and only the last segment is the name.
fn iam_user_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "iam")?;
    let name = resource.strip_prefix("user/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}

/// The resource part of an ARN whose service segment is `service`.
fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let parsed = ::service_common::arn::parse(arn)?;
    (parsed.vendor == service).then_some(parsed.resource)
}

