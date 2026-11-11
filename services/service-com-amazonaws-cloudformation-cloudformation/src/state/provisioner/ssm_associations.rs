// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of AWS Systems Manager's State Manager: the association an
//! `AWS::SSM::Association` declares. One file of [`super`]'s per-type arms, dispatched to by
//! its `Provisioner`.
//!
//! An association is not a NAMED resource of that service: `CreateAssociation` takes the
//! document the association applies (`Name`), the targets it applies it to and the schedule
//! it applies it on, takes no identifier at all, and answers the `AssociationId` Systems
//! Manager minted — the value this type's `Ref` returns and the handle `DescribeAssociation`,
//! `UpdateAssociation` and `DeleteAssociation` each address one by
//! (`smithy_query(operation="CreateAssociation,DescribeAssociation,UpdateAssociation,\
//! DeleteAssociation", model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/\
//! com-amazonaws-ssm-amazonssm.json")`, where that member is the only `@required` input of
//! the update and carries the UUID pattern Systems Manager refuses anything else against). So
//! the id that create answered is the resource's physical id, replacing the stand-in the
//! naming rules computed — the document `Name` a declaration states names the DOCUMENT, not
//! the association, and no Systems Manager call on an association would accept it.
//!
//! The type publishes no `Tags` property — Systems Manager tags DOCUMENTS and managed nodes
//! rather than the associations between them — so nothing propagates a tag to one.

use super::*;

/// The Systems Manager operations this module asks for, by name: the one call that creates a
/// State Manager association, the one that applies a changed declaration to one, the one that
/// reads one back and the one that removes it.
///
/// Named rather than spelled at each call because the provider REPORTS which of them was
/// refused ([`handler_refusal`]), so the name in the report and the name in the
/// `X-Amz-Target` cannot drift apart.
const CREATE_ASSOCIATION: &str = "CreateAssociation";
const UPDATE_ASSOCIATION: &str = "UpdateAssociation";
const DESCRIBE_ASSOCIATION: &str = "DescribeAssociation";
const DELETE_ASSOCIATION: &str = "DeleteAssociation";

/// The error code Systems Manager answers for an association it does not hold
/// (`AssociationDoesNotExist`: the modelled 404 of its read, its update and its delete).
const ASSOCIATION_DOES_NOT_EXIST: &str = "AssociationDoesNotExist";

/// The member `CreateAssociation` and `UpdateAssociation` answer the created association
/// under, and the member of it that carries the id Systems Manager minted.
const ASSOCIATION_DESCRIPTION: &str = "AssociationDescription";
const ASSOCIATION_ID: &str = "AssociationId";

/// The `AWS::SSM::Association` properties `CreateAssociation` takes under the SAME member
/// name.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(operation="CreateAssociation")` — every property of the type is a member of
/// that request, spelled identically, except `WaitForSuccessTimeoutSeconds`, which is how
/// long CLOUDFORMATION waits for the association to report success and is a member of no
/// Systems Manager call at all.
///
/// The same names are what the association is READ back under: an `AssociationDescription`
/// reports each of them under its own name, which is what the drift read below compares
/// against the declaration.
const ASSOCIATION_MEMBERS: &[&str] = &[
    "ApplyOnlyAtCronInterval",
    "AssociationName",
    "AutomationTargetParameterName",
    "CalendarNames",
    "ComplianceSeverity",
    "DocumentVersion",
    "InstanceId",
    "MaxConcurrency",
    "MaxErrors",
    "Name",
    "OutputLocation",
    "Parameters",
    "ScheduleExpression",
    "ScheduleOffset",
    "SyncCompliance",
    "Targets",
];

/// The same properties `UpdateAssociation` takes, which is every one of the create's except
/// the node a create may pair the association with: `InstanceId` is a member of the create and
/// of NO update ("InstanceId has been deprecated. To specify a managed node ID for an
/// association, use the Targets parameter"), so the node an association was created for is not
/// something an update can change — which is why `template::replacement_properties` names the
/// property for this type.
const UPDATED_ASSOCIATION_MEMBERS: &[&str] = &[
    "ApplyOnlyAtCronInterval",
    "AssociationName",
    "AutomationTargetParameterName",
    "CalendarNames",
    "ComplianceSeverity",
    "DocumentVersion",
    "MaxConcurrency",
    "MaxErrors",
    "Name",
    "OutputLocation",
    "Parameters",
    "ScheduleExpression",
    "ScheduleOffset",
    "SyncCompliance",
    "Targets",
];

