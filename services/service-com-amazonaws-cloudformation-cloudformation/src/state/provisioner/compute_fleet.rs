// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the FLEET types a stack launches instances with: the
//! Amazon EC2 LAUNCH TEMPLATE that holds the launch parameters, and the Amazon EC2 Auto
//! Scaling GROUP that launches from one. One file of [`super`]'s per-type arms, dispatched
//! to by its `Provisioner`.
//!
//! ## The rule this file carries
//!
//! A stack reports a resource created only where the service that OWNS the type really
//! holds it, and the identifiers and attributes the stack publishes for it are the ones that
//! service answered. These two types are how a template says "launch instances": the group
//! is the resource `autoscaling:DescribeAutoScalingGroups` answers for and the template the
//! one `ec2:DescribeLaunchTemplates` answers for, so a stack that reached `CREATE_COMPLETE`
//! with either declared is one whose client can address both.
//!
//! The two sides spell the same members differently, and each type is handled as its own API
//! spells it:
//!
//! * `AWS::EC2::LaunchTemplate`'s properties are `CreateLaunchTemplate`'s own members —
//!   `LaunchTemplateName`, `LaunchTemplateData` and `VersionDescription` — but the launch
//!   DATA is a deep structure, and Amazon EC2 speaks `ec2Query`: a structure's members
//!   follow its name (`LaunchTemplateData.ImageId`) and a LIST is flattened under its
//!   member's `xmlName`, indexed from 1 and with no `.member.` indirection
//!   (`LaunchTemplateData.SecurityGroupId.1`). [`push_ec2_member`] is that walk, so a
//!   declaration reaches EC2 as the document it declared rather than as one with its lists
//!   dropped;
//! * `AWS::AutoScaling::AutoScalingGroup`'s properties are `CreateAutoScalingGroup`'s own
//!   members, which is an `awsQuery` call like Amazon RDS's — so the evaluated document is
//!   walked by [`query_members`] — with three exceptions: the group's NAME, which
//!   CloudFormation decides and which is this type's physical id; its `Cooldown`, which that
//!   call takes as `DefaultCooldown`; and its `VPCZoneIdentifier`, which the type declares as
//!   a LIST of subnet ids where the call takes "one or more comma-separated subnet IDs" as
//!   one string.
//!
//! The group's tags are the one tag set here that is not a plain key/value pair: an Auto
//! Scaling group's `Tag` carries `PropagateAtLaunch` as well, "determines whether the tag is
//! added to new instances as they are launched in the group", so a declared tag keeps the
//! value its declaration gave it and a tag no declaration states carries none
//! ([`auto_scaling_group_tags`]).

use super::*;

/// Amazon EC2 Auto Scaling's signing name and the API version a request names beside the
/// action, from its model (`sigv4Name: autoscaling`, service shape `AutoScaling_2011_01_01`,
/// `version: 2011-01-01`), which speaks `awsQuery` — so its calls ride on [`call_query`].
const AUTOSCALING_SIGNING_NAME: &str = "autoscaling";
const AUTOSCALING_API_VERSION: &str = "2011-01-01";

/// The error code Amazon EC2 answers for a launch template it does not hold
/// (`InvalidLaunchTemplateId.NotFound`, that service's own API error code), which is a delete
/// asking for a state EC2 is already in.
const INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND: &str = "InvalidLaunchTemplateId.NotFound";

/// The `AWS::EC2::LaunchTemplate` properties `CreateLaunchTemplate` takes under the SAME
/// member name, and whether each is a scalar or the launch DATA structure.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(query="operation",operation="CreateLaunchTemplate",
/// model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/\
/// com-amazonaws-ec2-amazonec2.json")` — the type publishes `LaunchTemplateData`,
/// `LaunchTemplateName`, `TagSpecifications` and `VersionDescription`, and that call takes
/// each of them under its own name.
const EC2_LAUNCH_TEMPLATE_CREATE_MEMBERS: &[&str] = &["LaunchTemplateData", "VersionDescription"];

