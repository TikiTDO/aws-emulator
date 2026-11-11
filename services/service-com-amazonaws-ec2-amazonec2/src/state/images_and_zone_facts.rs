// SPDX-License-Identifier: CC0-1.0

//! The image store, the images AWS publishes in a Region, and the Availability Zone facts a
//! Region has — the ones no EC2 operation creates, so they are derived rather than stored.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Verified Access instance fits inside the store's ceiling
    /// ([`MAX_VERIFIED_ACCESS_INSTANCES`]).
    ///
    /// The same contract as [`Self::has_verified_access_group_capacity`]: a create asks
    /// first and refuses with [`verified_access_instance_limit_exceeded`] when it answers
    /// `false`.
    pub fn has_verified_access_instance_capacity(&self) -> bool {
        self.verified_access_instances.len() < MAX_VERIFIED_ACCESS_INSTANCES
    }

    /// Store `instance` under a freshly minted `VerifiedAccessInstanceId`, write that id onto
    /// the record, charge the caller-chosen bytes it and `client_token` retain against the
    /// store's byte budget, and answer the id it was given.
    ///
    /// The one way an instance is stored, so two writers cannot report one instance
    /// differently: `CreateVerifiedAccessInstance` lands here, and a test seeds an instance
    /// through the same seam. The charge happens at the insert the way
    /// [`Self::insert_verified_access_group`] charges a group's, so no path can store those
    /// bytes uncharged, and it is the SAME figure
    /// [`verified_access_instance_payload_bytes`] gives the create's pre-check — so a create
    /// still asks [`Self::has_verified_access_instance_capacity`] and
    /// [`Self::would_exceed_stored_bytes`] first and cannot overshoot either ceiling.
    pub fn insert_verified_access_instance(
        &mut self,
        mut instance: crate::ops::types::VerifiedAccessInstance,
        client_token: Option<&str>,
    ) -> String {
        let instance_id = self.next_id(VERIFIED_ACCESS_INSTANCE_ID_PREFIX);
        instance.verified_access_instance_id = Some(instance_id.clone());
        self.add_stored_bytes(verified_access_instance_payload_bytes(
            &instance,
            client_token,
        ));
        if let Some(token) = client_token {
            self.verified_access_instance_client_tokens
                .insert(token.to_string(), instance_id.clone());
        }
        self.verified_access_instances
            .insert(instance_id.clone(), instance);
        instance_id
    }

    /// The modification the `ModifyVerifiedAccessInstance` `ClientToken` `token` named, or
    /// `None` when no modification has been made under it.
    pub fn verified_access_instance_modification_for_token(
        &self,
        token: &str,
    ) -> Option<&VerifiedAccessInstanceModification> {
        self.verified_access_instance_modification_client_tokens
            .get(token)
    }

    /// Apply `modification` to the instance it names as of `at`, record it under
    /// `client_token` when the request carried one, and answer the instance as it NOW
    /// stands — `None` when this store holds no instance under that id.
    ///
    /// The one way a stored instance is modified, so what the modify answers and what a
    /// later describe of the same instance reports cannot differ: the view it hands back is
    /// the stored record itself. A member the modification leaves `None` is one the request
    /// did not carry, so the stored value stays as it was; a member it carries replaces that
    /// value. The caller-chosen bytes are re-charged across the write — the record this
    /// replaces is refunded and the record it becomes is charged — so a caller cannot grow
    /// the store past [`MAX_STORED_BYTES`] by modifying one instance repeatedly; a handler
    /// asks [`Self::would_exceed_stored_bytes_after_refund`] about that net change first.
    pub fn modify_verified_access_instance(
        &mut self,
        modification: &VerifiedAccessInstanceModification,
        client_token: Option<&str>,
        at: SystemTime,
    ) -> Option<crate::ops::types::VerifiedAccessInstance> {
        let held = self
            .verified_access_instances
            .get(&modification.verified_access_instance_id)?;
        let refund = verified_access_instance_payload_bytes(held, None);
        let mut view = modification.applied_to(held);
        // "The last updated time" (`VerifiedAccessInstance$LastUpdatedTime`): the instant this
        // modification was applied to the instance.
        view.last_updated_time = Some(iso8601_utc(at));
        let charge = verified_access_instance_payload_bytes(&view, None);
        self.verified_access_instances.insert(
            modification.verified_access_instance_id.clone(),
            view.clone(),
        );
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        if let Some(token) = client_token {
            self.add_stored_bytes(verified_access_instance_modification_payload_bytes(
                modification,
                token,
            ));
            self.verified_access_instance_modification_client_tokens
                .insert(token.to_string(), modification.clone());
        }
        Some(view)
    }

    /// The logging configuration stored for the Verified Access instance `instance_id`, or
    /// `None` when nothing has set one — in which case that instance carries
    /// [`default_verified_access_logs`].
    ///
    /// The one way a stored configuration is READ, so the modify that sets one and a later
    /// describe of it cannot report one instance's logging differently. A read never stores
    /// the default: an instance nothing has configured still has no entry after it has been
    /// described.
    pub fn verified_access_instance_logging_configuration(
        &self,
        instance_id: &str,
    ) -> Option<&crate::ops::types::VerifiedAccessLogs> {
        self.verified_access_instance_logging_configurations
            .get(instance_id)
    }

    /// The update the `ModifyVerifiedAccessInstanceLoggingConfiguration` `ClientToken`
    /// `token` named, or `None` when no update has been applied under it.
    pub fn verified_access_instance_logging_update_for_token(
        &self,
        token: &str,
    ) -> Option<&VerifiedAccessInstanceLoggingConfigurationUpdate> {
        self.verified_access_instance_logging_client_tokens
            .get(token)
    }

    /// Apply `update` to the logging configuration of the instance it names, record it under
    /// `client_token` when the request carried one, and answer the configuration as it NOW
    /// stands — `None` when this store holds no instance under that id.
    ///
    /// The one way a logging configuration is WRITTEN, so what the modify answers and what a
    /// later describe of the same instance reports cannot differ. An option the update leaves
    /// `None` is one the request did not carry, so the stored value stays as it was
    /// ([`VerifiedAccessInstanceLoggingConfigurationUpdate::applied_to`]); an instance that
    /// had no configuration at all starts from [`default_verified_access_logs`]. The
    /// caller-chosen bytes are re-charged across the write — the configuration this replaces
    /// is refunded and the one it becomes is charged — so a caller cannot grow the store past
    /// [`MAX_STORED_BYTES`] by configuring one instance repeatedly; a handler asks
    /// [`Self::would_exceed_stored_bytes_after_refund`] about that net change first.
    pub fn set_verified_access_instance_logging_configuration(
        &mut self,
        update: &VerifiedAccessInstanceLoggingConfigurationUpdate,
        client_token: Option<&str>,
    ) -> Option<crate::ops::types::VerifiedAccessLogs> {
        if !self
            .verified_access_instances
            .contains_key(&update.verified_access_instance_id)
        {
            return None;
        }
        let held = self
            .verified_access_instance_logging_configurations
            .get(&update.verified_access_instance_id);
        let refund = held.map_or(0, |logs| verified_access_logs_payload_bytes(logs, None));
        let current = held.cloned().unwrap_or_else(default_verified_access_logs);
        let applied = update.applied_to(&current);
        let charge = verified_access_logs_payload_bytes(&applied, None);
        self.verified_access_instance_logging_configurations.insert(
            update.verified_access_instance_id.clone(),
            applied.clone(),
        );
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        if let Some(token) = client_token {
            self.add_stored_bytes(verified_access_instance_logging_update_payload_bytes(
                update, token,
            ));
            self.verified_access_instance_logging_client_tokens
                .insert(token.to_string(), update.clone());
        }
        Some(applied)
    }

    /// What still depends on the Verified Access instance `instance_id`, as the ids of the
    /// depending resources.
    ///
    /// A Verified Access group is created ON an instance and an endpoint reports the instance
    /// its group belongs to, so either one still stored is a reference the instance cannot
    /// leave behind: removing the instance under it would leave a stored record naming an
    /// instance the account no longer holds. A trust provider ATTACHED to the instance is not
    /// one of these — it is a resource of its own that outlives the attachment, so the
    /// attachment simply ends with the instance.
    pub fn verified_access_instance_dependencies(&self, instance_id: &str) -> Vec<String> {
        let mut found: Vec<String> = self
            .verified_access_groups
            .iter()
            .filter(|(_, record)| {
                record.view.verified_access_instance_id.as_deref() == Some(instance_id)
            })
            .map(|(group_id, _)| group_id.clone())
            .collect();
        found.extend(
            self.verified_access_endpoints
                .iter()
                .filter(|(_, record)| {
                    record.view.verified_access_instance_id.as_deref() == Some(instance_id)
                })
                .map(|(endpoint_id, _)| endpoint_id.clone()),
        );
        found.sort();
        found
    }

    /// Remove the Verified Access instance `instance_id` and answer the record it was held
    /// as, or `None` when the store holds no such instance.
    ///
    /// The one way an instance leaves the store, so the delete's answer and what a later read
    /// of the account reports cannot disagree. The `CreateVerifiedAccessInstance` token that
    /// minted it goes with it — a token must never answer with an instance that is no longer
    /// stored — the logging configuration kept BESIDE the instance goes with it too, since
    /// that configuration is a fact of the instance and nothing else names it, and the bytes
    /// both held go back to the store's budget on the terms
    /// [`verified_access_instance_payload_bytes`] and [`verified_access_logs_payload_bytes`]
    /// took them on.
    ///
    /// `client_token` is the DELETE's own token: when the caller gave one, the removed record
    /// is kept under it ([`Self::verified_access_instance_deletions`]) so a retry of that
    /// delete answers what the first one answered, and the bytes it keeps are charged again
    /// under that token. That history is bounded by
    /// [`MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS`] entries, the oldest dropped (and
    /// its bytes given back) to make room; a delete that names no token frees the record
    /// outright.
    pub fn remove_verified_access_instance(
        &mut self,
        instance_id: &str,
        client_token: Option<&str>,
    ) -> Option<crate::ops::types::VerifiedAccessInstance> {
        let instance = self.verified_access_instances.remove(instance_id)?;
        let minting_token = self
            .verified_access_instance_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == instance_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &minting_token {
            self.verified_access_instance_client_tokens.remove(token);
        }
        self.release_stored_bytes(verified_access_instance_payload_bytes(
            &instance,
            minting_token.as_deref(),
        ));
        if let Some(logs) = self
            .verified_access_instance_logging_configurations
            .remove(instance_id)
        {
            self.release_stored_bytes(verified_access_logs_payload_bytes(&logs, None));
        }
        if let Some(token) = client_token {
            // One token names ONE delete, so a token used again replaces what it named
            // rather than leaving the history two entries to answer with.
            if let Some(position) = self
                .verified_access_instance_deletions
                .iter()
                .position(|(remembered, _)| remembered == token)
            {
                self.forget_verified_access_instance_deletion(position);
            }
            self.add_stored_bytes(verified_access_instance_payload_bytes(
                &instance,
                Some(token),
            ));
            self.verified_access_instance_deletions
                .push_back((token.to_string(), instance.clone()));
            while self.verified_access_instance_deletions.len()
                > MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS
            {
                self.forget_verified_access_instance_deletion(0);
            }
        }
        Some(instance)
    }

    /// The instance the delete that carried `client_token` removed, while this store still
    /// remembers that delete.
    ///
    /// The history is bounded ([`MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS`]), so a
    /// token whose delete has been dropped answers `None` — the same answer as a token no
    /// delete of this store ever carried.
    pub fn verified_access_instance_deletion(
        &self,
        client_token: &str,
    ) -> Option<&crate::ops::types::VerifiedAccessInstance> {
        self.verified_access_instance_deletions
            .iter()
            .find(|(remembered, _)| remembered == client_token)
            .map(|(_, instance)| instance)
    }

    /// Drop the remembered deletion at `position`, giving the bytes it held back to the
    /// store's budget: the record leaves the history on the same terms it entered it on.
    pub(super) fn forget_verified_access_instance_deletion(&mut self, position: usize) {
        if let Some((token, instance)) = self.verified_access_instance_deletions.remove(position) {
            self.release_stored_bytes(verified_access_instance_payload_bytes(
                &instance,
                Some(&token),
            ));
        }
    }

    /// Whether another Verified Access group fits inside the store's ceiling
    /// ([`MAX_VERIFIED_ACCESS_GROUPS`]).
    ///
    /// The same contract as [`Self::has_verified_access_endpoint_capacity`]: a create asks
    /// first and refuses with [`verified_access_group_limit_exceeded`] when it answers
    /// `false`.
    pub fn has_verified_access_group_capacity(&self) -> bool {
        self.verified_access_groups.len() < MAX_VERIFIED_ACCESS_GROUPS
    }

    /// Store `record` under a freshly minted `VerifiedAccessGroupId`, derive the members the
    /// SERVICE owns on it — the id, the ARN that id and `region` build
    /// ([`verified_access_group_arn`]) and the owning account — charge the caller-chosen
    /// bytes it and `client_token` retain against the store's byte budget, and answer the id
    /// it was given.
    ///
    /// The one way a group is stored, so two writers cannot report one group differently:
    /// `CreateVerifiedAccessGroup` lands here, and a test seeds a group through the same
    /// seam. The charge happens at the insert the way
    /// [`Self::insert_verified_access_endpoint`] charges an endpoint's, so no path can store
    /// those bytes uncharged, and it is the SAME figure
    /// [`verified_access_group_payload_bytes`] gives the create's pre-check — so a create
    /// still asks [`Self::has_verified_access_group_capacity`] and
    /// [`Self::would_exceed_stored_bytes`] first and cannot overshoot either ceiling.
    pub fn insert_verified_access_group(
        &mut self,
        mut record: VerifiedAccessGroupRecord,
        region: &str,
        client_token: Option<&str>,
    ) -> String {
        let group_id = self.next_id(VERIFIED_ACCESS_GROUP_ID_PREFIX);
        record.view.verified_access_group_id = Some(group_id.clone());
        record.view.verified_access_group_arn = Some(verified_access_group_arn(region, &group_id));
        record.view.owner = Some(::engine::emulator_account_id().to_string());
        self.add_stored_bytes(verified_access_group_payload_bytes(&record, client_token));
        if let Some(token) = client_token {
            self.verified_access_group_client_tokens
                .insert(token.to_string(), group_id.clone());
        }
        self.verified_access_groups.insert(group_id.clone(), record);
        group_id
    }

    /// Remove the Verified Access group `group_id` and answer it as this call leaves it —
    /// "The deletion time" (`VerifiedAccessGroup$DeletionTime`) set to `at` — or `None` when
    /// the store holds no such group.
    ///
    /// The one way a group leaves the store, so the delete's answer and what a later read of
    /// the account reports cannot disagree. The `CreateVerifiedAccessGroup` token that minted
    /// it goes with it — a token must never answer with a group that is no longer stored —
    /// and the bytes the record held go back to the store's budget on the terms
    /// [`verified_access_group_payload_bytes`] took them on.
    ///
    /// `client_token` is the DELETE's own token: when the caller gave one, the removed record
    /// is kept under it ([`Self::verified_access_group_deletions`]) so a retry of that delete
    /// answers what the first one answered, and the bytes it keeps are charged again under
    /// that token. That history is bounded by
    /// [`MAX_REMEMBERED_VERIFIED_ACCESS_GROUP_DELETIONS`] entries, the oldest dropped (and
    /// its bytes given back) to make room; a delete that names no token frees the record
    /// outright.
    pub fn remove_verified_access_group(
        &mut self,
        group_id: &str,
        client_token: Option<&str>,
        at: SystemTime,
    ) -> Option<crate::ops::types::VerifiedAccessGroup> {
        let mut record = self.verified_access_groups.remove(group_id)?;
        let minting_token = self
            .verified_access_group_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == group_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &minting_token {
            self.verified_access_group_client_tokens.remove(token);
        }
        self.release_stored_bytes(verified_access_group_payload_bytes(
            &record,
            minting_token.as_deref(),
        ));
        // The group's own record is what carries the deletion instant, so every report of
        // this delete — the answer and a retry's answer — names the same one.
        record.view.deletion_time = Some(iso8601_utc(at));
        // A report of the STORED group reports the record, in which "A description for the
        // Amazon Web Services Verified Access group" (`VerifiedAccessGroup$Description`) a
        // caller never set is empty text rather than an absent member — the create is what
        // echoes the request it was given, and this is a report of what was held.
        if record.view.description.is_none() {
            record.view.description = Some(String::new());
        }
        if let Some(token) = client_token {
            // One token names ONE delete, so a token used again replaces what it named
            // rather than leaving the history two entries to answer with.
            if let Some(position) = self
                .verified_access_group_deletions
                .iter()
                .position(|(remembered, _)| remembered == token)
            {
                self.forget_verified_access_group_deletion(position);
            }
            self.add_stored_bytes(verified_access_group_payload_bytes(&record, Some(token)));
            self.verified_access_group_deletions
                .push_back((token.to_string(), record.clone()));
            while self.verified_access_group_deletions.len()
                > MAX_REMEMBERED_VERIFIED_ACCESS_GROUP_DELETIONS
            {
                self.forget_verified_access_group_deletion(0);
            }
        }
        Some(record.view)
    }

    /// The group the delete that carried `client_token` removed, while this store still
    /// remembers that delete.
    ///
    /// The history is bounded ([`MAX_REMEMBERED_VERIFIED_ACCESS_GROUP_DELETIONS`]), so a
    /// token whose delete has been dropped answers `None` — the same answer as a token no
    /// delete of this store ever carried.
    pub fn verified_access_group_deletion(
        &self,
        client_token: &str,
    ) -> Option<&crate::ops::types::VerifiedAccessGroup> {
        self.verified_access_group_deletions
            .iter()
            .find(|(remembered, _)| remembered == client_token)
            .map(|(_, record)| &record.view)
    }

    /// Drop the remembered deletion at `position`, giving the bytes it held back to the
    /// store's budget: the record leaves the history on the same terms it entered it on.
    pub(super) fn forget_verified_access_group_deletion(&mut self, position: usize) {
        if let Some((token, record)) = self.verified_access_group_deletions.remove(position) {
            self.release_stored_bytes(verified_access_group_payload_bytes(&record, Some(&token)));
        }
    }

    /// Whether another Verified Access endpoint fits inside the store's ceiling
    /// ([`MAX_VERIFIED_ACCESS_ENDPOINTS`]).
    ///
    /// The same contract as [`Self::has_verified_access_trust_provider_capacity`]: a create
    /// asks first and refuses with [`verified_access_endpoint_limit_exceeded`] when it
    /// answers `false`.
    pub fn has_verified_access_endpoint_capacity(&self) -> bool {
        self.verified_access_endpoints.len() < MAX_VERIFIED_ACCESS_ENDPOINTS
    }

    /// Store `record` under a freshly minted `VerifiedAccessEndpointId`, write that id and
    /// the endpoint domain it names onto the record, charge the caller-chosen bytes it and
    /// `client_token` retain against the store's byte budget, and answer the id.
    ///
    /// The id is minted HERE rather than by the writer, so an endpoint never shares an id
    /// with another resource of this store, and the generated endpoint domain is built from
    /// that id here too ([`verified_access_endpoint_domain`]) so the two cannot disagree —
    /// `domain_prefix` is the request's `EndpointDomainPrefix` and `region` the Region it was
    /// signed for. The charge happens at the insert the way
    /// [`Self::insert_verified_access_trust_provider`] charges a trust provider's, so no path
    /// can store those bytes uncharged, and it is the SAME figure
    /// [`verified_access_endpoint_payload_bytes`] gives the create's pre-check — the prefix is
    /// charged, the derived rest of the domain is bounded — so a create still asks
    /// [`Self::has_verified_access_endpoint_capacity`] and
    /// [`Self::would_exceed_stored_bytes`] first and cannot overshoot either ceiling.
    pub fn insert_verified_access_endpoint(
        &mut self,
        mut record: VerifiedAccessEndpointRecord,
        domain_prefix: Option<&str>,
        region: &str,
        client_token: Option<&str>,
    ) -> String {
        let endpoint_id = self.next_id(VERIFIED_ACCESS_ENDPOINT_ID_PREFIX);
        record.view.verified_access_endpoint_id = Some(endpoint_id.clone());
        record.view.endpoint_domain = Some(verified_access_endpoint_domain(
            domain_prefix,
            &endpoint_id,
            region,
        ));
        self.add_stored_bytes(verified_access_endpoint_payload_bytes(
            &record,
            domain_prefix,
            client_token,
        ));
        if let Some(token) = client_token {
            self.verified_access_endpoint_client_tokens
                .insert(token.to_string(), endpoint_id.clone());
        }
        self.verified_access_endpoints
            .insert(endpoint_id.clone(), record);
        endpoint_id
    }
}

