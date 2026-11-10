Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateReplicationGroup|4xx->2xx|$.ErrorResponse
Issue-Key: elasticache|CreateReplicationGroup|4xx->2xx|status
Issue-Key: elasticache|DescribeReplicationGroups|4xx->2xx|$.ErrorResponse
Issue-Key: elasticache|DescribeReplicationGroups|4xx->2xx|status
Issue-Commit: 85304fbaaec1e137da409d02b027804fcd09632b

# ElastiCache replication-group refusals never raised: Create accepts an unknown security group and Describe answers 200 for a missing group

> _Note: this issue was identified against the code base at commit `85304fbaaec1e137da409d02b027804fcd09632b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationGroup

## Description
2 net-new wire causes on the elasticache replication-group path: two refusals AWS raises are never raised at all - the emulator answers HTTP 200 for both an invalid cross-service reference and a lookup of a group that does not exist. The replay marked neither step as owned by an open issue. Related open issues (20261108T093128Z-ec2-elasticache-elasticache-in-vpc-control-plane-diverges-from-aws.md, 20261102T083323Z-elasticache-refusals-carry-the-wrong-error-code-and.md) describe other CreateReplicationGroup refusals that carry the wrong code or message text, but neither covers these two missing refusals nor carries these fixtures' replay keys.

Cause 1 of 2: CreateReplicationGroup does not validate SecurityGroupIds against EC2. AWS: Smithy - CreateReplicationGroup.SecurityGroupIds targets SecurityGroupIdsList and the operation models InvalidParameterValueException (awsQueryError code InvalidParameterValue, HTTP 400). Recorded: a request whose SecurityGroupIds names a security group that does not exist in the account is refused HTTP 400, awsQuery Error with Type 'Sender', Code 'InvalidParameterValue' and the message 'Some security group Id not recognized by EC2: securityGroupIds[[<security-group-ids>]], awsAccountId[<account-id>]' - the fixed text is 'Some security group Id not recognized by EC2: securityGroupIds[[', ']], awsAccountId[' and ']'; the variable parts are the unrecognized SecurityGroupIds from the request and the caller's account id from the request's credentials. No replication group is created. Emulator: HTTP 200 with a ReplicationGroup body and no ErrorResponse at all - 'status: expected 400, got 200' and '$.ErrorResponse: missing'. Why: a client that mistypes or stales a security group id is told its cluster is being created; it then builds a group whose network access is not what it asked for, and the error path it tests for (InvalidParameterValue naming the bad ids) never fires. Fix: CreateReplicationGroup's validation in the elasticache handler - resolve every SecurityGroupIds member against the EC2 security-group state before writing any replication-group state, and raise InvalidParameterValueException with AWS's message template. Traces: elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#task#11, elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#5.

Cause 2 of 2: DescribeReplicationGroups answers 200 for an unknown ReplicationGroupId. AWS: Smithy - DescribeReplicationGroups models ReplicationGroupNotFoundFault (errorKind client, awsQueryError code ReplicationGroupNotFoundFault, HTTP 404, doc 'The specified replication group does not exist.'). Recorded: a describe naming a ReplicationGroupId that does not exist is refused HTTP 404 with Error.Type 'Sender', Error.Code 'ReplicationGroupNotFoundFault' and the message 'ReplicationGroup <replication-group-id> not found.' - the fixed text is 'ReplicationGroup ' and ' not found.'; the variable part is the ReplicationGroupId from the request. Emulator: HTTP 200 with no ErrorResponse - 'status: expected 404, got 200'. Why: a client that uses DescribeReplicationGroups as its existence check (before create, after delete, or while polling a teardown) reads success for a group that is not there, so it never detects deletion and never retries creation; code branching on ReplicationGroupNotFoundFault is unreachable. Fix: the DescribeReplicationGroups handler's lookup in elasticache - a ReplicationGroupId filter that matches nothing must raise ReplicationGroupNotFoundFault (404) rather than returning an empty/synthesised 200 result. Traces: elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#task#13, elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#9.

## Evidence
Both quotes are from the wire replay of this corpus (verdict: mismatch).

Cause 1 - fixture elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes, step task#11, operation CreateReplicationGroup (cluster also covers elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#5); expected status 400, actual status 200:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'Some security group Id not recognized by EC2: securityGroupIds[[<sg-id>]], awsAccountId[<account-id>]'}, 'RequestId': <shaped-not-asserted>}, got None)

Cause 2 - fixture elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes, step task#13, operation DescribeReplicationGroups (cluster also covers elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#9); expected status 404, actual status 200:
  status: expected 404, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ReplicationGroupNotFoundFault', 'Message': 'ReplicationGroup <replication-group-id> not found.'}, 'RequestId': <shaped-not-asserted>}, got None)

The security-group id, account id and replication-group id above are request/state values that fill AWS's message template; the status, error code and the template's fixed text are the requirement.

Smithy cited: CreateReplicationGroup errors include InvalidParameterValueException (code InvalidParameterValue, 400); DescribeReplicationGroups errors include ReplicationGroupNotFoundFault (code ReplicationGroupNotFoundFault, 404).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: call elasticache CreateReplicationGroup with a valid ReplicationGroupId/description/CacheNodeType and SecurityGroupIds naming a security group id that does not exist in the account - observe HTTP 200 and a ReplicationGroup body where AWS answers 400 InvalidParameterValue 'Some security group Id not recognized by EC2: securityGroupIds[[<sg-id>]], awsAccountId[<account-id>]' and creates nothing. Replay fixture elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes step task#11 (and elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes step task#5).
3. Cause 2: call DescribeReplicationGroups with a ReplicationGroupId that was never created - observe HTTP 200 where AWS answers 404 ReplicationGroupNotFoundFault 'ReplicationGroup <id> not found.'. Replay fixture elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes step task#13 (and elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes step task#9).
