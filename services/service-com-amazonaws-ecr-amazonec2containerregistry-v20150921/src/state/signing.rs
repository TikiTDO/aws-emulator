// SPDX-License-Identifier: CC0-1.0

//! The signing jobs a push starts under the registry's signing configuration, and the
//! status each one reports.
//!
//! A signing rule is a standing instruction: "When an image is pushed to a matching
//! repository, a signing job is created using the specified profile" (`SigningRule`'s own
//! documentation). So the jobs live on the IMAGE, written by the push that started them —
//! one per signing profile the configuration pointed at that repository at that instant —
//! exactly as the copies a push starts under the replication configuration do
//! ([`super::ImageReplication`]). A rule stored after a push therefore signs nothing the
//! registry already holds, deleting the configuration withdraws no signature already made,
//! and an image pushed while no rule matched its repository carries no job at all, which is
//! the empty `signingStatuses` list `DescribeImageSigningStatus` answers with.
//!
//! Whether the job SUCCEEDS is decided by the signing profile the rule names, which is AWS
//! Signer's resource, not this registry's: the profile is resolved through Signer's own
//! store ([`signer_store`]) at the push, so a job names a profile this emulator really
//! holds and can really sign with, or fails saying which. The resolution is made with no
//! lock of this service's store held, the way this crate's authorization gate reads the IAM
//! and STS stores.
//!
//! The status each job reports follows from the instant the clock owes it its terminal
//! value ([`IMAGE_SIGNING_DELAY`]), so every reader derives the same answer and no read has
//! to write.

use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_signer_wallabyservice::state as signer_store;

use crate::ops::types::{ImageSigningStatus, SigningRule};

use super::{wildcard_matches, SigningConfiguration};

/// The `SigningStatus` of a signing job that has not finished yet.
pub const SIGNING_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `SigningStatus` of a job whose signature was generated: "COMPLETE - The signature
/// was successfully generated" (`ImageSigningStatus$status`).
pub const SIGNING_STATUS_COMPLETE: &str = "COMPLETE";

/// The `SigningStatus` of a job that could not sign the image: "FAILED - Signing failed.
/// See failureCode and failureReason for details" (`ImageSigningStatus$status`).
pub const SIGNING_STATUS_FAILED: &str = "FAILED";

/// The only value `SigningRepositoryFilterType` carries: "Currently, only `WILDCARD_MATCH`
/// is supported, which uses wildcard patterns to match repository names"
/// (`SigningRepositoryFilter$filterType`).
pub const WILDCARD_MATCH_FILTER_TYPE: &str = "WILDCARD_MATCH";

/// The `failureCode` of a job whose rule names a signing profile AWS Signer does not hold.
pub const FAILURE_CODE_PROFILE_NOT_FOUND: &str = "SIGNING_PROFILE_NOT_FOUND";

/// The `failureCode` of a job whose signing profile exists but can no longer sign: a
/// canceled profile "cannot perform new signing jobs" (`signer:CancelSigningProfile`), and
/// neither can a revoked one.
pub const FAILURE_CODE_PROFILE_NOT_ACTIVE: &str = "SIGNING_PROFILE_NOT_ACTIVE";

/// How long after the push that creates it the clock owes a signing job its
/// `IN_PROGRESS` -> terminal transition.
///
/// AWS publishes no duration for signing an image on push, so this is a CHOSEN span: short
/// enough that no workflow waits on it, and never zero, so a caller reading the status
/// straight after the push still meets `IN_PROGRESS`. It is stored as a due instant and
/// never slept for: the terminal status is owed whether or not anybody calls again.
pub const IMAGE_SIGNING_DELAY: Duration = Duration::from_secs(3);

/// Why a signing job could not sign the image: the `failureCode` and the `failureReason`
/// its status carries, "only present if `status` is `FAILED`"
/// (`ImageSigningStatus$failureCode`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SigningFailure {
    pub code: String,
    pub reason: String,
}

/// One signing job a push created for one stored image, under ONE signing profile.
///
/// The job's outcome is decided when it is created — a signing job signs with the profile
/// the rule named, as that profile stood at the push — and the status it reports follows
/// from the instant the clock owes it that outcome, so every read derives the same answer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageSigning {
    /// The ARN of the AWS Signer signing profile this job signs with: the one the matched
    /// rule named.
    pub signing_profile_arn: String,
    /// When the push that created this job was made.
    pub started_at: SystemTime,
    /// When the clock owes this job its terminal status: `started_at` +
    /// [`IMAGE_SIGNING_DELAY`].
    pub settled_due_at: SystemTime,
    /// Why this job will fail, or `None` for one that signs the image.
    pub failure: Option<SigningFailure>,
}

impl ImageSigning {
    /// The `ImageSigningStatus` `DescribeImageSigningStatus` reports for this job.
    ///
    /// `failureCode` and `failureReason` are reported only with `FAILED`, which is what
    /// their own documentation states; a job the clock has not settled yet is
    /// `IN_PROGRESS` and carries neither, because nothing has failed.
    pub fn to_status(&self, now: SystemTime) -> ImageSigningStatus {
        let settled = self.settled_due_at <= now;
        let failure = self.failure.as_ref().filter(|_| settled);
        let status = match (settled, self.failure.is_some()) {
            (false, _) => SIGNING_STATUS_IN_PROGRESS,
            (true, false) => SIGNING_STATUS_COMPLETE,
            (true, true) => SIGNING_STATUS_FAILED,
        };
        ImageSigningStatus {
            signing_profile_arn: Some(self.signing_profile_arn.clone()),
            status: Some(status.to_string()),
            failure_code: failure.map(|failure| failure.code.clone()),
            failure_reason: failure.map(|failure| failure.reason.clone()),
        }
    }
}

