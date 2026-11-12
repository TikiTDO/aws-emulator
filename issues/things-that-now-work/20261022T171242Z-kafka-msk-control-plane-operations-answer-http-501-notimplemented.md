Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-describe-not-found kafka.configuration-invalid-server-properties-rejected kafka.configuration-concurrent-create-same-name-race kafka.create-vpc-connection-forbidden-without-permission kafka.describe-topic-partitions-malformed-name-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-describe-not-found|run#3|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: kafka.configuration-invalid-server-properties-rejected|run#5|AssertionError|assert code == "BadRequestException", (
Issue-Key: kafka.configuration-concurrent-create-same-name-race|run#9|AssertionError|assert len(created) == 1, f"expected exactly 1 winner, got {len(created)}: {created}"
Issue-Key: kafka.create-vpc-connection-forbidden-without-permission|run#18|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: kafka.describe-topic-partitions-malformed-name-rejected|run#9|AssertionError|assert malformed_code == "AccessDeniedException", f"got={malformed_code}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# MSK control-plane operations answer HTTP 501 NotImplemented where AWS answers a modeled client error (4 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterV2

## Description
FOUR distinct root causes, all of the same kind: an MSK operation that AWS answers with a 4xx client error answers 501 NotImplemented instead, so the rejection the client depends on never happens. All four must be closed.

Cause 1 of 4 — DescribeClusterV2 on a well-formed but nonexistent ClusterArn.
AWS: Smithy kafka#DescribeClusterV2 (GET /api/v2/clusters/{ClusterArn}) models NotFoundException (httpError 404) among its errors; the recorded response for an ARN that was never issued is HTTP 404 with Error.Code 'NotFoundException' (recorded message, as an example: 'The requested resource doesn\u2019t exist.').
Emulator: HTTP 501 with Error.Code 'NotImplemented' — 'operation DescribeClusterV2 is scaffolded but not implemented in this emulator'.
Why: a client cannot distinguish 'no such cluster' from 'service broken'; 501 is not retried or handled by any not-found branch, and botocore surfaces it as an unmodeled error so exception-type dispatch on NotFoundException fails.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/describe_cluster_v2.rs. NOTE: the suite also reports DescribeClusterV2 as an already-filed scaffold stub blocking kafka.deploy-msk-serverless-cfn-topic-createable; that filing does NOT carry this check's replay key, so kafka.cluster-describe-not-found must be re-replayed here.
Traces: kafka.cluster-describe-not-found.

Cause 2 of 4 — CreateConfiguration never validates and never conflicts.
AWS: Smithy kafka#CreateConfiguration (POST /v1/configurations) models BadRequestException (400) and ConflictException (409). Recorded behavior: ServerProperties that is not a valid Java properties document is refused with HTTP 400 Error.Code 'BadRequestException' and no configuration is created (the name is absent from ListConfigurations); a valid payload returns 200 with State 'ACTIVE' and LatestRevision.Revision 1; and four concurrent CreateConfiguration calls with one identical Name yield exactly one 200 winner and three HTTP 409 'ConflictException' losers (configuration names are a uniqueness namespace).
Emulator: every CreateConfiguration answers HTTP 501 'NotImplemented', so the malformed payload is neither accepted nor refused with the modeled code, and the race produces zero winners instead of one.
Why: clients that create MSK configurations idempotently rely on ConflictException to detect the loser of a race and on BadRequestException to surface a bad properties file; both become a hard 501.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs — ServerProperties parsing/validation, name-uniqueness serialization, and the ACTIVE/revision-1 initial state. NOTE: CreateConfiguration is also reported as an already-filed scaffold stub blocking 24 other checks; that filing does not carry these two replay keys.
Traces: kafka.configuration-invalid-server-properties-rejected, kafka.configuration-concurrent-create-same-name-race.

