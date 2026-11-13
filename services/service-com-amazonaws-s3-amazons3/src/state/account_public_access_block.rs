// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-level Block Public Access configuration in force on this store's buckets.
//!
//! S3 Block Public Access is configured at TWO levels: on a bucket, through this service's
//! own `PutPublicAccessBlock`, and on the ACCOUNT, through S3 Control's
//! `PutPublicAccessBlock` — and "when Amazon S3 receives a request to access a bucket or an
//! object, it determines whether the account or the bucket has a block public access
//! setting applied … if the request was made through an access point, Amazon S3 also checks
//! the block public access settings for the access point. If there is an existing block
//! public access setting that prohibits the requested access, Amazon S3 rejects the request"
//! (Amazon S3 User Guide, *Blocking public access to your Amazon S3 storage*). So the
//! settings in force on a bucket are the MOST RESTRICTIVE of the two configurations that
//! apply ([`settings_in_force`]): a setting on at either level is on.
//!
//! The account-level configuration is a fact the DATA PLANE is judged by, so it lives in the
//! store the data plane serves from — this one — exactly as an access point's route and
//! policy do (`state/access_points.rs`) and as a bucket's tag set does. S3 Control owns the
//! API over it and writes it here on the three calls that put, read and remove it; a copy of
//! its own in that service's store would mean the configuration a client set and the
//! configuration this data plane enforces were different records.
//!
//! Keyed by ACCOUNT id, because that is the scope of the configuration: one per account,
//! applying to every bucket that account owns ([`ComAmazonawsS3Amazons3State::
//! owner_account_public_access_block`] is the one in force on the buckets this store holds,
//! which this emulator serves for a single account).

use super::*;

/// How many accounts' configurations this store holds at once.
///
/// An EMULATOR ceiling, not an AWS quota (the service that writes them enforces its own,
/// `PUBLIC_ACCESS_BLOCK_QUOTA`): these records live in memory for the lifetime of the
/// process, so a caller naming account after account must not be able to grow this map
/// without end. It is set above that quota, so a put the owning service admits is always
/// recorded here, and past it a further account's configuration is not recorded rather than
/// one a live workflow set being dropped.
pub const MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS: usize = 10_000;

/// The Block Public Access settings in force on a bucket: the per-setting union of the
/// bucket's own configuration (`bucket`, `None` for a bucket carrying none) and the owning
/// account's (`account`, `None` for an account that has none).
///
/// The union, because the two levels are independent guardrails and "if there is an existing
/// block public access setting that prohibits the requested access, Amazon S3 rejects the
/// request" (Amazon S3 User Guide, *Blocking public access to your Amazon S3 storage*) — so
/// the most restrictive of the settings that apply is the one in force, and an account-level
/// setting cannot be turned off by a bucket's own configuration. With neither configuration
/// present nothing is blocked, which is the state a bucket whose configuration was deleted in
/// an account that has none is in.
pub fn settings_in_force(
    bucket: Option<BucketPublicAccessBlock>,
    account: Option<BucketPublicAccessBlock>,
) -> BucketPublicAccessBlock {
    let bucket = bucket.unwrap_or_default();
    let account = account.unwrap_or_default();
    BucketPublicAccessBlock {
        block_public_acls: bucket.block_public_acls || account.block_public_acls,
        ignore_public_acls: bucket.ignore_public_acls || account.ignore_public_acls,
        block_public_policy: bucket.block_public_policy || account.block_public_policy,
        restrict_public_buckets: bucket.restrict_public_buckets || account.restrict_public_buckets,
    }
}

impl ComAmazonawsS3Amazons3State {
    /// The account-level configuration `account_id` carries, or `None` when that account has
    /// none — the state an account that has never had one put is in, and the one
    /// `DeletePublicAccessBlock` returns it to.
    pub fn account_public_access_block(
        &self,
        account_id: &str,
    ) -> Option<BucketPublicAccessBlock> {
        self.account_public_access_blocks.get(account_id).copied()
    }
    /// The account-level configuration in force on the buckets this store holds, or `None`
    /// when that account has none.
    ///
    /// Every bucket of this store belongs to the one account this emulator serves
    /// (`::engine::emulator_account_id`, the account every bucket's owner and every ARN this
    /// service reports names), so that is the account whose configuration applies to it.
    pub fn owner_account_public_access_block(&self) -> Option<BucketPublicAccessBlock> {
        self.account_public_access_block(::engine::emulator_account_id())
    }

