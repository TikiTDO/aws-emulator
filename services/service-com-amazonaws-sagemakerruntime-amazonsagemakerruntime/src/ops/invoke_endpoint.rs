// SPDX-License-Identifier: CC0-1.0

//! @op InvokeEndpoint
//! IMPLEMENTED: decides the CALLER's authorization first — the identity-based IAM
//! policies in force on the principal the request signed as, for
//! `sagemaker:InvokeEndpoint` on the endpoint it addressed
//! ([`crate::state::authorization`]), refused 403 `AccessDeniedException` before anything
//! is resolved — then resolves the endpoint the request names in the SageMaker CONTROL
//! PLANE's store (through the one shared engine), refuses a request that names no
//! endpoint, an endpoint that is not yet `InService`, a `TargetVariant` the endpoint
//! does not host, `EnableExplanations` on an endpoint with no explainer configuration or
//! a `SessionId` naming no stateful session — then, once that addressing has resolved,
//! a `Body` past the maximum the model declares for it — and finally SERVES the inference
//! from the model deployed behind the variant the call routes to
//! (`sagemaker::state::model_serving`), answering the modelled 424 `ModelError` naming the
//! condition when there is nothing to compute from.
//! Every call is logged in [`crate::state`]. This is the ONLY file to touch for this
//! operation.
//!
//! ## Why the payload is examined here and not at the boundary
//!
//! The invocation is ADDRESSED to an endpoint by the `@httpLabel` in its URI, and the
//! service resolves that endpoint before it looks at what the request carries — so a
//! request naming an endpoint the account does not host is refused for that whatever its
//! payload is. That order is stated once for the whole service in
//! [`crate::service::error_envelope`], which also renders every refusal raised before
//! this handler runs.
//!
//! ## What answers the inference
//!
//! A SageMaker model is a CONTAINER IMAGE of customer code, which a generated emulator
//! cannot run. What this emulator CAN serve is a model it fitted itself: a training job or
//! an AutoML job run here writes its fitted parameters into the `model.tar.gz` under the
//! job's output location, `CreateModel` deploys that artifact as a container's
//! `ModelDataUrl`, and this operation reads the parameters back out of it and computes the
//! prediction for the payload the caller sent. So the body answered is a MEASUREMENT over
//! the caller's own request, from a model trained on the caller's own data.
//!
//! Where there is nothing to compute from — a variant whose model names no artifact, an
//! artifact that cannot be read, one this emulator's own runs did not produce, or a payload
//! holding no record the model applies to — nothing computed a prediction, and the answer
//! is the modelled error for an invocation the model did not serve rather than a
//! synthesised `Body`. Of the six errors the operation models, `ModelError` (424) is the
//! one that says exactly that: the model behind the endpoint did not return an inference.
//! The retryable ones would be false — `ModelNotReadyException` (429) and
//! `ServiceUnavailable` (503) both promise that waiting and retrying helps, and nothing
//! here ever becomes ready — and `ValidationError` (400) would blame a request that is
//! valid.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `InvokeEndpoint::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
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
//! # Modeled contract for `InvokeEndpoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json")
//! ```
//!
//! After you deploy a model into production using Amazon SageMaker AI hosting services,
//! your client applications use this API to get inferences from the model hosted at the
//! specified endpoint. For an overview of Amazon SageMaker AI, see How It Works. Amazon
//! SageMaker AI strips all POST headers except those supported by the API. Amazon SageMaker
//! AI might add additional headers. You should not rely on the behavior of headers outside
//! those enumerated in the request syntax. Calls to InvokeEndpoint are authenticated by
//! using Amazon Web Services Signature Version 4. For information, see Authenticating
//! Requests (Amazon Web Services Signature Version 4) in the Amazon S3 API Reference. A
//! customer's model containers must respond to requests within 60 seconds. The model itself
//! can have a maximum processing time of 60 seconds before responding to invocations. If
//! your model is going to take 50-60 seconds of processing time, the SDK socket timeout
//! should be set to be 70 seconds. Endpoints are scoped to an individual account, and are
//! not public. The URL does not contain the account ID, but Amazon SageMaker AI determines
//! the account ID from the authentication token that is supplied by the caller.
//!
//! ## Input
//!
//! * `Accept` (Header, len 0..1024, pattern ^\p{ASCII}*$) — The desired MIME type of the
//!   inference response from the model container.
//! * `Body` (BodyBlob, len 0..6291456) **required** — Provides input data, in the format
//!   specified in the ContentType request header. Amazon SageMaker AI passes all of the data
//!   in the body to the model. For information about the format of the request body, see
//!   Common Data Formats-Inference.
//! * `ContentType` (Header, len 0..1024, pattern ^\p{ASCII}*$) — The MIME type of the input
//!   data in the request body.
//! * `CustomAttributes` (CustomAttributesHeader, len 0..1024, pattern ^\p{ASCII}*$) —
//!   Provides additional information about a request for an inference submitted to a model
//!   hosted at an Amazon SageMaker AI endpoint. The information is an opaque value that is
//!   forwarded verbatim. You could use this value, for example, to provide an ID that you can
//!   use to track a request or to provide other metadata that a service endpoint was
//!   programmed to process. The value must consist of no more than 1024 visible US-ASCII
//!   characters as specified in Section 3.3.6. Field Value Components of the Hypertext
//!   Transfer Protocol (HTTP/1.1). The code in your model is responsible for setting or
//!   updating any custom attributes in the response. If your code does not set this value in
//!   the response, an …
//! * `EnableExplanations` (EnableExplanationsHeader, len 1..64, pattern .*) — An optional
//!   JMESPath expression used to override the EnableExplanations parameter of the
//!   ClarifyExplainerConfig API. See the EnableExplanations section in the developer guide
//!   for more information.
//! * `EndpointName` (EndpointName, len 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$)
//!   **required** — The name of the endpoint that you specified when you created the endpoint
//!   using the CreateEndpoint API.
//! * `InferenceComponentName` (InferenceComponentHeader, len 0..63, pattern
//!   ^[a-zA-Z0-9]([\-a-zA-Z0-9]*[a-zA-Z0-9])?$) — If the endpoint hosts one or more inference
//!   components, this parameter specifies the name of inference component to invoke.
//! * `InferenceId` (InferenceId, len 1..64, pattern ^\A\S[\p{Print}]*\z$) — If you provide a
//!   value, it is added to the captured data when you enable data capture on the endpoint.
//!   For information about data capture, see Capture Data.
//! * `PrefixAwareId` (PrefixAwareIdHeader, len 1..64, pattern ^\A\S[\p{Print}]*\z$) — An
//!   optional, stable identifier that serves as a routing hint for prefix-aware routing. The
//!   service routes requests with the same prefix and the same identifier to the same
//!   instance. If requests from different applications might have the same prompt prefix, set
//!   a different identifier for each application to differentiate their routing decisions.
//!   Applies only to endpoints configured with a RoutingStrategy of PREFIX_AWARE.
//! * `SessionId` (SessionIdOrNewSessionConstantHeader, len 0..256, pattern
//!   ^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$) — Creates a stateful session or
//!   identifies an existing one. You can do one of the following: Create a stateful session
//!   by specifying the value NEW_SESSION. Send your request to an existing stateful session
//!   by specifying the ID of that session. With a stateful session, you can send multiple
//!   requests to a stateful model. When you create a session with a stateful model, the model
//!   must create the session ID and set the expiration time. The model must also provide that
//!   information in the response to your request. You can get the ID and timestamp from the
//!   NewSessionId response parameter. For any subsequent request where you specify that
//!   session ID, SageMaker AI routes the request to the same …
//! * `TargetContainerHostname` (TargetContainerHostnameHeader, len 0..63, pattern
//!   ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$) — If the endpoint hosts multiple containers and is
//!   configured to use direct invocation, this parameter specifies the host name of the
//!   container to invoke.
//! * `TargetModel` (TargetModelHeader, len 1..1024, pattern ^\A\S[\p{Print}]*\z$) — The model
//!   to request for inference when invoking a multi-model endpoint.
//! * `TargetVariant` (TargetVariantHeader, len 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$)
//!   — Specify the production variant to send the inference request to when invoking an
//!   endpoint that is running two or more variants. Note that this parameter overrides the
//!   default behavior for the endpoint, which is to distribute the invocation traffic based
//!   on the variant weights. For information about how to use variant targeting to perform
//!   a/b testing, see Test models in production
//!
//! ## Output
//!
//! * `Body` (BodyBlob, len 0..6291456) **required** — Includes the inference provided by the
//!   model. For information about the format of the response body, see Common Data
//!   Formats-Inference. If the explainer is activated, the body includes the explanations
//!   provided by the model. For more information, see the Response section under Invoke the
//!   Endpoint in the Developer Guide.
//! * `ClosedSessionId` (SessionIdHeader, len 0..256, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$) —
//!   If you closed a stateful session with your request, the ID of that session.
//! * `ContentType` (Header, len 0..1024, pattern ^\p{ASCII}*$) — The MIME type of the
//!   inference returned from the model container.
//! * `CustomAttributes` (CustomAttributesHeader, len 0..1024, pattern ^\p{ASCII}*$) —
//!   Provides additional information in the response about the inference returned by a model
//!   hosted at an Amazon SageMaker AI endpoint. The information is an opaque value that is
//!   forwarded verbatim. You could use this value, for example, to return an ID received in
//!   the CustomAttributes header of a request or other metadata that a service endpoint was
//!   programmed to produce. The value must consist of no more than 1024 visible US-ASCII
//!   characters as specified in Section 3.3.6. Field Value Components of the Hypertext
//!   Transfer Protocol (HTTP/1.1). If the customer wants the custom attribute returned, the
//!   model must set the custom attribute to be included on the way back. The code in your
//!   model is …
//! * `InvokedProductionVariant` (Header, len 0..1024, pattern ^\p{ASCII}*$) — Identifies the
//!   production variant that was invoked.
//! * `NewSessionId` (NewSessionResponseHeader, len 0..256, pattern
//!   ^[a-zA-Z0-9](-*[a-zA-Z0-9])*;\sExpires=[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$)
//!   — If you created a stateful session with your request, the ID and expiration time that
//!   the model assigns to that session.
//!
//! ## Errors
//!
//! * `InternalDependencyException` (HTTP 530) — Your request caused an exception with an
//!   internal dependency. Contact customer support.
//! * `InternalFailure` (HTTP 500) — An internal failure occurred.
//! * `ModelError` (HTTP 424) — Model (owned by the customer in the container) returned 4xx or
//!   5xx error code.
//! * `ModelNotReadyException` (HTTP 429) — Either a serverless endpoint variant's resources
//!   are still being provisioned, or a multi-model endpoint is still downloading or loading
//!   the target model. Wait and try your request again.
//! * `ServiceUnavailable` (HTTP 503) — The service is unavailable. Try your call again.
//! * `ValidationError` (HTTP 400) — Inspect your request and try again.

