// SPDX-License-Identifier: CC0-1.0

//! @op IsAuthorizedWithToken
//!
//! `@readonly`: the principal comes from the TOKEN, not from the request. The token is
//! read and verified by [`crate::state::token`] — it is a compact JWS, its issuer is one
//! of the policy store's identity sources, that issuer signed it wherever the key set that
//! issuer publishes is readable in this process, it is the kind of token the member it
//! arrived in takes, it was issued to a client the source accepts and the
//! clock has not run out on it — and the principal it names, that principal's groups and
//! its attributes all come from its claims. Those entities join the request's own
//! `entities` slice, which carries only resource and action entities for that reason: an
//! entity whose type the store knows as a principal type is refused, and an action entity
//! may carry only its identifier and its entity type
//! ([`crate::state::cedar::check_token_request_entities`]). The decision is then whatever the real Cedar authorizer computed
//! over the store's persisted policies ([`crate::state::cedar::authorize`]), exactly as
//! [`crate::ops::is_authorized`] computes it for a principal the request names. It writes
//! nothing, and verifying a token neither consumes nor records it.
//!
//! A token the service cannot verify is refused with the modelled `ValidationException`,
//! worded as the sentence AWS answers for it — `Failed to verify <member>: <reason>` —
//! rather than being answered with a `DENY`, which would read as "evaluated and denied".
//! A token whose BODY is not a claim set at all never reached that verification, and is
//! refused as the stage that did fail — `Failed to decode <member>: <reason>`, carrying
//! what the claim-set deserialization reported.
//! A token whose AUDIENCE is none the identity source accepts is that same
//! `ValidationException` worded as the one fixed sentence AWS answers for it
//! ([`AUDIENCE_NOT_ACCEPTED`]), which reports neither the claim the audience was read from
//! nor the value it held.
//! A token whose ISSUER is no identity source of the store is the modelled
//! `ResourceNotFoundException` instead: the call references an identity source the store
//! does not hold. A signature of an issuer this emulator HOSTS that it cannot CHECK — a
//! key set it cannot read in this process, a key it cannot check a signature under — is
//! the modelled `InternalServerException`, because nothing was decided about the token.
//! A provider an `openIdConnectConfiguration` names is not that case: it publishes its key
//! set outside the emulator, so there is no signature to check here and the decision
//! follows the rules the token's own value settles. A failure
//! raised while EVALUATING is reported in-band on the 200, in `errors`, for the reason
//! [`crate::ops::is_authorized`] states. The policy store the request names is resolved
//! BEFORE any of the request's own payload — the presented tokens included — is read, so
//! a call against a store that does not exist is the 404 whatever else the request got
//! wrong.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `IsAuthorizedWithToken`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Makes an authorization decision about a service request described in the parameters. The
//! principal in this request comes from an external identity source in the form of an
//! identity token formatted as a JSON web token (JWT). The information in the parameters
//! can also define additional context that Verified Permissions can include in the
//! evaluation. The request is evaluated against all matching policies in the specified
//! policy store. The result of the decision is either Allow or Deny, along with a list of
//! the policies that resulted in the decision. Verified Permissions validates each token
//! that is specified in a request by checking its expiration date and its signature. Tokens
//! from an identity source user continue to be usable until they expire. Token revocation
//! and resource deletion have no effect on the validity of a token in your policy store
//!
//! ## Input
//!
//! * `accessToken` (Token, len 1..131072, pattern
//!   ^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$) — Specifies an access token for the
//!   principal to be authorized. This token is provided to you by the identity provider (IdP)
//!   associated with the specified identity source. You must specify either an accessToken,
//!   an identityToken, or both. Must be an access token. Verified Permissions returns an
//!   error if the token_use claim in the submitted token isn't access.
//! * `action` (ActionIdentifier) — Specifies the requested action to be authorized. Is the
//!   specified principal authorized to perform this action on the specified resource.
//! * `context` (ContextDefinition) — Specifies additional context that can be used to make
//!   more granular authorization decisions.
//! * `entities` (EntitiesDefinition) — (Optional) Specifies the list of resources and their
//!   associated attributes that Verified Permissions can examine when evaluating the
//!   policies. These additional entities and their attributes can be referenced and checked
//!   by conditional elements in the policies in the specified policy store. You can't include
//!   principals in this parameter, only resource and action entities. This parameter can't
//!   include any entities of a type that matches the user or group entity types that you
//!   defined in your identity source. The IsAuthorizedWithToken operation takes principal
//!   attributes from only the identityToken or accessToken passed to the operation. For
//!   action entities, you can include only their …
//! * `identityToken` (Token, len 1..131072, pattern
//!   ^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$) — Specifies an identity token for the
//!   principal to be authorized. This token is provided to you by the identity provider (IdP)
//!   associated with the specified identity source. You must specify either an accessToken,
//!   an identityToken, or both. Must be an ID token. Verified Permissions returns an error if
//!   the token_use claim in the submitted token isn't id.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store. Policies in this policy store will be used to make
//!   an authorization decision for the input. To specify a policy store, use its ID or alias
//!   name. When using an alias name, prefix it with policy-store-alias/. For example: ID:
//!   PSEXAMPLEabcdefg111111 Alias name: policy-store-alias/example-policy-store To view
//!   aliases, use ListPolicyStoreAliases.
//! * `resource` (EntityIdentifier) — Specifies the resource for which the authorization
//!   decision is made. For example, is the principal allowed to perform the action on the
//!   resource?
//!
//! ## Output
//!
//! * `decision` (Decision, one of: ALLOW | DENY) **required** — An authorization decision
//!   that indicates if the authorization request should be allowed or denied.
//! * `determiningPolicies` (DeterminingPolicyList) **required** — The list of determining
//!   policies used to make the authorization decision. For example, if there are multiple
//!   matching policies, where at least one is a forbid policy, then because forbid always
//!   overrides permit the forbid policies are the determining policies. If all matching
//!   policies are permit policies, then those policies are the determining policies. When no
//!   policies match and the response is the default DENY, there are no determining policies.
//! * `errors` (EvaluationErrorList) **required** — Errors that occurred while making an
//!   authorization decision. For example, a policy references an entity or entity attribute
//!   that does not exist in the slice.
//! * `principal` (EntityIdentifier) — The identifier of the principal in the ID or access
//!   token.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `IsAuthorizedWithTokenError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `IsAuthorizedWithTokenError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#IsAuthorizedWithToken",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum IsAuthorizedWithTokenErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::is_authorized_with_token::IsAuthorizedWithTokenError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type IsAuthorizedWithTokenError = crate::scaffolded::Scaffolded<IsAuthorizedWithTokenErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<IsAuthorizedWithTokenErrorModeled> for IsAuthorizedWithTokenError {
    fn from(modeled: IsAuthorizedWithTokenErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `IsAuthorizedWithToken` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct IsAuthorizedWithToken<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> IsAuthorizedWithToken<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for IsAuthorizedWithToken<Eng>
{
    type Input = IsAuthorizedWithTokenInput;
    type Output = IsAuthorizedWithTokenOutput;
    type Error = IsAuthorizedWithTokenError;

    const NAME: &'static str = "IsAuthorizedWithToken";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::IS_AUTHORIZED_WITH_TOKEN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::IS_AUTHORIZED_WITH_TOKEN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let not_found = || {
            IsAuthorizedWithTokenErrorModeled::ResourceNotFoundException(
                // An authorization call's not-found NAMES the resource: see
                // `crate::state::resource_not_found_naming`.
                crate::state::policy_store_reference(&input.policy_store_id)
                    .not_found_naming_the_resource(),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            IsAuthorizedWithTokenErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        // The store the request names is resolved BEFORE any of the request's own payload
        // is read, so a reference to a store that does not exist answers
        // `ResourceNotFoundException` rather than a refusal about the payload — the same
        // precedence [`crate::ops::is_authorized`] and `BatchIsAuthorized` follow.
        let store_id = state
            .resolve_store_id(
                &crate::state::request_region(ctx.parts()),
                &input.policy_store_id,
            )
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        // "You must specify either an accessToken, an identityToken, or both": the model
        // declares both members optional, so the boundary validator cannot require one of
        // them and this is the place that does. Both members are named, because either
        // one satisfies the rule.
        let presented: Vec<crate::state::token::PresentedToken<'_>> = [
            (
                crate::state::token::TokenKind::Access,
                input.access_token.as_deref(),
            ),
            (
                crate::state::token::TokenKind::Identity,
                input.identity_token.as_deref(),
            ),
        ]
        .into_iter()
        .filter_map(|(kind, value)| {
            value.map(|value| crate::state::token::PresentedToken { kind, value })
        })
        .collect();
        if presented.is_empty() {
            return Err(invalid_members([
                crate::ops::InvalidInput::new(crate::ops::paths::ACCESS_TOKEN, TOKEN_REQUIRED),
                crate::ops::InvalidInput::new(crate::ops::paths::IDENTITY_TOKEN, TOKEN_REQUIRED),
            ]));
        }

        // The principal is the token's, so only the other two members of the Cedar triple
        // are read off the request — and each of them must be present for the same reason
        // `IsAuthorized` requires all three.
        let required = |path: &str| {
            invalid(crate::ops::InvalidInput::new(
                path,
                REQUIRED_FOR_EVALUATION,
            ))
        };
        let resource = crate::state::cedar::request_entity(input.resource.as_ref())
            .ok_or_else(|| required(crate::ops::paths::RESOURCE))?;
        let action = crate::state::cedar::request_action(input.action.as_ref())
            .ok_or_else(|| required(crate::ops::paths::ACTION))?;

        // The context travels with the member it came from, so a refusal the evaluation
        // raises about it names the union arm the client sent.
        let context = crate::state::cedar::RequestContext {
            json: crate::state::cedar::context_to_cedar_json(input.context.as_ref())
                .map_err(invalid)?,
            path: crate::state::cedar::context_path(input.context.as_ref()),
        };
        let mut entities = crate::state::cedar::entities_to_cedar_json(
            input.entities.as_ref(),
            crate::state::cedar::MAX_ENTITIES_PER_REQUEST,
        )
        .map_err(invalid)?;

        // Each presented token is verified against the store's identity sources, and a
        // call that presented two is decided for the one principal they agree on.
        let now = ::std::time::SystemTime::now();
        let mut identity: Option<crate::state::token::TokenIdentity> = None;
        for token in presented {
            let verified = crate::state::token::verify(&*self.engine, store, token, now)
                .map_err(|refusal| refused(&input.policy_store_id, refusal))?;
            identity = Some(match identity {
                None => verified,
                Some(first) => first.merged_with(verified).ok_or_else(|| {
                    invalid_members([
                        crate::ops::InvalidInput::new(
                            crate::ops::paths::ACCESS_TOKEN,
                            DIFFERENT_PRINCIPALS,
                        ),
                        crate::ops::InvalidInput::new(
                            crate::ops::paths::IDENTITY_TOKEN,
                            DIFFERENT_PRINCIPALS,
                        ),
                    ])
                })?,
            });
        }
        let identity = identity.ok_or_else(|| {
            IsAuthorizedWithTokenErrorModeled::InternalServerException(InternalServerException {
                message: "no presented token was read".to_string(),
            })
        })?;

        // The `entities` slice of a call whose principal comes from the TOKEN carries only
        // resource and action entities: a principal entity is this call's to build out of
        // the token, and an action entity is named rather than described.
        let Some(supplied) = entities.as_array_mut() else {
            // Only the Cedar-JSON arm can carry something that is no entity LIST, and the
            // principal this call built belongs IN that list: refused here, where that is
            // assumed, rather than left to the evaluation to discover.
            return Err(invalid(crate::ops::InvalidInput::new(
                crate::ops::paths::ENTITIES_CEDAR_JSON,
                "it is not a Cedar entity store: an entity store is a list of entities",
            )));
        };
        // The principal types this store knows: the ones the identity source that issued
        // the token generates — including the group type it maps, which this call builds
        // entities of too — and the ones its schema declares.
        let mut principal_entity_types: Vec<crate::state::cedar::PrincipalEntityType> = identity
            .owned_entity_types
            .iter()
            .map(crate::state::cedar::PrincipalEntityType::from_identity_source)
            .collect();
        principal_entity_types.extend(
            crate::state::cedar::schema_principal_entity_types(
                store.schema.as_ref().map(|schema| schema.cedar_json.as_str()),
            )
            .map_err(refused_by_cedar)?,
        );
        crate::state::cedar::check_token_request_entities(supplied, &principal_entity_types)
            .map_err(invalid)?;
        supplied.extend(identity.entities());

        let verdict = crate::state::cedar::authorize(
            store,
            &identity.principal,
            &action,
            &resource,
            context,
            entities,
        )
        .map_err(refused_by_cedar)?;

        Ok(IsAuthorizedWithTokenOutput {
            decision: verdict.decision,
            determining_policies: verdict
                .determining_policies
                .into_iter()
                .map(|policy_id| DeterminingPolicyItem { policy_id })
                .collect(),
            // Required and always present: an empty list is what a clean evaluation
            // reports.
            errors: verdict
                .errors
                .into_iter()
                .map(|error_description| EvaluationErrorItem { error_description })
                .collect(),
            // "The identifier of the principal in the ID or access token" — the entity the
            // decision was made for, which is the token's and not the request's.
            principal: Some(EntityIdentifier {
                entity_type: identity.principal.entity_type,
                entity_id: identity.principal.entity_id,
            }),
        })
    }
}

