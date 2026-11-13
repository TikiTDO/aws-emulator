// SPDX-License-Identifier: CC0-1.0

//! # IAM instance profile associations
//!
//! [`ComAmazonawsEc2Amazonec2State::iam_instance_profile_associations`] holds the
//! associations between the account's instances and the IAM instance profiles they run
//! as, keyed by `IamInstanceProfileAssociationId`, as the modelled
//! [`IamInstanceProfileAssociation`] shape itself — the one every operation that reports
//! an association answers with — so a member a writer sets round-trips through a report
//! without a field-by-field copy that can silently drop one.
//!
//! "You cannot associate more than one IAM instance profile with an instance"
//! (`AssociateIamInstanceProfile`), so at most one association of this store names any
//! one instance.
//! [`ComAmazonawsEc2Amazonec2State::iam_instance_profile_association_by_instance`] is the
//! index of that rule: it answers which association an instance holds without a scan, and
//! it is what [`ComAmazonawsEc2Amazonec2State::instance_view`] reads to report the profile
//! on the instance itself, so the instance report and the association report cannot
//! disagree about one instance's profile. Both maps are written ONLY by the two seams
//! below, which is what keeps them in step.
//!
//! An association moves through the states `IamInstanceProfileAssociationState` publishes
//! (`smithy_query(query="shape",shape="IamInstanceProfileAssociationState")`), and both
//! moves are owed by the CLOCK rather than by a later call, on the same
//! [`TRANSITION_DELAY`] as every other transient state of this store:
//!
//! * a new association is recorded [`IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING`]
//!   and reaches [`IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED`] one span later, so a
//!   caller that reads straight after the write still sees the transient state;
//! * a disassociation records
//!   [`IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING`] and the association itself
//!   is GONE one span later — the state EC2's own association filters stop at
//!   (`DescribeIamInstanceProfileAssociationsRequest$Filters` documents `state` as
//!   "associating | associated | disassociating"), so a fully disassociated profile is
//!   reported as no association rather than as a terminal row.
//!
//! Replacing the profile an instance runs as is neither of those moves: the association
//! named is swapped for a new one over the same instance
//! ([`ComAmazonawsEc2Amazonec2State::replace_iam_instance_profile_association`]), so the old
//! association is gone at once and the new one starts its own `associating` span.
//!
//! The PROFILE an association reports is read from the account's IAM records
//! ([`resolve_iam_instance_profile`]): both members [`IamInstanceProfile`] carries — the ARN
//! and the unique id IAM minted for the profile — belong to the service that owns instance
//! profiles, so neither is derived from the reference the request named.
//!
//! An association id this store resolves to nothing is refused through ONE seam
//! ([`ComAmazonawsEc2Amazonec2State::unresolvable_iam_instance_profile_association_id`]), as
//! the malformed/not-found pair every id kind of this crate is read by.
//!
//! [`ComAmazonawsEc2Amazonec2State::iam_instance_profile_association_due_at`] is when that
//! move falls due, on the same monotonic clock as
//! [`ComAmazonawsEc2Amazonec2State::vpc_available_at`], and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it.
//!
//! A child module of [`crate::state`] rather than more lines in it: everything here belongs
//! to one resource and reaches the rest of the store through `use super::*`.

use std::time::{Instant, SystemTime};

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

use crate::ops::types::{
    IamInstanceProfile, IamInstanceProfileAssociation, IamInstanceProfileSpecification,
};

use super::*;

/// The id prefix an IAM instance profile association is minted with.
///
/// The model publishes no pattern for `IamInstanceProfileAssociationId` (it is a plain
/// `String` — `smithy_query(query="shape",shape="IamInstanceProfileAssociationId")`), so the
/// id takes EC2's own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`], with the `iip-assoc` prefix EC2 gives one. It
/// is minted by the store and never taken from a request.
pub const IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX: &str = "iip-assoc";

/// The state an association is recorded in: the `IamInstanceProfileAssociationState` member
/// `associating` (`crate::ops::types::IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_VALUES`), the
/// non-terminal state it sits in until [`TRANSITION_DELAY`] has passed.
pub const IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING: &str = "associating";

