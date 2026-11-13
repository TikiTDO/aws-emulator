Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution
Issue-Step: setup#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes elasticache-vvd-traj-replication-group-id-and-topology-mistakes elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes elasticache-vvd-traj-cache-subnet-group-build-mistakes
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateCacheSubnetGroup|2xx->4xx|status
Issue-Key: elasticache|DescribeCacheSubnetGroups|2xx->4xx|$.DescribeCacheSubnetGroupsResponse
Issue-Key: elasticache|DescribeCacheSubnetGroups|2xx->4xx|status
Issue-Key: elasticache|CreateCacheSubnetGroup|2xx->4xx|$.CreateCacheSubnetGroupResponse
Issue-Key: elasticache|DescribeCacheEngineVersions|2xx->2xx|$.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion.CacheEngineDescription
Issue-Key: elasticache|DescribeCacheEngineVersions|2xx->2xx|$.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion.CacheEngineVersionDescription
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# ElastiCache cache-subnet-group creation and engine-version catalog diverge from AWS on the wire

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheSubnetGroup

## Description
Two independent root causes on the IMPLEMENTED ElastiCache surface: a valid CreateCacheSubnetGroup request is refused outright (so the whole VPC cache topology cannot be built, and the dependent DescribeCacheSubnetGroups refuses too), and the seeded engine-version catalog serves abbreviated description strings. Each must be closed separately.

Cause 1 of 2: CreateCacheSubnetGroup answers HTTP 400 for subnet ids the account actually owns, so no subnet group is ever created and the following DescribeCacheSubnetGroups also fails.
AWS: Smithy: CreateCacheSubnetGroup input is CacheSubnetGroupName, CacheSubnetGroupDescription and SubnetIds (SubnetIdentifierList), all required; output CreateCacheSubnetGroupResult.CacheSubnetGroup. The only modeled refusals are CacheSubnetGroupAlreadyExistsFault, CacheSubnetGroupQuotaExceededFault, CacheSubnetQuotaExceededFault, InvalidSubnet, SubnetNotAllowedFault and TagQuotaPerResourceExceeded (all HTTP 400) — InvalidSubnet is for ids that do not name a subnet. Recorded behavior: subnet ids that EC2 CreateSubnet just returned in the caller's VPC are ACCEPTED with HTTP 200, and the response carries CacheSubnetGroup with CacheSubnetGroupName, CacheSubnetGroupDescription, VpcId, Subnets (each SubnetIdentifier plus SubnetAvailabilityZone.Name and SupportedNetworkTypes), SupportedNetworkTypes and ARN. DescribeCacheSubnetGroups for that name then answers HTTP 200 with the same group. (Names, vpc/subnet ids, AZs and the ARN account are per-run values; the requirement is 200 with the requested subnets and their zones projected into the group.)
Emulator: CreateCacheSubnetGroup answers HTTP 400 and no CreateCacheSubnetGroupResponse body at all, for exactly the subnets EC2 in this emulator just minted. The dependent DescribeCacheSubnetGroups for that name then also answers HTTP 400 with no DescribeCacheSubnetGroupsResponse, because the group does not exist.
Why: this is the first call of every VPC cache topology. A client that creates a VPC and subnets and then places a cache in them is blocked before it can reach CreateCacheCluster or CreateReplicationGroup; the failure also cascades into every later step of the fixture that needs the group.
Fix: the subnet resolution used by the CreateCacheSubnetGroup handler in the elasticache crate (crate::state::resolve_subnet / region_subnets) must resolve a subnet id against the subnets the EC2 service holds, and keep refusing only ids that name nothing. Related open issue 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md describes the same root cause from the behavioral suite (its Cause 1) but carries no wire replay keys and does not own these fixture steps (the replay report lists owned_by as empty for all three); this issue carries the wire keys.
Traces: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#setup#4 (cluster also covers setup#4 of elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes, elasticache-vvd-traj-replication-group-id-and-topology-mistakes and elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes), elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#task#5 (cluster also covers #task#6 and elasticache-vvd-traj-cache-subnet-group-build-mistakes#task#11), elasticache-vvd-traj-cache-subnet-group-build-mistakes#task#9.

