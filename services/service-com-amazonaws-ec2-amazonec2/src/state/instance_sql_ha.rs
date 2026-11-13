// SPDX-License-Identifier: CC0-1.0

//! The SQL Server High Availability registrations the account holds: for each instance
//! enabled for SQL Server High Availability standby detection monitoring, the Secrets
//! Manager secret the enablement named and the determination monitoring owes that node.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! `classic_link.rs` gives: one subject — the registration, its projection and the one seam
//! it is written through — in one file, and out of the end of `state.rs`, where every
//! concurrently landing EC2 fix inserts. Everything here is re-exported by `crate::state`,
//! so a handler reaches it as `crate::state::<item>`.
//!
//! The store itself is a field of [`ComAmazonawsEc2Amazonec2State`]
//! (`sql_ha_registrations`), because the state struct is one struct; only the code that
//! reads and writes it lives here.
//!
//! ## What is stored, and what is derived
//!
//! A registration is a FACT ABOUT an instance the account already holds, so the store keeps
//! only what the registration itself adds, keyed by the `InstanceId` it is a registration
//! of: the secret the enablement named and the role monitoring determined for the node,
//! with the instant that determination falls due. Everything else `RegisteredInstance`
//! reports is derived when the registration is projected
//! ([`ComAmazonawsEc2Amazonec2State::sql_ha_instance_view`]):
//!
//! * `HaStatus` is `processing` until the clock reaches the determination's due instant —
//!   "`processing` - The SQL Server High Availability status for the SQL Server High
//!   Availability instance is being updated" — and the determined status from then on, so
//!   the move off `processing` happens whether or not anybody called in the meantime and a
//!   read never has to write it back.
//! * `SqlServerLicenseUsage` follows from that status: standby nodes are the ones the
//!   waiver applies to ("If the instances are determined to be standby failover nodes,
//!   Amazon Web Services automatically applies SQL Server licensing fee waiver for those
//!   instances", `EnableInstanceSqlHaStandbyDetections`), so a `standby` node is `waived`
//!   and every other status is `full`.
//! * `LastUpdatedTime` is "the date and time when the instance's SQL Server High
//!   Availability status was last updated", which is the instant the reported status was
//!   reached: the determination's due instant once it has passed, and otherwise the instant
//!   the instance was registered.
//! * `Tags` are the instance's own, read off the instance record, so a tag `CreateTags`
//!   writes after the registration is reported by the next describe and no second copy can
//!   drift.
//!
//! ## What the two describes share
//!
//! Both SQL Server High Availability describes — the one that reports an instance's CURRENT
//! state and the one that reports the states it HAS HELD — publish the same `MaxResults`
//! range, the same filter catalog and the same id list, so reading their shared request
//! members ([`sql_ha_describe_selection`]), resolving the ids they name
//! ([`unresolved_sql_ha_instance_ids`]), answering a filter against a row
//! ([`sql_ha_filter_values`]) and projecting a registration
//! ([`ComAmazonawsEc2Amazonec2State::sql_ha_instance_history`], whose last entry is the
//! current state) all live here once. Only what each operation does with the result — a
//! single row per instance, or every state it has held inside the requested time window —
//! stays in its own handler.
//!
//! The store starts EMPTY and stays empty until something registers an instance.
//! [`ComAmazonawsEc2Amazonec2State::register_instance_sql_ha`] is the one way a registration
//! is stored — the seam `EnableInstanceSqlHaStandbyDetections`, still a scaffold, has to
//! write through — and it refuses a registration for an instance this account does not
//! hold, so a reported registration always names an instance that exists.

use super::*;

/// `HaStatus`'s `processing` member: "The SQL Server High Availability status for the SQL
/// Server High Availability instance is being updated" (`RegisteredInstance$HaStatus`). The
/// full catalog is the generated `crate::ops::types::HA_STATUS_VALUES`; these constants name
/// the members this slice reads and writes, so one value is spelled one way everywhere.
pub const HA_STATUS_PROCESSING: &str = "processing";