/// The `AWS::EC2::LaunchTemplate` properties the arm itself carries: the NAME CloudFormation
/// decides — "if you don't specify a name, CloudFormation generates a unique physical ID and
/// uses that ID for the name" — and the `TagSpecifications` of the TEMPLATE, whose tags reach
/// EC2 through the one call that tags a resource that exists ([`tag_ec2_resource`]), beside
/// the ones CloudFormation propagates.
const EC2_LAUNCH_TEMPLATE_PROPERTIES_NOT_SENT: &[&str] =
    &["LaunchTemplateName", "TagSpecifications"];

/// The `ResourceType` a `TagSpecifications` entry of an `AWS::EC2::LaunchTemplate` names the
/// template itself by: "to tag the launch template, the resource type must be
/// launch-template" (`CreateLaunchTemplate$TagSpecifications`). An entry naming any other
/// resource type tags the resources an instance LAUNCH creates, which the launch data's own
/// `TagSpecifications` carries, so it is not a tag of this resource.
const LAUNCH_TEMPLATE_RESOURCE_TYPE: &str = "launch-template";

/// The element name `ec2Query` flattens one LIST member under: its `xmlName` where the member
/// declares one, and the member's own name otherwise.
///
/// Source: the members of `RequestLaunchTemplateData` and of the structures it nests
/// (`smithy_query(query="shape",shape="RequestLaunchTemplateData")`): a list member carrying
/// an `xmlName` is flattened under THAT name (`SecurityGroupIds` -> `SecurityGroupId.1`,
/// `BlockDeviceMappings` -> `BlockDeviceMapping.1`), which is the protocol's rule, and one
/// carrying none under its own (`Ipv6Addresses.1.Ipv6Address`).
fn ec2_list_element(member: &str) -> &str {
    match member {
        "BlockDeviceMappings" => "BlockDeviceMapping",
        "ElasticGpuSpecifications" => "ElasticGpuSpecification",
        "ElasticInferenceAccelerators" => "ElasticInferenceAccelerator",
        // A network interface's `Groups` is the same `SecurityGroupIdStringList` the launch
        // data's own `SecurityGroupIds` is, under that list's `SecurityGroupId` element name.
        "Groups" | "SecurityGroupIds" => "SecurityGroupId",
        "Ipv4Prefixes" => "Ipv4Prefix",
        "Ipv6Prefixes" => "Ipv6Prefix",
        "LicenseSpecifications" => "LicenseSpecification",
        "NetworkInterfaces" => "NetworkInterface",
        "SecondaryInterfaces" => "SecondaryInterface",
        "SecurityGroups" => "SecurityGroup",
        "TagSpecifications" => "TagSpecification",
        "Tags" => "Tag",
        other => other,
    }
}

/// Write ONE member of an `ec2Query` request: a scalar under `<prefix><member>`, a structure's
/// own members below that name, and a LIST flattened under the element name
/// [`ec2_list_element`] gives it, indexed from 1.
///
/// `prefix` is the path of the structure this member belongs to, ending in `.` (empty at the
/// top level of a request). The walk is bounded by [`MAX_PROPERTY_NESTING`] for the reason
/// that constant records: the document comes from a caller-supplied template, so one nested
/// past the bound is REFUSED rather than walked or quietly truncated.
fn push_ec2_member(
    parameters: &mut Vec<(String, String)>,
    prefix: &str,
    member: &str,
    declared: &Value,
    depth: usize,
) -> Result<(), Refusal> {
    match declared {
        Value::Array(entries) => {
            let element = ec2_list_element(member);
            for (index, entry) in entries.iter().enumerate() {
                let position = index + 1;
                push_ec2_value(
                    parameters,
                    &format!("{prefix}{element}.{position}"),
                    entry,
                    depth + 1,
                )?;
            }
            Ok(())
        }
        value => push_ec2_value(parameters, &format!("{prefix}{member}"), value, depth + 1),
    }
}

/// Write one VALUE of an `ec2Query` request at the path `path`: a structure's members below
/// it ([`push_ec2_member`]) and a scalar as the parameter itself.
fn push_ec2_value(
    parameters: &mut Vec<(String, String)>,
    path: &str,
    declared: &Value,
    depth: usize,
) -> Result<(), Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match declared {
        Value::Object(members) => {
            for (member, value) in members {
                push_ec2_member(parameters, &format!("{path}."), member, value, depth)?;
            }
            Ok(())
        }
        // A list whose entries are themselves lists is no shape any member of these calls
        // declares, so there is no element name to index it under: it is refused rather than
        // flattened into something EC2 would read as a different member.
        Value::Array(_) => Err(too_deeply_nested()),
        scalar => {
            parameters.push((path.to_string(), attribute_text(scalar)));
            Ok(())
        }
    }
}