Cause 2 of 2: DescribeCacheEngineVersions serves abbreviated CacheEngineDescription and CacheEngineVersionDescription for the Valkey engine.
AWS: Smithy: DescribeCacheEngineVersions returns CacheEngineVersionMessage.CacheEngineVersions, a list of CacheEngineVersion whose members include Engine, EngineVersion, CacheParameterGroupFamily, CacheEngineDescription and CacheEngineVersionDescription; the operation models no errors. Recorded behavior for the Valkey engine: CacheEngineDescription is the full engine label 'Valkey Engine for ElastiCache', and CacheEngineVersionDescription is '<engine> version <full patch version>' — i.e. the description carries the patch-level version (8.0.1), not the minor-version family.
Emulator: HTTP 200, but CacheEngineDescription is 'Valkey' and CacheEngineVersionDescription is 'valkey version 8.0' — the engine label is truncated to the bare engine token and the version description is truncated to the minor family.
Why: these are the human-readable fields a client displays in an engine picker, and the version description is the only place the patch level is published for the entry; a client that renders or parses them shows or infers the wrong engine and the wrong version.
Fix: the seeded elasticache engine-version reference catalog that feeds DescribeCacheEngineVersions (the description strings per engine/version entry). Open issue 20261030T130325Z-elasticache-reference-catalogs-diverge-from-aws-engine-versions.md covers other fields of this catalog and does not own this step, so this cause is net-new.
Traces: elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#8.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against this checkout (230 steps, 150 passed, 80 mismatched, score 65.2%). Per cause, expected = recorded AWS response, actual = emulator:
Cause 1 — CreateCacheSubnetGroup, fixture elasticache-tgwc-traj-cache-subnet-group-vpc-resolution step setup#4 (cluster of 4): 'status: expected 200, got 400'. Same operation, fixture elasticache-vvd-traj-cache-subnet-group-build-mistakes step task#9: 'status: expected 200, got 400' and '$.CreateCacheSubnetGroupResponse: missing' where the recording holds CreateCacheSubnetGroupResult.CacheSubnetGroup with CacheSubnetGroupName, CacheSubnetGroupDescription, VpcId, two Subnets (SubnetIdentifier + SubnetAvailabilityZone.Name + SupportedNetworkTypes), SupportedNetworkTypes {member: ipv4} and ARN arn:aws:elasticache:<region>:<account>:subnetgroup:<name>. Downstream: DescribeCacheSubnetGroups, fixture elasticache-tgwc-traj-cache-subnet-group-vpc-resolution step task#5 (cluster of 3): 'status: expected 200, got 400' and '$.DescribeCacheSubnetGroupsResponse: missing' where the recording holds DescribeCacheSubnetGroupsResult.CacheSubnetGroups.CacheSubnetGroup for the same group.
Cause 2 — DescribeCacheEngineVersions, fixture elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes step task#8, status 200 == 200: $.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion.CacheEngineVersionDescription expected 'valkey version 8.0.1', got 'valkey version 8.0'; .CacheEngineDescription expected 'Valkey Engine for ElastiCache', got 'Valkey'.
Model evidence (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json): CreateCacheSubnetGroup requires SubnetIds (SubnetIdentifierList) and models only AlreadyExists/QuotaExceeded/InvalidSubnet/SubnetNotAllowed/TagQuota faults; DescribeCacheEngineVersions returns CacheEngineVersionList and models no errors.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire trace set simparity-wire-aws-v1.
2. Cause 1: ec2 CreateVpc(10.0.0.0/16), then ec2 CreateSubnet once per availability zone; take the returned subnet ids and call elasticache CreateCacheSubnetGroup(CacheSubnetGroupName=<name>, CacheSubnetGroupDescription=<text>, SubnetIds=[those ids]) -> expect HTTP 200 with CacheSubnetGroup carrying those subnets, their AZ names and the group ARN; observe HTTP 400 with no CreateCacheSubnetGroupResponse. Then call elasticache DescribeCacheSubnetGroups(CacheSubnetGroupName=<name>) -> expect 200 with that group; observe 400 with no body. Re-replay elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#setup#4, elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#task#5 and elasticache-vvd-traj-cache-subnet-group-build-mistakes#task#9.
3. Cause 2: elasticache DescribeCacheEngineVersions(Engine='valkey', EngineVersion='8.0') -> expect CacheEngineDescription 'Valkey Engine for ElastiCache' and CacheEngineVersionDescription naming the full patch version ('valkey version 8.0.1'); observe 'Valkey' and 'valkey version 8.0'. Re-replay elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#8.
