// SPDX-License-Identifier: CC0-1.0

//! The payment manager (`/payments/managers/`): one [`PaymentManagerRecord`] per
//! `paymentManagerId`, holding everything `CreatePaymentManager` persisted and the
//! status the clock owes it, so a create and every later read of the manager answer
//! from one store.
//!
//! Lives beside the stores in [`super`] rather than in any one `ops/` file, so the
//! create and every read of a payment manager cannot drift from each other.

use super::*;

use crate::ops::types::{CreatePaymentManagerOutput, GetPaymentManagerOutput};

/// The modelled not-found error for a payment manager this account does not hold.
///
/// This control plane words a missing resource in terms of the id the request named —
/// the sentence around the identifier is fixed, and the identifier itself is the
/// caller's own value — exactly as [`browser_not_found`] words a sibling resource.
/// Shared by the handlers rather than written per operation, so one payment manager
/// cannot be reported missing in two different sentences by two operations.
pub fn payment_manager_not_found(payment_manager_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Payment manager with ID {payment_manager_id} does not exist"
        )),
    }
}

/// The ARN resource type a payment manager carries. `PaymentManagerArn` ends in
/// `…:payment-manager/{paymentManagerId}` (`smithy_query shape=PaymentManagerArn`).
pub const PAYMENT_MANAGER_ARN_RESOURCE: &str = "payment-manager";

/// `arn:aws:bedrock-agentcore:{region}:{account}:payment-manager/{paymentManagerId}`
/// — `PaymentManagerArn`'s form.
pub fn payment_manager_arn(region: &str, payment_manager_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PAYMENT_MANAGER_ARN_RESOURCE}/{payment_manager_id}"),
    )
}

/// `PaymentManagerStatus` members this service drives. The enum's full value list is
/// `CREATING | UPDATING | DELETING | READY | CREATE_FAILED | UPDATE_FAILED |
/// DELETE_FAILED` (`shared_types::PAYMENT_MANAGER_STATUS_VALUES`, from `smithy_query
/// shape=PaymentManagerStatus`); the failure statuses belong to a provisioning that
/// failed, which nothing here makes happen.
pub const PAYMENT_MANAGER_STATUS_CREATING: &str = "CREATING";
pub const PAYMENT_MANAGER_STATUS_READY: &str = "READY";

/// How long a freshly created payment manager stays `CREATING` before the clock owes
/// it `READY`.
///
/// AWS documents no duration for the transition, so the span is chosen: long enough
/// that a caller acting straight after the create still observes the transient
/// status, short enough to keep a client script quick — the same span the sibling
/// resources' transitions take ([`super::REGISTRY_SETTLE_DELAY`]).
pub const PAYMENT_MANAGER_SETTLE_DELAY: Duration = super::REGISTRY_SETTLE_DELAY;

/// How many payment managers this emulator will hold.
///
/// `CreatePaymentManager` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is not a fidelity claim about that
/// quota but the emulator's own ceiling, set generously so no realistic workflow meets
/// it, so a caller looping on create cannot grow this process's memory without bound.
pub const MAX_PAYMENT_MANAGERS: usize = 1_000;

/// The member `CreatePaymentManager` refuses a `CUSTOM_JWT` request for when it
/// carries no authorizer configuration.
///
/// The model states the rule in prose on the operation itself — "If you specify
/// CUSTOM_JWT as the authorizerType, you must provide an authorizerConfiguration"
/// (`smithy_query doc shape=CreatePaymentManager`) — so the framework's constraint
/// walk cannot refuse it.
pub const AUTHORIZER_CONFIGURATION_MEMBER: &str = "authorizerConfiguration";

/// One payment manager: everything `CreatePaymentManager` persisted for it, and the
/// status the clock owes it.
///
/// Every read of the manager reports this record, so a create and a later read cannot
/// disagree about its configuration or status.
#[derive(Debug, Clone)]
pub struct PaymentManagerRecord {
    pub payment_manager_id: String,
    pub payment_manager_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub authorizer_type: String,
    pub authorizer_configuration: Option<AuthorizerConfiguration>,
    pub role_arn: String,
    pub kms_key_arn: Option<String>,
    /// The workload identity this manager runs under, minted 1:1 with it in the
    /// token vault's `default` directory exactly as a gateway's and a runtime's is
    /// ([`workload_identity_arn`]), so every read reports the one identity the
    /// create minted.
    pub workload_identity_details: Option<WorkloadIdentityDetails>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status the manager reports until [`Self::settles_at`].
    pub transient_status: String,
    /// The status it reports from then on.
    pub settled_status: String,
    /// When the clock owes the move from one to the other. Stored, never slept on: a
    /// read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
}