#[allow(unused_imports)]
use super::types::*;

/// The SageMaker CONTROL PLANE, whose store owns every endpoint this data plane
/// invokes. It is reached through the one shared engine — the same store
/// `CreateEndpoint` and `DescribeEndpoint` write and read — so this service keeps no
/// second copy of an endpoint that could drift from it.
use ::service_com_amazonaws_sagemaker_sagemaker as sagemaker;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `InvokeEndpointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `InvokeEndpointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.sagemakerruntime#InvokeEndpoint")]
#[derive(Debug)]
pub enum InvokeEndpointErrorModeled {
    InternalDependencyException(InternalDependencyException),
    InternalFailure(InternalFailure),
    ModelError(ModelError),
    ModelNotReadyException(ModelNotReadyException),
    ServiceUnavailable(ServiceUnavailable),
    ValidationError(ValidationError),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::invoke_endpoint::InvokeEndpointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type InvokeEndpointError = crate::scaffolded::Scaffolded<InvokeEndpointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<InvokeEndpointErrorModeled> for InvokeEndpointError {
    fn from(modeled: InvokeEndpointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// This operation's modelled 400 `ValidationError` ("Inspect your request and try
/// again.") — the error the real service answers for a request it refuses.
fn validation(message: impl Into<String>) -> InvokeEndpointError {
    InvokeEndpointErrorModeled::ValidationError(ValidationError {
        message: Some(message.into()),
    })
    .into()
}

/// This operation's modelled 500 `InternalFailure`, for a fault of this process rather
/// than of the request.
fn internal_failure(message: impl Into<String>) -> InvokeEndpointError {
    InvokeEndpointErrorModeled::InternalFailure(InternalFailure {
        message: Some(message.into()),
    })
    .into()
}

/// The `InvokeEndpoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct InvokeEndpoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeEndpoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for InvokeEndpoint<Eng>
{
    type Input = InvokeEndpointInput;
    type Output = InvokeEndpointOutput;
    type Error = InvokeEndpointError;

    const NAME: &'static str = "InvokeEndpoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INVOKE_ENDPOINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INVOKE_ENDPOINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS evaluates the caller's authorization at the front door, BEFORE the endpoint
        // the request addresses is resolved: a principal whose identity-based policies do
        // not allow `sagemaker:InvokeEndpoint` on that endpoint is refused 403
        // `AccessDeniedException` whatever the account hosts, so the refusal discloses
        // nothing and this invocation is never recorded as one the data plane served. The
        // decision is the SageMaker control plane's own gate, read through
        // [`crate::state::authorization`].
        if let Some(denial) = crate::state::authorization::refusal(
            &*self.engine,
            ctx,
            Self::NAME,
            &input.endpoint_name,
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }

        let Some(log) = crate::state::service_state(&*self.engine) else {
            return Err(internal_failure(crate::state::NO_STATE));
        };

        let now = ::std::time::SystemTime::now();
        // Everything the clock already owes this service's own store is materialised
        // before this request extends it: a queued asynchronous submission whose outcome
        // was due has it whether or not anybody called again.
        log.lock().settle_async_invocations(now);
        // "Endpoints are scoped to an individual account" (this operation's model doc),
        // and this emulator serves the one account every emulated resource belongs to.
        let account = ::engine::EMULATOR_ACCOUNT_ID;

        // This call, as the data-plane log replays it once its outcome is known. The
        // `@sensitive` members (the body, `CustomAttributes`) are deliberately absent:
        // only the body's SIZE is recorded.
        let attempt = |outcome: crate::state::InvocationOutcome| crate::state::EndpointInvocation {
            endpoint_name: input.endpoint_name.clone(),
            target_variant: input.target_variant.clone(),
            request_bytes: input.body.len(),
            content_type: input.content_type.clone(),
            accept: input.accept.clone(),
            inference_id: input.inference_id.clone(),
            target_model: input.target_model.clone(),
            session_id: input.session_id.clone(),
            outcome,
            at: now,
        };
        // Record the call: in this service's own log, and as one structured audit event
        // carrying the operation, the endpoint it named, the access-key id the REQUEST
        // presented and the outcome. Only request METADATA goes into either — never the
        // `@sensitive` body or custom attributes, and never a credential's secret.
        let record = |outcome: crate::state::InvocationOutcome| {
            let invocation = attempt(outcome);
            tracing::info!(
                operation = Self::NAME,
                endpoint = %invocation.endpoint_name,
                target_variant = invocation.target_variant.as_deref().unwrap_or_default(),
                request_bytes = invocation.request_bytes,
                outcome = outcome.label(),
                access_key_id = %::emulator_server::extract_akid(&ctx.parts().headers),
                "sagemaker runtime endpoint invocation"
            );
            log.lock().record_invocation(invocation);
        };
        // Record the invocation, then refuse it: a refused invocation is still one the
        // data plane served, the way the real service counts it.
        let refuse = |outcome: crate::state::InvocationOutcome, message: String| {
            record(outcome);
            validation(message)
        };

        // The endpoint is the control plane's resource, so it is resolved there and
        // nowhere else, through the one resolver all three operations of this service
        // share (`crate::state::resolve_endpoint`).
        let facts = match crate::state::resolve_endpoint(&*self.engine, &input.endpoint_name, now) {
            crate::state::EndpointLookup::NoControlPlane => {
                return Err(internal_failure(crate::state::NO_CONTROL_PLANE_STATE))
            }
            crate::state::EndpointLookup::NotFound => {
                // The one refusal the three routes state identically, discriminator and
                // all (`crate::scaffolded::Scaffolded::no_such_endpoint`).
                record(crate::state::InvocationOutcome::EndpointNotFound);
                return Err(crate::scaffolded::Scaffolded::no_such_endpoint(
                    &input.endpoint_name,
                ));
            }
            crate::state::EndpointLookup::Found(facts) => facts,
        };
        if facts.settled {
            // The transition is a state change of the control plane's resource, so it
            // leaves the same record there as the call that created the endpoint.
            sagemaker::state::record_state_change(Self::NAME, &facts.arn, ctx);
        }

        // An endpoint serves inference while a deployment is BEHIND it, which is not the
        // same as "its status is `InService`": the real service keeps serving an endpoint
        // through an update (`Updating`, `SystemUpdating`, `RollingBack`), because the
        // previous deployment stays up until the new one lands. What has nothing to route
        // to is an endpoint whose deployment has not landed or has gone away, so those
        // statuses are what is refused, named rather than inferred from the complement of
        // `InService`. (Today only `Creating` and `InService` are reachable, since
        // `UpdateEndpoint` is still a stub; the rule is stated for the whole modelled
        // enum, `ENDPOINT_STATUS_VALUES` in the SageMaker crate's `shared_types`.)
        if crate::state::NON_SERVING_ENDPOINT_STATUSES.contains(&facts.status.as_str()) {
            return Err(refuse(
                crate::state::InvocationOutcome::EndpointNotInService,
                format!(
                    "Endpoint {} of account {account} is not in service: its status is {}.",
                    input.endpoint_name, facts.status
                ),
            ));
        }

        // `TargetVariant` has to name one of the variants this endpoint hosts — the same
        // question `InvokeEndpointWithResponseStream` asks, so it is asked once, beside
        // `resolve_endpoint` (`crate::state::check_target_variant`).
        if let Some((outcome, message)) = crate::state::check_target_variant(
            &facts,
            &input.endpoint_name,
            input.target_variant.as_deref(),
        ) {
            return Err(refuse(outcome, message));
        }

        // `EnableExplanations` is "an optional JMESPath expression used to override the
        // EnableExplanations parameter of the ClarifyExplainerConfig API" (the member's
        // model doc), so there has to be an explainer configuration for it to override.
        if input.enable_explanations.is_some() && !facts.has_explainer_config {
            return Err(refuse(
                crate::state::InvocationOutcome::ExplanationsNotConfigured,
                format!(
                    "EnableExplanations is supported only on an endpoint whose endpoint \
                     configuration carries an ExplainerConfig; endpoint {} carries none.",
                    input.endpoint_name
                ),
            ));
        }

        // A stateful session is created BY THE MODEL, and no container runs here, so
        // `NEW_SESSION` falls through to the inference step below and every other ID names
        // a session that does not exist — the shared rule in
        // `crate::state::check_session_id`, which this operation's member admits
        // `NEW_SESSION` for.
        if let Some((outcome, message)) = crate::state::check_session_id(
            &input.endpoint_name,
            input.session_id.as_deref(),
            crate::state::NewSessionConstant::Admitted,
        ) {
            return Err(refuse(outcome, message));
        }

        // The payload is examined HERE, after everything the request's addressing resolved:
        // the endpoint a request names is resolved from the URI before its body is looked
        // at, so a request naming no hosted endpoint is refused for that whatever it
        // carries. The bound and the sentence are the model's and the framework's
        // (`crate::service::error_envelope::payload_over_maximum`).
        if let Some(message) =
            crate::service::error_envelope::payload_over_maximum(Self::INPUT_SCHEMA, input.body.len())
        {
            return Err(refuse(
                crate::state::InvocationOutcome::PayloadOverMaximum,
                message,
            ));
        }

        // Everything the control plane owns resolved; what is left is the inference itself.
        //
        // An endpoint's work is to answer what the model behind the variant computes for the
        // payload, so that is what happens here: the variant is resolved through the
        // endpoint's own configuration, its model's `ModelDataUrl` is read out of this
        // emulator's Amazon S3, and the prediction is COMPUTED from the parameters a training
        // or AutoML run of this emulator fitted and wrote into that artifact
        // (`sagemaker::state::model_serving`).
        //
        // Where there is nothing to compute from — a variant whose model names no artifact, an
        // artifact that cannot be read, one no run of this emulator produced (a model that
        // would need its container image run, which this emulator does not do), or a payload
        // holding no record the model applies to — NOTHING computed a prediction, and the
        // answer is the modelled error for an invocation the model did not serve, naming the
        // condition that stopped it. Never a synthesised `Body`.
        let region = sagemaker::state::request_region(ctx);
        let variants = crate::state::serving_variants(&*self.engine, &input.endpoint_name);
        // Without `TargetVariant` the real service distributes the request across the
        // endpoint's variants by their weights; the first variant the configuration listed is
        // the one taken here, so one request routes to one deployed model deterministically.
        let serving = match input.target_variant.as_deref() {
            Some(target) => variants
                .iter()
                .find(|variant| variant.variant_name == target),
            None => variants.first(),
        };
        let served = match serving {
            Some(variant) => {
                match sagemaker::state::load_served_model(ctx, &region, variant).await {
                    Ok(model) => sagemaker::state::serve_inference(
                        &model,
                        &input.body,
                        input.accept.as_deref(),
                    )
                    .map(|served| (variant.variant_name.clone(), served)),
                    Err(refusal) => Err(refusal),
                }
            }
            // The endpoint hosts no variant with a model behind it, so there is nothing
            // deployed to route to at all.
            None => Err(sagemaker::state::ServingRefusal::NoModelArtifact {
                model_name: input.endpoint_name.clone(),
            }),
        };
        let (variant_name, answer) = match served {
            Ok(answer) => answer,
            Err(refusal) => {
                record(crate::state::InvocationOutcome::NoInferenceContainer);
                return Err(InvokeEndpointErrorModeled::ModelError(ModelError {
                    message: Some(refusal.message()),
                    // Nothing answered, so there is no container status code, no container
                    // message and no container log stream to report.
                    original_status_code: None,
                    original_message: None,
                    log_stream_arn: None,
                })
                .into());
            }
        };
        record(crate::state::InvocationOutcome::Served);
        Ok(InvokeEndpointOutput {
            body: answer.body,
            content_type: Some(answer.content_type),
            // "Identifies the production variant that was invoked"
            // (`InvokeEndpointOutput$InvokedProductionVariant`): the one this call routed to.
            invoked_production_variant: Some(variant_name),
            // A model sets `CustomAttributes` and closes or opens a stateful session; the
            // model here is a set of fitted parameters rather than a container that could do
            // either, so none of the three is claimed.
            custom_attributes: None,
            new_session_id: None,
            closed_session_id: None,
        })
    }
}
