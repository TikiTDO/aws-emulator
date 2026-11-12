Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.broker-count-update-rejected-while-in-progress
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.broker-count-update-rejected-while-in-progress|run#20|AssertionError|assert over == ("NotFoundException", _NOT_FOUND_MESSAGE), f"over-range target must be a client fault, got {over}"
Issue-Commit: c8ee6f0e730dbbe032dcd7cd7a48f89a4679056d

# UpdateBrokerCount validates targetNumberOfBrokerNodes before resolving the cluster: an over-range target on a nonexistent cluster answers BadRequestException instead of NotFoundException

> _Note: this issue was identified against the code base at commit `c8ee6f0e730dbbe032dcd7cd7a48f89a4679056d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateBrokerCount

## Description
ONE net-new root cause (1 of 1): on MSK's UpdateBrokerCount path the emulator runs the input range validation for targetNumberOfBrokerNodes BEFORE it resolves the cluster named by the ClusterArn httpLabel, so a request that is both over-range and against a cluster that does not exist is refused as a syntactic validation failure rather than as a missing resource.

Cause 1 of 1: refusal order on UpdateBrokerCount is inverted (range check pre-empts the resource lookup).
AWS: Smithy kafka#UpdateBrokerCount, PUT /v1/clusters/{ClusterArn}/nodes/count, ClusterArn is a required @httpLabel; TargetNumberOfBrokerNodes (jsonName 'targetNumberOfBrokerNodes') targets __integerMin1Max15 with @range min 1 / max 15; modeled errors are BadRequestException(400), ForbiddenException(403), UnauthorizedException(401), InternalServerErrorException(500), ServiceUnavailableException(503) — NotFoundException is NOT in the modeled error list for this operation, so the not-found refusal is recorded behavior only. Recorded behavior: for a well-formed cluster ARN in the caller's own account that names no existing cluster, UpdateBrokerCount is refused with error code NotFoundException and message exactly "The requested resource doesn’t exist." — and that refusal is the SAME whether the target broker count is in range (4) or out of range (100): MSK resolves the cluster first and never reaches the targetNumberOfBrokerNodes range check for a cluster it cannot find. The message is entirely fixed text (note the U+2019 right single quotation mark in "doesn’t"); nothing is interpolated from the request.
Emulator: for the in-range target the emulator does answer NotFoundException with that exact message (that part of the check passes), but for TargetNumberOfBrokerNodes=100 on the same nonexistent cluster ARN it answers error code BadRequestException with message "1 validation error detected: Value '100' at 'targetNumberOfBrokerNodes' failed to satisfy constraint: Member must have value less than or equal to 15". Fixed text of that emulator message: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must have value less than or equal to "; its variable parts are the submitted value (100), the member wire name (targetNumberOfBrokerNodes) and the modeled maximum (15) — i.e. it is a shape-validation message emitted before any state lookup.
Why: a client cannot tell 'this cluster is gone' (stop, re-discover or re-create the cluster) from 'your scale-out request is malformed' (clamp/retry with a legal count), because which of the two it is told depends on a field value that is irrelevant once the cluster does not exist. Retry and reconciliation logic keyed on NotFoundException silently stops matching as soon as the desired broker count is out of range, and a caller that trusts the 400 will keep re-sending a corrected count against a cluster that will never exist.
Fix: in the kafka service crate, UpdateBrokerCount's handler must resolve the cluster named by the raw ClusterArn httpLabel and raise the NotFoundException refusal BEFORE its input validation evaluates the targetNumberOfBrokerNodes range; the range check must only decide the outcome once the cluster resolves. The sibling open issue 20261030T011042Z-kafka-msk-create-and-update-paths-answer-the-wrong.md records the OPPOSITE ordering fault on other paths (a resource lookup pre-empting MSK's own validation/authorization on CreateCluster/CreateReplicator/UpdateClusterConfiguration) and 20261029T202809Z-kafka-msk-create-and-update-inputs-aws-refuses-are.md records inputs accepted that AWS refuses; neither carries UpdateBrokerCount or this check, so this ordering is net-new and must be closed separately.
Traces: behavioral check kafka.broker-count-update-rejected-while-in-progress (assertion at the end of the check: "over-range target must be a client fault").

## Evidence
Suite: simparity-behavioral-aws-v1. Check kafka.broker-count-update-rejected-while-in-progress, verdict=mismatch, not marked ALREADY FILED and not OWNED by any open issue.

All earlier assertions of the check pass against the emulator: DescribeCluster on the ghost cluster ARN answers ('NotFoundException', 'The requested resource doesn’t exist.'); three concurrent UpdateBrokerCount calls with TargetNumberOfBrokerNodes=4 are all refused identically with ('NotFoundException', 'The requested resource doesn’t exist.'); DescribeClusterOperation on the never-issued operation ARN answers NotFoundException; ListClustersV2 does not list the ghost cluster.

The single divergence is the final step — UpdateBrokerCount(ClusterArn=<nonexistent, well-formed cluster ARN in the caller's account>, CurrentVersion=..., TargetNumberOfBrokerNodes=100):
  expected: ('NotFoundException', 'The requested resource doesn’t exist.')
  actual:   ('BadRequestException', "1 validation error detected: Value '100' at 'targetNumberOfBrokerNodes' failed to satisfy constraint: Member must have value less than or equal to 15")
AssertionError: over-range target must be a client fault, got ('BadRequestException', "1 validation error detected: Value '100' at 'targetNumberOfBrokerNodes' failed to satisfy constraint: Member must have value less than or equal to 15")

Smithy cited (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json): kafka#UpdateBrokerCount, PUT /v1/clusters/{ClusterArn}/nodes/count, code 200; input ClusterArn (httpLabel, required), CurrentVersion (jsonName currentVersion, required), TargetNumberOfBrokerNodes (jsonName targetNumberOfBrokerNodes, required, target __integerMin1Max15, range min 1 max 15); errors BadRequestException(400), ForbiddenException(403), UnauthorizedException(401), InternalServerErrorException(500), ServiceUnavailableException(503).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 suite.
2. Replay kafka.broker-count-update-rejected-while-in-progress.
3. Equivalently, by hand: build a well-formed MSK cluster ARN in the caller's own account and region that names no existing cluster (arn:aws:kafka:<region>:<account>:cluster/<unused-name>/00000000-0000-0000-0000-000000000000-1) and confirm DescribeCluster on it is refused NotFoundException / "The requested resource doesn’t exist.".
4. Call UpdateBrokerCount on that ARN with an in-range TargetNumberOfBrokerNodes (e.g. 4): observe NotFoundException with that fixed message (current emulator behavior, correct).
5. Call UpdateBrokerCount on the SAME ARN with an out-of-range TargetNumberOfBrokerNodes (e.g. 100): the emulator answers BadRequestException with the "1 validation error detected: Value '100' at 'targetNumberOfBrokerNodes' ..." message. It must answer the identical NotFoundException refusal ('The requested resource doesn’t exist.') that step 4 returned, because the cluster lookup precedes the range check.
6. Confirm the range check still applies once a cluster resolves: against an existing ACTIVE cluster, an out-of-range target must still be refused as a client validation fault, and no ClusterOperation may be created by either refusal.
7. Re-replay kafka.broker-count-update-rejected-while-in-progress; it must pass before submitting.
