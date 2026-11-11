// SPDX-License-Identifier: CC0-1.0

//! The groups Amazon RDS provides ITSELF in every account and region: the default DB
//! parameter groups and the default option groups.
//!
//! These are not account state — no request creates them, no request deletes them and no
//! request changes them — so they are not stored in
//! [`super::ComAmazonawsRdsAmazonrdsv19State`]. They are DERIVED, here, from the one
//! engine-version catalogue this service publishes
//! (`crate::ops::describe_db_engine_versions`): RDS creates a default DB parameter group
//! for every `DBParameterGroupFamily` it serves and a default option group for every
//! engine and major engine version, so the set of groups follows from the set of engine
//! versions rather than being a second list that could disagree with it.
//!
//! A read reports them beside the account's own groups ([`super::visible_db_parameter_groups`],
//! [`super::visible_option_groups`]), and a request that would delete or change one is
//! refused, because the group is the service's and not the account's.

use super::*;

/// How Amazon RDS describes a default DB parameter group: the one sentence it fills with
/// the family the group belongs to, the same way it describes a default option group
/// ([`default_option_group_description`]).
pub fn default_db_parameter_group_description(family: &str) -> String {
    format!("Default parameter group for {family}")
}

/// How Amazon RDS describes a default option group: the engine and the major engine
/// version the group belongs to.
pub fn default_option_group_description(engine: &str, major_engine_version: &str) -> String {
    format!("Default option group for {engine} {major_engine_version}")
}

/// The prefix of every option group name Amazon RDS provides itself.
///
/// RDS names such a group `default:<engine>-<major engine version>`
/// ([`super::default_option_group_name`]). The colon is outside the alphabet a created
/// group's name may use (`super::check_identifier` admits letters, numbers and hyphens
/// only), so a name of this form can only ever be one of RDS's own groups and never a
/// record this store holds — exactly the argument
/// [`super::DEFAULT_PARAMETER_GROUP_PREFIX`] rests on for parameter groups.
pub const DEFAULT_OPTION_GROUP_PREFIX: &str = "default:";

/// Whether `name` names one of the option groups Amazon RDS provides itself; the
/// option-group counterpart of [`super::is_default_parameter_group_name`].
pub fn is_default_option_group_name(name: &str) -> bool {
    name.to_ascii_lowercase()
        .starts_with(DEFAULT_OPTION_GROUP_PREFIX)
}

/// How Amazon RDS refuses a request that would delete one of the groups it provides
/// itself: the `Default <type> cannot be deleted: <name>` sentence, carrying the group the
/// request named.
///
/// `subject` is the resource type RDS names in that sentence — `DBParameterGroup` for a DB
/// parameter group, `OptionGroup` for an option group — so the two refusals are the one
/// wording with the one variable part each.
pub fn default_group_cannot_be_deleted_message(subject: &str, name: &str) -> String {
    format!("Default {subject} cannot be deleted: {name}")
}

/// The resource type [`default_group_cannot_be_deleted_message`] names for a DB parameter
/// group.
pub const DB_PARAMETER_GROUP_SUBJECT: &str = "DBParameterGroup";

/// The resource type [`default_group_cannot_be_deleted_message`] names for a DB CLUSTER
/// parameter group.
pub const DB_CLUSTER_PARAMETER_GROUP_SUBJECT: &str = "DBClusterParameterGroup";

/// The resource type [`default_group_cannot_be_deleted_message`] names for an option
/// group.
pub const OPTION_GROUP_SUBJECT: &str = "OptionGroup";

/// How Amazon RDS refuses a request that would change one of the option groups it provides
/// itself: ONE fixed sentence naming neither the group nor the member, the option-group
/// counterpart of [`super::CANNOT_MODIFY_DEFAULT_PARAMETER_GROUP_MESSAGE`].
pub const CANNOT_MODIFY_DEFAULT_OPTION_GROUP_MESSAGE: &str =
    "Cannot modify a default option group.";

impl DbParameterGroupRecord {
    /// The default DB parameter group of `family`, as RDS provides it in `region`: the
    /// group named [`super::default_db_parameter_group_name`], carrying the family it
    /// belongs to and the description RDS gives it.
    ///
    /// It holds NO parameter of its own. The engine's defaults belong to the family, which
    /// publishes them ([`super::published_db_parameters`]), and a describe of this group
    /// reports them sourced `engine-default` exactly as it does for a group a request
    /// created — which is what "the default parameters and values for the specified DB
    /// parameter group family" are.
    pub fn account_default(region: &str, family: &str) -> Self {
        Self {
            name: default_db_parameter_group_name(family),
            family: family.to_string(),
            description: default_db_parameter_group_description(family),
            region: region.to_string(),
            tags: Vec::new(),
            parameters: BTreeMap::new(),
        }
    }
}

