// SPDX-License-Identifier: CC0-1.0

//! The Contributor Insights rules another Amazon Web Services service MINTS for one of its own
//! resources, and the contributions that resource's data plane attributes to them.
//!
//! "When you enable a managed rule, you create a Contributor Insights rule that collects data
//! from Amazon Web Services services." (`PutManagedInsightRules`) — a service that turns
//! Contributor Insights on for one of its resources creates such a rule in THIS store, which is
//! the one `DescribeInsightRules` lists and `GetInsightRuleReport` reports from. The rule is
//! CloudWatch's resource either way: the service that minted it names it, says which fields its
//! contributors are keyed by and which measure it aggregates, and everything the rule holds
//! lives here.
//!
//! Such a rule is not written against the log-rule syntax ([`super::insight_rules`]): it
//! collects from a resource rather than from a log group, so its contributions arrive through
//! [`record_contributions`] — one per access the resource's data plane served — instead of
//! being read out of ingested log events. The FOLD a report answers from is the same one in
//! both cases ([`super::insight_rules::report`]), so a report of such a rule carries the same
//! series, ranking and ceilings as a report of a caller's own rule.
//!
//! The seams below are what a service crate calls, each taking this store's guard briefly and
//! on its own. LOCK ORDER: the calling service's own store is the OUTER lock and this one the
//! inner — a service that attributes an access it is serving holds its own guard while it calls
//! here, the order the rest of this tree uses for a cross-service write (the DynamoDB side's
//! `state::kinesis_delivery` documents the same one) — and nothing here reaches back into the
//! calling service's store, so the two can never be taken in the other order.

use super::{
    insight_rules::ContributorObservation,
    ComAmazonawsCloudwatchGraniteserviceversion20100801State as State, StoredInsightRule,
};
use crate::ops::types::InsightRule;

/// What a Contributor Insights rule a service minted for one of its resources collects: the
/// resource whose accesses it is attributed from, the fields its contributors are keyed by, and
/// the measure it aggregates per contributor.
///
/// The key labels are reported as the report's `KeyLabels` ("the strings used as the keys for
/// this rule", `GetInsightRuleReportOutput$KeyLabels`), and a contribution is attributed to a
/// rule when it carries the measure that rule aggregates, so one resource's rules collect
/// different measures of the same accesses without the service that minted them having to
/// address each rule by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceCollection {
    /// The ARN of the resource whose accesses this rule collects.
    pub resource_arn: String,
    /// The fields a contributor of this rule is identified by, in the order the rule lists
    /// them.
    pub key_labels: Vec<String>,
    /// The measure this rule aggregates per contributor, as the service that minted the rule
    /// names it.
    pub measure: String,
    /// The `Definition` document the minting service states this rule with ("the definition of
    /// the rule, as a JSON object", `InsightRule$Definition`).
    ///
    /// A client reads a minted rule's definition to learn WHICH resource of that service the
    /// rule collects for and under what, and the service that owns the resource is what states
    /// that document — so it travels with the collection rather than being written here.
    pub definition: String,
}

/// One contribution a resource's data plane attributes to the rules collecting a measure of it:
/// which measure, which contributor, how much, and when.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceContribution {
    /// The measure contributed to, which selects the rules this contribution reaches.
    pub measure: String,
    /// The values identifying the contributor, in the order a rule's key labels list them. A
    /// rule keyed by fewer labels than the contribution carries takes the leading ones; one
    /// keyed by more takes nothing from it, because the contributor it would name is not
    /// established.
    pub keys: Vec<String>,
    /// How much was contributed.
    pub value: f64,
    /// When, in milliseconds since the epoch — the unit an observation is stamped in.
    pub timestamp_ms: i64,
}

