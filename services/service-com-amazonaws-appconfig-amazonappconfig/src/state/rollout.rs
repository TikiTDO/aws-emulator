// SPDX-License-Identifier: CC0-1.0

//! Where ONE target of a gradual rollout stands, which is what decides whether a
//! deployment that is still rolling out is the deployment that target is served from.
//!
//! A deployment strategy's `GrowthFactor` is "the percentage of targets that received a
//! deployed configuration during each interval" and a deployment's `PercentageComplete`
//! is "the percentage of targets for which the deployment is available", so a rollout
//! that has reached part of its targets has reached exactly that part: the configuration
//! it deploys is in force for those targets and the previous one is still in force for
//! the rest. Which targets are in which group is decided per target and independently —
//! each target holds a fixed position in the range of percentage points a rollout
//! advances through, and is reached once the rollout has passed that position — so no
//! target is ever served a blend of two configurations, and a target already reached
//! stays reached as the rollout grows.
//!
//! The position is derived from the KEY that identifies the target, so one target keeps
//! its position across the calls it makes: for `GetConfiguration` that key is the
//! caller's own `ClientId` ("this ID enables AppConfig to deploy the configuration in
//! intervals, as defined in the deployment strategy"), and for the `appconfigdata` data
//! plane it is the configuration session, whose position is minted with the session and
//! carried by every poll of it.

use sha2::{Digest, Sha256};

use super::FULL_PERCENTAGE;

/// One target's position in a rollout: a percentage point in `[0, 100)`.
///
/// Compared against a deployment's `PercentageComplete`
/// ([`super::Deployment::serves_the_configuration_to`]): the rollout has reached this
/// target once it has passed this position, so position 0 is reached by any rollout that
/// has moved at all and no position is reached by one still at 0% of its targets.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct RolloutBucket(f32);

impl RolloutBucket {
    /// The position the target identified by `key` holds.
    ///
    /// Spread over the whole range by hashing the key, so positions are distributed
    /// evenly across the targets a service has and two targets' positions are
    /// independent of each other, and derived from NOTHING but the key, so the same
    /// target asking twice is in the same place in every rollout it is a target of.
    pub fn of_key(key: &str) -> Self {
        let digest = Sha256::digest(key.as_bytes());
        // The leading 8 bytes of the digest as a fraction of the range they span, which
        // spreads the keys uniformly over the percentage points. Folded over an iterator
        // rather than read out of a slice, so no length is assumed of the digest.
        let leading = digest
            .iter()
            .take(8)
            .fold(0u64, |bits, byte| (bits << 8) | u64::from(*byte));
        let fraction = leading as f64 / (u64::MAX as f64 + 1.0);
        Self::at_percentile(f64::from(FULL_PERCENTAGE) * fraction)
    }

    /// The position at `percentile` percentage points, held to the `[0, 100)` range a
    /// position is in: every target is reached by a rollout that completes, so no
    /// position is at or beyond the full percentage.
    pub fn at_percentile(percentile: f64) -> Self {
        let highest = FULL_PERCENTAGE - f32::EPSILON * FULL_PERCENTAGE;
        let percentile = if percentile.is_finite() {
            percentile as f32
        } else {
            0.0
        };
        Self(percentile.clamp(0.0, highest))
    }

    /// This position in percentage points.
    pub fn percentile(self) -> f32 {
        self.0
    }

    /// Whether a rollout that has reached `percentage_complete` percent of its targets
    /// has reached this one: it has once it is PAST this position, so a rollout at 0%
    /// has reached nobody and a completed one has reached everybody.
    pub fn is_reached_at(self, percentage_complete: f32) -> bool {
        self.0 < percentage_complete
    }
}
