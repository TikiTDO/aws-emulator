// SPDX-License-Identifier: CC0-1.0

//! The EKS Anywhere subscription slice of this service's state: the statuses and
//! spans a subscription moves through, the record the store keeps for one, its
//! projection onto the modelled shape, and the rules its term and license count
//! are held to.

use super::*;

/// The `EksAnywhereSubscriptionStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="EksAnywhereSubscriptionStatus"`) is
/// `CREATING | ACTIVE | UPDATING | EXPIRING | EXPIRED | DELETING`;
/// `EksAnywhereSubscription$status` is a plain string member, so the status is
/// stored as the wire token. `EXPIRING` is the only member no operation of this
/// service puts a subscription in: a term that runs out moves straight to
/// `EXPIRED` at its expiration date.
pub const SUBSCRIPTION_STATUS_CREATING: &str = "CREATING";
/// See [`SUBSCRIPTION_STATUS_CREATING`].
pub const SUBSCRIPTION_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`SUBSCRIPTION_STATUS_CREATING`] — the status a subscription reports
/// while an accepted `UpdateEksAnywhereSubscription` is being applied, until the
/// clock owes it `ACTIVE` again.
pub const SUBSCRIPTION_STATUS_UPDATING: &str = "UPDATING";
/// See [`SUBSCRIPTION_STATUS_CREATING`] — the status a subscription reports once
/// its term has run out, which the clock owes it at its expiration date.
pub const SUBSCRIPTION_STATUS_EXPIRED: &str = "EXPIRED";
/// See [`SUBSCRIPTION_STATUS_CREATING`] — the status a subscription reports
/// while an accepted `DeleteEksAnywhereSubscription` removes it, until the clock
/// owes the store its removal.
pub const SUBSCRIPTION_STATUS_DELETING: &str = "DELETING";

/// The only `EksAnywhereSubscriptionTermUnit` the API defines: "The term unit of
/// the subscription. Valid value is `MONTHS`"
/// (`EksAnywhereSubscriptionTerm$unit`).
pub const SUBSCRIPTION_TERM_UNIT_MONTHS: &str = "MONTHS";

/// The term durations a subscription may be created for: "Valid values are 12
/// and 36, indicating a 12 month or 36 month subscription"
/// (`EksAnywhereSubscriptionTerm$duration`).
pub const SUBSCRIPTION_TERM_DURATIONS: &[i32] = &[12, 36];

/// The license counts a subscription may be created with: "The number of
/// licenses to purchase with the subscription. Valid values are between 1 and
/// 100" (`CreateEksAnywhereSubscriptionRequest$licenseQuantity`). The member
/// carries a `@default` of 0 and no modelled range, so a count outside the
/// published one reaches the handler.
pub const MIN_SUBSCRIPTION_LICENSE_QUANTITY: i32 = 1;
/// See [`MIN_SUBSCRIPTION_LICENSE_QUANTITY`].
pub const MAX_SUBSCRIPTION_LICENSE_QUANTITY: i32 = 100;

/// The only `EksAnywhereSubscriptionLicenseType` the API defines: "The license
/// type for all licenses in the subscription. Valid value is CLUSTER"
/// (`CreateEksAnywhereSubscriptionRequest$licenseType`). The model's enum member
/// is `Cluster` (`smithy_query query="shape"
/// shape="EksAnywhereSubscriptionLicenseType"`), which is the token this slice
/// stores and reports.
pub const SUBSCRIPTION_LICENSE_TYPE_CLUSTER: &str = "Cluster";

/// How long a new subscription reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// `CreateEksAnywhereSubscription` publishes no duration for the provisioning it
/// starts (the licenses "are provisioned in Amazon Web Services License
/// Manager"), so this span is CHOSEN rather than cited, and matches
/// [`CLUSTER_ACTIVATION_DELAY`]: long enough that a caller which describes the
/// subscription straight after creating it still observes `CREATING`, short
/// enough to be worth waiting for. The due instant is stored on the record;
/// nothing sleeps for it.
pub const SUBSCRIPTION_ACTIVATION_DELAY: Duration = CLUSTER_ACTIVATION_DELAY;

/// How long a subscription whose delete was accepted reports `DELETING` before
/// the store drops it.
///
/// `DeleteEksAnywhereSubscription` publishes no duration for the removal it
/// starts, so this span is CHOSEN rather than cited, and matches
/// [`SUBSCRIPTION_ACTIVATION_DELAY`] for the same reason: non-zero, so a caller
/// that reads the subscription straight after deleting it still observes
/// `DELETING` rather than a subscription that was never there. The due instant is
/// stored on the record; nothing sleeps for it.
pub const SUBSCRIPTION_DELETION_DELAY: Duration = SUBSCRIPTION_ACTIVATION_DELAY;