impl State {
    /// Mint the rule `name` for `collection`, answering whether this store now holds it.
    ///
    /// A name this store already holds a rule for is LEFT as it is: minting does not overwrite
    /// a rule a caller wrote, and re-minting the same rule would throw away what it has already
    /// collected. The store's rule ceiling ([`super::MAX_INSIGHT_RULES`]) applies to a minted
    /// rule like any other, so a service cannot grow this store without bound.
    pub fn mint_resource_insight_rule(
        &mut self,
        name: &str,
        collection: ResourceCollection,
    ) -> bool {
        if self.insight_rules.contains_key(name) {
            return false;
        }
        if self.insight_rules.len() >= super::MAX_INSIGHT_RULES {
            // The ceiling is reached rather than the name being held already: the rule asked for
            // does not exist and is not created, which is the outcome a caller could act on, so
            // it is reported rather than passing for the idempotent case above.
            ::tracing::warn!(
                rule_name = %name,
                held = self.insight_rules.len(),
                "a Contributor Insights rule was not minted: this store holds as many as it can"
            );
            return false;
        }
        let definition = collection.definition.clone();
        self.insight_rules.insert(
            name.to_string(),
            StoredInsightRule {
                rule: InsightRule {
                    name: name.to_string(),
                    // A rule the service minted to collect data for a resource is collecting
                    // it: "The state of the rule. Valid values are ENABLED and DISABLED."
                    // (`PutInsightRuleInput$RuleState`)
                    state: super::INSIGHT_RULE_ENABLED.to_string(),
                    // "For managed rules, this is `{"Name": "ServiceLogRule", "Version": 1}`"
                    // (`InsightRule$Schema`), reported as the compact schema that value names.
                    schema: super::insight_rule_schema(super::MANAGED_INSIGHT_RULE_SCHEMA_NAME),
                    definition,
                    // "An optional built-in rule that Amazon Web Services manages."
                    // (`InsightRule$ManagedRule`) — this one was minted by the service that
                    // owns the resource, not written by the caller.
                    managed_rule: Some(true),
                    apply_on_transformed_logs: Some(false),
                },
                // Not a log rule: it collects from a resource, so it selects no log group and an
                // ingest attributes nothing to it.
                collection: None,
                observations: Vec::new(),
                tags: Vec::new(),
                // Nothing enabled it from a managed-rule template, so
                // `ListManagedInsightRules` reports no template for it.
                managed: None,
                resource: Some(collection),
            },
        );
        true
    }

    /// The names of the rules this store holds for `resource_arn`, ascending.
    #[must_use]
    pub fn resource_insight_rule_names(&self, resource_arn: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .insight_rules
            .iter()
            .filter(|(_, stored)| {
                stored
                    .resource
                    .as_ref()
                    .is_some_and(|collection| collection.resource_arn == resource_arn)
            })
            .map(|(name, _)| name.clone())
            .collect();
        names.sort_unstable();
        names
    }

    /// Delete the rules this store holds for `resource_arn`, answering their names, ascending.
    ///
    /// Only the rules minted FOR that resource go: a rule of another resource, and a rule a
    /// caller wrote, are left exactly as they were.
    pub fn delete_resource_insight_rules(&mut self, resource_arn: &str) -> Vec<String> {
        let names = self.resource_insight_rule_names(resource_arn);
        for name in &names {
            self.insight_rules.remove(name);
        }
        names
    }

    /// The rules COLLECTING for `resource_arn`, each with the collection it collects under: a
    /// rule that is not ENABLED is not evaluating anything, so it collects nothing while it is
    /// disabled.
    ///
    /// At most [`super::insight_rules::MAX_COLLECTING_RULES_PER_INGEST`] of them answer one
    /// access, the ceiling the log path holds the same factor to: the work one access costs is
    /// the rules collecting for the resource, and a rule past the ceiling collects nothing from
    /// it.
    #[must_use]
    fn resource_rules_collecting(&self, resource_arn: &str) -> Vec<(String, ResourceCollection)> {
        self.insight_rules
            .iter()
            .filter(|(_, stored)| stored.rule.state == super::INSIGHT_RULE_ENABLED)
            .filter_map(|(name, stored)| {
                stored
                    .resource
                    .as_ref()
                    .filter(|collection| collection.resource_arn == resource_arn)
                    .map(|collection| (name.clone(), collection.clone()))
            })
            .take(super::insight_rules::MAX_COLLECTING_RULES_PER_INGEST)
            .collect()
    }
}