/// Create the launch template an `AWS::EC2::LaunchTemplate` declares, in Amazon EC2.
///
/// "Ref returns the launch template ID" — an id EC2 ASSIGNS (`lt-` and 17 hexadecimal
/// characters), which is what this answers as the resource's physical id, so the stack's
/// `Ref`, its outputs and the `LaunchTemplate` of an Auto Scaling group of the same template
/// all carry an id EC2 really holds a template for. The two version numbers the type
/// publishes are EC2's as well — the template's latest and default version — so both are read
/// off the create's own answer rather than computed.
pub(super) async fn create_ec2_launch_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = Vec::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if EC2_LAUNCH_TEMPLATE_PROPERTIES_NOT_SENT.contains(&property.as_str()) {
                continue;
            }
            if !EC2_LAUNCH_TEMPLATE_CREATE_MEMBERS.contains(&property.as_str()) {
                return Err(property_not_sent(EC2_LAUNCH_TEMPLATE, property));
            }
            push_ec2_member(&mut parameters, "", property, value, 0)?;
        }
    }
    // `CreateLaunchTemplate$LaunchTemplateName` is required by that call while the type's own
    // `LaunchTemplateName` is optional, so the template is created under the name the stack
    // gave the resource.
    parameters.push(("LaunchTemplateName".to_string(), declared_name(resource)));
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateLaunchTemplate",
        &parameters,
    )
    .await?;
    let reported = ec2_reported(&answered, "launchTemplate");
    let launch_template_id = reported
        .get("LaunchTemplateId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateLaunchTemplate answered no launchTemplateId".to_string(),
        })?
        .to_string();
    tag_ec2_resource(
        ctx,
        region,
        &launch_template_id,
        &launch_template_tags(&resource.properties, propagated),
    )
    .await?;
    // The attributes the type publishes: the id EC2 assigned (which this type's
    // `LaunchTemplateId` return value is) and the two version numbers EC2 reported for the
    // template it created.
    let mut attributes = BTreeMap::new();
    attributes.insert("LaunchTemplateId".to_string(), launch_template_id.clone());
    for attribute in ["DefaultVersionNumber", "LatestVersionNumber"] {
        if let Some(value) = reported
            .get(attribute)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    Ok(Created {
        physical_resource_id: Some(launch_template_id),
        attributes,
    })
}

/// The tags a launch template is given: the ones a `TagSpecifications` entry naming the
/// TEMPLATE itself declares ([`LAUNCH_TEMPLATE_RESOURCE_TYPE`]), beside the ones
/// CloudFormation propagates to every resource it creates.
///
/// Built through [`tags_of`], so the merge of declared and propagated tags is the one every
/// other arm makes: the entries' own tag lists are collected into the `Tags` spelling that
/// helper reads.
fn launch_template_tags(
    properties: &Value,
    propagated: &[(String, String)],
) -> Vec<(String, String)> {
    let mut declared = Vec::new();
    let specifications = properties
        .get("TagSpecifications")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for specification in specifications {
        let names_the_template = specification
            .get("ResourceType")
            .and_then(Value::as_str)
            .is_some_and(|resource_type| resource_type == LAUNCH_TEMPLATE_RESOURCE_TYPE);
        if !names_the_template {
            continue;
        }
        if let Some(tags) = specification.get("Tags") {
            declared.push(tags.clone());
        }
    }
    let collected: Vec<Value> = declared
        .into_iter()
        .flat_map(|tags| match tags {
            Value::Array(entries) => entries,
            single => vec![single],
        })
        .collect();
    tags_of(
        &Value::Object(
            [("Tags".to_string(), Value::Array(collected))]
                .into_iter()
                .collect(),
        ),
        propagated,
    )
}

/// Delete the launch template a stack created, addressed by the id EC2 assigned it — this
/// resource's physical id. A template EC2 no longer holds
/// (`InvalidLaunchTemplateId.NotFound`) is already in the state the delete asks for.
pub(super) async fn delete_ec2_launch_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    launch_template_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "LaunchTemplateId".to_string(),
        launch_template_id.to_string(),
    )];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DeleteLaunchTemplate",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The launch template's configuration as Amazon EC2 reports it, for drift detection. A
