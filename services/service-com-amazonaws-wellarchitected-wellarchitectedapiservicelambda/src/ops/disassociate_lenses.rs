// SPDX-License-Identifier: CC0-1.0

//! @op DisassociateLenses
//! Disassociate a lens from a workload.
//!
//! The lenses a workload is reviewed against are held on the WORKLOAD —
//! `Workload$Lenses`, the list `AssociateLenses` adds to, every read of the workload
//! reports and every lens review of it is derived from — so this operation resolves the
//! workload and every lens its `LensAliases` names against the store, and drops the
//! review of each lens named, along with the answers and notes recorded under it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DisassociateLenses`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! Disassociate a lens from a workload. Up to 10 lenses can be disassociated from a
//! workload in a single API operation. The Amazon Web Services Well-Architected Framework
//! lens (wellarchitected) cannot be removed from a workload.
//!
//! ## Input
//!
//! * `LensAliases` (LensAliases, len 1..) **required** — List of lens aliases to associate or
//!   disassociate with a workload. Up to 10 lenses can be specified. Identify a lens using
//!   its LensSummary$LensAlias.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) **required** — The ID
//!   assigned to the workload. This ID is unique within an Amazon Web Services Region.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — The resource has already been processed, was deleted,
//!   or is too large.
//! * `InternalServerException` (HTTP 500) — There is a problem with the Well-Architected Tool
//!   API service.
//! * `ResourceNotFoundException` (HTTP 404) — The requested resource was not found.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DisassociateLensesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

#[derive(Debug, Clone, Default)]
pub struct DisassociateLensesOutput;

impl DisassociateLensesOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "PATCH",
        "/workloads/{WorkloadId}/disassociateLenses",
        Some(200u16),
    ));
}

pub static DISASSOCIATE_LENSES_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DisassociateLensesOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DisassociateLensesOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DisassociateLensesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#DisassociateLenses")]
#[derive(Debug)]
pub enum DisassociateLensesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::disassociate_lenses::DisassociateLensesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DisassociateLensesError = crate::scaffolded::Scaffolded<DisassociateLensesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DisassociateLensesErrorModeled> for DisassociateLensesError {
    fn from(modeled: DisassociateLensesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisassociateLenses` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DisassociateLenses<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisassociateLenses<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisassociateLenses<Eng>
{
    type Input = DisassociateLensesInput;
    type Output = DisassociateLensesOutput;
    type Error = DisassociateLensesError;

    const NAME: &'static str = "DisassociateLenses";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_LENSES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DISASSOCIATE_LENSES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            DisassociateLensesErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool workload store is unavailable".to_string(),
            })
        })?;

        // "Up to 10 lenses can be disassociated from a workload in a single API operation"
        // (this operation's doc), which the member itself does not state, so its LENGTH is
        // checked first — one comparison, before any per-entry work and before the shared
        // store lock is taken — and an oversized request is refused without being walked,
        // exactly as the sibling association refuses one.
        if input.lens_aliases.len() > crate::state::MAX_LENS_ALIASES_PER_REQUEST {
            return Err(DisassociateLensesErrorModeled::ValidationException(
                crate::state::too_many_lens_aliases_exception(input.lens_aliases.len()),
            )
            .into());
        }

        let mut store = state.lock();
        // "Disassociate a lens from a workload": the lenses are a member OF the workload,
        // so the workload has to exist to hold any — resolved here, before any lens the
        // request named, so a caller whose workload is gone is not told its lens is.
        let workload = store.workloads.get_mut(&input.workload_id).ok_or_else(|| {
            DisassociateLensesErrorModeled::ResourceNotFoundException(
                crate::state::workload_not_found(&input.workload_id),
            )
        })?;

        // "The Amazon Web Services Well-Architected Framework lens (wellarchitected) cannot
        // be removed from a workload" (this operation's doc): the service serves no
        // workload that lost it, so a request naming it is refused whole — however the name
        // is spelled, and whether or not this workload is reviewed against it, because
        // neither changes what the request asks for. What "names it" is the crate's one lens
        // identity rule ([`crate::state::lens_alias`]), the same rule the reviews below are
        // resolved by, so no name that would reach the framework lens's review gets past
        // this.
        if let Some(default_lens) = input
            .lens_aliases
            .iter()
            .find(|lens| crate::state::lens_alias(lens) == crate::state::FRAMEWORK_LENS_ALIAS)
        {
            return Err(DisassociateLensesErrorModeled::ValidationException(
                framework_lens_refusal(default_lens),
            )
            .into());
        }

        // Every lens the request names is resolved against the workload's OWN reviews
        // before the first one is dropped, so a request naming one lens the workload
        // reviews and one it does not changes nothing: a half-applied disassociation would
        // leave the caller unable to tell which half landed. A lens review exists exactly
        // while that lens is associated with the workload, so a lens the workload does not
        // review has no review to disassociate, whatever the lens catalog says about the
        // name. The walk stops at the first lens it refuses and costs one scan of the
        // attached lenses per name, over a list the quota above has already bounded. A lens
        // the workload does not review is a value of `LensAliases` this service refuses
        // rather than a resource it could not find
        // (`crate::service::lens_content_validation`).
        if let Some(unreviewed) = input
            .lens_aliases
            .iter()
            .find(|lens| !workload.reviews_lens(lens))
        {
            return Err(DisassociateLensesErrorModeled::ValidationException(
                crate::service::lens_content_validation::no_lens_review(
                    &input.workload_id,
                    unreviewed,
                ),
            )
            .into());
        }

        // `LensAliases` is `@length(min: 1)`, so the boundary validator has already refused
        // a request that names none and at least one review is dropped here. A list naming
        // the same lens twice drops it once, because every entry was resolved above against
        // the reviews as they stood before any removal.
        for lens in &input.lens_aliases {
            workload.disassociate_lens(lens);
        }
        // The lenses are a member of the workload (`Workload$Lenses`), so removing one
        // changes the workload, and `Workload$UpdatedAt` — "the date and time recorded" —
        // is what every read of the workload and of its lens reviews reports that change
        // by. The clock is read here, on the path that records it: every refusal above
        // leaves the workload as it stands, `UpdatedAt` included.
        workload.updated_at = std::time::SystemTime::now();

        Ok(DisassociateLensesOutput)
    }
}

/// The refusal for a request that asks for the Well-Architected Framework lens to be
/// removed: the modelled 400, because this operation models no other error for a request
/// the service will not serve.
///
/// The Framework lens is the lens every workload is reviewed against by default, so the
/// refusal is the one this service states for a default lens
/// ([`crate::state::default_lens_refusal`]) — naming the workload's lens collection, which
/// is what the request asks to change, and the alias the request named it by. A request
/// naming it more than once is one request, so it is refused once, by the first alias that
/// names it.
fn framework_lens_refusal(lens_alias: &str) -> ValidationException {
    crate::state::default_lens_refusal(lens_alias)
}
