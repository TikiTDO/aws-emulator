// SPDX-License-Identifier: CC0-1.0

//! The organization surface: the delegated administrator one Region's
//! organization has designated, and the organization-wide configuration that says
//! which features its member accounts come up with.
//!
//! A delegated administrator configures, once per account per Region, what
//! GuardDuty does for the OTHER accounts in its organization: whether the service
//! itself is turned on for them and, per feature, whether that feature is. The
//! configuration is a property of the administrator's detector — every operation
//! on it is bound under `/detector/{DetectorId}/admin`
//! (`smithy_query(operation="DescribeOrganizationConfiguration")`) — so it hangs
//! off [`Detector`] and `DeleteDetector` drops it with the detector.
//!
//! Each switch is stored ONCE, as the feature that governs it, and read back
//! through either the `Features` view or the deprecated `DataSources` one, exactly
//! as the detector's own switches are ([`apply_data_sources`]).

use super::*;

/// The account one Region's organization has designated as its GuardDuty
/// delegated administrator.
///
/// The record carries only what a read reports plus the instant a removal falls
/// due: `AdminAccount` has an account id and a status and nothing else
/// (`smithy_query(shape="AdminAccount")`), and the status is a FUNCTION of the
/// pending removal ([`Self::status`]) rather than a field a caller can set to a
/// third value.
#[derive(Debug, Clone)]
pub struct OrganizationAdminAccountRecord {
    /// The designated account, as the designating request named it.
    pub admin_account_id: String,
    /// When a removal has been accepted, the instant the CLOCK owes it — until
    /// then the record reports the transient status, and from then on it is gone.
    /// `None` while no removal is pending.
    pub disable_due_millis: Option<i64>,
}

impl OrganizationAdminAccountRecord {
    /// `AdminAccount.AdminStatus` — "whether the account is enabled as the
    /// delegated administrator" (`smithy_query(shape="AdminAccount")`): the
    /// transient status while a removal is pending, [`ENABLED`] otherwise.
    pub fn status(&self) -> &'static str {
        if self.disable_due_millis.is_some() {
            ADMIN_DISABLE_IN_PROGRESS
        } else {
            ENABLED
        }
    }

    /// Has the removal this record carries fallen due by `now`?
    ///
    /// `false` while no removal is pending, and `false` for one still inside its
    /// span: that record is reported with the transient status instead.
    pub fn disable_settled(&self, now: i64) -> bool {
        self.disable_due_millis.is_some_and(|due| now >= due)
    }

    /// Accept a removal of this delegated administrator, due
    /// [`ADMIN_DISABLE_SPAN_MILLIS`] after `now`.
    ///
    /// Idempotent: a second removal of the same administrator leaves the instant
    /// the first one made due where it was, so repeating the call cannot postpone
    /// the removal for ever.
    pub fn begin_disable(&mut self, now: i64) {
        if self.disable_due_millis.is_none() {
            self.disable_due_millis = Some(now.saturating_add(ADMIN_DISABLE_SPAN_MILLIS));
        }
    }

    /// Turn around a removal still in flight: the record keeps the account it
    /// holds, reports [`ENABLED`] again and the clock owes nothing. `true` when a
    /// removal was pending, `false` when there was nothing to turn around.
    ///
    /// The other half of the rule [`EntitySetRecord::set_activation`] and
    /// [`ThreatIntelSetRecord::set_activation`] implement: asking a transitional
    /// record for the status it has already settled on, or is already
    /// transitioning to, owes nothing and answers `false`, while asking for the
    /// status it is transitioning AWAY from replaces whatever was owed, so a
    /// request that reverses a transition still running turns it around instead of
    /// being refused or queueing behind it.
    pub fn cancel_disable(&mut self) -> bool {
        self.disable_due_millis.take().is_some()
    }
}

/// The modelled `AdminAccount` a read reports for one delegated administrator.
///
/// Both members are optional and both are facts the record holds, so the
/// projection fills them and lives here once.
pub fn organization_admin_account_view(
    record: &OrganizationAdminAccountRecord,
) -> crate::ops::types::list_organization_admin_accounts_types::AdminAccount {
    crate::ops::types::list_organization_admin_accounts_types::AdminAccount {
        admin_account_id: Some(record.admin_account_id.clone()),
        admin_status: Some(record.status().to_string()),
    }
}

