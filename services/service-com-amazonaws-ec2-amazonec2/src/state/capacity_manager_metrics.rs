// SPDX-License-Identifier: CC0-1.0

//! The capacity resources EC2 Capacity Manager measures, read as DIMENSION VALUES.
//!
//! Capacity Manager answers questions about capacity over a WINDOW of time, and every
//! question is asked along the same set of dimensions: "the available dimension values for
//! capacity metrics within a specified time range ... for discovering what accounts,
//! regions, instance families, and other dimensions have data available for filtering and
//! grouping" (`GetCapacityManagerMetricDimensions`). So this module holds the one reading
//! of "which capacity resource carries which dimension value", which the operation that
//! reports the available dimensions answers from — and which the operation that reports the
//! metric data itself (`GetCapacityManagerMetricData`, still a scaffold) asks the same way
//! when it is written, so the two cannot disagree about what the account has data for.
//!
//! Three things live here:
//!
//! * [`CapacityMetricSubject`], WHICH capacity resources a metric measures, read off the
//!   metric's own name — the modelled `Metric` catalog is grouped by subject, and
//!   "only dimensions that have data for these metrics will be returned"
//!   (`GetCapacityManagerMetricDimensionsRequest$MetricNames`) is a question about that
//!   subject's resources;
//! * [`ComAmazonawsEc2Amazonec2State::capacity_manager_dimension_rows`], the dimension
//!   values each such resource of the account carries — a REPORTING read, derived from the
//!   stored resources and the clock, so asking writes nothing;
//! * the readings of ONE dimension of a row ([`capacity_manager_dimension_value`]), of a
//!   row narrowed to the dimensions a request groups by
//!   ([`capacity_manager_dimension_projection`]) and of the filter conditions a request
//!   carries ([`capacity_manager_condition_holds`]).
//!
//! A resource carries a value for a dimension only when the resource HAS one: a reservation
//! with no end date carries no `reservation-end-timestamp`, and an instance carries no
//! reservation identity at all. A combination is "available" only when every dimension it
//! groups by carries a value, which is what [`capacity_manager_dimension_projection`]
//! answers `None` for.
//!
//! WHERE THIS IS COARSER THAN AWS, and why: nothing in this store links an instance to the
//! Capacity Reservation whose capacity it is using — a launch records only its
//! `CapacityReservationPreference`, never a target reservation (`instance_detail`) — so an
//! instance no Spot request launched counts as capacity no reservation covers, whether or not
//! a reservation of the account would have matched it. The split would need the launch path to
//! record which reservation an instance consumed; until it does, the alternative is reporting
//! no `unreserved-…` dimensions at all, which would hide capacity the account really is
//! using.

use super::*;

use crate::ops::types::CapacityManagerDimension;

/// Which capacity resources one of Capacity Manager's metrics measures.
///
/// The modelled `Metric` catalog (`smithy_query(query="shape",shape="Metric")`) is grouped
/// by subject in the metric names themselves, and each group measures a different resource
/// of this store, so the subject is what decides where a dimension row comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapacityMetricSubject {
    /// The account's Capacity Reservations — the `reservation-…` metrics, which measure the
    /// reserved capacity itself, and the `reserved-…` metrics, which measure the usage that
    /// capacity covered.
    Reservations,
    /// Spot capacity — the `spot-…` metrics, which measure the usage, interruptions and
    /// run time of instances launched from a Spot Instance request.
    SpotInstances,
    /// Capacity covered by no reservation — the `unreserved-…` metrics, which measure the
    /// usage of instances that are neither Spot nor reserved.
    UnreservedInstances,
}

/// The metric-name prefixes whose metrics measure a Capacity Reservation: the reserved
/// capacity (`reservation-…`) and the usage that capacity covered (`reserved-…`).
pub const RESERVATION_METRIC_PREFIXES: [&str; 2] = ["reservation-", "reserved-"];

/// The metric-name prefix of the metrics that measure Spot capacity.
pub const SPOT_METRIC_PREFIX: &str = "spot-";