/// Why an absent member of the authorization triple is refused: the two members this
/// operation reads off the request are modelled optional, so the boundary validator
/// cannot require them and this is the place that does.
const REQUIRED_FOR_EVALUATION: &str = "it is required to evaluate an authorization request";

/// Why a call presenting no token at all is refused.
const TOKEN_REQUIRED: &str =
    "an authorization request takes its principal from a token, so either an access token \
     or an identity token is required";

/// Why a call presenting two tokens of two different users is refused: one call decides
/// for one principal.
const DIFFERENT_PRINCIPALS: &str =
    "the access token and the identity token name different principals";

/// The refusal a presented token whose audience the identity source does not accept is
/// answered with: ONE fixed sentence, whichever member the token arrived in and whichever
/// of its configured audiences the source lists them as, so a client can tell an audience
/// failure apart from every other validation failure by matching it. It has no variable
/// part — in particular the rejected token's own audience is not reported back.
const AUDIENCE_NOT_ACCEPTED: &str = "Token audience claim contains no valid audience";

/// One offending request member as this operation's modelled `ValidationException`.
fn invalid(invalid: crate::ops::InvalidInput) -> IsAuthorizedWithTokenError {
    invalid_members([invalid])
}

/// Several offending request members as one modelled `ValidationException`, for a rule
/// that is about the members TOGETHER: each one is reported in `fieldList`, because any
/// of them is where a client fixes the request.
fn invalid_members(
    invalid: impl IntoIterator<Item = crate::ops::InvalidInput>,
) -> IsAuthorizedWithTokenError {
    IsAuthorizedWithTokenErrorModeled::ValidationException(crate::ops::validation_exception(
        invalid,
    ))
    .into()
}

