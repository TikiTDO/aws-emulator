// SPDX-License-Identifier: CC0-1.0

//! The zonal autoshift status ARC holds for a managed resource, and the rule the
//! operation that writes one applies to it.
//!
//! The status is ARC's own record ABOUT a resource rather than part of the resource, so
//! it is kept the same way a practice run configuration is: one map keyed by
//! `resourceIdentifier` ([`ZonalShiftState::zonal_autoshift_statuses`]), which the
//! accessors that answer about a managed resource merge in for the length of a call. A
//! resource the service that owns it registered with ARC therefore reports the status a
//! caller set for it exactly as a resource this store was seeded with does, and the
//! resources no caller has set one for report `DISABLED`.
//!
//! The rule the model does NOT express as a constraint, so it is applied here: zonal
//! autoshift can only be `ENABLED` for a resource that has a practice run configuration
//! — "Practice runs must be configured for zonal autoshift to be enabled" (the
//! `DeletePracticeRunConfiguration` documentation in this service's Smithy model), and
//! "When you enable zonal autoshift, you must also configure practice runs for the
//! resource" (`CreatePracticeRunConfigurationResponse$zonalAutoshiftStatus`) — so asking
//! for `ENABLED` where ARC holds no configuration is a conflict in the resource's
//! current state, answered with the two sentences ARC states for it.

use super::{
    ComAmazonawsArczonalshiftPercdataplaneState as ZonalShiftState, ManagedResource,
    ZonalAutoshiftStatus,
};
use crate::ops::types::ConflictException;

/// The message ARC answers when zonal autoshift is asked to be `ENABLED` for a resource
/// that has no practice run configuration.
///
/// It states what the resource lacks and what therefore cannot be done, in ARC's own
/// words, and names no identifier, like the other refusals of this service: the conflict
/// is a property of the resource the request already named. It has no variable part —
/// the requirement is the same for every resource and every caller.
pub const PRACTICE_CONFIGURATION_REQUIRED_MESSAGE: &str =
    "Resource does not have a practice run configuration. Cannot enable zonal autoshift.";

/// The `ConflictExceptionReason` ARC names for that conflict: the practice run
/// configuration enabling zonal autoshift requires does not exist
/// (`smithy_query(query="shape", shape="ConflictExceptionReason")`).
pub const ZONAL_AUTOSHIFT_PRACTICE_CONFIGURATION_MISSING_REASON: &str =
    "PracticeConfigurationDoesNotExist";

/// The refusal a caller gets for asking ARC to hold `requested` for `resource`, or
/// `None` when the resource's current state admits it.
///
/// Built here, once, so every operation that writes a zonal autoshift status takes the
/// same branch: `ENABLED` requires a practice run configuration, and `DISABLED` — the
/// status that withdraws the authorization — is accepted whatever the resource holds.
#[must_use]
pub fn zonal_autoshift_conflict(
    resource: &ManagedResource,
    requested: ZonalAutoshiftStatus,
) -> Option<ConflictException> {
    (requested == ZonalAutoshiftStatus::Enabled
        && resource.practice_run_configuration.is_none())
    .then(|| ConflictException {
        message: PRACTICE_CONFIGURATION_REQUIRED_MESSAGE.to_string(),
        reason: ZONAL_AUTOSHIFT_PRACTICE_CONFIGURATION_MISSING_REASON.to_string(),
        // The conflict is about the resource's configuration, not about a shift, so
        // there is no `zonalShiftId` to associate it with.
        zonal_shift_id: None,
    })
}

impl ZonalShiftState {
    /// The zonal autoshift status ARC holds for a resource, or `None` when no caller
    /// has set one for it.
    #[must_use]
    pub fn zonal_autoshift_status(
        &self,
        resource_identifier: &str,
    ) -> Option<ZonalAutoshiftStatus> {
        self.zonal_autoshift_statuses
            .get(resource_identifier)
            .copied()
    }

    /// Hold `status` for a resource, replacing the status it had.
    pub fn set_zonal_autoshift_status(
        &mut self,
        resource_identifier: &str,
        status: ZonalAutoshiftStatus,
    ) {
        self.zonal_autoshift_statuses
            .insert(resource_identifier.to_string(), status);
    }
}
