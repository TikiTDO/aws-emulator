Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-vpc-attributes-and-account-settings
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-vpc-attributes-and-account-settings ec2-modify-verified-access-instance-contract ec2-public-ipv4-pool-lifecycle ec2-vm-migration-and-image-performance-task-tracking
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet.item.state
Issue-Key: ec2|DescribeVerifiedAccessInstances|2xx->2xx|$.DescribeVerifiedAccessInstancesResponse.verifiedAccessInstanceSet.item.verifiedAccessInstanceTransitGatewayAttachmentSet
Issue-Key: ec2|DescribePublicIpv4Pools|2xx->2xx|$.DescribePublicIpv4PoolsResponse.publicIpv4PoolSet.item.description
Issue-Key: ec2|DescribeFastLaunchImages|2xx->4xx|$.DescribeFastLaunchImagesResponse
Issue-Key: ec2|DescribeFastLaunchImages|2xx->4xx|status
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# EC2 VPC, verified-access, public-IPv4-pool and fast-launch reads omit modeled members or refuse a valid request

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
Four net-new wire divergences share one root cause: these read paths return a projection narrower than AWS's - a modeled member is omitted (or an empty collection member is dropped instead of being emitted empty), a resource is left in a non-terminal state, and one read refuses outright where AWS answers an empty 200. This issue bundles 4 causes; all 4 must be closed. The open issues '20261030T000411Z-ec2-create-and-describe-responses-omit-modeled-output.md' and '20261029T215142Z-ec2-describe-projections-omit-modeled-members-and-seed.md' describe the same family on other operations but do not carry these replay keys.

Cause 1 of 4: DescribeVpcs leaves a created VPC in state 'pending' and omits dhcpOptionsSource.
AWS: recorded behavior only for the member value - the recording shows vpcSet.item.state 'available' once CreateVpc has returned, and vpcSet.item.dhcpOptionsSource 'vpc'.
Emulator: state stays 'pending' and dhcpOptionsSource is absent.
Why: clients wait for state 'available' before using a VPC, so they block; and a client reading dhcpOptionsSource to decide whether DHCP options are VPC- or account-scoped sees no value at all.
Fix: the CreateVpc/DescribeVpcs state transition plus the Vpc projection in the DescribeVpcs handler.
Traces: ec2-vpc-attributes-and-account-settings#task#2.

Cause 2 of 4: DescribeVerifiedAccessInstances omits the transit-gateway attachment collection.
AWS: recorded behavior only (this checkout's EC2 model defines VerifiedAccessInstance with VerifiedAccessInstanceId, Description, VerifiedAccessTrustProviders, CreationTime, LastUpdatedTime, Tags, FipsEnabled, CidrEndpointsCustomSubDomain and does not list the member). The recording shows verifiedAccessInstanceSet.item.verifiedAccessInstanceTransitGatewayAttachmentSet present and empty.
Emulator: the member is absent entirely.
Why: a consumer that iterates the attachment set gets a null collection instead of an empty one, which is a different outcome for strict deserializers.
Fix: the DescribeVerifiedAccessInstances projection - empty collections must still be emitted.
Traces: ec2-modify-verified-access-instance-contract#task#3.

Cause 3 of 4: DescribePublicIpv4Pools omits the pool description.
AWS: Smithy: PublicIpv4Pool.Description (xmlName 'description') - 'A description of the address pool.' The recording shows the member present and empty for a pool created without a description.
Emulator: description is absent from publicIpv4PoolSet.item.
Why: same strict-deserializer/round-trip problem as cause 2, and a client that echoes the pool description has nothing to read.
Fix: the DescribePublicIpv4Pools projection.
Traces: ec2-public-ipv4-pool-lifecycle#task#2.

Cause 4 of 4: DescribeFastLaunchImages refuses a valid request.
AWS: Smithy: DescribeFastLaunchImages has no modeled errors (errors == []) and returns FastLaunchImages (xmlName 'fastLaunchImageSet'); the recording answers HTTP 200 with fastLaunchImageSet present and empty.
Emulator: HTTP 400 and no DescribeFastLaunchImagesResponse body at all.
Why: 'no fast-launch images configured' is a normal, empty-result condition; answering 400 turns a routine inventory read into a hard failure for the caller.
Fix: the DescribeFastLaunchImages handler - it must answer 200 with an empty fastLaunchImageSet when nothing matches.
Traces: ec2-vm-migration-and-image-performance-task-tracking#task#9.

## Evidence
Cause 1: $.DescribeVpcsResponse.vpcSet.item.state expected 'available', got 'pending'; $.DescribeVpcsResponse.vpcSet.item.dhcpOptionsSource expected 'vpc', got None.
Cause 2: $.DescribeVerifiedAccessInstancesResponse.verifiedAccessInstanceSet.item.verifiedAccessInstanceTransitGatewayAttachmentSet expected '' (present, empty), got None.
Cause 3: $.DescribePublicIpv4PoolsResponse.publicIpv4PoolSet.item.description expected '' (present, empty), got None.
Cause 4: status expected 200, got 400; $.DescribeFastLaunchImagesResponse expected {requestId, fastLaunchImageSet: ''}, got None.

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-vpc-attributes-and-account-settings#task#2, ec2-modify-verified-access-instance-contract#task#3, ec2-public-ipv4-pool-lifecycle#task#2 and ec2-vm-migration-and-image-performance-task-tracking#task#9, diffing each response body against the recorded one.
