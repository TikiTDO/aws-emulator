Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-c9gd-m6gd
Issue-Step: task#43
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-c9gd-m6gd ec2-catalog-describe-instance-types-m6i-r4
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.12xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.16xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.8xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.medium"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].autoRecoverySupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].nitroTpmSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.metal"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m6g.xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.bandwidthWeightings
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].processorInfo.supportedFeatures
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.12xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.bandwidthWeightings
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].processorInfo.supportedFeatures
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.16xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.bandwidthWeightings
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].processorInfo.supportedFeatures
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.bandwidthWeightings
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].processorInfo.supportedFeatures
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.bandwidthWeightings
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.maximumNetworkInterfaces
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].processorInfo.supportedFeatures
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.8xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.bandwidthWeightings
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].processorInfo.supportedFeatures
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].ebsInfo.attachmentLimitType
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.bandwidthWeightings
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.connectionTrackingConfiguration.defaultTcpEstablishedTimeout
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.ipv4AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.ipv4AddressesPerSecondaryInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.ipv6AddressesPerInterface
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].processorInfo.supportedFeatures
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m8i-flex.xlarge"].rebootMigrationSupport
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# ec2 DescribeInstanceTypes catalog pages c9gd-m6gd and m6i-r4 serve wrong EBS/network attributes and omit modeled members

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
Two causes, both on ec2 DescribeInstanceTypes, each with its own fixture and step; both must be closed before this issue is submitted. The replay found 46 net-new DescribeInstanceTypes divergence clusters in the ec2-catalog-describe-instance-types-* fixture family (every one reported with stub_path empty and owned_by empty, i.e. no open issue carries their replay keys); the two below are the representatives a fixer must re-replay, and the same two defect classes recur per instance type across that family.

Cause 1 of 2: per-instance-type attribute values are emulator-authored and contradict the recorded catalog.
AWS: Smithy com.amazonaws.ec2 InstanceTypeInfo and its sub-structures model these as per-type facts — EbsInfo.EbsOptimizedInfo (BaselineBandwidthInMbps, BaselineIops, BaselineThroughputInMBps, MaximumBandwidthInMbps, MaximumIops, MaximumThroughputInMBps), EbsInfo.MaximumEbsAttachments, EbsInfo.AttachmentLimitType, NetworkInfo (Ipv4AddressesPerInterface, Ipv6AddressesPerInterface, MaximumNetworkInterfaces, MaximumNetworkCards, NetworkCards, NetworkPerformance, EncryptionInTransitSupported, SecondaryNetworkSupported, TrafficMirroringSupport), ProcessorInfo.SustainedClockSpeedInGhz, DedicatedHostsSupported and PlacementGroupInfo.SupportedStrategies. There is one correct value per instance type, which is what the recording carries (e.g. for m6g.medium: ebsOptimizedInfo baselineBandwidthInMbps 315, baselineIops 2500, baselineThroughputInMBps 39.375, maximumIops 20000, maximumEbsAttachments 27, networkInfo ipv4AddressesPerInterface 4, ipv6AddressesPerInterface 4, encryptionInTransitSupported false; for m8i-flex.16xlarge: dedicatedHostsSupported false, ebsInfo.attachmentLimitType 'dedicated', maximumEbsAttachments 48, ebsOptimizedInfo baseline/maximum 10000/20000 Mbps and 40000/80000 IOPS).
Emulator: it answers 200 but with values synthesized from the type name rather than the real catalog, so they differ per field and per type (replay examples: m6g.medium baselineBandwidthInMbps 287 vs 315, baselineIops 1500 vs 2500, baselineThroughputInMBps 35.875 vs 39.375, maximumIops 18750 vs 20000, maximumEbsAttachments 28 vs 27, ipv4/ipv6AddressesPerInterface 6 vs 4, encryptionInTransitSupported true vs false; m8i-flex.16xlarge dedicatedHostsSupported true vs false, attachmentLimitType 'shared' vs 'dedicated', maximumEbsAttachments 28 vs 48, ebsOptimizedInfo 13600/13600 Mbps and 60000/60000 IOPS vs 10000/20000 and 40000/80000).
Why: instance selection, capacity planning and validation logic read these numbers to choose a type (EBS/network headroom, ENI and IP budgets, clock speed, placement and dedicated-host support). A client sized against the emulator picks a type AWS would reject or under/over-provisions, and any test asserting a real type's published attributes fails against the emulator while passing against AWS.
Fix: the DescribeInstanceTypes instance-type catalog data the operation reads — replace the name-derived synthesis with the per-type facts; the handler's projection itself is otherwise shaped correctly.
Traces: ec2-catalog-describe-instance-types-c9gd-m6gd#task#43 (m6g.* page), ec2-catalog-describe-instance-types-m6i-r4#task#17 (m8i-flex.* page).

