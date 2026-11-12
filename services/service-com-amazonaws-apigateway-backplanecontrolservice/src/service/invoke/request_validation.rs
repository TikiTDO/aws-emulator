// SPDX-License-Identifier: CC0-1.0

//! The method's own REQUEST VALIDATOR: what a `RequestValidator` the method names makes of
//! the call before the integration is reached.
//!
//! `RequestValidator` declares which halves of a request are checked
//! (`validateRequestParameters`, `validateRequestBody`), and both halves are decided here:
//! the parameters off the call's own query string and headers, and the payload against the
//! JSON Schema of the `Model` the method maps the request's content type to. One concern,
//! in a file of its own, reached by BOTH sides of this plane — the `execute-api` data plane
//! off the wire and `TestInvokeMethod` out of the members a caller simulates a request
//! with — so one validator decides a call that reached a socket and one that never did.
//!
//! Moved out of the parent module verbatim: the parent resolves and serves a call, and this
//! is the one place that decides whether the method's own declarations admit it.

use super::*;

/// The `RequestValidator` the method names, or `None` when it names none the API holds.
///
/// A `Method` names its validator in `requestValidatorId`, and the `RequestValidator` that
/// id resolves to declares which halves of the request are checked
/// (`validateRequestParameters`, `validateRequestBody`, each `@default false`): a method
/// that names none, or names one of the two flags only, is checked exactly as far as it
/// declares — which is why the four modes differ in what they refuse. A
/// `requestValidatorId` naming no validator of the API validates nothing: that is a
/// configuration this emulator cannot apply, and inventing a refusal for it would refuse
/// requests AWS serves.
pub(super) fn declared_validator<'a>(
    api: &'a RestApiEntry,
    method: Option<&MethodEntry>,
) -> Option<&'a crate::state::RequestValidatorEntry> {
    api.request_validators
        .get(method?.request_validator_id.as_deref()?)
}

/// The refusal the PARAMETERS half of the method's request validator earns this call, or
/// `None` when it declares no parameter check or the call carries everything the method
/// declares required.
///
/// The parameters are checked FIRST, before the payload: a request that does not even
/// carry what the method declares required is refused for that rather than for whatever
/// its payload says.
///
/// `pub` with [`pending_body_check`] because `TestInvokeMethod` applies the SAME validator
/// to the request a caller simulates — "a body that violates the model attached through the
/// method's requestValidator is refused before the integration runs" holds whether the call
/// reached a socket or not — and it applies it through these two functions rather than a
/// reading of its own.
pub fn parameters_refusal(
    api: &RestApiEntry,
    method: Option<&MethodEntry>,
    call: &MethodRequest,
) -> Option<GatewayRefusal> {
    let validator = declared_validator(api, method)?;
    if !validator.validate_request_parameters {
        return None;
    }
    let missing = missing_parameters(method?, call);
    (!missing.is_empty()).then(|| bad_request_parameters(&missing))
}

/// The names of the request parameters the method declares REQUIRED that this call does
/// not carry, in the order the method declares them.
///
/// `Method$requestParameters` is "a key-value map defining required or optional method
/// request parameters … the value is a Boolean flag indicating whether the parameter is
/// required", so only the `true` ones can be missing. A parameter is carried when it is
/// present and NOT BLANK, which is what request validation verifies of a query-string
/// parameter or a header.
///
/// Read off the CALL's own parameters — the query string and the headers it carries, as the
/// one reader of each decoded them — so a request off the wire and one a caller simulated
/// are checked against the same declarations. A header name is compared case-insensitively,
/// because an HTTP field name is.
///
/// A PATH parameter is never missing: the request path matched the resource template that
/// declares it, which cannot match with that segment absent.
pub(super) fn missing_parameters(method: &MethodEntry, call: &MethodRequest) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for (expression, required) in &method.request_parameters {
        if !required {
            continue;
        }
        let Some((source, name)) = parameter_source(expression) else {
            continue;
        };
        let carried = match source {
            ParameterIn::Query => call
                .inputs
                .querystring
                .get(name)
                .is_some_and(|value| !value.is_empty()),
            ParameterIn::Header => call
                .inputs
                .header
                .iter()
                .find(|(sent, _)| sent.eq_ignore_ascii_case(name))
                .is_some_and(|(_, value)| !value.is_empty()),
            ParameterIn::Path => true,
        };
        if !carried && !missing.iter().any(|already| already == name) {
            missing.push(name.to_string());
        }
    }
    missing
}

