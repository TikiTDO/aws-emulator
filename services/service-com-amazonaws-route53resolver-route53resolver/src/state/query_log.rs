// SPDX-License-Identifier: CC0-1.0

use super::*;

/// `ResolverQueryLogConfigStatus` values this slice reports, out of the model's
/// enum (`smithy_query(query="shape",shape="ResolverQueryLogConfigStatus")`:
/// `CREATING | CREATED | DELETING | FAILED`).
///
/// `CREATING` is "Resolver is creating the query logging configuration", which
/// is what a create answers with, and `CREATED` is "the query logging
/// configuration was successfully created": the move between them is the one the
/// clock owes, [`PROVISIONING_DELAY`] after the call that started it. `DELETING`
/// describes a configuration a delete has been accepted for, which is what that
/// call answers about the configuration it took out, and `FAILED` one whose
/// destination Resolver cannot deliver to, which a create never produces.
pub const QUERY_LOG_CONFIG_STATUS_CREATING: &str = "CREATING";

/// See [`QUERY_LOG_CONFIG_STATUS_CREATING`].
pub const QUERY_LOG_CONFIG_STATUS_CREATED: &str = "CREATED";

/// See [`QUERY_LOG_CONFIG_STATUS_CREATING`].
pub const QUERY_LOG_CONFIG_STATUS_DELETING: &str = "DELETING";

/// `ResourceNotFoundException$ResourceType` for a query logging configuration:
/// the modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG: &str = "ResolverQueryLogConfig";

/// `ResolverQueryLogConfigAssociationStatus` values this slice reports, out of
/// the model's enum
/// (`smithy_query(query="shape",shape="ResolverQueryLogConfigAssociationStatus")`,
/// re-exported as
/// [`crate::ops::types::RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION_STATUS_VALUES`]).
///
/// `CREATING` is "Resolver is creating an association between an Amazon VPC and
/// a query logging configuration", which is what an associate answers with, and
/// `ACTIVE` is "the association ... was successfully created": the move between
/// them is the one the clock owes, [`PROVISIONING_DELAY`] after the call that
/// started it. `DELETING` is "Resolver is deleting this query logging
/// association", which is what the association a disassociate has just taken
/// out of its configuration reports, as that call's answer about "the
/// association that you deleted". `ACTION_NEEDED` and `FAILED` describe an
/// association Resolver could not create or delete, which this slice never
/// leaves outstanding.
pub const QUERY_LOG_CONFIG_ASSOCIATION_STATUS_CREATING: &str = "CREATING";

/// See [`QUERY_LOG_CONFIG_ASSOCIATION_STATUS_CREATING`].
pub const QUERY_LOG_CONFIG_ASSOCIATION_STATUS_ACTIVE: &str = "ACTIVE";

/// See [`QUERY_LOG_CONFIG_ASSOCIATION_STATUS_CREATING`].
pub const QUERY_LOG_CONFIG_ASSOCIATION_STATUS_DELETING: &str = "DELETING";

/// `ResourceNotFoundException$ResourceType` for one association between a VPC
/// and a query logging configuration: the modelled shape name of the resource,
/// as for [`RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG`].
pub const RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION: &str =
    "ResolverQueryLogConfigAssociation";

/// The most VPC associations one query logging configuration holds, for the same
/// reason as [`MAX_STORED_QUERY_LOG_CONFIGS`]: `AssociateResolverQueryLogConfig`
/// admits any VPC the configuration is not already associated with, so without a
/// bound a caller looping associates would grow one emulator process without
/// limit. An associate past this count is REFUSED by
/// [`ResolverQueryLogConfigRecord::can_hold_another_association`] with the
/// `LimitExceededException` the operation models, and a disassociate frees a
/// slot.
pub const MAX_QUERY_LOG_CONFIG_ASSOCIATIONS: usize = 1_000;

/// The most query logging configurations this store holds, for the same reason
/// as [`MAX_STORED_ENDPOINTS`]: `CreateResolverQueryLogConfig` admits any unseen
/// `CreatorRequestId`, so without a bound a caller looping creates would grow
/// one emulator process without limit. A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_query_log_config`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record.
pub const MAX_STORED_QUERY_LOG_CONFIGS: usize = 1_000;

