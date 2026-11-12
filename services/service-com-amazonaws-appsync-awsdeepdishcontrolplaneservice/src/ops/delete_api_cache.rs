// SPDX-License-Identifier: CC0-1.0

//! @op DeleteApiCache
//!
//! Deletes the API's cache instance: once the instance is gone the API holds no cache
//! at all, so every later reader of it answers not-found and a fresh `CreateApiCache`
//! is accepted again. The response carries no members.
//!
//! The instance is only torn down from the one status in which it has finished being
//! provisioned ([`crate::state::CACHE_AVAILABLE`]): while it is in any other status the
//! delete is refused with the operation's modelled `BadRequestException` (400) and the
//! cache is left exactly where it was, so a caller can read it again and retry.
//!
//! A delete addresses the cache through the API that owns it, so an `apiId` that is not
//! an API id is a bad request (400) and one that names no API of this account is a
//! not-found (404). The cache itself carries no identifier of its own — an API holds at
//! most one — so the teardown asks for the absence of the instance rather than for the
//! instance: on an API that holds no cache there is nothing left to tear down and the
//! call answers the modelled empty response (200), as it does for the delete that
//! emptied the slot. The absent-cache not-found belongs to the readers and mutators of
//! the cache, never to its deleter.
//!
//! What a later reader of the absent cache reports is what the LAST teardown removed
//! ([`crate::state::GraphqlApiRecord::take_api_cache`]): a delete that removed an
//! instance leaves a cache that was torn down, and a delete that found nothing leaves an
//! API that holds no cache at all.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DeleteApiCache::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteApiCache`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json")
//! ```
//!
//! Deletes an ApiCache object.
//!
//! ## Input
//!
//! * `apiId` (String) **required** — The API ID.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The request is not well formed. For example, a value
//!   is invalid or a required field is missing. Check the field values, and then try again.
//! * `ConcurrentModificationException` (HTTP 409) — Another modification is in progress at
//!   this time and it must complete before you can make your change.
//! * `InternalFailureException` (HTTP 500) — An internal AppSync error occurred. Try your
//!   request again.
//! * `NotFoundException` (HTTP 404) — The resource specified in the request was not found.
//!   Check the resource, and then try again.
//! * `UnauthorizedException` (HTTP 401) — You aren't authorized to perform this operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteApiCacheError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteApiCacheError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appsync#DeleteApiCache")]
#[derive(Debug)]
pub enum DeleteApiCacheErrorModeled {
    BadRequestException(BadRequestException),
    ConcurrentModificationException(ConcurrentModificationException),
    InternalFailureException(InternalFailureException),
    NotFoundException(NotFoundException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_api_cache::DeleteApiCacheError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteApiCacheError = crate::scaffolded::Scaffolded<DeleteApiCacheErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteApiCacheErrorModeled> for DeleteApiCacheError {
    fn from(modeled: DeleteApiCacheErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteApiCache` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteApiCache<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteApiCache<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteApiCache<Eng>
{
    type Input = DeleteApiCacheInput;
    type Output = DeleteApiCacheOutput;
    type Error = DeleteApiCacheError;

    const NAME: &'static str = "DeleteApiCache";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_API_CACHE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_API_CACHE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Which cache is deleted comes from the deserialized input and the clock; no
        // request metadata takes part, so `ctx` is unused.
        let _ = ctx;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeleteApiCacheErrorModeled::InternalFailureException(
                InternalFailureException {
                    message: Some("the appsync store is unavailable on this engine".to_string()),
                },
            )
            .into());
        };

        // An `apiId` that is not an API id at all is a malformed request, not a
        // missing resource (see `crate::state::is_api_id`).
        if !crate::state::is_api_id(&input.api_id) {
            return Err(DeleteApiCacheErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::invalid_api_id_message(&input.api_id)),
                ..Default::default()
            })
            .into());
        }

        let now = std::time::SystemTime::now();
        let mut guard = state.lock();
        // Apply what the clock owes before the status is read, so an instance whose
        // provisioning has already come due is deletable and this call re-decides no
        // pending transition.
        guard.settle_api_caches(now);

        let Some(api) = guard.graphql_apis.get_mut(&input.api_id) else {
            return Err(DeleteApiCacheErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::API_NOT_FOUND_MESSAGE.to_string()),
            })
            .into());
        };
        // The teardown precondition, which is about the instance the slot HOLDS: an
        // instance whose CREATION has not completed cannot be torn down — it is still
        // being created, or its creation failed. Once it has been created it is deletable
        // whatever the service is doing to it at the moment, so settings being applied to
        // an existing instance never make it undeletable. The refusal writes nothing, so
        // the cache is still reported afterwards in whatever status it is in.
        if api.api_cache.as_ref().is_some_and(|cache| !cache.was_created()) {
            return Err(DeleteApiCacheErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::CACHE_NOT_DELETABLE_MESSAGE.to_string()),
                ..Default::default()
            })
            .into());
        }

        // Removing the record is what makes the cache gone: `GetApiCache`,
        // `FlushApiCache` and `UpdateApiCache` all answer not-found for this API
        // afterwards, and a `CreateApiCache` is no longer a duplicate. An API that holds
        // no cache is already what the teardown asks for, whether a delete emptied the
        // slot or nothing ever filled it, so the same call is served there too. Either
        // way the teardown records what IT removed, so a later read reports a cache that
        // was torn down only after the delete that tore one down.
        api.take_api_cache();

        // The modelled response carries no members.
        Ok(DeleteApiCacheOutput {})
    }
}
