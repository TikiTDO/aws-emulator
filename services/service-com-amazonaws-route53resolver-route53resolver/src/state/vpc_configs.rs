// SPDX-License-Identifier: CC0-1.0

use super::*;

/// `Validation` values, the model's whole enum
/// (`smithy_query(query="shape",shape="Validation")`, re-exported as
/// [`crate::ops::types::VALIDATION_VALUES`]): the new DNSSEC validation setting
/// a request asks a VPC's configuration to take.
pub const VALIDATION_ENABLE: &str = "ENABLE";
pub const VALIDATION_DISABLE: &str = "DISABLE";
pub const VALIDATION_USE_LOCAL_RESOURCE_SETTING: &str = "USE_LOCAL_RESOURCE_SETTING";

/// `ResolverDNSSECValidationStatus` values the DNSSEC validation slice reports,
/// the model's whole enum
/// (`smithy_query(query="shape",shape="ResolverDNSSECValidationStatus")`,
/// re-exported as
/// [`crate::ops::types::RESOLVER_D_N_S_S_E_C_VALIDATION_STATUS_VALUES`]).
///
/// `ValidationStatus`' doc pairs each settled value with the transient one that
/// precedes it: "ENABLING: DNSSEC validation is being enabled but is not
/// complete", "ENABLED: DNSSEC validation is enabled", "DISABLING: DNSSEC
/// validation is being disabled but is not complete", "DISABLED: DNSSEC
/// validation is disabled".
pub const DNSSEC_STATUS_ENABLING: &str = "ENABLING";
pub const DNSSEC_STATUS_ENABLED: &str = "ENABLED";
pub const DNSSEC_STATUS_DISABLING: &str = "DISABLING";
pub const DNSSEC_STATUS_DISABLED: &str = "DISABLED";
pub const DNSSEC_STATUS_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING: &str =
    "UPDATING_TO_USE_LOCAL_RESOURCE_SETTING";
pub const DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING: &str = "USE_LOCAL_RESOURCE_SETTING";

/// The most configurations for DNSSEC validation this store holds records for,
/// for the same reason as [`MAX_STORED_ENDPOINTS`]: every write operation of
/// this service is reachable by any caller, so the store must not grow without
/// a bound. One record is allocated per distinct `ResourceId` whose validation
/// setting is moved off its default, and an update of a `ResourceId` already
/// stored stays allowed, because it allocates nothing.
pub const MAX_STORED_DNSSEC_CONFIGS: usize = 1_000;

/// The pair of `ResolverDNSSECValidationStatus` values one requested
/// `Validation` moves a VPC's configuration through: the transient status it
/// reports while the change is in flight, and the settled status it reports
/// once the clock owes the move.
///
/// `Validation`'s doc — "Be aware that it can take time for a validation status
/// change to be completed" — is why the transient status exists, and
/// `ValidationStatus`' doc is what pairs each settled value with it.
/// `USE_LOCAL_RESOURCE_SETTING` takes the enum's own
/// `UPDATING_TO_USE_LOCAL_RESOURCE_SETTING` as its in-flight value. `None` for
/// a value the model's `Validation` enum does not declare: a handler refuses
/// that rather than treating it as one of these.
pub fn dnssec_validation_transition(validation: &str) -> Option<(&'static str, &'static str)> {
    match validation {
        VALIDATION_ENABLE => Some((DNSSEC_STATUS_ENABLING, DNSSEC_STATUS_ENABLED)),
        VALIDATION_DISABLE => Some((DNSSEC_STATUS_DISABLING, DNSSEC_STATUS_DISABLED)),
        VALIDATION_USE_LOCAL_RESOURCE_SETTING => Some((
            DNSSEC_STATUS_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            DNSSEC_STATUS_USE_LOCAL_RESOURCE_SETTING,
        )),
        _ => None,
    }
}