/// Whether `rule` covers the repository named `repository_name`, so that a push into it
/// creates a signing job under the rule's profile.
///
/// A rule carrying no `repositoryFilters` covers every repository of the registry ("If no
/// filters are specified, all images pushed to the registry are signed",
/// `SigningRule$repositoryFilters`), and a `WILDCARD_MATCH` filter covers the repositories
/// its pattern matches, where "the `*` character matches any sequence of characters"
/// (`SigningRepositoryFilter$filter`) — the one wildcard matcher this crate has
/// ([`wildcard_matches`]). The filters of one rule are alternatives: a repository one of
/// them names is signed by that rule. A `filterType` the model widens the enum with later
/// covers nothing until it is read here, so a filter this code does not understand never
/// silently signs a repository.
pub fn signing_rule_covers(rule: &SigningRule, repository_name: &str) -> bool {
    match rule.repository_filters.as_deref() {
        None | Some([]) => true,
        Some(filters) => filters.iter().any(|filter| {
            filter.filter_type == WILDCARD_MATCH_FILTER_TYPE
                && wildcard_matches(&filter.filter, repository_name)
        }),
    }
}

/// The signing profiles a push into `repository_name` is signed under: the profile of every
/// rule of `configuration` whose repository filters cover that repository, in the order the
/// configuration lists them and with each profile named once.
///
/// "If the image matched signing rules that reference different signing profiles, a status
/// is returned for each profile" (`DescribeImageSigningStatus`): one job per PROFILE, so two
/// rules naming one profile are one signing of the image. The set is bounded by the model's
/// own `SigningRuleList` `@length(min: 0, max: 10)`, which the request boundary enforces
/// before a configuration is ever stored.
pub fn matched_signing_profiles(
    configuration: Option<&SigningConfiguration>,
    repository_name: &str,
) -> Vec<String> {
    let mut matched: Vec<String> = Vec::new();
    let rules = configuration
        .map(|configuration| configuration.rules.as_slice())
        .unwrap_or_default();
    for rule in rules {
        if !signing_rule_covers(rule, repository_name) {
            continue;
        }
        if !matched.contains(&rule.signing_profile_arn) {
            matched.push(rule.signing_profile_arn.clone());
        }
    }
    matched
}

/// The signing jobs a push into `repository_name` at `now` creates: one per signing profile
/// the registry's stored signing configuration points at that repository, each already
/// carrying the outcome the profile it names gives it.
///
/// Called by `PutImage` with NO lock of this service's store held: the registry's own
/// configuration is read and released here, and the profiles are then resolved against AWS
/// Signer's store, so this crate never holds two services' locks at once.
pub fn planned_signings<Eng: Send + Sync + 'static>(
    engine: &Eng,
    repository_name: &str,
    now: SystemTime,
) -> Vec<ImageSigning> {
    let Some(state) = super::service_state(engine) else {
        return Vec::new();
    };
    let matched = {
        let store = state.lock();
        matched_signing_profiles(store.signing_configuration.as_ref(), repository_name)
    };
    matched
        .into_iter()
        .map(|signing_profile_arn| ImageSigning {
            failure: signing_failure(engine, &signing_profile_arn),
            signing_profile_arn,
            started_at: now,
            settled_due_at: now + IMAGE_SIGNING_DELAY,
        })
        .collect()
}

/// Why a job signing with the profile `signing_profile_arn` names fails, or `None` when
/// that profile can sign the image.
///
/// The judgement is AWS Signer's, read from Signer's own store: the ARN has to name a
/// profile that store really holds — the whole ARN, so another account's or another
/// partition's profile of the same name resolves to nothing — and that profile has to be one
/// that can still start signing jobs ([`signer_store::SigningProfileRecord::can_sign`]).
fn signing_failure<Eng: Send + Sync + 'static>(
    engine: &Eng,
    signing_profile_arn: &str,
) -> Option<SigningFailure> {
    let not_found = || SigningFailure {
        code: FAILURE_CODE_PROFILE_NOT_FOUND.to_string(),
        reason: format!("The signing profile '{signing_profile_arn}' does not exist."),
    };
    let Some(signer) = signer_store::service_state(engine) else {
        return Some(not_found());
    };
    let Some((region, profile_name)) =
        signer_store::profile_region_and_name_in_arn(signing_profile_arn)
    else {
        return Some(not_found());
    };
    let store = signer.lock();
    let Some(profile) = store
        .profile(region, profile_name)
        .filter(|profile| profile.arn == signing_profile_arn)
    else {
        return Some(not_found());
    };
    if profile.can_sign() {
        return None;
    }
    Some(SigningFailure {
        code: FAILURE_CODE_PROFILE_NOT_ACTIVE.to_string(),
        reason: format!(
            "The signing profile '{signing_profile_arn}' has status '{}' and cannot sign \
             new images.",
            profile.status
        ),
    })
}