/// template it no longer holds (`InvalidLaunchTemplateId.NotFound`) was deleted.
///
/// A launch template is reported as a structure whose scalar members are the template's own
/// identity and version numbers — its launch DATA is a version's, which this read does not
/// carry — so those members are the actual configuration the declared one is compared
/// against, exactly as a DB instance's are ([`describe_db_instance`]).
pub(super) async fn describe_ec2_launch_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    launch_template_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "LaunchTemplateId.1".to_string(),
        launch_template_id.to_string(),
    )];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeLaunchTemplates",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(ec2_reported_entry(
            &answered,
            "launchTemplates",
        )))),
        Err(refusal) if refusal.code == INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The members `CreateAutoScalingGroup` takes, from Amazon EC2 Auto Scaling's own model
/// (`smithy_query(query="operation",operation="CreateAutoScalingGroup",
/// model_path="services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/\
/// smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json")`), with each list's
/// element name from its list shape's member `@xmlName` — none of these lists fixes one, so
/// every one carries the Smithy default, `member`.
///
/// This is the catalogue the declaration of an `AWS::AutoScaling::AutoScalingGroup` is walked
/// against: the type's properties ARE these members, so a property whose name is one of them
/// (or renames to one, [`AUTO_SCALING_GROUP_RENAMED_PROPERTIES`]) is passed through under it.
const CREATE_AUTO_SCALING_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("AvailabilityZoneDistribution", QueryMember::Structure),
    ("AvailabilityZoneIds", QueryMember::List("member")),
    ("AvailabilityZoneImpairmentPolicy", QueryMember::Structure),
    ("AvailabilityZones", QueryMember::List("member")),
    ("CapacityRebalance", QueryMember::Scalar),
    ("CapacityReservationSpecification", QueryMember::Structure),
    ("Context", QueryMember::Scalar),
    ("DefaultCooldown", QueryMember::Scalar),
    ("DefaultInstanceWarmup", QueryMember::Scalar),
    ("DeletionProtection", QueryMember::Scalar),
    ("DesiredCapacity", QueryMember::Scalar),
    ("DesiredCapacityType", QueryMember::Scalar),
    ("HealthCheckGracePeriod", QueryMember::Scalar),
    ("HealthCheckType", QueryMember::Scalar),
    ("InstanceId", QueryMember::Scalar),
    ("InstanceLifecyclePolicy", QueryMember::Structure),
    ("InstanceMaintenancePolicy", QueryMember::Structure),
    ("LaunchConfigurationName", QueryMember::Scalar),
    ("LaunchTemplate", QueryMember::Structure),
    ("LifecycleHookSpecificationList", QueryMember::List("member")),
    ("LoadBalancerNames", QueryMember::List("member")),
    ("MaxInstanceLifetime", QueryMember::Scalar),
    ("MaxSize", QueryMember::Scalar),
    ("MinSize", QueryMember::Scalar),
    ("MixedInstancesPolicy", QueryMember::Structure),
    ("NewInstancesProtectedFromScaleIn", QueryMember::Scalar),
    ("PlacementGroup", QueryMember::Scalar),
    ("ServiceLinkedRoleARN", QueryMember::Scalar),
    ("SkipZonalShiftValidation", QueryMember::Scalar),
    ("TargetGroupARNs", QueryMember::List("member")),
    ("TerminationPolicies", QueryMember::List("member")),
    ("TrafficSources", QueryMember::List("member")),
];

/// The one `AWS::AutoScaling::AutoScalingGroup` property whose `CreateAutoScalingGroup`
/// member is spelled differently: the type's `Cooldown` is that call's `DefaultCooldown`
/// ("the amount of time, in seconds, between one scaling activity ending and another one
/// starting"). Every other property is its member's own name.
const AUTO_SCALING_GROUP_RENAMED_PROPERTIES: &[(&str, &str)] = &[("Cooldown", "DefaultCooldown")];