// ---------------------------------------------------------------------------
// Modelled values and bounds the VPC create path uses
// ---------------------------------------------------------------------------

/// A VPC AWS is still creating: the `VpcState` enum's `pending` member, which is what
/// `CreateVpc` answers with until [`TRANSITION_DELAY`] has passed.
pub const VPC_STATE_PENDING: &str = "pending";

/// An IPv6 CIDR block still being associated with its VPC: the `VpcCidrBlockStateCode`
/// enum's `associating` member.
pub const CIDR_BLOCK_STATE_ASSOCIATING: &str = "associating";

/// An IPv6 CIDR block on its way OUT of the subnet or VPC that holds it: the
/// `SubnetCidrBlockStateCode` / `VpcCidrBlockStateCode` enums' `disassociating` member,
/// which both spell the same way
/// (`smithy_query(query="shape",shape="SubnetCidrBlockStateCode,VpcCidrBlockStateCode")`)
/// — what `DisassociateSubnetCidrBlock` answers, and a state in which the block is no
/// longer held ([`cidr_association_holds_its_block`]).
pub const CIDR_BLOCK_STATE_DISASSOCIATING: &str = "disassociating";

/// An IPv6 CIDR block the subnet or VPC that held it has let go of: the
/// `SubnetCidrBlockStateCode` / `VpcCidrBlockStateCode` enums' `disassociated` member, the
/// terminal state of a disassociation and the one the clock owes a `disassociating`
/// association ([`ComAmazonawsEc2Amazonec2State::subnet_ipv6_disassociated_at`]).
pub const CIDR_BLOCK_STATE_DISASSOCIATED: &str = "disassociated";

/// The tenancy a VPC may be created with beside [`DEFAULT_INSTANCE_TENANCY`]: the
/// `Tenancy` enum's `dedicated` member. Its third member, `host`, is excluded by
/// `CreateVpcRequest$InstanceTenancy`'s own documentation.
pub const TENANCY_DEDICATED: &str = "dedicated";

/// The `ResourceType` a `CreateVpc` tag specification may name: the VPC it creates.
pub const RESOURCE_TYPE_VPC: &str = "vpc";

// ---------------------------------------------------------------------------
// The attributes of a VPC (`VpcAttributeName`)
// ---------------------------------------------------------------------------

/// The `VpcAttributeName` enum's `enableDnsHostnames` member.
///
/// Named here so the handlers and [`VpcAttributes`] spell one attribute one way; the
/// generated catalog of every member is `crate::ops::types::VPC_ATTRIBUTE_NAME_VALUES`,
/// which `DescribeVpcAttribute` validates its `Attribute` against and the crate's tests
/// pin these three constants to.
pub const VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES: &str = "enableDnsHostnames";

/// The `VpcAttributeName` enum's `enableDnsSupport` member (see
/// [`VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES`] for the generated catalog these come from).
pub const VPC_ATTRIBUTE_ENABLE_DNS_SUPPORT: &str = "enableDnsSupport";

/// The `VpcAttributeName` enum's `enableNetworkAddressUsageMetrics` member (see
/// [`VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES`] for the generated catalog these come from).
pub const VPC_ATTRIBUTE_ENABLE_NETWORK_ADDRESS_USAGE_METRICS: &str =
    "enableNetworkAddressUsageMetrics";

/// The attributes of one VPC: one field per `VpcAttributeName` member, which is exactly
/// the set `DescribeVpcAttribute` reports and `ModifyVpcAttribute` sets.
///
/// Held in [`ComAmazonawsEc2Amazonec2State::vpc_attributes`] rather than on the modelled
/// [`Vpc`] shape, which carries no member for any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpcAttributes {
    /// `enableDnsSupport`: "whether the DNS resolution is supported for the VPC"
    /// (`ModifyVpcAttributeRequest$EnableDnsSupport`).
    pub enable_dns_support: bool,
    /// `enableDnsHostnames`: "whether the instances launched in the VPC get DNS
    /// hostnames" (`ModifyVpcAttributeRequest$EnableDnsHostnames`).
    pub enable_dns_hostnames: bool,
    /// `enableNetworkAddressUsageMetrics`: "whether Network Address Usage metrics are
    /// enabled for your VPC"
    /// (`ModifyVpcAttributeRequest$EnableNetworkAddressUsageMetrics`).
    pub enable_network_address_usage_metrics: bool,
}

impl VpcAttributes {
    /// The attributes a VPC is created with.
    ///
    /// Amazon VPC User Guide, "DNS attributes in your VPC": `enableDnsSupport` is `true`
    /// for a default VPC and for a VPC you create, while `enableDnsHostnames` is `true`
    /// for a default VPC and `false` for a VPC you create. Network Address Usage metrics
    /// are a VPC feature a caller turns on, so they start off.
    pub fn for_vpc(is_default: bool) -> Self {
        Self {
            enable_dns_support: true,
            enable_dns_hostnames: is_default,
            enable_network_address_usage_metrics: false,
        }
    }

    /// The value of the attribute `attribute` names, or `None` when that is not a
    /// `VpcAttributeName` member.
    pub fn value_of(&self, attribute: &str) -> Option<bool> {
        match attribute {
            VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES => Some(self.enable_dns_hostnames),
            VPC_ATTRIBUTE_ENABLE_DNS_SUPPORT => Some(self.enable_dns_support),
            VPC_ATTRIBUTE_ENABLE_NETWORK_ADDRESS_USAGE_METRICS => {
                Some(self.enable_network_address_usage_metrics)
            }
            _ => None,
        }
    }
}

/// `Ipv6AddressAttribute` of a block advertised on the internet from AWS.
pub const IPV6_ADDRESS_PUBLIC: &str = "public";

/// `IpSource` of an IPv6 block Amazon allocated.
pub const IP_SOURCE_AMAZON: &str = "amazon";

/// `IpSource` of an IPv6 block the customer brought to AWS.
pub const IP_SOURCE_BYOIP: &str = "byoip";

/// The largest number of VPCs this in-memory store holds.
///
/// A capacity bound, not an AWS quota (AWS's VPCs-per-Region quota is adjustable): the
/// store lives in this process's memory, so the number of VPCs a caller can create has to
/// be finite. `CreateVpc` answers [`VPC_LIMIT_EXCEEDED`] rather than growing without
/// limit.
pub const MAX_VPCS: usize = 1_000;

/// EC2's error code for a `CreateVpc` that would take the account past its VPC quota.
pub const VPC_LIMIT_EXCEEDED: &str = "VpcLimitExceeded";

/// The largest number of subnets this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`] and [`MAX_STORED_INSTANCES`], not an
/// AWS quota: each subnet is a record in this process's memory, and a default VPC alone
/// seeds one per Availability Zone, so the total has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_subnet_capacity`] is the predicate a writer asks
/// before inserting; a create past it refuses with [`SUBNET_LIMIT_EXCEEDED`].
pub const MAX_SUBNETS: usize = 4_096;

/// EC2's error code for a create that would take the account past its subnet quota —
/// "You've reached the limit on the number of subnets that you can create" (EC2 API error
/// codes).
pub const SUBNET_LIMIT_EXCEEDED: &str = "SubnetLimitExceeded";