/// The state the clock owes a recorded association: that enum's `associated` member.
pub const IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// The state a disassociation records: that enum's `disassociating` member, the
/// non-terminal state the association sits in until the clock removes it.
pub const IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING: &str = "disassociating";

/// The largest number of IAM instance profile associations this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_NETWORK_INTERFACE_PERMISSIONS`], not an AWS
/// quota: each association is a record in this process's memory, so the number a caller can
/// create has to be finite. At most one association names any one instance, so this bounds
/// the instances that may hold a profile as well.
/// [`ComAmazonawsEc2Amazonec2State::has_iam_instance_profile_association_capacity`] is the
/// predicate a writer asks before inserting.
pub const MAX_IAM_INSTANCE_PROFILE_ASSOCIATIONS: usize = 1_000;

/// EC2's code for a request whose state does not admit the action, which an instance that
/// already runs as a profile answers an attempt to give it a second one with.
pub const INCORRECT_STATE: &str = "IncorrectState";

/// The caller-chosen bytes one stored association retains: the instance profile reference
/// the request named, which is the only part of the record a caller sizes (the id, the
/// instance id, the state word and the timestamp are all this service's own and
/// fixed-size).
///
/// The ONE definition of that charge, the way [`network_interface_permission_payload_bytes`]
/// is for a permission: a writer charges exactly it, and the removal path gives exactly it
/// back.
pub fn iam_instance_profile_association_payload_bytes(profile: &IamInstanceProfile) -> usize {
    profile
        .arn
        .as_ref()
        .map_or(0, String::len)
        .saturating_add(profile.id.as_ref().map_or(0, String::len))
}

/// What replacing the profile one association is over did: the outcomes a caller has to
/// tell apart, so a replacement that found nothing to act on is never reported as one the
/// store had no room for.
#[derive(Debug, Clone)]
pub enum IamInstanceProfileAssociationReplacement {
    /// The swap was recorded: the association named is gone and this is the one that took
    /// its place, over the same instance and with the new profile.
    Replaced(IamInstanceProfileAssociation),
    /// The store holds no association under the id named, so nothing was replaced.
    NoSuchAssociation,
    /// The store has no room for the replacement, so the association named still stands.
    NoRoom,
}

