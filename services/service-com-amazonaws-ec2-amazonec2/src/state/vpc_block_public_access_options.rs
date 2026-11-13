// SPDX-License-Identifier: CC0-1.0

//! The VPC Block Public Access (BPA) options an account holds in one Region: the mode
//! `ModifyVpcBlockPublicAccessOptions` puts the Region into and
//! `DescribeVpcBlockPublicAccessOptions` reports back out of that one record.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there. The EXCLUSIONS that exempt a single VPC or subnet from these
//! options are a store of their own (see
//! [`ComAmazonawsEc2Amazonec2State::vpc_block_public_access_exclusions`]): an exclusion is a
//! resource with an id, these options are one setting per Region.

use super::*;

/// "`off`: VPC BPA is not enabled and traffic is allowed to and from internet gateways and
/// egress-only internet gateways in this Region"
/// (`VpcBlockPublicAccessOptions$InternetGatewayBlockMode`): the `InternetGatewayBlockMode`
/// enum's `off` member.
///
/// It is also the mode of a Region the account has never modified — BPA blocks nothing until
/// it is turned on — so a Region with no stored record reports it. The whole catalog is the
/// generated [`crate::ops::types::INTERNET_GATEWAY_BLOCK_MODE_VALUES`], which is why the two
/// blocking members are not spelled here.
pub const INTERNET_GATEWAY_BLOCK_MODE_OFF: &str = "off";

/// The `VpcBlockPublicAccessState` enum's `default-state` member: where a Region whose
/// options the account has never modified sits, nothing having put it into an update yet.
///
/// The generated catalog of all three states is
/// [`crate::ops::types::VPC_BLOCK_PUBLIC_ACCESS_STATE_VALUES`].
pub const VPC_BPA_STATE_DEFAULT: &str = "default-state";

/// The `VpcBlockPublicAccessState` enum's `update-in-progress` member: the NON-TERMINAL state
/// a modify leaves the Region's options in, which the clock owes
/// [`VPC_BPA_STATE_UPDATE_COMPLETE`] one [`TRANSITION_DELAY`] later.
pub const VPC_BPA_STATE_UPDATE_IN_PROGRESS: &str = "update-in-progress";

/// The `VpcBlockPublicAccessState` enum's `update-complete` member: the terminal state of a
/// Region whose options a modify has changed, reached once the clock has passed the due
/// instant of that update.
pub const VPC_BPA_STATE_UPDATE_COMPLETE: &str = "update-complete";

/// "If you have enabled VPC BPA at the Organization level, exclusions may be `not-allowed`.
/// Otherwise, they are `allowed`" (`VpcBlockPublicAccessOptions$ExclusionsAllowed`): the
/// `VpcBlockPublicAccessExclusionsAllowed` enum's `allowed` member.
///
/// The only writer of these options this emulator has is the account itself
/// ([`MANAGED_BY_ACCOUNT`], for the same reason), and enabling VPC BPA at the Organization
/// level is not an EC2 action, so exclusions are always allowed here. The generated catalog
/// is [`crate::ops::types::VPC_BLOCK_PUBLIC_ACCESS_EXCLUSIONS_ALLOWED_VALUES`].
pub const VPC_BPA_EXCLUSIONS_ALLOWED: &str = "allowed";

/// The modelled `InternetGatewayBlockMode` member `requested` names, or `None` when it names
/// none.
///
/// The catalog is the generated [`crate::ops::types::INTERNET_GATEWAY_BLOCK_MODE_VALUES`], so
/// this crate holds no second copy of it, and every member of it is a mode a modify accepts —
/// `off` is how the account turns VPC BPA back off. Answering a `&'static str` out of that
/// catalog is what keeps request text out of the store.
pub fn internet_gateway_block_mode(requested: &str) -> Option<&'static str> {
    catalog_member(crate::ops::types::INTERNET_GATEWAY_BLOCK_MODE_VALUES, requested)
}

