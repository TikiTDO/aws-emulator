// SPDX-License-Identifier: CC0-1.0

//! The Amazon EC2 instance a stack declares, LAUNCHED in Amazon EC2 itself.
//!
//! ## The rule this module carries
//!
//! A declared resource of a type whose owning service this emulator serves is created by that
//! service's own create call, so the call's REFUSAL is the resource's failure and the
//! identifier the call answers with is what the stack publishes. `AWS::EC2::Instance` is
//! created by `RunInstances` — "Ref returns the ID of the instance", the `i-…` identifier that
//! service assigns — so a declaration Amazon EC2 will not launch (an `ImageId` the account
//! cannot see, a subnet it does not hold, an `InstanceType` the service does not publish)
//! fails THAT resource with Amazon EC2's own refusal as its `ResourceStatusReason`, the
//! resources declared after it are never asked for, and the stack rolls back with no instance
//! left behind. A resource only RECORDED would publish an identifier
//! `DescribeInstances` reports nothing for, and a template that cannot work in Amazon EC2
//! would deploy as though it could.
//!
//! This type lives in its own module beside the algorithm that drives it for the reason
//! [`super::ec2_vpc_network`] records: the per-type catalogue of
//! [`super::super::provisioner`] is far past the length a change to this repository may
//! touch, so a type added now brings its own module. The ec2Query DECLARATION READERS it is
//! built from ([`scalar`], [`list`], [`required`], [`refuse_unmapped_properties`]) and the one
//! call that tags an Amazon EC2 resource ([`tag_resource`]) are that module's, shared rather
//! than copied: both modules write the same wire for the same service. The seam that reaches
//! this one is [`super::Owner`].
//!
//! ## What the type's properties are
//!
//! Source: AWS's CloudFormation resource reference for `AWS::EC2::Instance` read against
//! `RunInstances`'s own member list
//! (`smithy_query(query="shape",shape="RunInstancesRequest")`). The properties below are that
//! call's own members — most of them under the same name, the placement properties inside its
//! `Placement` structure, the monitoring switch inside its `Monitoring` one and the instance
//! profile inside its `IamInstanceProfile` one — and `MinCount`/`MaxCount` are
//! CloudFormation's to decide, a declaration of this type being exactly ONE instance.
//!
//! The tag set reaches Amazon EC2 through the one call that tags a resource that exists, for
//! the reason [`tag_resource`] records: it carries the three `aws:cloudformation:*` keys, and
//! `CreateTags` is the request that service admits them from.
//!
//! A declared property this module does not apply is REFUSED
//! ([`refuse_unmapped_properties`]), so an instance is never reported created with less than
//! its declaration asked for: a block device mapping, a network interface specification, CPU
//! options, a credit specification, enclave or hibernation options, a license specification, a
//! launch template, an `SsmAssociations` entry, an attached `Volumes` entry, a
//! `SourceDestCheck` switch (a `ModifyInstanceAttribute` this emulator does not serve) and an
//! `Ipv6Addresses` list are each one of those.
//!
//! ## What the stack waits for
//!
//! `RunInstances` answers the instance `pending` — nothing runs on it in that state — and
//! Amazon EC2 reports when it reaches `running`, so the stack holds the resource's creation
//! open until it does ([`readiness`]) and holds its deletion open until that service reports
//! it `terminated` ([`removal`]). The addresses and DNS names the type publishes are not
//! assigned until the instance is up, which is why they are read again once it is
//! ([`ready_attributes`]); until Amazon EC2 reports one, the attribute stays the EMPTY value
//! the naming rules gave it, which is what AWS answers for an address an instance has none of.

use std::collections::BTreeMap;

use serde_json::Value;

