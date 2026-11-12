// SPDX-License-Identifier: CC0-1.0

use super::*;

/// `Parameter$Source` for a value nobody changed: the parameter still carries
/// its family's engine default.
///
/// Source: `smithy_query(query="doc", shape="Parameter", member="Source")` —
/// "The source of the parameter value, such as \"engine-default\" or \"user\"".
pub const PARAMETER_SOURCE_ENGINE_DEFAULT: &str = "engine-default";

/// `Parameter$Source` for a value a caller set on the group.
pub const PARAMETER_SOURCE_USER: &str = "user";

/// The `Source` values a `DescribeClusterParameters` request may filter on, in
/// the documented order.
///
/// Source: `smithy_query(query="doc", shape="DescribeClusterParametersMessage",
/// member="Source")` — "Valid Values: user | engine-default".
pub const PARAMETER_SOURCES: [&str; 2] = [PARAMETER_SOURCE_USER, PARAMETER_SOURCE_ENGINE_DEFAULT];

/// `ClusterParameterGroupStatus$ParameterApplyStatus` /
/// `ClusterParameterStatus$ParameterApplyStatus` for a value the database
/// already carries.
///
/// Source: `smithy_query(query="doc", shape="ClusterParameterStatus",
/// member="ParameterApplyStatus")` — "`in-sync`: The parameter value is in sync
/// with the database."
pub const PARAMETER_APPLY_STATUS_IN_SYNC: &str = "in-sync";

/// The same member for a value an associated cluster only picks up when it is
/// next rebooted.
///
/// Source: the same doc sentence as [`PARAMETER_APPLY_STATUS_IN_SYNC`] —
/// "`pending-reboot`: The parameter value will be applied after the cluster
/// reboots."
pub const PARAMETER_APPLY_STATUS_PENDING_REBOOT: &str = "pending-reboot";

/// What a change to what a cluster parameter group HOLDS reports as the group's
/// `ParameterGroupStatus`: one sentence for both the modify and the reset, which
/// are the two operations that change it and the two that answer
/// `ClusterParameterGroupNameMessage`.
///
/// Source: `smithy_query(query="doc", shape="ClusterParameterGroupNameMessage",
/// member="ParameterGroupStatus")` — "if you made a change to a parameter group
/// name-value pair, then the change COULD be pending a reboot of an associated
/// cluster" — which is exactly what this sentence says: a dynamic-only change is
/// applied to the associated clusters straight away, while a static one waits
/// for their next reboot, as
/// `smithy_query(query="doc", operation="ResetClusterParameterGroup")`'s "For
/// parameter changes to take effect you must reboot any associated clusters"
/// has it. The status does NOT depend on which parameters the request carried:
/// it names both outcomes so a caller reads the one its own change falls under,
/// which is why one constant serves both operations.
pub const PARAMETER_GROUP_STATUS_UPDATED: &str =
    "Your parameter group has been updated. If you changed only dynamic parameters, associated clusters are being modified now. If you changed static parameters, all updates, including dynamic parameters, will be applied when you reboot the associated clusters.";

/// How many parameters one `ModifyClusterParameterGroup` request may carry.
///
/// Source: `smithy_query(query="doc", shape="ModifyClusterParameterGroupMessage",
/// member="Parameters")` — "A maximum of 20 parameters can be modified in a
/// single request."
pub const MAX_MODIFIABLE_PARAMETERS: usize = 20;

/// How many parameters one `ResetClusterParameterGroup` request may name.
///
/// Source: `smithy_query(query="doc", shape="ResetClusterParameterGroupMessage",
/// member="Parameters")` — "Constraints: A maximum of 20 parameters can be reset
/// in a single request."
pub const MAX_RESETTABLE_PARAMETERS: usize = 20;

/// The parameter the workload management (WLM) configuration is held in.
pub const WLM_JSON_CONFIGURATION: &str = "wlm_json_configuration";

/// The documented maximum length of one parameter's value, in characters, for
/// the parameters Redshift publishes one for — and `None` for every other, whose
/// value is bounded only by the modelled `length 0..` of `Parameter$ParameterValue`.
///
/// Source: `smithy_query(query="doc", shape="Parameter",
/// member="ParameterValue")` — "If ParameterName is wlm_json_configuration, then
/// the maximum size of ParameterValue is 8000 characters."
pub fn parameter_value_max_len(name: &str) -> Option<usize> {
    (name == WLM_JSON_CONFIGURATION).then_some(8_000)
}