/// How many Regions this emulator holds VPC BPA options for.
///
/// CHOSEN, like [`MAX_IMAGE_BLOCK_PUBLIC_ACCESS_REGIONS`] and for the same reason: the Region
/// comes from the caller's SigV4 credential scope, so without a ceiling a caller could modify
/// the options of unboundedly many Region names and nothing would ever reclaim the entries. An
/// emulator storage bound, not an AWS-modelled refusal —
/// `ModifyVpcBlockPublicAccessOptions` models no error at all.
pub const MAX_VPC_BPA_OPTION_REGIONS: usize = MAX_ENCRYPTION_BY_DEFAULT_REGIONS;

/// One Region's VPC BPA options: the mode in force, the state that mode's last update
/// reached, when that update was made, and the move the clock still owes it.
///
/// The modelled [`crate::ops::types::VpcBlockPublicAccessOptions`] is PROJECTED from this
/// ([`ComAmazonawsEc2Amazonec2State::vpc_block_public_access_options`]) rather than stored,
/// because the account id and Region it reports are facts of the request and the emulator
/// rather than of the setting, and the two words it carries are catalog members this store
/// holds as `&'static str` so a request's own text can never reach it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct VpcBlockPublicAccessOptionsRecord {
    /// "The current mode of VPC BPA" (`VpcBlockPublicAccessOptions$InternetGatewayBlockMode`),
    /// as a member of the modelled catalog ([`internet_gateway_block_mode`]).
    pub internet_gateway_block_mode: &'static str,
    /// "The current state of VPC BPA" (`VpcBlockPublicAccessOptions$State`): one of this
    /// module's `VPC_BPA_STATE_*` constants.
    pub state: &'static str,
    /// "The last time the VPC BPA mode was updated"
    /// (`VpcBlockPublicAccessOptions$LastUpdateTimestamp`) — the instant of the modify that
    /// set [`Self::internet_gateway_block_mode`], which is when the mode last changed.
    pub last_update_timestamp: SystemTime,
    /// The move the clock owes these options, while one is owed: `update-in-progress` ->
    /// [`VPC_BPA_STATE_UPDATE_COMPLETE`].
    pub pending: Option<ClockTransition<&'static str>>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// This account's VPC BPA options in `region`, as the modelled shape both operations
    /// report them with.
    ///
    /// The ONE projection, so the modify and the describe cannot report one Region's options
    /// differently. A Region with no stored record is one the account has never modified: VPC
    /// BPA blocks nothing there ([`INTERNET_GATEWAY_BLOCK_MODE_OFF`]) and nothing has put it
    /// into an update ([`VPC_BPA_STATE_DEFAULT`]), so it reports those without storing
    /// anything — and with no `LastUpdateTimestamp` at all, the mode never having been
    /// updated.
    ///
    /// A pure READ. The state move an update owes is applied by
    /// [`Self::complete_due_vpc_block_public_access_option_updates`], off the clock, so this
    /// cannot change what a later caller sees.
    pub fn vpc_block_public_access_options(
        &self,
        region: &str,
    ) -> crate::ops::types::VpcBlockPublicAccessOptions {
        let record = self.vpc_block_public_access_options.get(region);
        crate::ops::types::VpcBlockPublicAccessOptions {
            // "An Amazon Web Services account ID" (`$AwsAccountId`): the account this
            // emulator serves, which nothing a request carries can name.
            aws_account_id: Some(::engine::emulator_account_id().to_string()),
            // "An Amazon Web Services Region" (`$AwsRegion`): the options are per Region, so
            // the one asked about.
            aws_region: Some(region.to_string()),
            state: Some(record.map_or(VPC_BPA_STATE_DEFAULT, |held| held.state).to_string()),
            internet_gateway_block_mode: Some(
                record
                    .map_or(INTERNET_GATEWAY_BLOCK_MODE_OFF, |held| {
                        held.internet_gateway_block_mode
                    })
                    .to_string(),
            ),
            // "The reason for the current state": nothing has gone wrong with an account's
            // own update, so there is nothing to report.
            reason: None,
            last_update_timestamp: record
                .map(|held| system_time_to_date_time(held.last_update_timestamp)),
            managed_by: Some(MANAGED_BY_ACCOUNT.to_string()),
            exclusions_allowed: Some(VPC_BPA_EXCLUSIONS_ALLOWED.to_string()),
        }
    }

    /// Put `region`'s VPC BPA mode to `mode` and answer the options now in effect.
    ///
    /// `mode` is a `&'static str` so only a member of the modelled catalog can be stored
    /// ([`internet_gateway_block_mode`] is what turns a request's word into one), and the
    /// answer is read back off the store through [`Self::vpc_block_public_access_options`]
    /// rather than echoed, so the writer reports exactly what a later describe will.
    ///
    /// The options are left in the non-terminal [`VPC_BPA_STATE_UPDATE_IN_PROGRESS`] with the
    /// clock owing them [`VPC_BPA_STATE_UPDATE_COMPLETE`] one [`TRANSITION_DELAY`] later, the
    /// span every other clock-owed move of this crate takes, so a caller reading straight
    /// after the modify still sees the update in flight.
    pub fn set_vpc_block_public_access_options(
        &mut self,
        region: &str,
        mode: &'static str,
    ) -> crate::ops::types::VpcBlockPublicAccessOptions {
        let record = self
            .vpc_block_public_access_options
            .entry(region.to_string())
            .or_insert_with(|| VpcBlockPublicAccessOptionsRecord {
                internet_gateway_block_mode: INTERNET_GATEWAY_BLOCK_MODE_OFF,
                state: VPC_BPA_STATE_DEFAULT,
                last_update_timestamp: SystemTime::now(),
                pending: None,
            });
        record.internet_gateway_block_mode = mode;
        record.state = VPC_BPA_STATE_UPDATE_IN_PROGRESS;
        record.last_update_timestamp = SystemTime::now();
        record.pending = Some(ClockTransition::starting_now(VPC_BPA_STATE_UPDATE_COMPLETE));
        self.vpc_block_public_access_options(region)
    }

    /// Would modifying the options of `region` take the store past
    /// [`MAX_VPC_BPA_OPTION_REGIONS`]?
    ///
    /// A Region the store already holds a record for costs nothing, so modifying one again is
    /// never refused — the same contract as
    /// [`Self::would_exceed_image_block_public_access_regions`].
    pub fn would_exceed_vpc_block_public_access_option_regions(&self, region: &str) -> bool {
        !self.vpc_block_public_access_options.contains_key(region)
            && self.vpc_block_public_access_options.len() >= MAX_VPC_BPA_OPTION_REGIONS
    }

    /// Apply the `update-in-progress` -> `update-complete` move the clock owes every Region
    /// whose update has fallen due.
    ///
    /// Owed at the same instant whether or not anybody called, which is why it is applied
    /// from [`Self::materialize_due_transitions`] rather than by the modify: the update
    /// completed when its due instant passed, not when somebody next asked about it.
    ///
    /// `LastUpdateTimestamp` is left where the modify put it: it is "the last time the VPC BPA
    /// MODE was updated" (`VpcBlockPublicAccessOptions$LastUpdateTimestamp`), and this move
    /// changes the state the update reached rather than the mode in force.
    pub fn complete_due_vpc_block_public_access_option_updates(&mut self, now: Instant) {
        for record in self.vpc_block_public_access_options.values_mut() {
            let due = record
                .pending
                .as_ref()
                .filter(|pending| now >= pending.due_at)
                .map(|pending| pending.to);
            if let Some(state) = due {
                record.state = state;
                record.pending = None;
            }
        }
    }
}