/// The number of days in the month a subscription term is counted in.
///
/// A term is a count of MONTHS ([`SUBSCRIPTION_TERM_UNIT_MONTHS`]) and the
/// expiration date is "the Unix timestamp in seconds for when the subscription
/// will expire or auto renew" (`EksAnywhereSubscription$expirationDate`), so the
/// term has to be turned into a duration. Calendar months differ in length and
/// the API publishes no rule for which ones a term covers, so one month is taken
/// as 30 days — the span this emulator states once here rather than in each
/// caller.
pub const SUBSCRIPTION_MONTH: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The number of subscriptions this store holds, across every Region, before
/// `CreateEksAnywhereSubscription` answers `ResourceLimitExceededException`.
///
/// AWS publishes no quota for EKS Anywhere subscriptions, so the ceiling is
/// CHOSEN: it bounds the store, so repeated creates cannot grow it without
/// limit, and it is far above the handful of support contracts an account holds.
pub const MAX_STORED_SUBSCRIPTIONS: usize = 100;

/// One EKS Anywhere subscription, as this service stores it.
///
/// Everything an `EksAnywhereSubscription` reports is held here — including the
/// server-generated parts (`id`, `arn`, `createdAt`, the effective and
/// expiration dates and the licenses) — so a later read returns exactly what the
/// create wrote rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct EksAnywhereSubscriptionRecord {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub created_at: SystemTime,
    /// "The Unix timestamp in seconds for when the subscription is effective"
    /// (`EksAnywhereSubscription$effectiveDate`): a subscription is a contract
    /// that starts when it is bought, so this is the instant of the create.
    pub effective_date: SystemTime,
    /// "The Unix timestamp in seconds for when the subscription will expire or
    /// auto renew": the effective date plus the term the request asked for.
    pub expiration_date: SystemTime,
    pub license_quantity: i32,
    pub license_type: String,
    pub term_duration: i32,
    pub term_unit: String,
    pub status: String,
    /// When set, the instant at which the clock owes this subscription the move
    /// to `ACTIVE` — out of `CREATING` after the create, and out of `UPDATING`
    /// after an accepted update. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    /// When set, the instant at which the clock owes the store this
    /// subscription's removal: the record a `DeleteEksAnywhereSubscription`
    /// accepted, reporting `DELETING` until then.
    pub removed_at: Option<SystemTime>,
    pub auto_renew: bool,
    pub tags: BTreeMap<String, String>,
    pub client_request_token: Option<String>,
    /// The `clientRequestToken` of the last `UpdateEksAnywhereSubscription` this
    /// record took, so an SDK that RESENDS the same token is answered with the
    /// subscription that update produced rather than starting a second one.
    pub update_token: Option<String>,
}

impl EksAnywhereSubscriptionRecord {
    /// Materialise the status moves the clock already owes at `now`: the move out
    /// of `CREATING` once the licenses are provisioned, and the move to `EXPIRED`
    /// once the term has run out on a subscription that does not auto renew.
    ///
    /// Both were due at their stored instant whether or not anybody called, so
    /// applying them on read does not change what a later caller sees. A
    /// subscription that DOES auto renew is renewed at the same instant — "a
    /// boolean indicating whether or not a subscription will auto renew when it
    /// expires" (`EksAnywhereSubscription$autoRenew`) — so its term rolls
    /// forward instead of ending.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        // A subscription whose removal the clock owes is on its way out: nothing
        // else is owed on it, and the store drops it at that instant (see
        // [`ComAmazonawsEksAwswesleyfrontendState::materialize_due_transitions`]).
        if self.removed_at.is_some() {
            return;
        }
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = SUBSCRIPTION_STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
        if now < self.expiration_date {
            return;
        }
        let term = self.term();
        // A term of no length cannot be rolled forward — it would name the same
        // instant again — so such a subscription is reported as expired whatever its
        // auto-renew says. The term of every record this slice writes is one of the
        // published durations ([`SUBSCRIPTION_TERM_DURATIONS`]), so this is the
        // guard rather than the case.
        if !self.auto_renew || term.is_zero() {
            self.status = SUBSCRIPTION_STATUS_EXPIRED.to_string();
            return;
        }
        // The renewal the clock owes is computed rather than looped: however many
        // whole terms have passed since the last expiration, the next one is the
        // first instant after `now`. A clock so far past the representable range
        // that it cannot be placed leaves the date where it is rather than wrapping.
        let Ok(overdue) = now.duration_since(self.expiration_date) else {
            return;
        };
        let terms_passed = overdue.as_secs() / term.as_secs() + 1;
        let skipped = u32::try_from(terms_passed)
            .ok()
            .and_then(|terms| term.checked_mul(terms));
        if let Some(renewed) = skipped.and_then(|skipped| self.expiration_date.checked_add(skipped))
        {
            self.expiration_date = renewed;
        }
    }

    /// The duration this subscription's term covers.
    ///
    /// Zero for a term whose duration is not a positive count of months, which no
    /// record this slice writes carries: `CreateEksAnywhereSubscription` refuses
    /// every duration outside [`SUBSCRIPTION_TERM_DURATIONS`] before it writes one.
    pub(super) fn term(&self) -> Duration {
        let months = u32::try_from(self.term_duration.max(0)).unwrap_or(0);
        SUBSCRIPTION_MONTH
            .checked_mul(months)
            .unwrap_or(Duration::ZERO)
    }

    /// This record as the modelled `EksAnywhereSubscription` shape.
    pub fn to_subscription(&self) -> EksAnywhereSubscription {
        EksAnywhereSubscription {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            effective_date: Some(::aws_smithy_types::DateTime::from(self.effective_date)),
            expiration_date: Some(::aws_smithy_types::DateTime::from(self.expiration_date)),
            license_quantity: self.license_quantity,
            license_type: Some(self.license_type.clone()),
            term: Some(EksAnywhereSubscriptionTerm {
                duration: self.term_duration,
                unit: Some(self.term_unit.clone()),
            }),
            status: Some(self.status.clone()),
            auto_renew: self.auto_renew,
            // `licenseArns` names the Amazon Web Services License Manager ARN
            // the subscription's licenses live under and `licenses` carries "all
            // of the claims in the license token necessary to validate the
            // license for extended support". This emulator provisions nothing in
            // License Manager and signs no license token, so it reports neither
            // rather than naming a license nothing issued. Both are optional in
            // the model.
            license_arns: None,
            licenses: None,
            // `TagMap` is bounded 1..50, so a subscription with no tags reports
            // no map rather than an empty one.
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
        }
    }
}