/// The `AWS::AutoScaling::AutoScalingGroup` properties the walk does NOT turn into a
/// `CreateAutoScalingGroup` member, each for a reason of its own. Every other property the
/// catalogue does not carry is refused rather than dropped ([`query_members`]).
const AUTO_SCALING_GROUP_PROPERTIES_NOT_SENT: &[&str] = &[
    // The name CloudFormation decides, which is this resource's physical id ("Ref returns
    // the name of the Auto Scaling group"): the arm sets the member from that name, so the
    // walk must not also push the declared spelling of it.
    "AutoScalingGroupName",
    // The tag list carries the declared tags AND the ones CloudFormation propagates, with
    // the `PropagateAtLaunch` an Auto Scaling group's tag carries, so the arm builds it out
    // of both ([`auto_scaling_group_tags`]).
    "Tags",
    // A LIST of subnet ids where the call takes them as one comma-separated string, so the
    // arm joins them ([`comma_separated_subnets`]).
    "VPCZoneIdentifier",
];

/// Create the Auto Scaling group an `AWS::AutoScaling::AutoScalingGroup` declares, in Amazon
/// EC2 Auto Scaling, and report the ARN that service built for it.
///
/// "Ref returns the name of the Auto Scaling group", which is the name the declaration states
/// or the one CloudFormation generated — so the group is created under this resource's own
/// physical id and no identifier is taken over. The group's ARN is Auto Scaling's, carrying
/// the resource id IT minted, so it is read back from that service rather than computed: the
/// read is also what makes the create's success a resource that service really holds.
pub(super) async fn create_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let name = declared_name(resource);
    let mut parameters = query_members(
        &resource.properties,
        CREATE_AUTO_SCALING_GROUP_MEMBERS,
        AUTO_SCALING_GROUP_RENAMED_PROPERTIES,
        AUTO_SCALING_GROUP_PROPERTIES_NOT_SENT,
        AUTOSCALING_GROUP,
    )?;
    parameters.push(("AutoScalingGroupName".to_string(), name.clone()));
    if let Some(subnets) = comma_separated_subnets(&resource.properties) {
        parameters.push(("VPCZoneIdentifier".to_string(), subnets));
    }
    parameters.extend(auto_scaling_group_tags(&resource.properties, propagated));
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "CreateAutoScalingGroup",
        &parameters,
    )
    .await?;
    let reported = describe_auto_scaling_group(ctx, region, &name)
        .await?
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!(
                "CreateAutoScalingGroup was accepted but Amazon EC2 Auto Scaling reports no \
                 group named {name}"
            ),
        })?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("AutoScalingGroupARN")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
    {
        attributes.extend(super::template::arn_attributes(
            &resource.resource_type,
            arn,
        ));
    }
    Ok(Created::attributes(attributes))
}

/// The subnets an `AWS::AutoScaling::AutoScalingGroup` declares as its `VPCZoneIdentifier`,
/// as the one comma-separated string `CreateAutoScalingGroup` takes them as.
///
/// The type declares "a list of subnet IDs for a virtual private cloud (VPC) where instances
/// in the Auto Scaling group can be created" while the API member is "one or more
/// comma-separated subnet IDs", so the list is joined. A declaration that spells one subnet
/// without the sequence syntax is the one-entry list of itself, exactly as it is for every
/// other list property ([`push_list`]).
fn comma_separated_subnets(properties: &Value) -> Option<String> {
    let declared = properties.get("VPCZoneIdentifier")?;
    let subnets: Vec<String> = match declared {
        Value::Array(entries) => entries.iter().map(attribute_text).collect(),
        single => vec![attribute_text(single)],
    };
    let joined = subnets.join(",");
    (!joined.is_empty()).then_some(joined)
}

/// The `Tags` member of `CreateAutoScalingGroup`, as the awsQuery parameters that call takes
/// it in.
///
/// The tag set is the one [`tags_of`] builds for every type — the declared tags, in
/// declaration order, with the ones CloudFormation propagates after them — and each entry
/// also carries the `PropagateAtLaunch` its own declaration states, which is a member of this
/// service's `Tag` alone. A tag no declaration states carries none, so Auto Scaling applies
/// that member's own default rather than a value invented here.
fn auto_scaling_group_tags(
    properties: &Value,
    propagated: &[(String, String)],
) -> Vec<(String, String)> {
    let mut parameters = Vec::new();
    for (index, (key, value)) in tags_of(properties, propagated).iter().enumerate() {
        let position = index + 1;
        parameters.push((format!("Tags.member.{position}.Key"), key.clone()));
        parameters.push((format!("Tags.member.{position}.Value"), value.clone()));
        if let Some(propagate) = declared_propagate_at_launch(properties, key) {
            parameters.push((
                format!("Tags.member.{position}.PropagateAtLaunch"),
                propagate,
            ));
        }
    }
    parameters
}

