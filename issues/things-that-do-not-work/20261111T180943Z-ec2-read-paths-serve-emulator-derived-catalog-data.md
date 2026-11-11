Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-account-and-instance-type-metadata ec2-catalog-describe-vpc-endpoint-services ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics ec2-page-describe-instances-tag-filtered-two-pages ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter ec2-trajwire-deb-deb-launch-then-attach-device-mistakes ec2-trajwire-lia-state-filter-value-mistakes-and-tagging-route ec2-dvpc-traj-reference-path-instance-in-created-vpc ec2-byvpc-traj-stopped-instance-stays-in-its-vpc ec2-trajwire-lia-instance-id-from-another-region ec2-xrg-traj-same-vpc-grouping-not-peering ec2-xrg-traj-unfiltered-count-includes-terminated ec2-xrg-traj-vpc-filter-name-mistakes ec2-byvpc-traj-terminated-instance-loses-vpc-attribution ec2-byvpc-traj-swapped-text-columns-vpc-and-instance-ids ec2-madl-traj-vpc-subnet-discovery-filters ec2-vpc-attributes-and-account-settings ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup ec2-create-vpc-availability-and-delete-lifecycle ec2-dvpc-traj-default-vpc-id-agrees-across-account-attributes-and-filter ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation ec2-dvpc-traj-dry-run-probe-then-real-call ec2-dvpc-traj-isdefault-value-casing-answers-empty ec2-dvpc-traj-text-output-none-fed-back-as-id ec2-ec2mr-byvpc-vpc-create-round-trips-isdefault-and-cidr-associations ec2-ec2mr-dvpc-created-vpc-reports-isdefault-false-not-absent ec2-local-gateway-vpc-association-route-table-not-found ec2-catalog-describe-availability-zones ec2-snapshot-inventory-and-attribute-inspection ec2-catalog-describe-managed-prefix-lists ec2-catalog-describe-prefix-lists ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.2xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.4xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.medium"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.metal"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="a1.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.freeTierEligible
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.medium"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.medium"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.medium"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.medium"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.medium"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c1.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.2xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.4xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.8xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.8xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.large"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c3.xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceNameSet
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item[*]
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceNameSet.item[*]
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.groupSet
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.currentInstanceBootMode
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.blockDeviceMapping
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.privateDnsName
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet.item.dhcpOptionsSource
Issue-Key: ec2|DescribeAvailabilityZones|2xx->2xx|$.DescribeAvailabilityZonesResponse.availabilityZoneInfo.item[*]
Issue-Key: ec2|CreateVolume|2xx->2xx|$.CreateVolumeResponse.availabilityZoneId
Issue-Key: ec2|DescribeManagedPrefixLists|2xx->2xx|$.DescribeManagedPrefixListsResponse.prefixListSet.item[*]
Issue-Key: ec2|DescribePrefixLists|2xx->2xx|$.DescribePrefixListsResponse.prefixListSet.item[*]
Issue-Key: ec2|GetManagedPrefixListAssociations|4xx->2xx|$.Response
Issue-Key: ec2|GetManagedPrefixListAssociations|4xx->2xx|status
Issue-Key: ec2|ModifyManagedPrefixList|4xx->4xx|$.Response.Errors.Error.Message
Issue-Related: 20261029T215142Z-ec2-describe-projections-omit-modeled-members-and-seed.md
Issue-Related: 20261030T000411Z-ec2-read-paths-serve-emulator-authored-catalog-and.md
Issue-Related: 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md
Issue-Related: 20261030T063325Z-ec2-vpc-verified-access-public-ipv4-pool-and.md
Issue-Related: 20261030T124526Z-ec2-describeinstancetypes-serves-a-synthesized-instance-type-catalog.md
Issue-Related: 20261031T181451Z-ec2-instance-volume-snapshot-and-iam-profile-projections.md
Issue-Related: 20261103T071133Z-ec2-describeinstances-and-launch-template-projections-omit-modeled.md
Issue-Related: 20261109T225653Z-ec2-catalog-pages-r9g-z1d-and-vt1-plus.md
Issue-Commit: 69c900cbbb228bcd9ce31c977c27cf9cdda27129

# ec2 read paths serve emulator-derived catalog data and omit modeled members (10 causes)

