// SPDX-License-Identifier: CC0-1.0

//! @op CreateFlow
//! Persists a new flow in this service's store ([`crate::state::Flow`]) and answers
//! with the ARN it minted and the status the trigger type implies. `DescribeFlow`
//! reads back exactly what is stored here.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateFlow`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda/smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json")
//! ```
//!
//! Enables your application to create a new flow using Amazon AppFlow. You must create a
//! connector profile before calling this API. Please note that the Request Syntax below
//! shows syntax for multiple destinations, however, you can only transfer data to one item
//! in this list at a time. Amazon AppFlow does not currently support flows to multiple
//! destinations at once.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..256, pattern ^[ -~]+$, @idempotencyToken) — The
//!   clientToken parameter is an idempotency token. It ensures that your CreateFlow request
//!   completes only once. You choose the value to pass. For example, if you don't receive a
//!   response from your request, you can safely retry the request with the same clientToken
//!   parameter value. If you omit a clientToken value, the Amazon Web Services SDK that you
//!   are using inserts a value for you. This way, the SDK can safely retry requests multiple
//!   times after a network error. You must provide your own value for other use cases. If you
//!   specify input parameters that differ from your first request, an error occurs. If you
//!   use a different value for clientToken, Amazon AppFlow considers it a new call to …
//! * `description` (FlowDescription, len 0..2048, pattern ^[\w!@#\-.?,\s]*$) — A description
//!   of the flow you want to create.
//! * `destinationFlowConfigList` (DestinationFlowConfigList) **required** — The configuration
//!   that controls how Amazon AppFlow places data in the destination connector.
//! * `flowName` (FlowName, len 0..256, pattern ^[a-zA-Z0-9][\w!@#.-]+$) **required** — The
//!   specified name of the flow. Spaces are not allowed. Use underscores (_) or hyphens (-)
//!   only.
//! * `kmsArn` (KMSArn, len 20..2048, pattern ^arn:aws:kms:.*:[0-9]+:) — The ARN (Amazon
//!   Resource Name) of the Key Management Service (KMS) key you provide for encryption. This
//!   is required if you do not want to use the Amazon AppFlow-managed KMS key. If you don't
//!   provide anything here, Amazon AppFlow uses the Amazon AppFlow-managed KMS key.
//! * `metadataCatalogConfig` (MetadataCatalogConfig) — Specifies the configuration that
//!   Amazon AppFlow uses when it catalogs the data that's transferred by the associated flow.
//!   When Amazon AppFlow catalogs the data from a flow, it stores metadata in a data catalog.
//! * `sourceFlowConfig` (SourceFlowConfig) **required** — The configuration that controls how
//!   Amazon AppFlow retrieves data from the source connector.
//! * `tags` (TagMap, len 0..50) — The tags used to organize, track, or control access for
//!   your flow.
//! * `tasks` (Tasks) **required** — A list of tasks that Amazon AppFlow performs while
//!   transferring the data in the flow run.
//! * `triggerConfig` (TriggerConfig) **required** — The trigger settings that determine how
//!   and when the flow runs.
//!
//! ## Output
//!
//! * `flowArn` (FlowArn, len 0..512, pattern ^arn:aws:appflow:.*:[0-9]+:) — The flow's Amazon
//!   Resource Name (ARN).
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
// `CreateFlowError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appflow-sandstoneconfigurationservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateFlowError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appflow#CreateFlow")]
#[derive(Debug)]
pub enum CreateFlowErrorModeled {
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
/// (`crate::ops::create_flow::CreateFlowError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateFlowError = crate::scaffolded::Scaffolded<CreateFlowErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateFlowErrorModeled> for CreateFlowError {
    fn from(modeled: CreateFlowErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateFlow` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateFlow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateFlow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateFlow<Eng>
{
    type Input = CreateFlowInput;
    type Output = CreateFlowOutput;
    type Error = CreateFlowError;

    const NAME: &'static str = "CreateFlow";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_FLOW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_FLOW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateFlowErrorModeled::InternalServerException(InternalServerException {
                message: Some("the AppFlow emulator holds no state on this engine".to_string()),
            })
        })?;

        // A schedule is what makes a scheduled flow run, so AppFlow cannot create one
        // without it. Every other modelled constraint was already enforced at the
        // boundary, before this handler ran.
        if crate::state::scheduled_trigger_missing_schedule(&input.trigger_config) {
            return Err(CreateFlowErrorModeled::ValidationException(ValidationException {
                message: Some(
                    "triggerConfig.triggerProperties.scheduled is required when triggerType is Scheduled"
                        .to_string(),
                ),
            })
            .into());
        }

        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx.parts());
        let caller = crate::state::caller_arn(&*self.engine, ctx);
        let tags: ::std::collections::BTreeMap<String, String> =
            input.tags.clone().unwrap_or_default().into_iter().collect();
        let mut store = state.lock();

        // Every connector profile the definition names has to EXIST: "You must create a
        // connector profile before calling this API"
        // (`smithy_query(query="doc", shape="CreateFlow")`), and a profile that was never
        // created is "[t]he resource specified in the request (such as the source or
        // destination connector profile) … not found"
        // (`smithy_query(query="shape", shape="ResourceNotFoundException")`). Checked
        // before anything is stored, so a flow that could never run is not left behind.
        if let Some(missing) = store.unresolved_connector_profile(
            &input.source_flow_config,
            &input.destination_flow_config_list,
        ) {
            return Err(CreateFlowErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::no_such_connector_profile(missing)),
                },
            )
            .into());
        }

        // Every Amazon S3 location the definition names has to be one AppFlow could
        // actually use: the bucket exists and its policy grants AppFlow the actions a run
        // takes on that end of the transfer. Judged before anything is stored, so a flow
        // that could never move data is refused rather than created.
        if let Some(reason) = crate::state::beyond_destination_ceiling(
            &input.destination_flow_config_list,
        )
        .or_else(|| {
            crate::state::unusable_s3_location(
                &*self.engine,
                crate::state::CREATE_FLOW_REQUEST,
                &input.flow_name,
                &crate::state::flow_arn(&region, &input.flow_name),
                &input.source_flow_config,
                &input.destination_flow_config_list,
            )
        }) {
            return Err(CreateFlowErrorModeled::ValidationException(ValidationException {
                message: Some(reason),
            })
            .into());
        }

        // The definition AppFlow STORES, rather than the sparse body it arrived in: the
        // containers AppFlow materialises are filled in, and the flow's encryption key
        // is resolved, before the definition is compared or persisted — so a retry
        // carrying the same body renders the same definition the first call stored.
        let mut trigger_config = input.trigger_config;
        let mut destination_flow_config_list = input.destination_flow_config_list;
        let mut tasks = input.tasks;
        let mut metadata_catalog_config = input.metadata_catalog_config;
        crate::state::materialize_definition_containers(
            &mut trigger_config,
            &mut destination_flow_config_list,
            &mut tasks,
            &mut metadata_catalog_config,
        );
        let kms_arn = match input.kms_arn {
            Some(requested) => requested,
            None => store.managed_kms_key_arn(&region),
        };
        // The definition this request carries, as one comparable value. A definition
        // this service cannot render is a fault in the service, not in the request, so
        // it is reported as one rather than as a rejected input.
        let unrenderable = || {
            CreateFlowErrorModeled::InternalServerException(InternalServerException {
                message: Some(
                    "the AppFlow emulator could not render this flow definition for comparison"
                        .to_string(),
                ),
            })
        };
        let requested_definition = crate::state::definition_descriptor(
            &input.flow_name,
            input.description.as_deref(),
            Some(kms_arn.as_str()),
            &trigger_config,
            &input.source_flow_config,
            &destination_flow_config_list,
            &tasks,
            &tags,
            metadata_catalog_config.as_ref(),
        )
        .ok_or_else(unrenderable)?;

        // A retried CreateFlow carrying the same `@idempotencyToken` answers with the
        // FIRST result, rather than the conflict a second create would raise — unless
        // its INPUT differs from that first request, which the contract calls an error
        // ("If you specify input parameters that differ from your first request, an
        // error occurs"). The whole definition is compared, not just the name, so a
        // retry that would have stored something else is refused instead of being
        // answered 200 with a definition nobody persisted.
        if let Some(flow) = input
            .client_token
            .as_deref()
            .and_then(|token| store.flow_for_client_token(token))
        {
            if flow.definition_descriptor().ok_or_else(unrenderable)? != requested_definition {
                return Err(CreateFlowErrorModeled::ValidationException(ValidationException {
                    message: Some(format!(
                        "clientToken was already used to create the flow {} with a different definition",
                        flow.flow_name
                    )),
                })
                .into());
            }
            return Ok(CreateFlowOutput {
                flow_arn: Some(flow.flow_arn.clone()),
                flow_status: Some(flow.flow_status.as_str().to_string()),
            });
        }

        if store.flows.contains_key(&input.flow_name) {
            return Err(CreateFlowErrorModeled::ConflictException(ConflictException {
                message: Some(crate::state::flow_name_already_exists(&input.flow_name)),
            })
            .into());
        }

        // The account's flow quota, which also bounds the store: a flow past it is
        // refused rather than stored.
        if store.flows.len() >= crate::state::MAX_FLOWS_PER_ACCOUNT {
            return Err(CreateFlowErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "This account already holds the maximum of {} flows",
                        crate::state::MAX_FLOWS_PER_ACCOUNT
                    )),
                },
            )
            .into());
        }

        let flow_name = input.flow_name.clone();
        let flow = crate::state::Flow {
            flow_arn: crate::state::flow_arn(&region, &flow_name),
            flow_name: flow_name.clone(),
            client_token: input.client_token,
            // No `UpdateFlow` has re-defined this flow yet.
            last_update_client_token: None,
            description: input.description,
            kms_arn: Some(kms_arn),
            flow_status: crate::state::initial_flow_status(&trigger_config),
            flow_status_message: None,
            trigger_config,
            source_flow_config: input.source_flow_config,
            destination_flow_config_list,
            tasks,
            tags,
            metadata_catalog_config,
            created_at: now,
            last_updated_at: now,
            created_by: caller.clone(),
            last_updated_by: caller,
            // The first version of this flow's definition.
            schema_version: 1,
            executions: Vec::new(),
        };

        let output = CreateFlowOutput {
            flow_arn: Some(flow.flow_arn.clone()),
            flow_status: Some(flow.flow_status.as_str().to_string()),
        };
        store.flows.insert(flow_name, flow);
        Ok(output)
    }
}
