// SPDX-License-Identifier: CC0-1.0

//! The ClassicLink links the account holds: for each linked EC2-Classic instance, the
//! ClassicLink-enabled VPC it is linked to and the VPC security groups it was linked with.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! `client_vpn.rs` gives: one subject — the link, its bounds and its projection — in one
//! file, and out of the end of `state.rs`, where every concurrently landing EC2 fix
//! inserts. Everything here is re-exported by `crate::state`, so a handler reaches it as
//! `crate::state::<item>` exactly as it reaches the helpers in the parent module.
//!
//! The store itself is a field of [`ComAmazonawsEc2Amazonec2State`]
//! (`classic_link_instances`), because the state struct is one struct; only the code that
//! reads and writes it lives here.
//!
//! ## Why the link is stored, and not the instance again
//!
//! A ClassicLink link is a RELATIONSHIP between an instance the account already holds and
//! a VPC it already holds, so this store keeps only what the link itself adds — the VPC and
//! the security groups — keyed by the `InstanceId` it is a link of. Everything else
//! `ClassicLinkInstance` reports is read back off those resources by
//! [`ComAmazonawsEc2Amazonec2State::classic_link_instance_view`]: the instance's own tags
//! (which `CreateTags` and `DeleteTags` write) and each security group's name (which
//! `CreateSecurityGroup` writes). So a tag written after the link is reported by the next
//! describe, and no second copy of a tag or a group name can drift from the resource it
//! belongs to.
//!
//! The store starts EMPTY and stays empty until something links an instance: EC2-Classic
//! instances are not created by any EC2 API operation, and the link is made by
//! `AttachClassicLinkVpc`, which is still a scaffold.
//! [`ComAmazonawsEc2Amazonec2State::link_classic_link_vpc`] is the one way a link is
//! stored — the seam that operation writes through — and it keeps the store consistent by
//! refusing a link whose instance or VPC this account does not hold, so a reported link
//! always names resources that exist.
//!
//! ## Whether a VPC is enabled for ClassicLink is a fact of the VPC
//!
//! A link names a VPC, but being ENABLED for ClassicLink is a property of the VPC itself,
//! which the modelled `Vpc` shape carries no member for — so it is held beside the VPC
//! record, keyed by the same id, in
//! [`ComAmazonawsEc2Amazonec2State::classic_link_enabled_vpcs`].
//! [`ComAmazonawsEc2Amazonec2State::set_vpc_classic_link_enabled`] is the one way that flag
//! is written (`DisableVpcClassicLink` writes `false` through it; `EnableVpcClassicLink`,
//! still a scaffold, must write `true` through it), and
//! [`ComAmazonawsEc2Amazonec2State::vpc_classic_link_view`] is the one projection of it into
//! the modelled `VpcClassicLink` shape, so a writer and a reporter cannot disagree about one
//! VPC's status.
//!
//! ## DNS support is a SECOND fact of the VPC
//!
//! Whether a VPC resolves a linked EC2-Classic instance's DNS hostname to its private address
//! is enablement of its own — `Enable`/`DisableVpcClassicLinkDnsSupport` set it and
//! `DescribeVpcClassicLinkDnsSupport` reports it — so it is held in a second set keyed by the
//! same `VpcId` ([`ComAmazonawsEc2Amazonec2State::classic_link_dns_enabled_vpcs`]) rather
//! than inferred from the ClassicLink enablement above: a VPC may be enabled for ClassicLink
//! with DNS support off, and the two are written by different operations.
//! [`ComAmazonawsEc2Amazonec2State::set_vpc_classic_link_dns_enabled`] is the one way that
//! flag is written and
//! [`ComAmazonawsEc2Amazonec2State::classic_link_dns_support_view`] the one projection of it
//! into the modelled `ClassicLinkDnsSupport` shape.

use super::*;