/// The destinations Resolver delivers query logs to, as
/// `CreateResolverQueryLogConfig$DestinationArn` documents them — "an S3 bucket,
/// a CloudWatch Logs log group, or a Kinesis Data Firehose delivery stream" —
/// each as the `service` field its ARN carries together with the prefix that
/// field's resource part must start with, from the examples the same doc gives
/// (`arn:aws:s3:::amzn-s3-demo-bucket`,
/// `arn:aws:logs:us-west-1:123456789012:log-group:/mystack-testgroup-12ABC1AB12A1:*`,
/// `arn:aws:kinesis:us-east-2:0123456789:stream/my_stream_name`). A Firehose
/// delivery stream also carries its own `firehose` service with a
/// `deliverystream/` resource.
///
/// The third field is the name that kind of destination is selected by, from the
/// `Destination` filter `ListResolverQueryLogConfigs` documents
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverQueryLogConfigs**: `S3`, `CloudWatchLogs`, `KinesisFirehose`),
/// so one table states both which destinations are well formed and what names
/// each of them.
pub(super) const QUERY_LOG_DESTINATIONS: [(&str, &str, &str); 4] = [
    ("s3", "", "S3"),
    ("logs", "log-group:", "CloudWatchLogs"),
    ("kinesis", "stream/", "KinesisFirehose"),
    ("firehose", "deliverystream/", "KinesisFirehose"),
];

/// Whether `destination_arn` names one of the destinations Resolver can send
/// query logs to, i.e. is an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`) whose service and
/// resource are one of [`QUERY_LOG_DESTINATIONS`].
///
/// The model constrains `DestinationArn` only to length 1..600, so a string that
/// names no such destination reaches the handler, which refuses it with the
/// client error the operation models rather than storing a configuration whose
/// logs could never be delivered.
pub fn query_log_destination_is_wellformed(destination_arn: &str) -> bool {
    let fields: Vec<&str> = destination_arn.splitn(6, ':').collect();
    let [prefix, partition, service, _region, _account, resource] = fields.as_slice() else {
        return false;
    };
    if *prefix != "arn" || partition.is_empty() || resource.is_empty() {
        return false;
    }
    QUERY_LOG_DESTINATIONS
        .iter()
        .any(|(name, resource_prefix, _)| *name == *service && resource.starts_with(resource_prefix))
}

/// The Amazon Web Services service a stored `DestinationArn` delivers to, by the
/// name the `Destination` filter selects it with (see
/// [`QUERY_LOG_DESTINATIONS`]), or `None` for an ARN that names no destination
/// Resolver delivers to.
pub fn query_log_destination_kind(destination_arn: &str) -> Option<&'static str> {
    let fields: Vec<&str> = destination_arn.splitn(6, ':').collect();
    let [_, _, service, _, _, resource] = fields.as_slice() else {
        return None;
    };
    QUERY_LOG_DESTINATIONS
        .iter()
        .find(|(name, resource_prefix, _)| {
            *name == *service && resource.starts_with(resource_prefix)
        })
        .map(|(_, _, kind)| *kind)
}

/// The form a query logging destination is STORED in.
///
/// A CloudWatch Logs log group is addressed by an ARN that may carry the `:*`
/// log-stream wildcard
/// (`arn:<partition>:logs:<region>:<account>:log-group:<name>:*`, the form
/// `CreateResolverQueryLogConfig$DestinationArn`'s own example uses). That
/// wildcard names every stream in the group rather than part of the group's
/// identity, and the destination is the GROUP, so it is dropped and every read
/// of the configuration reports the canonical log-group ARN. Every other
/// destination is stored exactly as the request wrote it.
pub fn canonical_query_log_destination(destination_arn: &str) -> String {
    let fields: Vec<&str> = destination_arn.splitn(6, ':').collect();
    if let [_, _, service, _, _, resource] = fields.as_slice() {
        if *service == "logs" && resource.starts_with("log-group:") {
            if let Some(group) = destination_arn.strip_suffix(":*") {
                return group.to_string();
            }
        }
    }
    destination_arn.to_string()
}

/// The `InvalidParameterException` a query-logging write answers for a
/// `DestinationArn` that names none of the destinations Resolver delivers to.
///
/// `InvalidParameterException` is "one or more parameters in this request are not
/// valid" and the model gives it a `FieldName`, "the parameter that is not
/// valid", so the refusal names the member that was rejected.
pub fn invalid_query_log_destination(destination_arn: &str) -> InvalidParameterException {
    InvalidParameterException {
        message: format!(
            "Invalid destination ARN \"{destination_arn}\": query logs can be sent only to an \
             S3 bucket, a CloudWatch Logs log group or a Kinesis Data Firehose delivery stream."
        ),
        field_name: Some("DestinationArn".to_string()),
    }
}

