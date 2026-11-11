// SPDX-License-Identifier: CC0-1.0

//! The TRUST-STORE half of this service's store: what a trust store is while it is held,
//! what makes one unavailable to delete, which resources are associated with one, and the
//! wording every trust-store refusal shares.
//!
//! A trust store is the set of CA certificates an HTTPS listener authenticates CLIENT
//! certificates against (`MutualAuthenticationAttributes$TrustStoreArn`), so two facts about
//! one are derived from the listeners this same store holds rather than counted on the side:
//! which resources it is ASSOCIATED with ([`ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State::trust_store_associations`])
//! and therefore whether it is IN USE. The record itself lives beside every other record of
//! this service (`crate::state::TrustStoreRecord`), so a create, a describe, a tag write and
//! a delete all read and write the one copy.

use ::std::time::SystemTime;

use super::{
    ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as State, ListenerRecord,
};

/// How many trust stores this store holds before `CreateTrustStore` refuses with the
/// modelled `TooManyTrustStoresException`.
///
/// AWS publishes an adjustable per-account, per-Region quota, and this emulator models
/// neither Service Quotas nor an account's adjustments — so this is the emulator's OWN bound
/// on the store, exactly as [`super::LOAD_BALANCER_QUOTA`] is: far above any workflow an
/// emulator serves, and what stops the store growing without end, every record it holds
/// being released by `DeleteTrustStore`.
pub const TRUST_STORE_QUOTA: usize = 1_000;

/// The message Elastic Load Balancing answers a `TrustStoreNotFound` with for a call that
/// addresses a trust store by its ARN — the `TrustStoreArn` member every single-trust-store
/// operation takes, and the `TrustStoreArns` list a describe narrows by.
///
/// Built from the request: the ARN it named is the only thing the caller can act on, and the
/// refusal names the KIND of identifier it was given, which is what tells it apart from the
/// same refusal about a name ([`trust_store_name_not_found_message`]).
pub fn trust_store_arn_not_found_message(arn: &str) -> String {
    format!("Trust store ARN '{arn}' not found")
}

/// The message Elastic Load Balancing answers a `TrustStoreNotFound` with for a read that
/// addresses a trust store by NAME (`DescribeTrustStores$Names`), worded about the name the
/// request carried for the same reason the ARN form is worded about the ARN.
pub fn trust_store_name_not_found_message(name: &str) -> String {
    format!("Trust store name '{name}' not found")
}

/// The message a create whose `Name` a trust store of this account already holds is refused
/// with, under the modelled `DuplicateTrustStoreName`.
///
/// `CreateTrustStore$Name` is documented "This name must be unique per region and can't be
/// changed after creation", so the refusal names the NAME from the request: that is the one
/// value the caller changes to get a trust store.
pub fn duplicate_trust_store_name_message(name: &str) -> String {
    format!("A trust store with the name '{name}' already exists")
}

/// The message a delete of a trust store some resource is still associated with is refused
/// with, under the modelled `TrustStoreInUse`.
///
/// Built from the request and the store: the trust store the caller named, and the rule it
/// broke — what has to change before the delete can be served is the association, so the
/// refusal says that an association exists.
pub fn trust_store_in_use_message(arn: &str) -> String {
    format!("Trust store '{arn}' is associated with one or more resources")
}

/// The message a `DeleteSharedTrustStoreAssociation` naming a resource of the trust
/// store's OWN account is refused with, under the modelled `DeleteAssociationSameAccount`
/// ("The specified association can't be within the same account").
///
/// The operation deletes the association of a trust store SHARED with another account, so
/// an association inside one account is not one it can delete. The refusal names BOTH ARNs
/// the request carried — the trust store first — because what the caller has to change is
/// which of the two pair it addressed.
pub fn delete_association_same_account_message(
    trust_store_arn: &str,
    resource_arn: &str,
) -> String {
    format!(
        "Cannot remove the association because the trust store ARN '{trust_store_arn}' and the \
         resource ARN '{resource_arn}' are in the same account"
    )
}

