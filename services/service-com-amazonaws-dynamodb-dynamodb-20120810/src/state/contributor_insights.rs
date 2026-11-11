// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch side of Contributor Insights for DynamoDB: the insight rules an `ENABLE`
//! mints for a table or index, and the key activity its data plane attributes to them.
//!
//! "CloudWatch Contributor Insights for DynamoDB graphs display the partition key and (if
//! applicable) sort key of frequently accessed items and frequently throttled items"
//! (`UpdateContributorInsights`) — those graphs are CloudWatch Contributor Insights RULES over
//! the table's key activity, so turning the feature on creates them where CloudWatch holds
//! every insight rule ([`crate::state`]'s counterpart in that service), and
//! `DescribeInsightRules` and `GetInsightRuleReport` answer for them there. The setting itself
//! — the status and mode `DescribeContributorInsights` reports — is
//! [`super::ContributorInsights`]; this file is the other half.
//!
//! Which rules an enable mints follows from the resource and the mode:
//!
//! * one pair of kinds per key the resource's schema declares — the partition key always, the
//!   sort key only where there is one, since there is no sort key to graph otherwise;
//! * the ACCESSED half only in the mode that tracks all access, because the other mode is
//!   "throttled events only" (`UpdateContributorInsights$ContributorInsightsMode`);
//! * all of them minted by ONE enable, so they carry one creation instant in their names.
//!
//! The data plane then attributes each access of the resource's items to those rules
//! ([`record_access`]): what a request consumed on the partition (and sort) key it addressed,
//! and a throttled request's refusal, each as a contribution to the rule collecting that
//! measure. That is the service's own telemetry about the access rather than a change to what
//! the access returned: a read reports the same item whether or not insights are on.
//!
//! LOCKING: [`record_access`] and the minting seams take the CLOUDWATCH store's lock, the
//! caller holding this service's own — the one order this emulator uses, the same way a
//! table's Kinesis destination is delivered to ([`super::deliver_item_changes`]). That service
//! never reaches into this store.

use super::*;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

/// The prefix DynamoDB mints every Contributor Insights rule name under, which is what lets a
/// client tell the rules of this feature from the rules an account wrote itself.
pub const RULE_NAME_PREFIX: &str = "DynamoDBContributorInsights";

/// The kind of a rule over the most ACCESSED values of the partition key.
pub const RULE_KIND_PARTITION_ACCESSED: &str = "PKC";

/// The kind of a rule over the most THROTTLED values of the partition key.
pub const RULE_KIND_PARTITION_THROTTLED: &str = "PKT";

/// The kind of a rule over the most ACCESSED values of the sort key, which only a resource
/// whose schema declares one has.
pub const RULE_KIND_SORT_ACCESSED: &str = "SKC";

/// The kind of a rule over the most THROTTLED values of the sort key. See
/// [`RULE_KIND_SORT_ACCESSED`].
pub const RULE_KIND_SORT_THROTTLED: &str = "SKT";

/// The measure an ACCESSED-keys rule aggregates per contributor: the throughput the accesses of
/// that key consumed, which is what ranks the most accessed items.
pub const MEASURE_CONSUMED_THROUGHPUT_UNITS: &str = "ConsumedThroughputUnits";

/// The measure a THROTTLED-keys rule aggregates per contributor: the accesses of that key that
/// were throttled, which is what ranks the most throttled items.
pub const MEASURE_THROTTLE_COUNT: &str = "ThrottleCount";

/// What a rule over the partition key keys its contributors by: "the partition key ... of
/// frequently accessed items" (`UpdateContributorInsights`).
pub const KEY_LABEL_PARTITION: &str = "PartitionKey";

/// What a rule over the sort key adds to that: the pair of key values identifies the item.
pub const KEY_LABEL_SORT: &str = "SortKey";

/// One throttled access, as a throttled-keys rule counts it: one event.
const ONE_THROTTLE_EVENT: f64 = 1.0;

/// One kind of rule the feature mints: how its name spells the kind, what it aggregates, what
/// its contributors are keyed by, and which half of the feature it belongs to.
#[derive(Debug, Clone, Copy)]
struct RuleKind {
    /// The kind as the rule's name spells it.
    kind: &'static str,
    /// The measure the rule aggregates ([`MEASURE_CONSUMED_THROUGHPUT_UNITS`],
    /// [`MEASURE_THROTTLE_COUNT`]).
    measure: &'static str,
    /// Whether the rule keys its contributors by the sort key as well as the partition key,
    /// which makes it a rule only a resource that declares a sort key has.
    over_sort_key: bool,
}