/// The largest number of IPv6 CIDR block association records this store keeps for ONE
/// subnet.
///
/// A capacity bound in the style of [`MAX_SUBNETS`], not an AWS quota. "You can only
/// associate a single IPv6 CIDR block with your subnet" (`AssociateSubnetCidrBlock`), so a
/// subnet HOLDS one block at a time and this ceiling is never the rule a well-formed
/// request meets; it bounds the association RECORDS one subnet's set can grow to — a block
/// it still holds, plus any a later disassociation leaves behind to be reported — so a
/// caller looping over the associate cannot grow one subnet's record without bound even if
/// a writer forgets to ask.
/// [`ComAmazonawsEc2Amazonec2State::has_subnet_ipv6_association_capacity`] is the predicate
/// a writer asks before associating.
pub const MAX_IPV6_ASSOCIATIONS_PER_SUBNET: usize = 4;

/// The largest number of DHCP option sets this in-memory store holds.
///
/// A capacity bound in the style of [`MAX_VPCS`], not an AWS quota: this store mints one
/// default option set per REGION
/// ([`ComAmazonawsEc2Amazonec2State::default_dhcp_options_id`]), and the Region comes from
/// the request's SigV4 credential scope, which is caller-supplied text — so without a
/// ceiling a caller could mint a record per Region string it invents. It sits well above
/// the number of Regions any AWS partition has ([`REGIONS_ENABLED_BY_DEFAULT`] lists 17 of
/// the commercial one), so no client signing for real Regions reaches it.
/// [`ComAmazonawsEc2Amazonec2State::has_dhcp_option_set_capacity`] is the predicate a
/// writer asks before creating a VPC; a create past it refuses with
/// [`DHCP_OPTIONS_LIMIT_EXCEEDED`].
pub const MAX_DHCP_OPTION_SETS: usize = 64;

/// EC2's error code for a create that would take the account past its DHCP option set
/// quota (`DhcpOptionsLimitExceeded`, EC2 API error codes).
pub const DHCP_OPTIONS_LIMIT_EXCEEDED: &str = "DhcpOptionsLimitExceeded";

// ---------------------------------------------------------------------------
// CIDR blocks
// ---------------------------------------------------------------------------

/// The smallest IPv4 prefix length a VPC CIDR block may have.
///
/// Amazon VPC User Guide, VPC CIDR blocks: "The allowed block size is between a /16
/// netmask (65,536 IP addresses) and /28 netmask (16 IP addresses)."
pub const MIN_VPC_IPV4_PREFIX: u8 = 16;

/// The largest IPv4 prefix length a VPC CIDR block may have (see
/// [`MIN_VPC_IPV4_PREFIX`]).
pub const MAX_VPC_IPV4_PREFIX: u8 = 28;

/// Why a requested CIDR block cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CidrProblem {
    /// The value is not a CIDR block of the expected address family at all.
    Malformed,
    /// The value parses, but its prefix length is outside the allowed block size.
    PrefixLength,
}

/// EC2's error code for a VPC CIDR block whose SIZE is outside the one the service permits:
/// "InvalidVpc.Range — The specified CIDR block range is not valid. The block range must be
/// between a /16 netmask and /28 netmask" (EC2 API error codes).
///
/// A block size the resource does not permit is a RANGE violation, which is a different
/// answer from `InvalidParameterValue` for a value that is not a CIDR block at all: a
/// client that reads `InvalidVpc.Range` knows to retry with a smaller block, where a
/// parameter-value refusal says the value could not be read. [`INVALID_SUBNET_RANGE`] is
/// the same code for a subnet's block.
pub const INVALID_VPC_RANGE: &str = "InvalidVpc.Range";

/// Refuse a VPC CIDR block whose size is outside the published VPC block size: HTTP 400
/// [`INVALID_VPC_RANGE`].
///
/// The message is the fixed sentence this class carries plus the block the REQUEST asked
/// for: the published block size the value misses is what the error CODE stands for, and
/// restating it in the wording is detail the service does not send.
pub fn invalid_vpc_range<E>(cidr_block: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_VPC_RANGE,
        400,
        format!("The CIDR '{cidr_block}' is invalid."),
    )
}

/// A requested IPv4 VPC CIDR block in the canonical form AWS stores it in.
///
/// `CreateVpcRequest$CidrBlock`: "We modify the specified CIDR block to its canonical
/// form; for example, if you specify 100.68.0.18/18, we modify it to 100.68.0.0/18" — so
/// the host bits are cleared rather than the value being refused. The prefix length is
/// held to the published block size ([`MIN_VPC_IPV4_PREFIX`]..=[`MAX_VPC_IPV4_PREFIX`]).
///
/// The block itself is read by [`canonical_ipv4_cidr`], which the subnet path
/// ([`canonical_subnet_ipv4_cidr`]) reads its own blocks with too, so this crate keeps ONE
/// rule for what an IPv4 CIDR block is and only the published block size differs between
/// the two. That reader in turn parses the address with the standard library's `Ipv4Addr`,
/// the same parser [`parse_ipv4_cidr_block`] uses; the only difference between those two
/// is that this path CLEARS the host bits where that one requires them already clear.
pub fn canonical_vpc_ipv4_cidr(value: &str) -> Result<String, CidrProblem> {
    let canonical = canonical_ipv4_cidr(value)?;
    let (_, prefix_length) = split_ipv4_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if !(MIN_VPC_IPV4_PREFIX..=MAX_VPC_IPV4_PREFIX).contains(&prefix_length) {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// A requested IPv6 CIDR block in canonical form: the host bits cleared, the address
/// rendered the one way `Ipv6Addr` renders it.
///
/// Deliberately NO prefix-length rule, unlike the IPv4 path: the model states a size only
/// for the block AWS itself picks ("an Amazon-provided IPv6 CIDR block with a /56 prefix
/// length"), which this service mints at that size, while the size a caller may bring from
/// its own pool is a property of that pool and is documented nowhere the model or the
/// request carries. A bound invented here would refuse blocks AWS accepts.
pub fn canonical_ipv6_cidr(value: &str) -> Result<String, CidrProblem> {
    let (address, prefix_length) = value.split_once('/').ok_or(CidrProblem::Malformed)?;
    let base = u128::from(
        address
            .parse::<Ipv6Addr>()
            .map_err(|_| CidrProblem::Malformed)?,
    );
    let prefix_length: u8 = prefix_length.parse().map_err(|_| CidrProblem::Malformed)?;
    if prefix_length > 128 {
        return Err(CidrProblem::Malformed);
    }
    let mask = u128::MAX
        .checked_shl(u32::from(128 - prefix_length))
        .unwrap_or_default();
    Ok(format!("{}/{prefix_length}", Ipv6Addr::from(base & mask)))
}

// ---------------------------------------------------------------------------
// The rest of the EC2 error channel (the codes above live with the describes)
// ---------------------------------------------------------------------------

/// Refuse a request that leaves out a member the action requires, the way EC2 does: HTTP
/// 400 `MissingParameter`.
pub fn missing_parameter<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(MISSING_PARAMETER, 400, message)
}

/// Refuse members that are each valid but cannot be used together, the way EC2 does: HTTP
/// 400 `InvalidParameterCombination`.
pub fn invalid_parameter_combination<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INVALID_PARAMETER_COMBINATION, 400, message)
}

/// Refuse two parameters that cannot be given in one request: HTTP 400
/// `InvalidParameterCombination`, worded with the ONE sentence EC2 refuses such a pair with
/// — the fixed `The parameter … cannot be used with the parameter …` with the two
/// parameters' own names in it.
///
/// The pair every paged describe of this crate reads it for is a page size beside an
/// explicit selection: a request that NAMES the resources it wants is not walking a
/// collection, so `MaxResults` and a resource-id list select mutually exclusively and EC2
/// refuses them together rather than serving one and ignoring the other. Each caller names
/// the two parameters as its OWN refusal spells them — EC2 words the same refusal with the
/// public member name on some operations and with the wire set name on others, and in either
/// order — so the template lives here and the names stay with the operation.
pub fn parameters_cannot_be_combined<E>(first: &str, second: &str) -> Scaffolded<E> {
    invalid_parameter_combination(format!(
        "The parameter {first} cannot be used with the parameter {second}"
    ))
}

/// Refuse a create that would take the store past [`MAX_VPCS`]: HTTP 400
/// `VpcLimitExceeded`.
pub fn vpc_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of VPCs has been reached ({MAX_VPCS})"),
    )
}

/// Refuse a create whose Region has no default DHCP option set yet and would take the
/// store past [`MAX_DHCP_OPTION_SETS`]: HTTP 400 [`DHCP_OPTIONS_LIMIT_EXCEEDED`].
pub fn dhcp_options_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        DHCP_OPTIONS_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of DHCP options sets has been reached \
             ({MAX_DHCP_OPTION_SETS})"
        ),
    )
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// [`service_state`], with the "mounted on an engine that holds no state" case
/// already turned into the server error every handler answers it with.
pub fn require_service_state<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
) -> Result<
    ::engine::ServiceState<ComAmazonawsEc2Amazonec2State>,
    crate::scaffolded::Scaffolded<E>,
> {
    service_state(engine).ok_or_else(|| {
        crate::scaffolded::Scaffolded::internal_error(
            "the EC2 emulator is mounted on an engine that holds no state",
        )
    })
}

// ---------------------------------------------------------------------------
// AMIs
// ---------------------------------------------------------------------------

/// An AMI that "is successfully registered and can be used to launch an instance"
/// (`Image$State`): the `ImageState` enum's `available` member.
pub const IMAGE_STATE_AVAILABLE: &str = "available";

/// An AMI its owner has disabled: the `ImageState` enum's `disabled` member. Such an
/// image is left out of a report unless the request asks for it (`IncludeDisabled`).
pub const IMAGE_STATE_DISABLED: &str = "disabled";

/// "The type of image" (`Image$ImageType`) for an AMI, as opposed to a kernel or a RAM
/// disk image: the `ImageTypeValues` enum's `machine` member.
pub const IMAGE_TYPE_MACHINE: &str = "machine";

/// "The hypervisor type of the image. Only `xen` is supported" (`Image$Hypervisor`).
pub const IMAGE_HYPERVISOR_XEN: &str = "xen";

/// The virtualization type of every image published today: the `VirtualizationType`
/// enum's `hvm` member (`paravirtual` is the retired alternative).
pub const IMAGE_VIRTUALIZATION_TYPE_HVM: &str = "hvm";

/// The `VirtualizationType` enum's `paravirtual` member: the virtualization of the oldest
/// published images, whose own names carry it (`…-pv-…`).
pub const IMAGE_VIRTUALIZATION_TYPE_PARAVIRTUAL: &str = "paravirtual";

/// An AMI whose root volume is an Amazon EBS volume: the `DeviceType` enum's `ebs`
/// member (`Image$RootDeviceType`).
pub const IMAGE_ROOT_DEVICE_TYPE_EBS: &str = "ebs";

/// An AMI whose root device is the instance store: the `DeviceType` enum's
/// `instance-store` member. Such an image restores no snapshot, so it has no Amazon EBS
/// root block device mapping at all.
pub const IMAGE_ROOT_DEVICE_TYPE_INSTANCE_STORE: &str = "instance-store";

/// The root device name of an instance-store-backed image, which is where the published
/// images whose names end `-s3` carry their root file system.
pub const INSTANCE_STORE_ROOT_DEVICE_NAME: &str = "/dev/sda1";

/// "A value of `simple` indicates that enhanced networking with the Intel 82599 VF
/// interface is enabled" (the `sriov-net-support` filter of `DescribeImages`).
pub const IMAGE_SRIOV_NET_SUPPORT_SIMPLE: &str = "simple";

/// The EBS volume type an image's root snapshot is restored into unless its spec names
/// another: the `VolumeType` enum's `gp3` member.
pub const IMAGE_ROOT_VOLUME_TYPE: &str = "gp3";

/// The EBS volume type of the older published images whose own names carry it (`…-gp2`):
/// the `VolumeType` enum's `gp2` member.
pub const IMAGE_ROOT_VOLUME_TYPE_GP2: &str = "gp2";

/// The magnetic EBS volume type the published names that predate `gp2` carry (`…-ebs`):
/// the `VolumeType` enum's `standard` member.
pub const IMAGE_ROOT_VOLUME_TYPE_STANDARD: &str = "standard";

/// "Indicates whether deregistration protection is enabled for the AMI"
/// (`Image$DeregistrationProtection`) — off, nothing having enabled it.
pub const IMAGE_DEREGISTRATION_PROTECTION_DISABLED: &str = "disabled";

/// The one resource type an AMI create's `TagSpecifications` may name: "to tag the AMI, the
/// value for `ResourceType` must be `image`. If you specify another value for
/// `ResourceType`, the request fails" (`RegisterImageRequest$TagSpecifications`) — the
/// `image` member of `ResourceType`, and the first of the two
/// [`IMAGE_TAGGABLE_RESOURCES`] a restore of an AMI from Amazon S3 may name.
pub const IMAGE_TAGGABLE_RESOURCE: &str = "image";

/// The architecture an AMI is registered with when the request names none: "Default: For
/// Amazon EBS-backed AMIs, `i386`" (`RegisterImageRequest$Architecture`), which is the
/// `ArchitectureValues` enum's `i386` member. The instance-store default is "the
/// architecture specified in the manifest file", a file nothing here reads, so the one
/// documented default stands for both.
pub const REGISTERED_IMAGE_DEFAULT_ARCHITECTURE: &str = "i386";

/// The virtualization type an AMI is registered with when the request names none:
/// "Default: `paravirtual`" (`RegisterImageRequest$VirtualizationType`), which is
/// [`IMAGE_VIRTUALIZATION_TYPE_PARAVIRTUAL`]. The published catalog's own default is
/// `hvm` ([`ImageSpec::default`]), because that is what Amazon publishes today; this is
/// what the REQUEST means by leaving the member out.
pub const REGISTERED_IMAGE_DEFAULT_VIRTUALIZATION_TYPE: &str =
    IMAGE_VIRTUALIZATION_TYPE_PARAVIRTUAL;

/// The code EC2 answers for an AMI name outside the published name constraints.
pub const INVALID_AMI_NAME_MALFORMED: &str = "InvalidAMIName.Malformed";

/// The code EC2 answers for an AMI name another AMI of the account already carries.
pub const INVALID_AMI_NAME_DUPLICATE: &str = "InvalidAMIName.Duplicate";

