// SPDX-License-Identifier: CC0-1.0

//! One stored configuration aggregator: the sources it aggregates and the projections
//! the aggregator read paths report it through.

use super::*;

/// One persisted configuration aggregator.
///
/// An aggregator aggregates EITHER individual source accounts
/// ([`Self::account_aggregation_sources`]) OR an organization
/// ([`Self::organization_aggregation_source`]) — never both, which is what
/// `PutConfigurationAggregator` refuses.
#[derive(Debug, Clone)]
pub struct ConfigurationAggregatorEntry {
    pub name: String,
    /// The id Config assigned this aggregator when it was created
    /// (`config-aggregator-<base-36>`). It is the resource segment of the
    /// aggregator's ARN and never changes once minted.
    pub id: String,
    pub arn: String,
    pub account_aggregation_sources: Vec<AccountAggregationSource>,
    pub organization_aggregation_source: Option<OrganizationAggregationSource>,
    pub aggregator_filters: Option<AggregatorFilters>,
    pub created_at: SystemTime,
    /// When the aggregator's sources or filters were last replaced. Equals
    /// [`Self::created_at`] until the first update.
    pub last_updated_at: SystemTime,
    /// Tags are taken at creation and not updated afterwards
    /// (`PutConfigurationAggregator`: "Tags are added at creation and cannot be
    /// updated with this operation ... tags will not be updated, even if they are
    /// different. Use TagResource and UntagResource to update tags after creation"),
    /// which is what changes them from then on.
    pub tags: TagSet,
}

impl ConfigurationAggregatorEntry {
    /// The modelled `ConfigurationAggregator`, read back out of state.
    ///
    /// `CreatedBy` names the Amazon Web Services service that created the aggregator,
    /// so it stays unset for one a caller created.
    pub fn to_shape(&self) -> ConfigurationAggregator {
        ConfigurationAggregator {
            configuration_aggregator_name: Some(self.name.clone()),
            configuration_aggregator_arn: Some(self.arn.clone()),
            account_aggregation_sources: (!self.account_aggregation_sources.is_empty())
                .then(|| self.account_aggregation_sources.clone()),
            organization_aggregation_source: self.organization_aggregation_source.clone(),
            creation_time: Some(timestamp(self.created_at)),
            last_updated_time: Some(timestamp(self.last_updated_at)),
            created_by: None,
            aggregator_filters: self.aggregator_filters.clone(),
        }
    }

    /// Whether this aggregator aggregates the Config data of `account_id` in `region`.
    ///
    /// An aggregator aggregates either listed source accounts, each naming the Regions
    /// it takes in — every Region (`AllAwsRegions`) or the listed `AwsRegions` — or an
    /// organization, which takes in every account of that organization over the same
    /// choice of Regions. An account or a Region no source names is outside the
    /// aggregator's scope, so it holds nothing for it.
    pub fn aggregates_source(&self, account_id: &str, region: &str) -> bool {
        let takes_region = |all_aws_regions: bool, aws_regions: Option<&[String]>| {
            all_aws_regions
                || aws_regions.is_some_and(|regions| regions.iter().any(|named| named == region))
        };
        let listed_account = self.account_aggregation_sources.iter().any(|source| {
            source.account_ids.iter().any(|named| named == account_id)
                && takes_region(source.all_aws_regions, source.aws_regions.as_deref())
        });
        listed_account
            || self
                .organization_aggregation_source
                .as_ref()
                .is_some_and(|source| {
                    takes_region(source.all_aws_regions, source.aws_regions.as_deref())
                })
    }