/// `AdminStatus.DISABLE_IN_PROGRESS`: the status a delegated administrator whose
/// removal has been accepted reports until the removal falls due. See
/// [`ENABLED`] for the other member of the enum.
pub const ADMIN_DISABLE_IN_PROGRESS: &str = "DISABLE_IN_PROGRESS";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ADMIN_STATUS_VALUES,
    ADMIN_DISABLE_IN_PROGRESS
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ADMIN_STATUS_VALUES,
    ENABLED
));

/// How long a delegated administrator whose removal was accepted stays
/// [`ADMIN_DISABLE_IN_PROGRESS`] before the CLOCK owes its removal.
///
/// AWS documents no duration for removing a delegated administrator, so this span
/// is CHOSEN — the same one [`ENTITY_SET_TRANSITION_MILLIS`] uses, and non-zero
/// for the same reason: a caller that lists the administrators straight after
/// asking for the removal still observes the transient status.
pub const ADMIN_DISABLE_SPAN_MILLIS: i64 = ENTITY_SET_TRANSITION_MILLIS;

/// How many Regions this store will hold a delegated administrator for.
///
/// Per Region the store holds at most one record, but the Region is read from the
/// request's credential scope ([`request_region`]), so without a ceiling on the
/// number of partitions a caller signing for invented Region names could make the
/// process retain a record per distinct Region string. Emulator-chosen, like
/// [`MAX_DETECTION_RULE_REGIONS`], and enforced before the designation writes
/// ([`ComAmazonawsGuarddutyGuarddutyapiserviceState::organization_admin_regions_exhausted`]).
pub const MAX_ORGANIZATION_ADMIN_REGIONS: usize = MAX_DETECTION_RULE_REGIONS;

/// The wire name of the `AdminAccountId` member, as both the designation and the
/// removal carry it (`smithy_query(operation="EnableOrganizationAdminAccount")`
/// renders the `jsonName` `adminAccountId`). It is what a rejection names.
pub const ADMIN_ACCOUNT_ID_MEMBER: &str = "adminAccountId";

/// Refusal of a designation made while the Region's organization already has a
/// delegated administrator.
///
/// A read reports `len 0..1` administrators
/// (`smithy_query(operation="ListOrganizationAdminAccounts")`), so there is room
/// for one: changing it means removing the current one first. The message states
/// the rule and quotes neither the caller's account id nor the stored one.
pub const ORGANIZATION_ADMIN_ALREADY_DESIGNATED_MESSAGE: &str = "The request is rejected because the organization already has a GuardDuty delegated administrator. Remove the current delegated administrator before designating another account.";

/// Refusal of a removal asked for while the Region's organization has no
/// delegated administrator at all.
pub const NO_ORGANIZATION_ADMIN_MESSAGE: &str =
    "The request is rejected because the organization has no GuardDuty delegated administrator.";

/// Refusal of a removal naming an account that is not the delegated administrator
/// the organization has.
pub const NOT_THE_ORGANIZATION_ADMIN_MESSAGE: &str = "The request is rejected because the given account ID is not the organization's GuardDuty delegated administrator.";

/// Refusal of a designation that would partition the delegated-administrator
/// store into one Region more than it holds
/// ([`MAX_ORGANIZATION_ADMIN_REGIONS`]). States the rule without quoting the
/// caller's Region back, like [`DETECTOR_REGION_LIMIT_MESSAGE`].
pub const ORGANIZATION_ADMIN_REGION_LIMIT_MESSAGE: &str = "The request is rejected because this emulator will not hold a GuardDuty delegated administrator for any further Regions.";

/// `AutoEnableMembers` / `OrgFeatureStatus`: nothing is enabled automatically.
///
/// Both enums carry the same three members
/// (`smithy_query(shape="AutoEnableMembers,OrgFeatureStatus")`) — one says whether
/// GuardDuty itself is auto-enabled for the organization's accounts, the other
/// whether one feature is — so one constant serves both.
pub const AUTO_ENABLE_NONE: &str = "NONE";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::AUTO_ENABLE_MEMBERS_VALUES,
    AUTO_ENABLE_NONE
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ORG_FEATURE_STATUS_VALUES,
    AUTO_ENABLE_NONE
));