/// One ClassicLink link: where an EC2-Classic instance is linked, and with what.
///
/// Not the modelled `ClassicLinkInstance` shape, unlike most stores of this module,
/// because two of that shape's four members are not facts of the LINK: the instance's tags
/// and a security group's name belong to the instance and the group, and are read off them
/// when the link is projected.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ClassicLinkRecord {
    /// "The ID of the ClassicLink-enabled VPC" the instance is linked to
    /// (`AttachClassicLinkVpcRequest$VpcId`), reported as `ClassicLinkInstance$VpcId`.
    pub vpc_id: String,
    /// "The IDs of the security groups" the instance was linked with
    /// (`AttachClassicLinkVpcRequest$Groups`), in the order they were named, reported as
    /// `ClassicLinkInstance$Groups`.
    pub group_ids: Vec<String>,
}

/// The caller-chosen bytes one stored link holds: the VPC id and the security group ids a
/// writer named. The key is an instance id this service minted, so it is fixed-size.
///
/// The ONE definition of that charge, the way every other store of this module has one:
/// [`ComAmazonawsEc2Amazonec2State::link_classic_link_vpc`] charges exactly it, and a path
/// that takes a link back out must give back the same number.
pub fn classic_link_payload_bytes(record: &ClassicLinkRecord) -> usize {
    record
        .group_ids
        .iter()
        .fold(record.vpc_id.len(), |total, group_id| {
            total.saturating_add(group_id.len())
        })
}

impl ComAmazonawsEc2Amazonec2State {
    /// Link the instance `instance_id` to the ClassicLink-enabled VPC `vpc_id` with the VPC
    /// security groups `group_ids`, and answer whether the store took the link.
    ///
    /// `false` means the link was not stored, and the caller must refuse the request rather
    /// than report a link that does not exist. That happens when the account holds no such
    /// instance or no such VPC (the store never holds a link to a resource that is not
    /// there, so a describe cannot report one), when more security groups are named than an
    /// instance may carry ([`MAX_SECURITY_GROUPS_PER_INSTANCE`]), or when the link's
    /// caller-chosen bytes do not fit the store's byte budget
    /// ([`Self::would_exceed_stored_bytes_after_refund`]) — nothing in this emulator
    /// reclaims a stored link, so the one seam it is written through is where that budget
    /// has to be asked.
    ///
    /// An instance can be linked to one VPC at a time, so a second link for the same
    /// instance REPLACES the first and the bytes the first was charged are given back.
    ///
    /// Its only caller today is this crate's tests, because `AttachClassicLinkVpc` is still
    /// a scaffold: this is the seam that operation must write through, not dead code.
    pub fn link_classic_link_vpc(
        &mut self,
        instance_id: &str,
        vpc_id: &str,
        group_ids: Vec<String>,
    ) -> bool {
        if !self.instances.contains_key(instance_id) || !self.vpcs.contains_key(vpc_id) {
            return false;
        }
        if group_ids.len() > MAX_SECURITY_GROUPS_PER_INSTANCE {
            return false;
        }
        let record = ClassicLinkRecord {
            vpc_id: vpc_id.to_string(),
            group_ids,
        };
        let charge = classic_link_payload_bytes(&record);
        let refund = self
            .classic_link_instances
            .get(instance_id)
            .map_or(0, classic_link_payload_bytes);
        if self.would_exceed_stored_bytes_after_refund(charge, refund) {
            return false;
        }
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        self.classic_link_instances
            .insert(instance_id.to_string(), record);
        true
    }

    /// The modelled `ClassicLinkInstance` for `instance_id`, or `None` when that instance
    /// holds no ClassicLink link.
    ///
    /// The one projection of a link, so every operation that reports one reports it
    /// identically. Two of its members are read off the resources the link names rather
    /// than off the link: "Any tags assigned to the instance"
    /// (`ClassicLinkInstance$Tags`) comes from the instance record, so a tag written after
    /// the link is reported, and each group's "name of the security group"
    /// (`GroupIdentifier$GroupName`) comes from the security group store — absent for a
    /// group this account no longer holds, which is what AWS reports for a member a
    /// resource carries no value for.
    pub fn classic_link_instance_view(
        &self,
        instance_id: &str,
    ) -> Option<crate::ops::types::ClassicLinkInstance> {
        let record = self.classic_link_instances.get(instance_id)?;
        let instance = self.instances.get(instance_id)?;
        Some(crate::ops::types::ClassicLinkInstance {
            instance_id: Some(instance.instance_id.clone()),
            vpc_id: Some(record.vpc_id.clone()),
            groups: Some(
                record
                    .group_ids
                    .iter()
                    .map(|group_id| GroupIdentifier {
                        group_id: Some(group_id.clone()),
                        group_name: self
                            .security_groups
                            .get(group_id)
                            .and_then(|group| group.group_name.clone()),
                    })
                    .collect(),
            ),
            tags: Some(tag_list(&instance.tags)),
        })
    }

