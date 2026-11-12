Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-c9gd-m6gd ec2-catalog-describe-instance-types-r5-r8in
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.12xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.12xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.12xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.12xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.18xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.18xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.18xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.18xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.24xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.9xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.9xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.9xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.9xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.metal"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.12xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.12xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.12xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.12xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.16xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.16xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.16xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.16xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.24xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.8xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.metal"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5d.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.12xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.12xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.12xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.12xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.16xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.16xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.16xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.16xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.24xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.24xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.24xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.24xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="r5a.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Related: 20261030T232813Z-ec2-describeinstancetypes-catalog-sweep-returns-synthesized-ebs-network.md
Issue-Related: 20261104T201311Z-ec2-instance-type-catalog-is-synthesized-per-family.md
Issue-Related: 20261109T233303Z-ec2-describeinstancetypes-pages-c9gd-m6gd-and-r9g-z1d.md
Issue-Commit: 75cd4519d4d8ae943d7fac1c316cbee2ee5c3551

# ec2 DescribeInstanceTypes catalog pages serve synthesized per-type attributes and omit cpuBurstModel, trafficMirroringSupport and defaultEnaQueueCountPerInterface

> _Note: this issue was identified against the code base at commit `75cd4519d4d8ae943d7fac1c316cbee2ee5c3551`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
DescribeInstanceTypes answers HTTP 200 with a well-formed ec2Query XML body, but the per-instance-type attribute rows it serves are emulator-authored rather than the AWS catalog: a family-independent formula is used for the capacity numbers, several optional members are never emitted at all, and some boolean/enum capability flags are inverted. Every page of the ec2-catalog-describe-instance-types-* corpus that still diverges shows the same shape of fault, so this is one defect in the catalog table behind the operation, not a per-type accident.

The replay reports 158 still-unowned DescribeInstanceTypes clusters (all of them service=ec2, all HTTP 200 -> 200 body mismatches) spread over the ec2-catalog-describe-instance-types-* fixtures. Three of them are written up below as the causes to close; they were picked to cover three different generations/families (c5, m5d, r5a) so that a fix proven against them is a fix of the table and not of one row. Open issues 20261106T122956Z-ec2-instance-type-catalog-pages-a1-c9g-through.md and 20261106T142039Z-ec2-describeinstancetypes-catalog-pages-still-diverge-per-instance-type.md describe the same family of fault but carry different Issue-Keys/steps and do not cover these clusters (the replay reports owned_by empty for all three).

