Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-c9gd-m6gd ec2-catalog-describe-instance-types-m6i-r4 ec2-catalog-describe-instance-types-r9g-z1d
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.2xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.4xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.4xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.8xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.8xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.large"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.interfaceTypeSet.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item.phcSupport
Issue-Related: 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md
Issue-Related: 20261030T124526Z-ec2-describeinstancetypes-serves-a-synthesized-instance-type-catalog.md
Issue-Related: 20261030T210852Z-ec2-describeinstancetypes-synthesizes-instance-type-attributes-instead-of.md
Issue-Related: 20261111T180943Z-ec2-read-paths-serve-emulator-derived-catalog-data.md
Issue-Commit: 9f30cc5f50e7736550fc9d928f8a80b554132b0d

# ec2 DescribeInstanceTypes rows for the c4, c9gd, m6i, r4 and r9g pages serve synthesized hardware facts

> _Note: this issue was identified against the code base at commit `9f30cc5f50e7736550fc9d928f8a80b554132b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
DescribeInstanceTypes answers HTTP 200 but the InstanceTypeInfo rows it serves are generated from a small set of emulator defaults instead of the per-instance-type facts AWS publishes, and several modeled members are omitted entirely. This bundle carries 10 separate net-new divergence clusters (one per failing catalog step); they share one root cause: the instance-type catalog table. None of these replay keys is owned by an existing issue - the similar open issues 20261106T142039Z-ec2-describeinstancetypes-catalog-pages-still-diverge-per-instance-type.md, 20261106T122956Z-ec2-instance-type-catalog-pages-a1-c9g-through.md and 20261107T184734Z-ec2-describeinstancetypes-instance-type-catalog-returns-wrong-and.md do not carry them, so the steps below are still unexercised by any filed fix.

GENERAL FAULT
AWS: DescribeInstanceTypes returns, for each requested instance type, that type's published attributes (Smithy: DescribeInstanceTypes -> InstanceTypes/InstanceTypeInfo, members memoryInfo.sizeInMiB, vCpuInfo.defaultCores/defaultVCpus/validCores, ebsInfo.attachmentLimitType/encryptionSupport/maximumEbsAttachments/ebsOptimizedInfo.{baseline,maximum}{BandwidthInMbps,Iops,ThroughputInMBps}, networkInfo.{networkPerformance,maximumNetworkInterfaces,ipv4AddressesPerInterface,ipv6AddressesPerInterface,encryptionInTransitSupported,connectionTrackingConfiguration,networkCards}, processorInfo.{manufacturer,sustainedClockSpeedInGhz}, currentGeneration, burstablePerformanceSupported, autoRecoverySupported, dedicatedHostsSupported, instanceStorageSupported, instanceStorageInfo, gpuInfo, inferenceAcceleratorInfo, placementGroupInfo.supportedStrategies, supportedBootModes, supportedRootDeviceTypes, supportedVirtualizationTypes, phcSupport, rebootMigrationSupport, trafficMirroringSupport, cpuBurstModel). The recordings carry those values for every type the fixtures query.
Emulator: every row is filled from family-independent defaults - the same ebsOptimizedInfo quartet (575/3000/71.875 or 4750/18750/593.75), the same maximumEbsAttachments (27), the same networkInfo (25.0 Gbps card, 3 interfaces, 10 IPv4/IPv6 per interface, networkPerformance "25 Gigabit") - and a set of members is never emitted at all (cpuBurstModel, networkInfo.connectionTrackingConfiguration, networkInfo.ipv4AddressesPerSecondaryInterface, networkInfo.networkCards.item.defaultEnaQueueCountPerInterface, trafficMirroringSupport, rebootMigrationSupport, gpuInfo, inferenceAcceleratorInfo, instanceStorageInfo).
Why: clients size workloads from this call. Capacity planners, autoscaling sizing logic, and SDK/CLI validation that filters on accelerator presence, instance storage, ENA queue counts, network throughput or EBS attachment limits all take the wrong branch; a type with no local storage is reported as having it and vice versa, and accelerator-aware schedulers see no accelerator at all.
Fix: the instance-type catalog data the DescribeInstanceTypes handler reads (the service's static instance-type table) plus the serializer paths for the members that are never emitted.

Cause 1 of 10: fixture ec2-catalog-describe-instance-types-a1-c9g, step task#4 - the c4 family (c4.large, c4.xlarge, c4.2xlarge, c4.4xlarge, c4.8xlarge). currentGeneration 'true' instead of 'false'; ebsInfo.attachmentLimitType 'dedicated' instead of 'shared'; ebsOptimizedInfo 575/3000/71.875 instead of 500/4000/62.5; maximumEbsAttachments 27 instead of 40; memoryInfo.sizeInMiB 4096 instead of 3840 for c4.large; cpuBurstModel and networkInfo.connectionTrackingConfiguration absent; c4.8xlarge vCpuInfo.defaultCores/defaultVCpus/validCores wrong.
Cause 2 of 10: fixture ec2-catalog-describe-instance-types-c9gd-m6gd, step task#5 - ebsOptimizedInfo 20400/90000/2550.0 instead of 19000/80000/2375.0; maximumEbsAttachments 14 instead of 19; instanceStorageSupported 'true' instead of 'false'; networkInfo.encryptionInTransitSupported 'false' instead of 'true'; inferenceAcceleratorInfo (Qualcomm AI100 accelerators) absent; ipv4AddressesPerSecondaryInterface and networkCards.item.defaultEnaQueueCountPerInterface absent.
Cause 3 of 10: fixture ec2-catalog-describe-instance-types-c9gd-m6gd, step task#16 - autoRecoverySupported and dedicatedHostsSupported 'true' instead of 'false'; ebsInfo.attachmentLimitType 'shared' instead of 'dedicated'; ebsOptimizedInfo 4600/18750/575.0 instead of 8000/32000/1000.0; gpuInfo (NVIDIA L4) and instanceStorageInfo (1x450 GB ssd) absent; processorInfo.manufacturer wrong; rebootMigrationSupport absent.
Cause 4 of 10: fixture ec2-catalog-describe-instance-types-m6i-r4, step task#26 - memoryInfo.sizeInMiB 98304 instead of 262144; ipv4AddressesPerInterface and ipv6AddressesPerInterface 10 instead of 30; maximumNetworkInterfaces 3 instead of 8; networkCards baselineBandwidthInGbps 25.0 instead of 10.0; ebsOptimizedInfo 575/3000/71.875 instead of 10000/55000/1250.0; phcSupport, placementGroupInfo.supportedStrategies and supportedBootModes wrong.
Cause 5 of 10: fixture ec2-catalog-describe-instance-types-m6i-r4, step task#27 (cluster also covers task#29) - instanceStorageInfo (1x1900 GB ssd, nvmeSupport required) absent and instanceStorageSupported 'false' instead of 'true'; networkInfo.encryptionInTransitSupported 'false' instead of 'true'; same ebsOptimizedInfo and per-interface address-count defaults as Cause 4.
Cause 6 of 10: fixture ec2-catalog-describe-instance-types-m6i-r4, step task#28 - ebsOptimizedInfo 575/3000/71.875 instead of 10000/55000/1250.0; maximumEbsAttachments 27 instead of 10; networkCards.item.defaultEnaQueueCountPerInterface (8) absent; maximumNetworkInterfaces 3 instead of 8.
Cause 7 of 10: fixture ec2-catalog-describe-instance-types-m6i-r4, step task#30 - ebsOptimizedInfo 575/3000/71.875 instead of 14000/80000/1750.0; maximumEbsAttachments 27 instead of 16; trafficMirroringSupport ('supported') absent; networkCards.item.maximumNetworkInterfaces 3 instead of 8.
Cause 8 of 10: fixture ec2-catalog-describe-instance-types-m6i-r4, step task#31 - networkCards.item.defaultEnaQueueCountPerInterface (32) absent; vCpuInfo.defaultCores and vCpuInfo.defaultThreadsPerCore wrong; networkCards baselineBandwidthInGbps 25.0 instead of 10.0.
Cause 9 of 10: fixture ec2-catalog-describe-instance-types-m6i-r4, step task#32 (cluster also covers task#33 and task#34) - same synthesized ebsOptimizedInfo/network-card/per-interface values and absent ipv4AddressesPerSecondaryInterface and defaultEnaQueueCountPerInterface.
Cause 10 of 10: fixture ec2-catalog-describe-instance-types-r9g-z1d, step task#3 - burstablePerformanceSupported 'true' instead of 'false'; ebsInfo.encryptionSupport 'unsupported' instead of 'supported'; ebsInfo.attachmentLimitType 'dedicated' instead of 'shared'; memoryInfo.sizeInMiB 1024 instead of 627; networkPerformance and networkCards.item.networkPerformance '25 Gigabit' instead of 'Very Low'; networkCards peakBandwidthInGbps 25.0 instead of 0.28 and baselineBandwidthInGbps 25.0 instead of 0.07; connectionTrackingConfiguration, ipv6AddressesPerInterface, defaultEnaQueueCountPerInterface and trafficMirroringSupport absent; supportedRootDeviceTypes and supportedVirtualizationTypes wrong.

## Evidence
All ten clusters: expected status 200, actual status 200, verdict mismatch - the divergence is in the body.
Cause 1 (a1-c9g#task#4): $.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c4.large"].currentGeneration expected 'false', got 'true'; .ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps expected '500', got '575'; .ebsInfo.maximumEbsAttachments expected '40', got '27'; .memoryInfo.sizeInMiB expected '3840', got '4096'; .cpuBurstModel expected 'none', got None; .networkInfo.connectionTrackingConfiguration expected {'defaultTcpEstablishedTimeout':'432000','defaultUdpStreamTimeout':'180','defaultUdpTimeout':'30'}, got None.
Cause 2 (c9gd-m6gd#task#5): baselineBandwidthInMbps expected '19000', got '20400'; baselineIops expected '80000', got '90000'; maximumEbsAttachments expected '19', got '14'; instanceStorageSupported expected 'false', got 'true'; networkInfo.encryptionInTransitSupported expected 'true', got 'false'; inferenceAcceleratorInfo expected {'accelerators':{'member':{'count':'8','manufacturer':'Qualcomm',...}},'totalInferenceMemoryInMiB':'128000'}, got None.
Cause 3 (c9gd-m6gd#task#16): autoRecoverySupported expected 'false', got 'true'; dedicatedHostsSupported expected 'false', got 'true'; ebsInfo.attachmentLimitType expected 'dedicated', got 'shared'; baselineIops expected '32000', got '18750'; gpuInfo expected {'gpus':{'item':{'manufacturer':'NVIDIA','name':'L4',...}}}, got None; instanceStorageInfo expected {'disks':{'item':{'count':'1','sizeInGB':'450','type':'ssd'}},...}, got None.
Cause 4 (m6i-r4#task#26): memoryInfo.sizeInMiB expected '262144', got '98304'; networkInfo.ipv4AddressesPerInterface expected '30', got '10'; maximumNetworkInterfaces expected '8', got '3'; networkCards.item.baselineBandwidthInGbps expected '10.0', got '25.0'.
Cause 5 (m6i-r4#task#27): instanceStorageInfo expected {'disks':{'item':{'count':'1','sizeInGB':'1900','type':'ssd'}},'encryptionSupport':'required','nvmeSupport':'required','totalSizeInGB':'1900'}, got None; instanceStorageSupported expected 'true', got 'false'.
Cause 6 (m6i-r4#task#28): maximumEbsAttachments expected '10', got '27'; networkCards.item.defaultEnaQueueCountPerInterface expected '8', got None.
Cause 7 (m6i-r4#task#30): baselineBandwidthInMbps expected '14000', got '575'; maximumEbsAttachments expected '16', got '27'; trafficMirroringSupport expected 'supported', got None.
Cause 8 (m6i-r4#task#31): networkCards.item.defaultEnaQueueCountPerInterface expected '32', got None; maximumNetworkInterfaces expected '8', got '3'.
Cause 9 (m6i-r4#task#32): ipv4AddressesPerSecondaryInterface expected '0', got None; baselineThroughputInMBps expected '1250.0', got '71.875'.
Cause 10 (r9g-z1d#task#3): burstablePerformanceSupported expected 'false', got 'true'; ebsInfo.encryptionSupport expected 'supported', got 'unsupported'; memoryInfo.sizeInMiB expected '627', got '1024'; networkInfo.networkPerformance expected 'Very Low', got '25 Gigabit'; networkCards.item.peakBandwidthInGbps expected '0.28', got '25.0'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture ec2-catalog-describe-instance-types-a1-c9g step task#4: DescribeInstanceTypes with InstanceType filters for the c4 family, and compare every InstanceTypeInfo member against the recorded response.
3. Repeat for ec2-catalog-describe-instance-types-c9gd-m6gd steps task#5 and task#16, ec2-catalog-describe-instance-types-m6i-r4 steps task#26, task#27, task#28, task#30, task#31 and task#32, and ec2-catalog-describe-instance-types-r9g-z1d step task#3.
4. Each step returns 200 with family-independent default values and with cpuBurstModel, connectionTrackingConfiguration, defaultEnaQueueCountPerInterface, trafficMirroringSupport, gpuInfo, inferenceAcceleratorInfo and instanceStorageInfo missing.