/// `HaStatus`'s `active` member: "The SQL Server High Availability instance is an active node
/// in an SQL Server High Availability cluster" (see [`HA_STATUS_PROCESSING`]).
pub const HA_STATUS_ACTIVE: &str = "active";

/// `HaStatus`'s `standby` member: "The SQL Server High Availability instance is a standby
/// failover node in an SQL Server High Availability cluster" (see [`HA_STATUS_PROCESSING`]).
pub const HA_STATUS_STANDBY: &str = "standby";

/// `HaStatus`'s `invalid` member: "An error occurred due to misconfigured permissions, or
/// unable to dertemine SQL Server High Availability status" (see [`HA_STATUS_PROCESSING`]).
pub const HA_STATUS_INVALID: &str = "invalid";

/// `SqlServerLicenseUsage`'s `full` member: "The SQL Server High Availability instance is
/// using a full SQL Server license" (`RegisteredInstance$SqlServerLicenseUsage`). The full
/// catalog is the generated `crate::ops::types::SQL_SERVER_LICENSE_USAGE_VALUES`.
pub const SQL_SERVER_LICENSE_USAGE_FULL: &str = "full";

/// `SqlServerLicenseUsage`'s `waived` member: "The SQL Server High Availability instance is
/// waived from the SQL Server license" (see [`SQL_SERVER_LICENSE_USAGE_FULL`]).
pub const SQL_SERVER_LICENSE_USAGE_WAIVED: &str = "waived";

/// What monitoring determined about one registered node, and when that determination falls
/// due.
///
/// A registration does not report its node's role immediately: "Once enabled, Amazon Web
/// Services monitors the metadata for the instances to determine whether they are active or
/// standby nodes" (`EnableInstanceSqlHaStandbyDetections`), and until that has happened the
/// instance's `HaStatus` is `processing`. So the determination is held with the instant it
/// becomes the instance's status, and the projection compares that instant with the clock
/// rather than a reader writing the move back.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SqlHaDetermination {
    /// When this determination becomes the instance's reported `HaStatus`. Before it, the
    /// instance reads [`HA_STATUS_PROCESSING`].
    pub due_at: SystemTime,
    /// The status monitoring determined: one of [`HA_STATUS_ACTIVE`], [`HA_STATUS_STANDBY`]
    /// or [`HA_STATUS_INVALID`] — the three settled members of `HaStatus`.
    pub ha_status: String,
    /// "A brief description of the SQL Server High Availability status. If the instance is
    /// in the `invalid` High Availability status, this parameter includes the error message"
    /// (`RegisteredInstance$ProcessingStatus`). Absent when monitoring reported none.
    pub processing_status: Option<String>,
}

/// One instance's SQL Server High Availability registration: what enabling it added, beside
/// the instance the account already holds.
///
/// Not the modelled `RegisteredInstance` shape, unlike most stores of this module, because
/// four of that shape's seven members are not facts of the REGISTRATION: the status, the
/// license usage and the instant the status was reached follow from the determination below,
/// and the tags belong to the instance.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SqlHaRegistrationRecord {
    /// When the instance was registered for monitoring: the instant its first status — and
    /// so its `LastUpdatedTime` while no determination has fallen due — dates from.
    pub registered_at: SystemTime,
    /// "The ARN of the Secrets Manager secret containing the SQL Server access credentials"
    /// the enablement named (`EnableInstanceSqlHaStandbyDetectionsRequest$SqlServerCredentials`),
    /// reported as `RegisteredInstance$SqlServerCredentials`. Absent when the request named
    /// none, in which case "deafult local user credentials will be used".
    pub sql_server_credentials: Option<String>,
    /// What monitoring determined for this node, if it has reached a determination at all.
    /// `None` leaves the instance `processing` indefinitely, which is what an instance
    /// nothing has determined anything about reports.
    pub determination: Option<SqlHaDetermination>,
}