> _Note: this issue was identified against the code base at commit `69c900cbbb228bcd9ce31c977c27cf9cdda27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
This bundle carries TEN distinct net-new causes in the ec2 emulator, all of them read/refusal paths that answer HTTP 200 (or no refusal at all) with data the emulator derives itself instead of the data AWS serves. Each cause below must be closed before this issue is submitted; they are numbered 1 of 10 .. 10 of 10 and each names its own fixture and step.

Related open issues that do NOT cover these causes (they are scoped to other DescribeInstanceTypes catalog pages or other operations, and the replay reports these clusters as unowned): 20261109T233303Z-ec2-describeinstancetypes-pages-c9gd-m6gd-and-r9g-z1d.md, 20261110T000608Z-ec2-describeinstancetypes-pages-c9gd-m6gd-and-r5-r8in.md, 20261111T001230Z-ec2-describeinstancetypes-rows-for-m5zn-m6a-and-m6i.md, 20261111T015735Z-ec2-describeinstancetypes-catalog-pages-c9gd-m6gd-and-m6i.md, 20261111T001230Z-ec2-describe-and-create-responses-omit-modeled-members.md.

=== Cause 1 of 10: DescribeInstanceTypes serves size-derived hardware rows instead of the AWS instance-type catalog (a1/c1/c3/c4..c9g pages, and the single-type lookup) ===
AWS: recorded behavior only (the Smithy model types the members but fixes no values). For every instance type AWS returns a fixed, published hardware record: memoryInfo.sizeInMiB, vCpuInfo.defaultVCpus/defaultCores/validCores, instanceStorageInfo.disks[].count/sizeInGB and totalSizeInGB, ebsInfo.ebsOptimizedSupport/encryptionSupport/attachmentLimitType/maximumEbsAttachments and ebsInfo.ebsOptimizedInfo.{baseline,maximum}{BandwidthInMbps,Iops,ThroughputInMBps}, networkInfo.{networkPerformance,maximumNetworkInterfaces,ipv4AddressesPerInterface,ipv6Supported,ipv6AddressesPerInterface,trafficMirroringSupport,connectionTrackingConfiguration,networkCards[].{baselineBandwidthInGbps,peakBandwidthInGbps,networkPerformance,defaultEnaQueueCountPerInterface,maximumNetworkInterfaces}}, processorInfo.sustainedClockSpeedInGhz/supportedArchitectures, placementGroupInfo.supportedStrategies, dedicatedHostsSupported, autoRecoverySupported, nitroEnclavesSupport, nitroTpmSupport, rebootMigrationSupport, freeTierEligible, cpuBurstModel and supportedVirtualizationTypes. The values differ per type and are NOT a function of the size suffix.
Emulator: the rows are computed from the type's size. The recorded corpus shows, per type, dozens of value_mismatch entries plus members the emulator never emits at all (cpuBurstModel, networkInfo.ipv6AddressesPerInterface, networkInfo.connectionTrackingConfiguration, networkCards[].defaultEnaQueueCountPerInterface are 'missing'). Examples from the replay: a1.large ebsInfo.encryptionSupport 'unsupported' (AWS: 'supported'), a1.large networkInfo.ipv6Supported 'false' (AWS: 'true'), c1.medium memoryInfo.sizeInMiB '2048' (AWS: '1740'), c1.medium instanceStorageInfo.totalSizeInGB '25' (AWS: '350'), c3.8xlarge instanceStorageInfo.disks[].count '1' (AWS: '2'), c3.8xlarge ebsInfo.ebsOptimizedSupport 'supported' (AWS: 'unsupported'), ebsInfo.maximumEbsAttachments '27' (AWS: '39'), networkCards[].baselineBandwidthInGbps '25.0' (AWS: '5.0'). This divergence family is the single largest in the sweep: ~170 unowned clusters, essentially every page of the ec2-catalog-describe-instance-types-* fixtures plus the per-type read in ec2-account-and-instance-type-metadata.
Why: a client that sizes a workload, picks an EBS-optimized or IPv6-capable type, reads vCPU/memory for cost or capacity math, or branches on nitroTpmSupport/cpuBurstModel gets a different answer from the emulator than from AWS, and code that reads the omitted members raises KeyError against the emulator only.
Fix: the DescribeInstanceTypes state/catalog source for the service (the instance-type data table the operation reads), not the serializer: the operation must serve the published per-type record and emit every member AWS emits, instead of deriving fields from the size suffix.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#1 (representative), #task#2, #task#3 and the remaining pages of that fixture; ec2-account-and-instance-type-metadata#task#3.

=== Cause 2 of 10: DescribeVpcEndpointServices answers an empty result for a ServiceName-filtered call, and its catalog rows disagree with AWS ===
AWS: Smithy: DescribeVpcEndpointServices input ServiceNames (xmlName ServiceName) + Filters; output serviceNameSet + serviceDetailSet. Recorded behavior: a call naming an existing service (for example aws.api.global.codecatalyst) returns that service in BOTH serviceNameSet and serviceDetailSet, and each serviceDetail carries availabilityZoneIdSet/availabilityZoneSet covering all four us-west-2 zones, a long-form serviceId (vpce-svc-<17 hex>), every privateDnsName AWS publishes for the service (e.g. both access-analyzer.us-west-2.amazonaws.com and access-analyzer.us-west-2.api.aws) and no payerResponsibility member.
Emulator: for the name-filtered calls it answers HTTP 200 with serviceNameSet and serviceDetailSet both serialized as the empty value '' (type_mismatch, expected the single-item set). For the unfiltered catalog read it returns rows whose serviceId is a short form (vpce-svc-b24fccc0 vs vpce-svc-0df7f8ad818c92d25), whose availabilityZoneIdSet/availabilityZoneSet omit the fourth zone (usw2-az4 / us-west-2d), whose privateDnsNameSet holds only one name, and which add payerResponsibility that AWS does not send for these entries.
Why: endpoint-creation flows that first resolve a service by name see 'no such service' from the emulator and cannot proceed; code that reads serviceId or enumerates the service's zones gets an id/zone list AWS would never return.
Fix: the DescribeVpcEndpointServices handler (ServiceNames/Filters matching) and the VPC-endpoint-service catalog it reads.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#2 (name-filtered, empty answer; 17 steps in the cluster), ec2-catalog-describe-vpc-endpoint-services#task#15 (catalog rows).

=== Cause 3 of 10: DescribeInstances omits clientToken and metadataOptions.httpProtocolIpv4 and reports currentInstanceBootMode 'legacy-bios' ===
AWS: Smithy: Instance.ClientToken (xmlName clientToken) is always serialized — empty when the launch carried no client token — and Instance.CurrentInstanceBootMode targets InstanceBootModeValues (values: legacy-bios | uefi). metadataOptions.httpProtocolIpv4 is recorded behavior only (not in the checked-in model version): AWS emits it with value 'enabled' for every instance.
Emulator: clientToken is absent (expected ''), metadataOptions.httpProtocolIpv4 is absent (expected 'enabled'), and currentInstanceBootMode is 'legacy-bios' where AWS reports 'uefi' for the launched type.
Why: a client that reads instance['ClientToken'] or MetadataOptions['HttpProtocolIpv4'] raises KeyError against the emulator only, and boot-mode-sensitive logic (UEFI secure boot, NitroTPM) takes the wrong branch.
Fix: the DescribeInstances serializer/handler for Instance, plus the launch-time instance state that records the boot mode.
Traces: ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics#task#13 (7 steps), ec2-dvpc-traj-reference-path-instance-in-created-vpc#task#9, ec2-byvpc-traj-stopped-instance-stays-in-its-vpc#task#9.

=== Cause 4 of 10: the reservation-level groupSet is populated with the default security group ===
AWS: Smithy: Reservation.Groups (xmlName groupSet) is documented '<p>Not supported.</p>' — for VPC instances AWS always returns it empty and reports security groups on the instance/ENI instead.
Emulator: every DescribeInstances reservation carries groupSet with the VPC default security group (type_mismatch: expected '', got a one-item set with groupId/groupName 'default').
Why: a client that treats reservation.groupSet as authoritative sees groups AWS never reports there, and diffing tools report a spurious membership.
Fix: the DescribeInstances serializer for Reservation — stop emitting reservation-level groups.
Traces: ec2-trajwire-lia-instance-id-from-another-region#task#7 (6 steps).

=== Cause 5 of 10: a terminated instance keeps its block-device mapping, loses privateDnsName, and its state-transition reason is ISO-8601 instead of AWS's wording ===
AWS: recorded behavior only. Once an instance is terminated AWS returns blockDeviceMapping empty (the root volume is deleted on termination) and still serializes privateDnsName as an empty string; stateReason/reason text is 'User initiated (YYYY-MM-DD HH:MM:SS GMT)' — fixed text 'User initiated (' + the transition time formatted as 'YYYY-MM-DD HH:MM:SS GMT' + ')'. Only the time inside the parentheses is a per-run value; the format and the surrounding text are the requirement.
Emulator: the terminated instance still reports blockDeviceMapping with the attached root volume (type_mismatch, expected ''), omits privateDnsName entirely (expected ''), and renders the reason as 'User initiated (2026-11-11T17:57:46.128884Z)' — an ISO-8601 instant instead of AWS's 'YYYY-MM-DD HH:MM:SS GMT' form.
Why: termination-cleanup checks see a volume still attached to a terminated instance, readers of PrivateDnsName raise KeyError, and anything parsing the reason text with AWS's format fails.
Fix: the TerminateInstances state transition (detach/delete the root volume, keep an empty privateDnsName) and the state-reason formatter shared by the instance read paths.
Traces: ec2-byvpc-traj-terminated-instance-loses-vpc-attribution#task#7 (2 steps).

=== Cause 6 of 10: a created VPC stays in state 'pending' and DescribeVpcs omits dhcpOptionsSource ===
AWS: Smithy: Vpc.State targets VpcState (values: pending | available | deleting). A VPC created by CreateVpc is reported 'available' on the immediately following DescribeVpcs. dhcpOptionsSource is recorded behavior only (not in the checked-in model version): AWS emits it with value 'vpc' for a VPC using its own DHCP option set.
Emulator: DescribeVpcs reports state 'pending' and never transitions it to 'available', and never emits dhcpOptionsSource.
Why: any wait-for-available loop against a freshly created VPC never converges on the emulator, and a reader of DhcpOptionsSource raises KeyError.
Fix: the CreateVpc/DescribeVpcs state machine (settle the VPC to 'available') and the DescribeVpcs serializer for Vpc.
Traces: ec2-byvpc-traj-swapped-text-columns-vpc-and-instance-ids#task#6 (6 steps, state+dhcpOptionsSource), ec2-create-vpc-availability-and-delete-lifecycle#task#2 (11 steps, dhcpOptionsSource).

=== Cause 7 of 10: the Availability-Zone name-to-zoneId mapping is wrong, and CreateVolume inherits it ===
AWS: Smithy: AvailabilityZone.ZoneId (xmlName zoneId) and CreateVolumeResponse.availabilityZoneId. Recorded behavior pins the account's mapping for us-west-2: us-west-2a = usw2-az2, us-west-2b = usw2-az1, us-west-2d = usw2-az4. Whatever mapping the emulator adopts, DescribeAvailabilityZones and every operation that echoes a zone id (CreateVolume, and the other zone-id-bearing responses) must agree with it.
Emulator: DescribeAvailabilityZones serves a sequential mapping — us-west-2a = usw2-az1, us-west-2b = usw2-az2 (and the later items shift likewise) — so the a/b (and d) rows carry the wrong zoneId; CreateVolume in us-west-2a then returns availabilityZoneId 'usw2-az1' where the recording has 'usw2-az2'.
Why: a client that resolves a zone id for capacity placement, cross-account zone matching or quota lookup and then launches into it lands in a different physical zone than AWS would, and ids echoed by other operations cannot be joined back to the zone list.
Fix: the Availability-Zone catalog in the service's state (the zoneName->zoneId table) that DescribeAvailabilityZones and CreateVolume both read.
Traces: ec2-catalog-describe-availability-zones#task#1, ec2-snapshot-inventory-and-attribute-inspection#setup#1.

=== Cause 8 of 10: the AWS-managed prefix-list catalog returned by DescribeManagedPrefixLists is not AWS's ===
AWS: Smithy: ManagedPrefixList{prefixListId, prefixListName, addressFamily, state, prefixListArn, ownerId, tagSet}. Recorded behavior: the AWS-owned entries for a region use long-form ids (pl-0068613c321dee54b, pl-00a54069) whose prefixListArn is arn:aws:ec2:<region>:aws:prefix-list/<prefixListId>, and each name is paired with the matching addressFamily (com.amazonaws.<region>.<svc> = IPv4, com.amazonaws.<region>.ipv6.<svc> = IPv6).
Emulator: it serves a different, shorter-id set in a different order and mispairs names with address families — e.g. item[0] is 'com.amazonaws.us-west-2.ipv6.route53-healthchecks'/IPv6/pl-080941ad where AWS has 'com.amazonaws.us-west-2.route53-healthchecks'/IPv4/pl-0068613c321dee54b, and item[2] is '...ipv6.s3' where AWS has '...ipv6.ec2-instance-connect'.
Why: a security-group or route rule that references an AWS-managed prefix list by name resolves to a different id (and even a different address family) on the emulator, so the rule it builds does not match the one AWS would build.
Fix: the managed-prefix-list catalog in the service's state that DescribeManagedPrefixLists reads.
Traces: ec2-catalog-describe-managed-prefix-lists#task#1.

=== Cause 9 of 10: DescribePrefixLists serves placeholder CIDRs and wrong legacy prefix-list ids ===
AWS: Smithy: PrefixList{prefixListId, prefixListName, cidrSet}. Recorded behavior: each legacy prefix list carries the service's real published ranges — several entries per list (e.g. com.amazonaws.us-west-2.s3 carries ten IPv4 ranges such as 52.92.128.0/17 and 52.218.128.0/17) — and the ids/names/order of the recorded set.
Emulator: it returns one synthetic range per list drawn from the documentation/test ranges (198.18.8.0/21, 198.18.96.0/21, 2001:db8:5::/48, 2001:db8:8::/48, 2001:db8:9::/48) under different ids (pl-235f5af3, pl-fffbb61c, pl-47428f6c ...) and in a different order, so both the id/name pairing and every CIDR diverge.
Why: a client that reads a prefix list to build a firewall/route allow-list gets ranges that route nowhere near the real service, and the id it stores does not match the one AWS returns.
Fix: the prefix-list catalog in the service's state that DescribePrefixLists reads.
Traces: ec2-catalog-describe-prefix-lists#task#1.

=== Cause 10 of 10: the AWS-managed prefix-list refusals are missing or emulator-worded ===
AWS: Smithy: GetManagedPrefixListAssociations(PrefixListId required) and ModifyManagedPrefixList(PrefixListId required) model no errors (the EC2 query protocol carries the error in Response.Errors.Error), so the refusal comes from recorded behavior: both operations refuse an AWS-managed prefix list with HTTP 400, code 'UnsupportedOperation' and the fixed message 'The action is not supported for an AWS-managed prefix list.' — the text has no variable part.
Emulator: GetManagedPrefixListAssociations on an AWS-managed prefix list answers HTTP 200 with an association list instead of refusing (expected 400 / UnsupportedOperation, no $.Response error element at all); ModifyManagedPrefixList does refuse with 400 but writes its own message "The prefix list 'pl-e34ce16f' is an Amazon Web Services-managed prefix list, which cannot be modified" — emulator wording that additionally interpolates the prefix-list id AWS does not mention.
Why: a client that probes an AWS-managed list expects to be refused; the emulator's 200 makes it treat the list as customer-managed, and error-text/code matching on the modify path fails.
Fix: the validation of GetManagedPrefixListAssociations (add the AWS-managed refusal) and the message text in ModifyManagedPrefixList's validation.
Traces: ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership#task#10 (missing refusal), ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership#task#9 (wrong message).

## Evidence
Replay of trace set simparity-wire-aws-v1 (429 fixtures, 3941 steps, 2820 passed, 1043 mismatched, score 71.6%). Expected vs actual per cause:
1. DescribeInstanceTypes, HTTP 200 vs 200: a1.large ebsInfo.encryptionSupport expected 'supported', got 'unsupported'; a1.large networkInfo.ipv6Supported expected 'true', got 'false'; c1.medium memoryInfo.sizeInMiB expected '1740', got '2048'; c1.medium instanceStorageInfo.totalSizeInGB expected '350', got '25'; c3.8xlarge instanceStorageInfo.disks.item.count expected '2', got '1'; c3.8xlarge ebsInfo.ebsOptimizedSupport expected 'unsupported', got 'supported'; ebsInfo.maximumEbsAttachments expected '39', got '27'; cpuBurstModel / networkInfo.ipv6AddressesPerInterface / networkInfo.connectionTrackingConfiguration / networkCards.item.defaultEnaQueueCountPerInterface expected present, got None. (~170 unowned clusters across the ec2-catalog-describe-instance-types-* fixtures.)
2. DescribeVpcEndpointServices, 200 vs 200: serviceNameSet expected {'item': 'aws.api.global.codecatalyst'}, got ''; serviceDetailSet expected the one-item detail set, got ''. Catalog read: serviceId expected 'vpce-svc-0df7f8ad818c92d25', got 'vpce-svc-b24fccc0'; availabilityZoneIdSet expected ['usw2-az2','usw2-az1','usw2-az3','usw2-az4'], got ['usw2-az1','usw2-az2','usw2-az3']; privateDnsNameSet expected two names, got one; got an extra payerResponsibility member.
3. DescribeInstances, 200 vs 200: instancesSet.item.clientToken expected '', got None; instancesSet.item.metadataOptions.httpProtocolIpv4 expected 'enabled', got None; instancesSet.item.currentInstanceBootMode expected 'uefi', got 'legacy-bios'.
4. DescribeInstances, 200 vs 200: reservationSet.item.groupSet expected '' (empty), got {'item': {'groupId': 'sg-...', 'groupName': 'default'}}.
5. DescribeInstances on a terminated instance, 200 vs 200: blockDeviceMapping expected '', got the attached root volume; privateDnsName expected '', got None; reason expected 'User initiated (2026-11-03 10:23:03 GMT)' form, got 'User initiated (2026-11-11T17:57:46.128884Z)'.
6. DescribeVpcs, 200 vs 200: vpcSet.item.state expected 'available', got 'pending'; vpcSet.item.dhcpOptionsSource expected 'vpc', got None.
7. DescribeAvailabilityZones, 200 vs 200: us-west-2a zoneId expected 'usw2-az2', got 'usw2-az1'; us-west-2b zoneId expected 'usw2-az1', got 'usw2-az2'; us-west-2d zoneId expected 'usw2-az4'. CreateVolume, 200 vs 200: availabilityZoneId expected 'usw2-az2', got 'usw2-az1'.
8. DescribeManagedPrefixLists, 200 vs 200: item[0] expected prefixListName 'com.amazonaws.us-west-2.route53-healthchecks' / addressFamily 'IPv4' / prefixListId 'pl-0068613c321dee54b', got 'com.amazonaws.us-west-2.ipv6.route53-healthchecks' / 'IPv6' / 'pl-080941ad' (16 items diverge).
9. DescribePrefixLists, 200 vs 200: item[4] expected prefixListId 'pl-68a54001' with ten real S3 ranges, got 'pl-fffbb61c' with cidrSet {'item': '198.18.96.0/21'} (6 items diverge).
10. GetManagedPrefixListAssociations: expected HTTP 400 with Response.Errors.Error.Code 'UnsupportedOperation' and Message 'The action is not supported for an AWS-managed prefix list.', got HTTP 200 and no Response error element. ModifyManagedPrefixList, 400 vs 400: Message expected 'The action is not supported for an AWS-managed prefix list.', got "The prefix list 'pl-e34ce16f' is an Amazon Web Services-managed prefix list, which cannot be modified".

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator and inspect these fixtures/steps:
1. ec2-catalog-describe-instance-types-a1-c9g, steps task#1/task#2/task#3 (DescribeInstanceTypes, one page per step) and ec2-account-and-instance-type-metadata step task#3.
2. ec2-catalog-describe-vpc-endpoint-services, steps task#2 (DescribeVpcEndpointServices with ServiceName=aws.api.global.codecatalyst) and task#15 (unfiltered catalog page).
3. ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics step task#13; ec2-dvpc-traj-reference-path-instance-in-created-vpc step task#9; ec2-byvpc-traj-stopped-instance-stays-in-its-vpc step task#9 (DescribeInstances).
4. ec2-trajwire-lia-instance-id-from-another-region step task#7 (DescribeInstances reservation groupSet).
5. ec2-byvpc-traj-terminated-instance-loses-vpc-attribution step task#7 (DescribeInstances after TerminateInstances).
6. ec2-byvpc-traj-swapped-text-columns-vpc-and-instance-ids step task#6 and ec2-create-vpc-availability-and-delete-lifecycle step task#2 (DescribeVpcs right after CreateVpc).
7. ec2-catalog-describe-availability-zones step task#1 (DescribeAvailabilityZones) and ec2-snapshot-inventory-and-attribute-inspection step setup#1 (CreateVolume in us-west-2a).
8. ec2-catalog-describe-managed-prefix-lists step task#1 (DescribeManagedPrefixLists, AWS-owned entries).
9. ec2-catalog-describe-prefix-lists step task#1 (DescribePrefixLists).
10. ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership steps task#9 (ModifyManagedPrefixList on an AWS-managed list) and task#10 (GetManagedPrefixListAssociations on the same list).