/// The metric-name prefix of the metrics that measure capacity no reservation covers.
pub const UNRESERVED_METRIC_PREFIX: &str = "unreserved-";

/// What `metric` measures, or `None` for a name that is no modelled metric.
///
/// Every member of the modelled `Metric` catalog carries one of the prefixes above, so a
/// name this answers `None` for is one the catalog does not hold — which the operation
/// refuses rather than reporting dimensions for.
pub fn capacity_metric_subject(metric: &str) -> Option<CapacityMetricSubject> {
    if RESERVATION_METRIC_PREFIXES
        .iter()
        .any(|prefix| metric.starts_with(prefix))
    {
        return Some(CapacityMetricSubject::Reservations);
    }
    if metric.starts_with(SPOT_METRIC_PREFIX) {
        return Some(CapacityMetricSubject::SpotInstances);
    }
    if metric.starts_with(UNRESERVED_METRIC_PREFIX) {
        return Some(CapacityMetricSubject::UnreservedInstances);
    }
    None
}

/// `resource-region`: "the Amazon Web Services Region where the capacity resource is
/// located" (`CapacityManagerDimension$ResourceRegion`). The name is the modelled `GroupBy`
/// / `FilterByDimension` member, whose whole catalog is the generated
/// [`crate::ops::types::GROUP_BY_VALUES`].
pub const DIMENSION_RESOURCE_REGION: &str = "resource-region";

/// `availability-zone-id`: the zone the capacity resource is in, by id.
pub const DIMENSION_AVAILABILITY_ZONE_ID: &str = "availability-zone-id";

/// `account-id`: "the Amazon Web Services account ID that owns the capacity resource".
pub const DIMENSION_ACCOUNT_ID: &str = "account-id";

/// `account-name`: the owning account's NAME, which "is only available when Organizations
/// access is enabled for Capacity Manager" (`CapacityManagerDimension$AccountName`).
pub const DIMENSION_ACCOUNT_NAME: &str = "account-name";

/// `instance-family`: the EC2 instance family of the capacity resource.
pub const DIMENSION_INSTANCE_FAMILY: &str = "instance-family";

/// `instance-type`: the specific EC2 instance type of the capacity resource.
pub const DIMENSION_INSTANCE_TYPE: &str = "instance-type";

/// `instance-platform`: the platform or operating system of the instance.
pub const DIMENSION_INSTANCE_PLATFORM: &str = "instance-platform";

/// `reservation-arn`: the ARN of the capacity reservation.
pub const DIMENSION_RESERVATION_ARN: &str = "reservation-arn";

/// `reservation-id`: the id of the capacity reservation.
pub const DIMENSION_RESERVATION_ID: &str = "reservation-id";

/// `reservation-type`: which kind of capacity reservation it is.
pub const DIMENSION_RESERVATION_TYPE: &str = "reservation-type";

/// `reservation-create-timestamp`: when the reservation was created.
pub const DIMENSION_RESERVATION_CREATE_TIMESTAMP: &str = "reservation-create-timestamp";

/// `reservation-start-timestamp`: when the reservation becomes available for use.
pub const DIMENSION_RESERVATION_START_TIMESTAMP: &str = "reservation-start-timestamp";

/// `reservation-end-timestamp`: when the reservation expires.
pub const DIMENSION_RESERVATION_END_TIMESTAMP: &str = "reservation-end-timestamp";

/// `reservation-end-date-type`: whether the reservation has a fixed end date.
pub const DIMENSION_RESERVATION_END_DATE_TYPE: &str = "reservation-end-date-type";

/// `tenancy`: the tenancy of the hardware the capacity runs on.
pub const DIMENSION_TENANCY: &str = "tenancy";

/// `reservation-state`: the current state of the capacity reservation.
pub const DIMENSION_RESERVATION_STATE: &str = "reservation-state";