impl PaymentManagerRecord {
    /// The status at `now`: the transient one until the clock owes the transition,
    /// the settled one from then on. No read performs it — it is due at that instant
    /// whether or not anybody looks.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// The tags a response reports: the stored map for a manager that carries tags,
    /// and nothing for one that carries none — an empty map is nothing to report.
    fn reported_tags(&self) -> Option<std::collections::HashMap<String, String>> {
        (!self.tags.is_empty()).then(|| self.tags.clone().into_iter().collect())
    }

    /// This payment manager as `CreatePaymentManager`'s response envelope: every
    /// member `CreatePaymentManagerResponse` declares, read off the record just
    /// written. `CreatePaymentManagerResponse` declares no `description`, so the
    /// description the request carried is reported by a read of the manager.
    pub fn create_output(&self, now: SystemTime) -> CreatePaymentManagerOutput {
        CreatePaymentManagerOutput {
            payment_manager_arn: self.payment_manager_arn.clone(),
            payment_manager_id: self.payment_manager_id.clone(),
            name: self.name.clone(),
            authorizer_type: self.authorizer_type.clone(),
            authorizer_configuration: self.authorizer_configuration.clone(),
            role_arn: self.role_arn.clone(),
            workload_identity_details: self.workload_identity_details.clone(),
            created_at: timestamp(self.created_at),
            status: self.status_at(now).to_string(),
            tags: self.reported_tags(),
            kms_key_arn: self.kms_key_arn.clone(),
        }
    }

    /// This payment manager as `GetPaymentManager`'s response envelope — the members
    /// [`Self::create_output`] renders, off the same record, plus the two
    /// `GetPaymentManagerResponse` declares beyond it: the description the create
    /// persisted and the instant of the last write to the manager.
    pub fn get_output(&self, now: SystemTime) -> GetPaymentManagerOutput {
        let created = self.create_output(now);
        GetPaymentManagerOutput {
            payment_manager_arn: created.payment_manager_arn,
            payment_manager_id: created.payment_manager_id,
            name: created.name,
            description: self.description.clone(),
            authorizer_type: created.authorizer_type,
            authorizer_configuration: created.authorizer_configuration,
            role_arn: created.role_arn,
            workload_identity_details: created.workload_identity_details,
            created_at: created.created_at,
            last_updated_at: timestamp(self.updated_at),
            status: created.status,
            tags: created.tags,
            kms_key_arn: created.kms_key_arn,
        }
    }
}

impl ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Mint a `paymentManagerId` for `name` that no stored payment manager holds:
    /// the name lower-cased + `-` + 10 lower-case alphanumerics, per
    /// `PaymentManagerId`'s pattern `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`.
    ///
    /// `PaymentManagerName` is `^[a-zA-Z][a-zA-Z0-9]{0,47}$`, which admits upper case
    /// where the id's pattern does not, so the stem is the name lower-cased — the
    /// widest stem that pattern accepts for a given name.
    pub fn mint_payment_manager_id(&mut self, name: &str, at: SystemTime) -> String {
        let stem = name.to_lowercase();
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{stem}-{}", suffix_for(name, self.mint_counter, at));
            if !self.payment_managers.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The payment manager a previous request to `operation` carrying this
    /// `clientToken` produced, if the record is still held and the manager still
    /// exists.
    pub fn replayed_payment_manager_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let payment_manager_id = self
            .payment_manager_idempotency
            .get(&token_key(operation, client_token?))?;
        self.payment_managers
            .contains_key(payment_manager_id)
            .then(|| payment_manager_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held, for the same reason
    /// [`Self::remember_browser_client_token`] does.
    pub fn remember_payment_manager_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        payment_manager_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.payment_manager_idempotency,
            &mut self.payment_manager_idempotency_order,
            token_key(operation, &token),
            payment_manager_id.to_string(),
        );
    }
}