use super::ec2_vpc_network::{
    list, refuse_unmapped_properties, required, scalar, tag_resource, EC2_API_VERSION,
    EC2_SIGNING_NAME, NOT_FOUND_SUFFIX,
};
use super::{call_query, xml_scalar_children, Created, Readiness, Refusal, Removal};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(super) const EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// Every property `AWS::EC2::Instance` publishes that this module applies, from AWS's
/// CloudFormation resource reference for the type read against `RunInstances`'s member list.
///
/// A property outside this list is refused rather than dropped
/// ([`refuse_unmapped_properties`]).
const INSTANCE_PROPERTIES: &[&str] = &[
    "AdditionalInfo",
    "Affinity",
    "AvailabilityZone",
    "DisableApiTermination",
    "EbsOptimized",
    "HostId",
    "HostResourceGroupArn",
    "IamInstanceProfile",
    "ImageId",
    "InstanceInitiatedShutdownBehavior",
    "InstanceType",
    "Ipv6AddressCount",
    "KernelId",
    "KeyName",
    "Monitoring",
    "PlacementGroupName",
    "PrivateIpAddress",
    "RamdiskId",
    "SecurityGroupIds",
    "SecurityGroups",
    "SubnetId",
    "Tags",
    "Tenancy",
    "UserData",
];

/// The properties that are `RunInstances`'s own SCALAR members under the SAME name, each
/// travelling as that member's own ec2Query parameter.
const INSTANCE_SCALAR_MEMBERS: &[&str] = &[
    "AdditionalInfo",
    "DisableApiTermination",
    "EbsOptimized",
    "ImageId",
    "InstanceInitiatedShutdownBehavior",
    "InstanceType",
    "Ipv6AddressCount",
    "KernelId",
    "KeyName",
    "PrivateIpAddress",
    "RamdiskId",
    "SubnetId",
    "UserData",
];

/// The properties that are members of `RunInstances`'s `Placement` structure, each against the
/// member of that shape it becomes (`smithy_query(query="shape",shape="Placement")`): a
/// placement member travels as `Placement.<Member>`.
///
/// `PlacementGroupName` is the one property renamed on the way: the shape's member for the
/// group an instance is in is `GroupName`.
const INSTANCE_PLACEMENT_MEMBERS: &[(&str, &str)] = &[
    ("Affinity", "Affinity"),
    ("AvailabilityZone", "AvailabilityZone"),
    ("HostId", "HostId"),
    ("HostResourceGroupArn", "HostResourceGroupArn"),
    ("PlacementGroupName", "GroupName"),
    ("Tenancy", "Tenancy"),
];

/// The properties that are LIST members of that call, each against the parameter name
/// ec2Query flattens it under — the member's own `xmlName`, numbered from one
/// (`SecurityGroupId.1`), which is how that protocol spells a list.
const INSTANCE_LIST_MEMBERS: &[(&str, &str)] = &[
    ("SecurityGroupIds", "SecurityGroupId"),
    ("SecurityGroups", "SecurityGroup"),
];

/// How many instances a declaration of this type is: ONE. "Ref returns the ID of the
/// instance", singular, so the launch asks for exactly one and the resource IS that instance.
const ONE_INSTANCE: &str = "1";

/// The states Amazon EC2 reports an instance in (`InstanceStateName`): the one it answers a
/// launch in, the one a usable instance is in, and the one a termination has finished in. The
/// `shutting-down` state a termination passes through is every other state a terminated
/// instance can be reported in, so it is read as "not gone yet" rather than named.
const INSTANCE_STATE_PENDING: &str = "pending";
const INSTANCE_STATE_RUNNING: &str = "running";
const INSTANCE_STATE_TERMINATED: &str = "terminated";

/// The `InstanceAttributeName` that names an instance's boot configuration — "the user
/// data", the attribute `DescribeInstanceAttribute` reports it under
/// ([`boot_configuration`]).
const USER_DATA_ATTRIBUTE: &str = "userData";

/// Each `Fn::GetAtt` attribute of this type that Amazon EC2 reports as a DIRECT member of the
/// instance, as the element of an ec2Query answer that carries it.
///
/// Source: the type's "Return values" section read against the `Instance` shape's own wire
/// names (`smithy_query(query="shape",shape="Instance")`) — a public IPv4 address travels as
/// `ipAddress` and a public DNS name as `dnsName`, neither spelled as the property is. The
/// `AvailabilityZone` the type also publishes is NOT one of them: Amazon EC2 reports it inside
/// the instance's `placement`, which [`placement_zone`] reads.
const INSTANCE_ATTRIBUTE_ELEMENTS: &[(&str, &str)] = &[
    ("PrivateDnsName", "privateDnsName"),
    ("PrivateIp", "privateIpAddress"),
    ("PublicDnsName", "dnsName"),
    ("PublicIp", "ipAddress"),
];