/// What the store needs to record one association, so the id, the state and the byte charge
/// are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct IamInstanceProfileAssociationSpec {
    /// "The ID of the instance" (`AssociateIamInstanceProfileRequest$InstanceId`), already
    /// resolved against the instance store by the caller.
    pub instance_id: String,
    /// The IAM instance profile the instance is to run as, as the association reports it
    /// (`IamInstanceProfileAssociation$IamInstanceProfile`).
    pub iam_instance_profile: IamInstanceProfile,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another association fits inside the store's ceiling
    /// ([`MAX_IAM_INSTANCE_PROFILE_ASSOCIATIONS`]).
    ///
    /// The same contract as [`Self::has_network_interface_permission_capacity`]: a writer
    /// asks this first and refuses when it answers `false`.
    pub fn has_iam_instance_profile_association_capacity(&self) -> bool {
        self.iam_instance_profile_associations.len() < MAX_IAM_INSTANCE_PROFILE_ASSOCIATIONS
    }

    /// The association the instance `instance_id` holds, whatever state it is in, or `None`
    /// for an instance that runs as no instance profile.
    ///
    /// A READ ONLY, answered through the by-instance index rather than by scanning the
    /// store: what a writer asks to find the association an instance already has, and what
    /// [`Self::iam_instance_profile_of_instance`] reads.
    pub fn iam_instance_profile_association_over(
        &self,
        instance_id: &str,
    ) -> Option<&IamInstanceProfileAssociation> {
        let association_id = self
            .iam_instance_profile_association_by_instance
            .get(instance_id)?;
        self.iam_instance_profile_associations.get(association_id)
    }

    /// The instance profile `instance_id` RUNS AS, as `Instance$IamInstanceProfile` reports
    /// it, or `None` for an instance that runs as none.
    ///
    /// An association the clock has not finished taking away
    /// ([`IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING`]) no longer gives the
    /// instance a profile — the disassociation has been performed, and what the clock still
    /// owes is only the removal of the association record — so the instance reports none.
    pub fn iam_instance_profile_of_instance(&self, instance_id: &str) -> Option<IamInstanceProfile> {
        self.iam_instance_profile_association_over(instance_id)
            .filter(|association| {
                association.state.as_deref()
                    != Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING)
            })
            .and_then(|association| association.iam_instance_profile.clone())
    }

    /// Record an association between an instance and an IAM instance profile and answer it
    /// as every report of it will read, or `None` when the store has no room for it.
    ///
    /// The ONE writer seam: everything the service owns is derived here rather than taken
    /// from a caller — the id, the `associating` state the clock then moves to `associated`,
    /// the timestamp and the by-instance index — so two writers cannot report one
    /// association differently. The caller-chosen bytes the record retains
    /// ([`iam_instance_profile_association_payload_bytes`]) are charged against the store's
    /// byte budget here, and both ceilings are enforced here rather than trusted to the
    /// caller.
    pub fn insert_iam_instance_profile_association(
        &mut self,
        spec: IamInstanceProfileAssociationSpec,
    ) -> Option<IamInstanceProfileAssociation> {
        if !self.has_iam_instance_profile_association_capacity()
            || self.iam_instance_profile_associations.len() >= MAX_STORED_RECORDS_PER_STORE
        {
            return None;
        }
        let payload_bytes =
            iam_instance_profile_association_payload_bytes(&spec.iam_instance_profile);
        if self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        let association_id = self.next_id(IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX);
        let association = IamInstanceProfileAssociation {
            association_id: Some(association_id.clone()),
            instance_id: Some(spec.instance_id.clone()),
            iam_instance_profile: Some(spec.iam_instance_profile),
            state: Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING.to_string()),
            // "The time the IAM instance profile was associated with the instance"
            // (`IamInstanceProfileAssociation$Timestamp`): the instant this record was
            // made, which no later move of the association changes.
            timestamp: Some(system_time_to_date_time(SystemTime::now())),
        };
        self.add_stored_bytes(payload_bytes.saturating_add(STORED_RECORD_OVERHEAD_BYTES));
        self.iam_instance_profile_association_due_at
            .insert(association_id.clone(), due_instant(TRANSITION_DELAY));
        self.iam_instance_profile_association_by_instance
            .insert(spec.instance_id, association_id.clone());
        self.iam_instance_profile_associations
            .insert(association_id, association.clone());
        Some(association)
    }

    /// Swap the profile the association `association_id` is over for `iam_instance_profile`,
    /// over the SAME instance, and answer the association that took its place.
    ///
    /// "You can use this action to change the IAM instance profile that's associated with an
    /// instance without having to disassociate the existing IAM instance profile first"
    /// (`ReplaceIamInstanceProfileAssociation`): the association named is GONE when this
    /// answers — not left `disassociating` — and what replaces it is a new association of
    /// [`Self::insert_iam_instance_profile_association`], so the replacement carries its own
    /// id, its own timestamp and the `associating` state the clock then moves to
    /// `associated`, exactly as a first association does. At most one association names any
    /// one instance either way.
    ///
    /// Done HERE rather than as a remove and an insert in a handler so the swap is ATOMIC:
    /// the store's byte budget is asked about the NET change first
    /// ([`Self::would_exceed_stored_bytes_after_refund`]), so a replacement that will not fit
    /// leaves the association that is there exactly as it stands.
    pub fn replace_iam_instance_profile_association(
        &mut self,
        association_id: &str,
        iam_instance_profile: IamInstanceProfile,
    ) -> IamInstanceProfileAssociationReplacement {
        let Some(replaced) = self.iam_instance_profile_associations.get(association_id) else {
            return IamInstanceProfileAssociationReplacement::NoSuchAssociation;
        };
        // "The ID of the instance" the replaced association is over: the replacement is over
        // the same one, which is the whole point of replacing rather than associating.
        let Some(instance_id) = replaced.instance_id.clone() else {
            return IamInstanceProfileAssociationReplacement::NoSuchAssociation;
        };
        let refund = replaced
            .iam_instance_profile
            .as_ref()
            .map_or(0, iam_instance_profile_association_payload_bytes)
            .saturating_add(STORED_RECORD_OVERHEAD_BYTES);
        let charge = iam_instance_profile_association_payload_bytes(&iam_instance_profile)
            .saturating_add(STORED_RECORD_OVERHEAD_BYTES);
        if self.would_exceed_stored_bytes_after_refund(charge, refund) {
            return IamInstanceProfileAssociationReplacement::NoRoom;
        }
        self.remove_iam_instance_profile_association(association_id);
        match self.insert_iam_instance_profile_association(IamInstanceProfileAssociationSpec {
            instance_id,
            iam_instance_profile,
        }) {
            Some(association) => IamInstanceProfileAssociationReplacement::Replaced(association),
            // The removal above gave back a record and its bytes, and the budget was asked
            // about the net change before anything moved, so the insert cannot refuse the
            // replacement — answered rather than asserted, so a store that somehow has no
            // room says so instead of panicking.
            None => IamInstanceProfileAssociationReplacement::NoRoom,
        }
    }

    /// Start taking the association `association_id` away and answer it as the caller will
    /// read it, or `None` when the store holds no such association.
    ///
    /// The association is recorded [`IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING`]
    /// and the clock owes its REMOVAL one [`TRANSITION_DELAY`] later
    /// ([`Self::settle_due_iam_instance_profile_associations`]), so a caller that reads
    /// straight after the call still sees the transient state and the instance has already
    /// stopped running as the profile. An association that is ALREADY being taken away is
    /// answered as it stands, with the move the clock already owes it left where it is: the
    /// disassociation has been started once, and starting it again would move its due
    /// instant out.
    pub fn start_iam_instance_profile_disassociation(
        &mut self,
        association_id: &str,
    ) -> Option<IamInstanceProfileAssociation> {
        let association = self
            .iam_instance_profile_associations
            .get_mut(association_id)?;
        if association.state.as_deref()
            == Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING)
        {
            return Some(association.clone());
        }
        association.state =
            Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING.to_string());
        let association = association.clone();
        self.iam_instance_profile_association_due_at
            .insert(association_id.to_string(), due_instant(TRANSITION_DELAY));
        Some(association)
    }

    /// Remove the association `association_id` and answer it as it was last reported, or
    /// `None` when the store holds no such association.
    ///
    /// The exact reverse of [`Self::insert_iam_instance_profile_association`]: the record's
    /// bytes go back to the store's budget, and the index entry and the move the clock still
    /// owed it go with it, so an association followed by its removal leaves the store
    /// exactly where it started.
    pub fn remove_iam_instance_profile_association(
        &mut self,
        association_id: &str,
    ) -> Option<IamInstanceProfileAssociation> {
        let association = self
            .iam_instance_profile_associations
            .remove(association_id)?;
        if let Some(profile) = association.iam_instance_profile.as_ref() {
            self.release_stored_bytes(
                iam_instance_profile_association_payload_bytes(profile)
                    .saturating_add(STORED_RECORD_OVERHEAD_BYTES),
            );
        }
        if let Some(instance_id) = association.instance_id.as_deref() {
            // Only the entry that still points AT this association: an instance given a new
            // profile after this one was taken away points at the newer association.
            if self
                .iam_instance_profile_association_by_instance
                .get(instance_id)
                .is_some_and(|held| held == association_id)
            {
                self.iam_instance_profile_association_by_instance
                    .remove(instance_id);
            }
        }
        self.iam_instance_profile_association_due_at
            .remove(association_id);
        Some(association)
    }

    /// Apply the moves the clock owes this store's associations as of `now`: an
    /// `associating` association becomes `associated`, and a `disassociating` one is
    /// removed.
    ///
    /// Called from [`Self::materialize_due_transitions`], which every operation that reads
    /// or writes the store calls first: both moves were owed at their due instant whether or
    /// not anybody called, so applying them here changes nothing a later caller would not
    /// have seen anyway.
    pub(super) fn settle_due_iam_instance_profile_associations(&mut self, now: Instant) {
        let due: Vec<String> = self
            .iam_instance_profile_association_due_at
            .iter()
            .filter(|(_, due_at)| now >= **due_at)
            .map(|(association_id, _)| association_id.clone())
            .collect();
        for association_id in due {
            let disassociating = self
                .iam_instance_profile_associations
                .get(&association_id)
                .and_then(|association| association.state.as_deref())
                == Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_DISASSOCIATING);
            if disassociating {
                self.remove_iam_instance_profile_association(&association_id);
                continue;
            }
            if let Some(association) = self
                .iam_instance_profile_associations
                .get_mut(&association_id)
            {
                if association.state.as_deref()
                    == Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING)
                {
                    association.state =
                        Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED.to_string());
                }
            }
            // The move is applied once; nothing is owed on this association any more.
            self.iam_instance_profile_association_due_at
                .remove(&association_id);
        }
    }
}

