// SPDX-License-Identifier: CC0-1.0

//! @op AdminListUserAuthEvents
//!
//! Reports the threat-protection event history of one user of one pool: the events
//! the pool LOGGED about what was attempted against it
//! ([`crate::state::auth_events`]), most recent first, in pages of `MaxResults`.
//!
//! A pool whose add-ons are switched off logs no event history and has none to
//! report, which is the state `UserPoolAddOnNotEnabledException` names.
//!
//! ---
//!
//! # Modeled contract for `AdminListUserAuthEvents`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/smithy-model/com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json")
//! ```
//!
//! Requests a history of user activity and any risks detected as part of Amazon Cognito
//! threat protection. For more information, see Viewing user event history. Amazon Cognito
//! evaluates Identity and Access Management (IAM) policies in requests for this API
//! operation. For this operation, you must use IAM credentials to authorize requests, and
//! you must grant yourself the corresponding IAM permission in a policy. Learn more Signing
//! Amazon Web Services API Requests Using the Amazon Cognito user pools API and user pool
//! endpoints
//!
//! ## Input
//!
//! * `MaxResults` (QueryLimitType, range 0..60) — The maximum number of authentication events
//!   to return. Returns 60 events if you set MaxResults to 0, or if you don't include a
//!   MaxResults parameter.
//! * `NextToken` (PaginationKey, len 1..131072, pattern ^[\S]+$) — This API operation returns
//!   a limited number of results. The pagination token is an identifier that you can present
//!   in an additional API request with the same parameters. When you include the pagination
//!   token, Amazon Cognito returns the next set of items after the current list. Subsequent
//!   requests return a new pagination token. By use of this token, you can paginate through
//!   the full list of items.
//! * `UserPoolId` (UserPoolIdType, len 1..55, pattern ^[\w-]+_[0-9a-zA-Z]+$) **required** —
//!   The Id of the user pool that contains the user profile with the logged events.
//! * `Username` (UsernameType, len 1..128, pattern ^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$)
//!   **required** — The name of the user that you want to query or modify. The value of this
//!   parameter is typically your user's username, but it can be any of their alias
//!   attributes. If username isn't an alias attribute in your user pool, this value must be
//!   the sub of a local user or the username of a user from a third-party IdP.
//!
//! ## Output
//!
//! * `AuthEvents` (AuthEventsType) — The response object. It includes the EventID, EventType,
//!   CreationDate, EventRisk, and EventResponse.
//! * `NextToken` (PaginationKey, len 1..131072, pattern ^[\S]+$) — The identifier that Amazon
//!   Cognito returned with the previous request to this operation. When you include a
//!   pagination token in your request, Amazon Cognito returns the next set of items in the
//!   list. By use of this token, you can paginate through the full list of items.
//!
//! ## Errors
//!
//! * `InternalErrorException` (HTTP 500) — This exception is thrown when Amazon Cognito
//!   encounters an internal error.
//! * `InvalidParameterException` (HTTP 400) — This exception is thrown when the Amazon
//!   Cognito service encounters an invalid parameter.
//! * `NotAuthorizedException` (HTTP 403) — This exception is thrown when a user isn't
//!   authorized.
//! * `OperationNotEnabledException` (HTTP 400) — This exception is thrown when an operation
//!   is not available in the current region or for the current user pool configuration. This
//!   can occur when attempting to perform operations that are not supported in secondary
//!   replica regions.
//! * `ResourceNotFoundException` (HTTP 404) — This exception is thrown when the Amazon
//!   Cognito service can't find the requested resource.
//! * `TooManyRequestsException` (HTTP 429) — This exception is thrown when the user has made
//!   too many requests for a given operation.
//! * `UserNotFoundException` (HTTP 404) — This exception is thrown when a user isn't found.
//! * `UserPoolAddOnNotEnabledException` (HTTP 400) — This exception is thrown when user pool
//!   add-ons aren't enabled.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AdminListUserAuthEventsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AdminListUserAuthEventsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cognitoidentityprovider#AdminListUserAuthEvents")]
#[derive(Debug)]
pub enum AdminListUserAuthEventsErrorModeled {
    InternalErrorException(InternalErrorException),
    InvalidParameterException(InvalidParameterException),
    NotAuthorizedException(NotAuthorizedException),
    OperationNotEnabledException(OperationNotEnabledException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
    UserNotFoundException(UserNotFoundException),
    UserPoolAddOnNotEnabledException(UserPoolAddOnNotEnabledException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::admin_list_user_auth_events::AdminListUserAuthEventsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AdminListUserAuthEventsError = crate::scaffolded::Scaffolded<AdminListUserAuthEventsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AdminListUserAuthEventsErrorModeled> for AdminListUserAuthEventsError {
    fn from(modeled: AdminListUserAuthEventsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AdminListUserAuthEvents` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AdminListUserAuthEvents<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AdminListUserAuthEvents<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AdminListUserAuthEvents<Eng>
{
    type Input = AdminListUserAuthEventsInput;
    type Output = AdminListUserAuthEventsOutput;
    type Error = AdminListUserAuthEventsError;

    const NAME: &'static str = "AdminListUserAuthEvents";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ADMIN_LIST_USER_AUTH_EVENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ADMIN_LIST_USER_AUTH_EVENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_error());
        };

        // A read: this handler takes the lock to READ and writes nothing, so a later
        // caller sees exactly the history this one saw.
        let guard = state.lock();
        let pool = guard
            .pool(&input.user_pool_id)
            .ok_or_else(|| user_pool_not_found(&input.user_pool_id))?;
        // A pool with its add-ons off runs no threat protection, so it logged no event
        // history to report at all — which is a property of the POOL and is answered
        // before the user it was asked about is looked up.
        if !pool.add_ons_enabled() {
            return Err(add_ons_not_enabled());
        }
        let user = pool.user(&input.username).ok_or_else(user_not_found)?;

        // The listing a page token belongs to: this operation, over this pool's event
        // history of this user. The user's `sub` identifies the user however the
        // request named it, so a token stays valid across an alias and the `sub`, and
        // names no page of another user's history.
        let scope = format!("{}|{}|{}", Self::NAME, pool.id, user.sub);
        // "Requests a history of user activity" (this operation's own doc): the events
        // the pool logged, newest first, rendered from what is STORED.
        let events = user.auth_events_newest_first();
        let page = crate::state::auth_events::page_of_history(
            &scope,
            &events,
            input.max_results,
            input.next_token.as_deref(),
        )
        .ok_or_else(invalid_pagination_token)?;

        // A listing's result member is PRESENT and empty for a user nothing has been
        // attempted against yet, which is what a paginating client reads.
        Ok(AdminListUserAuthEventsOutput {
            auth_events: Some(page.events),
            next_token: page.next_token,
        })
    }
}

/// This operation's `ResourceNotFoundException` for an absent user pool.
fn user_pool_not_found(user_pool_id: &str) -> AdminListUserAuthEventsError {
    AdminListUserAuthEventsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(format!("User pool {user_pool_id} does not exist.")),
    })
    .into()
}

/// This operation's `UserNotFoundException` for a `Username` that names no user of
/// the pool ([`crate::state::USER_NOT_FOUND_MESSAGE`]).
fn user_not_found() -> AdminListUserAuthEventsError {
    AdminListUserAuthEventsErrorModeled::UserNotFoundException(UserNotFoundException {
        message: Some(crate::state::USER_NOT_FOUND_MESSAGE.to_string()),
    })
    .into()
}

/// The refusal this operation owes on a pool that activated no threat protection
/// ([`crate::state::USER_POOL_ADD_ONS_NOT_ENABLED_MESSAGE`]).
fn add_ons_not_enabled() -> AdminListUserAuthEventsError {
    AdminListUserAuthEventsErrorModeled::UserPoolAddOnNotEnabledException(
        UserPoolAddOnNotEnabledException {
            message: Some(crate::state::USER_POOL_ADD_ONS_NOT_ENABLED_MESSAGE.to_string()),
        },
    )
    .into()
}

/// This operation's `InvalidParameterException` for a `NextToken` that names no
/// page of this listing ([`crate::state::PAGINATION_TOKEN_INVALID_MESSAGE`]).
fn invalid_pagination_token() -> AdminListUserAuthEventsError {
    AdminListUserAuthEventsErrorModeled::InvalidParameterException(InvalidParameterException {
        message: Some(crate::state::PAGINATION_TOKEN_INVALID_MESSAGE.to_string()),
        // `reasonCode` is AWS's own classification of the bad parameter and this
        // service's model documents no value for it, so nothing is invented here.
        reason_code: None,
    })
    .into()
}

/// The modelled server error for an engine this service cannot reach.
fn internal_error() -> AdminListUserAuthEventsError {
    AdminListUserAuthEventsErrorModeled::InternalErrorException(InternalErrorException {
        message: Some("the cognito-idp emulator could not reach its state store".to_string()),
    })
    .into()
}
