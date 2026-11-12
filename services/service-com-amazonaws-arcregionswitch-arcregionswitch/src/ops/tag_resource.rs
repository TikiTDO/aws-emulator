// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//!
//! Merges the request's tags into the tags persisted against the plan the ARN names: a
//! key the plan does not carry is added, a key it does carry takes the new value, and a
//! key the request does not mention is left alone. An ARN no plan is stored under answers
//! the modelled `ResourceNotFoundException` (HTTP 404). Nothing else about the plan
//! changes — tagging is not a write to the plan's configuration, so the plan's `version`
//! and `updatedAt` stay as they were.
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
//! # Modeled contract for `TagResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")
//! ```
//!
//! Adds or updates tags for a Region switch resource. You can assign metadata to your
//! resources in the form of tags, which are key-value pairs.
//!
//! ## Input
//!
//! * `arn` (PlanArn, pattern
//!   ^arn:aws[a-zA-Z-]*:arc-region-switch::[0-9]{12}:plan/([a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?):([a-z0-9]{6})$)
//!   **required** — The Amazon Resource Name (ARN) for a tag that you add to a resource.
//! * `tags` (Tags, len 0..200) **required** — Tags that you add to a resource. You can add a
//!   maximum of 50 tags in Region switch.
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
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below.
// `IllegalArgumentException` — "the request processing has an invalid argument" (HTTP
// 400) — is a shape the SERVICE models that this operation does not list, widened in
// through `extra_errors` so a request that would push the plan past the documented
// maximum of `crate::state::MAX_TAGS_PER_PLAN` tags has a modelled client refusal to
// answer with.
#[smithy_macros::operation_error(
    "com.amazonaws.arcregionswitch#TagResource",
    extra_errors = ["IllegalArgumentException"]
)]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    IllegalArgumentException(IllegalArgumentException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::tag_resource::TagResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TagResourceError = crate::scaffolded::Scaffolded<TagResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TagResourceErrorModeled> for TagResourceError {
    fn from(modeled: TagResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TagResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TagResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TagResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TagResource<Eng>
{
    type Input = TagResourceInput;
    type Output = TagResourceOutput;
    type Error = TagResourceError;

    const NAME: &'static str = "TagResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TAG_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TAG_RESOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(TagResourceErrorModeled::InternalServerException(
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
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::plan_not_found_message(&input.arn),
                },
            )
            .into());
        };

        // A map's KEY target is not reached by the boundary validator (its VALUE target
        // is), so `TagKey`'s modelled length is enforced here.
        if let Some(rejection) =
            crate::state::tag_keys_rejection("tags", input.tags.keys().map(String::as_str))
        {
            return Err(TagResourceErrorModeled::IllegalArgumentException(
                IllegalArgumentException { message: rejection },
            )
            .into());
        }

        // What the plan would carry afterwards: a key it already has takes the new value
        // rather than adding to the count, so only the keys it does not have grow it.
        let added = input
            .tags
            .keys()
            .filter(|key| !record.tags.contains_key(*key))
            .count();
        let total = record.tags.len() + added;
        if total > crate::state::MAX_TAGS_PER_PLAN {
            return Err(TagResourceErrorModeled::IllegalArgumentException(
                IllegalArgumentException {
                    message: format!(
                        "tags: a Region switch resource carries a maximum of {} tags; \
                         this request would leave {total} on {}",
                        crate::state::MAX_TAGS_PER_PLAN, input.arn
                    ),
                },
            )
            .into());
        }

        // Merge, never replace: this operation "adds or updates tags", so a key the
        // request does not mention keeps the value it had.
        for (key, value) in input.tags {
            record.tags.insert(key, value);
        }
        Ok(TagResourceOutput {})
    }
}