/// Refuse a request whose association id this service cannot READ as one of its own: HTTP
/// 400 `InvalidParameterValue`, with the message EC2 builds from the value the request
/// named.
///
/// The malformed half of the pair every association-id reader of this resource answers
/// ([`ComAmazonawsEc2Amazonec2State::unresolvable_iam_instance_profile_association_id`]): an
/// id reaches EC2 encoded rather than as the identifier it stands for, so a value no issue
/// of this kind could have produced is refused BEFORE any lookup rather than reported as an
/// association that does not exist.
pub fn invalid_iam_instance_profile_association_id<E>(association_id: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!("Invalid Id: {association_id}"))
}

/// Refuse a request that named an association id this service could have ISSUED but holds
/// no association for: HTTP 400 [`INVALID_ASSOCIATION_ID_NOT_FOUND`], with the sentence EC2
/// words for an IAM instance profile association it cannot find.
///
/// The not-found half of that pair. The wording is this resource's own — recorded as "An
/// invalid association-id of '<id>' was given" rather than the `The <kind> '<id>' does not
/// exist` template [`invalid_association_id_not_found`] builds for a route table
/// association — so the id the request named is the only part of it that varies.
pub fn iam_instance_profile_association_id_not_found<E>(association_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ASSOCIATION_ID_NOT_FOUND,
        400,
        format!("An invalid association-id of '{association_id}' was given"),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Refuse an `IamInstanceProfileAssociationId` this store resolves to no association —
    /// the ONE reading of that failure, shared by every operation of this crate that
    /// resolves one, so two of them cannot classify one value differently.
    ///
    /// Which of the two refusals a value takes is the pair this crate reads every id kind
    /// by ([`Self::id_was_issued`]): a value the mint could have handed out is looked up and
    /// answered `InvalidAssociationID.NotFound` — the association an issued id named may
    /// since have gone, which is what a replacement of it leaves behind — while a value
    /// outside the issued id space is refused as an id this service cannot read at all
    /// ([`invalid_iam_instance_profile_association_id`]).
    pub fn unresolvable_iam_instance_profile_association_id<E>(
        &self,
        association_id: &str,
    ) -> Scaffolded<E> {
        if self.id_was_issued(IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX, association_id) {
            iam_instance_profile_association_id_not_found(association_id)
        } else {
            invalid_iam_instance_profile_association_id(association_id)
        }
    }
}