/// The `InvalidParameterException` a query-logging write answers for a `Name`
/// the modelled pattern excludes but the boundary validator cannot refuse; see
/// [`name_satisfies_pattern`] on why it reaches a handler at all.
pub fn invalid_query_log_config_name(name: &str) -> InvalidParameterException {
    InvalidParameterException {
        message: format!(
            "Invalid name \"{name}\": a query logging configuration name must carry at least one \
             letter, digit, hyphen, underscore, apostrophe or space, and cannot consist of only \
             digits."
        ),
        field_name: Some("Name".to_string()),
    }
}

/// The `ResourceNotFoundException` the query-logging operations answer for an id
/// they hold no configuration under, built here so every operation that resolves
/// a `ResolverQueryLogConfigId` reports the same body.
///
/// The sentence is the service's own for this fault
/// ([`RSLVR_CODE_QUERY_LOG_CONFIG_NOT_FOUND`]) and names nothing of the request:
/// Resolver does not name the configuration the caller asked for, so a caller
/// that cannot address one learns nothing else about it — the same reticence as
/// [`resource_not_found_message`], and the reason the fault is told apart by its
/// code rather than by the id in the message. It is closed with the trace clause
/// of the request ([`traced_message`]), which is the only part of the body that
/// is not fixed text.
pub fn resolver_query_log_config_not_found(trace_root: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(traced_message(
            rslvr_message(
                RSLVR_CODE_QUERY_LOG_CONFIG_NOT_FOUND,
                "The specified query logging configuration doesn't exist",
            ),
            trace_root,
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG.to_string()),
    }
}

/// The `ResourceNotFoundException` the query-logging operations answer for an
/// association they hold nothing under, built here so every operation that
/// addresses one — by its own id (`GetResolverQueryLogConfigAssociation`) or by
/// the (configuration, VPC) pair it was made from
/// (`DisassociateResolverQueryLogConfig`) — reports the same body.
///
/// The sentence is the service's own for this fault
/// ([`RSLVR_CODE_QUERY_LOG_CONFIG_ASSOCIATION_NOT_FOUND`]) and names nothing of
/// the request: Resolver names neither the association the caller asked for, nor
/// the configuration or the VPC it would join, so a caller that cannot address
/// an association learns nothing else about it — the same reticence as
/// [`resource_not_found_message`]. Its code is what distinguishes it from a
/// configuration that is itself gone ([`resolver_query_log_config_not_found`]),
/// and it is closed with the same trace clause of the request
/// ([`traced_message`]).
pub fn resolver_query_log_config_association_not_found(
    trace_root: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(traced_message(
            rslvr_message(
                RSLVR_CODE_QUERY_LOG_CONFIG_ASSOCIATION_NOT_FOUND,
                "The specified query logging configuration association doesn't exist",
            ),
            trace_root,
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION.to_string()),
    }
}

/// The `InvalidRequestException` `DeleteResolverQueryLogConfig` answers for a
/// configuration a VPC is still associated with.
///
/// The documented precondition of the delete is the sentence itself — "before
/// you can delete a query logging configuration, you must first disassociate
/// all VPCs from the configuration" — so the message states the remedy and
/// echoes nothing from the request or the store: neither the configuration, nor
/// a VPC, nor how many associations are still held, exactly as
/// [`resolver_rule_in_use`] states the same remedy for a rule. No recorded
/// response available here states a service error code for this fault — as for
/// [`unknown_resolver_rule_arn`] — so none is invented, and the HTTP status
/// (400) and the error shape are what the contract fixes.
pub fn query_log_config_still_associated() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(
            "Before you can delete a query logging configuration, you must first disassociate \
             all VPCs from the configuration."
                .to_string(),
        ),
    }
}

