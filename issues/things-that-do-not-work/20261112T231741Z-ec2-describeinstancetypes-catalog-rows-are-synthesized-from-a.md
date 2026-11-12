Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-a1-c9g
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-a1-c9g ec2-catalog-describe-instance-types-r9g-z1d
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.12xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.16xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.24xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.8xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c5a.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.12xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.16xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.24xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.2xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.2xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.2xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.2xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.2xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.32xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.interfaceTypeSet.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.48xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.4xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.4xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.4xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.4xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.4xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.4xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.4xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.8xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.8xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.8xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.8xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.8xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.8xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.8xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.8xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.interfaceTypeSet.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.96xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.large"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.large"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.large"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.large"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.interfaceTypeSet.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-48xl"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.interfaceTypeSet.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.metal-96xl"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.xlarge"].networkInfo.networkCards.item.additionalFlexibleNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.xlarge"].networkInfo.networkCards.item.maximumEnaQueueCount
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.xlarge"].networkInfo.networkCards.item.maximumEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="c8id.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.2xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.2xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.2xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.2xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.2xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.large"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.large"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.large"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.large"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.medium"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.medium"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.medium"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.medium"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.medium"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.medium"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.medium"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.micro"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.micro"].freeTierEligible
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.micro"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.micro"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.micro"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.micro"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.micro"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.micro"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.nano"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.nano"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.nano"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.nano"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.nano"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.nano"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.nano"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.small"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.small"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.small"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.small"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.small"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.small"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.small"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.xlarge"].networkInfo.networkCards.item.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="t3.xlarge"].vCpuInfo.validCores.item
Issue-Related: 20261031T012056Z-ec2-describeinstancetypes-omits-modeled-per-type-members-and.md
Issue-Related: 20261106T142039Z-ec2-describeinstancetypes-catalog-pages-still-diverge-per-instance-type.md
Issue-Commit: c3c8a5d5636f74138d44709d078c46bc0a5a7c2e

# ec2 DescribeInstanceTypes catalog rows are synthesized from a size formula instead of the real per-type hardware facts