/// Refuse giving an instance a second IAM instance profile: HTTP 400
/// [`INCORRECT_STATE`], naming the instance that already holds one.
///
/// "You cannot associate more than one IAM instance profile with an instance"
/// (`AssociateIamInstanceProfile`), and an instance that already runs as one is in a state
/// that does not admit another, which is what this code says.
pub fn existing_iam_instance_profile_association<E>(instance_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INCORRECT_STATE,
        400,
        format!("There is an existing association for instance {instance_id}"),
    )
}

/// The most characters an IAM instance profile name may carry: IAM's own
/// `instanceProfileNameType` (len 1..128, pattern `^[\w+=,.@-]+$`), the type every
/// `InstanceProfileName` member of the IAM model carries, so a name EC2 accepts here is one
/// IAM could hold a profile under.
pub const MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS: usize = 128;

/// Whether `name` is a name IAM could hold an instance profile under: between one and
/// [`MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS`] of the characters
/// `instanceProfileNameType`'s pattern admits.
///
/// A path is NOT one of them: a name is the last segment of the profile's ARN, and `/` is
/// what separates it from the path.
pub fn iam_instance_profile_name_is_well_formed(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+=,.@_-".contains(&byte))
}

/// The instance profile the account's IAM records hold under `name`, as EC2 reports a
/// profile.
///
/// `IamInstanceProfile$Id` is "The ID of the instance profile" — IAM's own
/// `InstanceProfile$InstanceProfileId`, which no EC2 call mints — so both members an
/// association reports come from the component that OWNS instance profiles, through IAM's
/// own read of one ([`iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State::instance_profile_report`]),
/// the way this crate's authorization gate reads that store rather than keeping a second
/// copy of it. The ARN comes from the same record, so a profile IAM holds under a PATH is
/// reported under the ARN that path gives it.
///
/// `None` when this account's IAM records hold no profile under that name: a profile this
/// emulator has no record of is reported as the request spelled it and with NO id, rather
/// than with one invented here.
fn iam_held_instance_profile<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
) -> Option<IamInstanceProfile> {
    let iam = iam_store::service_state(engine)?;
    let held = iam
        .lock()
        .instance_profile_report(&iam_store::name_key(name))?;
    Some(IamInstanceProfile {
        arn: Some(held.arn),
        id: Some(held.instance_profile_id),
    })
}