Cause 1 of 3: Intel C5 rows (fixture ec2-catalog-describe-instance-types-a1-c9g, step task#5).
AWS: Smithy com.amazonaws.ec2#InstanceTypeInfo models currentGeneration (CurrentGenerationFlag, boolean), dedicatedHostsSupported (DedicatedHostFlag, boolean), processorInfo (ProcessorInfo), vCpuInfo.validCores (VCpuInfo), ebsInfo.ebsOptimizedInfo (EbsOptimizedInfo with baselineBandwidthInMbps/baselineIops/baselineThroughputInMBps/maximumBandwidthInMbps/maximumIops/maximumThroughputInMBps), networkInfo (NetworkInfo) and networkInfo.networkCards (NetworkCardInfo, which models defaultEnaQueueCountPerInterface). AWS returns, for each instance type, that type's published facts: a previous-generation C5 type reports currentGeneration false, the EBS-optimized baseline/maximum figures published for that size, the sustained clock speed of the physical processor family, and the full validCores list for the size. The members cpuBurstModel (on the instance type) and networkInfo.trafficMirroringSupport are NOT present in this repo's on-disk EC2 Smithy model, so for those two the requirement is recorded behavior only: the recording shows AWS emitting cpuBurstModel='none' and networkInfo.trafficMirroringSupport='supported' for these types.
Emulator: for c5.24xlarge it answers currentGeneration 'true' (AWS 'false'), dedicatedHostsSupported 'true' (AWS 'false'), ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps 20400 (AWS 19000), baselineIops 90000 (AWS 80000), baselineThroughputInMBps 2550.0 (AWS 2375.0), maximumBandwidthInMbps 20400 (AWS 19000), maximumIops 90000 (AWS 80000), maximumThroughputInMBps 2550.0 (AWS 2375.0), processorInfo.sustainedClockSpeedInGhz 3.1 (AWS 3.6), and omits cpuBurstModel, networkInfo.trafficMirroringSupport and networkInfo.networkCards.item.defaultEnaQueueCountPerInterface entirely. The same cluster also diverges on vCpuInfo.validCores entries and on networkInfo.networkPerformance / networkCards peakBandwidthInGbps and baselineBandwidthInGbps for c5.12xlarge, c5.18xlarge, c5.2xlarge, c5.4xlarge, c5.9xlarge, c5.large, c5.metal (also autoRecoverySupported, ebsInfo.maximumEbsAttachments, rebootMigrationSupport, supportedBootModes) and c5.xlarge.
Why: a client that sizes workloads from this catalog gets wrong answers for exactly the questions the API exists to answer. Capacity planners read ebsOptimizedInfo to pick a type that can sustain a volume's throughput and will over-commit by ~7-8%; schedulers that filter on currentGeneration or dedicatedHostsSupported will offer previous-generation C5 as current and as Dedicated-Host-capable; code that reads cpuBurstModel, trafficMirroringSupport or defaultEnaQueueCountPerInterface sees an absent member and cannot distinguish 'not supported' from 'not reported'.
Fix: the static instance-type catalog data the DescribeInstanceTypes handler reads in the ec2 service crate (the per-type attribute table, not the XML writer) must carry the recorded AWS facts per type instead of deriving them from size; the two unmodeled members (cpuBurstModel, networkInfo.trafficMirroringSupport) additionally need emitting in that operation's serializer.
Traces: ec2-catalog-describe-instance-types-a1-c9g#task#5.

Cause 2 of 3: M5d rows (fixture ec2-catalog-describe-instance-types-c9gd-m6gd, step task#38).
AWS: as above, plus Smithy com.amazonaws.ec2#InstanceTypeInfo models instanceStorageInfo (InstanceStorageInfo, with disks count/sizeInGB), ebsInfo.maximumEbsAttachments and rebootMigrationSupport (RebootMigrationSupport enum). AWS reports the NVMe disk count and size actually attached to each M5d size, that size's EBS attachment limit, and whether reboot migration is supported for the type.
Emulator: for m5d.large it answers ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps 575 (AWS 650), baselineIops 3000 (AWS 3600), baselineThroughputInMBps 71.875 (AWS 81.25), networkInfo.networkCards.item.baselineBandwidthInGbps 0.52 (AWS 0.75), rebootMigrationSupport 'supported' (AWS 'unsupported'), and omits cpuBurstModel, defaultEnaQueueCountPerInterface and trafficMirroringSupport; for m5d.xlarge networkCards baselineBandwidthInGbps is 1.04 (AWS 1.25). The cluster additionally diverges on instanceStorageInfo.disks.item.count and sizeInGB, ebsInfo.maximumEbsAttachments, networkPerformance/peakBandwidthInGbps and validCores across m5d.12xlarge, m5d.16xlarge, m5d.24xlarge, m5d.2xlarge, m5d.4xlarge, m5d.8xlarge, m5d.large, m5d.metal and m5d.xlarge.
Why: instance-store-backed workloads choose a d-suffixed type precisely by disk count and size, and a wrong rebootMigrationSupport changes whether a client plans for an in-place reboot or a stop/start; both are read straight out of this response.
Fix: same component as Cause 1 - the per-type catalog rows for the M5d family (disks, EBS attachment limits, reboot-migration support, EBS-optimized and network-card figures).
Traces: ec2-catalog-describe-instance-types-c9gd-m6gd#task#38.

Cause 3 of 3: R5a rows (fixture ec2-catalog-describe-instance-types-r5-r8in, step task#2).
AWS: as above. For the AMD R5a family AWS reports dedicatedHostsSupported false, the published EBS-optimized baseline and maximum figures for each size, and the published network-card baseline bandwidth.
Emulator: for r5a.large it answers dedicatedHostsSupported 'true' (AWS 'false'), ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps 575 (AWS 650), baselineIops 3000 (AWS 3600), baselineThroughputInMBps 71.875 (AWS 81.25), maximumBandwidthInMbps 4750 (AWS 2880), maximumIops 18750 (AWS 16000), maximumThroughputInMBps 593.75 (AWS 360.0), networkInfo.networkCards.item.baselineBandwidthInGbps 0.52 (AWS 0.75), and omits cpuBurstModel, defaultEnaQueueCountPerInterface and trafficMirroringSupport. The same divergence repeats for r5a.12xlarge, r5a.16xlarge, r5a.24xlarge, r5a.2xlarge, r5a.4xlarge, r5a.8xlarge and r5a.xlarge, including validCores, networkPerformance and peakBandwidthInGbps.
Why: here the emulator over-reports the EBS maximum by 65% (4750 vs 2880 Mbps), so a client provisioning a volume against an r5a.large believes it has headroom AWS does not give it; and advertising Dedicated Hosts support for a family that has none makes a host-placement request look valid.
Fix: same component as Cause 1 - the per-type catalog rows for the R5a family.
Traces: ec2-catalog-describe-instance-types-r5-r8in#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 158 unowned, unstubbed DescribeInstanceTypes clusters, all service=ec2, verdict=mismatch, expected_status 200 / actual_status 200 (the fault is the body, not the status). Representative diffs reported by the replay:

ec2-catalog-describe-instance-types-a1-c9g#task#5 (divergence_key service=ec2|DescribeInstanceTypes|status=200->200):
  item[instanceType="c5.24xlarge"].cpuBurstModel: missing (expected 'none', got None)
  item[instanceType="c5.24xlarge"].currentGeneration: expected 'false', got 'true'
  item[instanceType="c5.24xlarge"].dedicatedHostsSupported: expected 'false', got 'true'
  item[instanceType="c5.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '19000', got '20400'
  item[instanceType="c5.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops: expected '80000', got '90000'
  item[instanceType="c5.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps: expected '2375.0', got '2550.0'
  item[instanceType="c5.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps: expected '19000', got '20400'
  item[instanceType="c5.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops: expected '80000', got '90000'
  item[instanceType="c5.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps: expected '2375.0', got '2550.0'
  item[instanceType="c5.24xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface: missing (expected '8', got None)
  item[instanceType="c5.24xlarge"].networkInfo.trafficMirroringSupport: missing (expected 'supported', got None)
  item[instanceType="c5.24xlarge"].processorInfo.sustainedClockSpeedInGhz: expected '3.6', got '3.1'
  (plus c5.12xlarge / c5.18xlarge / c5.2xlarge / c5.4xlarge / c5.9xlarge / c5.large / c5.metal / c5.xlarge keys for cpuBurstModel, currentGeneration, ebsOptimizedInfo.*, networkCards.baselineBandwidthInGbps, networkCards.peakBandwidthInGbps, networkCards.networkPerformance, networkInfo.networkPerformance, trafficMirroringSupport, processorInfo.sustainedClockSpeedInGhz, vCpuInfo.validCores.item[*], and for c5.metal also autoRecoverySupported, ebsInfo.maximumEbsAttachments, rebootMigrationSupport, supportedBootModes.item)

ec2-catalog-describe-instance-types-c9gd-m6gd#task#38:
  item[instanceType="m5d.large"].cpuBurstModel: missing (expected 'none', got None)
  item[instanceType="m5d.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '650', got '575'
  item[instanceType="m5d.large"].ebsInfo.ebsOptimizedInfo.baselineIops: expected '3600', got '3000'
  item[instanceType="m5d.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps: expected '81.25', got '71.875'
  item[instanceType="m5d.large"].networkInfo.networkCards.item.baselineBandwidthInGbps: expected '0.75', got '0.52'
  item[instanceType="m5d.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface: missing (expected '8', got None)
  item[instanceType="m5d.large"].networkInfo.trafficMirroringSupport: missing (expected 'supported', got None)
  item[instanceType="m5d.large"].rebootMigrationSupport: expected 'unsupported', got 'supported'
  item[instanceType="m5d.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps: expected '1.25', got '1.04'
  (plus instanceStorageInfo.disks.item.count / sizeInGB and ebsInfo.maximumEbsAttachments keys for m5d.12xlarge, m5d.16xlarge, m5d.24xlarge, m5d.4xlarge, m5d.8xlarge, m5d.metal)

ec2-catalog-describe-instance-types-r5-r8in#task#2:
  item[instanceType="r5a.large"].cpuBurstModel: missing (expected 'none', got None)
  item[instanceType="r5a.large"].dedicatedHostsSupported: expected 'false', got 'true'
  item[instanceType="r5a.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '650', got '575'
  item[instanceType="r5a.large"].ebsInfo.ebsOptimizedInfo.baselineIops: expected '3600', got '3000'
  item[instanceType="r5a.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps: expected '81.25', got '71.875'
  item[instanceType="r5a.large"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps: expected '2880', got '4750'
  item[instanceType="r5a.large"].ebsInfo.ebsOptimizedInfo.maximumIops: expected '16000', got '18750'
  item[instanceType="r5a.large"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps: expected '360.0', got '593.75'
  item[instanceType="r5a.large"].networkInfo.networkCards.item.baselineBandwidthInGbps: expected '0.75', got '0.52'
  item[instanceType="r5a.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface: missing (expected '8', got None)
  item[instanceType="r5a.large"].networkInfo.trafficMirroringSupport: missing (expected 'supported', got None)

Model evidence (repo on-disk Smithy, services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): com.amazonaws.ec2#InstanceTypeInfo members currentGeneration/dedicatedHostsSupported/autoRecoverySupported (boolean flags), rebootMigrationSupport (enum), supportedBootModes (list), instanceStorageInfo, ebsInfo, networkInfo, processorInfo, vCpuInfo; com.amazonaws.ec2#EbsOptimizedInfo members baselineBandwidthInMbps/baselineIops/baselineThroughputInMBps/maximumBandwidthInMbps/maximumIops/maximumThroughputInMBps; com.amazonaws.ec2#NetworkCardInfo members baselineBandwidthInGbps/peakBandwidthInGbps/networkPerformance/defaultEnaQueueCountPerInterface. cpuBurstModel and NetworkInfo.trafficMirroringSupport are absent from that model, so their requirement is taken from the recording only.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from this checkout.
2. Replay wire fixture ec2-catalog-describe-instance-types-a1-c9g and stop at step task#5 (a DescribeInstanceTypes call over the ec2Query protocol for the C5 page).
3. Compare the DescribeInstanceTypesResponse body against the recording: for item[instanceType="c5.24xlarge"] the emulator returns currentGeneration true, dedicatedHostsSupported true, ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps 20400, baselineIops 90000, baselineThroughputInMBps 2550.0, maximumBandwidthInMbps 20400, maximumIops 90000, maximumThroughputInMBps 2550.0, processorInfo.sustainedClockSpeedInGhz 3.1, and no cpuBurstModel, no networkInfo.trafficMirroringSupport and no networkInfo.networkCards.item.defaultEnaQueueCountPerInterface, where AWS returns false/false/19000/80000/2375.0/19000/80000/2375.0/3.6 plus 'none', 'supported' and 8.
4. Repeat with fixture ec2-catalog-describe-instance-types-c9gd-m6gd step task#38 (M5d page: rebootMigrationSupport, instanceStorageInfo disks, EBS attachment limits) and fixture ec2-catalog-describe-instance-types-r5-r8in step task#2 (R5a page: dedicatedHostsSupported and the EBS-optimized maxima).
5. Both assertions fail with HTTP 200 on each side and a body mismatch, so the whole corpus re-replay is the check: a correct per-type catalog closes all three clusters at once.