/// Whether the account holds NO VPC under `resource_id`.
///
/// The per-VPC singleton configurations this service maintains — the DNS
/// Firewall configuration, the Resolver behavior configuration and the
/// configuration for DNSSEC validation — belong to a VPC of the caller's
/// account, and which VPCs those are is a fact of the account's own Amazon EC2
/// inventory ([`unresolvable_vpc_id`], the one place that resolution happens)
/// rather than a value this service's caller supplies. So this is the
/// precondition every one of those operations checks before it reports a
/// configuration or writes one: an id naming no VPC of the account addresses no
/// configuration at all, and is refused.
///
/// LOCKING. As [`unresolvable_vpc_id`]: the Amazon EC2 store lock is taken for
/// this one pass and released before returning, so a handler calls this with no
/// lock of its own held.
pub fn vpc_configuration_resource_is_unknown<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_id: &str,
) -> bool {
    unresolvable_vpc_id(engine, [resource_id]).is_some()
}

/// Whether the account holds NO VPC and NO Route 53 Profile under
/// `resource_id`.
///
/// `ResolverConfig$ResourceId` is "the ID of the Amazon Virtual Private Cloud
/// VPC or a Route 53 Profile that you're configuring Resolver for", so the
/// Resolver BEHAVIOR configuration belongs to either of them — which is one
/// member more than the DNS Firewall and DNSSEC validation configurations,
/// whose own `ResourceId` docs name a VPC alone. Both inventories belong to the
/// service that owns the resource: the VPCs of the account's own Amazon EC2
/// inventory ([`vpc_configuration_resource_is_unknown`]) and the Profiles
/// Route 53 Profiles holds, read through its own accessors.
///
/// LOCKING. As [`unresolvable_vpc_id`]: each store's lock is taken for one pass
/// and released before returning, and no two are held at once, so a handler
/// calls this with no lock of its own held.
pub fn resolver_configuration_resource_is_unknown<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_id: &str,
) -> bool {
    vpc_configuration_resource_is_unknown(engine, resource_id)
        && !account_holds_profile(engine, resource_id)
}

/// Whether the account's own Route 53 Profiles inventory holds the Profile
/// `profile_id` names — the one `GetProfile` answers from, keyed by the `Id`
/// that service mints.
fn account_holds_profile<Eng: Send + Sync + 'static>(engine: &Eng, profile_id: &str) -> bool {
    let Some(state) =
        ::service_com_amazonaws_route53profiles_route53profiles::state::service_state(engine)
    else {
        // No store for that service on this engine is no inventory to resolve
        // against; the VPC half of the precondition has then already made the
        // same allowance ([`unresolvable_ec2_id`]), so nothing is refused for
        // want of an inventory.
        return false;
    };
    let held = state.lock().profiles.contains_key(profile_id);
    held
}

/// The `ResourceNotFoundException` the DNS Firewall per-VPC configuration
/// operations answer for a `ResourceId` that names no VPC of the account.
///
/// Built here so `GetFirewallConfig` and `UpdateFirewallConfig` cannot drift
/// apart: both report the can't-find sentence naming the id the REQUEST sent,
/// under the DNS Firewall slice's own code for a resource the caller cannot
/// address ([`RSLVR_CODE_RESOURCE_NOT_FOUND`], which
/// [`resource_not_found_message`] renders), closed with the trace clause of the
/// request ([`traced_message`]). No `ResourceType` is reported
/// beside it: the resource that could not be found is a VPC of the account's
/// Amazon EC2 inventory, which is not one of this service's own modelled
/// resource types.
pub fn firewall_config_resource_not_found(
    resource_id: &str,
    trace_root: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(traced_message(
            resource_not_found_message(resource_id),
            trace_root,
        )),
        resource_type: None,
    }
}

/// The same refusal for the Resolver behavior configuration operations
/// (`GetResolverConfig`, `UpdateResolverConfig`), whose recorded responses carry
/// the one can't-find sentence under the Resolver code
/// ([`RSLVR_CODE_RESOLVER_RESOURCE_NOT_FOUND`]) rather than the DNS Firewall
/// one, and carry it ALONE: the trace clause the DNS Firewall slice closes its
/// refusal with ([`firewall_config_resource_not_found`]) is not part of this
/// one, as the recorded responses of the two show.
pub fn resolver_config_resource_not_found(resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(coded_resource_not_found_message(
            RSLVR_CODE_RESOLVER_RESOURCE_NOT_FOUND,
            resource_id,
        )),
        resource_type: None,
    }
}