/// The kinds of rule the feature has, in the order an enable mints them.
///
/// The partition-key pair first and the sort-key pair after it, each pair ACCESSED then
/// THROTTLED, so the rules of one resource are minted in one stable order.
const RULE_KINDS: [RuleKind; 4] = [
    RuleKind {
        kind: RULE_KIND_PARTITION_ACCESSED,
        measure: MEASURE_CONSUMED_THROUGHPUT_UNITS,
        over_sort_key: false,
    },
    RuleKind {
        kind: RULE_KIND_PARTITION_THROTTLED,
        measure: MEASURE_THROTTLE_COUNT,
        over_sort_key: false,
    },
    RuleKind {
        kind: RULE_KIND_SORT_ACCESSED,
        measure: MEASURE_CONSUMED_THROUGHPUT_UNITS,
        over_sort_key: true,
    },
    RuleKind {
        kind: RULE_KIND_SORT_THROTTLED,
        measure: MEASURE_THROTTLE_COUNT,
        over_sort_key: true,
    },
];

impl RuleKind {
    /// Whether an enable in `mode` on a resource that declares a sort key (or not) mints this
    /// kind: the ACCESSED half only where the mode tracks all access, and the sort-key pair only
    /// where there is a sort key to graph.
    fn minted_in(&self, mode: &str, has_sort_key: bool) -> bool {
        let tracks_access = mode != CONTRIBUTOR_INSIGHTS_MODE_THROTTLED_KEYS;
        let tracked = self.measure == MEASURE_THROTTLE_COUNT || tracks_access;
        tracked && (!self.over_sort_key || has_sort_key)
    }

    /// The labels a rule of this kind keys its contributors by.
    fn key_labels(&self) -> Vec<String> {
        let mut labels = vec![KEY_LABEL_PARTITION.to_string()];
        if self.over_sort_key {
            labels.push(KEY_LABEL_SORT.to_string());
        }
        labels
    }
}

/// The name a rule of `kind` minted for `resource` at `minted_at` takes:
/// `DynamoDBContributorInsights-<kind>-<resource>-<epoch millis>`.
///
/// The resource part is the table's name, and an index's rule names the index after the table
/// it belongs to, so a name identifies the resource the rule collects for; the suffix is the
/// instant the enable minted it, in the millisecond unit it is observed in, which is what makes
/// a second enable of the same resource mint names of its own rather than reusing the first
/// one's.
fn rule_name(kind: &str, resource: &str, minted_at: SystemTime) -> String {
    let millis = minted_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{RULE_NAME_PREFIX}-{kind}-{resource}-{millis}")
}

/// The resource part of those names for the table `table_name`, or for the index `index_name`
/// inside it.
///
/// A table name and an index name are both `[a-zA-Z0-9_.-]` (their modelled patterns), so no
/// separator can be one neither may carry: a table called `t-i` and the index `i` of a table
/// `t` come to the same label, and therefore to the same name, as a client reading the name
/// alone would also find. WHICH resource a rule collects for is never read off the name: the
/// rule carries the resource's ARN, which is what attribution, listing and deletion resolve it
/// by, and what its `Definition` reports beside the table and index names.
fn rule_resource_label(table_name: &str, index_name: Option<&str>) -> String {
    match index_name {
        Some(index_name) => format!("{table_name}-{index_name}"),
        None => table_name.to_string(),
    }
}

/// The ARN of the resource whose rules these are: the table, or the index inside it.
fn insights_resource_arn(region: &str, table_name: &str, index_name: Option<&str>) -> String {
    match index_name {
        Some(index_name) => index_arn(region, table_name, index_name),
        None => table_arn(region, table_name),
    }
}

/// The `Definition` document one minted rule carries: which DynamoDB resource it collects the
/// key activity of, and what it aggregates for each key.
///
/// `InsightRule$Definition` is "the definition of the rule, as a JSON object", and a rule this
/// feature minted is not one a caller wrote — so what it reports is the resource and the
/// contribution DynamoDB minted it for: the table by NAME (and the index beside it, for an
/// index's rule), which is how a client reading the described rules tells which table a rule
/// graphs, the resource's ARN, and the keys and measure the rule ranks its contributors by.
fn rule_definition(
    kind: &RuleKind,
    region: &str,
    table_name: &str,
    index_name: Option<&str>,
) -> String {
    let mut definition = ::serde_json::json!({
        "TableName": table_name,
        "Resource": insights_resource_arn(region, table_name, index_name),
        "Contribution": {
            "Keys": kind.key_labels(),
            "ValueOf": kind.measure,
        },
    });
    if let Some(index_name) = index_name {
        definition["IndexName"] = ::serde_json::json!(index_name);
    }
    definition.to_string()
}