/// The `PropagateAtLaunch` the declaration states for the tag keyed `key`, where it states
/// one: the member of an Auto Scaling group's own `Tag` that "determines whether the tag is
/// added to new instances as they are launched in the group".
fn declared_propagate_at_launch(properties: &Value, key: &str) -> Option<String> {
    let declared = properties.get("Tags")?.as_array()?;
    declared
        .iter()
        .find(|tag| tag.get("Key").and_then(Value::as_str) == Some(key))
        .and_then(|tag| tag.get("PropagateAtLaunch"))
        .map(attribute_text)
}

/// Delete the Auto Scaling group a stack created, addressed by the name it was created under
/// — this resource's physical id.
///
/// A group Auto Scaling no longer holds is already in the state the delete asks for, which is
/// read from that service's own report of the group rather than from the refusal: every
/// absent-group refusal of this API is the one `ValidationError` code, so the code alone
/// cannot tell an absent group from a request it refused for another reason.
///
/// `ForceDelete` is set because CloudFormation deletes the group AND the instances it
/// launched: without it, Auto Scaling refuses the delete of a group that still has instances
/// (`ResourceInUse`), and the stack would be left reporting a resource it could not remove.
pub(super) async fn delete_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<(), Refusal> {
    if describe_auto_scaling_group(ctx, region, name).await?.is_none() {
        return Ok(());
    }
    let parameters = vec![
        ("AutoScalingGroupName".to_string(), name.to_string()),
        ("ForceDelete".to_string(), "true".to_string()),
    ];
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DeleteAutoScalingGroup",
        &parameters,
    )
    .await
    .map(|_| ())
}

/// The members `UpdateAutoScalingGroup` takes, from the same model: that call "updates the
/// configuration for the specified Auto Scaling group", and the members it does NOT carry
/// are the ones a group's configuration cannot be changed through
/// ([`AUTO_SCALING_GROUP_CREATE_ONLY_PROPERTIES`]).
const UPDATE_AUTO_SCALING_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("AvailabilityZoneDistribution", QueryMember::Structure),
    ("AvailabilityZoneIds", QueryMember::List("member")),
    ("AvailabilityZoneImpairmentPolicy", QueryMember::Structure),
    ("AvailabilityZones", QueryMember::List("member")),
    ("CapacityRebalance", QueryMember::Scalar),
    ("CapacityReservationSpecification", QueryMember::Structure),
    ("Context", QueryMember::Scalar),
    ("DefaultCooldown", QueryMember::Scalar),
    ("DefaultInstanceWarmup", QueryMember::Scalar),
    ("DeletionProtection", QueryMember::Scalar),
    ("DesiredCapacity", QueryMember::Scalar),
    ("DesiredCapacityType", QueryMember::Scalar),
    ("HealthCheckGracePeriod", QueryMember::Scalar),
    ("HealthCheckType", QueryMember::Scalar),
    ("InstanceLifecyclePolicy", QueryMember::Structure),
    ("InstanceMaintenancePolicy", QueryMember::Structure),
    ("LaunchConfigurationName", QueryMember::Scalar),
    ("LaunchTemplate", QueryMember::Structure),
    ("MaxInstanceLifetime", QueryMember::Scalar),
    ("MaxSize", QueryMember::Scalar),
    ("MinSize", QueryMember::Scalar),
    ("MixedInstancesPolicy", QueryMember::Structure),
    ("NewInstancesProtectedFromScaleIn", QueryMember::Scalar),
    ("PlacementGroup", QueryMember::Scalar),
    ("ServiceLinkedRoleARN", QueryMember::Scalar),
    ("SkipZonalShiftValidation", QueryMember::Scalar),
    ("TerminationPolicies", QueryMember::List("member")),
];

