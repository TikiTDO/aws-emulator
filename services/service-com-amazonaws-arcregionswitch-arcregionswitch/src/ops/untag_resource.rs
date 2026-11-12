// SPDX-License-Identifier: CC0-1.0

//! @op UntagResource
//!
//! Removes the named keys from the tags persisted against the plan the ARN names. A key
//! the plan does not carry is simply not there afterwards — removal is idempotent, so
//! repeating the call is not an error — and a key the request does not name is left
//! alone. An ARN no plan is stored under answers the modelled
//! `ResourceNotFoundException` (HTTP 404).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read and write it through the
//! guard it hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries
//! request metadata only; the engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UntagResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Removes tags from a Region switch resource.
//!
//! ## Input
//!
//! * `arn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) for a tag you remove a resource from.
//! * `resourceTagKeys` (TagKeys, len 0..200) **required** — Tag keys that you remove from a
//!   resource.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception, or failure. HTTP Status Code: 500
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource was not found. HTTP
//!   Status Code: 404

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UntagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UntagResourceError` alias below. The boundary validator already
// refuses a `resourceTagKeys` entry outside `TagKey`'s `@length`, so the only failures
// this handler raises are the two the operation itself declares and nothing has to be
// widened in through `extra_errors`.
#[smithy_macros::operation_error("com.amazonaws.arcregionswitch#UntagResource")]
#[derive(Debug)]
pub enum UntagResourceErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::untag_resource::UntagResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UntagResourceError = crate::scaffolded::Scaffolded<UntagResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UntagResourceErrorModeled> for UntagResourceError {
    fn from(modeled: UntagResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UntagResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UntagResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UntagResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UntagResource<Eng>
{
    type Input = UntagResourceInput;
    type Output = UntagResourceOutput;
    type Error = UntagResourceError;

    const NAME: &'static str = "UntagResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UNTAG_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UNTAG_RESOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(UntagResourceErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Region switch plan store is unavailable".to_string(),
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.settle(now);
        let Some(record) = store.plans.get_mut(&input.arn) else {
            return Err(UntagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::plan_not_found_message(&input.arn),
                },
            )
            .into());
        };

        // Removal is idempotent: a key the plan does not carry needs nothing done to it,
        // so naming it is not a refusal.
        for key in &input.resource_tag_keys {
            record.tags.remove(key);
        }
        Ok(UntagResourceOutput {})
    }
}