/// The instance profile an `IamInstanceProfileSpecification` names, as an association
/// reports it (`IamInstanceProfileAssociation$IamInstanceProfile`).
///
/// The specification carries "the Amazon Resource Name (ARN) of the instance profile"
/// (`IamInstanceProfileSpecification$Arn`) or "the name of the instance profile"
/// (`IamInstanceProfileSpecification$Name`) — two spellings of ONE profile, so exactly one
/// of them identifies it: both together are an `InvalidParameterCombination` and neither is
/// a `MissingParameter`. Whichever spelling the request took, the profile is resolved
/// against the account's IAM records ([`iam_held_instance_profile`]), so an association
/// reports the ARN and the unique id IAM holds that profile under — and a reference IAM has
/// no record of is reported as the request spelled it, with no id, rather than refused: this
/// service holds no record of what IAM does and does not have.
///
/// The ONE reading of that member, shared by every operation that takes one, so two of them
/// cannot accept different values for one profile.
pub fn resolve_iam_instance_profile<E, Eng: Send + Sync + 'static>(
    engine: &Eng,
    specification: &IamInstanceProfileSpecification,
) -> Result<IamInstanceProfile, Scaffolded<E>> {
    // ec2Query carries an omitted member and an empty one identically, and an empty value
    // names no profile either way, so both read as "not given".
    let arn = specification
        .arn
        .as_deref()
        .filter(|arn| !arn.is_empty());
    let name = specification
        .name
        .as_deref()
        .filter(|name| !name.is_empty());
    match (arn, name) {
        (Some(_), Some(_)) => Err(invalid_parameter_combination(
            "Specify either iamInstanceProfile.name or iamInstanceProfile.arn, but not both",
        )),
        (None, None) => Err(missing_parameter(
            "Either iamInstanceProfile.name or iamInstanceProfile.arn must be specified",
        )),
        (Some(arn), None) => {
            if !iam_instance_profile_arn_is_well_formed(arn) {
                return Err(invalid_parameter_value(format!(
                    "Value ({arn}) for parameter iamInstanceProfile.arn is invalid. \
                     Invalid IAM Instance Profile ARN"
                )));
            }
            // An ARN names the profile IAM holds under exactly that ARN: the name is the
            // last segment of it, and the record is read only when the ARN IAM holds that
            // profile under is the one the request sent, so an ARN whose path names no
            // profile of this account is not quietly answered with another profile's id.
            Ok(::service_common::iam_role::instance_profile_name(arn)
                .and_then(|name| iam_held_instance_profile(engine, name))
                .filter(|held| held.arn.as_deref() == Some(arn))
                .unwrap_or_else(|| IamInstanceProfile {
                    arn: Some(arn.to_string()),
                    id: None,
                }))
        }
        (None, Some(name)) => {
            if !iam_instance_profile_name_is_well_formed(name) {
                return Err(invalid_parameter_value(format!(
                    "Value ({name}) for parameter iamInstanceProfile.name is invalid. \
                     Invalid IAM Instance Profile name"
                )));
            }
            Ok(
                iam_held_instance_profile(engine, name).unwrap_or_else(|| IamInstanceProfile {
                    arn: Some(::engine::emulator_arn(
                        "iam",
                        "",
                        &format!("{IAM_INSTANCE_PROFILE_RESOURCE_PREFIX}{name}"),
                    )),
                    id: None,
                }),
            )
        }
    }
}