/// One association between a VPC and a Resolver query logging configuration, as
/// stored. It lives on the configuration it belongs to, so the configuration's
/// `AssociationCount` and the associations it holds cannot disagree.
#[derive(Debug, Clone)]
pub struct ResolverQueryLogConfigAssociationRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// association order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub resolver_query_log_config_id: String,
    /// The VPC "that you want this query logging configuration to log queries
    /// for".
    pub resource_id: String,
    /// The transient `ResolverQueryLogConfigAssociationStatus` reported until
    /// [`Self::active_at`] falls due.
    pub status: String,
    pub creation_time: String,
    /// The instant at which the move to
    /// [`QUERY_LOG_CONFIG_ASSOCIATION_STATUS_ACTIVE`] is owed.
    pub active_at: Instant,
}

impl ResolverQueryLogConfigAssociationRecord {
    /// This association's `ResolverQueryLogConfigAssociationStatus` as of `now`:
    /// the transient status until the transition the clock owes falls due,
    /// [`QUERY_LOG_CONFIG_ASSOCIATION_STATUS_ACTIVE`] after it. Computed, so a
    /// read never has to write to settle one.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.active_at {
            &self.status
        } else {
            QUERY_LOG_CONFIG_ASSOCIATION_STATUS_ACTIVE
        }
    }

    /// Project the record onto the modelled `ResolverQueryLogConfigAssociation`.
    ///
    /// `Error` is left unset, and `ErrorMessage` with it: "if the value of
    /// `Status` is a value other than `FAILED`, `Error` is null", and this slice
    /// produces no failed association.
    pub fn to_association(&self, now: Instant) -> ResolverQueryLogConfigAssociation {
        ResolverQueryLogConfigAssociation {
            id: Some(self.id.clone()),
            resolver_query_log_config_id: Some(self.resolver_query_log_config_id.clone()),
            resource_id: Some(self.resource_id.clone()),
            status: Some(self.status_at(now).to_string()),
            error: None,
            error_message: None,
            creation_time: Some(self.creation_time.clone()),
        }
    }

    /// Whether this association satisfies one
    /// `ListResolverQueryLogConfigAssociations` filter: whether the value it
    /// presents for that member is among the filter's values, as
    /// [`ResolverQueryLogConfigRecord::matches_filter`] decides it for a
    /// configuration. The status it is matched on is the one it reports as of
    /// `now`, so a filtered page and a read of the same association cannot
    /// disagree.
    pub fn matches_filter(
        &self,
        key: QueryLogConfigAssociationFilterKey,
        values: &HashSet<String>,
        now: Instant,
    ) -> bool {
        match key {
            // `Filter$Name`'s `CreationTime` entry spells this member's values
            // "in Unix time format and Coordinated Universal Time (UTC)", while
            // every read reports the modelled RFC 3339 string, so a filter value
            // in either spelling selects the association: the whole-second Unix
            // form names the second the VPC was associated in.
            QueryLogConfigAssociationFilterKey::CreationTime => {
                values.contains(self.creation_time.as_str())
                    || values.contains(&rfc3339_unix_seconds(&self.creation_time))
            }
            // "If the value of `Status` is a value other than `FAILED`, `Error`
            // is null": [`Self::to_association`] reports no `Error` for any
            // association this slice stores, because it leaves none failed, so a
            // filter on that member selects none of them.
            QueryLogConfigAssociationFilterKey::Error => false,
            QueryLogConfigAssociationFilterKey::Id => values.contains(self.id.as_str()),
            QueryLogConfigAssociationFilterKey::ResolverQueryLogConfigId => {
                values.contains(self.resolver_query_log_config_id.as_str())
            }
            QueryLogConfigAssociationFilterKey::ResourceId => {
                values.contains(self.resource_id.as_str())
            }
            QueryLogConfigAssociationFilterKey::Status => values.contains(self.status_at(now)),
        }
    }

    /// The value this association is ordered by when a request sorts on `key`:
    /// the member's own value, as a number for the one member that is a point in
    /// time and as text for the rest (see [`QueryLogConfigSortValue`]).
    pub fn sort_value(
        &self,
        key: QueryLogConfigAssociationSortKey,
        now: Instant,
    ) -> QueryLogConfigSortValue {
        match key {
            QueryLogConfigAssociationSortKey::CreationTime => {
                QueryLogConfigSortValue::Number(rfc3339_epoch_nanos(&self.creation_time))
            }
            // Null on every association this slice stores, for the reason
            // [`Self::matches_filter`] gives, so every association ties on it
            // and the stable sort leaves them in association order.
            QueryLogConfigAssociationSortKey::Error => {
                QueryLogConfigSortValue::Text(String::new())
            }
            QueryLogConfigAssociationSortKey::Id => {
                QueryLogConfigSortValue::Text(self.id.clone())
            }
            QueryLogConfigAssociationSortKey::ResolverQueryLogConfigId => {
                QueryLogConfigSortValue::Text(self.resolver_query_log_config_id.clone())
            }
            QueryLogConfigAssociationSortKey::ResourceId => {
                QueryLogConfigSortValue::Text(self.resource_id.clone())
            }
            QueryLogConfigAssociationSortKey::Status => {
                QueryLogConfigSortValue::Text(self.status_at(now).to_string())
            }
        }
    }
}