/// `reservation-instance-match-criteria`: how instances are matched to the reservation.
pub const DIMENSION_RESERVATION_INSTANCE_MATCH_CRITERIA: &str =
    "reservation-instance-match-criteria";

/// `reservation-unused-financial-owner`: "the Amazon Web Services account ID that is
/// financially responsible for unused capacity reservation costs".
pub const DIMENSION_RESERVATION_UNUSED_FINANCIAL_OWNER: &str =
    "reservation-unused-financial-owner";

/// The `ReservationType` of an on-demand Capacity Reservation
/// (`smithy_query(query="shape",shape="ReservationType")`), which is what Capacity Manager
/// calls a reservation the account made itself — the `default`
/// [`CAPACITY_RESERVATION_TYPE_DEFAULT`] of `CapacityReservation$ReservationType`.
pub const DIMENSION_RESERVATION_TYPE_ODCR: &str = "odcr";

/// That same enum's `capacity-block`: the capacity of a Capacity Block, which the
/// reservation carries a [`CAPACITY_RESERVATION_TYPE_CAPACITY_BLOCK`] type and a
/// `CapacityBlockId` for.
pub const DIMENSION_RESERVATION_TYPE_CAPACITY_BLOCK: &str = "capacity-block";

/// Which `ReservationType` Capacity Manager reports for one stored Capacity Reservation.
///
/// `CapacityReservation$ReservationType` names the kinds a reservation is CREATED as
/// (`default` or `capacity-block`) while the dimension's own `ReservationType` names them
/// `odcr` and `capacity-block`, so the two catalogs are mapped here in one place: capacity
/// bought as a Capacity Block — which also carries that block's `CapacityBlockId` — is
/// `capacity-block`, and every other reservation is an on-demand one.
pub fn capacity_manager_reservation_type(reservation: &CapacityReservation) -> &'static str {
    let capacity_block = reservation.capacity_block_id.is_some()
        || reservation.reservation_type.as_deref() == Some(CAPACITY_RESERVATION_TYPE_CAPACITY_BLOCK);
    if capacity_block {
        DIMENSION_RESERVATION_TYPE_CAPACITY_BLOCK
    } else {
        DIMENSION_RESERVATION_TYPE_ODCR
    }
}

/// Does a capacity resource whose own window is `[resource_start, resource_end]` have data
/// inside the window `[query_start, query_end]` a request asked about?
///
/// "Only dimensions with data in this time range will be returned"
/// (`GetCapacityManagerMetricDimensionsRequest$StartTime`), so the test is an OVERLAP of the
/// two windows rather than containment: a reservation that started before the window and
/// runs past it had data throughout. An open end — a reservation with no end date, or usage
/// nothing has recorded the end of — is a window that has not closed, so it overlaps every
/// window that starts at all.
pub fn capacity_window_overlaps(
    resource_start: Option<::aws_smithy_types::DateTime>,
    resource_end: Option<::aws_smithy_types::DateTime>,
    query_start: ::aws_smithy_types::DateTime,
    query_end: ::aws_smithy_types::DateTime,
) -> bool {
    resource_start.is_none_or(|start| date_time_key(start) <= date_time_key(query_end))
        && resource_end.is_none_or(|end| date_time_key(end) >= date_time_key(query_start))
}

impl ComAmazonawsEc2Amazonec2State {
    /// The dimension values every capacity resource of `subject` with data in
    /// `[query_start, query_end]` carries, as of `now`.
    ///
    /// REPORTING ONLY: every row is derived from a stored resource and the clock, so two
    /// reads at one instant answer the same rows and a read changes nothing a later caller
    /// sees. One row per resource, with every dimension that resource carries filled in —
    /// narrowing to the dimensions a request asked about, and folding the rows that then
    /// agree into one combination, is the reader's own job
    /// ([`capacity_manager_dimension_projection`]).
    pub fn capacity_manager_dimension_rows(
        &self,
        subject: CapacityMetricSubject,
        query_start: ::aws_smithy_types::DateTime,
        query_end: ::aws_smithy_types::DateTime,
        now: SystemTime,
    ) -> Vec<CapacityManagerDimension> {
        match subject {
            CapacityMetricSubject::Reservations => {
                self.reserved_capacity_dimension_rows(query_start, query_end, now)
            }
            CapacityMetricSubject::SpotInstances => {
                self.instance_capacity_dimension_rows(true, query_start, query_end)
            }
            CapacityMetricSubject::UnreservedInstances => {
                self.instance_capacity_dimension_rows(false, query_start, query_end)
            }
        }
    }