impl OptionGroupRecord {
    /// The default option group of `engine` at `major_engine_version`, as RDS provides it
    /// in `region`: the group named [`super::default_option_group_name`], holding no option
    /// — a default group is the empty one every DB instance of that engine and version
    /// starts with.
    pub fn account_default(region: &str, engine: &str, major_engine_version: &str) -> Self {
        Self {
            name: default_option_group_name(engine, major_engine_version),
            region: region.to_string(),
            description: default_option_group_description(engine, major_engine_version),
            engine_name: engine.to_string(),
            major_engine_version: major_engine_version.to_string(),
            tags: Vec::new(),
            options: BTreeMap::new(),
            copied_from: None,
        }
    }
}

/// Every default DB parameter group the account holds in `region`, in name order: one per
/// `DBParameterGroupFamily` this service publishes an engine version of.
pub fn default_db_parameter_groups(region: &str) -> Vec<DbParameterGroupRecord> {
    let mut groups: Vec<DbParameterGroupRecord> =
        crate::ops::describe_db_engine_versions::published_parameter_group_families()
            .into_iter()
            .map(|family| DbParameterGroupRecord::account_default(region, family))
            .collect();
    groups.sort_by(|left, right| left.name.cmp(&right.name));
    groups
}

/// The default DB parameter group `named` in `region`, or `None` when the name is not one
/// of RDS's own.
///
/// What follows the prefix has to BE a family — an engine this front door serves followed by
/// that engine's version ([`crate::ops::describe_engine_default_parameters::family_engine`]
/// carries that reading) — but it need not be a family the engine-version catalogue
/// publishes a row for: that catalogue is this emulator's published sample of the engine
/// versions AWS serves, so reporting `default.<a family it lists no version of>` as missing
/// would deny a group RDS does provide. A value that names no family at all names no
/// group either.
pub fn default_db_parameter_group(region: &str, named: &str) -> Option<DbParameterGroupRecord> {
    let folded = named.to_ascii_lowercase();
    let family = folded.strip_prefix(DEFAULT_PARAMETER_GROUP_PREFIX)?;
    crate::ops::describe_engine_default_parameters::family_engine(family)
        .map(|_| DbParameterGroupRecord::account_default(region, family))
}

/// Every default option group the account holds in `region`, in name order: one per engine
/// and major engine version this service publishes, for the engines an option group can be
/// associated with at all ([`super::OPTION_GROUP_ENGINE_NAMES`]).
pub fn default_option_groups(region: &str) -> Vec<OptionGroupRecord> {
    let mut groups: Vec<OptionGroupRecord> =
        crate::ops::describe_db_engine_versions::published_engine_major_versions()
            .into_iter()
            .filter(|(engine, _)| OPTION_GROUP_ENGINE_NAMES.contains(engine))
            .map(|(engine, major)| OptionGroupRecord::account_default(region, engine, major))
            .collect();
    groups.sort_by(|left, right| left.name.cmp(&right.name));
    groups
}

/// The default option group `named` in `region`, or `None` when the name is not one of
/// RDS's own.
///
/// Unlike a default parameter group, this resolves only against the groups the
/// engine-version catalogue accounts for: an option group carries its engine and its major
/// engine version as MEMBERS, and the name alone cannot say which engine
/// `default:oracle-se2-cdb-19` belongs to (`oracle-se2` at version `cdb-19` reads the same
/// way), so a group is answered only where the published catalogue says which engine and
/// version it is for.
pub fn default_option_group(region: &str, named: &str) -> Option<OptionGroupRecord> {
    if !is_default_option_group_name(named) {
        return None;
    }
    crate::ops::describe_db_engine_versions::published_engine_major_versions()
        .into_iter()
        .filter(|(engine, _)| OPTION_GROUP_ENGINE_NAMES.contains(engine))
        .find(|(engine, major)| default_option_group_name(engine, major).eq_ignore_ascii_case(named))
        .map(|(engine, major)| OptionGroupRecord::account_default(region, engine, major))
}
