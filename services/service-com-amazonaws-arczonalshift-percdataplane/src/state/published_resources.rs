// SPDX-License-Identifier: CC0-1.0

//! The managed resources the services that OWN them have registered with ARC.
//!
//! ARC models no operation that creates a managed resource — "Resources are automatically
//! registered with ARC by Amazon Web Services services" (the `StartZonalShift`
//! documentation in this service's Smithy model) — so a resource becomes managed through
//! the service that owns it: zonal-shift integration is turned on for a load balancer
//! through Elastic Load Balancing, for an Auto Scaling group through Amazon EC2 Auto
//! Scaling. Those services cannot see this store and this service cannot see theirs, so the
//! registrations live in the ONE shared registry
//! ([`::service_common::arc_managed_resources::ManagedResourceRegistry`]), which the engine
//! keys by TYPE.
//!
//! READ, NEVER COPIED. An operation that answers about a managed resource reads the
//! registry ([`published_resources`]) and merges it with this store's own resources for the
//! length of that call ([`ZonalShiftState::managed_resource_inventory`]). Nothing is
//! written into this store on the way: the registry stays the authority on what it
//! published, so a registration its owner has withdrawn cannot be reported by a later read,
//! and no read changes what the next caller sees.
//!
//! What a registration carries is what ARC reports about a resource it did not create: the
//! ARN a caller names it by, its name, and the Availability Zones it is deployed in — the
//! key set of `appliedWeights`. Everything else ARC answers for one is ARC's own and is
//! computed from this store: the zonal shifts in progress and the weights they move.

use std::collections::BTreeMap;

use ::service_common::arc_managed_resources::{ManagedResourceRegistry, RegisteredResource};

use super::{
    ComAmazonawsArczonalshiftPercdataplaneState as ZonalShiftState, ManagedResource,
    ZonalAutoshiftStatus,
};

/// The SHARED managed-resource registry in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// Answer `None` by reading no registration: an engine carrying no registry holds none,
/// which is exactly what an emulator no other service has registered anything with
/// reports.
pub fn managed_resource_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ManagedResourceRegistry>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ManagedResourceRegistry>)
}

/// The resources the owning services have registered with ARC right now, in ARN order.
///
/// Every operation that answers about a managed resource takes this ONCE, before it locks
/// this service's own store, and hands it to
/// [`ZonalShiftState::managed_resource_inventory`] or
/// [`ZonalShiftState::authorized_resource`] — so none of them can disagree about which
/// resources ARC manages, and the registry is never held while this store is.
#[must_use]
pub fn published_resources<Eng: Send + Sync + 'static>(engine: &Eng) -> Vec<RegisteredResource> {
    managed_resource_registry(engine)
        .map(|registry| registry.lock().resources())
        .unwrap_or_default()
}

impl ZonalShiftState {
    /// Every managed resource ARC holds, keyed by `resourceIdentifier` in ARN order: the
    /// ones this store was seeded with and the ones `published` carries.
    ///
    /// Merged for the length of one call rather than taken into the store, so a
    /// registration the owner has withdrawn is simply not in the next answer. A resource
    /// this store holds ITSELF is the one reported, since this store is the authority on
    /// its own resources; a published one carries the facts its owner recorded and the
    /// defaults ARC reports for a resource nothing has been configured for.
    ///
    /// The practice run configuration is ARC's own record about a resource, so every
    /// resource in the answer carries the one this store holds for it, whichever way the
    /// resource reached ARC.
    #[must_use]
    pub fn managed_resource_inventory(
        &self,
        published: &[RegisteredResource],
    ) -> BTreeMap<String, ManagedResource> {
        let mut inventory: BTreeMap<String, ManagedResource> = published
            .iter()
            .map(|resource| {
                (
                    resource.arn.clone(),
                    ManagedResource {
                        arn: resource.arn.clone(),
                        name: resource.name.clone(),
                        availability_zones: resource.availability_zones.clone(),
                        autoshifts: Vec::new(),
                        practice_run_configuration: None,
                        zonal_autoshift_status: ZonalAutoshiftStatus::Disabled,
                    },
                )
            })
            .collect();
        for (arn, resource) in &self.managed_resources {
            inventory.insert(arn.clone(), resource.clone());
        }
        for (arn, resource) in &mut inventory {
            resource.practice_run_configuration = self.practice_run_configuration(arn);
        }
        inventory
    }

    /// The managed resource `resource_identifier` names, or `None` for an identifier no
    /// managed resource ARC holds carries.
    ///
    /// The identifier is the ARN of the resource and nothing more
    /// (`ResourceIdentifier`'s documentation: "The identifier is the Amazon Resource Name
    /// (ARN) for the resource"), so a resource is matched by that ARN as the owning
    /// service recorded it. The resource carries the practice run configuration this
    /// store holds for it, which is ARC's own record about it.
    #[must_use]
    pub fn managed_resource(
        &self,
        published: &[RegisteredResource],
        resource_identifier: &str,
    ) -> Option<ManagedResource> {
        let mut resource = self
            .managed_resources
            .get(resource_identifier)
            .cloned()
            .or_else(|| {
                published
                    .iter()
                    .find(|resource| resource.arn == resource_identifier)
                    .map(|resource| ManagedResource {
                        arn: resource.arn.clone(),
                        name: resource.name.clone(),
                        availability_zones: resource.availability_zones.clone(),
                        autoshifts: Vec::new(),
                        practice_run_configuration: None,
                        zonal_autoshift_status: ZonalAutoshiftStatus::Disabled,
                    })
            })?;
        resource.practice_run_configuration = self.practice_run_configuration(resource_identifier);
        Some(resource)
    }
}