/// A Resolver query logging configuration, as stored.
///
/// `AssociationCount` is DERIVED from [`Self::associations`] and never held
/// separately — the same way [`FirewallDomainListRecord::domain_count`] derives
/// from its domains — so the count and the associations cannot disagree.
#[derive(Debug, Clone, Default)]
pub struct ResolverQueryLogConfigRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    pub owner_id: String,
    pub destination_arn: String,
    pub creator_request_id: String,
    /// The `ShareStatus` this configuration reports; the emulator has one
    /// account, so it is [`SHARE_STATUS_NOT_SHARED`].
    pub share_status: String,
    /// The associations between this configuration and the VPCs it logs queries
    /// for, in association order — what `AssociateResolverQueryLogConfig` adds
    /// to and `DisassociateResolverQueryLogConfig` takes out of. A configuration
    /// is created with none: "to specify which VPCs you want to log queries for,
    /// you use AssociateResolverQueryLogConfig".
    pub associations: Vec<ResolverQueryLogConfigAssociationRecord>,
    /// The `ResolverQueryLogConfigStatus` this configuration reports, as of
    /// [`Self::settled_at`].
    pub status: String,
    /// `CreateResolverQueryLogConfig$Tags`, kept with the record so they are not
    /// silently discarded. `ResolverQueryLogConfig` models no `Tags` member, so
    /// no response reports them: `ListTagsForResource` is a separate operation.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    /// The instant at which the move from the transient [`Self::status`] to
    /// [`QUERY_LOG_CONFIG_STATUS_CREATED`] is owed, or `None` for a
    /// configuration with no transition in flight.
    pub settled_at: Option<Instant>,
    /// The resource policy attached to this configuration, as the last
    /// `PutResolverQueryLogConfigPolicy` for it wrote it, and `None` while no
    /// policy has been attached.
    ///
    /// Held on the configuration's own record for the same reason as
    /// [`ResolverRuleRecord::resolver_rule_policy`]: a resource policy belongs
    /// to the resource it shares, so a read of the policy resolves through the
    /// same record every other query-logging operation reads, and a
    /// configuration the store no longer holds has no policy to report.
    pub resolver_query_log_config_policy: Option<String>,
}

impl ResolverQueryLogConfigRecord {
    /// `AssociationCount`: "The number of VPCs that are associated with the
    /// query logging configuration."
    pub fn association_count(&self) -> i32 {
        i32::try_from(self.associations.len()).unwrap_or(i32::MAX)
    }

    /// This configuration's association with the VPC `resource_id`, if it holds
    /// one.
    ///
    /// A configuration and a VPC are associated at most once — "to associate
    /// more than one VPC with a configuration, submit one
    /// AssociateResolverQueryLogConfig request for each VPC" — so this is what
    /// `AssociateResolverQueryLogConfig` consults before writing a second record
    /// for the same pair. The scan is over a list bounded by
    /// [`MAX_QUERY_LOG_CONFIG_ASSOCIATIONS`].
    pub fn association_for_vpc(
        &self,
        resource_id: &str,
    ) -> Option<&ResolverQueryLogConfigAssociationRecord> {
        self.associations
            .iter()
            .find(|record| record.resource_id == resource_id)
    }

    /// Whether one more VPC may be associated with this configuration, i.e. it
    /// still holds fewer than [`MAX_QUERY_LOG_CONFIG_ASSOCIATIONS`]. An
    /// associate that this refuses answers `LimitExceededException` and writes
    /// nothing.
    pub fn can_hold_another_association(&self) -> bool {
        self.associations.len() < MAX_QUERY_LOG_CONFIG_ASSOCIATIONS
    }