/// `AutoEnableMembers` / `OrgFeatureStatus`: auto-enabled for the accounts that
/// JOIN the organization from now on. See [`AUTO_ENABLE_NONE`] for the enums.
pub const AUTO_ENABLE_NEW: &str = "NEW";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::AUTO_ENABLE_MEMBERS_VALUES,
    AUTO_ENABLE_NEW
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ORG_FEATURE_STATUS_VALUES,
    AUTO_ENABLE_NEW
));

/// `AutoEnableMembers` / `OrgFeatureStatus`: auto-enabled for EVERY account in the
/// organization — the accounts already in it as well as the ones that join. See
/// [`AUTO_ENABLE_NONE`] for the enums.
pub const AUTO_ENABLE_ALL: &str = "ALL";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::AUTO_ENABLE_MEMBERS_VALUES,
    AUTO_ENABLE_ALL
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ORG_FEATURE_STATUS_VALUES,
    AUTO_ENABLE_ALL
));

/// Every `OrgFeature` member, in the order a read reports them: the crate's
/// MODEL-DERIVED list, so a feature the model gains cannot be missed here.
pub const ORG_FEATURES: &[&str] = crate::ops::types::shared_types::ORG_FEATURE_VALUES;

/// The wire name of the `AutoEnableOrganizationMembers` member
/// (`smithy_query(operation="UpdateOrganizationConfiguration")`).
pub const AUTO_ENABLE_ORGANIZATION_MEMBERS_MEMBER: &str = "autoEnableOrganizationMembers";

/// The wire name of the deprecated organization-level `AutoEnable` member.
pub const AUTO_ENABLE_MEMBER: &str = "autoEnable";

/// The message a request that names both auto-enable views, or neither, is refused
/// with: "You must provide a value for either `autoEnableOrganizationMembers` or
/// `autoEnable`, but not both"
/// (`smithy_query(query="doc",shape="UpdateOrganizationConfiguration")`).
pub fn auto_enable_exclusive_message() -> String {
    format!(
        "The request is rejected because exactly one of {AUTO_ENABLE_ORGANIZATION_MEMBERS_MEMBER} and {AUTO_ENABLE_MEMBER} must be specified."
    )
}

/// The `OrgFeatureStatus` the deprecated BOOLEAN auto-enable view names.
///
/// A view that can only say yes or no cannot distinguish the two "yes" members, so
/// yes is [`AUTO_ENABLE_NEW`]: the deprecated flag is about the accounts that join
/// ("whether GuardDuty is automatically enabled for accounts added to the
/// organization" — `smithy_query(query="doc",shape="DescribeOrganizationConfigurationResponse",member="AutoEnable")`),
/// which is exactly what `NEW` means.
pub fn org_status_for_flag(auto_enable: bool) -> &'static str {
    if auto_enable {
        AUTO_ENABLE_NEW
    } else {
        AUTO_ENABLE_NONE
    }
}

/// How a stored `OrgFeatureStatus` reads back through the deprecated BOOLEAN view:
/// anything other than [`AUTO_ENABLE_NONE`] is an auto-enablement, so it reads
/// back as true.
pub fn org_status_flag(status: &str) -> bool {
    status != AUTO_ENABLE_NONE
}

/// One `OrgFeature`'s auto-enablement, as the store holds it.
#[derive(Debug, Clone)]
pub struct OrgFeatureRecord {
    /// An `OrgFeature` member — one of [`ORG_FEATURES`].
    pub name: String,
    /// `OrgFeatureStatus`: whether this feature is enabled automatically for the
    /// organization's accounts, and for which of them.
    pub auto_enable: String,
    /// The feature's additional configurations, as a request set them. Empty until
    /// one does: GuardDuty documents no additional configuration a feature carries
    /// by default, so nothing is invented here.
    pub additional_configuration: Vec<OrgAdditionalConfigRecord>,
}

