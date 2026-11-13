// SPDX-License-Identifier: CC0-1.0

//! @op AssociateOpsItemRelatedItem
//! Associates a related-item resource with an OpsItem: it mints the association's id and
//! persists the association in [`crate::state::ops_item_related_items`], which is what
//! `ListOpsItemRelatedItems` reports. The rules it is held to that the model cannot state
//! — the association and resource types OpsCenter supports, and one resource ARN being a
//! related item of an OpsItem at most once — live beside that store.
//!
//!
//! ---
//!
//! # Modeled contract for `AssociateOpsItemRelatedItem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Associates a related item to a Systems Manager OpsCenter OpsItem. For example, you can
//! associate an Incident Manager incident or analysis with an OpsItem. Incident Manager and
//! OpsCenter are tools in Amazon Web Services Systems Manager.
//!
//! ## Input
//!
//! * `AssociationType` (OpsItemRelatedItemAssociationType) **required** — The type of
//!   association that you want to create between an OpsItem and a resource. OpsCenter
//!   supports IsParentOf and RelatesTo association types.
//! * `OpsItemId` (OpsItemId, pattern ^(oi)-[0-9a-f]{12}$) **required** — The ID of the
//!   OpsItem to which you want to associate a resource as a related item.
//! * `ResourceType` (OpsItemRelatedItemAssociationResourceType) **required** — The type of
//!   resource that you want to associate with an OpsItem. OpsCenter supports the following
//!   types: AWS::SSMIncidents::IncidentRecord: an Incident Manager incident.
//!   AWS::SSM::Document: a Systems Manager (SSM) document.
//! * `ResourceUri` (OpsItemRelatedItemAssociationResourceUri) **required** — The Amazon
//!   Resource Name (ARN) of the Amazon Web Services resource that you want to associate with
//!   the OpsItem.
//!
//! ## Output
//!
//! * `AssociationId` (OpsItemRelatedItemAssociationId) — The association ID.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `OpsItemConflictException` (HTTP 400) — The specified OpsItem is in the process of being
//!   deleted.
//! * `OpsItemInvalidParameterException` (HTTP 400) — A specified parameter argument isn't
//!   valid. Verify the available arguments and try again.
//! * `OpsItemLimitExceededException` (HTTP 400) — The request caused OpsItems to exceed one
//!   or more quotas.
//! * `OpsItemNotFoundException` (HTTP 400) — The specified OpsItem ID doesn't exist. Verify
//!   the ID and try again.
//! * `OpsItemRelatedItemAlreadyExistsException` (HTTP 400) — The Amazon Resource Name (ARN)
//!   is already associated with the OpsItem.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssociateOpsItemRelatedItemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateOpsItemRelatedItemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#AssociateOpsItemRelatedItem")]
#[derive(Debug)]
pub enum AssociateOpsItemRelatedItemErrorModeled {
    InternalServerError(InternalServerError),
    OpsItemConflictException(OpsItemConflictException),
    OpsItemInvalidParameterException(OpsItemInvalidParameterException),
    OpsItemLimitExceededException(OpsItemLimitExceededException),
    OpsItemNotFoundException(OpsItemNotFoundException),
    OpsItemRelatedItemAlreadyExistsException(OpsItemRelatedItemAlreadyExistsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_ops_item_related_item::AssociateOpsItemRelatedItemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateOpsItemRelatedItemError = crate::scaffolded::Scaffolded<AssociateOpsItemRelatedItemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateOpsItemRelatedItemErrorModeled> for AssociateOpsItemRelatedItemError {
    fn from(modeled: AssociateOpsItemRelatedItemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateOpsItemRelatedItem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateOpsItemRelatedItem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateOpsItemRelatedItem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateOpsItemRelatedItem<Eng>
{
    type Input = AssociateOpsItemRelatedItemInput;
    type Output = AssociateOpsItemRelatedItemOutput;
    type Error = AssociateOpsItemRelatedItemError;

    const NAME: &'static str = "AssociateOpsItemRelatedItem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_OPS_ITEM_RELATED_ITEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_OPS_ITEM_RELATED_ITEM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::ops_item_related_items as related_items;

        // `OpsItemId`'s pattern is a MODELLED constraint, so the centralized walk has
        // already refused an id outside it. What is checked here is what the model cannot
        // say: the documented value sets, the OpsItem the association hangs off, the one
        // association per resource ARN, and the store's own bound.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            AssociateOpsItemRelatedItemErrorModeled::InternalServerError(InternalServerError {
                message: Some("the OpsItem store is unavailable".to_string()),
            })
        })?;