    /// Take this configuration's association with the VPC `resource_id` out of
    /// it, answering the record that was removed, or `None` when it holds no
    /// association with that VPC.
    ///
    /// The counterpart of [`Self::association_for_vpc`], for the write side:
    /// `DisassociateResolverQueryLogConfig` addresses an association by the
    /// configuration-and-VPC pair it was made from, so the removal is keyed on
    /// that pair. The remaining associations keep their association order, and
    /// the configuration's `AssociationCount`, derived from them, drops by one.
    pub fn remove_association_for_vpc(
        &mut self,
        resource_id: &str,
    ) -> Option<ResolverQueryLogConfigAssociationRecord> {
        let at = self
            .associations
            .iter()
            .position(|record| record.resource_id == resource_id)?;
        Some(self.associations.remove(at))
    }

    /// This configuration's `ResolverQueryLogConfigStatus` as of `now`: the
    /// transient status until the transition the clock owes falls due,
    /// [`QUERY_LOG_CONFIG_STATUS_CREATED`] after it, and the stored status
    /// itself when no transition is owed — the same shape as
    /// [`FirewallDomainListRecord::status_at`].
    pub fn status_at(&self, now: Instant) -> &str {
        match self.settled_at {
            Some(due) if now < due => &self.status,
            Some(_) => QUERY_LOG_CONFIG_STATUS_CREATED,
            None => &self.status,
        }
    }

    /// Whether this configuration satisfies one `ListResolverQueryLogConfigs`
    /// filter: whether the value it presents for that member is among the
    /// filter's values, as [`ResolverRuleRecord::matches_filter`] decides it for
    /// a rule. The status it is matched on is the one it reports as of `now`, so
    /// a filtered page and a read of the same configuration cannot disagree.
    pub fn matches_filter(
        &self,
        key: QueryLogConfigFilterKey,
        values: &HashSet<String>,
        now: Instant,
    ) -> bool {
        match key {
            QueryLogConfigFilterKey::Arn => values.contains(self.arn.as_str()),
            QueryLogConfigFilterKey::AssociationCount => {
                values.contains(&self.association_count().to_string())
            }
            // `Filter$Name`'s `CreationTime` entry spells this member's values
            // "in Unix time format and Coordinated Universal Time (UTC)", while
            // every read reports the modelled RFC 3339 string, so a filter value
            // in either spelling selects the configuration: the whole-second
            // Unix form names the second the configuration was created in.
            QueryLogConfigFilterKey::CreationTime => {
                values.contains(self.creation_time.as_str())
                    || values.contains(&rfc3339_unix_seconds(&self.creation_time))
            }
            QueryLogConfigFilterKey::CreatorRequestId => {
                values.contains(self.creator_request_id.as_str())
            }
            QueryLogConfigFilterKey::Destination => query_log_destination_kind(&self.destination_arn)
                .is_some_and(|kind| values.contains(kind)),
            QueryLogConfigFilterKey::DestinationArn => {
                values.contains(self.destination_arn.as_str())
            }
            QueryLogConfigFilterKey::Id => values.contains(self.id.as_str()),
            QueryLogConfigFilterKey::Name => values.contains(self.name.as_str()),
            QueryLogConfigFilterKey::OwnerId => values.contains(self.owner_id.as_str()),
            QueryLogConfigFilterKey::ShareStatus => values.contains(self.share_status.as_str()),
            QueryLogConfigFilterKey::Status => values.contains(self.status_at(now)),
        }
    }

    /// The value this configuration is ordered by when a request sorts on `key`:
    /// the member's own value, as a number for the two members that are not text
    /// and as text for the rest (see [`QueryLogConfigSortValue`]).
    pub fn sort_value(&self, key: QueryLogConfigSortKey, now: Instant) -> QueryLogConfigSortValue {
        match key {
            QueryLogConfigSortKey::Arn => QueryLogConfigSortValue::Text(self.arn.clone()),
            QueryLogConfigSortKey::AssociationCount => {
                QueryLogConfigSortValue::Number(i128::from(self.association_count()))
            }
            QueryLogConfigSortKey::CreationTime => {
                QueryLogConfigSortValue::Number(rfc3339_epoch_nanos(&self.creation_time))
            }
            QueryLogConfigSortKey::CreatorRequestId => {
                QueryLogConfigSortValue::Text(self.creator_request_id.clone())
            }
            QueryLogConfigSortKey::DestinationArn => {
                QueryLogConfigSortValue::Text(self.destination_arn.clone())
            }
            QueryLogConfigSortKey::Id => QueryLogConfigSortValue::Text(self.id.clone()),
            QueryLogConfigSortKey::Name => QueryLogConfigSortValue::Text(self.name.clone()),
            QueryLogConfigSortKey::OwnerId => QueryLogConfigSortValue::Text(self.owner_id.clone()),
            QueryLogConfigSortKey::ShareStatus => {
                QueryLogConfigSortValue::Text(self.share_status.clone())
            }
            QueryLogConfigSortKey::Status => {
                QueryLogConfigSortValue::Text(self.status_at(now).to_string())
            }
        }
    }

