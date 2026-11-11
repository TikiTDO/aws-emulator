Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-dry-run-permission-checks-across-mutations
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-dry-run-permission-checks-across-mutations ec2-ebs-volume-delete-and-snapshot-not-found ec2-fast-snapshot-restore-and-public-access-lockdown ec2-page-create-volume-client-token-idempotency ec2-copy-volumes-same-az ec2-describe-volumes-attributes-and-unknown-id-rejection ec2-t12-ebs-volume-attach-detach ec2-catalog-describe-availability-zones ec2-describe-availability-zones-contract ec2-create-vpc-availability-and-delete-lifecycle ec2-local-gateway-vpc-association-route-table-not-found ec2-instance-reboot-and-status-report ec2-instance-monitoring-toggle ec2-launch-template-versioning ec2-elastic-ip-lifecycle ec2-snapshot-inventory-and-attribute-inspection
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVolume|2xx->2xx|$.CreateVolumeResponse.snapshotId
Issue-Key: ec2|CreateVolume|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$.DescribeVolumesResponse.volumeSet.item.operator
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$.DescribeVolumesResponse.volumeSet.item.snapshotId
Issue-Key: ec2|DescribeVolumes|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeAvailabilityZones|2xx->2xx|$.DescribeAvailabilityZonesResponse.availabilityZoneInfo.item[*]
Issue-Key: ec2|DescribeAvailabilityZones|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet.item.dhcpOptionsSource
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.groupSet
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.blockDeviceMapping
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.bootMode
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.capacityReservationSpecification
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.cpuOptions
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.currentInstanceBootMode
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.dnsName
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.enaSupport
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.enclaveOptions
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.groupSet
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.hypervisor
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.maintenanceOptions
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.metadataOptions
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.networkInterfaceSet
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.operator
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.placement.availabilityZoneId
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.placement.groupName
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.placement.tenancy
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.privateDnsName
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.privateDnsNameOptions
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.productCodes
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.rootDeviceName
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.rootDeviceType
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.sourceDestCheck
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.stateReason
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.subnetId
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.virtualizationType
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.vpcId
Issue-Key: ec2|RunInstances|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.groupSet
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.blockDeviceMapping
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.bootMode
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.capacityReservationSpecification
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.cpuOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.currentInstanceBootMode
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.dnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.enaSupport
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.enclaveOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.groupSet
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.hibernationOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.hypervisor
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.ipAddress
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.maintenanceOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.metadataOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkPerformanceOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.operator
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.placement.availabilityZoneId
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.placement.groupName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.placement.tenancy
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.platformDetails
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.privateDnsName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.privateDnsNameOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.productCodes
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.publicIpDnsNameOptions
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.rootDeviceName
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.rootDeviceType
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.secondaryInterfaceSet
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.sourceDestCheck
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.subnetId
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.usageOperation
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.usageOperationUpdateTime
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.virtualizationType
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.vpcId
Issue-Key: ec2|DescribeInstances|2xx->2xx|$headers.content-type
Issue-Key: ec2|CreateLaunchTemplate|2xx->2xx|$.CreateLaunchTemplateResponse.launchTemplate.operator
Issue-Key: ec2|CreateLaunchTemplate|2xx->2xx|$headers.content-type
Issue-Key: ec2|DeleteLaunchTemplate|2xx->2xx|$.DeleteLaunchTemplateResponse.launchTemplate.defaultVersionNumber
Issue-Key: ec2|DeleteLaunchTemplate|2xx->2xx|$.DeleteLaunchTemplateResponse.launchTemplate.latestVersionNumber
Issue-Key: ec2|DeleteLaunchTemplate|2xx->2xx|$.DeleteLaunchTemplateResponse.launchTemplate.operator
Issue-Key: ec2|DeleteLaunchTemplate|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeAddresses|2xx->2xx|$.DescribeAddressesResponse.addressesSet.item.tagSet.item
Issue-Key: ec2|CreateVolume|2xx->2xx|$.CreateVolumeResponse.availabilityZoneId
Issue-Related: 20261023T102725Z-ec2-responses-send-content-type-text-xml-without.md
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 response bodies omit modeled members and report wrong resource values

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVolume

## Description
Ten successful (HTTP 200) responses are serialized with modeled members missing or with wrong values. All ten must be closed before submitting.

