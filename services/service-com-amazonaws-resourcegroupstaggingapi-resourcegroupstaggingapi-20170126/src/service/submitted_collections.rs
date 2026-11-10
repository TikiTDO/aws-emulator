// SPDX-License-Identifier: CC0-1.0

//! What THIS service states about its own constraint clauses, and the operations it serves
//! through the layer that states them.
//!
//! The rule, the wrapper and every guard around it are the tree's shared ones
//! (`service_common::submitted_collections`, the one home for the half of a constraint
//! refusal the framework's walk cannot state — the COLLECTION a clause about a collection
//! member is about: a `@length` on a map or a list is checked ON THE MEMBER, so the only
//! rendering the framework has for the rejected value is a summary of its size, where AWS
//! states the collection itself).
//!
//! This file supplies only the facts that are this service's own: the error code its
//! constraint refusals carry, and the order it states a map's entries in — the order the
//! REQUEST wrote them, which the built input (a `HashMap`) does not keep.

use std::sync::Arc;

use service_common::submitted_collections::{StatedClause, StatedClauseTable};
use smithy_server_runtime::OperationRoutingTable;

/// The error code a constraint refusal carries, which is the one the framework's own
/// renderer picks for it on this service's JSON protocol: the restated message travels with
/// the code it would have travelled with, because a client branches on the code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Serve one `crate::ops::<module>::<Operation>` per line through the shared layer, each
/// built from the shared engine and filed under its own modelled name.
macro_rules! serve {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.quoting(
                crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
            );
        )+
    };
}

/// The same modelled operations the generated table holds, each served so that a constraint
/// clause about a collection member states the collection the request carried, and built
/// from the SAME shared engine, so a wrapped operation reads and writes the one store.
pub(super) fn quoting_submitted_collections<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> OperationRoutingTable {
    let mut table = StatedClauseTable::new(StatedClause::under(VALIDATION_EXCEPTION));
    serve!(
        table,
        engine,
        describe_report_creation::DescribeReportCreation,
        get_compliance_summary::GetComplianceSummary,
        get_resources::GetResources,
        get_tag_keys::GetTagKeys,
        get_tag_values::GetTagValues,
        list_required_tags::ListRequiredTags,
        start_report_creation::StartReportCreation,
        tag_resources::TagResources,
        untag_resources::UntagResources,
    );
    table.finish()
}