/// Mint the rules an `ENABLE` on `table_name` (or on the index `index_name` inside it) owes, in
/// the mode in force, answering the names the resource now holds.
///
/// Called by the enable itself: the feature's rules exist from the call that turned it on, which
/// is what makes them discoverable through `DescribeInsightRules` while the setting is still
/// settling.
///
/// A second enable brings the resource's rule set to the one the mode in force calls for, and
/// nothing else: a kind the mode tracks that the resource has no rule of is minted, a rule of a
/// kind the mode tracks is LEFT as it is — re-minting it under a fresh instant would both lose
/// what it collected and grow the store with every call — and a rule of a kind the mode no
/// longer tracks is deleted, because a resource tracking "throttled events only" is not still
/// collecting all access.
pub fn mint_rules<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    table_name: &str,
    index_name: Option<&str>,
    mode: &str,
    has_sort_key: bool,
    minted_at: SystemTime,
) -> Vec<String> {
    let resource_arn = insights_resource_arn(region, table_name, index_name);
    let label = rule_resource_label(table_name, index_name);
    let of_kind = |kind: &RuleKind| format!("{RULE_NAME_PREFIX}-{}-", kind.kind);
    // What the resource holds now, read from the store that holds it.
    let held = cloudwatch::rules_for_resource(engine, &resource_arn);
    let tracked: Vec<&RuleKind> = RULE_KINDS
        .iter()
        .filter(|kind| kind.minted_in(mode, has_sort_key))
        .collect();
    // The rules of a kind the mode in force does not track go: the mode says which halves of
    // the key activity the resource collects, so a narrowed mode leaves none of the other
    // collecting.
    let untracked: Vec<String> = held
        .iter()
        .filter(|name| {
            !tracked
                .iter()
                .any(|kind| name.starts_with(&of_kind(kind)))
        })
        .cloned()
        .collect();
    if !untracked.is_empty() {
        cloudwatch::delete_rules_named(engine, &untracked);
    }
    let rules: Vec<(String, cloudwatch::ResourceCollection)> = tracked
        .iter()
        .filter(|kind| !held.iter().any(|name| name.starts_with(&of_kind(kind))))
        .map(|kind| {
            (
                rule_name(kind.kind, &label, minted_at),
                cloudwatch::ResourceCollection {
                    resource_arn: resource_arn.clone(),
                    key_labels: kind.key_labels(),
                    measure: kind.measure.to_string(),
                    definition: rule_definition(kind, region, table_name, index_name),
                },
            )
        })
        .collect();
    cloudwatch::mint_rules(engine, &rules);
    cloudwatch::rules_for_resource(engine, &resource_arn)
}

/// Delete the rules of `table_name` (or of the index `index_name` inside it), answering the
/// names deleted.
///
/// Called by a `DISABLE`: the graphs the feature provides are its rules, so turning it off
/// removes them — and only the ones of THAT resource, so another table's rules, and another
/// index's, keep collecting.
pub fn delete_rules<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    table_name: &str,
    index_name: Option<&str>,
) -> Vec<String> {
    cloudwatch::delete_rules_for_resource(
        engine,
        &insights_resource_arn(region, table_name, index_name),
    )
}

/// The names of the rules CloudWatch holds for `table_name` (or for the index inside it),
/// ascending: what `DescribeContributorInsights` reports as the rules of the resource.
///
/// Read from the store that HOLDS those rules, so a rule deleted in CloudWatch is not reported
/// here as one that exists.
pub fn rule_names<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    table_name: &str,
    index_name: Option<&str>,
) -> Vec<String> {
    cloudwatch::rules_for_resource(
        engine,
        &insights_resource_arn(region, table_name, index_name),
    )
}

/// What one access of a resource's items contributed to its key activity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Access {
    /// The access was served, consuming this many capacity units on the key it addressed.
    Consumed(f64),
    /// The access was refused for exceeding the throughput the resource is provisioned for, so
    /// the key it addressed was throttled.
    Throttled,
}

impl Access {
    /// The measure this access contributes to.
    fn measure(self) -> &'static str {
        match self {
            Self::Consumed(_) => MEASURE_CONSUMED_THROUGHPUT_UNITS,
            Self::Throttled => MEASURE_THROTTLE_COUNT,
        }
    }

    /// How much it contributes: the units a served access consumed, and one event for a
    /// throttled one.
    fn value(self) -> f64 {
        match self {
            Self::Consumed(units) => units,
            Self::Throttled => ONE_THROTTLE_EVENT,
        }
    }
}

/// What one access contributed, as [`record_access`] takes it: the contributor keys the access
/// addressed, and what it made of them.
pub type KeyAccess = (Vec<String>, Access);