/// One `OrgFeatureAdditionalConfiguration`'s auto-enablement, as the store holds
/// it.
#[derive(Debug, Clone)]
pub struct OrgAdditionalConfigRecord {
    /// An `OrgFeatureAdditionalConfiguration` member
    /// (`smithy_query(shape="OrgFeatureAdditionalConfiguration")`), which the
    /// boundary refuses a request outside of before a handler sees it.
    pub name: String,
    /// `OrgFeatureStatus`, as on the feature that carries it.
    pub auto_enable: String,
}

/// The organization configuration of one delegated administrator's detector.
#[derive(Debug, Clone)]
pub struct OrganizationConfiguration {
    /// `AutoEnableMembers`: whether GuardDuty itself is enabled automatically for
    /// the organization's accounts.
    pub auto_enable_members: String,
    /// One record per [`ORG_FEATURES`] member, in that order, so a read reports the
    /// whole set once each and a page token means the same thing on the next call.
    pub features: Vec<OrgFeatureRecord>,
}

impl Default for OrganizationConfiguration {
    /// What an administrator's detector reports before any
    /// `UpdateOrganizationConfiguration`: nothing is enabled automatically, which
    /// is what [`AUTO_ENABLE_NONE`] says for GuardDuty itself and for every
    /// feature. The administrator "must manage GuardDuty for each account in the
    /// organization individually"
    /// (`smithy_query(query="doc",shape="UpdateOrganizationConfigurationRequest",member="AutoEnableOrganizationMembers")`)
    /// until a request says otherwise.
    fn default() -> Self {
        Self {
            auto_enable_members: AUTO_ENABLE_NONE.to_string(),
            features: ORG_FEATURES
                .iter()
                .map(|name| OrgFeatureRecord {
                    name: (*name).to_string(),
                    auto_enable: AUTO_ENABLE_NONE.to_string(),
                    additional_configuration: Vec::new(),
                })
                .collect(),
        }
    }
}

impl OrganizationConfiguration {
    /// Is GuardDuty itself auto-enabled for EVERY account in the organization?
    ///
    /// The administrator that configured this keeps GuardDuty on for each of its
    /// organization's accounts, so the organization — not a single API call — owns
    /// each member relationship: an account has to leave the organization before
    /// its relationship can end or its monitoring stop. Every operation that would
    /// do either reads this before it writes anything, on the administrator's side
    /// of the relationship (see [`organization_managed_members_message`]) and on
    /// the member's own (see [`organization_managed_administrator_message`]).
    pub fn auto_enables_all_members(&self) -> bool {
        self.auto_enable_members == AUTO_ENABLE_ALL
    }

    /// One feature's `OrgFeatureStatus`, or [`AUTO_ENABLE_NONE`] when the
    /// configuration does not carry that feature.
    pub fn feature_auto_enable(&self, name: &str) -> &str {
        self.features
            .iter()
            .find(|feature| feature.name == name)
            .map_or(AUTO_ENABLE_NONE, |feature| feature.auto_enable.as_str())
    }

    /// Set one feature's `OrgFeatureStatus` and hand back the record, so a caller
    /// can write its additional configurations too.
    ///
    /// `None` for a name that is not an [`ORG_FEATURES`] member: the stored set is
    /// exactly that set and stays in its order, which is the invariant
    /// [`Self::features`] states and the page token a read issues depends on, so a
    /// name outside it is IGNORED rather than appended. Nothing can reach here with
    /// one anyway — `OrgFeature` is an enum, so the boundary refuses a value outside
    /// it before a handler runs — and the record for a member the stored list somehow
    /// lacks is inserted at its place in that order.
    pub fn set_feature(&mut self, name: &str, auto_enable: &str) -> Option<&mut OrgFeatureRecord> {
        let rank = |feature: &str| ORG_FEATURES.iter().position(|member| *member == feature);
        let wanted = rank(name)?;
        let position = match self
            .features
            .iter()
            .position(|feature| feature.name == name)
        {
            Some(position) => position,
            None => {
                let position = self
                    .features
                    .iter()
                    .position(|feature| rank(&feature.name).is_none_or(|held| held > wanted))
                    .unwrap_or(self.features.len());
                self.features.insert(
                    position,
                    OrgFeatureRecord {
                        name: name.to_string(),
                        auto_enable: auto_enable.to_string(),
                        additional_configuration: Vec::new(),
                    },
                );
                position
            }
        };
        let feature = &mut self.features[position];
        feature.auto_enable = auto_enable.to_string();
        Some(feature)
    }
}