    /// The settings in force on `bucket`: its own configuration together with its account's
    /// ([`settings_in_force`]).
    ///
    /// For a reader that already holds the bucket record. A path that holds the record
    /// MUTABLY reads the account half off the store first ([`Self::
    /// owner_account_public_access_block`]) and joins it with [`settings_in_force`], since
    /// the store cannot be borrowed again while it does.
    pub fn public_access_block_in_force(&self, bucket: &Bucket) -> BucketPublicAccessBlock {
        settings_in_force(
            bucket.public_access_block(),
            self.owner_account_public_access_block(),
        )
    }

    /// Record `settings` as the whole configuration of the account `account_id`, replacing
    /// whatever that account carried.
    ///
    /// A replacement, not a merge: the account-level put "creates or modifies the
    /// `PublicAccessBlock` configuration for an Amazon Web Services account"
    /// (`PutPublicAccessBlock`) from the complete configuration it carries, exactly as the
    /// bucket-level put replaces a bucket's ([`Bucket::set_public_access_block`]).
    ///
    /// Called by the service that owns the API over it. Answers whether the configuration was
    /// recorded: past [`MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS`] a FURTHER account's is not, and
    /// nothing already recorded is dropped to make room — a caller that cannot record one
    /// must not report it as stored.
    pub fn record_account_public_access_block(
        &mut self,
        account_id: &str,
        settings: BucketPublicAccessBlock,
    ) -> bool {
        if self.account_public_access_blocks.len() >= MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS
            && !self.account_public_access_blocks.contains_key(account_id)
        {
            return false;
        }
        self.account_public_access_blocks
            .insert(account_id.to_string(), settings);
        true
    }

    /// Remove the configuration of the account `account_id`, leaving it with none, and report
    /// whether it carried one.
    ///
    /// What the account-level delete does, and it releases the slot the account held in the
    /// bounded map — so the guardrail stops applying to this store's buckets at the moment the
    /// configuration stops existing.
    pub fn forget_account_public_access_block(&mut self, account_id: &str) -> bool {
        self.account_public_access_blocks.remove(account_id).is_some()
    }

    /// How many accounts' configurations this store holds.
    pub fn account_public_access_blocks(&self) -> usize {
        self.account_public_access_blocks.len()
    }
}

/// The configuration the account `account_id` carries, read out of the store in `engine`.
///
/// For the service that owns the API over it, and for any seam that holds no guard of this
/// store's: it takes this store's guard, copies the settings out and hands back an owned
/// value, so nothing holds the guard across another store's.
pub fn account_public_access_block_in<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
) -> Option<BucketPublicAccessBlock> {
    crate::state::service_state(engine)
        .and_then(|state| state.lock().account_public_access_block(account_id))
}

/// Record `settings` as the configuration of the account `account_id` in the store in
/// `engine`, answering whether the store recorded it (see
/// [`ComAmazonawsS3Amazons3State::record_account_public_access_block`]). `None` when `engine`
/// holds no such store at all, which is not the same answer as a refusal to record.
pub fn record_account_public_access_block_in<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
    settings: BucketPublicAccessBlock,
) -> Option<bool> {
    crate::state::service_state(engine).map(|state| {
        state
            .lock()
            .record_account_public_access_block(account_id, settings)
    })
}