/// Is `name` a name an AMI may carry?
///
/// "Constraints: 3-128 alphanumeric characters, parentheses (`()`), square brackets (`[]`),
/// spaces (` `), periods (`.`), slashes (`/`), dashes (`-`), single quotes (`'`), at-signs
/// (`@`), or underscores(`_`)" (`RegisterImageRequest$Name`). The LENGTH half of that
/// sentence is the member's own `@length` constraint, refused at the boundary before any
/// handler runs, so this reads the character set — the half no constraint expresses.
pub fn image_name_is_well_formed(name: &str) -> bool {
    name.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || matches!(
                character,
                '(' | ')' | '[' | ']' | ' ' | '.' | '/' | '-' | '\'' | '@' | '_'
            )
    })
}

/// Refuse an AMI name outside those constraints, the way EC2 does: HTTP 400
/// `InvalidAMIName.Malformed`.
///
/// The message is the service's own fixed sentence for this error class, which states the
/// bounds and the characters rather than echoing the name back.
pub fn invalid_ami_name_malformed<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_AMI_NAME_MALFORMED,
        400,
        "AMI names must be between 3 and 128 characters long, and may contain letters, \
         digits, '(', ')', '.', '-', '/' and '_'",
    )
}

/// Refuse an AMI name another AMI of the account already carries: HTTP 400
/// `InvalidAMIName.Duplicate`, naming the name at fault and the AMI that already has it.
///
/// "The name must be unique for AMIs in the Region for this account"
/// (`CreateRestoreImageTaskRequest$Name` states the rule the whole API holds a name to), so
/// both variable parts come from the request and from this store: the name the caller sent
/// and the id of the image [`ComAmazonawsEc2Amazonec2State::image_named`] resolved it to.
pub fn invalid_ami_name_duplicate<E>(name: &str, image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_AMI_NAME_DUPLICATE,
        400,
        format!("AMI name {name} is already in use by AMI {image_id}"),
    )
}

/// `Image$BootMode`'s `uefi-preferred`: the image boots UEFI where the instance type
/// supports it and legacy BIOS otherwise.
pub const BOOT_MODE_UEFI_PREFERRED: &str = "uefi-preferred";

/// `Image$BootMode`'s `uefi`.
pub const BOOT_MODE_UEFI: &str = "uefi";

/// `Image$BootMode`'s `legacy-bios`.
pub const BOOT_MODE_LEGACY_BIOS: &str = "legacy-bios";

/// "If `v2.0`, it indicates that IMDSv2 is specified in the AMI"
/// (`Image$ImdsSupport`).
pub const IMDS_SUPPORT_V2: &str = "v2.0";

/// "This value is set to `windows` for Windows AMIs; otherwise, it is blank"
/// (`Image$Platform`), whose modelled `PlatformValues` enum publishes the single value
/// `Windows` — so that is the value a stored image carries.
pub const IMAGE_PLATFORM_WINDOWS: &str = "Windows";

/// The `PlatformDetails` of a Linux image: "the platform details associated with the
/// billing code of the AMI" (`Image$PlatformDetails`).
pub const PLATFORM_DETAILS_LINUX: &str = "Linux/UNIX";

/// The `PlatformDetails` of a Windows image.
pub const PLATFORM_DETAILS_WINDOWS: &str = "Windows";

/// The `UsageOperation` billing code of a Linux image (`Image$UsageOperation`).
pub const USAGE_OPERATION_LINUX: &str = "RunInstances";

/// The `UsageOperation` billing code of a Windows image — Amazon's billing-code table
/// pairs `RunInstances:0002` with Windows ("Understand AMI billing information").
pub const USAGE_OPERATION_WINDOWS: &str = "RunInstances:0002";

/// The root device name of the published Linux images (`Image$RootDeviceName`).
pub const LINUX_ROOT_DEVICE_NAME: &str = "/dev/xvda";

/// The root device name of the published Windows images.
pub const WINDOWS_ROOT_DEVICE_NAME: &str = "/dev/sda1";

/// The root volume size, in GiB, of a published Linux image.
pub const LINUX_ROOT_VOLUME_SIZE: i32 = 8;

/// The root volume size, in GiB, of a published Windows image.
pub const WINDOWS_ROOT_VOLUME_SIZE: i32 = 30;

/// The owner alias Amazon's own published AMIs carry: "the owner alias (`amazon` |
/// `aws-backup-vault` | `aws-marketplace`)" (`Image$ImageOwnerAlias`).
pub const IMAGE_OWNER_ALIAS_AMAZON: &str = "amazon";

/// Every owner alias `DescribeImagesRequest$Owners` accepts beside an account id and
/// `self`: "you can specify a combination of Amazon Web Services account IDs, self,
/// amazon, aws-backup-vault, and aws-marketplace".
pub const IMAGE_OWNER_ALIASES: [&str; 3] = ["amazon", "aws-backup-vault", "aws-marketplace"];

/// The account id Amazon publishes its Amazon Linux AMIs from, which the published
/// catalog's images are owned by; `amazon` is the alias for it
/// (`IMAGE_OWNER_ALIAS_AMAZON`).
pub const AMAZON_IMAGE_OWNER_ID: &str = "137112412989";

/// "self (the sender of the request)" — the `Owners` and `ExecutableUsers` term that
/// names the calling account.
pub const OWNER_SELF: &str = "self";

/// "all (public AMIs)" — the `ExecutableUsers` term that names every account.
pub const EXECUTABLE_BY_ALL: &str = "all";

/// How many digits an Amazon Web Services account id has.
pub const AWS_ACCOUNT_ID_DIGITS: usize = 12;

/// How many characters of an ISO 8601 UTC timestamp carry the instant down to the
/// second: `YYYY-MM-DDThh:mm:ss`.
pub(super) const ISO8601_SECONDS_WIDTH: usize = 19;

/// How many images this store holds, so a writer cannot grow it without bound. A create
/// asks [`ComAmazonawsEc2Amazonec2State::has_image_capacity`] first, the way a VPC create
/// asks [`ComAmazonawsEc2Amazonec2State::has_vpc_capacity`].
pub const MAX_IMAGES: usize = 4_096;

/// The code EC2 answers for an AMI id the account can see no image for.
pub const INVALID_AMI_ID_NOT_FOUND: &str = "InvalidAMIID.NotFound";

/// The code EC2 answers for something that is not an AMI id at all.
pub const INVALID_AMI_ID_MALFORMED: &str = "InvalidAMIID.Malformed";

/// The code EC2 answers for an owner or executable-by term that is neither an account id
/// nor one of the aliases — "you must specify a valid value for the owner or executableBy
/// parameters, such as an Amazon Web Services account ID" (EC2 error codes,
/// `InvalidUserID.Malformed`).
pub const INVALID_USER_ID_MALFORMED: &str = "InvalidUserID.Malformed";

/// Is `id` shaped like an AMI id?
///
/// An image id is `ami-` followed by 8 or 17 lowercase hexadecimal digits — the short and
/// the long id form, the same two forms [`instance_id_is_well_formed`] accepts. EC2
/// answers `InvalidAMIID.Malformed` for anything else, BEFORE it looks the id up, which is
/// a different answer from `InvalidAMIID.NotFound` for a well-formed id no image has.
pub fn image_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed("ami", id)
}

/// Is `id` shaped like an Amazon Web Services account id — [`AWS_ACCOUNT_ID_DIGITS`]
/// decimal digits?
pub fn aws_account_id_is_well_formed(id: &str) -> bool {
    id.len() == AWS_ACCOUNT_ID_DIGITS && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// Does the ISO 8601 UTC timestamp `timestamp` name an instant before `reference`?
///
/// Both are `YYYY-MM-DDThh:mm:ss` with an optional fraction and the `Z` zone — a
/// fixed-width UTC rendering whose lexicographic order IS its chronological order — so the
/// comparison is made over the first [`ISO8601_SECONDS_WIDTH`] characters, which every
/// such rendering carries whether or not it also carries a fraction.
pub(super) fn iso8601_is_before(timestamp: &str, reference: &str) -> bool {
    fn to_the_second(value: &str) -> &str {
        value
            .char_indices()
            .nth(ISO8601_SECONDS_WIDTH)
            .map_or(value, |(index, _)| &value[..index])
    }
    to_the_second(timestamp) < to_the_second(reference)
}

/// Is `image` deprecated as of `now` — does it carry a `DeprecationTime` that has passed?
///
/// "The date and time to deprecate the AMI" (`Image$DeprecationTime`): an AMI with no such
/// time is never deprecated, and one whose time is still in the future is not deprecated
/// yet, so both are reported without `IncludeDeprecated`.
pub fn image_is_deprecated(image: &Image, now: SystemTime) -> bool {
    let reference = iso8601_utc(now);
    image
        .deprecation_time
        .as_deref()
        .is_some_and(|at| iso8601_is_before(at, &reference))
}

/// A stored ISO 8601 UTC timestamp STRING as the modelled timestamp type, or `None` when
/// there is no string or its rendering does not read as an instant.
///
/// EC2 models the SAME fact as a string on one shape and as a timestamp on another — an
/// image's `CreationDate` against `ImageAncestryEntry$CreationDate`, a Client VPN
/// attachment's `CreationTime` against `TransitGatewayAttachment$CreationTime` — so every
/// report that carries the timestamp form of a stored string reads it back through this ONE
/// conversion rather than re-deriving the instant from the clock, and no two reports of one
/// record can disagree about when it happened.
pub fn parse_iso8601_timestamp(text: Option<&str>) -> Option<::aws_smithy_types::DateTime> {
    text.and_then(|text| {
        ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
            .ok()
    })
}

/// The `Image$CreationDate` of a stored image as the modelled timestamp type, or `None` for
/// an image that carries none, or one whose stored rendering does not read as an instant.
///
/// "The date and time the image was created" is an ISO 8601 UTC STRING on the `Image` shape,
/// and a TIMESTAMP wherever a report carries the same fact — `ImageWatermark`'s
/// `SourceImageCreationTime` ("the creation date of the source AMI, in the following format:
/// YYYY-MM-DDTHH:MM:SS.ssssss+HH:MM") and `ImageAncestryEntry$CreationDate` ("the date and
/// time when this AMI was created"). Both read the stored rendering back through the one
/// [`parse_iso8601_timestamp`] every such report uses.
pub fn image_creation_time(creation_date: Option<&str>) -> Option<::aws_smithy_types::DateTime> {
    parse_iso8601_timestamp(creation_date)
}

/// Refuse a request that named an AMI id the account can see no image for, the way EC2
/// does: HTTP 400 `InvalidAMIID.NotFound`.
///
/// The message is EC2's own: it names the ids it could not resolve as a BRACKETED list,
/// because the refusal is made over the ids the request supplied — "The image id
/// '[ami-12345678]' does not exist" is what the real service answers, recorded by the
/// corpus unit `ec2.op-contract-request-spot-instances`. A refusal here names the one id at
/// fault.
pub fn invalid_ami_id_not_found<E>(image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_AMI_ID_NOT_FOUND,
        400,
        format!("The image id '[{image_id}]' does not exist"),
    )
}

/// Refuse an AMI id a request named that EC2 cannot resolve as one at all: HTTP 400
/// `InvalidAMIID.Malformed`.
///
/// The message is [`invalid_id_message`], the one wording every malformed-id refusal in
/// this crate uses, so one code reads one way across every operation that answers it.
pub fn invalid_ami_id_malformed<E>(image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(INVALID_AMI_ID_MALFORMED, 400, invalid_id_message(image_id))
}

/// Whether `id` is an AMI id EC2 could have ISSUED: `ami-` followed by the long id form
/// ([`issued_long_id_is_well_formed`]) — 17 lowercase hexadecimal digits under a leading
/// `0`, not all of them `0`.
///
/// The stricter reading of [`image_id_is_well_formed`], for the validator that resolves an
/// `ImageId` a LAUNCH is to run: a value inside the generic id form but outside the one EC2
/// issues — the `ami-1234567890abcdef0` shape documentation examples and hand-written probes
/// reach for — is answered `InvalidAMIID.Malformed` before any image is looked up, which is
/// what the recorded `RunInstances` refusal of that shape shows, and the clause a
/// LONG-form instance id is read under too ([`instance_id_is_well_formed`]). The
/// describes of the account's images keep the wider [`image_id_is_well_formed`] on their own
/// recorded evidence (a short-form id there is answered `InvalidAMIID.NotFound`), so which
/// reading an operation uses is a fact of that operation's validator.
pub fn issued_image_id_is_well_formed(id: &str) -> bool {
    issued_long_id_is_well_formed("ami", id)
}

/// Refuse an AMI id the launch validator cannot read: HTTP 400 `InvalidAMIID.Malformed`,
/// worded the way EC2's id parser words it there — [`invalid_id_message`]'s sentence
/// followed by the id form it expected, `Invalid id: "<id>" (expecting "ami-...")`.
///
/// The fixed text is the parser's; the one variable part is the value the REQUEST sent,
/// bounded the way [`invalid_id_message`] bounds every echoed id. Kept beside
/// [`invalid_ami_id_malformed`] rather than replacing it, because the shared template is
/// what the describes of this service's images were recorded answering and this longer
/// sentence is what the launch path was.
pub fn invalid_ami_id_malformed_expecting_ami<E>(image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_AMI_ID_MALFORMED,
        400,
        format!("{} (expecting \"ami-...\")", invalid_id_message(image_id)),
    )
}

/// Refuse an owner or executable-by term that names no account and no alias: HTTP 400
/// `InvalidUserID.Malformed`.
pub fn invalid_user_id_malformed<E>(user_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_USER_ID_MALFORMED,
        400,
        format!("Invalid user id: '{user_id}'"),
    )
}