/// Launch the instance an `AWS::EC2::Instance` declares, in Amazon EC2 — so a client that
/// deployed the stack and then calls `ec2 describe-instances` with the id the stack published
/// reads the instance back, and a template Amazon EC2 would refuse fails the resource instead
/// of being reported deployed.
///
/// `Err` is that service's own refusal of the launch, which is what the resource fails with.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, INSTANCE_PROPERTIES)?;
    // "An AMI ID is required to launch an instance and must be specified here or in a launch
    // template" (`RunInstancesRequest$ImageId`), and this type's declaration is the only place
    // one can come from here — a launch template is a property this module does not apply.
    required(resource, "ImageId")?;
    let mut parameters: Vec<(String, String)> = vec![
        ("MinCount".to_string(), ONE_INSTANCE.to_string()),
        ("MaxCount".to_string(), ONE_INSTANCE.to_string()),
    ];
    for member in INSTANCE_SCALAR_MEMBERS {
        if let Some(value) = scalar(resource, member) {
            parameters.push(((*member).to_string(), value));
        }
    }
    for (property, member) in INSTANCE_PLACEMENT_MEMBERS {
        if let Some(value) = scalar(resource, property) {
            parameters.push((format!("Placement.{member}"), value));
        }
    }
    for (property, member) in INSTANCE_LIST_MEMBERS {
        for (index, entry) in list(resource, property).iter().enumerate() {
            parameters.push((format!("{member}.{}", index + 1), entry.clone()));
        }
    }
    // "Specifies whether detailed monitoring is enabled for the instance" is a STRUCTURE on
    // that call (`RunInstancesMonitoringEnabled$Enabled`) where the type states it as the one
    // boolean it is, so the declared value travels as that structure's own member.
    if let Some(enabled) = scalar(resource, "Monitoring") {
        parameters.push(("Monitoring.Enabled".to_string(), enabled));
    }
    // "The name of an IAM instance profile" is this type's property, and that call takes the
    // profile as a structure naming it either way (`IamInstanceProfileSpecification$Name`), so
    // the declared name travels as that member.
    if let Some(profile) = scalar(resource, "IamInstanceProfile") {
        parameters.push(("IamInstanceProfile.Name".to_string(), profile));
    }
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "RunInstances",
        &parameters,
    )
    .await?;
    // The launch answers a RESERVATION holding the instances it placed, so the instance is the
    // first entry of that answer's own `instancesSet` — this launch asked for one.
    let reported = instance_entry(&answered);
    let instance_id = reported
        .get("instanceId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: "InvalidRequest".to_string(),
            message: "RunInstances answered no instanceId".to_string(),
        })?
        .to_string();
    tag_resource(
        ctx,
        region,
        &instance_id,
        &tags_of(&resource.properties, propagated),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(instance_id),
        attributes: published_attributes(&answered),
    })
}

/// Terminate the instance a stack launched, addressed by the id Amazon EC2 minted for it —
/// this resource's physical id, which is what `Ref` returns for the type.
///
/// An instance that service no longer holds is already in the state the delete asks for
/// ([`NOT_FOUND_SUFFIX`], which `InvalidInstanceID.NotFound` carries), and one a termination
/// has already been asked for is answered by that service as the termination already under
/// way.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "TerminateInstances",
        &[(
            "InstanceId.1".to_string(),
            resource.physical_resource_id.clone(),
        )],
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code.ends_with(NOT_FOUND_SUFFIX) => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether Amazon EC2 reports the launched instance ready for a client to use.
///
/// `RunInstances` answers the instance [`INSTANCE_STATE_PENDING`], in which nothing runs on
/// it, and that service reports when it reaches [`INSTANCE_STATE_RUNNING`] — so the stack holds
/// the resource's creation open until it does, and a client that reads the stack as complete
/// finds an instance it can really use. A state no wait brings a NEW instance out of (either
/// state of a termination, a stopped instance, or an instance that service holds none of)
/// fails the resource with what that service reports rather than waiting the window out.
pub(super) async fn readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(described) = describe(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Amazon EC2 holds no instance for {physical_resource_id}"
        )));
    };
    match instance_state(&described).as_deref() {
        Some(INSTANCE_STATE_RUNNING) => Ok(Readiness::Ready),
        Some(INSTANCE_STATE_PENDING) => Ok(Readiness::Pending(INSTANCE_STATE_PENDING.to_string())),
        Some(other) => Ok(Readiness::Failed(format!(
            "Amazon EC2 reports instance {physical_resource_id} in state {other}"
        ))),
        None => Ok(Readiness::Failed(format!(
            "Amazon EC2 reports no state for instance {physical_resource_id}"
        ))),
    }
}