/// The caller-chosen bytes one stored registration holds: the secret ARN the enablement
/// named and the status words a determination carries. The key is an instance id this
/// service minted, so it is fixed-size.
///
/// The ONE definition of that charge, the way every other store of this module has one:
/// [`ComAmazonawsEc2Amazonec2State::register_instance_sql_ha`] charges exactly it, and a
/// path that takes a registration back out must give back the same number.
pub fn sql_ha_registration_payload_bytes(record: &SqlHaRegistrationRecord) -> usize {
    let credentials = record
        .sql_server_credentials
        .as_ref()
        .map_or(0, String::len);
    let determined = record.determination.as_ref().map_or(0, |determination| {
        determination.ha_status.len()
            + determination
                .processing_status
                .as_ref()
                .map_or(0, String::len)
    });
    credentials.saturating_add(determined)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Register the instance `instance_id` for SQL Server High Availability standby
    /// detection monitoring with `record`, and answer whether the store took it.
    ///
    /// `false` means NOTHING was written and the caller must refuse the request rather than
    /// report a registration that does not exist: the account holds no such instance (the
    /// store never holds a registration of an instance that is not there, so a describe
    /// cannot report one), or the registration's caller-chosen bytes do not fit the store's
    /// byte budget ([`Self::would_exceed_stored_bytes_after_refund`]).
    ///
    /// An instance is registered once, so registering one again REPLACES its registration —
    /// which is how re-enabling an instance with different credentials reads — and the bytes
    /// the first was charged are given back.
    ///
    /// `EnableInstanceSqlHaStandbyDetections` is the operation that writes through here, and
    /// [`Self::deregister_instance_sql_ha`] is what takes a registration back out.
    pub fn register_instance_sql_ha(
        &mut self,
        instance_id: &str,
        record: SqlHaRegistrationRecord,
    ) -> bool {
        if !self.instances.contains_key(instance_id) {
            return false;
        }
        let charge = sql_ha_registration_payload_bytes(&record);
        let refund = self
            .sql_ha_registrations
            .get(instance_id)
            .map_or(0, sql_ha_registration_payload_bytes);
        if self.would_exceed_stored_bytes_after_refund(charge, refund) {
            return false;
        }
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        self.sql_ha_registrations
            .insert(instance_id.to_string(), record);
        true
    }

    /// Take the SQL Server High Availability registration of `instance_id` back out of the
    /// store, and answer the instance as it read the instant before — `None` when that
    /// instance held no registration at all.
    ///
    /// The one way a registration is removed, the mirror of [`Self::register_instance_sql_ha`]:
    /// the bytes that registration was charged are given back, so a pair of enable and
    /// disable calls leaves the store's budget exactly where it started. "Once disabled,
    /// Amazon Web Services no longer monitors the metadata for the instances"
    /// (`DisableInstanceSqlHaStandbyDetections`) — there is no longer a registration to
    /// derive a status from, which is why the row is projected BEFORE the record is dropped:
    /// what a disable reports is the state monitoring had reached for that node.
    pub fn deregister_instance_sql_ha(
        &mut self,
        instance_id: &str,
        now: SystemTime,
    ) -> Option<crate::ops::types::RegisteredInstance> {
        let reported = self.sql_ha_instance_view(instance_id, now);
        let removed = self.sql_ha_registrations.remove(instance_id)?;
        self.release_stored_bytes(sql_ha_registration_payload_bytes(&removed));
        reported
    }

    /// Every SQL Server High Availability state the instance `instance_id` HAS HELD as of
    /// `now`, oldest first — empty when that instance is not registered for SQL Server High
    /// Availability monitoring, or is no longer an instance this account holds.
    ///
    /// The one timeline of a registration, so the operation that reports an instance's
    /// CURRENT state ([`Self::sql_ha_instance_view`], its last entry) and the one that
    /// reports its historical states cannot describe the same instance differently. A READ:
    /// every entry is DERIVED from the stored registration and the clock (see the module
    /// docs), so two calls a moment apart report the same timeline and nothing is written
    /// back.
    ///
    /// A registration's timeline is the states monitoring took it through: it begins
    /// [`HA_STATUS_PROCESSING`] at the instant the instance was registered — "Once enabled,
    /// Amazon Web Services monitors the metadata for the instances to determine whether they
    /// are active or standby nodes" (`EnableInstanceSqlHaStandbyDetections`) — and reaches the
    /// determined state at the instant that determination fell due, which is why a
    /// determination the clock has not reached yet is no state the instance has held.
    pub fn sql_ha_instance_history(
        &self,
        instance_id: &str,
        now: SystemTime,
    ) -> Vec<SqlHaHistoryState> {
        let Some(record) = self.sql_ha_registrations.get(instance_id) else {
            return Vec::new();
        };
        let Some(instance) = self.instances.get(instance_id) else {
            return Vec::new();
        };
        // The instance's own tags and the secret the enablement named are facts of the
        // registration rather than of one state, so every entry of the timeline carries the
        // ones it holds NOW: a tag `CreateTags` writes after the registration is reported by
        // the next describe, and no second copy can drift.
        let tags = tag_list(&instance.tags);
        let state_at = |ha_status: &str,
                        processing_status: Option<&str>,
                        reached_at: SystemTime|
         -> SqlHaHistoryState {
            SqlHaHistoryState {
                reached_at,
                instance: crate::ops::types::RegisteredInstance {
                    instance_id: Some(instance.instance_id.clone()),
                    sql_server_license_usage: Some(
                        sql_server_license_usage_of(ha_status).to_string(),
                    ),
                    ha_status: Some(ha_status.to_string()),
                    processing_status: processing_status.map(str::to_string),
                    last_updated_time: Some(system_time_to_date_time(reached_at)),
                    sql_server_credentials: record.sql_server_credentials.clone(),
                    tags: Some(tags.clone()),
                },
            }
        };

        let mut history = vec![state_at(HA_STATUS_PROCESSING, None, record.registered_at)];
        // A determination the clock has not reached yet has not happened: the instance is
        // still having its status updated, and the state it will reach is not one it has held.
        if let Some(determination) = record
            .determination
            .as_ref()
            .filter(|determination| now >= determination.due_at)
        {
            history.push(state_at(
                determination.ha_status.as_str(),
                determination.processing_status.as_deref(),
                determination.due_at,
            ));
        }
        history
    }

    /// The modelled `RegisteredInstance` for `instance_id` as of `now`, or `None` when that
    /// instance is not registered for SQL Server High Availability monitoring — or is no
    /// longer an instance this account holds.
    ///
    /// The one projection of a registration, so every operation that reports one reports it
    /// identically, and a READ: the CURRENT state is the LAST state the instance has held
    /// ([`Self::sql_ha_instance_history`]), so the current and the historical report of one
    /// instance are the same derivation read at two ends.
    pub fn sql_ha_instance_view(
        &self,
        instance_id: &str,
        now: SystemTime,
    ) -> Option<crate::ops::types::RegisteredInstance> {
        self.sql_ha_instance_history(instance_id, now)
            .pop()
            .map(|state| state.instance)
    }
}