/// What one AMI is stored with: the facts about the image itself, everything the service
/// derives being left to [`ComAmazonawsEc2Amazonec2State::insert_image`].
#[derive(Debug, Clone)]
pub struct ImageSpec {
    /// "The name of the AMI that was provided during image creation" (`Image$Name`).
    pub name: String,
    /// "The description of the AMI that was provided during image creation".
    pub description: String,
    /// One of `ARCHITECTURE_VALUES_VALUES`.
    pub architecture: String,
    /// `Some(`[`IMAGE_PLATFORM_WINDOWS`]`)` for a Windows image, `None` otherwise.
    pub platform: Option<String>,
    /// The billing platform details ([`PLATFORM_DETAILS_LINUX`]).
    pub platform_details: String,
    /// The billing usage operation ([`USAGE_OPERATION_LINUX`]).
    pub usage_operation: String,
    /// One of `BOOT_MODE_VALUES_VALUES`, for an image that declares one.
    pub boot_mode: Option<String>,
    /// [`IMDS_SUPPORT_V2`] for an image that requires IMDSv2.
    pub imds_support: Option<String>,
    /// The root device name of the image's block device mapping.
    pub root_device_name: String,
    /// The size, in GiB, of the root volume the image's snapshot restores into.
    pub root_volume_size: i32,
    /// The EBS volume type that volume is created as.
    pub root_volume_type: String,
    /// One of `VIRTUALIZATION_TYPE_VALUES` ([`IMAGE_VIRTUALIZATION_TYPE_HVM`]).
    pub virtualization_type: String,
    /// Where the image's root file system lives ([`IMAGE_ROOT_DEVICE_TYPE_EBS`]). An
    /// instance-store-backed image restores no snapshot, so it carries no Amazon EBS root
    /// block device mapping.
    pub root_device_type: String,
    /// One of `IMAGE_STATE_VALUES`.
    pub state: String,
    /// "Indicates whether the image has public launch permissions" (`Image$Public`).
    pub public: bool,
    /// The account that owns the image.
    pub owner_id: String,
    /// The owner's alias, for an image published under one.
    pub owner_alias: Option<String>,
    /// "The date and time the image was created" (`Image$CreationDate`), an ISO 8601 UTC
    /// timestamp.
    pub creation_date: String,
    /// "The date and time to deprecate the AMI" (`Image$DeprecationTime`), for an image
    /// that carries one.
    pub deprecation_time: Option<String>,
    /// "Any product codes associated with the AMI" (`Image$ProductCodes`): the
    /// Marketplace or DevPay codes a PAID image carries, each with its
    /// `ProductCodeValues` type. Empty for an image nobody sells.
    pub product_codes: Vec<ProductCode>,
    /// "Indicates whether the image is eligible for Amazon Web Services Free Tier".
    pub free_tier_eligible: bool,
    /// "The name of the public Systems Manager parameter that resolves to this AMI",
    /// for an image one resolves to.
    pub public_ssm_parameter_name: Option<String>,
    /// "Any tags assigned to the image" (`Image$Tags`).
    pub tags: Vec<Tag>,
    /// "Any block device mapping entries" (`Image$BlockDeviceMappings`) the image was
    /// REGISTERED with, each restoring its device from a snapshot the caller named.
    ///
    /// `None` for an image whose mappings this store derives instead: one root mapping
    /// restored from a snapshot [`ComAmazonawsEc2Amazonec2State::insert_image`] mints, which
    /// is what the published catalog and an import's AMI carry.
    pub block_device_mappings: Option<Vec<BlockDeviceMapping>>,
    /// "The ID of the kernel" (`Image$KernelId`), for an image that boots one.
    pub kernel_id: Option<String>,
    /// "The ID of the RAM disk associated with the image" (`Image$RamdiskId`).
    pub ramdisk_id: Option<String>,
    /// "A value of `simple` indicates that enhanced networking with the Intel 82599 VF
    /// interface is enabled" (`Image$SriovNetSupport`) — `None` for an image nothing
    /// enabled it for.
    pub sriov_net_support: Option<String>,
    /// "Indicates whether enhanced networking with ENA is enabled" (`Image$EnaSupport`), for
    /// an image that reports the facility at all: `None` is an image that reports nothing for
    /// it, which is what an AMI nothing enabled ENA for carries.
    pub ena_support: Option<bool>,
    /// "If the image is configured for NitroTPM support, the value is `v2.0`"
    /// (`Image$TpmSupport`).
    pub tpm_support: Option<String>,
    /// "The location of the AMI" (`Image$ImageLocation`), for an image registered from a
    /// manifest the request named. `None` derives it as `<owner>/<name>`.
    pub image_location: Option<String>,
    /// "Base64 representation of the non-volatile UEFI variable store"
    /// (`ImageAttribute$UefiData`), for an image registered with one. The `Image` shape
    /// carries no member for it, so it is held beside the image in
    /// [`ComAmazonawsEc2Amazonec2State::image_uefi_data`].
    pub uefi_data: Option<String>,
}

/// An EBS-backed Linux machine image of the calling account, created now and available:
/// the shape of an image a register or create operation stores. Every published catalog
/// row below starts from [`published_amazon_image`], which starts from this.
impl Default for ImageSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            architecture: "x86_64".to_string(),
            platform: None,
            platform_details: PLATFORM_DETAILS_LINUX.to_string(),
            usage_operation: USAGE_OPERATION_LINUX.to_string(),
            boot_mode: None,
            imds_support: None,
            root_device_name: LINUX_ROOT_DEVICE_NAME.to_string(),
            root_volume_size: LINUX_ROOT_VOLUME_SIZE,
            root_volume_type: IMAGE_ROOT_VOLUME_TYPE.to_string(),
            virtualization_type: IMAGE_VIRTUALIZATION_TYPE_HVM.to_string(),
            root_device_type: IMAGE_ROOT_DEVICE_TYPE_EBS.to_string(),
            state: IMAGE_STATE_AVAILABLE.to_string(),
            public: false,
            owner_id: ::engine::emulator_account_id().to_string(),
            owner_alias: None,
            creation_date: iso8601_utc(SystemTime::now()),
            deprecation_time: None,
            product_codes: Vec::new(),
            free_tier_eligible: false,
            public_ssm_parameter_name: None,
            tags: Vec::new(),
            block_device_mappings: None,
            kernel_id: None,
            ramdisk_id: None,
            sriov_net_support: Some(IMAGE_SRIOV_NET_SUPPORT_SIMPLE.to_string()),
            // An HVM image offers enhanced networking with ENA, which is what this default
            // describes; "enhanced networking with the Intel 82599 VF interface" above and
            // ENA are both HVM facilities, so a paravirtual row of the published catalog
            // reports neither.
            ena_support: Some(true),
            tpm_support: None,
            image_location: None,
            uefi_data: None,
        }
    }
}

/// The caller-chosen bytes one stored AMI holds: its tags, and every string of the record
/// whose LENGTH a request chose — the name (which the derived `ImageLocation` holds a
/// second copy of), the description, the architecture, the root device name (stored on the
/// image and on its block device mapping), the volume type, the virtualization type, the
/// root device type, the state, the billing strings,
/// the creation date, the owning account and its alias, the deprecation time, the public
/// parameter name, the platform, the boot mode and the IMDS support. The ids, the snapshot
/// and the fixed facts of an EBS-backed HVM image are derived by this module, so they are
/// fixed-size.
///
/// The ONE definition of that charge, the way [`address_payload_bytes`] is for an Elastic
/// IP address: [`ComAmazonawsEc2Amazonec2State::insert_image`] charges exactly it, so no
/// path can store those bytes uncharged, and a writer that asks
/// [`ComAmazonawsEc2Amazonec2State::would_exceed_stored_bytes`] first asks it with this
/// number. A string stored twice is counted twice, because that is what the store holds.
pub fn image_payload_bytes(spec: &ImageSpec) -> usize {
    let optional = |value: Option<&String>| -> usize { value.map_or(0, String::len) };
    tag_list_bytes(&spec.tags)
        // `Image$Name`, and the `<owner>/<name>` `ImageLocation` derived from it.
        .saturating_add(spec.name.len().saturating_mul(2))
        .saturating_add(spec.description.len())
        .saturating_add(spec.architecture.len())
        // `Image$RootDeviceName`, and the block device mapping's own `DeviceName`.
        .saturating_add(spec.root_device_name.len().saturating_mul(2))
        .saturating_add(spec.root_volume_type.len())
        .saturating_add(spec.virtualization_type.len())
        .saturating_add(spec.root_device_type.len())
        .saturating_add(spec.state.len())
        .saturating_add(spec.platform_details.len())
        .saturating_add(spec.usage_operation.len())
        .saturating_add(spec.creation_date.len())
        // The owning account, and the `ImageLocation` prefix when no alias replaces it.
        .saturating_add(spec.owner_id.len().saturating_mul(2))
        // `Image$ImageOwnerAlias`, and the `ImageLocation` prefix when it is set.
        .saturating_add(optional(spec.owner_alias.as_ref()).saturating_mul(2))
        .saturating_add(optional(spec.deprecation_time.as_ref()))
        .saturating_add(optional(spec.public_ssm_parameter_name.as_ref()))
        .saturating_add(optional(spec.platform.as_ref()))
        .saturating_add(optional(spec.boot_mode.as_ref()))
        .saturating_add(optional(spec.imds_support.as_ref()))
        .saturating_add(product_code_list_bytes(&spec.product_codes))
        // The mappings a register supplied, and the few strings of a registered AMI the
        // published catalog's rows leave to this module. The explicit `ImageLocation` is
        // charged ON TOP of the name it replaces rather than instead of it, which can only
        // refuse a later write earlier — never admit one the budget forbids.
        .saturating_add(block_device_mapping_list_bytes(
            spec.block_device_mappings.as_deref().unwrap_or_default(),
        ))
        .saturating_add(optional(spec.kernel_id.as_ref()))
        .saturating_add(optional(spec.ramdisk_id.as_ref()))
        .saturating_add(optional(spec.sriov_net_support.as_ref()))
        .saturating_add(optional(spec.tpm_support.as_ref()))
        .saturating_add(optional(spec.image_location.as_ref()))
        .saturating_add(optional(spec.uefi_data.as_ref()))
}

/// The caller-chosen bytes a block device mapping list holds: each entry's device name, its
/// virtual name, the empty-string marker that omits a device, and the strings of its Amazon
/// EBS half — the snapshot it restores from, the volume type, and the KMS key and Outpost it
/// names.
///
/// The ONE definition of that charge, the way [`tag_list_bytes`] is for a tag list, so every
/// store that keeps mappings beside a resource charges them the same way. The numbers and
/// flags of a mapping are fixed-size, so only the strings are counted.
pub fn block_device_mapping_list_bytes(mappings: &[BlockDeviceMapping]) -> usize {
    let optional = |value: Option<&String>| -> usize { value.map_or(0, String::len) };
    mappings.iter().fold(0usize, |total, mapping| {
        let ebs = mapping.ebs.as_ref();
        total
            .saturating_add(optional(mapping.device_name.as_ref()))
            .saturating_add(optional(mapping.virtual_name.as_ref()))
            .saturating_add(optional(mapping.no_device.as_ref()))
            .saturating_add(optional(ebs.and_then(|ebs| ebs.snapshot_id.as_ref())))
            .saturating_add(optional(ebs.and_then(|ebs| ebs.volume_type.as_ref())))
            .saturating_add(optional(ebs.and_then(|ebs| ebs.kms_key_id.as_ref())))
            .saturating_add(optional(ebs.and_then(|ebs| ebs.outpost_arn.as_ref())))
    })
}