/// The URL the model `model_name` of the API `api_id` is a JSON Schema resource under —
/// the identifier API Gateway gives a model's schema, which is what a `$ref` from another
/// model of the same API names.
pub(super) fn model_schema_url(api_id: &str, model_name: &str) -> String {
    format!("https://apigateway.amazonaws.com/restapis/{api_id}/models/{model_name}")
}

/// The schema resources this validator resolves a `$ref` against: NOTHING but what this
/// module registered itself.
///
/// A `Model`'s schema is a document any caller stores, and `$ref` is a dynamic resolution
/// mechanism, so a schema could name a resource by any URL — including a `file:` one.
/// Registering this as the compiler's loader REPLACES the one it comes with, so a
/// reference the API's own models do not satisfy resolves to an error instead of reaching
/// the host's filesystem or network: the only resources are the ones
/// [`PendingBodyCheck::refusal`] adds, which are models of the api being invoked.
#[derive(Debug)]
pub(super) struct NoSchemasBeyondTheApi;

impl ::boon::UrlLoader for NoSchemasBeyondTheApi {
    fn load(&self, url: &str) -> Result<::serde_json::Value, Box<dyn ::std::error::Error>> {
        tracing::warn!(
            reference = %url,
            "the model's schema references a resource outside the api's own models, which \
             this emulator resolves nothing for"
        );
        Err("this emulator resolves no schema outside the api's own models".into())
    }
}

/// What validating one request's PAYLOAD needs, owned: the schema to validate it against
/// and the sibling schemas that schema references.
///
/// Owned rather than borrowed on purpose. Compiling and evaluating a JSON Schema is the
/// one piece of work in this data plane whose cost a caller's own RECORDS decide, so it is
/// done with the shared store's lock RELEASED (see [`serve_resolved`]) — one request's
/// payload check cannot hold up every other request to this service. The copy is of the
/// stored schema text, which `CreateModel` bounds at
/// [`crate::ops::create_model::MAX_MODEL_SCHEMA_CHARS`], and of the siblings that schema
/// names, so the work and the copy are bounded by records a control-plane call had to
/// store rather than by anything the invoke carries.
#[derive(Debug)]
pub struct PendingBodyCheck {
    /// The api whose model this is, for the log and the schema's resource URL.
    pub(super) api_id: String,
    /// The model the method declared for this request's content type.
    pub(super) model_name: String,
    /// That model's schema, as stored.
    pub(super) schema: String,
    /// The sibling models the schema REFERENCES by their model-schema URL, as
    /// `(url, stored schema)`.
    pub(super) referenced: Vec<(String, String)>,
}

