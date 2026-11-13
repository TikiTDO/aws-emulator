// SPDX-License-Identifier: CC0-1.0

//! @op UpdateFlow
//! Re-defines a flow already in this service's store ([`crate::state::Flow`]) and
//! answers with the status that flow now carries. `DescribeFlow` reads back exactly
//! what is stored here; the flow's identity (its ARN, name, tags, encryption key,
//! creation record and run history) is not part of a re-definition and is left alone.
//! An accepted re-definition also finishes a flow that was still a DRAFT, which
//! activates it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateFlow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Updates an existing flow.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..256, pattern ^[ -~]+$, @idempotencyToken) — The
//!   clientToken parameter is an idempotency token. It ensures that your UpdateFlow request
//!   completes only once. You choose the value to pass. For example, if you don't receive a
//!   response from your request, you can safely retry the request with the same clientToken
//!   parameter value. If you omit a clientToken value, the Amazon Web Services SDK that you
//!   are using inserts a value for you. This way, the SDK can safely retry requests multiple
//!   times after a network error. You must provide your own value for other use cases. If you
//!   specify input parameters that differ from your first request, an error occurs. If you
//!   use a different value for clientToken, Amazon AppFlow considers it a new call to …
//! * `description` (FlowDescription, len 0..2048, pattern ^[\w!@#\-.?,\s]*$) — A description
//!   of the flow.
//! * `destinationFlowConfigList` (DestinationFlowConfigList) **required** — The configuration
//!   that controls how Amazon AppFlow transfers data to the destination connector.
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) **required** — The
//!   specified name of the flow. Spaces are not allowed. Use underscores (_) or hyphens (-)
//!   only.
//! * `metadataCatalogConfig` (MetadataCatalogConfig) — Specifies the configuration that
//!   Amazon AppFlow uses when it catalogs the data that's transferred by the associated flow.
//!   When Amazon AppFlow catalogs the data from a flow, it stores metadata in a data catalog.
//! * `sourceFlowConfig` (SourceFlowConfig) **required** — Contains information about the
//!   configuration of the source connector used in the flow.
//! * `tasks` (Tasks) **required** — A list of tasks that Amazon AppFlow performs while
//!   transferring the data in the flow run.
//! * `triggerConfig` (TriggerConfig) **required** — The trigger settings that determine how
//!   and when the flow runs.
//!
//! ## Output
//!
//! * `flowStatus` (FlowStatus, one of: Active | Deleted | Deprecated | Draft | Errored |
//!   Suspended) — Indicates the current status of the flow.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — AppFlow/Requester has invalid or missing
//!   permissions.
//! * `ConflictException` (HTTP 409) — There was a conflict when processing the request (for
//!   example, a flow with the given name already exists within the account. Check for
//!   conflicting resource names and try again.
//! * `ConnectorAuthenticationException` (HTTP 401) — An error occurred when authenticating
//!   with the connector endpoint.
//! * `ConnectorServerException` (HTTP 400) — An error occurred when retrieving data from the
//!   connector endpoint.
//! * `InternalServerException` (HTTP 500) — An internal service error occurred during the
//!   processing of your request. Try again later.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request (such as
//!   the source or destination connector profile) is not found.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request would cause a service quota
//!   (such as the number of flows) to be exceeded.
//! * `ValidationException` (HTTP 400) — The request has invalid or missing parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateFlowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateFlowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#UpdateFlow")]
#[derive(Debug)]
pub enum UpdateFlowErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    ConnectorAuthenticationException(ConnectorAuthenticationException),
    ConnectorServerException(ConnectorServerException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_flow::UpdateFlowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateFlowError = crate::scaffolded::Scaffolded<UpdateFlowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateFlowErrorModeled> for UpdateFlowError {
    fn from(modeled: UpdateFlowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateFlow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateFlow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateFlow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateFlow<Eng>
{
    type Input = UpdateFlowInput;
    type Output = UpdateFlowOutput;
    type Error = UpdateFlowError;

    const NAME: &'static str = "UpdateFlow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_FLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_FLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        mut input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateFlowErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        // The same rule `CreateFlow` applies to a definition it stores: a scheduled flow
        // runs on its schedule, so it cannot be defined without one. Every other
        // modelled constraint was already enforced at the boundary, before this ran.
        if crate::state::scheduled_trigger_missing_schedule(&input.trigger_config) {
            return Err(UpdateFlowErrorModeled::ValidationException(ValidationException {
                message: Some(
                    "triggerConfig.triggerProperties.scheduled is required when triggerType is Scheduled"
                        .to_string(),
                ),
            })
            .into());
        }

        let now = ::std::time::SystemTime::now();
        let caller = crate::state::caller_arn(&*self.engine, ctx);
        // The same containers `CreateFlow` materialises on a definition it stores, so a
        // re-defined flow is shaped exactly like a created one and a read of it answers
        // with the same members.
        crate::state::materialize_definition_containers(
            &mut input.trigger_config,
            &mut input.destination_flow_config_list,
            &mut input.tasks,
            &mut input.metadata_catalog_config,
        );
        let input = input;
        // A definition this service cannot render is a fault in the service, not in the
        // request, so it is reported as one rather than as a rejected input.
        let unrenderable = || {
            UpdateFlowErrorModeled::InternalServerException(InternalServerException {
                message: Some(
                    "the AppFlow emulator could not render this flow definition for comparison"
                        .to_string(),
                ),
            })
        };

        let mut store = state.lock();

        // The flow being re-defined is resolved first — a request about a flow that does
        // not exist is that absence, whatever else it carries — and then the same rule
        // `CreateFlow` applies to a definition it stores: every connector profile the new
        // definition names has to exist, or the re-defined flow could never run.
        if !store.flows.contains_key(&input.flow_name) {
            return Err(UpdateFlowErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::no_such_flow(&input.flow_name)),
                },
            )
            .into());
        }
        if let Some(missing) = store.unresolved_connector_profile(
            &input.source_flow_config,
            &input.destination_flow_config_list,
        ) {
            return Err(UpdateFlowErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::no_such_connector_profile(missing)),
                },
            )
            .into());
        }
        // And every Amazon S3 location the new definition names has to be one AppFlow
        // could use, by the same rule and through the same resolution `CreateFlow` applies
        // — so a re-definition cannot leave behind a flow that `CreateFlow` would have
        // refused.
        if let Some(reason) = crate::state::beyond_destination_ceiling(
            &input.destination_flow_config_list,
        )
        .or_else(|| {
            crate::state::unusable_s3_location(
                &*self.engine,
                crate::state::UPDATE_FLOW_REQUEST,
                &input.flow_name,
                &crate::state::flow_arn(
                    &crate::state::request_region(ctx.parts()),
                    &input.flow_name,
                ),
                &input.source_flow_config,
                &input.destination_flow_config_list,
            )
        }) {
            return Err(UpdateFlowErrorModeled::ValidationException(ValidationException {
                message: Some(reason),
            })
            .into());
        }
        let flow = store.flows.get_mut(&input.flow_name).ok_or_else(|| {
            UpdateFlowErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::no_such_flow(&input.flow_name)),
            })
        })?;

        // A retried UpdateFlow carrying the same `@idempotencyToken` completes once: the
        // update it already applied is answered again rather than applied a second time,
        // so nothing about the flow — not even when it was last updated — moves. Its
        // INPUT is compared, because the contract makes a token reused with different
        // parameters an error ("If you specify input parameters that differ from your
        // first request, an error occurs"), and the WHOLE definition is compared rather
        // than the name alone, so a retry that would have stored something else is
        // refused instead of answered 200 for a definition nobody persisted. Rendering
        // the definition costs something, so it happens only on this path.
        if flow.last_update_client_token.is_some()
            && flow.last_update_client_token == input.client_token
        {
            // The definition this request would leave in place: the members `UpdateFlow`
            // carries, plus the ones it does not and therefore cannot change (the name,
            // the encryption key, the tags), read from the flow itself.
            let requested_definition = crate::state::definition_descriptor(
                &flow.flow_name,
                input.description.as_deref(),
                flow.kms_arn.as_deref(),
                &input.trigger_config,
                &input.source_flow_config,
                &input.destination_flow_config_list,
                &input.tasks,
                &flow.tags,
                input.metadata_catalog_config.as_ref(),
            )
            .ok_or_else(unrenderable)?;
            if flow.definition_descriptor().ok_or_else(unrenderable)? != requested_definition {
                return Err(UpdateFlowErrorModeled::ValidationException(ValidationException {
                    message: Some(format!(
                        "clientToken was already used to update the flow {} with a different definition",
                        flow.flow_name
                    )),
                })
                .into());
            }
            return Ok(UpdateFlowOutput {
                flow_status: Some(flow.flow_status.as_str().to_string()),
            });
        }

        // `schemaVersion` counts a change to the data schema — the field mappings, the
        // field data types and the partition keys — and nothing else, so the schema is
        // compared BEFORE the new definition is stored, and the version stays where it
        // is when a re-definition touches only the rest of the flow.
        let schema_changed =
            crate::state::schema_descriptor(&input.tasks) != flow.schema_descriptor();

        flow.description = input.description;
        flow.trigger_config = input.trigger_config;
        flow.source_flow_config = input.source_flow_config;
        flow.destination_flow_config_list = input.destination_flow_config_list;
        flow.tasks = input.tasks;
        flow.metadata_catalog_config = input.metadata_catalog_config;
        flow.last_updated_at = now;
        flow.last_updated_by = caller;
        flow.last_update_client_token = input.client_token;
        if schema_changed {
            flow.schema_version += 1;
        }
        // A flow whose definition is still being put together is the DRAFT `CreateFlow`
        // leaves behind; an accepted re-definition is what finishes it, so the flow is
        // then active. This is what the recorded `UpdateFlow` of a draft flow answers.
        // A status the flow's own lifecycle moved it to is NOT reset here: re-defining a
        // suspended or errored flow does not resume or clear it.
        if flow.flow_status == crate::state::FlowStatus::Draft {
            flow.flow_status = crate::state::FlowStatus::Active;
        }

        Ok(UpdateFlowOutput {
            flow_status: Some(flow.flow_status.as_str().to_string()),
        })
    }
}