/// The ARN of the subscription `id` in `region`: "The Amazon Resource Name (ARN)
/// for the subscription" (`EksAnywhereSubscription$arn`), in the same
/// `eks`-namespace form every other ARN this service mints takes.
pub fn eks_anywhere_subscription_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("eks", region, &format!("eksanywheresubscription/{id}"))
}

/// The sentence `DeleteEksAnywhereSubscription` refuses a subscription whose
/// status is not one it may remove with.
///
/// The rule is the operation's own: it "deletes an expired or inactive
/// subscription", and a live one "can only be cancelled within 7 days of
/// creation and [is] cancelled by creating a ticket in the Amazon Web Services
/// Support Center" (`DeleteEksAnywhereSubscription`) — so the status the
/// subscription is actually in is the only part of the sentence that varies.
pub fn subscription_not_deletable_message(status: &str) -> String {
    format!(
        "Subscription in {status} status cannot be deleted. Only an expired or \
         inactive subscription can be deleted."
    )
}

/// Why the subscription in `status` is not one `DeleteEksAnywhereSubscription`
/// may remove, or `None` when it is one.
///
/// Only a subscription whose term has run out is removable: every other status
/// is a subscription that is still live, which this API does not cancel.
pub fn subscription_delete_refusal(status: &str) -> Option<String> {
    (status != SUBSCRIPTION_STATUS_EXPIRED).then(|| subscription_not_deletable_message(status))
}

/// The sentence `UpdateEksAnywhereSubscription` refuses a subscription whose
/// status leaves nothing to renew with. See [`subscription_update_refusal`].
pub fn subscription_not_updatable_message(status: &str) -> String {
    format!("Subscription in {status} status cannot be updated.")
}

/// Why the subscription in `status` is not one `UpdateEksAnywhereSubscription`
/// may change, or `None` when it is one.
///
/// The operation changes whether the subscription "will auto renew when it
/// expires" (`EksAnywhereSubscription$autoRenew`), so a subscription whose term
/// has already run out has nothing left for it to act on, and one whose removal
/// has been accepted is on its way out of the account: both are refused as the
/// request's being invalid given the state rather than silently reviving the
/// subscription.
pub fn subscription_update_refusal(status: &str) -> Option<String> {
    matches!(
        status,
        SUBSCRIPTION_STATUS_EXPIRED | SUBSCRIPTION_STATUS_DELETING
    )
    .then(|| subscription_not_updatable_message(status))
}

/// Why `term` is not a term a subscription may be created for, or `None` when it
/// is one.
///
/// The rule is the one the model states for the member: "Valid values are MONTHS
/// for term unit and 12 or 36 for term duration"
/// (`EksAnywhereSubscriptionTerm`). The `unit` member targets an enum, so a
/// value outside it is already refused before a handler runs, and an ABSENT unit
/// is not: a term that names no unit names no length. The duration carries no
/// modelled range at all.
pub fn subscription_term_refusal(term: &EksAnywhereSubscriptionTerm) -> Option<String> {
    if !SUBSCRIPTION_TERM_DURATIONS.contains(&term.duration) {
        return Some(format!(
            "term duration not one of: {}",
            SUBSCRIPTION_TERM_DURATIONS
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    match term.unit.as_deref() {
        Some(unit) if unit == SUBSCRIPTION_TERM_UNIT_MONTHS => None,
        Some(unit) => Some(format!(
            "term unit not one of: {SUBSCRIPTION_TERM_UNIT_MONTHS}, got {}",
            elided_value(unit, MAX_ECHOED_VALUE)
        )),
        None => Some(format!(
            "term unit must be {SUBSCRIPTION_TERM_UNIT_MONTHS}"
        )),
    }
}