/// One state a registered instance has held, and the instant it reached it.
///
/// The instant is carried BESIDE the projected row rather than read back out of its
/// `LastUpdatedTime`: a describe that selects states over a time window compares instants, and
/// reading one back out of the modelled timestamp would compare a rendering of it instead.
#[derive(Debug, Clone)]
pub struct SqlHaHistoryState {
    /// When the instance reached this state: "the date and time when the instance's SQL
    /// Server High Availability status was last updated" (`RegisteredInstance$LastUpdatedTime`)
    /// as of that state, which is the instant the row reports.
    pub reached_at: SystemTime,
    /// The state itself, as the modelled `RegisteredInstance` every SQL Server High
    /// Availability describe reports.
    pub instance: crate::ops::types::RegisteredInstance,
}

/// The `SqlServerLicenseUsage` an instance in the SQL Server High Availability status
/// `ha_status` is on.
///
/// The waiver is what the monitoring is FOR: "If the instances are determined to be standby
/// failover nodes, Amazon Web Services automatically applies SQL Server licensing fee waiver
/// for those instances" (`EnableInstanceSqlHaStandbyDetections`). So a standby node is
/// [`SQL_SERVER_LICENSE_USAGE_WAIVED`] and every other status — an active node, one still
/// being determined, one whose status could not be determined at all — is on the
/// [`SQL_SERVER_LICENSE_USAGE_FULL`] license it was launched with.
pub fn sql_server_license_usage_of(ha_status: &str) -> &'static str {
    if ha_status == HA_STATUS_STANDBY {
        SQL_SERVER_LICENSE_USAGE_WAIVED
    } else {
        SQL_SERVER_LICENSE_USAGE_FULL
    }
}