/// The action `DisassociateMembers` attempts, as the refusal below names it.
pub const DISASSOCIATE_MEMBER_ACTION: &str = "disassociate";

/// The action `StopMonitoringMembers` attempts, as the refusal below names it.
pub const STOP_MONITORING_MEMBER_ACTION: &str = "stop monitoring";

/// The action `DeleteMembers` attempts, as the refusal below names it.
pub const DELETE_MEMBER_ACTION: &str = "delete";

/// The message a call that would end or suspend a member relationship the
/// ORGANIZATION manages is refused with, naming the `action` it attempted — one of
/// [`DISASSOCIATE_MEMBER_ACTION`], [`STOP_MONITORING_MEMBER_ACTION`] and
/// [`DELETE_MEMBER_ACTION`].
///
/// "With `autoEnableOrganizationMembers` configuration for your organization set to
/// `ALL`, you'll receive an error if you attempt to disassociate a member account
/// before removing them from your organization"
/// (`smithy_query(query="doc",shape="DisassociateMembers")`), and the same
/// configuration refuses stopping monitoring
/// (`smithy_query(query="doc",shape="StopMonitoringMembers")`) and disabling
/// GuardDuty for a member (`smithy_query(query="doc",shape="DeleteMembers")`): one
/// precondition over the operations that take a member relationship away, so one
/// message states it and names the remedy the documentation gives.
///
/// The precondition holds for the whole call rather than per account: a member
/// record says which accounts this administrator holds, and nothing records which
/// of them joined through the organization rather than through an invitation, so
/// every member of an administrator that auto-enables all of its organization's
/// accounts is one of them. A later change that records how a member was added can
/// narrow it to those accounts.
pub fn organization_managed_members_message(action: &str) -> String {
    format!(
        "The request is rejected because the current account cannot {action} the given member account ID while {AUTO_ENABLE_ORGANIZATION_MEMBERS_MEMBER} is set to {AUTO_ENABLE_ALL} for the organization. Remove the account from the organization first."
    )
}

/// The message a MEMBER account's own request to leave the administrator
/// relationship the ORGANIZATION manages is refused with.
///
/// The same precondition [`organization_managed_members_message`] states, worded
/// for the side of the relationship that names no member account — the caller is
/// the member: "with `autoEnableOrganizationMembers` configuration for your
/// organization set to `ALL`, you'll receive an error if you attempt to disable
/// GuardDuty in a member account"
/// (`smithy_query(query="doc",shape="DisassociateFromAdministratorAccount")`). It
/// names the same remedy the documentation gives for the administrator-side calls,
/// because it is the same one: the account leaves the organization first.
pub fn organization_managed_administrator_message() -> String {
    format!(
        "The request is rejected because the current account cannot disassociate from its administrator account while {AUTO_ENABLE_ORGANIZATION_MEMBERS_MEMBER} is set to {AUTO_ENABLE_ALL} for the organization. Remove the account from the organization first."
    )
}

/// Refuse an organization `Features` list the operation documents as
/// contradictory, with the message the refusal carries.
///
/// Every value in the list is an enum member the boundary already checked, so what
/// is left is the rule that spans two entries, which
/// [`refuse_runtime_monitoring_overlap`] holds for a detector's list and an
/// organization's alike.
///
/// The rule is judged on the auto-enablement each feature ENDS UP with, not on what
/// the request mentions: the request's value where it states one, and `configuration`'s
/// stored value otherwise (a list that omits a feature, or names it without an
/// `AutoEnable`, leaves that switch alone — see
/// [`apply_org_feature_configurations`]). So enabling one of the two while the other
/// is already stored enabled is refused, and naming one of them without a status
/// while enabling the other is not.
pub fn validate_org_feature_configurations(
    configuration: &OrganizationConfiguration,
    features: Option<&Vec<crate::ops::types::OrganizationFeatureConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    let enabled = |wanted: &str| {
        // The LAST entry for a name is the one that lands, as applying the list
        // writes them in order.
        let requested = features
            .iter()
            .rfind(|feature| feature.name.as_deref() == Some(wanted))
            .and_then(|feature| feature.auto_enable.as_deref());
        requested.unwrap_or_else(|| configuration.feature_auto_enable(wanted)) != AUTO_ENABLE_NONE
    };
    refuse_runtime_monitoring_overlap(enabled)
}