Cause 3 of 4 — CreateVpcConnection is not authorized, it is unimplemented.
AWS: recorded behavior only for the status/code — a principal with an explicit identity-based Deny on kafka:CreateVpcConnection is refused by request authorization BEFORE the MSK control plane sees the request, with Error.Code 'AccessDeniedException' (the generic IAM code; kafka#CreateVpcConnection itself models only BadRequest/Forbidden/InternalServerError/ServiceUnavailable/TooManyRequests/Unauthorized, which is why the refusal is not ForbiddenException). The recorded message is request-derived — 'User: <assumed-role arn> is not authorized to perform: kafka:CreateVpcConnection on resource: <TargetClusterArn> with an explicit deny in an identity-based policy' — so the rule, not the recorded text, is the requirement.
Emulator: HTTP 501 'NotImplemented' for CreateVpcConnection, so the denied call is neither authorized nor refused, and the check's positive control (the same identity calling a non-denied action) is never reached.
Why: an emulator that answers 501 here cannot be used to test least-privilege policies: a deny that AWS enforces is indistinguishable from an unimplemented endpoint, and after the Deny is removed the identical call still fails.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/create_vpc_connection.rs plus the request-authorization layer that must evaluate the caller's identity policy before the handler runs.
Traces: kafka.create-vpc-connection-forbidden-without-permission.

Cause 4 of 4 — DescribeTopicPartitions does not gate on ARN shape.
AWS: Smithy kafka#DescribeTopicPartitions (GET /v1/clusters/{ClusterArn}/topics/{TopicName}/partitions) models BadRequestException (400) and NotFoundException (404). Recorded behavior: authorization is evaluated ahead of any resource lookup, so a ClusterArn whose resource segment is not of the form 'cluster/<name>/<uuid>' cannot be matched against the caller's resource-scoped permissions and is refused with Error.Code 'AccessDeniedException', while a well-formed-but-nonexistent cluster ARN passes that gate and fails with 'NotFoundException' — two separate, ordered gates.
Emulator: HTTP 501 'NotImplemented', so both gates collapse into one unimplemented endpoint and the ordering cannot be observed.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/describe_topic_partitions.rs — ARN-shape/authorization gate first, then cluster resolution.
Traces: kafka.describe-topic-partitions-malformed-name-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, 0/192 passed; kafka 0/191.

Cause 1 — kafka.cluster-describe-not-found:
  msk.describe_cluster_v2(ClusterArn='arn:aws:kafka:us-west-2:<acct>:cluster/sim2real-behav-nonexistent-<rand>/00000000-0000-0000-0000-000000000000-1')
  expected: HTTP 404, Error.Code 'NotFoundException'
  actual:   AssertionError: expected NotFoundException, got NotImplemented  (HTTP 501, 'operation DescribeClusterV2 is scaffolded but not implemented in this emulator')

Cause 2 — kafka.configuration-invalid-server-properties-rejected:
  kafka.create_configuration(Name=<new>, ServerProperties=b'this line has no equals sign and is not valid java properties syntax')
  expected: HTTP 400, Error.Code 'BadRequestException', and the name absent from ListConfigurations
  actual:   AssertionError: expected BadRequestException for malformed ServerProperties, got 'NotImplemented'
kafka.configuration-concurrent-create-same-name-race:
  4 parallel kafka.create_configuration(Name=<same>, KafkaVersions=['3.6.0'], ServerProperties=b'num.partitions=1\n')
  expected: exactly 1 HTTP 200 winner (LatestRevision.Revision == 1) and 3 x HTTP 409 'ConflictException'
  actual:   AssertionError: expected exactly 1 winner, got 0: []

Cause 3 — kafka.create-vpc-connection-forbidden-without-permission:
  role with AmazonMSKFullAccess attached plus an inline Deny on kafka:CreateVpcConnection calls
  create_vpc_connection(TargetClusterArn=<well-formed, nonexistent>, Authentication='IAM', VpcId='vpc-0...', ClientSubnets=['subnet-0...'], SecurityGroups=['sg-0...'])
  expected: Error.Code 'AccessDeniedException'
  actual:   AssertionError: expected AccessDeniedException, got 'NotImplemented'  (HTTP 501)

Cause 4 — kafka.describe-topic-partitions-malformed-name-rejected:
  describe_topic_partitions(ClusterArn=<resource segment not 'cluster/<name>/<uuid>'>, TopicName=<any>)
  expected: Error.Code 'AccessDeniedException' (and 'NotFoundException' for the well-formed nonexistent ARN control)
  actual:   AssertionError: got=NotImplemented  (HTTP 501)

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: kafka.describe_cluster_v2(ClusterArn='arn:aws:kafka:<region>:<account>:cluster/does-not-exist/00000000-0000-0000-0000-000000000000-1') -> expect HTTP 404 NotFoundException; observe HTTP 501 NotImplemented.
3. Cause 2a: kafka.create_configuration(Name='cfg-bad', ServerProperties=b'no equals sign here') -> expect HTTP 400 BadRequestException and that 'cfg-bad' never appears in list_configurations; observe 501. Then kafka.create_configuration(Name='cfg-good', ServerProperties=b'auto.create.topics.enable=true\n') -> expect 200 with State 'ACTIVE'.
4. Cause 2b: issue four concurrent create_configuration calls with one identical Name -> expect 1 success and 3 x HTTP 409 ConflictException; observe 0 successes.
5. Cause 3: create an IAM role with AmazonMSKFullAccess plus an inline Deny on kafka:CreateVpcConnection, assume it, and call kafka.create_vpc_connection(TargetClusterArn=<well-formed nonexistent cluster arn>, Authentication='IAM', VpcId='vpc-00000000000000000', ClientSubnets=['subnet-00000000000000000'], SecurityGroups=['sg-00000000000000000']) -> expect AccessDeniedException; observe 501 NotImplemented. Remove the Deny and repeat -> expect a non-AccessDenied service error.
6. Cause 4: kafka.describe_topic_partitions(ClusterArn='arn:aws:kafka:<region>:<account>:cluster/not-a-valid-shape', TopicName='t') -> expect AccessDeniedException; then the same call with a well-formed nonexistent cluster ARN -> expect NotFoundException; observe 501 for both.
7. Re-replay all five check ids in trace_ids.
