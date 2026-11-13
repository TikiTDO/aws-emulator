// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arm of Amazon Redshift's cluster PARAMETER GROUP: the group an
//! `AWS::Redshift::ClusterParameterGroup` declares. One file of [`super`]'s per-type arms,
//! dispatched to by its `Provisioner`.
//!
//! "Ref returns the parameter group name", and the type states that name itself
//! (`ParameterGroupName`, read through `super::template::declared_name_property`), so the
//! resource's physical id is the name the template asked for and the group is created in
//! Amazon Redshift under exactly it. That matters beyond the output: Amazon Redshift resolves
//! a cluster's `ClusterParameterGroupName` against the groups it HOLDS before it will create
//! a cluster configured from one (`ClusterParameterGroupNotFoundFault`), so a stack that
//! declares both has to create this one for real too — the position
//! `AWS::Redshift::ClusterSubnetGroup` is in.
//!
//! The type's remaining properties are `CreateClusterParameterGroup`'s own members
//! ([`CREATE_CLUSTER_PARAMETER_GROUP_MEMBERS`]), with one exception: its `Parameters` is the
//! `Parameter` LIST `ModifyClusterParameterGroup` takes, which no create member carries, so
//! the declared settings are a SECOND call ([`modify_cluster_parameters`]) — the same pair a
//! client makes. That is why this type does not share the Amazon RDS parameter-group arm
//! (`super::ParameterGroupCalls`), whose types declare their settings as a MAP and whose
//! calls are that service's own.

use super::*;

/// `AWS::Redshift::ClusterParameterGroup`: the parameter group a provisioned Redshift
/// cluster's `ClusterParameterGroupName` names, which `CreateClusterParameterGroup` creates,
/// `ModifyClusterParameterGroup` writes the settings of and `DeleteClusterParameterGroup`
/// removes.
pub(crate) const REDSHIFT_CLUSTER_PARAMETER_GROUP: &str =
    "AWS::Redshift::ClusterParameterGroup";

/// The error code Amazon Redshift answers for a cluster parameter group it does not hold:
/// `ClusterParameterGroupNotFoundFault`'s own `awsQueryError` code
/// (`smithy_query(query="operation", operation="DeleteClusterParameterGroup")`), which is
/// what an awsQuery client branches on.
const PARAMETER_GROUP_NOT_FOUND: &str = "ClusterParameterGroupNotFound";

/// The members `CreateClusterParameterGroup` takes, from Amazon Redshift's own model
/// (`smithy_query(query="operation", operation="CreateClusterParameterGroup")`), with the tag
/// list's element name from its list shape's member `@xmlName`
/// (`smithy_query(query="shape", shape="TagList")`).
///
/// This is the catalogue the declaration of the type is walked against: its `Description` and
/// `ParameterGroupFamily` are these members' own names, so a property the catalogue does not
/// carry is refused rather than dropped ([`query_members`]).
const CREATE_CLUSTER_PARAMETER_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("Description", QueryMember::Scalar),
    ("ParameterGroupFamily", QueryMember::Scalar),
    ("ParameterGroupName", QueryMember::Scalar),
    ("Tags", QueryMember::List("Tag")),
];

/// The properties of the type the walk over the declaration does NOT turn into a create
/// member: the name this arm pushes (it is the resource's physical id), the `Parameters` list
/// that travels as the modify below, and the tag list this arm merges with the ones
/// CloudFormation propagates.
const PARAMETER_GROUP_PROPERTIES_NOT_SENT: &[&str] =
    &["ParameterGroupName", "Parameters", "Tags"];

/// The element name `ModifyClusterParameterGroup`'s `Parameters` list carries, from its list
/// shape's member `@xmlName` (`smithy_query(query="shape", shape="ParametersList")`).
const PARAMETER_ELEMENT: &str = "Parameter";