Cause 1 of 10: CreateVolume omits snapshotId.
AWS: the response carries <snapshotId/> (empty when the volume was not created from a snapshot). Smithy: CreateVolume output_shape Volume, member SnapshotId xmlName 'snapshotId'.
Emulator: the element is absent entirely. Cluster covers 5 steps across 4 fixtures.
Why: a client that reads snapshotId to decide whether a volume is snapshot-derived gets a null instead of the empty marker, so the branch is undecidable.
Fix: the CreateVolume response serializer.
Traces: ec2-dry-run-permission-checks-across-mutations#setup#1.

Cause 2 of 10: DescribeVolumes omits snapshotId and operator.
AWS: each volumeSet item carries snapshotId (empty when absent) and operator {managed:'false', hiddenByDefault:'false'}. Smithy: Volume members SnapshotId xmlName 'snapshotId' and Operator xmlName 'operator' (target OperatorResponse).
Emulator: both absent. Cluster covers 5 steps.
Fix: the DescribeVolumes response serializer / volume state projection.
Traces: ec2-copy-volumes-same-az#task#4.

Cause 3 of 10: DescribeAvailabilityZones omits three modeled members and maps zone ids to the wrong zone names.
AWS: each availabilityZoneInfo item carries groupLongName ('US West (Oregon) 1'), geographySet ({item:{name:'United States of America'}}) and subGeographySet ({item:{name:'Oregon'}}), and us-west-2a has zoneId usw2-az2 while us-west-2b has usw2-az1. Smithy: AvailabilityZone members GroupLongName xmlName 'groupLongName', Geography xmlName 'geographySet', SubGeography xmlName 'subGeographySet'.
Emulator: those three elements are absent, and zoneId is assigned in name order (us-west-2a -> usw2-az1, us-west-2b -> usw2-az2), i.e. the AZ-name-to-AZ-id mapping is inverted relative to AWS.
Why: AZ ids are the stable, account-independent identifier clients use to pin placement across accounts; an inverted mapping silently places resources in a different physical zone than requested, and the missing geography members break zone-metadata consumers.
Fix: the availability-zone catalog data and the DescribeAvailabilityZones serializer.
Traces: ec2-catalog-describe-availability-zones#task#1 (cluster also covers ec2-describe-availability-zones-contract#task#1).

Cause 4 of 10: DescribeVpcs omits dhcpOptionsSource.
AWS: each vpcSet item carries dhcpOptionsSource 'vpc' (recorded behavior only - the checked-in Smithy Vpc shape has no DhcpOptionsSource member, so this is a model-newer field present in every recording).
Emulator: absent. Cluster covers 2 steps.
Fix: the DescribeVpcs response serializer.
Traces: ec2-create-vpc-availability-and-delete-lifecycle#task#2.

Cause 5 of 10: RunInstances returns a heavily truncated instance.
AWS: the RunInstancesResponse instancesSet item carries groupSet, privateDnsName, dnsName, productCodes, operator, placement.availabilityZoneId/groupName/tenancy, subnetId, vpcId, sourceDestCheck, stateReason {code:'pending', message:'pending'}, rootDeviceType 'ebs', rootDeviceName '/dev/xvda', blockDeviceMapping, virtualizationType 'hvm', hypervisor 'xen', networkInterfaceSet, metadataOptions, cpuOptions, enclaveOptions, maintenanceOptions, bootMode and more.
Emulator: ~28 of those members are absent and groupSet is serialized as the empty string where a list of one group is expected.
Why: a launch client reads subnetId/vpcId/networkInterfaceSet/privateDnsName straight off the launch response; with them null it cannot wire the instance up without re-describing, and even then (cause 6) the data is not there.
Fix: the RunInstances response serializer and the instance state it projects.
Traces: ec2-instance-reboot-and-status-report#setup#1.

Cause 6 of 10: DescribeInstances returns the same truncated instance.
AWS: the reservationSet item carries groupSet and the instancesSet item carries dnsName, productCodes, operator, publicIpDnsNameOptions, placement.availabilityZoneId/groupName/tenancy, subnetId, vpcId, ipAddress, sourceDestCheck, groupSet, rootDeviceType/Name, blockDeviceMapping, networkInterfaceSet, metadataOptions and the rest.
Emulator: ~35 keys diverge - absent, or groupSet serialized as '' instead of a list.
Why: no client can determine an instance's network placement from the emulator.
Fix: the DescribeInstances response serializer and instance state. Cluster covers 2 steps. (privateDnsName also differs, but it is a generated host name and is not itself a requirement.)
Traces: ec2-instance-monitoring-toggle#task#5.

