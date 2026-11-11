Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.apply-pending-maintenance-action rds.rds-max-allocated-storage-echoes-and-refuses-a-threshold-under-ten-percent-above rds.serverless-v2-auto-pause-default-300-and-ceiling-echoed-rows-committed rds.serverless-v2-auto-pause-seconds-echoed-and-below-floor-refused
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.apply-pending-maintenance-action|run#14|AssertionError|assert bogus_code == ERR_RESOURCE_NOT_FOUND, f"unexpected error code: {bogus_code!r}"
Issue-Key: rds.rds-max-allocated-storage-echoes-and-refuses-a-threshold-under-ten-percent-above|run#8|AssertionError|raise AssertionError(
Issue-Key: rds.serverless-v2-auto-pause-default-300-and-ceiling-echoed-rows-committed|run#16|AssertionError|assert cluster.get("ServerlessV2ScalingConfiguration") == defaulted, (
Issue-Key: rds.serverless-v2-auto-pause-seconds-echoed-and-below-floor-refused|run#20|AssertionError|assert error.get("Message") == _BELOW_FLOOR_MESSAGE, (
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# rds: four control-plane validation and representation divergences (ApplyPendingMaintenanceAction not-found code, MaxAllocatedStorage 10% floor, Serverless v2 auto-pause default and refusal text)

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyPendingMaintenanceAction

## Description
4 net-new root causes in the RDS control plane: one wrong error class, one missing validation, one missing output default, one wrong refusal message. Each has its own failing check.

Cause 1 of 4: ApplyPendingMaintenanceAction on a well-formed ARN for a DB instance that does not exist is refused with the wrong error class.
AWS: Smithy binds ApplyPendingMaintenanceAction's errors to InvalidDBClusterStateFault, InvalidDBInstanceStateFault and ResourceNotFoundFault; ResourceNotFoundFault carries httpError 404 and awsQueryError code `ResourceNotFoundFault`. A syntactically valid ARN naming a nonexistent resource is a not-found, not a malformed parameter, and the message's fixed text is `Resource not found: ` followed by the resource identifier taken from the request's ResourceIdentifier ARN.
Emulator: the same call answers error code `InvalidParameterValue` (HTTP 400), so the request is classified as malformed rather than as a missing resource; the positive control on a real, available instance does NOT come back not-found, so the handler is specifically mis-classifying the missing-resource case.
Why: a client cannot distinguish "this resource is gone" from "you sent me a bad ARN". Retry/ignore logic keyed on ResourceNotFoundFault (idempotent teardown, maintenance sweepers) never fires, and a 400 InvalidParameterValue reads as a caller bug the client will not recover from, instead of the 404 AWS returns.
Fix: the not-found path of the ApplyPendingMaintenanceAction handler — ARN resolution must raise ResourceNotFoundFault (404) with `Resource not found: <identifier>` when the ARN parses but names no DB instance/cluster.
Traces: rds.apply-pending-maintenance-action.

Cause 2 of 4: CreateDBInstance accepts a storage-autoscaling threshold below the documented 10% floor.
AWS: MaxAllocatedStorage is the upper limit RDS can autoscale storage to; the Smithy member documents only the setting and points at the storage-autoscaling user-guide page, so the floor itself is AWS-documented/recorded behavior: the maximum storage threshold must be at least 10 percent above AllocatedStorage. A value under that is refused with error code `InvalidParameterCombination` and the message, whose text is fixed with no variable part, `The maximum allocated storage must be increased by at least 10 percent.`; the refused create leaves no DB instance behind (DescribeDBInstances answers not-found for the identifier).
Emulator: CreateDBInstance with AllocatedStorage=20 and MaxAllocatedStorage=21 (105% of the allocation) is ACCEPTED and the instance is created. No validation of the threshold against the allocation is performed.
Why: the emulator is over-permissive on a documented input constraint. Code that is supposed to be rejected at create time instead provisions an instance whose autoscaling ceiling AWS would never allow, so a template or script that is invalid against AWS passes here and fails in production — the exact inversion a fidelity emulator must not have.
Fix: input validation in the CreateDBInstance handler (and the same rule on ModifyDBInstance), comparing MaxAllocatedStorage against AllocatedStorage before the instance record is created.
Traces: rds.rds-max-allocated-storage-echoes-and-refuses-a-threshold-under-ten-percent-above.