/// The contributor keys a key or item map addresses on `table`: its partition-key value as
/// text, then its sort-key value where the table declares a sort key and the map carries one.
///
/// Empty for a map that carries no value for the table's partition key, or one whose value is
/// of no type a key attribute may be declared as: such a map addresses no contributor, so there
/// is nothing to attribute to one. The texts are the store's own rendering of a key value
/// ([`authorization::condition_key_text`]), so one item is one contributor however a request
/// spelled its key.
#[must_use]
pub fn contributor_keys(table: &Table, map: &Item) -> Vec<String> {
    let Some(partition) = map
        .get(table.partition_key_name())
        .and_then(authorization::condition_key_text)
    else {
        return Vec::new();
    };
    let mut keys = vec![partition];
    if let Some(sort) = table
        .sort_key_name()
        .and_then(|name| map.get(name))
        .and_then(authorization::condition_key_text)
    {
        keys.push(sort);
    }
    keys
}

/// Attribute the accesses `accesses` made of `table_name` — or of the index `index_name` inside
/// it — to the Contributor Insights rules collecting for that resource.
///
/// Each entry is the contributor keys the access addressed ([`contributor_keys`]) and what it
/// contributed. Nothing is attributed when the resource is not collecting key activity (the
/// feature is off for it), when no entry names a contributor, or when the CloudWatch store is
/// not mounted — and a contribution of a measure none of the resource's rules collects reaches
/// none of them, which is what makes the narrower mode collect only throttled events.
pub fn record_access<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &ComAmazonawsDynamodbDynamodb20120810State,
    region: &str,
    table_name: &str,
    index_name: Option<&str>,
    accesses: &[KeyAccess],
    at: SystemTime,
) {
    // Read from THIS service's store, which the caller holds: a resource the feature is off for
    // has no rule to collect anything, so the CloudWatch store is not even reached for it.
    let collecting = store
        .table(region, table_name)
        .ok()
        .and_then(|table| table.contributor_insights(index_name).ok())
        .is_some_and(ContributorInsights::collecting);
    if !collecting {
        return;
    }
    let timestamp_ms = i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    let contributions: Vec<cloudwatch::ResourceContribution> = accesses
        .iter()
        .filter(|(keys, _)| !keys.is_empty())
        .map(|(keys, access)| cloudwatch::ResourceContribution {
            measure: access.measure().to_string(),
            keys: keys.clone(),
            value: access.value(),
            timestamp_ms,
        })
        .collect();
    cloudwatch::record_contributions(
        engine,
        &insights_resource_arn(region, table_name, index_name),
        &contributions,
    );
}

/// The same for ONE access of one key map, which is what every single-item operation makes.
pub fn record_key_access<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &ComAmazonawsDynamodbDynamodb20120810State,
    region: &str,
    table_name: &str,
    key: &Item,
    access: Access,
    at: SystemTime,
) {
    let Ok(table) = store.table(region, table_name) else {
        return;
    };
    let keys = contributor_keys(table, key);
    record_access(
        engine,
        store,
        region,
        table_name,
        None,
        &[(keys, access)],
        at,
    );
}

/// Attribute the key activity of the item changes `changes` to the rules collecting for the
/// tables they were made on.
///
/// The WRITE side of the feature, taken at the seam every landed write hands its changes to
/// ([`super::publish_item_changes`]): one change is one write of one item, so the item's key is
/// the contributor and what the table was charged for that change
/// ([`Table::write_capacity`] — the same accounting the response bills) is what it contributed.
/// A write that landed nothing queues no change and so contributes nothing.
pub fn record_item_changes<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &ComAmazonawsDynamodbDynamodb20120810State,
    changes: &[PendingStreamRecord],
    at: SystemTime,
) {
    // Grouped by the table the changes were made on, so one request's writes to one table are
    // attributed in one pass — and in a stable order, whatever order the queue holds them in.
    let mut by_table: BTreeMap<(&str, &str), Vec<KeyAccess>> = BTreeMap::new();
    for change in changes {
        // Only a CALLER's write is an access of the key it addressed: an expiring item's
        // deletion is the clock's own and is charged to nobody.
        if !change.by_caller {
            continue;
        }
        let Ok(table) = store.table(&change.region, &change.table_name) else {
            continue;
        };
        let units = table
            .write_capacity(change.old_image.as_ref(), change.new_image.as_ref())
            .table;
        by_table
            .entry((change.region.as_str(), change.table_name.as_str()))
            .or_default()
            .push((contributor_keys(table, &change.keys), Access::Consumed(units)));
    }
    for ((region, table_name), accesses) in by_table {
        record_access(engine, store, region, table_name, None, &accesses, at);
    }
}
