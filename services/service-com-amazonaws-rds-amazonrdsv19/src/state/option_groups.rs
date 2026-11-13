// SPDX-License-Identifier: CC0-1.0

use super::*;

/// One option a `ModifyOptionGroup` call added to a group: the caller's CONFIGURATION
/// of it.
///
/// The option's own facts — its description, whether it is permanent or persistent,
/// the settings it publishes and each setting's default, data type and allowed values
/// — are NOT copied in here. They are catalogue data ([`option_catalog`]), which
/// [`OptionRecord::to_wire`] reads at projection time, so a stored group cannot drift
/// from the catalogue the way a stored copy could.
#[derive(Debug, Clone, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct OptionRecord {
    /// The catalogue's own spelling of the option's name.
    pub name: String,
    /// `OptionConfiguration$Port`, when the request named one; otherwise the option's
    /// published port, if it has one.
    pub port: Option<i32>,
    /// `OptionConfiguration$OptionVersion`, when the request named one.
    pub version: Option<String>,
    /// `OptionConfiguration$DBSecurityGroupMemberships`.
    pub db_security_group_memberships: Vec<String>,
    /// `OptionConfiguration$VpcSecurityGroupMemberships`.
    pub vpc_security_group_memberships: Vec<String>,
    /// The values the caller set, each with the setting's catalogue name, in the order
    /// the requests that set them named them — only for the settings it set. A setting
    /// absent here reports the catalogue's default, which is what makes `Value` fall back
    /// to `DefaultValue`, and the order is kept because it is the order the configured
    /// settings are REPORTED in (see [`OptionRecord::to_wire`]).
    pub settings: Vec<(String, String)>,
}

impl OptionRecord {
    /// The value the caller set for the setting `name`, or `None` where it set none and
    /// the setting reports its published default.
    pub fn setting_value(&self, name: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|(stored, _)| stored == name)
            .map(|(_, value)| value.as_str())
    }

    /// Record `value` for the setting `name`, which must be the catalogue's own spelling.
    ///
    /// A setting the same request, or a later one, sets again keeps the position its
    /// first configuration gave it and reports the newest value: configuring a setting is
    /// not the same event as configuring it for the first time.
    pub fn set_setting(&mut self, name: &str, value: &str) {
        match self.settings.iter_mut().find(|(stored, _)| stored == name) {
            Some(stored) => stored.1 = value.to_string(),
            None => self.settings.push((name.to_string(), value.to_string())),
        }
    }

    /// Project the stored option onto the modelled `Option` wire shape, in the context
    /// of the group's engine: the engine is half of the catalogue key, because an
    /// option one engine publishes is a different catalogue entry than another's.
    pub fn to_wire(&self, engine_name: &str) -> Option2 {
        let facts = option_catalog::option_facts(engine_name, &self.name);
        let settings = facts
            .map(|facts| {
                // A setting the caller CONFIGURED is reported ahead of the ones left at
                // their published default, in the order the request named them; the rest
                // follow in the order the option publishes them. That is the order the
                // recorded service reports an option's settings in: a group whose option
                // was added with one setting configured reports that setting first and
                // the remaining ones in catalogue order, and a group that configured none
                // reports the catalogue order unchanged.
                let configured = self
                    .settings
                    .iter()
                    .filter_map(|(name, _)| facts.setting(name));
                let defaulted = facts
                    .settings
                    .iter()
                    .filter(|setting| self.setting_value(setting.name).is_none());
                configured
                    .chain(defaulted)
                    .map(|setting| OptionSetting {
                        name: Some(setting.name.to_string()),
                        // The value the caller set, else the published default: that
                        // is what "The current value of the option setting" is for a
                        // setting nobody has set.
                        value: Some(
                            self.setting_value(setting.name)
                                .map(str::to_string)
                                .unwrap_or_else(|| setting.default_value.to_string()),
                        ),
                        default_value: Some(setting.default_value.to_string()),
                        description: Some(setting.description.to_string()),
                        apply_type: Some(setting.apply_type.to_string()),
                        data_type: Some(setting.data_type.to_string()),
                        // A free-form setting publishes no value list, and reports no
                        // `AllowedValues` member rather than an empty one.
                        allowed_values: (!setting.allowed_values.is_empty())
                            .then(|| setting.allowed_values.to_string()),
                        is_modifiable: Some(setting.is_modifiable),
                        is_collection: Some(setting.is_collection),
                    })
                    .collect()
            })
            // Unreachable with a record this service stored: `ModifyOptionGroup`
            // refuses an option the group's engine does not publish, so every stored
            // option has catalogue facts. An option whose entry has since been
            // dropped from the catalogue reports what the record itself holds rather
            // than invented metadata.
            .unwrap_or_default();
        Option2 {
            option_name: Some(self.name.clone()),
            option_description: facts.map(|facts| facts.description.to_string()),
            persistent: facts.map(|facts| facts.persistent),
            permanent: facts.map(|facts| facts.permanent),
            // "If required, the port configured for this option to use" — so no port
            // at all for an option that requires none.
            port: self.port.or_else(|| facts.and_then(|facts| facts.port)),
            option_version: self.version.clone(),
            option_settings: Some(settings),
            // Both membership lists are reported even when the option was configured
            // with none: an empty list says the option opened access to nothing, which
            // is not the same claim as no list at all.
            db_security_group_memberships: Some(
                self.db_security_group_memberships
                    .iter()
                    .map(|name| DBSecurityGroupMembership {
                        db_security_group_name: Some(name.clone()),
                        status: Some("authorized".to_string()),
                    })
                    .collect(),
            ),
            vpc_security_group_memberships: Some(
                self.vpc_security_group_memberships
                    .iter()
                    .map(|id| VpcSecurityGroupMembership {
                        vpc_security_group_id: Some(id.clone()),
                        status: Some("active".to_string()),
                    })
                    .collect(),
            ),
        }
    }
}

