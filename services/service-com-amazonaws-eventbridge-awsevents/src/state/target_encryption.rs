// SPDX-License-Identifier: CC0-1.0

//! The customer managed key a TARGET is encrypted with — an Amazon SQS queue or
//! an Amazon SNS topic — and whether this service may have a data key generated
//! under it, which is what handing one event to such a target needs.
//!
//! The rule the services follow: a queue or topic carrying `KmsMasterKeyId` has
//! every message written to it encrypted under that key, and the target's own
//! service performs that encryption by calling `kms:GenerateDataKey` for the
//! writer — so the KEY's own policy is a second document in force over a
//! delivery to such a target, beside the target's own access policy, and a key
//! policy that does not admit this service's principal is a write that does not
//! happen however freely the target's policy grants the delivery's action.
//!
//! The decision is KMS's OWN ([`kms_authorization::authorize_use`]) over the
//! key record the KMS service's own handlers wrote, so `PutKeyPolicy` moves it
//! with nothing else changed and the two services cannot disagree about what a
//! key policy says.

use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_com_amazonaws_kms_trentservice::state::authorization as kms_authorization;

/// The attribute Amazon SNS and Amazon SQS both report an encrypted resource's
/// key under: "the ID of an Amazon Web Services managed customer master key
/// (CMK) for Amazon SNS or a custom CMK" (`GetTopicAttributes`'s
/// `KmsMasterKeyId`), and "the ID of an Amazon Web Services managed customer
/// master key (CMK) for Amazon SQS or a custom CMK"
/// (`GetQueueAttributes`'s `KmsMasterKeyId`).
pub const KMS_MASTER_KEY_ATTRIBUTE: &str = "KmsMasterKeyId";

/// The KMS operation encrypting ONE delivered message performs: "GenerateDataKey
/// returns a unique symmetric data key for use outside of KMS", which is how a
/// service encrypts a payload under a customer managed key.
const GENERATE_DATA_KEY_OPERATION: &str = "GenerateDataKey";

/// The KMS store in the SAME shared engine, which is where the key a target is
/// encrypted with lives. `None` means the engine carries no such store — a
/// deployment with no KMS to ask, which is evidence of nothing.
pub fn kms_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<KmsState>> {
    kms_store::service_state(engine)
}

/// The KMS store, as its own service's state struct — the one `CreateKey` and
/// `PutKeyPolicy` write.
pub type KmsState = ::service_com_amazonaws_kms_trentservice::ComAmazonawsKmsTrentserviceState;

/// Whether the key `identifier` names withholds `kms:GenerateDataKey` from this
/// service, so a message EventBridge hands to the target `resource_arn` names
/// cannot be encrypted at all.
///
/// The use is one the target's own service makes on the writer's behalf rather
/// than one this account asked KMS for directly, so it is judged as arriving
/// THROUGH that service ([`kms_authorization::RequestedUse::through_service`]) —
/// `via_service` is that service's endpoint, which is what a key policy scoped by
/// `kms:ViaService` is written against. The principal it is judged FOR is this
/// service's own ([`super::SERVICE_PRINCIPAL`]), acting for the target, because
/// EventBridge delivers as itself and not as any identity of the account. Such a
/// principal holds no identity-based policies, so the KEY's own policy is the
/// only authority over the use: a key carrying only the policy `CreateKey`
/// generates for the account root admits it nothing until a statement names it.
///
/// `false` wherever nothing of the kind can be shown, so a delivery is never
/// withheld on a guess: an engine carrying no KMS store, an identifier naming no
/// key this Region holds, an Amazon Web Services managed key (whose policy AWS
/// manages for the service that owns it), and a key that cannot be used in a
/// cryptographic operation right now at all — that is a different failure from a
/// permission the key withholds, and this reports only the permission.
///
/// Call it with no store of this service locked: the KMS guard is taken here and
/// released before it answers.
pub fn generate_data_key_denied(
    kms: Option<&::engine::ServiceState<KmsState>>,
    region: &str,
    identifier: &str,
    resource_arn: &str,
    via_service: &str,
) -> bool {
    let Some(kms) = kms else {
        // Visible to an operator: a lookup nothing answered lets the delivery
        // through, so without this line it would be indistinguishable from a key
        // whose policy KMS said admits this service.
        ::tracing::warn!(
            "no KMS service is mounted beside EventBridge; an encrypted target's key is accepted"
        );
        return false;
    };
    let caller =
        kms_authorization::Caller::service_principal(super::SERVICE_PRINCIPAL, resource_arn);
    let store = kms.lock();
    let kms_store::KeyRef::Found(key) = store.resolve(region, identifier) else {
        return false;
    };
    if key.key_manager != kms_store::KEY_MANAGER_CUSTOMER || !key.is_usable() {
        return false;
    }
    kms_authorization::authorize_use(
        &caller,
        &store,
        region,
        key,
        kms_authorization::RequestedUse::through_service(GENERATE_DATA_KEY_OPERATION, via_service),
        // No encryption context is presented, so a grant constrained to one does
        // not permit this use: the authority it carries is not claimed rather
        // than assumed.
        None,
        None,
    )
    .is_err()
}

/// The endpoint of the service `vendor` names in `region`, as a key policy's
/// `kms:ViaService` key spells it: `<vendor>.<region>.amazonaws.com`.
///
/// One spelling for every target kind, so the two deliveries this module decides
/// — a queue's and a topic's — present the via-service the same way.
pub fn via_service(vendor: &str, region: &str) -> String {
    format!("{vendor}.{region}.amazonaws.com")
}