    /// The dimension values of the account's Capacity Reservations with capacity in the
    /// window — the rows the `reservation-…` and `reserved-…` metrics are measured along.
    ///
    /// A reservation's own window runs from when its capacity starts ("the timestamp when
    /// the capacity reservation becomes active and available for use" — its `StartDate`,
    /// falling back to the `CreateDate` of one created for immediate use) to its `EndDate`,
    /// which an `unlimited` reservation does not have. The state is read through the
    /// reservation record's own [`CapacityReservationRecord::view_at`], so the state a
    /// dimension reports is the state `DescribeCapacityReservations` reports at the same
    /// instant.
    fn reserved_capacity_dimension_rows(
        &self,
        query_start: ::aws_smithy_types::DateTime,
        query_end: ::aws_smithy_types::DateTime,
        now: SystemTime,
    ) -> Vec<CapacityManagerDimension> {
        self.capacity_reservations
            .values()
            .filter_map(|record| {
                let reservation = record.view_at(now);
                let starts = reservation.start_date.or(reservation.create_date);
                if !capacity_window_overlaps(starts, reservation.end_date, query_start, query_end) {
                    return None;
                }
                Some(CapacityManagerDimension {
                    resource_region: reservation
                        .availability_zone
                        .as_deref()
                        .map(|zone| region_of_zone_name(zone).to_string()),
                    availability_zone_id: reservation.availability_zone_id.clone(),
                    account_id: reservation.owner_id.clone(),
                    // The owning account's NAME is an Organizations fact, and nothing in
                    // this emulator names an account, so no row carries one — the same
                    // answer AWS gives an account whose Capacity Manager has no
                    // Organizations access.
                    account_name: None,
                    instance_family: reservation
                        .instance_type
                        .as_deref()
                        .map(|instance_type| instance_family_of(instance_type).to_string()),
                    instance_type: reservation.instance_type.clone(),
                    instance_platform: reservation.instance_platform.clone(),
                    reservation_arn: reservation.capacity_reservation_arn.clone(),
                    reservation_id: reservation.capacity_reservation_id.clone(),
                    reservation_type: Some(
                        capacity_manager_reservation_type(&reservation).to_string(),
                    ),
                    reservation_create_timestamp: reservation.create_date,
                    reservation_start_timestamp: reservation.start_date,
                    reservation_end_timestamp: reservation.end_date,
                    reservation_end_date_type: reservation.end_date_type.clone(),
                    tenancy: reservation.tenancy.clone(),
                    reservation_state: reservation.state.clone(),
                    reservation_instance_match_criteria: reservation.instance_match_criteria.clone(),
                    reservation_unused_financial_owner: reservation
                        .unused_reservation_billing_owner_id
                        .clone(),
                    // "Only tags that have been activated for monitoring via
                    // UpdateCapacityManagerMonitoredTagKeys are included"
                    // (`CapacityManagerDimension$Tags`). A reported combination carries
                    // exactly the dimensions the request grouped by
                    // ([`capacity_manager_dimension_projection`]) and the modelled
                    // `GroupBy` catalog names no tag dimension at all, so no combination of
                    // this report is one along a tag key and a row carries none — rather
                    // than this reporting every tag a resource happens to have, monitored
                    // or not.
                    tags: None,
                })
            })
            .collect()
    }