    /// Project the record onto the modelled `ResolverQueryLogConfig`.
    pub fn to_query_log_config(&self, now: Instant) -> ResolverQueryLogConfig {
        ResolverQueryLogConfig {
            id: Some(self.id.clone()),
            owner_id: Some(self.owner_id.clone()),
            status: Some(self.status_at(now).to_string()),
            share_status: Some(self.share_status.clone()),
            association_count: self.association_count(),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            destination_arn: Some(self.destination_arn.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            creation_time: Some(self.creation_time.clone()),
        }
    }
}

/// The member a `ListResolverQueryLogConfigs` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverQueryLogConfigs**): `Arn`, `AssociationCount`, `CreationTime`,
/// `CreatorRequestId`, `Destination`, `DestinationArn`, `Id`, `Name`, `OwnerId`,
/// `ShareStatus`, `Status`. It is a different catalog from [`FilterKey`]'s,
/// because the valid filter names "depend on the action that you're including
/// the filter in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryLogConfigFilterKey {
    Arn,
    AssociationCount,
    CreationTime,
    CreatorRequestId,
    /// "The Amazon Web Services service that you want to forward query logs to",
    /// which is the KIND of the configuration's destination and not its ARN (see
    /// [`query_log_destination_kind`]).
    Destination,
    DestinationArn,
    Id,
    Name,
    OwnerId,
    ShareStatus,
    Status,
}

impl QueryLogConfigFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Arn => "Arn",
            Self::AssociationCount => "AssociationCount",
            Self::CreationTime => "CreationTime",
            Self::CreatorRequestId => "CreatorRequestId",
            Self::Destination => "Destination",
            Self::DestinationArn => "DestinationArn",
            Self::Id => "Id",
            Self::Name => "Name",
            Self::OwnerId => "OwnerId",
            Self::ShareStatus => "ShareStatus",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], as
    /// [`FilterKey::parse`] folds the endpoint catalog's names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "ARN" => Some(Self::Arn),
            "ASSOCIATIONCOUNT" => Some(Self::AssociationCount),
            "CREATIONTIME" => Some(Self::CreationTime),
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DESTINATION" => Some(Self::Destination),
            "DESTINATIONARN" => Some(Self::DestinationArn),
            "ID" => Some(Self::Id),
            "NAME" => Some(Self::Name),
            "OWNERID" => Some(Self::OwnerId),
            "SHARESTATUS" => Some(Self::ShareStatus),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}

/// The member a `ListResolverQueryLogConfigs` request sorts its configurations
/// by.
///
/// The catalog is exactly the one that operation's `SortBy` documents
/// (`smithy_query(query="doc",shape="ListResolverQueryLogConfigsRequest",member="SortBy")`):
/// `Arn`, `AssociationCount`, `CreationTime`, `CreatorRequestId`,
/// `DestinationArn`, `Id`, `Name`, `OwnerId`, `ShareStatus`, `Status`. It is NOT
/// the filter catalog: `Destination` can be filtered on but not sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryLogConfigSortKey {
    Arn,
    AssociationCount,
    CreationTime,
    CreatorRequestId,
    DestinationArn,
    Id,
    Name,
    OwnerId,
    ShareStatus,
    Status,
}