/// What monitoring determines about a node this emulator registers, and when that
/// determination falls due.
///
/// "Once enabled, Amazon Web Services monitors the metadata for the instances to determine
/// whether they are active or standby nodes in the SQL Server High Availability cluster"
/// (`EnableInstanceSqlHaStandbyDetections`), reading that metadata off the SQL Server the
/// instance runs through the Systems Manager agent on it. NOTHING RUNS INSIDE AN INSTANCE OF
/// THIS EMULATOR, so there is no cluster metadata to read and no node role to find: the
/// determination monitoring reaches is therefore [`HA_STATUS_INVALID`], the member of
/// `HaStatus` for exactly that outcome — "an error occurred due to misconfigured
/// permissions, or unable to dertemine SQL Server High Availability status"
/// (`RegisteredInstance$HaStatus`) — and never a role nothing determined, which would put a
/// licensing-fee waiver on an instance on no evidence at all.
///
/// It falls due one [`TRANSITION_DELAY`] after the enablement, like every other move this
/// store records rather than sleeps for, so a caller reading straight after the enable still
/// sees the [`HA_STATUS_PROCESSING`] the registration starts in.
pub fn undetermined_sql_ha_status(enabled_at: SystemTime) -> SqlHaDetermination {
    SqlHaDetermination {
        due_at: enabled_at + TRANSITION_DELAY,
        ha_status: HA_STATUS_INVALID.to_string(),
        processing_status: Some(SQL_HA_STATUS_UNDETERMINED_DETAIL.to_string()),
    }
}

/// "A brief description of the SQL Server High Availability status. If the instance is in the
/// `invalid` High Availability status, this parameter includes the error message"
/// (`RegisteredInstance$ProcessingStatus`) — the one this emulator's monitoring has, for the
/// reason [`undetermined_sql_ha_status`] gives.
pub const SQL_HA_STATUS_UNDETERMINED_DETAIL: &str =
    "Unable to determine the SQL Server High Availability status";

/// Refuse the `InstanceIds` entries that name no instance registered for SQL Server High
/// Availability monitoring: HTTP 400 `InvalidInstanceID.NotFound`, in the wording the SQL
/// Server High Availability describes answer.
///
/// ONE refusal for the whole request rather than one per id, and its own wording rather than
/// [`crate::scaffolded::Scaffolded::invalid_instance_id_not_found`]'s: this surface's
/// validator does not distinguish an id the account holds no instance for from one whose
/// instance is not enabled for monitoring — both are "not found in account or SQL HA standby
/// detection not enabled" — so it names every id it could not resolve and says so once. A
/// second named constructor rather than a literal at a call site for the reason
/// [`crate::scaffolded::Scaffolded::instance_id_is_malformed`] is one: EC2's per-operation
/// validators do not word an unresolvable instance id the same way, and keeping each wording
/// in one place is what stops a third call site from inventing a third.
///
/// `unresolved` is in the order the request named them, so the sentence lists them the way
/// the caller sent them.
pub fn sql_ha_instance_ids_not_found<E>(unresolved: &[&str]) -> Scaffolded<E> {
    Scaffolded::service_error(
        // The same wire code `crate::scaffolded::Scaffolded::invalid_instance_id_not_found`
        // answers; only the sentence differs.
        "InvalidInstanceID.NotFound",
        400,
        format!(
            "Requested instance ID(s) [{}] not found in account or SQL HA standby detection \
             not enabled.",
            unresolved.join(", ")
        ),
    )
}