/// The message the `InvalidOptionGroupStateFault` carries where an option group cannot be
/// deleted because a DB instance is still associated with it ("You can't delete an option
/// group that is associated with a DB instance").
///
/// Amazon RDS words it from the REQUEST alone: the group name the request asked to delete,
/// SINGLE-QUOTED, in the one sentence that states the group is in use — not by whom. The
/// instance holding the group is this service's state and the sentence does not report it,
/// so nothing but the requested name is interpolated.
pub fn option_group_in_use_message(name: &str) -> String {
    format!("The option group '{name}' cannot be deleted because it is in use.")
}

/// A stored option group: everything `CreateOptionGroup` persists about one, plus the
/// options `ModifyOptionGroup` has added to it.
///
/// The ARN is NOT stored as text: it is a function of the stored region and name,
/// which [`OptionGroupRecord::to_wire`] computes, so it cannot drift from the record
/// the way a stored copy could.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct OptionGroupRecord {
    pub name: String,
    /// The region the group was created in; the scope of its ARN.
    pub region: String,
    pub description: String,
    pub engine_name: String,
    pub major_engine_version: String,
    pub tags: Vec<Tag>,
    /// The options `ModifyOptionGroup` added, by the catalogue's own spelling of each
    /// option's name. Ordered, so the group reports its options the same way on every
    /// read, and keyed by name, so adding an option twice updates its configuration
    /// instead of storing a second copy of it.
    pub options: BTreeMap<String, OptionRecord>,
    /// Where this group was copied from, for a group `CopyOptionGroup` produced, and
    /// `None` for one created from scratch — whose `SourceOptionGroup`,
    /// `SourceAccountId` and `CopyTimestamp` are then reported not at all, because
    /// nothing was copied for them to describe.
    pub copied_from: Option<CopiedOptionGroup>,
}

/// The copy provenance of an option group `CopyOptionGroup` produced: the three members
/// `OptionGroup` publishes for a copied group, recorded ONCE when the copy is taken, so
/// every later read of the group reports the same answer.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CopiedOptionGroup {
    /// `SourceOptionGroup` — "the name of the option group from which this option group
    /// is copied": the source's own name, whichever form the request named it by.
    pub source_option_group: String,
    /// `SourceAccountId` — "the Amazon Web Services account ID for the option group from
    /// which this option group is copied".
    pub source_account_id: String,
    /// `CopyTimestamp` — "when the option group was copied", fixed at the instant the
    /// copy was taken.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub copy_timestamp: ::aws_smithy_types::DateTime,
}

impl OptionGroupRecord {
    /// `arn:aws:rds:<region>:<account>:og:<name>` — `og` is the resource type RDS
    /// gives an option group in its ARN format.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(ARN_NAMESPACE, &self.region, &format!("og:{}", self.name))
    }

    /// Project the stored record onto the modelled `OptionGroup` output shape.
    pub fn to_wire(&self) -> OptionGroup {
        let options: Vec<Option2> = self
            .options
            .values()
            .map(|option| option.to_wire(&self.engine_name))
            .collect();
        OptionGroup {
            option_group_name: Some(self.name.clone()),
            option_group_description: Some(self.description.clone()),
            engine_name: Some(self.engine_name.clone()),
            major_engine_version: Some(self.major_engine_version.clone()),
            // A group nothing has added an option to reports an EMPTY list rather than
            // no list at all: the group exists and has nothing in it.
            // `AllowsVpcAndNonVpcInstanceMemberships` — "Indicates whether this option
            // group can be applied to both VPC and non-VPC instances" — is ALWAYS
            // reported, so a caller can tell a group it may apply anywhere from one it
            // may not. A group holding no option constrains nothing and can be applied
            // to both; once it holds an option, that option's own placement
            // requirements bind the group to instances of one network type, so the
            // answer is `false`.
            allows_vpc_and_non_vpc_instance_memberships: Some(options.is_empty()),
            options: Some(options),
            option_group_arn: Some(self.arn()),
            // `SourceOptionGroup`, `SourceAccountId` and `CopyTimestamp` describe a group
            // that was COPIED from another one, so they are reported exactly for a group
            // this service holds that provenance for, and not at all for one created from
            // scratch — a group nothing was copied to make has no source to name.
            source_option_group: self
                .copied_from
                .as_ref()
                .map(|copy| copy.source_option_group.clone()),
            source_account_id: self
                .copied_from
                .as_ref()
                .map(|copy| copy.source_account_id.clone()),
            copy_timestamp: self.copied_from.as_ref().map(|copy| copy.copy_timestamp),
            // `VpcId` belongs to a group bound to ONE VPC, which nothing here creates:
            // "If AllowsVpcAndNonVpcInstanceMemberships is false, this field is blank.
            // If AllowsVpcAndNonVpcInstanceMemberships is true and this field is blank,
            // then this option group can be applied to both VPC and non-VPC
            // instances." No group here is bound to a VPC, so the member is left
            // unreported rather than carrying a made-up value.
            ..OptionGroup::default()
        }
    }
}