/// Why the value of a parameter whose value is a JSON DOCUMENT is not one, or
/// `Ok(())` for a value that is — and for every parameter that holds a scalar,
/// which this rule says nothing about.
///
/// `wlm_json_configuration` holds the workload management configuration, which
/// is a JSON list of query queues (`smithy_query(query="doc", shape="Parameter",
/// member="ParameterValue")` calls out that one parameter's value as the JSON
/// configuration, which is why it alone publishes a size). A value that cannot
/// be read as that list configures no queue at all, so it is refused rather than
/// stored for a cluster to fail on later. The parameter publishes no
/// `AllowedValues`, so the domain cannot be stated as a value set — which is why
/// the check lives here beside [`EngineParameter::admits`] rather than in it.
pub fn json_parameter_value_refusal(name: &str, value: &str) -> Result<(), String> {
    if name != WLM_JSON_CONFIGURATION {
        return Ok(());
    }
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Array(_)) => Ok(()),
        _ => Err(format!(
            "the value of '{name}' must be a JSON list of workload management queues."
        )),
    }
}

/// Why a request naming a parameter the group's family does not publish is
/// refused, naming the `ParameterName` it carried.
///
/// One sentence for the write and the reset, so one condition reads the same
/// whichever operation reports it: the name is not a parameter of this family,
/// so there is no parameter to set or to put back.
pub fn parameter_not_found_message(parameter_name: &str) -> String {
    format!("Could not find parameter with name: {parameter_name}")
}

/// `Parameter$ApplyType` for a parameter an associated cluster picks up without
/// being rebooted.
///
/// Source: `smithy_query(query="shape", shape="ParameterApplyType")` — the enum
/// declares exactly `static` and `dynamic`.
pub const APPLY_TYPE_DYNAMIC: &str = "dynamic";

/// `Parameter$ApplyType` for a parameter whose new value an associated cluster
/// only sees after a reboot.
pub const APPLY_TYPE_STATIC: &str = "static";

/// `Parameter$DataType` for a parameter that takes `true` or `false`.
pub const DATA_TYPE_BOOLEAN: &str = "boolean";
/// `Parameter$DataType` for a parameter that takes a whole number.
pub const DATA_TYPE_INTEGER: &str = "integer";
/// `Parameter$DataType` for a parameter that takes free text.
pub const DATA_TYPE_STRING: &str = "string";

/// The two values every boolean parameter takes, as Redshift publishes the set
/// in `Parameter$AllowedValues` ("The valid range of values for the
/// parameter").
pub(super) const BOOLEAN_ALLOWED_VALUES: &str = "true,false";

/// One engine default parameter: a setting every cluster parameter group of a
/// published family publishes, with the value it carries before anybody changes
/// it.
///
/// This is the CATALOG entry, not a stored record: a group persists only the
/// values a caller changed (see [`StoredParameterGroup::parameter_values`]), so
/// the description, data type, allowed values, apply type and modifiability a
/// read reports come from here for every group of the family at once.
#[derive(Debug, Clone, Copy)]
pub struct EngineParameter {
    /// `Parameter$ParameterName`, as the engine spells it in its own
    /// configuration.
    pub name: &'static str,
    /// `Parameter$ParameterValue` before any change: the family's engine
    /// default.
    pub default_value: &'static str,
    /// `Parameter$Description` — "A description of the parameter".
    pub description: &'static str,
    /// `Parameter$DataType` — one of [`DATA_TYPE_BOOLEAN`],
    /// [`DATA_TYPE_INTEGER`] or [`DATA_TYPE_STRING`].
    pub data_type: &'static str,
    /// `Parameter$AllowedValues` — the published value set, or `None` for a
    /// parameter the engine itself parses and publishes no set for.
    pub allowed_values: Option<&'static str>,
    /// `Parameter$ApplyType` — [`APPLY_TYPE_STATIC`] when an associated cluster
    /// only sees a new value after a reboot, [`APPLY_TYPE_DYNAMIC`] otherwise.
    pub apply_type: &'static str,
    /// `Parameter$IsModifiable` — "If true, the parameter can be modified."
    pub is_modifiable: bool,
}