/// A refusal [`crate::state::cedar`] raised, on this operation's error channel.
///
/// The same split `IsAuthorized` makes: a member the caller can fix is the modelled
/// `ValidationException`, and service-side state it cannot — a stored schema that will
/// not parse, a store holding more Cedar than one request evaluates — is a server fault.
/// An authorization request carries no policy content, so reasons reaching the schema arm
/// are about content the store already holds, which is service-side state too.
fn refused_by_cedar(refusal: crate::state::cedar::Refusal) -> IsAuthorizedWithTokenError {
    match refusal {
        crate::state::cedar::Refusal::InvalidInput(field) => invalid(field),
        crate::state::cedar::Refusal::ServiceFault(message) => {
            IsAuthorizedWithTokenErrorModeled::InternalServerException(InternalServerException {
                message,
            })
            .into()
        }
        crate::state::cedar::Refusal::SchemaValidation(reasons) => {
            IsAuthorizedWithTokenErrorModeled::InternalServerException(InternalServerException {
                message: reasons.join("; "),
            })
            .into()
        }
    }
}

/// The modelled error a token this call could not use is refused with.
///
/// A token that does not verify is refused as a BAD REQUEST, and the service words that
/// refusal as a sentence naming the member and what failed — `Failed to verify
/// IdentityToken: signature is invalid` — so the message is that sentence rather than this
/// service's fixed `Invalid input`, while `fieldList` still names the member for a client
/// that reads it. An audience the identity source does not accept is the exception the
/// service words as fixed text instead ([`AUDIENCE_NOT_ACCEPTED`]).
///
/// A token whose BODY did not DECODE into a claim set is the same bad request worded for
/// the stage it failed at — `Failed to decode IdentityToken: <what the deserialization
/// reported>` — because nothing about such a token was verified: a client reading the
/// message is told to repair the token it presented, where the verify sentence would send
/// it to the trust its policy store configures.
///
/// A token whose issuer is no identity source of the store is the other kind of answer:
/// nothing about the token is wrong, and what the call referenced — an identity source for
/// that issuer IN this policy store — does not exist, so it is the modelled not-found,
/// worded as the reference it was: the ISSUER the token claims and the policy store the
/// request addressed, which `policy_store_id` carries. The exception's own `resourceId` /
/// `resourceType` members still name the missing identity source.
///
/// A token whose signature could not be CHECKED is the third: nothing about the request is
/// wrong and this service cannot say whether the signature is good, which is the modelled
/// server fault rather than a refusal of the token.
fn refused(
    policy_store_id: &str,
    refusal: crate::state::token::TokenRefusal,
) -> IsAuthorizedWithTokenError {
    match refusal {
        crate::state::token::TokenRefusal::NotDecoded { kind, reason } => {
            IsAuthorizedWithTokenErrorModeled::ValidationException(ValidationException {
                message: format!("Failed to decode {}: {reason}", kind.reported_name()),
                field_list: Some(vec![ValidationExceptionField {
                    path: kind.path().to_string(),
                    message: reason,
                }]),
            })
            .into()
        }
        crate::state::token::TokenRefusal::NotVerified { kind, reason } => {
            IsAuthorizedWithTokenErrorModeled::ValidationException(ValidationException {
                message: format!("Failed to verify {}: {reason}", kind.reported_name()),
                field_list: Some(vec![ValidationExceptionField {
                    path: kind.path().to_string(),
                    message: reason,
                }]),
            })
            .into()
        }
        // The audience rule has its own fixed sentence, so this refusal is NOT composed
        // from a reason about the token: the message is that sentence alone, and
        // `fieldList` names the member the token arrived in without reporting anything the
        // token claimed.
        crate::state::token::TokenRefusal::AudienceNotAccepted { kind } => {
            IsAuthorizedWithTokenErrorModeled::ValidationException(ValidationException {
                message: AUDIENCE_NOT_ACCEPTED.to_string(),
                field_list: Some(vec![ValidationExceptionField {
                    path: kind.path().to_string(),
                    message: AUDIENCE_NOT_ACCEPTED.to_string(),
                }]),
            })
            .into()
        }
        crate::state::token::TokenRefusal::NoIdentitySource(issuer) => {
            IsAuthorizedWithTokenErrorModeled::ResourceNotFoundException(
                crate::ops::identity_source_not_found_for_issuer(&issuer, policy_store_id),
            )
            .into()
        }
        crate::state::token::TokenRefusal::NotCheckable { issuer, reason } => {
            IsAuthorizedWithTokenErrorModeled::InternalServerException(InternalServerException {
                message: format!(
                    "the signature of the token the issuer {issuer} minted could not be \
                     checked: {reason}"
                ),
            })
            .into()
        }
    }
}