/// Write a `Features` list onto an organization configuration. A feature the list
/// does not name keeps the auto-enablement it already had, the way
/// [`apply_feature_configurations`] leaves the detector's own switches — and so
/// does a feature the list names WITHOUT an `AutoEnable`, which is an optional
/// member (`smithy_query(shape="OrganizationFeatureConfiguration")`): an entry that
/// states no auto-enablement states nothing to change about it, and may still carry
/// the additional configurations below.
///
/// A feature's additional configurations are stored ONE PER
/// `OrgFeatureAdditionalConfiguration` member, keyed by that name, the way the
/// feature switches themselves are stored one per `OrgFeature` member
/// ([`OrganizationConfiguration::set_feature`]) and the way the member side keys the
/// same switches ([`additional_configuration_records`]): a name a request repeats is
/// ONE configuration and the last entry naming it is the one that lands, as for the
/// feature switches above. That is also what keeps the stored configuration no
/// longer than the two model catalogs however long a request's lists are, so every
/// later walk of it — a read, and the write that carries an auto-enablement onto the
/// administrator's member accounts — is bounded by the model rather than by the
/// request.
pub fn apply_org_feature_configurations(
    configuration: &mut OrganizationConfiguration,
    features: Option<&Vec<crate::ops::types::OrganizationFeatureConfiguration>>,
) {
    let Some(features) = features else { return };
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        let auto_enable = feature
            .auto_enable
            .as_deref()
            .unwrap_or_else(|| configuration.feature_auto_enable(name))
            .to_string();
        // `None` for a name no `OrgFeature` carries, which the boundary refuses
        // before this runs: such an entry writes nothing at all.
        let Some(record) = configuration.set_feature(name, &auto_enable) else {
            continue;
        };
        if let Some(additional) = feature.additional_configuration.as_ref() {
            let mut by_name: BTreeMap<&str, OrgAdditionalConfigRecord> = BTreeMap::new();
            for configuration in additional {
                let Some(name) = configuration.name.as_deref() else {
                    continue;
                };
                by_name.insert(
                    name,
                    OrgAdditionalConfigRecord {
                        name: name.to_string(),
                        auto_enable: configuration
                            .auto_enable
                            .as_deref()
                            .unwrap_or(AUTO_ENABLE_NONE)
                            .to_string(),
                    },
                );
            }
            record.additional_configuration = by_name.into_values().collect();
        }
    }
}

/// Write the deprecated organization `DataSources` view onto an organization
/// configuration.
///
/// `DataSources` is "deprecated, use Features instead"
/// (`smithy_query(operation="UpdateOrganizationConfiguration")`), so each node is
/// stored as the `OrgFeature` that governs it — one source of truth, read back
/// through either view, exactly as [`apply_data_sources`] stores the detector's own
/// data sources. The boolean becomes an `OrgFeatureStatus` through
/// [`org_status_for_flag`].
pub fn apply_org_data_sources(
    configuration: &mut OrganizationConfiguration,
    data_sources: Option<&crate::ops::types::OrganizationDataSourceConfigurations>,
) {
    let Some(data_sources) = data_sources else {
        return;
    };
    if let Some(s3) = data_sources.s3_logs.as_ref() {
        configuration.set_feature(FEATURE_S3_DATA_EVENTS, org_status_for_flag(s3.auto_enable));
    }
    if let Some(kubernetes) = data_sources.kubernetes.as_ref() {
        configuration.set_feature(
            FEATURE_EKS_AUDIT_LOGS,
            org_status_for_flag(kubernetes.audit_logs.auto_enable),
        );
    }
    if let Some(auto_enable) = data_sources
        .malware_protection
        .as_ref()
        .and_then(|malware| malware.scan_ec2_instance_with_findings.as_ref())
        .and_then(|scan| scan.ebs_volumes.as_ref())
        .and_then(|volumes| volumes.auto_enable)
    {
        configuration.set_feature(
            FEATURE_EBS_MALWARE_PROTECTION,
            org_status_for_flag(auto_enable),
        );
    }
}