    /// Set whether the VPC `vpc_id` is enabled for ClassicLink, and answer whether the store
    /// took the change.
    ///
    /// `false` means NOTHING was written and the caller must refuse the request rather than
    /// report a status it did not set: the account holds no such VPC, and the store never
    /// holds the ClassicLink status of a VPC that is not there, so a describe cannot report
    /// one.
    ///
    /// The ONE seam a VPC's ClassicLink enablement is written through, so the operations that
    /// set it cannot disagree about what the flag means: `DisableVpcClassicLink` writes
    /// `false` through it, and `EnableVpcClassicLink` — still a scaffold, as
    /// `AttachClassicLinkVpc` is — must write `true` through it rather than touching the
    /// store itself. Idempotent: setting the value a VPC already holds leaves the store
    /// exactly as it was, which is why disabling a VPC that was never enabled succeeds
    /// without changing anything.
    pub fn set_vpc_classic_link_enabled(&mut self, vpc_id: &str, enabled: bool) -> bool {
        if !self.vpcs.contains_key(vpc_id) {
            return false;
        }
        if enabled {
            self.classic_link_enabled_vpcs.insert(vpc_id.to_string());
        } else {
            self.classic_link_enabled_vpcs.remove(vpc_id);
        }
        true
    }

    /// Is the VPC `vpc_id` enabled for ClassicLink?
    ///
    /// The one reading of [`Self::classic_link_enabled_vpcs`]: a VPC the set does not hold is
    /// not enabled, so a VPC nobody has enabled and one that has been disabled again answer
    /// the same — which is what AWS reports for both.
    pub fn vpc_is_classic_link_enabled(&self, vpc_id: &str) -> bool {
        self.classic_link_enabled_vpcs.contains(vpc_id)
    }

    /// Does the VPC `vpc_id` have any EC2-Classic instance linked to it?
    ///
    /// The question "You cannot disable ClassicLink for a VPC that has EC2-Classic instances
    /// linked to it" (`DisableVpcClassicLink`) asks of the link store, answered off the links
    /// themselves so it cannot drift from what `DescribeClassicLinkInstances` reports.
    pub fn vpc_has_linked_classic_instances(&self, vpc_id: &str) -> bool {
        self.classic_link_instances
            .values()
            .any(|record| record.vpc_id == vpc_id)
    }

    /// The modelled `VpcClassicLink` for `vpc_id`, or `None` when the account holds no such
    /// VPC.
    ///
    /// The one projection of a VPC's ClassicLink status, so every operation that reports one
    /// reports it identically. "Any tags assigned to the VPC" (`VpcClassicLink$Tags`) is read
    /// off the VPC record rather than copied, so a tag `CreateTags` writes after the status
    /// was set is reported by the next describe.
    pub fn vpc_classic_link_view(
        &self,
        vpc_id: &str,
    ) -> Option<crate::ops::types::VpcClassicLink> {
        let vpc = self.vpcs.get(vpc_id)?;
        Some(crate::ops::types::VpcClassicLink {
            vpc_id: Some(vpc_id.to_string()),
            classic_link_enabled: Some(self.vpc_is_classic_link_enabled(vpc_id)),
            tags: Some(vpc.tags.clone().unwrap_or_default()),
        })
    }