/// The `Id` of a VPC's configuration for DNSSEC validation. AWS names one
/// `rdsc-<hex>`; every result fits `ResourceId` (length 1..64).
///
/// Derived from the VPC id rather than taken from the id sequence, for the same
/// reason as [`resolver_config_id`]: no operation creates a configuration for
/// DNSSEC validation (`UpdateResolverDnssecConfig` only moves the validation
/// setting of the one a VPC already has), so one exists for a VPC whether or not
/// anything has ever written to it, and its id has to be a function of the VPC
/// it belongs to. The same VPC therefore reports the same `Id` from every call,
/// before and after an update.
pub fn dnssec_config_id(resource_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/dnssec-config/v1");
    feed_field(&mut hasher, resource_id.as_bytes());
    format!("rdsc-{}", hex::encode(&hasher.finalize()[..8]))
}

/// One configuration for DNSSEC validation, as stored.
///
/// The reported `ValidationStatus` is computed from the stored status pair and
/// the due instant, so a read never has to write to settle one — the same shape
/// as [`ResolverEndpointRecord`].
///
/// A configuration for DNSSEC validation is a per-VPC singleton the service
/// maintains itself — there is no operation that creates or deletes one — so a
/// record exists only for a VPC whose validation setting was moved off its
/// default; every other VPC reports the default one
/// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_dnssec_config`]
/// derives. `Id` and `OwnerId` are therefore the same whether the record exists
/// or not, so a VPC's configuration does not change identity when it is updated
/// — the same shape as [`ResolverConfigRecord`].
#[derive(Debug, Clone)]
pub struct ResolverDnssecConfigRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// order `ListResolverDnssecConfigs` reports the configurations in,
    /// independent of the hash order of the map that holds them.
    ///
    /// `0` on the default configuration
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_dnssec_config`]
    /// derives for a VPC the store holds no record for: the id sequence starts
    /// at 1, so a stored record never carries it.
    pub seq: u64,
    /// The `Id` of this configuration, [`dnssec_config_id`] of its VPC, stable
    /// for the life of the VPC: a later update of the same VPC keeps it.
    pub id: String,
    /// `OwnerId`: "The owner account ID of the virtual private cloud (VPC)".
    pub owner_id: String,
    /// `ResourceId`: the VPC this configuration is for.
    pub resource_id: String,
    /// The `ValidationStatus` reported while the requested change is in flight.
    pub status: String,
    /// The `ValidationStatus` reported once the move the clock owes falls due.
    pub settled_status: String,
    /// When that move falls due.
    pub settled_at: Instant,
}

impl ResolverDnssecConfigRecord {
    /// This configuration's `ValidationStatus` as of `now`: the transient
    /// status until the move the clock owes falls due, the settled one after
    /// it.
    pub fn validation_status_at(&self, now: Instant) -> &str {
        if now < self.settled_at {
            &self.status
        } else {
            &self.settled_status
        }
    }

    /// Project the record onto the modelled `ResolverDnssecConfig`.
    pub fn to_dnssec_config(&self, now: Instant) -> ResolverDnssecConfig {
        ResolverDnssecConfig {
            id: Some(self.id.clone()),
            owner_id: Some(self.owner_id.clone()),
            resource_id: Some(self.resource_id.clone()),
            validation_status: Some(self.validation_status_at(now).to_string()),
        }
    }

    /// Whether this configuration satisfies one `ListResolverDnssecConfigs`
    /// filter: does the value it presents for `key` appear in `values`?
    pub fn matches_filter(
        &self,
        key: DnssecConfigFilterKey,
        values: &HashSet<String>,
        now: Instant,
    ) -> bool {
        match key {
            DnssecConfigFilterKey::Id => values.contains(self.id.as_str()),
            DnssecConfigFilterKey::OwnerId => values.contains(self.owner_id.as_str()),
            DnssecConfigFilterKey::ResourceId => values.contains(self.resource_id.as_str()),
            DnssecConfigFilterKey::ValidationStatus => values.contains(self.validation_status_at(now)),
        }
    }
}

/// The member a `ListResolverDnssecConfigs` filter selects on.
///
/// `Filter$Name`'s doc ("The valid values for `Name` depend on the action that
/// you're including the filter in") names no catalog for this operation, so the
/// catalog is the members the listed shape presents
/// (`smithy_query(query="shape",shape="ResolverDnssecConfig")`): `Id`,
/// `OwnerId`, `ResourceId`, `ValidationStatus`. A name outside it is refused
/// rather than silently ignored, so a caller is never handed a page that does
/// not answer the query it asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnssecConfigFilterKey {
    Id,
    OwnerId,
    ResourceId,
    ValidationStatus,
}

impl DnssecConfigFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Id => "Id",
            Self::OwnerId => "OwnerId",
            Self::ResourceId => "ResourceId",
            Self::ValidationStatus => "ValidationStatus",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the catalog.
    /// The name is folded by [`fold_filter_name`], as [`FilterKey::parse`] folds
    /// the endpoint catalog's names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "ID" => Some(Self::Id),
            "OWNERID" => Some(Self::OwnerId),
            "RESOURCEID" => Some(Self::ResourceId),
            "VALIDATIONSTATUS" => Some(Self::ValidationStatus),
            _ => None,
        }
    }
}

/// The cursor scope of one `ListResolverDnssecConfigs` query, for the same
/// reason and in the same shape as [`endpoint_list_scope`]'s.
pub fn dnssec_config_list_scope(filters: &[(DnssecConfigFilterKey, HashSet<String>)]) -> String {
    filter_list_scope(
        "ListResolverDnssecConfigs",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    )
}

/// `AutodefinedReverseFlag` values, the model's whole enum
/// (`smithy_query(query="shape",shape="AutodefinedReverseFlag")`, re-exported as
/// [`crate::ops::types::AUTODEFINED_REVERSE_FLAG_VALUES`]): the setting a
/// request asks a VPC's Resolver configuration to take for the autodefined
/// rules for reverse DNS lookups.
pub const AUTODEFINED_REVERSE_FLAG_ENABLE: &str = "ENABLE";
pub const AUTODEFINED_REVERSE_FLAG_DISABLE: &str = "DISABLE";
pub const AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING: &str = "USE_LOCAL_RESOURCE_SETTING";

/// `ResolverAutodefinedReverseStatus` values a VPC's Resolver configuration
/// reports, the model's whole enum
/// (`smithy_query(query="shape",shape="ResolverAutodefinedReverseStatus")`,
/// re-exported as
/// [`crate::ops::types::RESOLVER_AUTODEFINED_REVERSE_STATUS_VALUES`]).
///
/// `ResolverConfig$AutodefinedReverse`'s doc pairs each settled value with the
/// transient one that precedes it: "ENABLING: Autodefined rules for reverse DNS
/// lookups are being enabled but are not complete", "ENABLED: Autodefined rules
/// for reverse DNS lookups are enabled", "DISABLING: Autodefined rules for
/// reverse DNS lookups are being disabled but are not complete", "DISABLED:
/// Autodefined rules for reverse DNS lookups are disabled".
pub const AUTODEFINED_REVERSE_ENABLING: &str = "ENABLING";
pub const AUTODEFINED_REVERSE_ENABLED: &str = "ENABLED";
pub const AUTODEFINED_REVERSE_DISABLING: &str = "DISABLING";
pub const AUTODEFINED_REVERSE_DISABLED: &str = "DISABLED";
pub const AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING: &str =
    "UPDATING_TO_USE_LOCAL_RESOURCE_SETTING";
pub const AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING: &str = "USE_LOCAL_RESOURCE_SETTING";

/// `LimitExceededException$ResourceType` for a VPC's Resolver behavior
/// configuration: the modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_RESOLVER_CONFIG: &str = "ResolverConfig";

/// The most VPC Resolver configurations this store holds records for, for the
/// same reason as [`MAX_STORED_FIREWALL_CONFIGS`]: `UpdateResolverConfig`
/// admits any `ResourceId`, so without a bound a caller looping updates over
/// fresh VPC ids would grow one emulator process without limit. An update that
/// would need a record past this count is refused and writes nothing; one for a
/// VPC the store already holds a record for allocates nothing and stays
/// allowed, and so does a read of any VPC, which stores nothing at all.
pub const MAX_STORED_RESOLVER_CONFIGS: usize = 1_000;

/// The pair of `ResolverAutodefinedReverseStatus` values one requested
/// `AutodefinedReverseFlag` moves a VPC's Resolver configuration through: the
/// transient status it reports while the change is in flight, and the settled
/// status it reports once the clock owes the move.
///
/// `AutodefinedReverseFlag`'s doc — "It can take some time for the status change
/// to be completed" — is why the transient status exists, and
/// `ResolverConfig$AutodefinedReverse`'s doc is what pairs each settled value
/// with it. `USE_LOCAL_RESOURCE_SETTING` takes the status enum's own
/// `UPDATING_TO_USE_LOCAL_RESOURCE_SETTING` as its in-flight value. `None` for a
/// value the model's `AutodefinedReverseFlag` enum does not declare: a handler
/// refuses that rather than treating it as one of these.
pub fn autodefined_reverse_transition(flag: &str) -> Option<(&'static str, &'static str)> {
    match flag {
        AUTODEFINED_REVERSE_FLAG_ENABLE => {
            Some((AUTODEFINED_REVERSE_ENABLING, AUTODEFINED_REVERSE_ENABLED))
        }
        AUTODEFINED_REVERSE_FLAG_DISABLE => {
            Some((AUTODEFINED_REVERSE_DISABLING, AUTODEFINED_REVERSE_DISABLED))
        }
        AUTODEFINED_REVERSE_FLAG_USE_LOCAL_RESOURCE_SETTING => Some((
            AUTODEFINED_REVERSE_UPDATING_TO_USE_LOCAL_RESOURCE_SETTING,
            AUTODEFINED_REVERSE_USE_LOCAL_RESOURCE_SETTING,
        )),
        _ => None,
    }
}

/// The Resolver behavior configuration of ONE VPC, as stored.
///
/// A VPC's Resolver configuration is a singleton the service maintains itself —
/// there is no operation that creates or deletes one — so a record exists only
/// for a VPC whose configuration was moved off its default; every other VPC
/// reports the default one
/// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_config`]
/// derives. `Id` and `OwnerId` are therefore the same whether the record exists
/// or not, so a VPC's configuration does not change identity when it is
/// updated — the same shape as [`FirewallConfigRecord`].
#[derive(Debug, Clone)]
pub struct ResolverConfigRecord {
    /// Creation order, for the list operation's pages (the same role as
    /// [`FirewallConfigRecord::seq`]).
    ///
    /// `0` on the default configuration
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_config`]
    /// derives for a VPC the store holds no record for: the id sequence starts
    /// at 1, so a stored record never carries it.
    pub seq: u64,
    /// The `Id` of this configuration, [`resolver_config_id`] of its VPC.
    pub id: String,
    /// `ResourceId`: "The ID of the Amazon Virtual Private Cloud VPC or a Route
    /// 53 Profile that you're configuring Resolver for."
    pub resource_id: String,
    /// `OwnerId`: "The owner account ID of the Amazon Virtual Private Cloud
    /// VPC."
    pub owner_id: String,
    /// The `AutodefinedReverse` status reported while the requested change is
    /// in flight.
    pub status: String,
    /// The `AutodefinedReverse` status reported once the move the clock owes
    /// falls due.
    pub settled_status: String,
    /// When that move falls due.
    pub settled_at: Instant,
}

impl ResolverConfigRecord {
    /// This configuration's `AutodefinedReverse` as of `now`: the transient
    /// status until the move the clock owes falls due, the settled one after
    /// it. Computed, so a read never has to write to settle one.
    pub fn autodefined_reverse_at(&self, now: Instant) -> &str {
        if now < self.settled_at {
            &self.status
        } else {
            &self.settled_status
        }
    }

    /// Project the record onto the modelled `ResolverConfig` shape the Resolver
    /// configuration operations report.
    pub fn to_resolver_config(&self, now: Instant) -> ResolverConfig {
        ResolverConfig {
            id: Some(self.id.clone()),
            resource_id: Some(self.resource_id.clone()),
            owner_id: Some(self.owner_id.clone()),
            autodefined_reverse: Some(self.autodefined_reverse_at(now).to_string()),
        }
    }
}