impl EngineParameter {
    /// The modelled `Parameter` this setting reports for one group.
    ///
    /// `override_value` is the value that group PERSISTED for it, `None` when the
    /// group never changed it — which is exactly what decides `Source`: a value a
    /// caller set reports [`PARAMETER_SOURCE_USER`], an untouched one
    /// [`PARAMETER_SOURCE_ENGINE_DEFAULT`].
    pub fn to_api(
        &self,
        override_value: Option<&str>,
        minimum_engine_version: Option<&str>,
    ) -> Parameter {
        Parameter {
            parameter_name: Some(self.name.to_string()),
            parameter_value: Some(override_value.unwrap_or(self.default_value).to_string()),
            description: Some(self.description.to_string()),
            source: Some(
                if override_value.is_some() {
                    PARAMETER_SOURCE_USER
                } else {
                    PARAMETER_SOURCE_ENGINE_DEFAULT
                }
                .to_string(),
            ),
            data_type: Some(self.data_type.to_string()),
            allowed_values: self.allowed_values.map(str::to_string),
            apply_type: Some(self.apply_type.to_string()),
            is_modifiable: Some(self.is_modifiable),
            minimum_engine_version: minimum_engine_version.map(str::to_string),
        }
    }

    /// Whether the published `AllowedValues` set admits `value`.
    ///
    /// Source: `smithy_query(query="doc", shape="Parameter",
    /// member="AllowedValues")` — "The valid range of values for the parameter".
    /// Redshift publishes that set as a comma-separated list whose items are
    /// either a literal value (`true,false`) or, for a numeric parameter, an
    /// INCLUSIVE range written `low-high` with a possibly negative low (`-15-2`,
    /// `0-10`, `0,100-2147483647`). A parameter that publishes no set admits
    /// every value the engine itself parses, so it admits whatever it is given.
    pub fn admits(&self, value: &str) -> bool {
        let Some(allowed) = self.allowed_values else {
            return true;
        };
        let numeric = self.data_type == DATA_TYPE_INTEGER;
        allowed.split(',').map(str::trim).any(|item| {
            if item == value {
                return true;
            }
            if !numeric {
                return false;
            }
            let Some((low, high)) = allowed_range(item) else {
                return false;
            };
            value
                .parse::<i64>()
                .is_ok_and(|wanted| (low..=high).contains(&wanted))
        })
    }

    /// The inclusive bounds the published `AllowedValues` set spans, for a
    /// parameter whose values are NUMBERS; `None` for one that publishes no set
    /// or holds something other than a number.
    ///
    /// The set is published as a comma-separated list whose items are each a
    /// single value or an inclusive `low-high` range (see [`Self::admits`]), so
    /// the span of the whole set runs from its lowest low to its highest high —
    /// which is the range a refusal reports, whatever shape the set has.
    pub fn published_range(&self) -> Option<(i64, i64)> {
        if self.data_type != DATA_TYPE_INTEGER {
            return None;
        }
        let mut bounds: Option<(i64, i64)> = None;
        for item in self.allowed_values?.split(',').map(str::trim) {
            let (low, high) = allowed_range(item)
                .or_else(|| item.parse::<i64>().ok().map(|single| (single, single)))?;
            bounds = Some(match bounds {
                Some((lowest, highest)) => (lowest.min(low), highest.max(high)),
                None => (low, high),
            });
        }
        bounds
    }
}

/// Why the value a request carried for `parameter` is not one it admits, or
/// `None` for a value it does admit.
///
/// Amazon Redshift words a parameter-value refusal from the PARAMETER, never
/// from where the value sat in the request, and picks the sentence by what is
/// wrong with the value:
///
/// * a value that is not of the parameter's own `Parameter$DataType` at all —
///   anything but a published boolean literal for a `boolean`, anything that is
///   not a whole number for an `integer` — is refused on its FORM, naming the
///   data type and the value the request carried;
/// * a whole number the published `Parameter$AllowedValues` range does not
///   contain is refused on its MAGNITUDE, naming the parameter, the value and
///   the bounds the catalogue advertises ([`EngineParameter::published_range`]).
///
/// One function for every path that sets a parameter value, so the refusal a
/// caller reads depends on the value it sent and not on which operation it sent
/// it to.
pub fn parameter_value_refusal(parameter: &EngineParameter, value: &str) -> Option<String> {
    if parameter.admits(value) {
        return None;
    }
    if let (true, Some((low, high))) = (
        value.parse::<i64>().is_ok(),
        parameter.published_range(),
    ) {
        return Some(format!(
            "The {} of {value} is outside of the acceptable range of {low}-{high}",
            parameter.name
        ));
    }
    Some(format!(
        "parameter value is not a valid {}:{value}",
        parameter.data_type
    ))
}