/// The most rows a SQL Server High Availability describe reports in one page: the top of the
/// modelled `@range` on `DescribeInstanceSqlHaStatesRequestMaxResultsInteger` (1..1000,
/// `smithy_query(query="operation",operation="DescribeInstanceSqlHaStates")`), which both
/// describes of this surface take their `MaxResults` from — and which is therefore also the
/// page a request naming none gets.
pub const MAX_SQL_HA_ROWS_PER_PAGE: usize = 1000;

/// The smallest page a SQL Server High Availability describe's `MaxResults` may ask for: the
/// bottom of that same modelled `@range`.
pub const MIN_SQL_HA_ROWS_PER_PAGE: usize = 1;

/// The filter names the SQL Server High Availability describes document, from
/// `DescribeInstanceSqlHaStatesRequest$Filters` and
/// `DescribeInstanceSqlHaHistoryStatesRequest$Filters`, which publish the same catalog:
///
/// ```text
/// smithy_query(query="doc",shape="DescribeInstanceSqlHaStatesRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// `tag:<key>` is the one name with a variable part — "The tag key and value pair assigned to
/// the instance … specify tag:Owner for the filter name and TeamA for the filter value" — so
/// it is matched by prefix rather than listed here. Every entry must be answered by
/// [`sql_ha_filter_values`].
pub const SQL_HA_STATE_FILTERS: [&str; 3] = ["haStatus", "sqlServerLicenseUsage", "tag-key"];

/// Is `name` a filter the SQL Server High Availability describes document? "Filter names are
/// case-sensitive" (`Filter$Name`), so the comparison is exact — and a `tag:` prefix with no
/// key names no tag, so it is as undocumented as any other misspelling.
pub fn sql_ha_filter_is_documented(name: &str) -> bool {
    name.strip_prefix(TAG_FILTER_PREFIX)
        .is_some_and(|key| !key.is_empty())
        || SQL_HA_STATE_FILTERS.contains(&name)
}

/// The values the EC2 filter `name` compares against one reported SQL Server High Availability
/// state — the per-surface half of [`matches_every_filter`], which holds the AND/OR evaluation
/// every filtered describe of this crate shares.
///
/// Each one is read off the SAME projection the response reports, so a filter can never select
/// on something the row does not show — which is also why a HISTORICAL state is selected by
/// the status THAT state carried rather than by the instance's current one. A filter naming an
/// attribute this row does not carry compares against NO values, so it selects nothing — the
/// answer for `tag:<key>` on an instance carrying no tag of that key. `name` is one of
/// [`SQL_HA_STATE_FILTERS`] or a `tag:<key>`.
pub fn sql_ha_filter_values<'a>(
    instance: &'a crate::ops::types::RegisteredInstance,
    name: &str,
) -> Vec<Cow<'a, str>> {
    if let Some(key) = name.strip_prefix(TAG_FILTER_PREFIX) {
        return instance
            .tags
            .iter()
            .flatten()
            .filter(|tag| tag.key.as_deref() == Some(key))
            .filter_map(|tag| tag.value.as_deref().map(Cow::Borrowed))
            .collect();
    }

    let text = |value: Option<&'a str>| -> Vec<Cow<'a, str>> {
        value.map(Cow::Borrowed).into_iter().collect()
    };
    match name {
        // "haStatus - The SQL Server High Availability status of the SQL Server High
        // Availability instance (processing | active | standby | invalid)."
        "haStatus" => text(instance.ha_status.as_deref()),
        // "sqlServerLicenseUsage - The license type for the SQL Server license
        // (full | waived)."
        "sqlServerLicenseUsage" => text(instance.sql_server_license_usage.as_deref()),
        // "tag-key - The tag key assigned to the instance", regardless of the tag value.
        "tag-key" => instance
            .tags
            .iter()
            .flatten()
            .filter_map(|tag| tag.key.as_deref().map(Cow::Borrowed))
            .collect(),
        // Unreachable: `sql_ha_filter_is_documented` has already refused every other name. An
        // undefined name compares against nothing, so it could never widen a result even if
        // that check were bypassed.
        _ => Vec::new(),
    }
}