/// Create the association an `AWS::SSM::Association` declares, so a client that deployed the
/// stack and then calls `ssm DescribeAssociation` with the id the stack published reads the
/// association back rather than being refused the value as no association id at all.
///
/// The id Systems Manager minted REPLACES the stand-in the naming rules computed: it is what
/// `Ref` and the type's `AssociationId` attribute resolve to, and what every Systems Manager
/// call on the association takes.
pub(super) async fn create_ssm_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let request = association_request(&resource.properties, ASSOCIATION_MEMBERS);
    let answered = call_ssm(ctx, region, CREATE_ASSOCIATION, &Value::Object(request))
        .await
        .map_err(|refusal| handler_refusal(CREATE_ASSOCIATION, refusal))?;
    let association_id = answered_association_id(&answered)?;
    let mut attributes = BTreeMap::new();
    attributes.insert(ASSOCIATION_ID.to_string(), association_id.clone());
    Ok(Created {
        physical_resource_id: Some(association_id),
        attributes,
    })
}

/// Apply a CHANGED declaration to the association the stack created, through
/// `UpdateAssociation` — the one call that changes an association that exists, which takes
/// the id Systems Manager minted together with the new declaration.
///
/// Every member of the create is a member of the update too, including the document `Name`:
/// an association is identified by its id, so a declaration naming another document is a
/// change to the association rather than a different association.
pub(super) async fn update_ssm_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut request = association_request(declared, UPDATED_ASSOCIATION_MEMBERS);
    request.insert(
        ASSOCIATION_ID.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let answered = call_ssm(ctx, region, UPDATE_ASSOCIATION, &Value::Object(request))
        .await
        .map_err(|refusal| handler_refusal(UPDATE_ASSOCIATION, refusal))?;
    // The association keeps the id it was created under — the update answers the same
    // association — so what the resource reports is that id, taken from the answer where it
    // carries one and from the resource itself otherwise.
    let association_id = answered_association_id(&answered)
        .unwrap_or_else(|_| resource.physical_resource_id.clone());
    let mut attributes = BTreeMap::new();
    attributes.insert(ASSOCIATION_ID.to_string(), association_id);
    Ok(Created::attributes(attributes))
}

/// Delete the association a stack created, by the id it was created under — this resource's
/// physical id, which is what `Ref` returns for the type. One Systems Manager no longer holds
/// (`AssociationDoesNotExist`, judged on the CODE and never on a message that carries the
/// request's own values) is already in the state the delete asks for.
pub(super) async fn delete_ssm_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ ASSOCIATION_ID: physical_resource_id });
    match call_ssm(ctx, region, DELETE_ASSOCIATION, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == ASSOCIATION_DOES_NOT_EXIST => Ok(()),
        Err(refusal) => Err(handler_refusal(DELETE_ASSOCIATION, refusal)),
    }
}

/// The association's configuration as Systems Manager reports it, for drift detection: the
/// `AssociationDescription` its own read answers for the id that is this resource's physical
/// id, under the property names the type declares. One it no longer holds
/// (`AssociationDoesNotExist`) was deleted, which is what `None` reports.
pub(super) async fn describe_ssm_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ ASSOCIATION_ID: physical_resource_id });
    match call_ssm(ctx, region, DESCRIBE_ASSOCIATION, &request).await {
        Ok(answer) => {
            // The members an `AssociationDescription` reports under the property names the
            // type declares are the very names the create sends, so one table serves both.
            let reported: Vec<(&str, &str)> = ASSOCIATION_MEMBERS
                .iter()
                .map(|member| (*member, *member))
                .collect();
            Ok(Some(reported_as_properties(
                &reported_member(&answer, ASSOCIATION_DESCRIPTION),
                &reported,
            )))
        }
        Err(refusal) if refusal.code == ASSOCIATION_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The request one declaration describes: the members named identically on both sides
/// ([`ASSOCIATION_MEMBERS`]), each exactly as the declaration resolved it.
///
/// The values are passed through rather than re-rendered as text: every one of these members
/// is a structure, a list, a map, a number or a boolean on the Systems Manager side as well
/// as on the template's, so a value re-spelled as a string would be a value that service
/// refuses. A property declared as `null` states no value at all, so it is left out rather
/// than sent as a null member.
fn association_request(properties: &Value, members: &[&str]) -> Map<String, Value> {
    let mut request = Map::new();
    if let Some(declared) = properties.as_object() {
        for member in members {
            match declared.get(*member) {
                Some(Value::Null) | None => continue,
                Some(value) => request.insert((*member).to_string(), value.clone()),
            };
        }
    }
    request
}

/// The id Systems Manager minted for the association it answered with.
///
/// An answer carrying none would leave no handle a client could call that service with and
/// nothing for the stack to publish, so it is a failed create rather than a created resource.
fn answered_association_id(answered: &Value) -> Result<String, Refusal> {
    let association_id = reported_text(
        &reported_member(answered, ASSOCIATION_DESCRIPTION),
        ASSOCIATION_ID,
    )
    .unwrap_or_default();
    if association_id.is_empty() {
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "AWS Systems Manager answered a created association without an \
                      AssociationId"
                .to_string(),
        });
    }
    Ok(association_id)
}