/// The message a `DeleteSharedTrustStoreAssociation` naming a resource that is not
/// associated with the trust store is refused with, under the modelled
/// `AssociationNotFound` ("The specified association does not exist"): the pair the request
/// named, which is what the caller fixes.
pub fn trust_store_association_not_found_message(
    trust_store_arn: &str,
    resource_arn: &str,
) -> String {
    format!("Resource '{resource_arn}' is not associated with trust store '{trust_store_arn}'")
}

/// The message a create past [`TRUST_STORE_QUOTA`] is refused with, under the modelled
/// `TooManyTrustStores`: the bound this emulator holds, stated the way the sibling quota
/// refusals of this service state theirs.
pub fn too_many_trust_stores_message() -> String {
    format!("This emulator holds at most {TRUST_STORE_QUOTA} trust stores")
}

impl State {
    /// The resources associated with the trust store `arn` names, in the order a paginated
    /// read walks them (by resource ARN, which is stable whatever the store's iteration
    /// order).
    ///
    /// A trust store is associated with a resource by being the trust store that resource
    /// authenticates client certificates against, which in this model is an HTTPS
    /// LISTENER's mutual authentication configuration
    /// (`MutualAuthenticationAttributes$TrustStoreArn`, set by `CreateListener` and
    /// `ModifyListener`). So the associations are derived from the listeners this store
    /// holds — a listener that moves off that trust store, and one that goes away with its
    /// load balancer, is no longer associated with it — rather than from a list kept beside
    /// them that nothing would retract.
    pub fn trust_store_associations(&self, arn: &str) -> Vec<String> {
        let mut associated: Vec<String> = self
            .listeners
            .values()
            .filter(|listener| listener_authenticates_against(listener, arn))
            .map(|listener| listener.arn.clone())
            .collect();
        associated.sort();
        associated
    }

    /// Remove the trust store `arn` names, with the name it held, answering whether it was
    /// there.
    ///
    /// Nothing cascades: a trust store that any resource is still associated with is not
    /// deleted at all (the modelled `TrustStoreInUse`, which the operation raises before
    /// reaching here), so no listener is ever left pointing at a trust store this removed.
    pub fn remove_trust_store(&mut self, arn: &str) -> bool {
        let Some(removed) = self.trust_stores.remove(arn) else {
            return false;
        };
        self.name_to_trust_store_arn.remove(&removed.name);
        true
    }

    /// Mint the ARN of a new trust store named `name` in `region`, with a resource id no
    /// resource this store holds carries.
    ///
    /// The form is the one every trust store ARN of this service carries —
    /// `truststore/<name>/<id>` under this service's own namespace — so the ARN a create
    /// mints is the ARN a describe, a tag write and a delete resolve.
    pub fn mint_trust_store_arn(&mut self, name: &str, region: &str) -> String {
        let id = self.mint_id();
        ::emulator_server::emulator_arn(
            super::SERVICE_NAMESPACE,
            region,
            &format!("truststore/{name}/{id}"),
        )
    }
}

/// Whether `listener` authenticates client certificates against the trust store `arn`
/// names.
///
/// Read off what the listener is EFFECTIVELY configured with
/// ([`ListenerRecord::effective_mutual_authentication`]), which is what a read of that
/// listener reports and what its traffic is served under, so the association and the
/// listener's own reported configuration cannot disagree. A listener whose protocol carries
/// no mutual authentication at all has none.
fn listener_authenticates_against(listener: &ListenerRecord, arn: &str) -> bool {
    listener
        .effective_mutual_authentication()
        .and_then(|attributes| attributes.trust_store_arn)
        .is_some_and(|trust_store_arn| trust_store_arn == arn)
}

/// When the clock owes a trust store created at `now` its move to
/// [`super::TRUST_STORE_ACTIVE`]: the settling span every clock-owed transition of this
/// service is due after ([`super::SETTLE_AFTER`]), so a caller acting straight after the
/// create still observes the `CREATING` it was answered with.
pub fn trust_store_active_at(now: SystemTime) -> SystemTime {
    now + super::SETTLE_AFTER
}