/// The caller-chosen bytes a `ProductCode` list holds: each entry's code and its type.
///
/// The ONE definition of that charge, the way [`tag_list_bytes`] is for a tag list, so
/// every store that keeps product codes beside a resource charges them the same way.
pub fn product_code_list_bytes(codes: &[ProductCode]) -> usize {
    codes.iter().fold(0usize, |total, code| {
        total
            .saturating_add(code.product_code_id.as_ref().map_or(0, String::len))
            .saturating_add(code.product_code_type.as_ref().map_or(0, String::len))
    })
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another image fits inside the store's ceiling ([`MAX_IMAGES`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// instead of growing the store past its ceiling. A writer asks
    /// [`Self::would_exceed_stored_bytes`] with [`image_payload_bytes`] beside it, because a
    /// count of records is no bound on the caller-chosen bytes each one holds.
    pub fn has_image_capacity(&self) -> bool {
        // A recycled AMI is still held — it is restorable until its retention period runs
        // out — so it counts against the ceiling the same way a visible image does, and
        // restoring one can never be refused for want of room.
        self.images.len().saturating_add(self.images_in_recycle_bin.len()) < MAX_IMAGES
    }

    /// Store an AMI, and answer the `ImageId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the image's own id, the snapshot id of its root block device mapping, the
    /// manifest location `<owner>/<name>` the `manifest-location` filter selects on, and
    /// the fixed facts of a machine image — so two writers cannot report the same image
    /// differently. An image whose root device is the instance store
    /// ([`IMAGE_ROOT_DEVICE_TYPE_INSTANCE_STORE`]) restores no snapshot, so it is stored
    /// with no root block device mapping and spends no snapshot id; enhanced networking
    /// follows the virtualization, which is what offers it. Like [`Self::insert_vpc`], the caller-chosen bytes of
    /// the record are charged against the store's byte budget here, counted by the ONE
    /// definition of that charge ([`image_payload_bytes`]) rather than by the tags alone:
    /// a count ceiling ([`MAX_IMAGES`]) is no memory bound on its own, so every string
    /// whose LENGTH a request chose is charged against [`MAX_STORED_BYTES`] too.
    pub fn insert_image(&mut self, spec: ImageSpec) -> String {
        // Counted BEFORE `spec` is taken apart below, so the charge covers what the
        // caller supplied rather than what survived into the record.
        let payload_bytes = image_payload_bytes(&spec);
        let image_id = self.next_id("ami");
        let ebs_backed = spec.root_device_type == IMAGE_ROOT_DEVICE_TYPE_EBS;
        // A snapshot is minted only for an image this module DERIVES the root mapping of:
        // the mappings a register supplied name the caller's own snapshots, so there is
        // nothing to mint for them.
        let derived_root_snapshot_id = (ebs_backed && spec.block_device_mappings.is_none())
            .then(|| self.next_id("snap"));
        let owner = spec
            .owner_alias
            .clone()
            .unwrap_or_else(|| spec.owner_id.clone());
        let image = Image {
            platform_details: Some(spec.platform_details),
            usage_operation: Some(spec.usage_operation),
            // "Any block device mapping entries" (`Image$BlockDeviceMappings`): the ones the
            // image was REGISTERED with when the spec carries them — each restoring its
            // device from the snapshot the request named — and otherwise the root volume's,
            // restored from the snapshot minted above, with none at all for an image whose
            // root device is the instance store, which restores none.
            block_device_mappings: Some(spec.block_device_mappings.unwrap_or_else(|| {
                derived_root_snapshot_id
                    .map(|snapshot_id| BlockDeviceMapping {
                        ebs: Some(EbsBlockDevice {
                            delete_on_termination: Some(true),
                            snapshot_id: Some(snapshot_id),
                            volume_size: Some(spec.root_volume_size),
                            volume_type: Some(spec.root_volume_type),
                            encrypted: Some(false),
                            iops: None,
                            kms_key_id: None,
                            throughput: None,
                            outpost_arn: None,
                            availability_zone: None,
                            volume_initialization_rate: None,
                            availability_zone_id: None,
                            ebs_card_index: None,
                        }),
                        no_device: None,
                        device_name: Some(spec.root_device_name.clone()),
                        virtual_name: None,
                    })
                    .into_iter()
                    .collect()
            })),
            description: Some(spec.description),
            // "Indicates whether enhanced networking with ENA is enabled"
            // (`Image$EnaSupport`): reported where the image has it enabled, and left out
            // where it has not — an image nothing enabled it for reports nothing.
            ena_support: spec.ena_support,
            hypervisor: Some(IMAGE_HYPERVISOR_XEN.to_string()),
            image_owner_alias: spec.owner_alias,
            name: Some(spec.name.clone()),
            root_device_name: Some(spec.root_device_name),
            root_device_type: Some(spec.root_device_type),
            sriov_net_support: spec.sriov_net_support,
            // "The reason for the state change" (`Image$StateReason`): an image that
            // never left `available` has had no state change to report.
            state_reason: None,
            tags: Some(spec.tags),
            virtualization_type: Some(spec.virtualization_type),
            boot_mode: spec.boot_mode,
            // "If the image is configured for NitroTPM support, the value is `v2.0`"
            // (`Image$TpmSupport`): set for an image registered with that support, and for
            // no other image this store holds.
            tpm_support: spec.tpm_support,
            deprecation_time: spec.deprecation_time,
            imds_support: spec.imds_support,
            // Set only "if the AMI was created using CreateImage" (`Image$SourceInstanceId`).
            source_instance_id: None,
            deregistration_protection: Some(
                IMAGE_DEREGISTRATION_PROTECTION_DISABLED.to_string(),
            ),
            // "when the AMI was last used to launch an EC2 instance … there is a 24-hour
            // delay before that usage is reported" (`Image$LastLaunchedTime`), so an
            // image this store has only just published reports none.
            last_launched_time: None,
            // "If `true`, the AMI satisfies the criteria for Allowed AMIs"
            // (`Image$ImageAllowed`), which is the value of an account whose Allowed AMIs
            // criteria every image satisfies — among them an account that declares none,
            // which is what [`Self::allowed_images_settings`] starts empty at. Evaluating
            // stored criteria against an image is `DescribeImages`'s own behaviour and the
            // `ImageCriterion` matcher it would need does not exist yet, so nothing writes
            // another value here.
            image_allowed: Some(true),
            // Set only for an image copied from another (`CopyImage`).
            source_image_id: None,
            source_image_region: None,
            free_tier_eligible: Some(spec.free_tier_eligible),
            public_ssm_parameter_name: spec.public_ssm_parameter_name,
            // No watermark is attached until something attaches one.
            image_watermarks: Some(Vec::new()),
            // An image whose owner published no instance-type specification is
            // compatible with every instance type its architecture offers.
            instance_type_specification: None,
            image_id: Some(image_id.clone()),
            // "The location of the AMI" (`Image$ImageLocation`): the manifest location the
            // register named, and otherwise `<owner>/<name>` — the location the
            // `manifest-location` filter selects on either way.
            image_location: Some(
                spec.image_location
                    .unwrap_or_else(|| format!("{owner}/{}", spec.name)),
            ),
            state: Some(spec.state),
            owner_id: Some(spec.owner_id),
            creation_date: Some(spec.creation_date),
            public: Some(spec.public),
            // "Any product codes associated with the AMI" (`Image$ProductCodes`): the
            // codes the image was registered with, which is none for an image nobody
            // sells.
            product_codes: Some(spec.product_codes),
            architecture: Some(spec.architecture),
            image_type: Some(IMAGE_TYPE_MACHINE.to_string()),
            // "The ID of the kernel" / "the ID of the RAM disk associated with the image"
            // (`Image$KernelId`, `Image$RamdiskId`): the ones the image was registered
            // with. An HVM image carries neither, which is what leaving both out means.
            kernel_id: spec.kernel_id,
            ramdisk_id: spec.ramdisk_id,
            platform: spec.platform,
        };
        self.add_stored_bytes(payload_bytes);
        // "Base64 representation of the non-volatile UEFI variable store"
        // (`ImageAttribute$UefiData`): a fact of the image the `Image` shape carries no
        // member for, so it is held beside it under the same id — written here, with the
        // image, so no path can store one without the other.
        if let Some(uefi_data) = spec.uefi_data {
            self.image_uefi_data.insert(image_id.clone(), uefi_data);
        }
        self.images.insert(image_id.clone(), image);
        image_id
    }

    /// The id of the AMI of the calling account that carries the name `name`, or `None` when
    /// no image of this account does.
    ///
    /// "The name must be unique for AMIs in the Region for this account"
    /// (`CreateRestoreImageTaskRequest$Name`), so a register that would take a name already
    /// in use is refused with [`invalid_ami_name_duplicate`] naming the image this resolves
    /// to. The scope is the ACCOUNT's own AMIs: a public image of another account, which
    /// this account can see but does not own, does not reserve a name against it. An AMI in
    /// the Recycle Bin is deregistered and holds no name against a register either — it is
    /// restorable, so what it would clash with is the restore rather than this call.
    ///
    /// The lowest such id, the store being ordered by id, so one name is always reported
    /// against the same image.
    pub fn image_named(&self, name: &str) -> Option<String> {
        let account = ::engine::emulator_account_id();
        self.images
            .iter()
            .find(|(_, image)| {
                image.owner_id.as_deref() == Some(account) && image.name.as_deref() == Some(name)
            })
            .map(|(image_id, _)| image_id.clone())
    }

    /// The snapshot id of `image_id`'s ROOT block device mapping, or `None` when this
    /// account holds no such image or that image's root device carries no snapshot.
    ///
    /// A fact OF the image, read back off the stored [`Image`] rather than recomputed:
    /// [`Self::insert_image`] mints the root device's snapshot id and writes it into the
    /// mapping, so this is the snapshot an EBS-backed AMI restores its root volume from —
    /// which is what a root volume replacement restored from an AMI, or to the original
    /// root volume's launch state, creates the replacement volume from
    /// (`ReplaceRootVolumeTask$SnapshotId`, "the ID of the snapshot used to create the
    /// replacement root volume").
    ///
    /// The ROOT mapping is the one whose `DeviceName` is the image's own
    /// `RootDeviceName`; an image that declares none falls back to the first mapping that
    /// carries a snapshot, so a single-mapping AMI is read the same either way.
    pub fn image_root_snapshot_id(&self, image_id: &str) -> Option<String> {
        let image = self.images.get(image_id)?;
        let root_device = image.root_device_name.as_deref();
        let mappings = image.block_device_mappings.as_deref().unwrap_or_default();
        mappings
            .iter()
            .find(|mapping| {
                root_device.is_some() && mapping.device_name.as_deref() == root_device
            })
            .or_else(|| {
                mappings
                    .iter()
                    .find(|mapping| mapping.ebs.as_ref().is_some_and(|ebs| ebs.snapshot_id.is_some()))
            })
            .and_then(|mapping| mapping.ebs.as_ref())
            .and_then(|ebs| ebs.snapshot_id.clone())
    }

    /// The id of a registered AMI of this account that restores one of its devices from
    /// `snapshot_id`, or `None` when no image does.
    ///
    /// Read off the stored [`Image`] records, whose block device mappings carry the
    /// snapshot each EBS device is restored from: "You cannot delete a snapshot of the
    /// root device of an EBS volume used by a registered AMI. You must first deregister the
    /// AMI before you can delete the snapshot" (`DeleteSnapshot`), so this is the image a
    /// delete of that snapshot has to name. An image the Recycle Bin holds is DEREGISTERED
    /// ("it can be restored before its retention period expires", `DeregisterImage`), so it
    /// is not one of the registered AMIs this answers with.
    ///
    /// The lowest image id that uses the snapshot, the store being ordered by id, so one
    /// snapshot used by several images is always reported against the same one.
    pub fn image_using_snapshot(&self, snapshot_id: &str) -> Option<String> {
        self.images
            .iter()
            .find(|(_, image)| {
                image
                    .block_device_mappings
                    .iter()
                    .flatten()
                    .filter_map(|mapping| mapping.ebs.as_ref())
                    .any(|ebs| ebs.snapshot_id.as_deref() == Some(snapshot_id))
            })
            .map(|(image_id, _)| image_id.clone())
    }

    /// Give the store the public images Amazon publishes ([`published_image_specs`]), and
    /// answer their ids.
    ///
    /// "The images available to you include public images" (`DescribeImages`) and no EC2
    /// operation creates them, so they are part of a fresh account's state the same way
    /// its default VPC is.
    pub fn insert_published_images(&mut self) -> Vec<String> {
        let mut inserted = Vec::new();
        for spec in published_image_specs() {
            if !self.has_image_capacity() {
                break;
            }
            inserted.push(self.insert_image(spec));
        }
        inserted
    }

    /// Whether `image_id` is a long-form id whose identifier names a resource this service
    /// issued that is no AMI of this account at all.
    ///
    /// The AMI counterpart of [`Self::instance_id_identifier_was_never_issued`], read
    /// against the record this store keeps of the AMI ids it issued rather than against a
    /// counter: every resource kind but the instance draws its identifiers from the ONE
    /// [`Self::next_id`] counter, so the identifiers spent on AMIs are the ones the
    /// catalogue holds or the Recycle Bin still holds — "it can be restored before its
    /// retention period expires" (`DeregisterImage`), so a recycled AMI's id is its own
    /// however invisible the image is, and either is looked up and reported as an image the
    /// account cannot see. A long-form value whose identifier the mint HAS reached while no
    /// AMI ever carried it names another kind of resource, so it is no AMI id and is refused
    /// before any lookup; one the mint has not reached is left to the lookup.
    pub fn image_id_identifies_another_resource(&self, image_id: &str) -> bool {
        if self.images.contains_key(image_id) || self.images_in_recycle_bin.contains_key(image_id)
        {
            return false;
        }
        long_form_identifier(image_id)
            .is_some_and(|identifier| identifier <= u128::from(self.id_counter))
    }

    /// Every public Systems Manager parameter name this account can resolve, and the id of
    /// the AMI it resolves to.
    ///
    /// `Image$PublicSsmParameterName` is "the name of the public Systems Manager parameter
    /// that resolves to this AMI, under the `aws/service/` namespace", so the stored images
    /// are what a `resolve:ssm:` value is resolved against — the one place this emulator
    /// records which AMI a parameter names, rather than a second table of ids that could
    /// disagree with the catalog. The member holds ONE name per image while AWS publishes
    /// some images under more than one name, so the names that share an image's entry
    /// ([`super::PUBLIC_SSM_PARAMETER_ALIASES`]) are resolved to the same id here.
    ///
    /// Only an image `account` can see resolves: a parameter whose image has been
    /// deregistered or recycled names no AMI, the same way the describe reports none.
    pub fn public_ssm_parameter_image_ids(&self, account: &str) -> BTreeMap<String, String> {
        let mut resolved: BTreeMap<String, String> = self
            .images
            .values()
            .filter(|image| image_is_visible(image, account))
            .filter_map(|image| {
                Some((
                    image.public_ssm_parameter_name.clone()?,
                    image.image_id.clone()?,
                ))
            })
            .collect();
        for (alias, published_as) in super::PUBLIC_SSM_PARAMETER_ALIASES {
            if let Some(image_id) = resolved.get(*published_as).cloned() {
                resolved.insert((*alias).to_string(), image_id);
            }
        }
        resolved
    }
}

/// The prefix an `ImageId` carries when it names a Systems Manager parameter instead of an
/// AMI.
///
/// "Alternatively, you can specify a Systems Manager parameter, using one of the following
/// formats. The Systems Manager parameter will resolve to an AMI ID on launch. To reference a
/// public parameter: `resolve:ssm:public-parameter`"
/// (`RequestLaunchTemplateData$ImageId`).
pub const SSM_IMAGE_ALIAS_PREFIX: &str = "resolve:ssm:";

/// The Systems Manager parameter name `image_id` is an alias for, or `None` when it names an
/// AMI directly.
///
/// The ONE reading of that value in this crate: the launch that resolves a parameter to the
/// AMI it names and the describe that reports what a stored one resolves to cannot read the
/// same value two ways. The parameter the value carries is resolved against the image
/// catalogue ([`ComAmazonawsEc2Amazonec2State::public_ssm_parameter_image_ids`]), which is
/// the one place this emulator records which AMI a parameter names.
///
/// Whatever follows the prefix is the parameter name, so the documented `<name>:<version-number>`
/// and `<name>:<label>` forms are read as names of their own and resolve to no AMI here: this
/// emulator records one AMI per published parameter and no versions or labels of it, and a value
/// that named a version would otherwise be answered with an image chosen for a version nothing
/// published.
pub fn ssm_image_alias_parameter(image_id: &str) -> Option<&str> {
    image_id
        .strip_prefix(SSM_IMAGE_ALIAS_PREFIX)
        .filter(|parameter| !parameter.is_empty())
}

/// The code EC2 answers a request whose `ImageId` names Systems Manager parameters that
/// resolve to no AMI (EC2 error codes, `SsmInvalidParameter`).
pub const SSM_INVALID_PARAMETER: &str = "SsmInvalidParameter";

