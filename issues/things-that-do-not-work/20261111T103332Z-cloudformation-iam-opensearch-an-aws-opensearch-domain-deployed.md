Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__opensearch.cfn-opensearch-vpc-domain-in-place-update-then-not-found
Issue-Key: cloudformation__iam__opensearch.cfn-opensearch-vpc-domain-in-place-update-then-not-found|_assert_domain#19|AssertionError|assert got == want, f"DescribeDomain {domain_name}: {got!r}, expected {want!r}"
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# cloudformation__iam__opensearch: an AWS::OpenSearch::Domain deployed with VPCOptions loses its SecurityGroupIds — DescribeDomain reports VPCOptions.SecurityGroupIds absent

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Cause 1 of 1: the security groups given in a VPC domain's VPCOptions are dropped, so the domain's VPC configuration reads back incomplete.
AWS: OpenSearch DescribeDomain returns DomainStatus.VPCOptions with VPCId, SubnetIds, AvailabilityZones and SecurityGroupIds. For a domain created inside a VPC — directly or through an AWS::OpenSearch::Domain resource whose VPCOptions.SecurityGroupIds names a security group — SecurityGroupIds echoes exactly the security group ids that were supplied, because they are the groups attached to the domain's VPC endpoints and are what governs who can reach the endpoint. The member is present and non-empty for every VPC domain.
Emulator: DescribeDomain reports the rest of VPCOptions correctly (VPCId, SubnetIds, AvailabilityZones all match what was deployed) but SecurityGroupIds is absent/None, while the stack did declare a security group for the domain. The value supplied at create time is accepted and then not stored or not projected into the describe response.
Why: the security groups are the domain's only network-access control; a client that reads them back to verify or to reconcile its VPC wiring (which is exactly what a CloudFormation-deployed VPC domain is for) sees an unconfigured domain and cannot tell a dropped configuration from a domain that genuinely has no groups. Infrastructure-drift and conformance checks on a VPC domain's reachability cannot run at all.
Fix: the OpenSearch CreateDomain/UpdateDomainConfig state for VPCOptions.SecurityGroupIds and the DescribeDomain serializer that projects it (and, if it is the CloudFormation path that drops it, the AWS::OpenSearch::Domain provider's mapping of VPCOptions onto CreateDomain).
Traces: cloudformation__iam__opensearch.cfn-opensearch-vpc-domain-in-place-update-then-not-found.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### cloudformation__iam__opensearch` section (0/1 passed).

[mismatch] cloudformation__iam__opensearch.cfn-opensearch-vpc-domain-in-place-update-then-not-found
  AssertionError: DescribeDomain sim2real-osd-<id>: {... 'VPC': {'VPCId': 'vpc-<id>', 'SubnetIds': ['subnet-<id>'], 'AvailabilityZones': ['us-west-2a'], 'SecurityGroupIds': None}, ...}, expected {... 'VPC': {'VPCId': 'vpc-<id>', 'SubnetIds': ['subnet-<id>'], 'AvailabilityZones': ['us-west-2a'], 'SecurityGroupIds': ['sg-<id>']}, ...}

Every other projected member of the domain status (EngineVersion OpenSearch_2.19, InstanceType, InstanceCount, DedicatedMasterEnabled, ZoneAwarenessEnabled, EBS, EncryptionAtRest, NodeToNode, EnforceHTTPS, PublicEndpoint, EndpointKeys ['vpc']) matched; only VPCOptions.SecurityGroupIds diverges. Vpc/subnet/sg/domain ids above are per-run values — the requirement is that SecurityGroupIds echoes the ids the stack supplied.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. CreateStack with a VPC, a subnet, a security group and an AWS::OpenSearch::Domain whose VPCOptions names that subnet and that security group; wait for CREATE_COMPLETE.
3. Call OpenSearch DescribeDomain for the deployed domain and read DomainStatus.VPCOptions.
4. AWS reports SecurityGroupIds containing the deployed security group id; the emulator omits the member.
5. Replay cloudformation__iam__opensearch.cfn-opensearch-vpc-domain-in-place-update-then-not-found.