        // A type outside the documented sets names an association OpsCenter does not make
        // or a resource it does not relate, so it is refused rather than persisted as an
        // association no read could make sense of.
        let rejection = related_items::association_type_rejection(&input.association_type)
            .or_else(|| related_items::resource_type_rejection(&input.resource_type));
        if let Some(rejection) = rejection {
            return Err(
                AssociateOpsItemRelatedItemErrorModeled::OpsItemInvalidParameterException(
                    OpsItemInvalidParameterException {
                        parameter_names: Some(vec![rejection.parameter]),
                        message: Some(rejection.message),
                    },
                )
                .into(),
            );
        }

        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // A delete that had settled by now was owed whether or not anybody called, so an
        // OpsItem it removed is gone before this association resolves the id.
        store.materialize_due(now);

        // An association hangs off an OpsItem, so there has to BE one: an id this Region
        // holds no OpsItem under is refused the way every operation that cannot resolve an
        // OpsItem id refuses it
        // (`crate::state::ops_items::ops_item_not_found_message`).
        if store.ops_item(&region, &input.ops_item_id).is_none() {
            return Err(
                AssociateOpsItemRelatedItemErrorModeled::OpsItemNotFoundException(
                    OpsItemNotFoundException {
                        message: Some(crate::state::ops_items::ops_item_not_found_message()),
                    },
                )
                .into(),
            );
        }

        // One resource ARN is a related item of an OpsItem at most once: a second
        // association of the same ARN is the collision
        // `OpsItemRelatedItemAlreadyExistsException` reports. Systems Manager words that
        // refusal as the member at fault followed by the frame it appends to a refusal of
        // this operation, and the exception's own members carry the ARN and the OpsItem it
        // collided on.
        if store
            .related_item_for_resource(&region, &input.ops_item_id, &input.resource_uri)
            .is_some()
        {
            return Err(
                AssociateOpsItemRelatedItemErrorModeled::OpsItemRelatedItemAlreadyExistsException(
                    OpsItemRelatedItemAlreadyExistsException {
                        message: Some(related_items::already_exists_message(
                            // A response of this protocol carries no request id, so the
                            // frame names none.
                            None,
                        )),
                        resource_uri: Some(input.resource_uri.clone()),
                        ops_item_id: Some(input.ops_item_id.clone()),
                    },
                )
                .into(),
            );
        }

        // The emulator's own bound on the store, named as the emulator's: the published
        // OpsCenter quota is not restated here.
        let limit_exceeded = |limit: usize| {
            AssociateOpsItemRelatedItemError::from(
                AssociateOpsItemRelatedItemErrorModeled::OpsItemLimitExceededException(
                    OpsItemLimitExceededException {
                        resource_types: Some(vec![input.resource_type.clone()]),
                        limit: i32::try_from(limit).unwrap_or(i32::MAX),
                        limit_type: Some("RelatedItem".to_string()),
                        message: Some(format!(
                            "This emulator holds at most {limit} OpsItem related items across \
                             every Region"
                        )),
                    },
                ),
            )
        };

        // "The Amazon Resource Name (ARN) of the IAM entity that created the OpsItem
        // event": this association was made by the one principal this emulator's
        // credential speaks for.
        let created_by = ::service_common::caller_principal::ambient_user_arn();
        let record = related_items::RelatedItemRecord {
            association_id: store.next_related_item_association_id(),
            ops_item_id: input.ops_item_id.clone(),
            region,
            resource_type: input.resource_type.clone(),
            association_type: input.association_type.clone(),
            resource_uri: input.resource_uri.clone(),
            created_by: created_by.clone(),
            created_time: now,
            // Nothing has updated this association yet, so its last update IS its create.
            last_modified_by: created_by,
            last_modified_time: now,
        };
        let association_id = record.association_id.clone();
        store.insert_related_item(record).map_err(limit_exceeded)?;

        Ok(AssociateOpsItemRelatedItemOutput {
            association_id: Some(association_id),
        })
    }
}
