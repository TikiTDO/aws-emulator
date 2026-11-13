// SPDX-License-Identifier: CC0-1.0

//! What one SERVED call costs the API it named: the capacity it consumed, and the
//! observations it left for the stores that report them.
//!
//! Step Functions meters its APIs per operation: every call of an API consumes capacity
//! of that API's own bucket, and the series it is counted in
//! ([`crate::state::observability::CONSUMED_CAPACITY_METRIC`], dimensioned on
//! [`crate::state::observability::API_NAME_DIMENSION`]) is what lets a client see how
//! close its own traffic is to the quota of the API it is making — a signal no series
//! about the executions can carry, because the capacity is consumed by the REQUEST
//! whatever that request then did.
//!
//! Metering is a property of SERVING a call and not of any one operation, exactly as the
//! management event ([`super::management_events`]) is, so it is not written per handler:
//! this wrapper sits around the one operation every admitted request of this service is
//! served through, so no handler can forget it and none counts a call twice. It is also
//! where everything that call queued about the records it touched is handed over
//! ([`crate::state::observability::publish`]), with no store of this service locked:
//! a count of what a state machine HOLDS is owed to no clock, so without this it would
//! wait for the next caller.
//!
//! A request refused BEFORE it is served — one IAM declined ([`super::identity_authorization`])
//! — reaches no API of this service, so it consumes no capacity of one. Neither does a request
//! whose routing key names no operation of this service: the name a call is counted under is
//! resolved against this service's own catalog of its operations, so the values the API-name
//! dimension can take are the model's rather than a caller's, however many requests are made.

use ::std::sync::Arc;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// Serves one call of this service and counts what it consumed.
///
/// It holds no operation of its own: a request is served through the shared wrapper that
/// names the request each response answers
/// (`service_common::query_error_envelope::RequestIdHeaderOperation`), which resolves the
/// operation the service's own routing table names for it — so the framework still owns
/// the whole deserialize -> validate -> handle -> serialize sequence.
pub struct Metered<Eng> {
    /// The shared engine: the store the datapoint is queued on, and the stores it is
    /// published into, are the ones every service of this emulator shares.
    engine: Arc<Eng>,
    /// Serves the call, and names the request its response answers.
    served: ::service_common::query_error_envelope::RequestIdHeaderOperation,
}

impl<Eng> Metered<Eng> {
    /// Meter every call this service serves, publishing through `engine`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            engine,
            served: ::service_common::query_error_envelope::RequestIdHeaderOperation,
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for Metered<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let answered = self.served.run(svc, req, ctx).await;
            // WHICH API was called: the routing key this protocol resolves, which is a
            // caller's header until it has been RESOLVED against this service's own
            // catalog of the operations it serves
            // ([`super::identity_authorization::served_operation`]). That resolution is
            // what bounds the series: the name a call is counted under is one of this
            // service's own modelled operation names, so the values this dimension can
            // take are the model's and never grow with the requests made. A key that
            // names no operation of this service \u2014 one the routing table refused \u2014
            // reached no API of it and is metered in none.
            if let Some(api_name) = ::aws_smithy_schema_server::rpc::routing_key_from_x_amz_target(
                req,
            )
            .ok()
            .and_then(super::identity_authorization::served_operation)
            {
                crate::state::observability::meter_api_call(&*self.engine, api_name);
            }
            answered
        })
    }

    /// `None` is "defer to the modelled operation the routing key names": metering must
    /// not change which schemes sign an operation.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}