/// Remove the configuration of the account `account_id` from the store in `engine`, answering
/// whether that account carried one. `None` when `engine` holds no such store at all.
pub fn forget_account_public_access_block_in<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
) -> Option<bool> {
    crate::state::service_state(engine)
        .map(|state| state.lock().forget_account_public_access_block(account_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 12-digit account id, as `AccountId`'s `^\d{12}$` requires, distinct per `index`.
    fn account(index: usize) -> String {
        format!("{:012}", 100_000_000_000u64 + index as u64)
    }

    /// The four settings as one configuration, in the order
    /// `PublicAccessBlockConfiguration` declares its members.
    fn settings(flags: [bool; 4]) -> BucketPublicAccessBlock {
        BucketPublicAccessBlock {
            block_public_acls: flags[0],
            ignore_public_acls: flags[1],
            block_public_policy: flags[2],
            restrict_public_buckets: flags[3],
        }
    }

    /// The four settings of `in_force`, in the same order.
    fn flags_of(in_force: BucketPublicAccessBlock) -> [bool; 4] {
        [
            in_force.block_public_acls,
            in_force.ignore_public_acls,
            in_force.block_public_policy,
            in_force.restrict_public_buckets,
        ]
    }

    #[test]
    fn the_settings_in_force_are_the_union_of_the_two_levels() {
        // Every combination of a bucket-level and an account-level configuration, including
        // the rows where one level or the other carries none at all: the setting in force is
        // on when it is on at either level.
        let configurations: Vec<Option<[bool; 4]>> = ::std::iter::once(None)
            .chain((0..16u8).map(|bits| {
                Some([
                    bits & 1 != 0,
                    bits & 2 != 0,
                    bits & 4 != 0,
                    bits & 8 != 0,
                ])
            }))
            .collect();
        for bucket in &configurations {
            for account in &configurations {
                let expected = [0, 1, 2, 3].map(|index| {
                    bucket.is_some_and(|flags| flags[index])
                        || account.is_some_and(|flags| flags[index])
                });
                let in_force = settings_in_force(
                    bucket.map(settings),
                    account.map(settings),
                );
                assert_eq!(
                    flags_of(in_force),
                    expected,
                    "case bucket {bucket:?} with account {account:?}: expected {expected:?}, \
                     got {:?}",
                    flags_of(in_force)
                );
            }
        }
    }

    #[test]
    fn a_recorded_configuration_is_in_force_until_it_is_forgotten() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        let owner = ::engine::emulator_account_id();
        assert_eq!(
            store.owner_account_public_access_block(),
            None,
            "case a fresh store: expected no account-level configuration, got {:?}",
            store.owner_account_public_access_block()
        );

        assert!(
            store.record_account_public_access_block(owner, settings([true; 4])),
            "case the owner account: expected the configuration recorded"
        );
        assert_eq!(
            store.owner_account_public_access_block().map(flags_of),
            Some([true; 4]),
            "case the owner account: expected the settings just recorded, got {:?}",
            store.owner_account_public_access_block()
        );
        // Another account's configuration is a record of its own and says nothing about this
        // store's buckets.
        assert!(store.record_account_public_access_block(&account(0), settings([false; 4])));
        assert_eq!(
            store.account_public_access_block(&account(0)).map(flags_of),
            Some([false; 4]),
            "case another account: expected its own settings, got {:?}",
            store.account_public_access_block(&account(0))
        );
        assert_eq!(
            store.owner_account_public_access_block().map(flags_of),
            Some([true; 4]),
            "case another account: expected the owner's configuration untouched"
        );

        // A second record for one account replaces it rather than adding a record.
        assert!(store.record_account_public_access_block(owner, settings([false, true, false, true])));
        assert_eq!(
            (
                store.account_public_access_blocks(),
                store.owner_account_public_access_block().map(flags_of)
            ),
            (2, Some([false, true, false, true])),
            "case a replacing record: expected one record per account carrying the last \
             settings, got {} record(s)",
            store.account_public_access_blocks()
        );

        assert!(
            store.forget_account_public_access_block(owner),
            "case a recorded account: expected the delete to report the configuration it removed"
        );
        assert_eq!(
            (
                store.owner_account_public_access_block(),
                store.forget_account_public_access_block(owner)
            ),
            (None, false),
            "case a forgotten account: expected none in force and a repeat delete to report none"
        );
    }

    #[test]
    fn the_ceiling_bounds_how_many_accounts_the_store_holds() {
        let mut store = ComAmazonawsS3Amazons3State::default();
        for index in 0..MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS {
            assert!(
                store.record_account_public_access_block(&account(index), settings([true; 4])),
                "case account {index}: expected it recorded under the ceiling"
            );
        }
        // An account already recorded stays writable at the ceiling, because replacing its
        // configuration adds no record.
        assert!(
            store.record_account_public_access_block(&account(0), settings([false; 4])),
            "case a replace at the ceiling: expected it accepted"
        );
        assert!(
            !store.record_account_public_access_block(
                &account(MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS),
                settings([true; 4])
            ),
            "case one past the ceiling: expected the record refused, got {} held",
            store.account_public_access_blocks()
        );
        assert_eq!(
            (
                store.account_public_access_blocks(),
                store.account_public_access_block(&account(0)).map(flags_of)
            ),
            (MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS, Some([false; 4])),
            "case one past the ceiling: expected the ceiling held and nothing dropped, got {}",
            store.account_public_access_blocks()
        );
        // A delete releases a slot, so the store cannot wedge.
        assert!(store.forget_account_public_access_block(&account(0)));
        assert!(
            store.record_account_public_access_block(
                &account(MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS),
                settings([true; 4])
            ),
            "case after a delete: expected the released slot to take a further account"
        );
    }
}