> _Note: this issue was identified against the code base at commit `c3c8a5d5636f74138d44709d078c46bc0a5a7c2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
DescribeInstanceTypes answers 200 with a complete-looking instanceTypeSet, but the per-type attribute values are derived from the type's size suffix rather than taken from the instance-type catalog, and several modeled members are never emitted at all. The same defect shows up on every catalog page the corpus replays; three distinct replay clusters are bundled here. All three are net-new: no open issue carries their replay keys (the related open issues 20261112T182924Z-ec2-describeinstancetypes-rows-for-the-c4-c9gd-m6i.md and 20261112T211315Z-ec2-describeinstancetypes-catalog-pages-serve-synthesized-per-type.md describe the same family of fault for other pages but do not carry these fixture/step bindings).

Cause 1 of 3 — compute-optimized AMD rows (c5a.* page). Fixture ec2-catalog-describe-instance-types-a1-c9g, step task#6.
AWS: Smithy ec2#InstanceTypeInfo exposes per-type facts that AWS serves from its instance-type catalog, including cpuBurstModel, dedicatedHostsSupported, hibernationSupported, ebsInfo.ebsOptimizedInfo (baselineBandwidthInMbps / baselineIops / baselineThroughputInMBps / maximumBandwidthInMbps / maximumIops / maximumThroughputInMBps), networkInfo.trafficMirroringSupport, networkInfo.networkCards.item.baselineBandwidthInGbps and networkInfo.networkCards.item.defaultEnaQueueCountPerInterface (Smithy: ec2#NetworkCardInfo member DefaultEnaQueueCountPerInterface, xmlName defaultEnaQueueCountPerInterface, 'The default number of the ENA queues for each interface'), plus vCpuInfo.validCores. For a given instance type these are fixed catalog facts, not functions of vCPU count; a type that does not burst reports cpuBurstModel 'none', a type that supports neither dedicated hosts nor hibernation reports those as false, and traffic mirroring support is stated explicitly.
Emulator: for the c5a family the emulator omits cpuBurstModel, networkInfo.trafficMirroringSupport and networkCards.item.defaultEnaQueueCountPerInterface entirely, and computes the remaining numbers from the size suffix — in the replayed step it reported dedicatedHostsSupported true and hibernationSupported true where the recording has false, and ebsOptimizedInfo baseline/maximum bandwidth, IOPS and throughput and networkCards baselineBandwidthInGbps all off by a scaling factor (one recorded row: baselineBandwidthInMbps 1150 vs 400, baselineIops 6000 vs 1600, baselineThroughputInMBps 143.75 vs 50.0, maximumBandwidthInMbps 4750 vs 3170, maximumIops 18750 vs 13300, maximumThroughputInMBps 593.75 vs 396.25, networkCards baselineBandwidthInGbps 1.04 vs 1.25). vCpuInfo.validCores is also emitted with extra entries that the real catalog does not list for the type. The recorded numbers above are examples of the formula, not the requirement: the requirement is that each type's values come from the catalog.
Why: clients that size EBS or network capacity, or that gate hibernation / dedicated-host / traffic-mirroring features on this call, read values that do not exist on AWS — a workload sized against the emulator's figures is mis-provisioned, and a client that branches on the absent cpuBurstModel or trafficMirroringSupport member sees an unset optional where AWS always states a value.
Fix: the DescribeInstanceTypes handler's catalog/state data for the instance-type rows (and its serializer for the members that are never emitted), not the request path.

Cause 2 of 3 — Nitro storage-dense metal rows (c8id.* page). Fixture ec2-catalog-describe-instance-types-a1-c9g, step task#31.
AWS: same modeled members, plus ebsInfo.maximumEbsAttachments, ebsInfo.attachmentLimitType, instanceStorageInfo.disks.item.count / sizeInGB / totalSizeInGB, networkInfo.bandwidthWeightings, networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout, networkInfo.ipv4AddressesPerInterface, ipv4AddressesPerSecondaryInterface, ipv6AddressesPerInterface, maximumNetworkInterfaces, networkCards.item.additionalFlexibleNetworkInterfaces / maximumEnaQueueCount / maximumEnaQueueCountPerInterface, networkInfo.secondaryNetworkSupported, processorInfo.supportedFeatures and rebootMigrationSupport — all catalog facts for the type.
Emulator: for the c8id family it again omits cpuBurstModel, bandwidthWeightings, connectionTracking and the ENA-queue members, and reports computed values for the rest; the replayed step shows ebsInfo.maximumEbsAttachments 15 vs the recorded 79, instanceStorageInfo.disks.item.count 12 vs 6 and sizeInGB 1900 vs 3800 (i.e. the same total split into the wrong number of disks), and ebsOptimizedInfo baseline/maximum bandwidth 81600 vs 80000, IOPS 360000 vs 480000 and throughput 10200.0 vs 10000.0.
Why: a client that lays out instance-store volumes or counts EBS attachment slots from this catalog gets a machine shape AWS never offers; the disk count/size split in particular changes how many ephemeral devices a client expects to find on the instance.
Fix: same component — the instance-type catalog data behind the DescribeInstanceTypes handler.

Cause 3 of 3 — burstable rows (t3.* page). Fixture ec2-catalog-describe-instance-types-r9g-z1d, step task#5.
AWS: for a burstable type AWS states cpuBurstModel 'credit', network performance as the burstable text form ('Up to 5 Gigabit') with networkCards baselineBandwidthInGbps well below peakBandwidthInGbps, defaultEnaQueueCountPerInterface per size, trafficMirroringSupport 'supported', rebootMigrationSupport 'supported', freeTierEligible for t3.micro, and vCpuInfo.validCores as a single value for a 2-vCPU size.
Emulator: cpuBurstModel, defaultEnaQueueCountPerInterface and trafficMirroringSupport are omitted; networkPerformance is rendered as a fixed-rate string ('12.5 Gigabit' / 'Up to 10 Gigabit') instead of the type's burstable text; baselineBandwidthInGbps and peakBandwidthInGbps are both set to the computed peak (recorded example t3.xlarge: baseline 12.5 vs 1.024, peak 12.5 vs 5.0); rebootMigrationSupport is 'unsupported' where the recording has 'supported'; freeTierEligible diverges for t3.micro; and vCpuInfo.validCores is a list ['1','2'] where the recording has the single value '2' (a type_mismatch, so the member's wire shape differs too, not only its value).
Why: burstable types are exactly the ones whose baseline-vs-peak distinction clients act on; publishing baseline == peak makes a t3 look like a fixed-performance instance, and the validCores shape change breaks a client that reads a scalar there.
Fix: the instance-type catalog data plus the vCpuInfo.validCores serializer in the DescribeInstanceTypes handler.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All three clusters: verdict mismatch, expected_status 200, actual_status 200, service ec2, operation DescribeInstanceTypes, stub_path empty, owned_by empty.

Cause 1 (ec2-catalog-describe-instance-types-a1-c9g#task#6), divergence_key service=ec2|DescribeInstanceTypes|status=200->200, sample diffs:
  $...item[instanceType="c5a.xlarge"].cpuBurstModel: missing (expected 'none', got None)
  $...item[instanceType="c5a.xlarge"].dedicatedHostsSupported: value_mismatch (expected 'false', got 'true')
  $...item[instanceType="c5a.xlarge"].hibernationSupported: value_mismatch (expected 'false', got 'true')
  $...item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: value_mismatch (expected '400', got '1150')
  $...item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops: value_mismatch (expected '1600', got '6000')
  $...item[instanceType="c5a.xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps: value_mismatch (expected '396.25', got '593.75')
  $...item[instanceType="c5a.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps: value_mismatch (expected '1.25', got '1.04')
  $...item[instanceType="c5a.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface: missing (expected '8', got None)
  $...item[instanceType="c5a.xlarge"].networkInfo.trafficMirroringSupport: missing (expected 'supported', got None)
  (cluster key also lists c5a.large/2xlarge/4xlarge/8xlarge/12xlarge/16xlarge/24xlarge with the same member set plus vCpuInfo.validCores.item[n])

Cause 2 (ec2-catalog-describe-instance-types-a1-c9g#task#31), sample diffs:
  $...item[instanceType="c8id.metal-96xl"].cpuBurstModel: missing (expected 'none', got None)
  $...item[instanceType="c8id.metal-96xl"].ebsInfo.maximumEbsAttachments: value_mismatch (expected '79', got '15')
  $...item[instanceType="c8id.metal-96xl"].instanceStorageInfo.disks.item.count: value_mismatch (expected '6', got '12')
  $...item[instanceType="c8id.metal-96xl"].instanceStorageInfo.disks.item.sizeInGB: value_mismatch (expected '3800', got '1900')
  $...item[instanceType="c8id.metal-96xl"].ebsInfo.ebsOptimizedInfo.baselineIops: value_mismatch (expected '480000', got '360000')
  $...item[instanceType="c8id.metal-96xl"].networkInfo.bandwidthWeightings: missing (expected {'item': ['default','ebs-1','vpc-1']}, got None)
  (cluster key also lists c8id.2xlarge/12xlarge/16xlarge/24xlarge/32xlarge with attachmentLimitType, connectionTrackingConfiguration.defaultTcpEstablishedTimeout, ipv4/ipv6AddressesPerInterface, maximumNetworkInterfaces, networkCards.item.additionalFlexibleNetworkInterfaces / maximumEnaQueueCount / maximumEnaQueueCountPerInterface, secondaryNetworkSupported, trafficMirroringSupport, processorInfo.supportedFeatures, rebootMigrationSupport)

Cause 3 (ec2-catalog-describe-instance-types-r9g-z1d#task#5), sample diffs:
  $...item[instanceType="t3.xlarge"].cpuBurstModel: missing (expected 'credit', got None)
  $...item[instanceType="t3.xlarge"].networkInfo.networkCards.item.baselineBandwidthInGbps: value_mismatch (expected '1.024', got '12.5')
  $...item[instanceType="t3.xlarge"].networkInfo.networkCards.item.peakBandwidthInGbps: value_mismatch (expected '5.0', got '12.5')
  $...item[instanceType="t3.xlarge"].networkInfo.networkCards.item.networkPerformance: value_mismatch (expected 'Up to 5 Gigabit', got '12.5 Gigabit')
  $...item[instanceType="t3.xlarge"].networkInfo.networkCards.item.defaultEnaQueueCountPerInterface: missing (expected '4', got None)
  $...item[instanceType="t3.xlarge"].rebootMigrationSupport: value_mismatch (expected 'supported', got 'unsupported')
  $...item[instanceType="t3.xlarge"].vCpuInfo.validCores.item: type_mismatch (expected '2', got ['1','2'])
  $...item[instanceType="t3.large"].networkInfo.networkCards.item.networkPerformance: value_mismatch (expected 'Up to 5 Gigabit', got 'Up to 10 Gigabit')
  (cluster key also lists t3.nano/micro/small/medium/2xlarge, including t3.micro freeTierEligible)

Model grounding: Smithy ec2#NetworkCardInfo member DefaultEnaQueueCountPerInterface (xmlName defaultEnaQueueCountPerInterface) is modeled, so its absence is a missing modeled member, not an AWS-side omission.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Fixture ec2-catalog-describe-instance-types-a1-c9g, step task#6: DescribeInstanceTypes for the c5a family — compare instanceTypeSet rows against the recorded response; cpuBurstModel / trafficMirroringSupport / defaultEnaQueueCountPerInterface are absent and the ebsOptimizedInfo and networkCards numbers, dedicatedHostsSupported and hibernationSupported differ.
4. Same fixture, step task#31: DescribeInstanceTypes for the c8id family — ebsInfo.maximumEbsAttachments, instanceStorageInfo.disks.item.count/sizeInGB, bandwidthWeightings and the ENA-queue members diverge.
5. Fixture ec2-catalog-describe-instance-types-r9g-z1d, step task#5: DescribeInstanceTypes for the t3 family — networkPerformance/baseline/peak bandwidth, cpuBurstModel, rebootMigrationSupport and the vCpuInfo.validCores shape diverge.
