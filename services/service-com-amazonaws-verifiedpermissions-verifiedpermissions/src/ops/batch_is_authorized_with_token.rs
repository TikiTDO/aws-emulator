// SPDX-License-Identifier: CC0-1.0

//! @op BatchIsAuthorizedWithToken
//!
//! `@readonly`: the principal of every request in the batch comes from the presented
//! TOKEN, read and verified by [`crate::state::token`] exactly as
//! [`crate::ops::is_authorized_with_token`] verifies it — a compact JWS whose issuer is an
//! identity source of the policy store, signed by that issuer wherever that issuer's key
//! set is readable in this process, of the kind the member it arrived in takes, issued to a
//! client the source accepts and still inside its validity window — and the principal it
//! names, that principal's groups and its attributes all come from its claims. Each of the
//! batch's requests supplies only the other two members
//! of the Cedar triple (and its own `context`), so the batch is a series of decisions for
//! ONE principal and the "identical principal or resource" rule
//! [`crate::ops::batch_is_authorized`] enforces is satisfied by construction, with nothing
//! to check.
//!
//! The store's Cedar text is parsed ONCE for the whole batch
//! ([`crate::state::cedar::load_policies`]) and each request is evaluated against that one
//! policy set and that one entity slice ([`crate::state::cedar::evaluate`]), answering one
//! result per request IN THE ORDER THEY WERE REQUESTED. Every decision, determining policy
//! and evaluation error is whatever the real Cedar authorizer computed; nothing here is
//! canned, and it writes nothing — verifying a token neither consumes nor records it.
//!
//! The `entities` slice is the BATCH's, built once, and carries only resource and action
//! entities for the same reason the single-request token call's does: the principal is the
//! token's to build, so an entity whose type the store knows as a principal type is
//! refused and an action entity may carry only its identifier and its entity type
//! ([`crate::state::cedar::check_token_request_entities`]). Its capacity is the one this
//! operation documents for its own member
//! ([`crate::state::cedar::MAX_ENTITIES_PER_TOKEN_BATCH`]), and the batch carries at most
//! [`crate::state::cedar::MAX_BATCH_REQUESTS`] requests.
//!
//! A token the service cannot verify is refused with the modelled `ValidationException`,
//! worded as the sentence AWS answers for it — `Failed to verify <member>: <reason>` —
//! rather than every result being answered with a `DENY`, which would read as "evaluated
//! and denied"; a token whose BODY is not a claim set at all never reached that
//! verification and is refused as the stage that did fail — `Failed to decode <member>:
//! <reason>`, carrying what the claim-set deserialization reported. An audience none of the
//! source's clients match is that same exception
//! worded as the one fixed sentence AWS answers for it ([`AUDIENCE_NOT_ACCEPTED`]). A
//! token whose ISSUER is no identity source of the store is the modelled
//! `ResourceNotFoundException`, and a signature of an issuer this emulator HOSTS that it
//! cannot check is the modelled `InternalServerException`, because nothing was decided
//! about the token. A
//! failure raised while EVALUATING one request is reported in that request's own `errors`
//! on the 200, for the reason [`crate::ops::batch_is_authorized`] states. The policy store
//! the request names is resolved BEFORE any of the request's own payload — the presented
//! tokens included — is read, so a call against a store that does not exist is the 404
//! whatever else the request got wrong.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `BatchIsAuthorizedWithToken`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Makes a series of decisions about multiple authorization requests for one token. The
//! principal in this request comes from an external identity source in the form of an
//! identity or access token, formatted as a JSON web token (JWT). The information in the
//! parameters can also define additional context that Verified Permissions can include in
//! the evaluations. The request is evaluated against all policies in the specified policy
//! store that match the entities that you provide in the entities declaration and in the
//! token. The result of the decisions is a series of Allow or Deny responses, along with
//! the IDs of the policies that produced each decision. The entities of a
//! BatchIsAuthorizedWithToken API request can contain up to 100 resources and up to 99 user
//! groups. The requests of a BatchIsAuthorizedWithToken API request can contain up to 30
//! requests. The BatchIsAuthorizedWithToken operation doesn't have its own IAM permission.
//! To authorize this operation for Amazon Web Services principals, include the permission
//! verifiedpermissions:IsAuthorizedWithToken in their IAM policies.
//!
//! ## Input
//!
//! * `accessToken` (Token, len 1..131072, pattern
//!   ^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$) — Specifies an access token for the
//!   principal that you want to authorize in each request. This token is provided to you by
//!   the identity provider (IdP) associated with the specified identity source. You must
//!   specify either an accessToken, an identityToken, or both. Must be an access token.
//!   Verified Permissions returns an error if the token_use claim in the submitted token
//!   isn't access.
//! * `entities` (EntitiesDefinition) — (Optional) Specifies the list of resources and their
//!   associated attributes that Verified Permissions can examine when evaluating the
//!   policies. These additional entities and their attributes can be referenced and checked
//!   by conditional elements in the policies in the specified policy store. You can't include
//!   principals in this parameter, only resource and action entities. This parameter can't
//!   include any entities of a type that matches the user or group entity types that you
//!   defined in your identity source. The BatchIsAuthorizedWithToken operation takes
//!   principal attributes from only the identityToken or accessToken passed to the operation.
//!   For action entities, you can include only their …
//! * `identityToken` (Token, len 1..131072, pattern
//!   ^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$) — Specifies an identity (ID) token
//!   for the principal that you want to authorize in each request. This token is provided to
//!   you by the identity provider (IdP) associated with the specified identity source. You
//!   must specify either an accessToken, an identityToken, or both. Must be an ID token.
//!   Verified Permissions returns an error if the token_use claim in the submitted token
//!   isn't id.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store. Policies in this policy store will be used to make
//!   an authorization decision for the input. To specify a policy store, use its ID or alias
//!   name. When using an alias name, prefix it with policy-store-alias/. For example: ID:
//!   PSEXAMPLEabcdefg111111 Alias name: policy-store-alias/example-policy-store To view
//!   aliases, use ListPolicyStoreAliases.
//! * `requests` (BatchIsAuthorizedWithTokenInputList, len 1..) **required** — An array of up
//!   to 30 requests that you want Verified Permissions to evaluate.
//!
//! ## Output
//!
//! * `principal` (EntityIdentifier) — The identifier of the principal in the ID or access
//!   token.
//! * `results` (BatchIsAuthorizedWithTokenOutputList) **required** — A series of Allow or
//!   Deny decisions for each request, and the policies that produced them. These results are
//!   returned in the order they were requested.
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
// `BatchIsAuthorizedWithTokenError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `BatchIsAuthorizedWithTokenError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#BatchIsAuthorizedWithToken",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum BatchIsAuthorizedWithTokenErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_is_authorized_with_token::BatchIsAuthorizedWithTokenError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type BatchIsAuthorizedWithTokenError = crate::scaffolded::Scaffolded<BatchIsAuthorizedWithTokenErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<BatchIsAuthorizedWithTokenErrorModeled> for BatchIsAuthorizedWithTokenError {
    fn from(modeled: BatchIsAuthorizedWithTokenErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchIsAuthorizedWithToken` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchIsAuthorizedWithToken<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchIsAuthorizedWithToken<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchIsAuthorizedWithToken<Eng>
{
    type Input = BatchIsAuthorizedWithTokenInput;
    type Output = BatchIsAuthorizedWithTokenOutput;
    type Error = BatchIsAuthorizedWithTokenError;

    const NAME: &'static str = "BatchIsAuthorizedWithToken";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_IS_AUTHORIZED_WITH_TOKEN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_IS_AUTHORIZED_WITH_TOKEN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let not_found = || {
            BatchIsAuthorizedWithTokenErrorModeled::ResourceNotFoundException(
                // An authorization call's not-found NAMES the resource: see
                // `crate::state::resource_not_found_naming`.
                crate::state::policy_store_reference(&input.policy_store_id)
                    .not_found_naming_the_resource(),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            BatchIsAuthorizedWithTokenErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the service state store is unavailable".to_string(),
                },
            )
        })?;
        let state = state.lock();
        // The store the request names is resolved BEFORE any of the request's own payload
        // — the presented tokens included — is read, so a reference to a store that does
        // not exist answers `ResourceNotFoundException` rather than a refusal about the
        // payload, the same precedence every other authorization operation follows.
        let store_id = state
            .resolve_store_id(
                &crate::state::request_region(ctx.parts()),
                &input.policy_store_id,
            )
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        // The model bounds `requests` below (`@length min 1`) but not above, so the
        // published batch size is refused here rather than at the boundary. The list's size
        // is a member of the request's own payload, so it is read AFTER the store the
        // request references has been resolved, like every other refusal below.
        if input.requests.len() > crate::state::cedar::MAX_BATCH_REQUESTS {
            return Err(invalid(crate::ops::InvalidInput::new(
                crate::ops::paths::REQUESTS,
                format!(
                    "it carries {} authorization requests, more than the {} a batch may carry",
                    input.requests.len(),
                    crate::state::cedar::MAX_BATCH_REQUESTS
                ),
            )));
        }

        // "You must specify either an accessToken, an identityToken, or both": the model
        // declares both members optional, so the boundary validator cannot require one of
        // them and this is the place that does. Both members are named, because either one
        // satisfies the rule.
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

        // The principal is the token's, so each request of the batch carries only the other
        // two members of the Cedar triple — and each of them must be present, because a
        // request is evaluated as a complete triple. An absent one names the member within
        // the request it belongs to, rather than being answered with a deny that would read
        // as "evaluated and denied".
        let required = |index: usize, member: &str| {
            invalid(
                crate::ops::InvalidInput::new(member, REQUIRED_FOR_EVALUATION)
                    .within_batch_request(index),
            )
        };
        let mut pairs: Vec<Pair> = Vec::with_capacity(input.requests.len());
        let mut contexts: Vec<serde_json::Value> = Vec::with_capacity(input.requests.len());
        for (index, item) in input.requests.iter().enumerate() {
            pairs.push(Pair {
                action: crate::state::cedar::request_action(item.action.as_ref())
                    .ok_or_else(|| required(index, crate::ops::paths::ACTION))?,
                resource: crate::state::cedar::request_entity(item.resource.as_ref())
                    .ok_or_else(|| required(index, crate::ops::paths::RESOURCE))?,
            });
            contexts.push(
                crate::state::cedar::context_to_cedar_json(item.context.as_ref())
                    // The conversion names the member within ONE request's `context`; the
                    // batch says which request that was.
                    .map_err(|e| invalid(e.within_batch_request(index)))?,
            );
        }

        // `entities` is a property of the BATCH, not of one request: one entity slice is
        // built once and every request is evaluated against it. A refusal here names
        // `/entities` itself, with no request index, because that is where the member lives.
        let mut entities = crate::state::cedar::entities_to_cedar_json(
            input.entities.as_ref(),
            crate::state::cedar::MAX_ENTITIES_PER_TOKEN_BATCH,
        )
        .map_err(invalid)?;

        // Each presented token is verified against the store's identity sources, and a
        // call that presented two is decided for the one principal they agree on.
        let now = ::std::time::SystemTime::now();
        let mut identity: Option<crate::state::token::TokenIdentity> = None;
        for token in presented {
            let verified = crate::state::token::verify(&*self.engine, store, token, now)
                .map_err(|refusal| refused_token(&input.policy_store_id, refusal))?;
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
            BatchIsAuthorizedWithTokenErrorModeled::InternalServerException(
                InternalServerException {
                    message: "no presented token was read".to_string(),
                },
            )
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

        // Every policy of the store, parsed ONCE for the whole batch, and one entity store
        // built once from the slice the batch carried plus the entities the token named.
        let prepared = crate::state::cedar::load_policies(store).map_err(refused_by_cedar)?;
        let entities =
            crate::state::cedar::build_entities(entities, prepared.schema()).map_err(invalid)?;

        let mut results = Vec::with_capacity(input.requests.len());
        for (index, ((item, pair), context)) in input
            .requests
            .iter()
            .zip(&pairs)
            .zip(contexts)
            .enumerate()
        {
            let verdict = crate::state::cedar::evaluate(
                &prepared,
                &identity.principal,
                &pair.action,
                &pair.resource,
                crate::state::cedar::RequestContext {
                    json: context,
                    path: crate::state::cedar::context_path(item.context.as_ref()),
                },
                &entities,
            )
            // The evaluation names the member within ONE request; the batch says which
            // request that was.
            .map_err(|e| invalid(e.within_batch_request(index)))?;

            results.push(BatchIsAuthorizedWithTokenOutputItem {
                // "The authorization request that initiated the decision": the request is
                // echoed back so a caller can line a result up with it.
                request: item.clone(),
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
            });
        }

        Ok(BatchIsAuthorizedWithTokenOutput {
            // "These results are returned in the order they were requested."
            results,
            // "The identifier of the principal in the ID or access token" — the entity
            // every result was decided for, which is the token's and not any request's.
            principal: Some(EntityIdentifier {
                entity_type: identity.principal.entity_type,
                entity_id: identity.principal.entity_id,
            }),
        })
    }
}

/// What ONE request of this batch supplies of the Cedar triple: the principal is the
/// token's, so a request carries the other two members and nothing else.
struct Pair {
    action: crate::state::ScopeEntity,
    resource: crate::state::ScopeEntity,
}

/// Why an absent member of a request's half-triple is refused: both members are modelled
/// optional, so the boundary validator cannot require them and this is the place that does.
const REQUIRED_FOR_EVALUATION: &str = "it is required to evaluate an authorization request";

/// Why a call presenting no token at all is refused.
const TOKEN_REQUIRED: &str =
    "an authorization request takes its principal from a token, so either an access token \
     or an identity token is required";

/// Why a call presenting two tokens of two different users is refused: one call decides for
/// one principal.
const DIFFERENT_PRINCIPALS: &str =
    "the access token and the identity token name different principals";

/// The refusal a presented token whose audience the identity source does not accept is
/// answered with: ONE fixed sentence, whichever member the token arrived in and whichever
/// of its configured audiences the source lists them as, so a client can tell an audience
/// failure apart from every other validation failure by matching it. It has no variable
/// part — in particular the rejected token's own audience is not reported back.
const AUDIENCE_NOT_ACCEPTED: &str = "Token audience claim contains no valid audience";

/// One offending request member as this operation's modelled `ValidationException`, whose
/// `fieldList` names it.
fn invalid(invalid: crate::ops::InvalidInput) -> BatchIsAuthorizedWithTokenError {
    invalid_members([invalid])
}

/// Several offending request members as one modelled `ValidationException`, for a rule that
/// is about the members TOGETHER: each one is reported in `fieldList`, because any of them
/// is where a client fixes the request.
fn invalid_members(
    invalid: impl IntoIterator<Item = crate::ops::InvalidInput>,
) -> BatchIsAuthorizedWithTokenError {
    BatchIsAuthorizedWithTokenErrorModeled::ValidationException(
        crate::ops::validation_exception(invalid),
    )
    .into()
}

/// A refusal [`crate::state::cedar`] raised, on this operation's error channel.
///
/// The same split the other authorization operations make: a member the caller can fix is
/// the modelled `ValidationException`, and service-side state it cannot — a stored schema
/// that will not parse, a store holding more Cedar than one request evaluates — is a server
/// fault. An authorization request carries no policy content, so reasons reaching the
/// schema arm are about content the store already holds, which is service-side state too.
fn refused_by_cedar(refusal: crate::state::cedar::Refusal) -> BatchIsAuthorizedWithTokenError {
    match refusal {
        crate::state::cedar::Refusal::InvalidInput(field) => invalid(field),
        crate::state::cedar::Refusal::ServiceFault(message) => {
            BatchIsAuthorizedWithTokenErrorModeled::InternalServerException(
                InternalServerException { message },
            )
            .into()
        }
        crate::state::cedar::Refusal::SchemaValidation(reasons) => {
            BatchIsAuthorizedWithTokenErrorModeled::InternalServerException(
                InternalServerException {
                    message: reasons.join("; "),
                },
            )
            .into()
        }
    }
}

/// The modelled error a token this call could not use is refused with — the same three
/// answers [`crate::ops::is_authorized_with_token`] gives, because the token rules are the
/// token's and not the calling operation's.
///
/// A token that does not verify is refused as a BAD REQUEST, worded as the sentence naming
/// the member and what failed — `Failed to verify IdentityToken: signature is invalid` —
/// while `fieldList` still names the member for a client that reads it. An audience the
/// identity source does not accept is the exception the service words as fixed text instead
/// ([`AUDIENCE_NOT_ACCEPTED`]).
///
/// A token whose BODY did not DECODE into a claim set is the same bad request worded for the
/// stage it failed at — `Failed to decode IdentityToken: <what the deserialization
/// reported>` — because nothing about such a token was verified: a client reading the
/// message is told to repair the token it presented, where the verify sentence would send it
/// to the trust its policy store configures.
///
/// A token whose issuer is no identity source of the store is the other kind of answer:
/// nothing about the token is wrong, and what the call referenced — an identity source for
/// that issuer IN this policy store — does not exist, so it is the modelled not-found,
/// worded as the reference it was.
///
/// A token whose signature could not be CHECKED is the third: nothing about the request is
/// wrong and this service cannot say whether the signature is good, which is the modelled
/// server fault rather than a refusal of the token.
fn refused_token(
    policy_store_id: &str,
    refusal: crate::state::token::TokenRefusal,
) -> BatchIsAuthorizedWithTokenError {
    match refusal {
        crate::state::token::TokenRefusal::NotDecoded { kind, reason } => {
            BatchIsAuthorizedWithTokenErrorModeled::ValidationException(ValidationException {
                message: format!("Failed to decode {}: {reason}", kind.reported_name()),
                field_list: Some(vec![ValidationExceptionField {
                    path: kind.path().to_string(),
                    message: reason,
                }]),
            })
            .into()
        }
        crate::state::token::TokenRefusal::NotVerified { kind, reason } => {
            BatchIsAuthorizedWithTokenErrorModeled::ValidationException(ValidationException {
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
            BatchIsAuthorizedWithTokenErrorModeled::ValidationException(ValidationException {
                message: AUDIENCE_NOT_ACCEPTED.to_string(),
                field_list: Some(vec![ValidationExceptionField {
                    path: kind.path().to_string(),
                    message: AUDIENCE_NOT_ACCEPTED.to_string(),
                }]),
            })
            .into()
        }
        crate::state::token::TokenRefusal::NoIdentitySource(issuer) => {
            BatchIsAuthorizedWithTokenErrorModeled::ResourceNotFoundException(
                crate::ops::identity_source_not_found_for_issuer(&issuer, policy_store_id),
            )
            .into()
        }
        crate::state::token::TokenRefusal::NotCheckable { issuer, reason } => {
            BatchIsAuthorizedWithTokenErrorModeled::InternalServerException(
                InternalServerException {
                    message: format!(
                        "the signature of the token the issuer {issuer} minted could not be \
                         checked: {reason}"
                    ),
                },
            )
            .into()
        }
    }
}