    /// The dimension values of the account's INSTANCE capacity in the window: the Spot
    /// instances when `spot` is set, and the instances no Spot request launched otherwise —
    /// the rows the `spot-…` and `unreserved-…` metrics are measured along.
    ///
    /// An instance carries the placement and machine dimensions of the capacity it uses and
    /// NO reservation identity — the modelled dimension shape's reservation members are
    /// facts of a reservation — so a request grouping by one of those gets no combination
    /// from instance usage, which is what "only dimensions that have data for these metrics
    /// will be returned" says.
    ///
    /// The usage window starts when the instance launched and, for an instance the account
    /// still holds, has no recorded end: nothing in this store records WHEN an instance
    /// stopped using capacity, so that window is open ([`capacity_window_overlaps`]) and an
    /// instance launched before the query's end has data in it. A TERMINATED instance is
    /// skipped entirely — it holds no capacity at all any more, and the store cannot date the
    /// capacity it once held, so reporting its dimensions would claim capacity the account no
    /// longer has.
    fn instance_capacity_dimension_rows(
        &self,
        spot: bool,
        query_start: ::aws_smithy_types::DateTime,
        query_end: ::aws_smithy_types::DateTime,
    ) -> Vec<CapacityManagerDimension> {
        self.instances
            .values()
            .filter_map(|instance| {
                if instance.spot_instance_request_id.is_some() != spot {
                    return None;
                }
                if instance.state == InstanceStateName::Terminated {
                    return None;
                }
                let launched = system_time_to_date_time(instance.launch_time);
                if !capacity_window_overlaps(Some(launched), None, query_start, query_end) {
                    return None;
                }
                // The ONE projection of a stored instance, so the platform and the tenancy
                // a dimension reports are the ones `DescribeInstances` reports.
                let view = self.instance_view(instance);
                let placement = view.placement.as_ref();
                Some(CapacityManagerDimension {
                    resource_region: Some(
                        region_of_zone_name(&instance.placement_az).to_string(),
                    ),
                    availability_zone_id: placement
                        .and_then(|placement| placement.availability_zone_id.clone()),
                    account_id: Some(
                        self.reservations
                            .get(&instance.reservation_id)
                            .map(|reservation| reservation.owner_id.clone())
                            .unwrap_or_else(|| ::engine::emulator_account_id().to_string()),
                    ),
                    instance_family: Some(
                        instance_family_of(&instance.instance_type).to_string(),
                    ),
                    instance_type: Some(instance.instance_type.clone()),
                    instance_platform: view.platform_details.clone(),
                    tenancy: placement.and_then(|placement| placement.tenancy.clone()),
                    ..CapacityManagerDimension::default()
                })
            })
            .collect()
    }
}

/// The value `dimension` names on `row`, as the wire carries it — `None` when the resource
/// the row came from carries no value for that dimension, and for a name that is no modelled
/// dimension at all.
///
/// A timestamp is read in the ISO 8601 rendering the wire carries it as
/// ([`filter_timestamp_values`]), which is also what a filter condition's own values are
/// written in, so a comparison is made on what the caller was shown.
pub fn capacity_manager_dimension_value(
    row: &CapacityManagerDimension,
    dimension: &str,
) -> Option<String> {
    let timestamp = |at: Option<::aws_smithy_types::DateTime>| {
        filter_timestamp_values(at)
            .into_iter()
            .next()
            .map(|value| value.into_owned())
    };
    match dimension {
        DIMENSION_RESOURCE_REGION => row.resource_region.clone(),
        DIMENSION_AVAILABILITY_ZONE_ID => row.availability_zone_id.clone(),
        DIMENSION_ACCOUNT_ID => row.account_id.clone(),
        DIMENSION_ACCOUNT_NAME => row.account_name.clone(),
        DIMENSION_INSTANCE_FAMILY => row.instance_family.clone(),
        DIMENSION_INSTANCE_TYPE => row.instance_type.clone(),
        DIMENSION_INSTANCE_PLATFORM => row.instance_platform.clone(),
        DIMENSION_RESERVATION_ARN => row.reservation_arn.clone(),
        DIMENSION_RESERVATION_ID => row.reservation_id.clone(),
        DIMENSION_RESERVATION_TYPE => row.reservation_type.clone(),
        DIMENSION_RESERVATION_CREATE_TIMESTAMP => timestamp(row.reservation_create_timestamp),
        DIMENSION_RESERVATION_START_TIMESTAMP => timestamp(row.reservation_start_timestamp),
        DIMENSION_RESERVATION_END_TIMESTAMP => timestamp(row.reservation_end_timestamp),
        DIMENSION_RESERVATION_END_DATE_TYPE => row.reservation_end_date_type.clone(),
        DIMENSION_TENANCY => row.tenancy.clone(),
        DIMENSION_RESERVATION_STATE => row.reservation_state.clone(),
        DIMENSION_RESERVATION_INSTANCE_MATCH_CRITERIA => {
            row.reservation_instance_match_criteria.clone()
        }
        DIMENSION_RESERVATION_UNUSED_FINANCIAL_OWNER => {
            row.reservation_unused_financial_owner.clone()
        }
        _ => None,
    }
}