Cause 7 of 10: CreateLaunchTemplate omits launchTemplate.operator.
AWS: operator {managed:'false'}.
Emulator: absent.
Fix: the CreateLaunchTemplate response serializer.
Traces: ec2-launch-template-versioning#setup#1.

Cause 8 of 10: DeleteLaunchTemplate reports stale version numbers.
AWS: after a second version is created, the deleted template reports defaultVersionNumber '2' and latestVersionNumber '2'; it also carries operator.
Emulator: both report '1' and operator is absent - CreateLaunchTemplateVersion did not advance the template's version counters.
Why: a client that reads latestVersionNumber to pick the version to launch launches the wrong one.
Fix: launch-template version bookkeeping in EC2 state, plus the DeleteLaunchTemplate serializer.
Traces: ec2-launch-template-versioning#teardown#9.

Cause 9 of 10: DescribeAddresses collapses a two-tag tagSet to one element.
AWS: tagSet.item is a list of both tags ([{key:'Name',...},{key:'Tier',...}]).
Emulator: a single item object - only the first tag survives, so this is a list-serialization defect, not a formatting difference.
Why: tag-driven selection logic silently misses resources whose matching tag is not the first one.
Fix: tag-list serialization in the DescribeAddresses response path (check whether the same single-element collapse affects other tagSet writers).
Traces: ec2-elastic-ip-lifecycle#task#5.

Cause 10 of 10: CreateVolume reports the wrong availabilityZoneId for the requested zone.
AWS: a volume created in us-west-2b reports availabilityZoneId usw2-az2.
Emulator: usw2-az1 - the same inverted AZ-name-to-AZ-id mapping as cause 3, now on a created resource. The response also omits snapshotId (cause 1).
Why: the volume reports a physical zone it is not in, so placement checks that compare an instance's and a volume's zone id disagree with AWS.
Fix: the shared AZ-name-to-AZ-id resolution used by the CreateVolume handler (same data as cause 3).
Traces: ec2-snapshot-inventory-and-attribute-inspection#setup#1.

## Evidence
Cause 1 - '$.CreateVolumeResponse.snapshotId: missing (expected \'\', got None)' (5 steps). Cause 2 - volumeSet.item.snapshotId missing and volumeSet.item.operator missing (expected {'managed':'false','hiddenByDefault':'false'}) (5 steps). Cause 3 - availabilityZoneInfo.item[0..3] mismatch: expected us-west-2a/usw2-az2 with groupLongName+geographySet+subGeographySet, got us-west-2a/usw2-az1 without them. Cause 4 - vpcSet.item.dhcpOptionsSource missing (expected 'vpc'). Cause 5 - 28 RunInstancesResponse keys diverge, including instancesSet.item.subnetId/vpcId/networkInterfaceSet/stateReason missing and groupSet type_mismatch (expected {'item': {...}}, got ''). Cause 6 - 35 DescribeInstancesResponse keys diverge in the same way. Cause 7 - launchTemplate.operator missing. Cause 8 - defaultVersionNumber expected '2' got '1', latestVersionNumber expected '2' got '1', operator missing. Cause 9 - addressesSet.item.tagSet.item type_mismatch (expected a 2-element list, got a single object). Cause 10 - CreateVolumeResponse.availabilityZoneId expected 'usw2-az2', got 'usw2-az1'.

## Steps to Reproduce
Replay the fixture steps named under Traces. Minimal repros: (1) ec2:CreateVolume AvailabilityZone=us-west-2a Size=8 and look for <snapshotId/>. (2) ec2:DescribeVolumes on that volume. (3) ec2:DescribeAvailabilityZones and compare zoneName/zoneId pairs and the groupLongName/geographySet/subGeographySet elements. (4) ec2:DescribeVpcs. (5) ec2:RunInstances into a subnet. (6) ec2:DescribeInstances on that instance. (7) ec2:CreateLaunchTemplate. (8) ec2:CreateLaunchTemplateVersion then ec2:DeleteLaunchTemplate. (9) ec2:CreateTags with two tags on an Elastic IP then ec2:DescribeAddresses. (10) ec2:CreateVolume AvailabilityZone=us-west-2b and read availabilityZoneId.