    /// Whether this aggregator aggregates resources of `resource_type`: every type it
    /// discovers, unless its `AggregatorFilters` carry a resource-type filter, and
    /// then only the types that filter lists.
    ///
    /// A filter lists the values it takes IN ([`AGGREGATOR_FILTER_TYPE_INCLUDE`] is
    /// the only published `AggregatorFilterType`), so a filter that lists no type at
    /// all narrows nothing. The `ServicePrincipal` filter selects which SERVICE-LINKED
    /// recorders' data the aggregator takes in, and a resource in this store's
    /// inventory carries no service principal to match it against, so it narrows
    /// nothing here.
    pub fn aggregates_resource_type(&self, resource_type: &str) -> bool {
        let Some(filter) = self
            .aggregator_filters
            .as_ref()
            .and_then(|filters| filters.resource_type.as_ref())
        else {
            return true;
        };
        let listed = filter.value.as_deref().unwrap_or_default();
        if listed.is_empty() {
            return true;
        }
        match filter.r#type.as_deref() {
            // A filter type this emulator does not know cannot be read as a narrowing,
            // so it takes in every type rather than silently hiding resources.
            Some(kind) if kind != AGGREGATOR_FILTER_TYPE_INCLUDE => true,
            _ => listed.iter().any(|named| named == resource_type),
        }
    }

    /// The sources this aggregator takes in: the kind of source, the sources it names
    /// and the Regions each of them is collected from, or `None` for an aggregator
    /// that names no source at all.
    ///
    /// An aggregator aggregates EITHER one account aggregation source
    /// (`AccountAggregationSourceList`, length 0..1) OR an organization source, so the
    /// sources it takes in are ONE set of source ids collected over ONE set of
    /// Regions: an account source names the accounts it lists, under
    /// [`AGGREGATED_SOURCE_TYPE_ACCOUNT`], and an organization source names the
    /// organization, under [`AGGREGATED_SOURCE_TYPE_ORGANIZATION`] and identified by
    /// `account_id` — the account whose organization this store stands for, which is
    /// the only organization identity it holds.
    ///
    /// Both lists come back SORTED and without repeats, because one (source, Region)
    /// pair is one sync relationship however many times the source names it, and
    /// because the pairs in that order are the order a page of statuses is served in.
    pub fn aggregated_sources<'a>(
        &'a self,
        account_id: &'a str,
        aggregator_region: &'a str,
    ) -> Option<AggregatedSources<'a>> {
        let (source_type, source_ids, source) =
            if let Some(source) = self.account_aggregation_sources.first() {
                let accounts = source.account_ids.iter().map(String::as_str).collect();
                (AGGREGATED_SOURCE_TYPE_ACCOUNT, accounts, Some(source))
            } else {
                (AGGREGATED_SOURCE_TYPE_ORGANIZATION, vec![account_id], None)
            };
        let (all_aws_regions, aws_regions) = match source {
            Some(source) => (source.all_aws_regions, source.aws_regions.as_deref()),
            None => {
                let source = self.organization_aggregation_source.as_ref()?;
                (source.all_aws_regions, source.aws_regions.as_deref())
            }
        };
        let sources = AggregatedSources {
            source_type,
            last_update_status: AGGREGATED_SOURCE_STATUS_SUCCEEDED,
            source_ids: sorted_unique(source_ids),
            aws_regions: sorted_unique(aggregated_regions(
                all_aws_regions,
                aws_regions,
                aggregator_region,
            )),
        };
        (!sources.source_ids.is_empty() && !sources.aws_regions.is_empty()).then_some(sources)
    }

    /// The instant the aggregation of this aggregator's CURRENT sources completes,
    /// which is when each of them first has a sync status to report and the instant
    /// that status was last verified at.
    ///
    /// Writing an aggregator's sources starts the aggregation of those sources; the
    /// status of a relationship exists only once data has moved across it, so until the
    /// clock owes that first aggregation the aggregator reports no status at all (see
    /// [`AGGREGATION_COMPLETES_AFTER`]).
    pub fn aggregated_at(&self) -> SystemTime {
        self.last_updated_at + AGGREGATION_COMPLETES_AFTER
    }

    /// Whether the aggregation of this aggregator's current sources has completed as of
    /// `now`, which is what decides whether there is any source status to report.
    pub fn has_aggregated(&self, now: SystemTime) -> bool {
        now >= self.aggregated_at()
    }

    /// The sync status this aggregator reports for the source `source_id`, of kind
    /// `source_type`, collected from `aws_region`.
    ///
    /// `LastUpdateTime` is the instant the relationship was last verified, which is
    /// when the aggregation of the aggregator's current sources completed
    /// ([`Self::aggregated_at`]), so it is stable across reads and moves only when the
    /// sources themselves do.
    pub fn source_status(
        &self,
        source_id: &str,
        source_type: &str,
        aws_region: &str,
    ) -> AggregatedSourceStatus {
        AggregatedSourceStatus {
            source_id: Some(source_id.to_string()),
            source_type: Some(source_type.to_string()),
            aws_region: Some(aws_region.to_string()),
            last_update_status: Some(AGGREGATED_SOURCE_STATUS_SUCCEEDED.to_string()),
            last_update_time: Some(timestamp(self.aggregated_at())),
            // No sync served here has failed, so there is no error to report: see
            // [`AGGREGATED_SOURCE_STATUS_SUCCEEDED`].
            last_error_code: None,
            last_error_message: None,
        }
    }
}

/// How long after an aggregator's sources are written the aggregation of those sources
/// completes.
///
/// A sync status describes a relationship DATA HAS MOVED ACROSS, so an aggregator whose
/// sources were just written has none to report yet. Neither the model nor the
/// operation's documentation states how long that first aggregation takes, so this span
/// is CHOSEN: it is long enough that a caller reading the statuses straight after
/// writing the sources still sees the aggregator before it has aggregated anything —
/// which is what makes the sequencing observable — and short enough for a poller to
/// wait out. Only the clock moves it, so the read that reports the statuses never
/// writes.
pub const AGGREGATION_COMPLETES_AFTER: Duration = Duration::from_secs(3);