/// Refuse a launch template whose `ImageId` alias resolves to no AMI: HTTP 400
/// [`SSM_INVALID_PARAMETER`].
///
/// "The Systems Manager parameter will resolve to an AMI ID on launch"
/// (`RequestLaunchTemplateData$ImageId`), and EC2 resolves it when the template VERSION is
/// created, not only when an instance is launched: a template whose alias names no parameter
/// is refused there and then, so the client learns of it where it can still fix the value.
///
/// The sentence is EC2's: the fixed text `The following aliases are invalid: ` followed by
/// the parameter names it could not resolve, which are the request's own values with the
/// `resolve:ssm:` prefix stripped — the names the alias NAMES, not the aliases as written.
/// The plural is EC2's too: the service reports the set it could not resolve, which for a
/// launch template's one `ImageId` is one name. Each is bounded the way every echoed id of
/// this crate is ([`echoed_resource_id`]), the member carrying no modelled length of its own.
pub fn ssm_aliases_are_invalid<E>(parameters: &[&str]) -> Scaffolded<E> {
    let named = parameters
        .iter()
        .map(|parameter| echoed_resource_id(parameter))
        .collect::<Vec<String>>()
        .join(", ");
    Scaffolded::service_error(
        SSM_INVALID_PARAMETER,
        400,
        format!("The following aliases are invalid: {named}"),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Refuse a launch template `ImageId` whose `resolve:ssm:` alias this account resolves to
    /// no AMI, leaving every other value — a literal `ami-` id, or no `ImageId` at all — to
    /// the launch that will read it.
    ///
    /// The ONE reading both template writers share, so `CreateLaunchTemplate` and
    /// `CreateLaunchTemplateVersion` cannot disagree about which alias a stored version may
    /// carry. The parameter is resolved against the image catalogue
    /// ([`Self::public_ssm_parameter_image_ids`]), the one place this emulator records which
    /// AMI a parameter names, so a template is refused exactly when a launch from it could
    /// not have resolved its AMI.
    pub fn require_resolvable_image_alias<E>(&self, image_id: Option<&str>) -> Result<(), Scaffolded<E>> {
        let Some(parameter) = image_id.and_then(ssm_image_alias_parameter) else {
            return Ok(());
        };
        if self
            .public_ssm_parameter_image_ids(::engine::emulator_account_id())
            .contains_key(parameter)
        {
            return Ok(());
        }
        Err(ssm_aliases_are_invalid(&[parameter]))
    }
}

/// A public AMI Amazon publishes: owned by Amazon's publishing account
/// ([`AMAZON_IMAGE_OWNER_ID`]) under the `amazon` alias, with public launch permissions,
/// and eligible for the Free Tier as the Amazon Linux and Windows base images are.
pub(super) fn published_amazon_image(name: &str, architecture: &str, creation_date: &str) -> ImageSpec {
    ImageSpec {
        name: name.to_string(),
        architecture: architecture.to_string(),
        creation_date: creation_date.to_string(),
        public: true,
        owner_id: AMAZON_IMAGE_OWNER_ID.to_string(),
        owner_alias: Some(IMAGE_OWNER_ALIAS_AMAZON.to_string()),
        free_tier_eligible: true,
        ..ImageSpec::default()
    }
}

/// How long before the catalog is seeded Amazon Linux 2's standard support ended — the
/// instant every Amazon Linux 2 build of the catalog carries as its `DeprecationTime`.
///
/// CHOSEN, and relative rather than a calendar date, so the catalog always holds an image
/// that IS deprecated: a client auditing deprecation reads the contrast between an image
/// `DescribeImages` leaves out and one `IncludeDeprecated` brings back, and a fixed date
/// either has not been reached yet or recedes without bound as this emulator's clock
/// advances. It is before every build below, so no build is published after the support it
/// was published under had ended.
pub(super) const AL2_SUPPORT_ENDED_DAYS_AGO: u64 = 120;

/// How long before the catalog is seeded the older of its two Amazon Linux 2 builds was
/// published — the age of a build still in wide use when its family's support ended, so a
/// deprecation audit has a build of a realistic age to read.
pub(super) const AL2_OLDER_BUILD_DAYS_AGO: u64 = 450;

/// The same for the newer of the two, published later in that family's life.
pub(super) const AL2_NEWER_BUILD_DAYS_AGO: u64 = 390;

/// The seconds in a day, for the catalog's relative build dates.
pub(super) const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The instant `days_ago` days before now, truncated to the start of that UTC day.
///
/// TRUNCATED so the catalog is stable: every reader of [`published_image_specs`] within one
/// day of this process's life gets the same timestamp to the second, and the build date a
/// published name carries is exactly the date the image reports.
pub(super) fn rolling_catalog_date(days_ago: u64) -> SystemTime {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(days_ago.saturating_mul(SECONDS_PER_DAY));
    UNIX_EPOCH + Duration::from_secs(seconds - seconds % SECONDS_PER_DAY)
}

/// `at` as the `YYYYMMDD` build date an Amazon Linux published name carries, read off the
/// same rendering the image reports as its `CreationDate` so the two cannot disagree.
pub(super) fn published_build_date(at: SystemTime) -> String {
    iso8601_utc(at)
        .chars()
        .take(ISO8601_DATE_WIDTH)
        .filter(char::is_ascii_digit)
        .collect()
}

/// The characters of the date part of an ISO 8601 UTC rendering (`YYYY-MM-DD`).
pub(super) const ISO8601_DATE_WIDTH: usize = 10;

/// One deprecated Amazon Linux 2 build of the published catalog, built `days_ago` days
/// before the catalog was seeded.
///
/// The name is the one Amazon documents for an Amazon Linux 2 image —
/// `amzn2-ami-hvm-<version>-<architecture>-<root volume>`, whose version carries the build
/// date — so the name, the `CreationDate` and the description all name the same build. Its
/// `DeprecationTime` is the family's end of standard support
/// ([`AL2_SUPPORT_ENDED_DAYS_AGO`]), which has passed, so the image is deprecated and
/// `DescribeImages` leaves it out unless the request asks for deprecated images.
pub(super) fn deprecated_amazon_linux_2_image(days_ago: u64) -> ImageSpec {
    let created = rolling_catalog_date(days_ago);
    let version = format!("2.0.{}.0", published_build_date(created));
    ImageSpec {
        description: format!("Amazon Linux 2 AMI {version} x86_64 HVM gp2"),
        boot_mode: Some(BOOT_MODE_LEGACY_BIOS.to_string()),
        root_volume_type: IMAGE_ROOT_VOLUME_TYPE_GP2.to_string(),
        deprecation_time: Some(iso8601_utc(rolling_catalog_date(
            AL2_SUPPORT_ENDED_DAYS_AGO,
        ))),
        ..published_amazon_image(
            &format!("amzn2-ami-hvm-{version}-x86_64-gp2"),
            "x86_64",
            &iso8601_utc(created),
        )
    }
}

/// The public AMIs a fresh account can already see.
///
/// The Amazon Linux families open it: ONE ROW PER PUBLISHED ALIAS
/// ([`super::published_amazon_linux_specs`]), each named the way Amazon documents its
/// published names and carrying the build date of its version as its `CreationDate` — which
/// is what makes "the newest `CreationDate` of the images matching a name pattern" pick the
/// same image on every run. The rows after them are the ones no public parameter names: a
/// previous build of two Amazon Linux 2023 variants, because that is the choice a caller
/// resolving an AMI by name pattern has to make, and two Amazon Linux 2 builds carrying the
/// `DeprecationTime` their end of standard support fell on, so a deprecated image is part of
/// the catalog rather than only of a test.
///
/// The AWS PCS sample AMIs ([`pcs_sample_image_specs`]) close the catalog: they are
/// published the same way and under the Parameter Store names AWS publishes their ids
/// under.
pub fn published_image_specs() -> Vec<ImageSpec> {
    // Every variant Amazon publishes a public Systems Manager parameter for, one row per
    // published alias — the hierarchy a client that cannot hardcode an AMI id enumerates to
    // find one.
    let mut specs = super::published_amazon_linux_specs();
    specs.extend([
        // The previous build of two Amazon Linux 2023 variants: an older `CreationDate`, and
        // no public Systems Manager parameter, which resolves to the latest build only.
        ImageSpec {
            description: "Amazon Linux 2023 AMI 2023.5.20240903.0 x86_64 HVM kernel-6.1"
                .to_string(),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            ..published_amazon_image(
                "al2023-ami-2023.5.20240903.0-kernel-6.1-x86_64",
                "x86_64",
                "2024-09-03T22:17:11.000Z",
            )
        },
        ImageSpec {
            description: "Amazon Linux 2023 AMI 2023.5.20240903.0 x86_64 HVM kernel-6.1 Minimal"
                .to_string(),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            ..published_amazon_image(
                "al2023-ami-minimal-2023.5.20240903.0-kernel-6.1-x86_64",
                "x86_64",
                "2024-09-03T22:17:11.000Z",
            )
        },
        // Amazon Linux 2 reached the end of its standard support, which is the
        // `DeprecationTime` every build published before it carries — so these builds are
        // deprecated whenever the catalog is read. Both their build dates and that instant
        // are relative to the moment the catalog is seeded ([`rolling_catalog_date`]): a
        // published AMI family is republished on a rolling schedule and AWS's own dates move
        // with it, so a frozen calendar date would make the catalog's deprecated images
        // arbitrarily old as this emulator's clock advances, and the age a client audits
        // deprecation by ("how long ago was this deprecated build published?") would answer
        // differently every month. Two builds, because the choice a caller resolving a
        // deprecated AMI by name pattern has to make is between builds.
        deprecated_amazon_linux_2_image(AL2_OLDER_BUILD_DAYS_AGO),
        deprecated_amazon_linux_2_image(AL2_NEWER_BUILD_DAYS_AGO),
        ImageSpec {
            description: "Microsoft Windows Server 2022 Full Locale English AMI provided by Amazon"
                .to_string(),
            platform: Some(IMAGE_PLATFORM_WINDOWS.to_string()),
            platform_details: PLATFORM_DETAILS_WINDOWS.to_string(),
            usage_operation: USAGE_OPERATION_WINDOWS.to_string(),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            root_device_name: WINDOWS_ROOT_DEVICE_NAME.to_string(),
            root_volume_size: WINDOWS_ROOT_VOLUME_SIZE,
            public_ssm_parameter_name: Some(
                "/aws/service/ami-windows-latest/Windows_Server-2022-English-Full-Base"
                    .to_string(),
            ),
            ..published_amazon_image(
                "Windows_Server-2022-English-Full-Base-2024.11.13",
                "x86_64",
                "2024-11-13T07:36:19.000Z",
            )
        },
    ]);
    specs.extend(pcs_sample_image_specs());
    // The images a service publishes a METADATA hierarchy about rather than a bare alias.
    specs.extend(super::published_image_metadata_specs());
    specs
}

/// The build date every AWS PCS sample AMI of the catalog was published on, carried by its
/// name as well as by its `CreationDate` so the two cannot disagree — the same way every
/// Amazon Linux row above carries the build date of the version it names.
const PCS_SAMPLE_AMI_BUILD_DATE: &str = "2025-01-15";

/// The `CreationDate` of those images: the build date above, at the time of day the
/// catalog's other rows carry one at.
const PCS_SAMPLE_AMI_CREATION_DATE: &str = "2025-01-15T09:21:44.000Z";

/// One published AWS PCS sample AMI: `(Slurm version, operating system, architecture)`.
///
/// AWS PCS publishes a sample AMI per Slurm version, operating system and architecture, and
/// publishes each one's id in Parameter Store under
/// [`pcs_sample_ami_parameter_name`] — which is how a client picks the image for a compute
/// node group without naming a Region-specific id. Two Slurm versions are published,
/// because choosing between them is the choice a client reading the hierarchy has to make,
/// and both architectures of the newer one, because a node group's instance types decide
/// which it needs.
const PCS_SAMPLE_AMI_ROWS: &[(&str, &str, &str)] = &[
    ("24.11", "amzn2", "x86_64"),
    ("24.11", "amzn2", "arm64"),
    ("24.11", "al2023", "x86_64"),
    ("24.11", "al2023", "arm64"),
    ("24.05", "amzn2", "x86_64"),
];

/// The public Systems Manager parameter name AWS PCS publishes a sample AMI's id under:
/// `/aws/service/pcs/<Slurm version>/<operating system>/<architecture>/latest/ami-id`.
pub fn pcs_sample_ami_parameter_name(slurm: &str, os: &str, architecture: &str) -> String {
    format!("/aws/service/pcs/{slurm}/{os}/{architecture}/latest/ami-id")
}

/// The AWS PCS sample AMIs of the published catalog, one per [`PCS_SAMPLE_AMI_ROWS`] entry.
///
/// Public images like every other row of [`published_image_specs`], each carrying the
/// Parameter Store name its id is published under, so the one place this emulator records
/// which AMI a public parameter names
/// ([`ComAmazonawsEc2Amazonec2State::public_ssm_parameter_image_ids`]) holds the PCS
/// hierarchy too and a client resolving it reaches an image this account can really launch.
pub fn pcs_sample_image_specs() -> Vec<ImageSpec> {
    PCS_SAMPLE_AMI_ROWS
        .iter()
        .map(|(slurm, os, architecture)| {
            // The boot mode each variant is published with, as the Amazon Linux rows above
            // carry it: 64-bit Arm boots UEFI, Amazon Linux 2023 on x86 prefers it, and
            // Amazon Linux 2 on x86 boots the legacy BIOS.
            let boot_mode = match (*os, *architecture) {
                (_, "arm64") => BOOT_MODE_UEFI,
                ("al2023", _) => BOOT_MODE_UEFI_PREFERRED,
                _ => BOOT_MODE_LEGACY_BIOS,
            };
            ImageSpec {
                description: format!(
                    "AWS PCS sample AMI for Slurm {slurm} on {os} {architecture}"
                ),
                boot_mode: Some(boot_mode.to_string()),
                // Amazon Linux 2023 requires IMDSv2, as its own rows above do; Amazon
                // Linux 2 declares no such requirement.
                imds_support: (*os == "al2023").then(|| IMDS_SUPPORT_V2.to_string()),
                // An HPC compute-node image is no Free Tier image, unlike the Amazon Linux
                // and Windows base rows above.
                free_tier_eligible: false,
                public_ssm_parameter_name: Some(pcs_sample_ami_parameter_name(
                    slurm,
                    os,
                    architecture,
                )),
                ..published_amazon_image(
                    &format!(
                        "aws-pcs-sample_ami-{os}-{architecture}-slurm-{slurm}-{}",
                        PCS_SAMPLE_AMI_BUILD_DATE.replace('-', ".")
                    ),
                    architecture,
                    PCS_SAMPLE_AMI_CREATION_DATE,
                )
            }
        })
        .collect()
}

// The public Systems Manager parameter names AWS publishes as a SECOND name for an image
// `published_image_specs` already publishes under one are
// `super::PUBLIC_SSM_PARAMETER_ALIASES`, beside the table that publishes those images.

// ---------------------------------------------------------------------------
// Application status: what `DescribeApplicationStatus` aggregates
// ---------------------------------------------------------------------------

/// `ApplicationStatusEnum`'s `ok` member. The full catalog is the generated
/// `crate::ops::types::APPLICATION_STATUS_ENUM_VALUES`; these constants name the members
/// the aggregation below chooses between, so one value is spelled one way wherever it is
/// read or written.
pub const APPLICATION_STATUS_OK: &str = "ok";

/// `ApplicationStatusEnum`'s `impaired` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_IMPAIRED: &str = "impaired";

/// `ApplicationStatusEnum`'s `initializing` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_INITIALIZING: &str = "initializing";

/// `ApplicationStatusEnum`'s `insufficient-data` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_INSUFFICIENT_DATA: &str = "insufficient-data";

/// `ApplicationStatusEnum`'s `not-applicable` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_NOT_APPLICABLE: &str = "not-applicable";

/// `ApplicationStatusEnum`'s `suppressed` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_SUPPRESSED: &str = "suppressed";

/// `ApplicationStatusCheckEnum`'s `passed` member: one CHECK's own status, which is a
/// different enum from the instance-level [`APPLICATION_STATUS_OK`] above. The full
/// catalog is `crate::ops::types::APPLICATION_STATUS_CHECK_ENUM_VALUES`.
pub const APPLICATION_STATUS_CHECK_PASSED: &str = "passed";

/// `ApplicationStatusCheckEnum`'s `failed` member (see
/// [`APPLICATION_STATUS_CHECK_PASSED`]).
pub const APPLICATION_STATUS_CHECK_FAILED: &str = "failed";

/// `ApplicationStatusCheckEnum`'s `initializing` member (see
/// [`APPLICATION_STATUS_CHECK_PASSED`]).
pub const APPLICATION_STATUS_CHECK_INITIALIZING: &str = "initializing";

/// `ApplicationStatusCheckEnum`'s `insufficient-data` member (see
/// [`APPLICATION_STATUS_CHECK_PASSED`]).
pub const APPLICATION_STATUS_CHECK_INSUFFICIENT_DATA: &str = "insufficient-data";

/// `ApplicationStatusCheckEnum`'s `not-applicable` member (see
/// [`APPLICATION_STATUS_CHECK_PASSED`]): a check that does not apply to the instance, so
/// it is not one of the checks the instance-level status is derived from.
pub const APPLICATION_STATUS_CHECK_NOT_APPLICABLE: &str = "not-applicable";

/// `AggregationStatusEnum`'s `included` member: a check whose result "determines the
/// instance-level application status returned by DescribeApplicationStatus"
/// (`CreateApplicationStatusCheckRequest$Aggregation`). The full catalog is
/// `crate::ops::types::AGGREGATION_STATUS_ENUM_VALUES`.
pub const AGGREGATION_INCLUDED: &str = "included";

/// `AggregationStatusEnum`'s `excluded` member (see [`AGGREGATION_INCLUDED`]): "Checks
/// with `Aggregation` set to `excluded` do not affect this value"
/// (`ApplicationStatus$Status`).
pub const AGGREGATION_EXCLUDED: &str = "excluded";

/// A suppression of application status reporting for one instance.
///
/// `EnableApplicationStatusCheckSuppression` puts one in force and
/// `DisableApplicationStatusCheckSuppression` lifts it; while it is in force the
/// instance-level status is [`APPLICATION_STATUS_SUPPRESSED`] whatever the checks report.
#[derive(Debug, Clone)]
pub struct ApplicationStatusSuppression {
    /// When the suppression took effect: the `StatusSince` of the `suppressed` status it
    /// produces ("The date and time when the current status started",
    /// `ApplicationStatus$StatusSince`).
    pub since: SystemTime,
    /// When reporting resumes, reported as `ApplicationStatus$ResumeAt` ("The date and
    /// time when application status reporting resumes after suppression"). `None` when
    /// the request named no duration, which "suppresses health check results
    /// indefinitely until you call DisableApplicationStatusCheckSuppression"
    /// (`EnableApplicationStatusCheckSuppressionRequest$DurationSeconds`) — so there is
    /// no instant to report.
    pub resume_at: Option<SystemTime>,
}

impl ApplicationStatusSuppression {
    /// Whether this suppression is still in force at `now`.
    ///
    /// A suppression with an end stops suppressing once the clock has passed it, whether
    /// or not anybody called in the meantime, so a reader DERIVES that from the stored
    /// instant instead of writing the expiry back.
    pub fn in_force_at(&self, now: SystemTime) -> bool {
        self.resume_at.is_none_or(|resume_at| now < resume_at)
    }
}

/// What one instance's application health status is derived from.
///
/// The per-check results are stored as the MODELLED [`ApplicationStatusDetail`] the
/// describe reports, keyed by `ApplicationStatusCheckId`, so one check holds exactly one
/// result per instance and no projection can drop a member of it. The instance-level
/// status is NOT stored: it is a function of these details and of the suppression
/// ([`aggregated_application_status`]), so two operations cannot disagree about it.
#[derive(Debug, Clone, Default)]
pub struct InstanceApplicationStatusRecord {
    /// The result of every application status check associated with this instance, by
    /// `ApplicationStatusCheckId`.
    pub details: BTreeMap<String, ApplicationStatusDetail>,
    /// The suppression in force for this instance, if any.
    pub suppression: Option<ApplicationStatusSuppression>,
}

/// A modelled timestamp as a sortable key, so two of them can be ordered without
/// assuming anything about the timestamp type's own comparison traits.
///
/// The ONE definition of that key for this crate: the modelled timestamp type carries no
/// `Ord`, and comparing two instants through `f64` seconds would lose the precision a
/// `MillisecondDateTime` keeps, so every path that orders or windows timestamps — a
/// describe's `*TimeBegin`/`*TimeEnd` window included — compares what this returns.
pub fn date_time_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// When one check's current result started: its `StatusSince` ("The date and time when
/// the current status started"), falling back to its `StatusTimeStamp` for a result that
/// carries no `StatusSince`.
pub(super) fn detail_since(detail: &ApplicationStatusDetail) -> Option<::aws_smithy_types::DateTime> {
    detail.status_since.or(detail.status_time_stamp)
}

/// "The date and time of the last status update" (`ApplicationStatus$StatusTimeStamp`)
/// for an instance whose checks reported `details`: the most recent update any of them
/// carries, included or excluded — every check of the instance is one of its status
/// updates, while only the included ones decide the instance-level status.
pub fn last_application_status_update(
    details: &[ApplicationStatusDetail],
) -> Option<::aws_smithy_types::DateTime> {
    details
        .iter()
        .filter_map(|detail| detail.status_time_stamp.or(detail.check_update_time))
        .max_by_key(|at| date_time_key(*at))
}

/// The instance-level application status, and the instant it started, derived from the
/// per-check `details` of ONE instance.
///
/// `ApplicationStatus$Status`'s own documentation is the whole rule: `impaired` when "at
/// least one included check failed", `initializing` when "at least one included check is
/// initializing, and no included check is impaired", `insufficient-data` when "at least
/// one included check has insufficient data, and no included check is impaired or
/// initializing", `ok` when "all included checks passed", and `not-applicable` when "no
/// checks with `Aggregation` set to `included` apply to the instance" — a check reporting
/// `not-applicable` being one that does not apply, and "checks with `Aggregation` set to
/// `excluded` do not affect this value".
///
/// The instant is when that predicate began to hold, read off the per-check
/// `StatusSince`: the EARLIEST of the checks that satisfy an "at least one" predicate,
/// and for `ok` the LATEST of them, which is when the last check turned `passed`. For
/// `not-applicable` no check carries one, so the answer is `None` and the caller supplies
/// the instant the instance has had no applicable check since.
pub fn aggregated_application_status(
    details: &[ApplicationStatusDetail],
) -> (&'static str, Option<::aws_smithy_types::DateTime>) {
    let with_status = |status: &'static str| -> Vec<&ApplicationStatusDetail> {
        details
            .iter()
            .filter(|detail| detail.aggregation.as_deref() == Some(AGGREGATION_INCLUDED))
            .filter(|detail| detail.status.as_deref() == Some(status))
            .collect()
    };

    // The precedence the member documents, worst first: one failed check impairs the
    // instance however many others pass, and an initializing one outranks a check with
    // insufficient data.
    for (check_status, instance_status) in [
        (APPLICATION_STATUS_CHECK_FAILED, APPLICATION_STATUS_IMPAIRED),
        (
            APPLICATION_STATUS_CHECK_INITIALIZING,
            APPLICATION_STATUS_INITIALIZING,
        ),
        (
            APPLICATION_STATUS_CHECK_INSUFFICIENT_DATA,
            APPLICATION_STATUS_INSUFFICIENT_DATA,
        ),
    ] {
        let matching = with_status(check_status);
        if !matching.is_empty() {
            let started = matching
                .iter()
                .filter_map(|detail| detail_since(detail))
                .min_by_key(|at| date_time_key(*at));
            return (instance_status, started);
        }
    }

    // "ok – All included checks passed", which the instance has been only since the last
    // of them turned `passed`.
    let passed = with_status(APPLICATION_STATUS_CHECK_PASSED);
    if !passed.is_empty() {
        let started = passed
            .iter()
            .filter_map(|detail| detail_since(detail))
            .max_by_key(|at| date_time_key(*at));
        return (APPLICATION_STATUS_OK, started);
    }

    // No included check applies: the instance has none, or every one it has reports
    // `not-applicable` (or a status this emulator does not read), and an excluded check
    // is not one that applies either.
    (APPLICATION_STATUS_NOT_APPLICABLE, None)
}