/// How many parameters one call carries: "A maximum of 20 parameters can be modified in a
/// single request" (`ModifyClusterParameterGroupMessage$Parameters`), and the same maximum for
/// the reset (`ResetClusterParameterGroupMessage$Parameters`: "A maximum of 20 parameters can
/// be reset in a single request") — so a declaration carrying more reaches Amazon Redshift as
/// several calls rather than as one request that service would refuse.
///
/// How many calls that is is bounded by the declared list, which is bounded by the template
/// body the stack was created from, and Amazon Redshift's own catalogue of the settings a
/// family has refuses a name that is not one of them — so neither this loop nor the group it
/// writes grows with anything a caller can choose freely.
const PARAMETERS_PER_CALL: usize = 20;

/// Create the parameter group an `AWS::Redshift::ClusterParameterGroup` declares, under the
/// name the declaration states — this resource's physical id, which is what its `Ref`
/// returns.
///
/// The declared `Parameters` are the second call ([`modify_cluster_parameters`]): a group is
/// created carrying none of its own settings, and the values a template declares are what
/// `DescribeClusterParameters` then reports for it. The type publishes no `Fn::GetAtt`
/// attribute, so the group's name is the whole of what the stack publishes for it.
pub(crate) async fn create_cluster_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_CLUSTER_PARAMETER_GROUP_MEMBERS,
        // Every property of this type is its member's own name.
        &[],
        PARAMETER_GROUP_PROPERTIES_NOT_SENT,
        REDSHIFT_CLUSTER_PARAMETER_GROUP,
    )?;
    parameters.push((
        "ParameterGroupName".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "CreateClusterParameterGroup",
        &parameters,
    )
    .await?;
    modify_cluster_parameters(
        ctx,
        region,
        &resource.physical_resource_id,
        declared_parameters(&resource.properties),
    )
    .await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Write the parameter settings a declaration carries into the group named
/// `physical_resource_id`, through the modify a client makes for them.
///
/// The type's `Parameters` is already the `Parameter` structure list
/// `ModifyClusterParameterGroup` takes — each entry a `ParameterName` and a `ParameterValue`
/// — so the declared list IS that member and is written entry by entry
/// ([`push_list`]), in batches of [`PARAMETERS_PER_CALL`]. A declaration carrying none makes
/// no call at all: the group then holds no value of its own and every parameter it reports is
/// its family's engine default.
async fn modify_cluster_parameters(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    declared: Vec<Value>,
) -> Result<(), Refusal> {
    for batch in declared.chunks(PARAMETERS_PER_CALL) {
        let mut parameters = vec![(
            "ParameterGroupName".to_string(),
            physical_resource_id.to_string(),
        )];
        push_list(
            &mut parameters,
            "Parameters",
            PARAMETER_ELEMENT,
            &Value::Array(batch.to_vec()),
            0,
        )?;
        call_query(
            ctx,
            REDSHIFT_SIGNING_NAME,
            region,
            REDSHIFT_API_VERSION,
            "ModifyClusterParameterGroup",
            &parameters,
        )
        .await?;
    }
    Ok(())
}

/// The parameter settings a declaration of the type carries: the entries of its `Parameters`
/// list, or none where it declares none. A value the template did not write as a list is the
/// one-entry list of itself, exactly as [`push_list`] reads one.
fn declared_parameters(properties: &Value) -> Vec<Value> {
    match properties.get("Parameters") {
        None => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(single) => vec![single.clone()],
    }
}

/// The part of the declaration that is NOT its parameter settings: everything but the
/// `Parameters` list and the tag list, which travel on their own
/// ([`modify_cluster_parameters`], [`Provisioner::retag`]).
fn parameter_group_configuration(properties: &Value) -> Map<String, Value> {
    properties
        .as_object()
        .map(|properties| {
            properties
                .iter()
                .filter(|(property, _)| !matches!(property.as_str(), "Parameters" | "Tags"))
                .map(|(property, value)| (property.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The parameter names an entry of either revision's `Parameters` list names, in the order
/// the list declares them.
fn parameter_names(declared: &[Value]) -> Vec<String> {
    declared
        .iter()
        .filter_map(|entry| entry.get("ParameterName").and_then(scalar_text))
        .collect()
}

/// Apply a CHANGED declaration of the type to the group the stack already has.
///
/// A group's `Parameters` are the one part of the declaration Amazon Redshift can change on a
/// group that exists, and `ModifyClusterParameterGroup` is the call that changes it — the
/// same call the create makes for the values a template declares — so a deployment that
/// states a parameter differently, or states one more, reaches that service as that call. A
/// parameter the new declaration DROPS is put back to the engine's default through the group's
/// own reset (`ResetClusterParameterGroup`), which is what AWS does for it. The group keeps
/// its name, so it keeps the handle a cluster names it by: this reports no attribute of its
/// own.
///
/// A change to any OTHER property — the group's family or its description, neither of which
/// is a member of the modify at all — is REFUSED rather than quietly dropped: AWS applies such
/// a change by creating a new group, and this emulator creates no replacement, so the stack
/// never reports a group configured differently from its declaration.
pub(crate) async fn update_cluster_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if parameter_group_configuration(&resource.properties)
        != parameter_group_configuration(declared)
    {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not replace an existing {REDSHIFT_CLUSTER_PARAMETER_GROUP} \
                 whose declaration changes a property Amazon Redshift cannot change on a group \
                 that exists; the resource was left as it is."
            ),
        });
    }

    let before = declared_parameters(&resource.properties);
    let after = declared_parameters(declared);
    // Only the settings this deployment states differently are written: a parameter neither
    // revision changed already carries the value the group holds.
    let changed: Vec<Value> = after
        .iter()
        .filter(|entry| !before.contains(entry))
        .cloned()
        .collect();
    modify_cluster_parameters(ctx, region, &resource.physical_resource_id, changed).await?;
    let names_after = parameter_names(&after);
    let dropped: Vec<String> = parameter_names(&before)
        .into_iter()
        .filter(|name| !names_after.contains(name))
        .collect();
    reset_cluster_parameters(ctx, region, &resource.physical_resource_id, &dropped).await?;
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// Put the parameters a deployment no longer declares back to the engine's default, through
/// the group's own reset: "Sets one or more parameters of the specified parameter group to
/// their default values", which takes the `Parameter` list naming them, in batches of
/// [`PARAMETERS_PER_CALL`] for the reason that constant records. A deployment that drops none
/// makes no call.
async fn reset_cluster_parameters(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    dropped: &[String],
) -> Result<(), Refusal> {
    for batch in dropped.chunks(PARAMETERS_PER_CALL) {
        let mut parameters = vec![(
            "ParameterGroupName".to_string(),
            physical_resource_id.to_string(),
        )];
        for (index, name) in batch.iter().enumerate() {
            parameters.push((
                format!("Parameters.{PARAMETER_ELEMENT}.{}.ParameterName", index + 1),
                name.clone(),
            ));
        }
        call_query(
            ctx,
            REDSHIFT_SIGNING_NAME,
            region,
            REDSHIFT_API_VERSION,
            "ResetClusterParameterGroup",
            &parameters,
        )
        .await?;
    }
    Ok(())
}

/// Delete the parameter group a stack created, addressed by its name — this resource's
/// physical id. A group Amazon Redshift no longer has (`ClusterParameterGroupNotFound`) is
/// already in the state the delete asks for.
///
/// Amazon Redshift refuses the delete of a group a cluster is still associated with, so that
/// cluster's deletion comes first: CloudFormation deletes a stack's resources in dependency
/// order, and a cluster's `ClusterParameterGroupName` is what makes this group its
/// dependency.
pub(crate) async fn delete_cluster_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "ParameterGroupName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "DeleteClusterParameterGroup",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == PARAMETER_GROUP_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The parameter group's configuration as Amazon Redshift reports it, for drift detection. A
/// group it no longer has (`ClusterParameterGroupNotFound`) was deleted.
///
/// Its scalar members are read the way every other awsQuery record's are, which covers the two
/// properties a declaration can drift in under the same name (`Description`,
/// `ParameterGroupFamily`); `Parameters` is no member of this record at all, so it takes no
/// part in the comparison — the position every arm whose settings are read by another call is
/// in, and what [`super::drift`] documents.
pub(crate) async fn describe_cluster_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "ParameterGroupName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "DescribeClusterParameterGroups",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered,
            "ClusterParameterGroup",
        )))),
        Err(refusal) if refusal.code == PARAMETER_GROUP_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}