/// One `AllowedValues` item read as an inclusive numeric range, or `None` when
/// the item is not one.
///
/// The separator is the first `-` AFTER the first character, because the low
/// bound itself may be negative (`-15-2` is -15 through 2).
pub(super) fn allowed_range(item: &str) -> Option<(i64, i64)> {
    let separator = item.char_indices().skip(1).find(|&(_, c)| c == '-')?.0;
    let low = item[..separator].parse::<i64>().ok()?;
    let high = item[separator + 1..].parse::<i64>().ok()?;
    Some((low, high))
}

/// The engine default parameters a cluster parameter group of a published family
/// reports, in `ParameterName` order — which is the order a page walks, so the
/// `Marker` cursor stays a stable offset into one sequence.
///
/// Source: what the engine itself publishes for the one family this service
/// offers — `aws redshift describe-default-cluster-parameters
/// --parameter-group-family redshift-1.0`, the same catalog the Amazon Redshift
/// Cluster Management Guide's "Amazon Redshift parameter groups"
/// default-parameter table lists. Each entry's `Description`, `ApplyType`,
/// `DataType` and `AllowedValues` are that output's, VERBATIM, including its own
/// wording and spacing: a client renders the description as it arrives and reads
/// `ApplyType` to decide whether a change needs a reboot, so neither is the
/// emulator's to rephrase. The entries whose change an associated cluster sees
/// only after a reboot are published `static` (`enable_case_sensitive_identifier`,
/// `enable_user_activity_logging`, `require_ssl`, `statement_timeout`,
/// `use_fips_ssl`, `wlm_json_configuration`); every other setting applies
/// dynamically. Every setting in the catalog is one a caller may change, hence
/// [`EngineParameter::is_modifiable`] on all of them.
pub const ENGINE_DEFAULT_PARAMETERS: &[EngineParameter] = &[
    EngineParameter {
        name: "auto_analyze",
        default_value: "true",
        description: "Collects table statistics automatically in the background.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "auto_mv",
        default_value: "true",
        description: "Creates and refreshes materialized views automatically.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "datestyle",
        default_value: "ISO, MDY",
        description: "Sets the display format for date and time values.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "enable_case_sensitive_identifier",
        default_value: "false",
        description: "Makes database, table and column names case sensitive.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "enable_user_activity_logging",
        default_value: "false",
        description: "Logs each query run against the database to the user activity log.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "extra_float_digits",
        default_value: "0",
        description: "Sets the number of digits displayed for floating-point values.",
        data_type: DATA_TYPE_INTEGER,
        allowed_values: Some("-15-2"),
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "max_concurrency_scaling_clusters",
        default_value: "1",
        description: "Sets the number of concurrency scaling clusters the cluster may use.",
        data_type: DATA_TYPE_INTEGER,
        allowed_values: Some("0-10"),
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "max_cursor_result_set_size",
        default_value: "default",
        description: "Sets the maximum result set size of a cursor, in megabytes.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "query_group",
        default_value: "default",
        description: "Applies a user-defined label to the queries run in the same session.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "require_ssl",
        default_value: "false",
        description: "require ssl for all databaseconnections",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "search_path",
        default_value: "$user, public",
        description: "Sets the schema search order for names that are not schema-qualified.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_DYNAMIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "statement_timeout",
        default_value: "0",
        description: "Aborts any statement that takes over the specified number of milliseconds.",
        data_type: DATA_TYPE_INTEGER,
        allowed_values: Some("0,100-2147483647"),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "use_fips_ssl",
        default_value: "false",
        description: "Uses the FIPS-compliant SSL library for connections to the database.",
        data_type: DATA_TYPE_BOOLEAN,
        allowed_values: Some(BOOLEAN_ALLOWED_VALUES),
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
    EngineParameter {
        name: "wlm_json_configuration",
        default_value: "[{\"auto_wlm\":true}]",
        description: "The workload management configuration, as a JSON array of queues.",
        data_type: DATA_TYPE_STRING,
        allowed_values: None,
        apply_type: APPLY_TYPE_STATIC,
        is_modifiable: true,
    },
];

/// The engine default parameters a cluster parameter group of family `family`
/// publishes: [`ENGINE_DEFAULT_PARAMETERS`] for a family this service publishes,
/// and NOTHING for any other — a group of an unknown family reports no
/// parameters rather than another family's.
///
/// The family catalogue is read off [`published_parameter_group_families`], the
/// same place `CreateClusterParameterGroup` checks a requested family against, so
/// a group that could be created always has parameters to report.
pub fn family_parameters(family: &str) -> &'static [EngineParameter] {
    let published = published_parameter_group_families()
        .iter()
        .any(|published| published == family);
    if published {
        ENGINE_DEFAULT_PARAMETERS
    } else {
        &[]
    }
}