/// The Region the Availability Zone named `zone_name` belongs to: the name without its
/// trailing zone letter.
///
/// A zone's name is the Region's name with a letter appended
/// ([`availability_zones_in_region`]), so this is the ONE reading of "which Region is this
/// zone in" — used by [`zone_id_for_zone_name`] beside it and by every record whose Region
/// is its zone's (a launched instance's root volume, its primary interface and the public
/// DNS name it reports), so a resource and the instance that holds it can never be placed
/// in two Regions. A name carrying no zone letter is already a Region name and is answered
/// unchanged.
pub fn region_of_zone_name(zone_name: &str) -> &str {
    zone_name
        .strip_suffix(|letter: char| letter.is_ascii_lowercase())
        .unwrap_or(zone_name)
}

/// The `ZoneId` of the Availability Zone named `zone_name`.
///
/// A zone's name and its id are the SAME derived pair ([`availability_zones_in_region`]),
/// so the id is read back out of the Region the name belongs to rather than stored a
/// second time beside it. `None` for a name no Region of that form has a zone for.
pub fn zone_id_for_zone_name(zone_name: &str) -> Option<String> {
    let region = region_of_zone_name(zone_name);
    // A name carrying no zone letter is a Region name, which names no zone.
    if region == zone_name {
        return None;
    }
    availability_zones_in_region(region)
        .into_iter()
        .find(|zone| zone.zone_name == zone_name)
        .map(|zone| zone.zone_id)
}

impl ComAmazonawsEc2Amazonec2State {
    /// The modelled `InstanceApplicationStatus` for one stored instance, as of `now`.
    ///
    /// Everything in it is derived from what the store holds — the instance's zone and
    /// tags, the per-check results written for it, and the suppression in force — so this
    /// is a READ: an instance nothing has reported a check for is reported
    /// `not-applicable`, which is the status the model documents for exactly that case,
    /// rather than given one here.
    pub fn instance_application_status(
        &self,
        instance: &InstanceRecord,
        now: SystemTime,
    ) -> InstanceApplicationStatus {
        let record = self.application_status.get(&instance.instance_id);
        let details: Vec<ApplicationStatusDetail> = record
            .map(|record| record.details.values().cloned().collect())
            .unwrap_or_default();
        // A suppression whose end the clock has passed suppresses nothing any more.
        let suppression = record
            .and_then(|record| record.suppression.as_ref())
            .filter(|suppression| suppression.in_force_at(now));

        let application_status = match suppression {
            // "suppressed – Application status reporting is suppressed for the
            // instance": the suppression, not the checks, is what the instance-level
            // status reports while it is in force, and `ResumeAt` is when reporting
            // resumes (absent while the suppression has no end).
            Some(suppression) => {
                let suppressed_since = system_time_to_date_time(suppression.since);
                ApplicationStatus {
                    status: Some(APPLICATION_STATUS_SUPPRESSED.to_string()),
                    // "The date and time of the last status update" is the latest of them
                    // whichever side it came from: a check that reported while the
                    // instance was suppressed is still a status update of it.
                    status_time_stamp: Some(
                        last_application_status_update(&details)
                            .filter(|updated| {
                                date_time_key(*updated) > date_time_key(suppressed_since)
                            })
                            .unwrap_or(suppressed_since),
                    ),
                    status_since: Some(suppressed_since),
                    resume_at: suppression.resume_at.map(system_time_to_date_time),
                    details: Some(details),
                }
            }
            None => {
                let (status, started) = aggregated_application_status(&details);
                // While no check has reported anything, the instance has had no
                // applicable check — and so this status — since it came into being.
                let launched_at = system_time_to_date_time(instance.launch_time);
                ApplicationStatus {
                    status: Some(status.to_string()),
                    status_time_stamp: Some(
                        last_application_status_update(&details).unwrap_or(launched_at),
                    ),
                    status_since: Some(started.unwrap_or(launched_at)),
                    // Reporting is not suppressed, so nothing is waiting to resume.
                    resume_at: None,
                    details: Some(details),
                }
            }
        };

        InstanceApplicationStatus {
            instance_id: Some(instance.instance_id.clone()),
            availability_zone: Some(instance.placement_az.clone()),
            availability_zone_id: zone_id_for_zone_name(&instance.placement_az),
            application_status: Some(application_status),
            tags: Some(tag_list(&instance.tags)),
        }
    }
}