Cause 2 of 2: modeled instance-type members are omitted entirely.
AWS: the same InstanceTypeInfo closure models CpuBurstModel, NetworkInfo.DefaultNetworkCardIndex, NetworkInfo.EnaSrdSupported, NetworkInfo.FlexibleEnaQueuesSupport, NetworkInfo.BandwidthWeightings, NetworkInfo.ConnectionTrackingConfiguration.DefaultTcpEstablishedTimeout, PhcSupport, RebootMigrationSupport, NitroEnclavesSupport, NitroTpmSupport, AutoRecoverySupported and ProcessorInfo.SupportedFeatures; the recording shows AWS serializing them for every type in these pages (e.g. m6g.medium cpuBurstModel 'none', defaultNetworkCardIndex '0', enaSrdSupported 'false', flexibleEnaQueuesSupport 'unsupported'; m8i-flex.16xlarge networkInfo.bandwidthWeightings item ['default','ebs-1','vpc-1']).
Emulator: the members are absent from the serialized instanceTypeSet item (replay: 'missing (expected <value>, got None)').
Why: an SDK getter returns null where AWS returns a value, so a client cannot tell 'unsupported' from 'unknown'; feature gating on ENA Express, flexible ENA queues, PHC, reboot migration or Nitro Enclaves/TPM silently takes the wrong branch.
Fix: the DescribeInstanceTypes output serializer plus the catalog rows it reads — populate and emit these members instead of dropping them.
Traces: ec2-catalog-describe-instance-types-m6i-r4#task#17, ec2-catalog-describe-instance-types-c9gd-m6gd#task#43.

Open issues that describe this operation's catalog in general terms but do NOT carry these replay keys (the replay reported owned_by empty for all 46 clusters, so none of them covers these steps): 20261106T142039Z-ec2-describeinstancetypes-catalog-pages-still-diverge-per-instance-type.md, 20261106T122956Z-ec2-instance-type-catalog-pages-a1-c9g-through.md, 20261107T184734Z-ec2-describeinstancetypes-instance-type-catalog-returns-wrong-and.md, 20261104T201311Z-ec2-instance-type-catalog-is-synthesized-per-family.md. A fixer should re-replay the whole ec2-catalog-describe-instance-types-* family, not only the two steps named above, because the catalog is wrong per instance type.

## Evidence
Replay of trace set simparity-wire-aws-v1 (429 fixtures, 3941 steps, 1055 mismatched; 46 net-new DescribeInstanceTypes clusters, all with stub_path empty and owned_by empty).

ec2-catalog-describe-instance-types-c9gd-m6gd#task#43 — DescribeInstanceTypes, expected 200 / got 200, e.g. for instanceType="m6g.medium":
  .cpuBurstModel: missing (expected 'none', got None)
  .ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '315', got '287'
  .ebsInfo.ebsOptimizedInfo.baselineIops: expected '2500', got '1500'
  .ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps: expected '39.375', got '35.875'
  .ebsInfo.ebsOptimizedInfo.maximumIops: expected '20000', got '18750'
  .ebsInfo.maximumEbsAttachments: expected '27', got '28'
  .networkInfo.defaultNetworkCardIndex: missing (expected '0', got None)
  .networkInfo.enaSrdSupported: missing (expected 'false', got None)
  .networkInfo.encryptionInTransitSupported: expected 'false', got 'true'
  .networkInfo.flexibleEnaQueuesSupport: missing (expected 'unsupported', got None)
  .networkInfo.ipv4AddressesPerInterface: expected '4', got '6'
  .networkInfo.ipv6AddressesPerInterface: expected '4', got '6'