/// The attributes Amazon EC2 publishes for an instance it NOW reports running: its addresses
/// and DNS names, which "[are] not available until the instance enters the `running` state"
/// (`Instance$PrivateDnsName`, `$PublicDnsName`), and the zone it was placed in.
///
/// Read once the wait above comes through, so an `Fn::GetAtt` on one of them resolves to the
/// value that service really assigned rather than to the stand-in a reference to a resource
/// that did not exist yet was resolved against.
pub(super) async fn ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    match describe(ctx, region, physical_resource_id).await? {
        Some(described) => Ok(published_attributes(&described)),
        None => Ok(BTreeMap::new()),
    }
}

/// Whether Amazon EC2 has finished terminating a deleted instance.
///
/// `TerminateInstances` moves the instance into `shutting-down` and that service reports
/// [`INSTANCE_STATE_TERMINATED`] when it has finished with it, so the stack holds the
/// resource's deletion open until it is gone there — and a client that reads the deletion as
/// complete finds nothing still being torn down. An instance that service no longer reports at
/// all is gone.
pub(super) async fn removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    let Some(described) = describe(ctx, region, physical_resource_id).await? else {
        return Ok(Removal::Gone);
    };
    match instance_state(&described) {
        None => Ok(Removal::Gone),
        Some(state) if state == INSTANCE_STATE_TERMINATED => Ok(Removal::Gone),
        Some(state) => Ok(Removal::Pending(state)),
    }
}

/// What Amazon EC2 reports about the instance `instance_id`, or `None` for an instance it does
/// not hold (`InvalidInstanceID.NotFound`, which carries [`NOT_FOUND_SUFFIX`]).
///
/// The ONE read behind [`readiness`], [`ready_attributes`] and [`removal`], so all three ask
/// that service the same question and read its answer the same way.
async fn describe(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_id: &str,
) -> Result<Option<String>, Refusal> {
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeInstances",
        &[("InstanceId.1".to_string(), instance_id.to_string())],
    )
    .await
    {
        Ok(answered) => Ok(Some(answered)),
        Err(refusal) if refusal.code.ends_with(NOT_FOUND_SUFFIX) => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The instance an answer carries, as the scalar members Amazon EC2 reports for it.
///
/// Read out of the `instancesSet` the answer nests its instances in, so an element of the same
/// name elsewhere in the document cannot be mistaken for one of this instance's members. Every
/// call through here names ONE instance, so that set holds one entry.
fn instance_entry(document: &str) -> ::serde_json::Map<String, Value> {
    let entries = document
        .find("<instancesSet>")
        .map_or(document, |start| &document[start..]);
    xml_scalar_children(entries, "item")
}

/// The BOOT CONFIGURATION Amazon EC2 holds for one launched instance — "the user data"
/// (`InstanceAttribute$UserData`), as that service stored it — or `None` for an instance
/// launched without any, and for one Amazon EC2 holds nothing for.
///
/// `DescribeInstanceAttribute` reports the attribute asked about as
/// `<userData><value>…</value></userData>`, carrying no value for an instance that has none;
/// the text is the base64 the launch supplied, which is the form the API takes it in.
///
/// This is what says what the instance was told to do when it came up, which the signals a
/// resource's `CreationPolicy` waits for are read against
/// ([`super::instance_signals`]): the instance that sends such a signal on AWS is one whose
/// own boot configuration asks it to.
pub(super) async fn boot_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_id: &str,
) -> Result<Option<String>, Refusal> {
    let answered = match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeInstanceAttribute",
        &[
            ("InstanceId".to_string(), instance_id.to_string()),
            ("Attribute".to_string(), USER_DATA_ATTRIBUTE.to_string()),
        ],
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code.ends_with(NOT_FOUND_SUFFIX) => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    Ok(xml_scalar_children(&answered, USER_DATA_ATTRIBUTE)
        .get("value")
        .and_then(Value::as_str)
        .filter(|user_data| !user_data.is_empty())
        .map(str::to_string))
}

