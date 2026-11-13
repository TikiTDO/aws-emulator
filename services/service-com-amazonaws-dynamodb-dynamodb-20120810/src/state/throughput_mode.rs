// SPDX-License-Identifier: CC0-1.0

//! Which member of a request states the read/write capacity mode the table is created or
//! updated with.
//!
//! It lives beside [`super`] rather than inside it because it is about the REQUEST's
//! members — which of them names the mode — while the parent module is the table and item
//! store itself; `super` re-exports it, so every caller still names it `crate::state::…`.

/// The capacity mode a request states, or `None` for one that states none.
///
/// `CreateTable` and `UpdateTable` each declare TWO members for that one setting, both
/// targeting the `BillingMode` enum: `BillingMode` and `TableThroughputMode`
/// (`smithy_query(query="operation", operation="CreateTable")`). They are two spellings of
/// one fact — the mode a table's description reports under
/// `BillingModeSummary.BillingMode` and under
/// `TableThroughputModeSummary.TableThroughputMode` alike — so a request states the mode
/// through either one, and reading only the first leaves a request that used the second
/// with no mode at all: it would be answered as though it had asked for the default.
///
/// `BillingMode` is read first where a request carries both. The two cannot disagree about
/// a mode they both name without one of them being ignored either way, and this is the ONE
/// place that precedence is decided, so the create and the update cannot differ about it.
///
/// Each member is held to the enum's value set before any handler runs, so what comes back
/// here is a value the model admits.
pub fn requested_throughput_mode<'m>(
    billing_mode: Option<&'m str>,
    throughput_mode: Option<&'m str>,
) -> Option<&'m str> {
    billing_mode.or(throughput_mode)
}

/// The largest provisioned-capacity figure the service accepts for either dimension of a
/// read/write pair.
///
/// `ProvisionedThroughput$ReadCapacityUnits` and `$WriteCapacityUnits` target
/// `PositiveLongObject` — a long with a `@range` minimum of 1 and no maximum
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ProvisionedThroughput")`)
/// — so the MODEL admits every positive long, while the service holds a capacity figure in
/// a 32-bit signed integer: a value that does not fit it is refused as out of bounds while
/// it is still being read, rather than as a quota the account could have raised
/// ([`capacity_out_of_bounds_message`] is that refusal, and it is not a quota sentence).
/// The ceiling is therefore that integer's own maximum.
pub const MAX_PROVISIONED_CAPACITY_UNITS: i64 = i32::MAX as i64;

/// AWS's fixed text for a capacity figure the service cannot hold: the sentence states the
/// value the request supplied and the member it supplied it for, and nothing of the table,
/// the index or the bound — the fact is about the value as it was read.
pub fn capacity_out_of_bounds_message(member: &str, value: i64) -> String {
    format!("Given value {value} for {member} is out of bounds")
}

/// Hold every capacity figure `requested` states to [`MAX_PROVISIONED_CAPACITY_UNITS`],
/// naming the first member that is past it.
///
/// Every `ProvisionedThroughput` a request carries is held to it — the table's own and each
/// index's alike — because the bound is on the FIGURE rather than on the resource it was
/// stated for. Both dimensions are read in the order the model declares them, so a request
/// that states two unusable figures is told about the first.
pub fn check_capacity_bounds<'t>(
    requested: impl IntoIterator<Item = &'t crate::ops::types::ProvisionedThroughput>,
) -> Result<(), super::RequestError> {
    for throughput in requested {
        for (member, value) in [
            ("ReadCapacityUnits", throughput.read_capacity_units),
            ("WriteCapacityUnits", throughput.write_capacity_units),
        ] {
            if value > MAX_PROVISIONED_CAPACITY_UNITS {
                return Err(super::RequestError::Validation(
                    capacity_out_of_bounds_message(member, value),
                ));
            }
        }
    }
    Ok(())
}

/// AWS's fixed text for a VECTOR index asked for on a table that is not in the on-demand
/// capacity mode: the sentence states the capacity mode a vector index needs and nothing of
/// the table or of the index that asked for one.
pub const VECTOR_INDEX_REQUIRES_ON_DEMAND_MESSAGE: &str =
    "One or more parameter values were invalid: Vector indexes are only supported for \
     PAY_PER_REQUEST tables";

/// Refuse `requested` vector indexes on a table whose capacity mode is `billing_mode`
/// unless that mode is the on-demand one.
///
/// A vector index serves searches whose cost is not a provisioned figure, so it exists only
/// on an on-demand table — and the mode is the TABLE's, so the rule holds wherever a vector
/// index is declared: with the table, in a declaration an import creates one from, or
/// through an update's own `Create`. A request that declares none is no defect whatever the
/// mode.
pub fn check_vector_index_capacity_mode(
    billing_mode: &str,
    requested: usize,
) -> Result<(), super::RequestError> {
    if requested > 0 && billing_mode != "PAY_PER_REQUEST" {
        return Err(super::RequestError::Validation(
            VECTOR_INDEX_REQUIRES_ON_DEMAND_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

/// The rule for one INDEX of a provisioned table: "an index's capacity mode is the
/// table's", so every secondary index of a `PROVISIONED` table carries its own capacity.
///
/// AWS's recorded wording for it names the two capacity members and the index they are
/// missing from. The create and the update hold an index to the same rule and stated this
/// sentence in identical words, so it has one home; the words themselves are unchanged.
pub fn index_capacity_required_message(index_name: &str) -> String {
    format!(
        "One or more parameter values were invalid: Both ReadCapacityUnits and \
         WriteCapacityUnits must be specified for index: {index_name}"
    )
}

/// The refusal of a capacity move that MOVES NOTHING: a request that asks for the
/// read/write pair already in force asks for no change at all.
///
/// "The provisioned throughput settings of the table" and of an index
/// (`UpdateTable$ProvisionedThroughput`, `UpdateGlobalSecondaryIndexAction$ProvisionedThroughput`)
/// are what such a request re-states, and a decrease of the table's capacity is a counted,
/// rationed action ("NumberOfDecreasesToday" —
/// `ProvisionedThroughputDescription$NumberOfDecreasesToday`), so a call that would spend
/// one of those on a value that does not differ is permanently invalid rather than applied:
/// AWS refuses it, and the table is not transitioned.
///
/// The sentence states the pair in force and the pair requested, so the caller can see
/// from the message alone that the two are equal; everything else about it is fixed text.
/// `subject` is the resource the request addressed — the table, or the index it named —
/// and `holder` is what the two capacity figures are reported as provisioned for.
pub fn throughput_unchanged_message(
    subject: &str,
    holder: &str,
    current: &crate::ops::types::ProvisionedThroughput,
    requested: &crate::ops::types::ProvisionedThroughput,
) -> String {
    format!(
        "The provisioned throughput for {subject} will not change. The requested value equals \
         the current value. Current ReadCapacityUnits provisioned for {holder}: {}. Requested \
         ReadCapacityUnits: {}. Current WriteCapacityUnits provisioned for {holder}: {}. \
         Requested WriteCapacityUnits: {}. Refer to the Amazon DynamoDB Developer Guide for \
         current limits and how to request higher limits.",
        current.read_capacity_units,
        requested.read_capacity_units,
        current.write_capacity_units,
        requested.write_capacity_units
    )
}
