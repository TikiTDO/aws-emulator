// SPDX-License-Identifier: CC0-1.0

//! The floating-point members of this service's responses, rendered the way AWS renders
//! one on the XML wire: as a decimal carrying a fractional part.
//!
//! # The gap this closes
//!
//! A Smithy `double` leaves an AWS XML response as a decimal — a whole value reads `0.0`,
//! `50.0`, never `0`, `50` — while Rust's own rendering drops the fractional part of a
//! whole value, and the framework's XML writer hands the value straight to `to_string()`.
//! So every whole-valued floating-point member this service answers with left it in a
//! form no AWS service answers with, and a strict parser reading such a member — a target
//! value, the lower bound of a step, a forecast value, an instance-requirement limit —
//! saw an integer where the API documents a decimal.
//!
//! The rule is the PROTOCOL's, not this service's, and the writer it belongs in cannot be
//! edited here (`vendor/` is read-only), so it is applied at the one seam a service owns:
//! each operation is served behind
//! [`service_common::query_decimal_floats::DecimalFloatsOperation`], which runs the
//! framework's own request walk and changes nothing but how a floating-point leaf of the
//! response is written.
//!
//! # Which operations are served through it
//!
//! Every operation whose MODELLED OUTPUT can reach a floating-point member. This model
//! declares two floating-point shapes —
//! `smithy_query(query="shape",shape="MetricScale,NullablePositiveDouble")`, both
//! `double` — and three operations' outputs reach them:
//!
//! * `DescribePolicies`, through a reported policy's `StepAdjustments` bounds, its
//!   `TargetTrackingConfiguration$TargetValue` and its
//!   `PredictiveScalingConfiguration` metric specifications' `TargetValue`;
//! * `DescribeAutoScalingGroups`, through a group's mixed-instances policy, whose
//!   launch-template overrides carry `InstanceRequirements` — `MemoryGiBPerVCpu`,
//!   `TotalLocalStorageGB` and `NetworkBandwidthGbps` state their `Min` and `Max` as
//!   `NullablePositiveDouble`;
//! * `GetPredictiveScalingForecast`, through each forecast's
//!   `PredictiveScalingForecastValues`.
//!
//! An operation whose output declares no floating-point member is not listed, because for
//! it the wrapper is a no-op: the rule reaches a value only where the model puts one.

use std::sync::Arc;

use service_common::query_decimal_floats::DecimalFloatsOperation;
use smithy_server_runtime::{
    ErasedOperation, Operation, OperationRoutingTable, PreDispatchProtocol,
};

/// The envelope a pre-dispatch failure of a wrapped operation renders in: this service
/// speaks `awsQuery` (not the EC2 dialect), which is the same value the generated
/// per-operation shim passes.
const QUERY_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Query { ec2: false };

/// The operations of this service whose response can carry a floating-point member, each
/// served behind the renderer above, keyed by the routing key the protocol resolves.
///
/// A PARTIAL table: a request it does not hold is served by the generated routing table
/// unchanged (see [`crate::service::query_error_envelope`]). Each row is filed under the
/// operation's own [`Operation::NAME`], the model-derived key the generated table uses, so
/// a wrapped operation cannot land under a key the protocol never resolves.
pub fn table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let mut table = OperationRoutingTable::new();
    serve(
        &mut table,
        crate::ops::describe_policies::DescribePolicies::new(Arc::clone(engine)),
    );
    serve(
        &mut table,
        crate::ops::describe_auto_scaling_groups::DescribeAutoScalingGroups::new(Arc::clone(
            engine,
        )),
    );
    serve(
        &mut table,
        crate::ops::get_predictive_scaling_forecast::GetPredictiveScalingForecast::new(
            Arc::clone(engine),
        ),
    );
    table
}

/// File one already-constructed operation under its modelled name, served behind the
/// decimal renderer.
fn serve<O: Operation + ErasedOperation + 'static>(
    table: &mut OperationRoutingTable,
    operation: O,
) {
    table.add_operation(
        O::NAME,
        Box::new(DecimalFloatsOperation::new(operation, QUERY_ENVELOPE)),
    );
}

#[cfg(test)]
mod tests {
    use super::table;
    use std::sync::Arc;

    /// Every operation this module serves is filed under the key the protocol resolves —
    /// its modelled operation name — and nothing else is, so a request for an operation
    /// with no floating-point member in its output still reaches the generated table.
    #[test]
    fn each_wrapped_operation_is_filed_under_its_modelled_name() {
        let table = table(&Arc::new(()));
        for name in [
            "DescribePolicies",
            "DescribeAutoScalingGroups",
            "GetPredictiveScalingForecast",
        ] {
            assert!(
                table.get(name).is_some(),
                "case {name:?}: expected the operation to be served behind the decimal \
                 renderer"
            );
        }
        for name in ["PutScalingPolicy", "DescribeScalingActivities", "Nonesuch"] {
            assert!(
                table.get(name).is_none(),
                "case {name:?}: expected no row, so the generated table serves it"
            );
        }
    }
}