/// `Parameter$MinimumEngineVersion` — "The earliest engine version to which the
/// parameter can apply".
///
/// Every published engine version's family carries the whole catalog, so the
/// earliest version any of its parameters can apply to is the first entry of
/// [`published_cluster_versions`]. Derived rather than spelled out, so adding an
/// engine version cannot make the reported value disagree with the catalog.
pub fn minimum_engine_version() -> Option<String> {
    published_cluster_versions()
        .into_iter()
        .next()
        .and_then(|version| version.cluster_version)
}

/// How many parameter groups the ACCOUNT's own groups may number before
/// `CreateClusterParameterGroup` refuses with the modelled
/// `ClusterParameterGroupQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: Amazon Redshift publishes an
/// adjustable quota on parameter groups per account per Region, and this
/// emulator models neither Service Quotas nor an account's adjustments, so this
/// is the emulator's OWN bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::parameter_groups`]
/// rather than a claim about any account's limit. The default parameter groups
/// Redshift owns are outside it — the quota bounds what a caller can add.
pub const PARAMETER_GROUP_QUOTA: usize = 1_000;

/// Which sentence a `ClusterParameterGroupNotFoundFault` is worded with.
///
/// The same split as [`ClusterNotFoundWording`]: the fault's status (404) and
/// awsQuery code (`ClusterParameterGroupNotFound`) are what the model fixes,
/// while the fixed text around the `ParameterGroupName` the request named is an
/// attribute of the OPERATION. Recorded refusals show the sentences below in
/// use, so naming each one here keeps the choice at the handler that makes
/// it and no operation can quietly inherit another one's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterParameterGroupNotFoundWording {
    /// `ClusterParameterGroup not found: <ParameterGroupName>` — what the
    /// operations that DEREFERENCE a group they do not own answer: the read of
    /// a group's parameters (`DescribeClusterParameters`) and the cluster
    /// operations configured from a group (`CreateCluster`,
    /// `RestoreFromClusterSnapshot`).
    ShapeNameThenName,
    /// `ParameterGroup not found: <ParameterGroupName>` — what the operations
    /// whose TARGET is the group itself answer
    /// (`DeleteClusterParameterGroup` and `ResetClusterParameterGroup`).
    ShortNameThenName,
    /// `Parameter group does not exist` — what the parameter-group MODIFY,
    /// `ModifyClusterParameterGroup`, answers. The one wording with NO variable
    /// part: the refusal does not name the group the request asked for, so it is
    /// passed one only to keep every caller of
    /// [`cluster_parameter_group_not_found_message`] uniform.
    DoesNotExist,
    /// `ClusterParameterGroup <ParameterGroupName> not found.` — what the
    /// group LISTING, `DescribeClusterParameterGroups`, answers when its
    /// `ParameterGroupName` filter names no group: the identifier is embedded
    /// and the sentence ends in a period.
    NameThenNotFound,
}

/// The `ClusterParameterGroupNotFoundFault` message `wording` words, naming the
/// `ParameterGroupName` the request carried.
pub fn cluster_parameter_group_not_found_message(
    wording: ClusterParameterGroupNotFoundWording,
    parameter_group_name: &str,
) -> String {
    match wording {
        ClusterParameterGroupNotFoundWording::ShapeNameThenName => {
            format!("ClusterParameterGroup not found: {parameter_group_name}")
        }
        ClusterParameterGroupNotFoundWording::ShortNameThenName => {
            format!("ParameterGroup not found: {parameter_group_name}")
        }
        ClusterParameterGroupNotFoundWording::NameThenNotFound => {
            format!("ClusterParameterGroup {parameter_group_name} not found.")
        }
        ClusterParameterGroupNotFoundWording::DoesNotExist => {
            "Parameter group does not exist".to_string()
        }
    }
}

/// Why a create under a name the account already holds a parameter group under
/// is refused, naming the `ParameterGroupName` the request carried.
///
/// The NAME is what an account's parameter groups are unique on, so the refusal
/// names it and nothing is overwritten. The sentence carries no trailing
/// punctuation.
pub fn cluster_parameter_group_already_exists_message(parameter_group_name: &str) -> String {
    format!("Parameter group {parameter_group_name} already exists")
}