/// The state Amazon EC2 reports the described instance in, from the instance's own
/// `instanceState` structure (`Instance$State`, whose `name` is the `InstanceStateName`).
fn instance_state(document: &str) -> Option<String> {
    xml_scalar_children(document, "instanceState")
        .get("name")
        .and_then(Value::as_str)
        .filter(|state| !state.is_empty())
        .map(str::to_string)
}

/// The zone the described instance was placed in, from its own `placement` structure
/// (`Instance$Placement`, whose `availabilityZone` is the zone).
fn placement_zone(document: &str) -> Option<String> {
    xml_scalar_children(document, "placement")
        .get("availabilityZone")
        .and_then(Value::as_str)
        .filter(|zone| !zone.is_empty())
        .map(str::to_string)
}

/// The `Fn::GetAtt` attributes this type publishes, as the answer reports them.
///
/// A value that service has not assigned yet is reported as no attribute rather than as an
/// empty one: an instance that is still coming up has no public address, and the attribute is
/// read again once it is up ([`ready_attributes`]).
fn published_attributes(document: &str) -> BTreeMap<String, String> {
    let reported = instance_entry(document);
    let mut attributes = BTreeMap::new();
    for (attribute, element) in INSTANCE_ATTRIBUTE_ELEMENTS {
        if let Some(value) = reported
            .get(*element)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert((*attribute).to_string(), value.to_string());
        }
    }
    if let Some(zone) = placement_zone(document) {
        attributes.insert("AvailabilityZone".to_string(), zone);
    }
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer document carrying one instance, as Amazon EC2's ec2Query wire spells it.
    fn answer(state: &str, public_ip: &str) -> String {
        format!(
            "<RunInstancesResponse><reservationId>r-1</reservationId><instancesSet><item>\
             <instanceId>i-0123456789abcdef0</instanceId>\
             <instanceState><code>16</code><name>{state}</name></instanceState>\
             <privateIpAddress>10.0.0.5</privateIpAddress>\
             <privateDnsName>ip-10-0-0-5.ec2.internal</privateDnsName>\
             <ipAddress>{public_ip}</ipAddress>\
             <placement><availabilityZone>us-east-1a</availabilityZone></placement>\
             </item></instancesSet></RunInstancesResponse>"
        )
    }

    /// One row of the table: what the case is, the state Amazon EC2 reports the instance in,
    /// the public address it has assigned (empty for one it has not), and the attributes the
    /// type must then publish.
    type Row<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, &'a str)]);

    /// The instance's identity, its state and the attributes the type publishes are each read
    /// from the member of the answer Amazon EC2 reports them in — including the two whose wire
    /// name is not the property's (`ipAddress`, `dnsName`) — and a value that service has not
    /// assigned is reported as no attribute at all.
    #[test]
    fn the_published_attributes_are_the_members_amazon_ec2_reports() {
        let rows: &[Row<'_>] = &[
            (
                "an instance that is up, with a public address",
                "running",
                "203.0.113.7",
                &[
                    ("AvailabilityZone", "us-east-1a"),
                    ("PrivateDnsName", "ip-10-0-0-5.ec2.internal"),
                    ("PrivateIp", "10.0.0.5"),
                    ("PublicIp", "203.0.113.7"),
                ],
            ),
            (
                "an instance with no public address assigned",
                "pending",
                "",
                &[
                    ("AvailabilityZone", "us-east-1a"),
                    ("PrivateDnsName", "ip-10-0-0-5.ec2.internal"),
                    ("PrivateIp", "10.0.0.5"),
                ],
            ),
        ];
        for (case, state, public_ip, expected) in rows {
            let document = answer(state, public_ip);
            let read = instance_entry(&document)
                .get("instanceId")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                read.as_deref(),
                Some("i-0123456789abcdef0"),
                "case {case:?}: expected the answer's own instanceId, got {read:?}"
            );
            let reported_state = instance_state(&document);
            assert_eq!(
                reported_state.as_deref(),
                Some(*state),
                "case {case:?}: expected state {state:?}, got {reported_state:?}"
            );
            let attributes = published_attributes(&document);
            let expected: BTreeMap<String, String> = expected
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                attributes, expected,
                "case {case:?}: expected {expected:?}, got {attributes:?}"
            );
        }
    }
}
