// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the constraint layer refused — and the one half of that
//! wording the layer itself cannot state: the value a clause about a COLLECTION is about.
//!
//! A constraint refusal of this service is the framework's envelope over one clause per
//! violation, each stating the value that was rejected, the member path it sat at and the
//! requirement it failed. The value is part of the rule rather than prose: it is the only
//! place the request's own input is reported back, so a client comparing it against what it
//! sent has to see its own bytes.
//!
//! The framework's constraint walk has no rendering for a COLLECTION. `@length` on a list or
//! a map is checked on the member itself, before the walk descends into the elements, and
//! what it stores on the violation is a SUMMARY of the size — `<n> items` for a list,
//! `<n> entries` for a map — which is not a value any caller sent. AWS states the collection
//! there: a list as `[<element>, …]`, a map as `{<key>=<value>, …}`.
//!
//! A constraint the collection's ELEMENTS broke is stated at the collection too: the
//! framework reports one clause per offending element, at
//! `<collection>.<position>.member`, naming only the constraint that element broke, while
//! this service states ONE clause at `<collection>`, quoting the whole list and listing
//! every requirement the element shape declares.
//!
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is handed
//! the violations alone and never the request they were found in, so neither value can be
//! recovered there. [`QuotingSubmittedValues`] is where they are: a per-operation wrapper,
//! which has the request and can re-read it.
//!
//! Nothing is re-implemented and no check is repeated in other words: the violations and the
//! submitted values come from the tree's shared collecting walk
//! (`service_common::validation::refusing_violations_with_mapped_echoes`, which runs the
//! protocol's own deserializer and the framework's own `constraints::validate` and owns
//! every bound on what one clause may quote), the clauses, the envelope and the restoring of
//! a quoted collection from the shared renderings of
//! `service_common::validation_message`, the sentences inside them from the framework's own
//! `Violation::message`, and the status and the envelope from the framework's refusal
//! channel.
//!
//! # The order a quoted collection is stated in
//!
//! A collection a clause embeds is stated in the order the TABLE holding it is walked, not
//! sorted and not in the order the request wrote it: the admissible values of an enumerated
//! type in ascending bucket of each value's string hash
//! (`service_common::validation_message::in_hashed_enum_order`), and a map's entries in
//! ascending bucket of each key's, two keys of one bucket in key order
//! (`service_common::submitted_collections::in_hashed_entry_order`). Both are the one shared
//! derivation of that hash at two table widths, so neither can drift from the other; what
//! this service chooses is that it states those orders at all.
//!
//! Neither order reads anything but the collection itself, so a clause a HANDLER of this
//! service words about a map it has already read states the same order as one restated here
//! (`crate::state::rendered_map`). What this wrapper is needed for is the ENTRIES: the
//! walk's own rendering of a map member is a summary of its size, so the entries are read
//! back from the request
//! (`service_common::submitted_collections::quoting_submitted_maps`, the shared restating
//! every bound on what one clause may quote still belongs to).

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use service_common::query_error_envelope::with_request_id_header;
use service_common::submitted_collections::{in_hashed_entry_order, quoting_submitted_maps};
use service_common::validation::{refusing_violations_with_mapped_echoes, unanchored};
use service_common::validation_message::{
    aws_validation_refusal_over_elements_quoting, aws_validation_refusal_with,
    in_hashed_enum_order, quoting_the_submitted_collection,
};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused, Service, Violation,
};

use super::error_type_header::with_error_type_header;