/// The `AWS::AutoScaling::AutoScalingGroup` properties no member of
/// `UpdateAutoScalingGroup` carries, so a declaration that changes one is asking for a change
/// Amazon EC2 Auto Scaling cannot make to a group that exists: the instance a group was built
/// from, the load balancers and target groups it is attached to (each its own
/// `Attach…`/`Detach…` call), its lifecycle hooks (`PutLifecycleHook`), its traffic sources
/// and its tags (`CreateOrUpdateTags`/`DeleteTags`).
const AUTO_SCALING_GROUP_CREATE_ONLY_PROPERTIES: &[&str] = &[
    "InstanceId",
    "LifecycleHookSpecificationList",
    "LoadBalancerNames",
    "Tags",
    "TargetGroupARNs",
    "TrafficSources",
];

/// Apply a CHANGED `AWS::AutoScaling::AutoScalingGroup` declaration to the group that
/// exists, through `UpdateAutoScalingGroup` — "updates the configuration for the specified
/// Auto Scaling group".
///
/// A declaration that changes a property no member of that call carries is REFUSED
/// ([`AUTO_SCALING_GROUP_CREATE_ONLY_PROPERTIES`]): the group would be left as it is, and a
/// stack reporting the change as applied would be reporting a group it does not have.
pub(super) async fn update_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if let Some(property) = AUTO_SCALING_GROUP_CREATE_ONLY_PROPERTIES
        .iter()
        .find(|property| resource.properties.get(**property) != declared.get(**property))
    {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not change the {property} of an existing {AUTOSCALING_GROUP}, \
                 which no member of UpdateAutoScalingGroup carries; the resource was left as it \
                 is."
            ),
        });
    }
    let mut parameters = query_members(
        declared,
        UPDATE_AUTO_SCALING_GROUP_MEMBERS,
        AUTO_SCALING_GROUP_RENAMED_PROPERTIES,
        AUTO_SCALING_GROUP_PROPERTIES_NOT_SENT,
        AUTOSCALING_GROUP,
    )?;
    parameters.push((
        "AutoScalingGroupName".to_string(),
        resource.physical_resource_id.clone(),
    ));
    if let Some(subnets) = comma_separated_subnets(declared) {
        parameters.push(("VPCZoneIdentifier".to_string(), subnets));
    }
    call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "UpdateAutoScalingGroup",
        &parameters,
    )
    .await?;
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// Apply a CHANGED `AWS::EC2::LaunchTemplate` declaration to the template that exists, by
/// creating a NEW VERSION of it — `CreateLaunchTemplateVersion` "creates a new version of a
/// launch template" from the launch data the call carries — and making that version the
/// template's default one, which is what `ModifyLaunchTemplate` does ("you can specify which
/// version of the launch template to set as the default version").
///
/// That is the change the type publishes: its `LaunchTemplateData` is updatable with no
/// interruption, a launch template being a versioned document, so the template keeps the
/// `lt-` id it was created under and the two version numbers the type publishes move —
/// which is what an Auto Scaling group reading `LatestVersionNumber` launches from next. Both
/// are read off the answer rather than counted here.
///
/// A declaration whose launch DATA and version description are unchanged asks for no new
/// version: only the template's TAGS can have changed, and those reach EC2 through the one
/// call that tags a resource that exists ([`super::Provisioner::retag`]).
pub(super) async fn update_ec2_launch_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let mut parameters = Vec::new();
    for member in EC2_LAUNCH_TEMPLATE_CREATE_MEMBERS {
        if let Some(value) = declared.get(member) {
            push_ec2_member(&mut parameters, "", member, value, 0)?;
        }
    }
    let same_version = EC2_LAUNCH_TEMPLATE_CREATE_MEMBERS
        .iter()
        .all(|member| resource.properties.get(member) == declared.get(member));
    if same_version {
        return Ok(Some(Created::attributes(BTreeMap::new())));
    }
    // The template is addressed by the id EC2 minted for it, which is this resource.
    parameters.push((
        "LaunchTemplateId".to_string(),
        resource.physical_resource_id.clone(),
    ));
    // Each call is held in a box of its own, as every call a walk this deep makes is: what the
    // deployment keeps on the stack for this arm is then its own bookkeeping rather than
    // Amazon EC2's whole request chain ([`provisioning::reconcile`]).
    let answered = Box::pin(call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateLaunchTemplateVersion",
        &parameters,
    ))
    .await?;
    // "The version number" of the version just created
    // (`LaunchTemplateVersion$VersionNumber`), which the template's default version is then
    // set to.
    let version_number = ec2_reported(&answered, "launchTemplateVersion")
        .get("VersionNumber")
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateLaunchTemplateVersion answered no versionNumber".to_string(),
        })?
        .to_string();
    let modified = Box::pin(call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "ModifyLaunchTemplate",
        &[
            (
                "LaunchTemplateId".to_string(),
                resource.physical_resource_id.clone(),
            ),
            // `ModifyLaunchTemplateRequest$DefaultVersion` travels under the `xmlName` the
            // model declares for it (`SetDefaultVersion`).
            ("SetDefaultVersion".to_string(), version_number),
        ],
    ))
    .await?;
    // The two version numbers the type publishes, as the template EC2 now holds reports them.
    let reported = ec2_reported(&modified, "launchTemplate");
    let mut attributes = BTreeMap::new();
    for attribute in ["DefaultVersionNumber", "LatestVersionNumber"] {
        if let Some(value) = reported
            .get(attribute)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    Ok(Some(Created::attributes(attributes)))
}