(the cluster's key list repeats the same field classes for m6g.large/xlarge/2xlarge/4xlarge/8xlarge/12xlarge/16xlarge/metal, adding phcSupport, rebootMigrationSupport, processorInfo.sustainedClockSpeedInGhz, networkInfo.networkCards/networkPerformance/maximumNetworkCards/secondaryNetworkSupported/trafficMirroringSupport, nitroEnclavesSupport and nitroTpmSupport)

ec2-catalog-describe-instance-types-m6i-r4#task#17 — DescribeInstanceTypes, expected 200 / got 200, e.g. for instanceType="m8i-flex.16xlarge":
  .cpuBurstModel: missing (expected 'none', got None)
  .dedicatedHostsSupported: expected 'false', got 'true'
  .ebsInfo.attachmentLimitType: expected 'dedicated', got 'shared'
  .ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '10000', got '13600'
  .ebsInfo.ebsOptimizedInfo.baselineIops: expected '40000', got '60000'
  .ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps: expected '1250.0', got '1700.0'
  .ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps: expected '20000', got '13600'
  .ebsInfo.ebsOptimizedInfo.maximumIops: expected '80000', got '60000'
  .ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps: expected '2500.0', got '1700.0'
  .ebsInfo.maximumEbsAttachments: expected '48', got '28'
  .networkInfo.bandwidthWeightings: missing (expected {'item': ['default','ebs-1','vpc-1']}, got None)
(the same cluster lists connectionTrackingConfiguration.defaultTcpEstablishedTimeout, defaultNetworkCardIndex, enaSrdSupported, flexibleEnaQueuesSupport, ipv4AddressesPerInterface, ipv4AddressesPerSecondaryInterface, ipv6AddressesPerInterface, maximumNetworkCards/Interfaces, networkCards, networkPerformance, secondaryNetworkSupported, trafficMirroringSupport, nitroEnclavesSupport, phcSupport, placementGroupInfo.supportedStrategies[0..2], processorInfo.supportedFeatures, processorInfo.sustainedClockSpeedInGhz and rebootMigrationSupport for every m8i-flex.* size)

## Steps to Reproduce
1. Replay fixture ec2-catalog-describe-instance-types-c9gd-m6gd and stop at step task#43 (DescribeInstanceTypes over the m6g.* family). Diff the response's instanceTypeSet items against the recording: ebsOptimizedInfo baseline/maximum bandwidth-IOPS-throughput, maximumEbsAttachments, networkInfo ipv4/ipv6AddressesPerInterface and encryptionInTransitSupported carry emulator-computed numbers, and cpuBurstModel, networkInfo.defaultNetworkCardIndex, networkInfo.enaSrdSupported and networkInfo.flexibleEnaQueuesSupport are absent from the XML altogether.
2. Replay fixture ec2-catalog-describe-instance-types-m6i-r4 and stop at step task#17 (m8i-flex.* page): dedicatedHostsSupported, ebsInfo.attachmentLimitType, maximumEbsAttachments and all six ebsOptimizedInfo numbers differ, and networkInfo.bandwidthWeightings, connectionTrackingConfiguration.defaultTcpEstablishedTimeout, phcSupport, rebootMigrationSupport, processorInfo.supportedFeatures and placementGroupInfo.supportedStrategies are missing.
3. Re-replay the rest of the ec2-catalog-describe-instance-types-* fixture family after the catalog is corrected: the sweep reported 46 net-new clusters of this operation, one per catalog page/step, so a per-family spot check is not sufficient.