/// `row` narrowed to `group_by` — the COMBINATION that row stands for — or `None` when the
/// resource carries no value for one of those dimensions.
///
/// "The dimensions to group by when retrieving available dimension values. This determines
/// which dimension combinations are returned"
/// (`GetCapacityManagerMetricDimensionsRequest$GroupBy`), so a reported combination carries
/// exactly the grouped dimensions and nothing else: two resources that agree on them are one
/// combination. A resource carrying no value for a grouped dimension has no data for that
/// combination at all, which is why that is `None` here rather than a combination with a
/// member missing.
pub fn capacity_manager_dimension_projection(
    row: &CapacityManagerDimension,
    group_by: &[String],
) -> Option<CapacityManagerDimension> {
    let mut projected = CapacityManagerDimension::default();
    for dimension in group_by {
        match dimension.as_str() {
            DIMENSION_RESOURCE_REGION => {
                projected.resource_region = Some(row.resource_region.clone()?);
            }
            DIMENSION_AVAILABILITY_ZONE_ID => {
                projected.availability_zone_id = Some(row.availability_zone_id.clone()?);
            }
            DIMENSION_ACCOUNT_ID => projected.account_id = Some(row.account_id.clone()?),
            DIMENSION_ACCOUNT_NAME => projected.account_name = Some(row.account_name.clone()?),
            DIMENSION_INSTANCE_FAMILY => {
                projected.instance_family = Some(row.instance_family.clone()?);
            }
            DIMENSION_INSTANCE_TYPE => projected.instance_type = Some(row.instance_type.clone()?),
            DIMENSION_INSTANCE_PLATFORM => {
                projected.instance_platform = Some(row.instance_platform.clone()?);
            }
            DIMENSION_RESERVATION_ARN => {
                projected.reservation_arn = Some(row.reservation_arn.clone()?);
            }
            DIMENSION_RESERVATION_ID => {
                projected.reservation_id = Some(row.reservation_id.clone()?);
            }
            DIMENSION_RESERVATION_TYPE => {
                projected.reservation_type = Some(row.reservation_type.clone()?);
            }
            DIMENSION_RESERVATION_CREATE_TIMESTAMP => {
                projected.reservation_create_timestamp = Some(row.reservation_create_timestamp?);
            }
            DIMENSION_RESERVATION_START_TIMESTAMP => {
                projected.reservation_start_timestamp = Some(row.reservation_start_timestamp?);
            }
            DIMENSION_RESERVATION_END_TIMESTAMP => {
                projected.reservation_end_timestamp = Some(row.reservation_end_timestamp?);
            }
            DIMENSION_RESERVATION_END_DATE_TYPE => {
                projected.reservation_end_date_type = Some(row.reservation_end_date_type.clone()?);
            }
            DIMENSION_TENANCY => projected.tenancy = Some(row.tenancy.clone()?),
            DIMENSION_RESERVATION_STATE => {
                projected.reservation_state = Some(row.reservation_state.clone()?);
            }
            DIMENSION_RESERVATION_INSTANCE_MATCH_CRITERIA => {
                projected.reservation_instance_match_criteria =
                    Some(row.reservation_instance_match_criteria.clone()?);
            }
            DIMENSION_RESERVATION_UNUSED_FINANCIAL_OWNER => {
                projected.reservation_unused_financial_owner =
                    Some(row.reservation_unused_financial_owner.clone()?);
            }
            // Unreachable: the reporting operation refuses a dimension the catalog does not
            // hold before it reads anything. A name nothing carries a value for could never
            // name a combination either way.
            _ => return None,
        }
    }
    Some(projected)
}