/// The lifecycle state Amazon EC2 Auto Scaling reports an instance of a group in once that
/// instance is serving: "`InService` — The instance is running and in service"
/// (`Instance$LifecycleState`), which is the state a group's own capacity is counted in.
const LIFECYCLE_IN_SERVICE: &str = "InService";

/// The ids of the Amazon EC2 instances the group `name` reports IN SERVICE right now, in the
/// order that service reports them.
///
/// `DescribeAutoScalingGroups` reports a group's `Instances` as one `<member>` per instance,
/// each carrying that instance's own `InstanceId` and `LifecycleState`, so the list is read
/// through the one reader of a list of STRUCTURES this crate has
/// ([`xml_member_structures`]): each entry is read on its own, so an entry that carries one of
/// the two members and not the other is an entry whose other member is absent, and not a
/// pairing of one instance's id with another's state. The empty list is what a group holding
/// no serving instance reports — and what a name that service holds no group for reports,
/// there being no instance to name for it.
pub(crate) async fn in_service_instance_ids(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Vec<String>, Refusal> {
    let parameters = vec![(
        "AutoScalingGroupNames.member.1".to_string(),
        name.to_string(),
    )];
    let answered = call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DescribeAutoScalingGroups",
        &parameters,
    )
    .await?;
    Ok(xml_member_structures(&answered, "Instances")
        .into_iter()
        .filter(|instance| {
            instance.get("LifecycleState").and_then(Value::as_str) == Some(LIFECYCLE_IN_SERVICE)
        })
        .filter_map(|instance| {
            instance
                .get("InstanceId")
                .and_then(Value::as_str)
                .filter(|instance_id| !instance_id.is_empty())
                .map(str::to_string)
        })
        .collect())
}

/// The Auto Scaling group as that service reports it NOW, or `None` where it reports no such
/// group: the scalar members of the one entry `DescribeAutoScalingGroups` answers for the
/// name, which are named exactly as this type's properties are (`MinSize`, `MaxSize`,
/// `DesiredCapacity`, `HealthCheckType`, `VPCZoneIdentifier`) beside the group's own ARN.
///
/// This is both the read the create confirms itself with and the ACTUAL configuration drift
/// detection compares the declared one against. An awsQuery answer carries each group as a
/// `<member>` of its `AutoScalingGroups` list, and the request names ONE group, so the first
/// such element is that group.
pub(super) async fn describe_auto_scaling_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "AutoScalingGroupNames.member.1".to_string(),
        name.to_string(),
    )];
    let answered = call_query(
        ctx,
        AUTOSCALING_SIGNING_NAME,
        region,
        AUTOSCALING_API_VERSION,
        "DescribeAutoScalingGroups",
        &parameters,
    )
    .await?;
    let reported = xml_scalar_children(&answered, "member");
    Ok(reported
        .get("AutoScalingGroupName")
        .and_then(Value::as_str)
        .is_some_and(|reported_name| reported_name == name)
        .then_some(Value::Object(reported)))
}