/// What a SQL Server High Availability describe's shared request members select, once they
/// have been read and refused.
#[derive(Debug)]
pub struct SqlHaDescribeSelection {
    /// The page size the request asked for, or [`MAX_SQL_HA_ROWS_PER_PAGE`] when it named
    /// none.
    pub limit: usize,
    /// The request's `Filters`, each value compiled into its pattern ONCE rather than per
    /// candidate.
    pub filters: Vec<CompiledFilter>,
    /// The cursor the request's `NextToken` carried, if any.
    pub start_at: Option<String>,
}

/// Read the request members the two SQL Server High Availability describes share —
/// `InstanceIds`, `MaxResults`, `Filters` and `NextToken` — refusing every one this surface
/// does not serve.
///
/// ONE reading of them, so the current-state and the historical describe cannot cap, refuse or
/// resume a request differently: both publish the same `MaxResults` range, the same filter
/// catalog and the same id list, so a request either surface refuses is refused by both with
/// the same sentence.
///
/// Every request-controlled factor of the matching cost — ids, filters, values per filter and
/// bytes per value — is capped HERE, before any work proportional to it is done and before the
/// store is locked; their PRODUCT is then charged to one [`MatchBudget`] by the caller, which
/// is the bound the per-factor caps cannot give. Nothing here reads the store, so whether a
/// request is readable never depends on what the account holds.
pub fn sql_ha_describe_selection<E>(
    instance_ids: &[String],
    max_results: Option<i32>,
    filters: &[crate::ops::types::Filter],
    next_token: Option<&str>,
) -> Result<SqlHaDescribeSelection, Scaffolded<E>> {
    // "The IDs of the SQL Server High Availability instances to describe. If omitted, the API
    // returns … all SQL Server High Availability instances" (`InstanceIds`): every entry is
    // resolved against the store by the caller, so the list is capped the way every other EC2
    // request's id list is, before any work proportional to it is done.
    if instance_ids.len() > MAX_RESOURCE_IDS {
        return Err(Scaffolded::invalid_parameter_value(format!(
            "InstanceIds carries {} ids, above the maximum of {MAX_RESOURCE_IDS}",
            instance_ids.len()
        )));
    }

    // "The maximum number of results to return for the request in a single page"
    // (`MaxResults`). The modelled `@range` of 1..1000 refuses anything outside it before a
    // handler runs, so the refusal below cannot be reached today — but a page size is REFUSED
    // rather than silently repaired, so widening that range can never turn into a quietly
    // truncated page. A request that names none gets the largest page this surface serves, so
    // the `NextToken` path behaves the same whether or not a page size was asked for.
    let limit = match max_results {
        None => MAX_SQL_HA_ROWS_PER_PAGE,
        Some(max_results) => usize::try_from(max_results)
            .ok()
            .filter(|page_size| {
                (MIN_SQL_HA_ROWS_PER_PAGE..=MAX_SQL_HA_ROWS_PER_PAGE).contains(page_size)
            })
            .ok_or_else(|| {
                Scaffolded::invalid_parameter_value(format!(
                    "Invalid value '{max_results}' for maxResults. It must be between \
                     {MIN_SQL_HA_ROWS_PER_PAGE} and {MAX_SQL_HA_ROWS_PER_PAGE}"
                ))
            })?,
    };

    // Matching cost is filters x values x value length x reported rows, and the model bounds
    // none of them.
    if filters.len() > MAX_FILTERS {
        return Err(Scaffolded::invalid_parameter_value(format!(
            "The request carries {} filters, above the maximum of {MAX_FILTERS}",
            filters.len()
        )));
    }
    // A filter name this surface does not document is refused whether or not any row would
    // have matched it, so an unsupported filter can never read as "no results".
    let mut compiled: Vec<CompiledFilter> = Vec::with_capacity(filters.len());
    for filter in filters {
        let name = filter.name.as_deref().unwrap_or_default();
        if !sql_ha_filter_is_documented(name) {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "The filter '{name}' is invalid"
            )));
        }
        let values: &[String] = filter.values.as_deref().unwrap_or_default();
        if values.len() > MAX_FILTER_VALUES {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "The filter '{name}' carries {} values, above the maximum of \
                 {MAX_FILTER_VALUES}",
                values.len()
            )));
        }
        // A capped NUMBER of comparisons is not capped WORK: the matcher's cost is the product
        // of the pattern's length and the value's, so each value's size is bounded too, before
        // any matching runs under the lock.
        if let Some(oversize) = values
            .iter()
            .find(|value| value.len() > MAX_FILTER_VALUE_BYTES)
        {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "A value of the filter '{name}' carries {} bytes, above the maximum of \
                 {MAX_FILTER_VALUE_BYTES}",
                oversize.len()
            )));
        }
        compiled.push(CompiledFilter::new(name.to_string(), values.to_vec()));
    }

    // "The token to use to retrieve the next page of results" (`NextToken`) — anything else is
    // refused rather than read as a position, and refused against the store's contents
    // nowhere: whether a bogus token is caught never depends on what the account holds, only
    // on the token itself.
    let start_at = match next_token {
        None => None,
        Some(token) => {
            Some(decode_page_token(token).ok_or_else(|| invalid_pagination_token_for(token))?)
        }
    };

    // "The IDs of the SQL Server High Availability instances to describe": a value that is no
    // instance id at all is refused as malformed, before the store is read at all, through the
    // one place this crate reads an id in either published form
    // ([`instance_id_reads_as_one`], whose doc says why these operations take that reading) —
    // the request itself is unreadable, which does not depend on what the account holds.
    for instance_id in instance_ids {
        if !instance_id_reads_as_one(instance_id) {
            return Err(Scaffolded::invalid_instance_id_malformed(instance_id));
        }
    }

    Ok(SqlHaDescribeSelection {
        limit,
        filters: compiled,
        start_at,
    })
}

/// The ids of `instance_ids` that name no registration this store holds, each named ONCE
/// however many times the request repeated it, in the order the request named them.
///
/// The one resolution step the SQL Server High Availability describes share: an id either
/// surface cannot resolve is an error, not an absent row, and neither distinguishes an id the
/// account holds no instance for from one whose instance is not enabled for monitoring — so
/// the caller refuses the whole list through [`sql_ha_instance_ids_not_found`].
pub fn unresolved_sql_ha_instance_ids<'a>(
    store: &ComAmazonawsEc2Amazonec2State,
    instance_ids: &'a [String],
) -> Vec<&'a str> {
    let mut seen = ::std::collections::HashSet::new();
    instance_ids
        .iter()
        .map(String::as_str)
        .filter(|instance_id| !store.sql_ha_registrations.contains_key(*instance_id))
        .filter(|instance_id| seen.insert(*instance_id))
        .collect()
}