/// The error code a constraint refusal of this service carries.
///
/// No operation of this service declares it, and that is the point: the refusal is the
/// protocol's, not one of the service's own modelled failures — see
/// `crate::service::EmulatedService::override_refusal`.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The message a constraint refusal carries: how many of the request's values failed, then
/// one clause per failure — each naming the member, the value the request gave it and the
/// requirement that value failed, so the caller can fix the request from the text alone.
///
/// Every part of it is a shared rendering: the envelope and the clauses are
/// `service_common::validation_message`'s (which takes them from the framework's own), a
/// `@pattern` is quoted as the EXPRESSION a value must match rather than as the model's
/// anchored literal (`service_common::validation::unanchored` — the anchors are what makes
/// it a whole-value match, which "must satisfy regular expression pattern" already says), an
/// enumerated type's value set is listed in the order the table of admissible values is
/// walked whatever order the model declares its members in
/// (`service_common::validation_message::in_hashed_enum_order`), and a constraint broken by
/// a list's ELEMENTS is stated once at the list.
///
/// `quoted_collection` answers, for a member path a clause is stated at, the collection that
/// member held — the one value no violation carries. `None` from it states the clause
/// without a value, which is the fail-safe direction.
pub fn validation_error_message(
    violations: &[Violation],
    quoted_collection: impl Fn(&str) -> Option<String>,
    input_schema: &'static ::aws_smithy_schema::Schema,
) -> Option<String> {
    let stated: Vec<Violation> = violations.iter().map(in_hashed_enum_order).collect();
    aws_validation_refusal_over_elements_quoting(
        &Refusal::Invalid(stated),
        &[input_schema],
        unanchored,
        quoted_collection,
    )
    .map(|refused| refused.message)
}

/// The refusal this service states for one the framework raised with no request at hand —
/// [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal)'s seam,
/// which is handed the violations alone.
///
/// The same shared renderings as [`validation_error_message`], minus the one half that needs
/// the request: a clause about a collection states no value here rather than a summary the
/// caller never sent, and a constraint broken by a list's elements keeps the framework's own
/// per-element clause. `None` keeps the framework's whole wording.
pub fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let stated: Vec<Violation> = violations.iter().map(in_hashed_enum_order).collect();
    aws_validation_refusal_with(&Refusal::Invalid(stated), unanchored)
}

/// One modelled operation of this service, served so that a refusal states the value the
/// request carried — and so that every response names the request it answers and the
/// exception it is refused with.
///
/// It makes no routing decision and holds no state of its own: the operation it wraps is the
/// one the generated routing table names for the same key, built from the SAME shared
/// engine, and a request this wrapper does not refuse is handed to it untouched — so the
/// framework still owns the whole deserialize -> validate -> handle -> serialize sequence and
/// every status, shape and message it answers with.
///
/// The cost is DELIBERATE: an accepted request's body is read twice, here and in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation.
pub struct QuotingSubmittedValues<Op> {
    operation: Op,
}

impl<Op> QuotingSubmittedValues<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: Op) -> Self {
        Self { operation }
    }
}

/// `resp` as this service answers it: naming the request it answers and, when it is a
/// failure, the exception shape its body names. Both reports are idempotent, so a response
/// the definition's own renderers already named is left exactly as they wrote it.
fn reported(resp: RtResponse) -> RtResponse {
    with_error_type_header(with_request_id_header(resp))
}

impl<Op> ErasedOperation for QuotingSubmittedValues<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let (refused, submitted_lists) =
                refusing_violations_with_mapped_echoes::<Op>(svc.definition().protocol(), req);
            let (reported_violations, echoes): (Vec<Violation>, Vec<_>) =
                refused.into_iter().unzip();
            // A clause about a MAP member first: the walk's own rendering of such a member
            // is a summary of its SIZE, and the entries behind it are read back from the
            // request in the order the table holding them is walked. A clause about
            // anything else is left for the echo below, which is what carries a LIST.
            let quoted_maps =
                quoting_submitted_maps(reported_violations, req, in_hashed_entry_order);
            let refused: Vec<Violation> = quoted_maps
                .into_iter()
                .zip(echoes)
                .map(|(violation, echo)| quoting_the_submitted_collection(violation, echo))
                .collect();
            if !refused.is_empty() {
                // A clause stated at a collection quotes the list that member held, which
                // the collecting walk recorded under the same member path.
                let message = validation_error_message(
                    &refused,
                    |member| submitted_lists.get(member).cloned(),
                    Op::INPUT_SCHEMA,
                );
                if let Some(message) = message {
                    return reported(svc.definition().refusal_response(
                        aws_smithy_schema_server::ProtocolError::Invalid(refused),
                        Some(Refused {
                            code: VALIDATION_EXCEPTION.to_string(),
                            message,
                        }),
                        req,
                    ));
                }
            }
            reported(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it, so this service's anonymous credential exchange stays
    /// anonymous.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}