Cause 3 of 4: an omitted SecondsUntilAutoPause is not defaulted in the described Serverless v2 scaling configuration.
AWS: Smithy ServerlessV2ScalingConfigurationInfo.SecondsUntilAutoPause documents "This property is only shown when the minimum capacity for the cluster is set to 0 ACUs … This value ranges between 300 seconds (five minutes) and 86,400 seconds (one day). The default is 300 seconds." So for a cluster created with MinCapacity 0 and SecondsUntilAutoPause omitted, DescribeDBClusters must report the member with the service default 300.
Emulator: DescribeDBClusters returns ServerlessV2ScalingConfiguration = {'MinCapacity': 0.0, 'MaxCapacity': 1.0} — the SecondsUntilAutoPause member is absent instead of present with 300.
Why: a client reading back its own cluster cannot learn the idle period the service will actually apply; code that reads the field to display or re-submit the configuration sees it as unset and will either crash on the missing key or re-submit a configuration that differs from the live one.
Fix: output defaulting for ServerlessV2ScalingConfigurationInfo in the DB-cluster serializer/state (populate SecondsUntilAutoPause=300 whenever MinCapacity is 0 and no value was supplied).
Traces: rds.serverless-v2-auto-pause-default-300-and-ceiling-echoed-rows-committed.

Cause 4 of 4: the below-floor SecondsUntilAutoPause refusal uses the emulator's own wording, not AWS's.
AWS: a SecondsUntilAutoPause outside the modeled 300–86,400 second range is refused with the message whose text is entirely fixed — `SecondsUntilAutoPause must be from 300 to 86400 seconds.` — with no part interpolated from the request; the range 300..86400 is the Smithy-documented domain of the member.
Emulator: the same request is refused with `invalid value 299 for ServerlessV2ScalingConfiguration.SecondsUntilAutoPause: must be a value from 300 to 86400 seconds` — different fixed text, different punctuation, and it interpolates the offending value and the member path, which AWS's message does not. (The `299` in the recorded string is the request's own value, not part of the requirement; the requirement is that the message is AWS's fixed sentence.)
Why: a client that matches on the refusal message — error-message assertions, user-facing mapping of a validation failure, log-based alerting — does not match, and a request AWS and the emulator both refuse is reported with incompatible text.
Fix: the validation message for ServerlessV2ScalingConfiguration.SecondsUntilAutoPause in the CreateDBCluster/ModifyDBCluster validation path.
Traces: rds.serverless-v2-auto-pause-seconds-echoed-and-below-floor-refused.

## Evidence
Cause 1 — rds.apply-pending-maintenance-action:
  Expected error code: 'ResourceNotFoundFault' (HTTP 404) with message `Resource not found: <bogus-instance-id>`.
  Actual: AssertionError: unexpected error code: 'InvalidParameterValue'.
  Positive control: the same call against the real, available instance did NOT return ResourceNotFoundFault, so the handler is not blanket-refusing.

Cause 2 — rds.rds-max-allocated-storage-echoes-and-refuses-a-threshold-under-ten-percent-above:
  Expected: ClientError InvalidParameterCombination, message `The maximum allocated storage must be increased by at least 10 percent.`, and no instance left behind.
  Actual: AssertionError: CreateDBInstance accepted MaxAllocatedStorage=21 with AllocatedStorage=20; the threshold must be at least 10% more. (the call returned 200 and created the instance)

Cause 3 — rds.serverless-v2-auto-pause-default-300-and-ceiling-echoed-rows-committed:
  Expected DescribeDBClusters ServerlessV2ScalingConfiguration: {'MinCapacity': 0.0, 'MaxCapacity': 1.0, 'SecondsUntilAutoPause': 300}
  Actual:                                                        {'MinCapacity': 0.0, 'MaxCapacity': 1.0}

Cause 4 — rds.serverless-v2-auto-pause-seconds-echoed-and-below-floor-refused:
  Expected message: 'SecondsUntilAutoPause must be from 300 to 86400 seconds.'
  Actual message:   'invalid value 299 for ServerlessV2ScalingConfiguration.SecondsUntilAutoPause: must be a value from 300 to 86400 seconds'

## Steps to Reproduce
Cause 1 (rds.apply-pending-maintenance-action): create a mysql db.t3.micro instance and wait for `available`; call ApplyPendingMaintenanceAction(ResourceIdentifier=<its ARN>, ApplyAction='system-update', OptInType='immediate') and confirm it is not refused not-found; then call it with a well-formed ARN `arn:aws:rds:<region>:<account>:db:<identifier-that-does-not-exist>` and read Error.Code/Error.Message.

Cause 2 (rds.rds-max-allocated-storage-…): CreateDBInstance(Engine=postgres, DBInstanceClass=db.t3.micro, StorageType=gp2, AllocatedStorage=20, MaxAllocatedStorage=21, …) and observe it succeeds instead of being refused InvalidParameterCombination.

Cause 3 (rds.serverless-v2-auto-pause-default-300-…): CreateDBCluster for Aurora Serverless v2 with ServerlessV2ScalingConfiguration={MinCapacity: 0.0, MaxCapacity: 1.0} and SecondsUntilAutoPause omitted; wait for `available`; DescribeDBClusters and read ServerlessV2ScalingConfiguration.

Cause 4 (rds.serverless-v2-auto-pause-seconds-echoed-and-below-floor-refused): issue the same create/modify with ServerlessV2ScalingConfiguration.SecondsUntilAutoPause=299 and read Error.Message.