    /// Set whether the VPC `vpc_id` resolves a linked EC2-Classic instance's DNS hostname to
    /// its private address, and answer whether the store took the change.
    ///
    /// `false` means NOTHING was written and the caller must refuse the request rather than
    /// report a status it did not set, exactly as [`Self::set_vpc_classic_link_enabled`]
    /// does: the account holds no such VPC, and the store never holds the DNS support status
    /// of a VPC that is not there, so a describe cannot report one.
    ///
    /// The ONE seam a VPC's ClassicLink DNS support is written through, so the operations
    /// that set it cannot disagree about what the flag means: `EnableVpcClassicLinkDnsSupport`
    /// must write `true` and `DisableVpcClassicLinkDnsSupport` `false` through it rather than
    /// touching the store itself. Both are still scaffolds, so its only caller today is this
    /// crate's tests — the seam those operations have to write through, not dead code, the
    /// same position [`Self::link_classic_link_vpc`] is in. Idempotent: setting the value a
    /// VPC already holds leaves the store exactly as it was.
    pub fn set_vpc_classic_link_dns_enabled(&mut self, vpc_id: &str, enabled: bool) -> bool {
        if !self.vpcs.contains_key(vpc_id) {
            return false;
        }
        if enabled {
            self.classic_link_dns_enabled_vpcs.insert(vpc_id.to_string());
        } else {
            self.classic_link_dns_enabled_vpcs.remove(vpc_id);
        }
        true
    }

    /// Does the VPC `vpc_id` have ClassicLink DNS support?
    ///
    /// The one reading of [`Self::classic_link_dns_enabled_vpcs`]: a VPC the set does not
    /// hold does not, so a VPC nobody has enabled it for and one it has been disabled for
    /// again answer the same — which is what AWS reports for both.
    pub fn vpc_has_classic_link_dns_support(&self, vpc_id: &str) -> bool {
        self.classic_link_dns_enabled_vpcs.contains(vpc_id)
    }

    /// The modelled `ClassicLinkDnsSupport` for `vpc_id`, or `None` when the account holds no
    /// such VPC.
    ///
    /// The one projection of a VPC's ClassicLink DNS support status, so every operation that
    /// reports one reports it identically — the DNS-support counterpart of
    /// [`Self::vpc_classic_link_view`]. The shape carries only the two members, so there is
    /// nothing to read off the VPC record beyond its existence.
    pub fn classic_link_dns_support_view(
        &self,
        vpc_id: &str,
    ) -> Option<crate::ops::types::ClassicLinkDnsSupport> {
        self.vpcs.get(vpc_id)?;
        Some(crate::ops::types::ClassicLinkDnsSupport {
            vpc_id: Some(vpc_id.to_string()),
            classic_link_dns_supported: Some(self.vpc_has_classic_link_dns_support(vpc_id)),
        })
    }
}

/// Refuse a `VpcId` that is no VPC id at all on the surfaces that word it this way: HTTP 400
/// [`INVALID_CARRIER_GATEWAY_VPC_ID_MALFORMED`] (`InvalidVpcId.Malformed`) with the sentence
/// `<value> is not a valid VPC id`, built from the value the REQUEST sent and bounded by
/// [`MAX_ECHOED_VPC_ID_CHARS`] the way every other echoed VPC id is.
///
/// The same wire code as [`invalid_vpc_id_malformed`] in a DIFFERENT wording, and a second
/// named constructor rather than a literal at a call site for the reason
/// [`crate::scaffolded::Scaffolded::instance_id_is_malformed`] is one: EC2's per-operation
/// validators do not word an unreadable id the same way, WHICH wording an operation answers
/// is a fact of that operation, and keeping both here is what stops a third call site from
/// inventing a third. `DescribeVpcClassicLinkDnsSupport` is the operation recorded answering
/// this one, for a `VpcIds` entry outside the `vpc-` id form.
pub fn vpc_id_is_not_valid<E>(vpc_id: &str) -> Scaffolded<E> {
    let echoed: String = vpc_id.chars().take(MAX_ECHOED_VPC_ID_CHARS).collect();
    Scaffolded::service_error(
        INVALID_CARRIER_GATEWAY_VPC_ID_MALFORMED,
        400,
        format!("{echoed} is not a valid VPC id"),
    )
}