/// What separates one dimension's value from the next in the KEY a combination is folded
/// and ordered by: a control character no Region, zone, account id, instance type, platform,
/// ARN, id, state or timestamp of this service carries, so two different combinations can
/// never key the same.
pub const DIMENSION_KEY_SEPARATOR: char = '\u{1}';

/// The key one combination of `row` over `group_by` is folded and ordered by: its dimension
/// values in the order the request grouped by them.
///
/// Both the identity of a combination (two rows with one key are one combination) and the
/// total order the report pages over, so a pagination cursor can name the combination the
/// next page starts after.
pub fn capacity_manager_dimension_key(
    row: &CapacityManagerDimension,
    group_by: &[String],
) -> String {
    group_by
        .iter()
        .map(|dimension| capacity_manager_dimension_value(row, dimension).unwrap_or_default())
        .collect::<Vec<String>>()
        .join(&DIMENSION_KEY_SEPARATOR.to_string())
}

/// `Comparison`'s `equals`: the condition holds for the dimension value the first of its
/// values names (`smithy_query(query="shape",shape="Comparison")`).
pub const CAPACITY_MANAGER_COMPARISON_EQUALS: &str = "equals";

/// `Comparison`'s `in`: the condition holds for a dimension value ANY of its values names.
pub const CAPACITY_MANAGER_COMPARISON_IN: &str = "in";

/// Does the condition `(dimension, comparison, values)` hold for `row`?
///
/// "For 'equals' comparison, only the first value is used. For 'in' comparison, any matching
/// value will satisfy the condition" (`DimensionCondition$Values`). A row carrying no value
/// for the dimension satisfies neither, so a condition narrows to the resources that HAVE
/// the dimension it names.
///
/// Every comparison is charged to `budget`, the one countdown a request's whole filtering
/// shares ([`MatchBudget`]): the cost of answering a request is the product of the rows it
/// is asked over, the conditions it carries and the values each of them names, and only a
/// cap on that PRODUCT bounds it however the caller combines the three. A spent budget
/// answers "no match" here and is reported as [`MatchBudget::exhausted`] by the caller,
/// which is a different answer from "nothing matched".
pub fn capacity_manager_condition_holds(
    row: &CapacityManagerDimension,
    dimension: &str,
    comparison: &str,
    values: &[String],
    budget: &mut MatchBudget,
) -> bool {
    // Reading the row's own value for the dimension is a step of its own, so a request
    // carrying conditions that match nothing is charged for the work of finding that out.
    if !budget.spend() {
        return false;
    }
    let Some(held) = capacity_manager_dimension_value(row, dimension) else {
        return false;
    };
    // Every comparison the modelled `Comparison` catalog holds, named explicitly: a value
    // the catalog does not hold (which the boundary refuses before a handler runs) compares
    // against nothing rather than inheriting another comparison's meaning.
    match comparison {
        CAPACITY_MANAGER_COMPARISON_EQUALS => values
            .first()
            .is_some_and(|value| budget.spend() && *value == held),
        CAPACITY_MANAGER_COMPARISON_IN => {
            values.iter().any(|value| budget.spend() && *value == held)
        }
        _ => false,
    }
}