/// The payload check the method's request validator declares for this call, or `None` when
/// it declares none that can be applied.
///
/// `Method$requestModels` is "a key-value map specifying data schemas … a key is a content
/// type and the value is a `Model` name", so a content type the method maps no model to
/// declares nothing to validate against. `None` is also the answer when the model names no
/// record of the API or carries no schema: each is a configuration this emulator cannot
/// validate against, and inventing a refusal for it would refuse requests AWS serves.
///
/// `pub` for the reason [`parameters_refusal`] gives: `TestInvokeMethod` applies the same
/// validator to the request a caller simulates, through this function and
/// [`PendingBodyCheck::refusal`].
pub fn pending_body_check(
    api: &RestApiEntry,
    method: Option<&MethodEntry>,
    call: &MethodRequest,
) -> Option<PendingBodyCheck> {
    let validator = declared_validator(api, method)?;
    if !validator.validate_request_body {
        return None;
    }
    let model_name = method?.request_models.get(call.media_type())?;
    let schema = api.models.get(model_name)?.schema.as_deref()?.to_string();
    // A schema composed of references to OTHER models of the API resolves them: API
    // Gateway identifies a model's schema by [`model_schema_url`], so a sibling the schema
    // REFERENCES BY THAT URL is carried over with it. Only a referenced sibling is, so the
    // work one request causes is bounded by the references the schema carries rather than
    // by how many models the API holds.
    let referenced = if schema.contains("$ref") {
        api.models
            .iter()
            .filter(|(name, _)| *name != model_name)
            .filter_map(|(name, sibling)| {
                let url = model_schema_url(&api.id, name);
                let text = sibling.schema.clone()?;
                schema.contains(&url).then_some((url, text))
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(PendingBodyCheck {
        api_id: api.id.clone(),
        model_name: model_name.clone(),
        schema,
        referenced,
    })
}

impl PendingBodyCheck {
    /// The refusal this request's payload earns against the model, or `None` when it
    /// satisfies it.
    ///
    /// `CreateModel$schema` is "the schema for the model. For application/json models,
    /// this should be JSON schema draft 4 model", so the payload is validated against that
    /// document by a JSON Schema validator with draft 4 as the default dialect for a schema
    /// that names none. A payload that is not a JSON document at all does not satisfy a
    /// JSON Schema, so an unparseable (including an empty) body is refused exactly as a
    /// structurally wrong one is; a body the transport did not buffer in memory
    /// (`None`) is one this emulator cannot read, so it is validated against nothing.
    ///
    /// A stored schema this validator cannot READ as a JSON Schema — a document that is a
    /// JSON object, which is all `CreateModel` requires of it, but no schema, or one whose
    /// references resolve to nothing — is a method the stage cannot serve as configured:
    /// the validator the method declares cannot be applied to anything, so the call is
    /// answered through `API_CONFIGURATION_ERROR` exactly as an integration this module
    /// cannot perform is, rather than served as if the check had passed.
    ///
    /// The work is bounded by its two inputs: the schema is at most
    /// [`crate::ops::create_model::MAX_MODEL_SCHEMA_CHARS`] and the payload is what the
    /// front door admitted, and a JSON Schema evaluation applies each keyword of the one
    /// to the members of the other. Nothing here compiles a pattern into a backtracking
    /// engine either — the validator's regexes are the `regex` crate's, which has no
    /// backtracking — so a stored `pattern` cannot make the evaluation super-linear in the
    /// payload.
    pub fn refusal(&self, payload: Option<&[u8]>) -> Option<GatewayRefusal> {
        let Ok(schema) = ::serde_json::from_str::<::serde_json::Value>(&self.schema) else {
            return Some(self.unapplicable_schema("it is not a JSON document"));
        };
        let Ok(instance) = ::serde_json::from_slice::<::serde_json::Value>(payload?) else {
            return Some(bad_request_body(unreadable_payload_detail()));
        };

        let mut compiler = ::boon::Compiler::new();
        // The only schema resources are the ones registered below: see
        // [`NoSchemasBeyondTheApi`] for why the compiler's own loader is replaced.
        compiler.use_loader(Box::new(NoSchemasBeyondTheApi));
        // "For application/json models, this should be JSON schema draft 4 model"
        // (`CreateModel$schema`), so draft 4 is the dialect a schema that declares no
        // `$schema` of its own is read under; one that declares a dialect keeps it.
        compiler.set_default_draft(::boon::Draft::V4);
        for (url, text) in &self.referenced {
            if let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(text) {
                let _ = compiler.add_resource(url, document);
            }
        }
        let url = model_schema_url(&self.api_id, &self.model_name);
        if compiler.add_resource(&url, schema).is_err() {
            return Some(self.unapplicable_schema("it is not a schema resource"));
        }
        let mut schemas = ::boon::Schemas::new();
        let Ok(compiled) = compiler.compile(&url, &mut schemas) else {
            return Some(self.unapplicable_schema(
                "it is not a JSON Schema this emulator can compile, or it references a \
                 resource that resolves to nothing",
            ));
        };
        schemas
            .validate(&instance, compiled)
            .err()
            .map(|failed| bad_request_body(validation_detail(&failed)))
    }

    /// The refusal a stored schema the validator cannot be built from earns, said once:
    /// the method declares a check its own records make impossible, which is a
    /// configuration the stage cannot serve the call through.
    pub(super) fn unapplicable_schema(&self, reason: &str) -> GatewayRefusal {
        tracing::warn!(
            api_id = %self.api_id,
            model = %self.model_name,
            reason = %reason,
            "the method's request validator cannot be applied: the model's schema is not \
             one this emulator can validate against"
        );
        API_CONFIGURATION_ERROR
    }
}

/// The validation detail a payload that is not a JSON document at all earns: there is no
/// schema finding to report, because the document the schema would be applied to could not
/// be read.
pub(super) fn unreadable_payload_detail() -> String {
    "[the request body could not be read as a JSON document]".to_string()
}

/// The validation's own finding about the payload, as one line, for
/// `$context.error.validationErrorString`.
///
/// It is the VALIDATOR's report — which member of the payload failed which keyword of the
/// model's schema — flattened to one line and bracketed the way API Gateway brackets that
/// member in a gateway response, so a developer reading the refusal can see which part of
/// the payload the model rejected. Nothing of it is written here: the schema's own URL line
/// is dropped (it names the model this emulator compiled, not the payload) and the rest is
/// the validator's text.
pub(super) fn validation_detail(failed: &::boon::ValidationError) -> String {
    let reported = failed.to_string();
    let findings = reported
        .split_once('\n')
        .map(|(_schema, findings)| findings)
        .unwrap_or(reported.as_str());
    let flattened: Vec<&str> = findings
        .split_whitespace()
        .filter(|word| *word != "-")
        .collect();
    format!("[{}]", flattened.join(" "))
}