/// The observation `contribution` makes to a rule collecting under `collection`, or `None` when
/// the contribution is not one that rule collects: another measure, or fewer key values than the
/// rule's labels — a contributor the rule could not name.
#[must_use]
fn observation(
    collection: &ResourceCollection,
    contribution: &ResourceContribution,
) -> Option<ContributorObservation> {
    if collection.measure != contribution.measure {
        return None;
    }
    if collection.key_labels.is_empty() || contribution.keys.len() < collection.key_labels.len() {
        return None;
    }
    Some(ContributorObservation {
        timestamp_ms: contribution.timestamp_ms,
        keys: contribution.keys[..collection.key_labels.len()].to_vec(),
        value: contribution.value,
    })
}

/// Mint `rules` — `(name, collection)` — in the CloudWatch store the shared engine holds,
/// answering the names it now holds for the resources they collect for.
///
/// The seam a service crate calls when it turns Contributor Insights on for one of its
/// resources. Nothing is minted when that store is not mounted, which is the one case this
/// answers nothing for.
pub fn mint_rules<Eng: Send + Sync + 'static>(
    engine: &Eng,
    rules: &[(String, ResourceCollection)],
) -> Vec<String> {
    let Some(state) = super::service_state(engine) else {
        return Vec::new();
    };
    let mut store = state.lock();
    let mut minted = Vec::with_capacity(rules.len());
    for (name, collection) in rules {
        if store.mint_resource_insight_rule(name, collection.clone()) {
            minted.push(name.clone());
        }
    }
    minted
}

/// The names of the Contributor Insights rules that store holds for `resource_arn`, ascending —
/// the rules a reader of the resource reports, read from the store that holds them rather than
/// from anything the minting service remembered.
pub fn rules_for_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_arn: &str,
) -> Vec<String> {
    super::service_state(engine)
        .map(|state| state.lock().resource_insight_rule_names(resource_arn))
        .unwrap_or_default()
}

/// Delete the rules that store holds for `resource_arn`, answering their names.
///
/// The seam a service crate calls when it turns Contributor Insights off for one of its
/// resources.
pub fn delete_rules_for_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_arn: &str,
) -> Vec<String> {
    super::service_state(engine)
        .map(|state| state.lock().delete_resource_insight_rules(resource_arn))
        .unwrap_or_default()
}

/// Delete the SERVICE-MINTED rules named in `names`, answering the names deleted.
///
/// The seam a service crate calls when one of its resources stops collecting part of what it
/// was collecting — a narrowed configuration — rather than stopping altogether. Only a rule
/// this store holds as a minted one goes: a caller's own rule that happens to carry one of
/// those names is left alone, since nothing the minting service configured created it.
pub fn delete_rules_named<Eng: Send + Sync + 'static>(
    engine: &Eng,
    names: &[String],
) -> Vec<String> {
    let Some(state) = super::service_state(engine) else {
        return Vec::new();
    };
    let mut store = state.lock();
    names
        .iter()
        .filter(|name| {
            store
                .insight_rules
                .get(name.as_str())
                .is_some_and(|stored| stored.resource.is_some())
        })
        .cloned()
        .collect::<Vec<String>>()
        .into_iter()
        .filter(|name| store.insight_rules.remove(name).is_some())
        .collect()
}

/// Attribute `contributions` to the rules collecting for `resource_arn`.
///
/// The seam a service crate's DATA plane calls: one access of the resource contributes to each
/// rule collecting a measure that access carries, as the access happens, in the one store
/// `GetInsightRuleReport` reports from. The caller holds its own store's guard while it calls —
/// the lock order the module docs state — and this takes THIS store's guard twice and briefly,
/// with the per-rule fold in between under no guard of this store at all.
pub fn record_contributions<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_arn: &str,
    contributions: &[ResourceContribution],
) {
    if contributions.is_empty() {
        return;
    }
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let collecting = state.lock().resource_rules_collecting(resource_arn);
    if collecting.is_empty() {
        return;
    }
    // Computed with no guard held, exactly as an ingest's evaluation is, and bounded by the
    // rules of one resource times the accesses one request served.
    let collected: Vec<(String, Vec<ContributorObservation>)> = collecting
        .into_iter()
        .map(|(name, collection)| {
            let observations = contributions
                .iter()
                .filter_map(|contribution| observation(&collection, contribution))
                .collect();
            (name, observations)
        })
        .collect();
    state.lock().keep_contributor_observations(&collected);
}