impl QueryLogConfigSortKey {
    /// The canonical name of this key, used to describe one query's ordering in
    /// a pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Arn => "Arn",
            Self::AssociationCount => "AssociationCount",
            Self::CreationTime => "CreationTime",
            Self::CreatorRequestId => "CreatorRequestId",
            Self::DestinationArn => "DestinationArn",
            Self::Id => "Id",
            Self::Name => "Name",
            Self::OwnerId => "OwnerId",
            Self::ShareStatus => "ShareStatus",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire `SortBy`, or `None` when it names no member of the
    /// documented catalog. Folded by [`fold_filter_name`] for the same reason a
    /// filter name is: the service accepts the upper-snake spelling of a member
    /// name as well as the documented one.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "ARN" => Some(Self::Arn),
            "ASSOCIATIONCOUNT" => Some(Self::AssociationCount),
            "CREATIONTIME" => Some(Self::CreationTime),
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DESTINATIONARN" => Some(Self::DestinationArn),
            "ID" => Some(Self::Id),
            "NAME" => Some(Self::Name),
            "OWNERID" => Some(Self::OwnerId),
            "SHARESTATUS" => Some(Self::ShareStatus),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}

/// The value one configuration, or one of its VPC associations, is ordered by
/// under a `SortBy`.
///
/// `AssociationCount` is a count and `CreationTime` a point in time, so both are
/// ordered as NUMBERS — a count of 10 after one of 9, and an earlier instant
/// before a later one whatever subsecond digits either timestamp was rendered
/// with. Every other sortable member is a string and is ordered as text. One
/// query sorts on one member, so every value compared in it is built by the same
/// arm.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryLogConfigSortValue {
    Number(i128),
    Text(String),
}

/// The member a `ListResolverQueryLogConfigAssociations` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverQueryLogConfigAssociations**): `CreationTime`, `Error`, `Id`,
/// `ResolverQueryLogConfigId`, `ResourceId`, `Status`. It is a different catalog
/// from [`QueryLogConfigFilterKey`]'s, because the valid filter names "depend on
/// the action that you're including the filter in": an association is selected
/// on the configuration and the VPC it joins, a configuration on its own
/// members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryLogConfigAssociationFilterKey {
    CreationTime,
    /// "If the value of `Status` is `FAILED`, specify the cause:
    /// `DESTINATION_NOT_FOUND` or `ACCESS_DENIED`."
    Error,
    Id,
    ResolverQueryLogConfigId,
    ResourceId,
    Status,
}

impl QueryLogConfigAssociationFilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::CreationTime => "CreationTime",
            Self::Error => "Error",
            Self::Id => "Id",
            Self::ResolverQueryLogConfigId => "ResolverQueryLogConfigId",
            Self::ResourceId => "ResourceId",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], as
    /// [`QueryLogConfigFilterKey::parse`] folds the configuration catalog's
    /// names.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "CREATIONTIME" => Some(Self::CreationTime),
            "ERROR" => Some(Self::Error),
            "ID" => Some(Self::Id),
            "RESOLVERQUERYLOGCONFIGID" => Some(Self::ResolverQueryLogConfigId),
            "RESOURCEID" => Some(Self::ResourceId),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}

/// The member a `ListResolverQueryLogConfigAssociations` request sorts its
/// associations by.
///
/// The catalog is exactly the one that operation's `SortBy` documents
/// (`smithy_query(query="doc",shape="ListResolverQueryLogConfigAssociationsRequest",member="SortBy")`):
/// `CreationTime`, `Error`, `Id`, `ResolverQueryLogConfigId`, `ResourceId`,
/// `Status` — the same six members the filter catalog carries, so every member
/// an association can be selected on it can also be ordered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryLogConfigAssociationSortKey {
    CreationTime,
    Error,
    Id,
    ResolverQueryLogConfigId,
    ResourceId,
    Status,
}

impl QueryLogConfigAssociationSortKey {
    /// The canonical name of this key, used to describe one query's ordering in
    /// a pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::CreationTime => "CreationTime",
            Self::Error => "Error",
            Self::Id => "Id",
            Self::ResolverQueryLogConfigId => "ResolverQueryLogConfigId",
            Self::ResourceId => "ResourceId",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire `SortBy`, or `None` when it names no member of the
    /// documented catalog. Folded by [`fold_filter_name`] for the same reason a
    /// filter name is.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "CREATIONTIME" => Some(Self::CreationTime),
            "ERROR" => Some(Self::Error),
            "ID" => Some(Self::Id),
            "RESOLVERQUERYLOGCONFIGID" => Some(Self::ResolverQueryLogConfigId),
            "RESOURCEID" => Some(Self::ResourceId),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}
