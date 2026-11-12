Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.asghc-target-group-health-check-interval-lowered-below-the-stored-timeout-rejected
Issue-Key: ec2__elbv2.asghc-target-group-health-check-interval-lowered-below-the-stored-timeout-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# ec2__elbv2: ModifyTargetGroup accepts a HealthCheckIntervalSeconds lowered below the target group's stored HealthCheckTimeoutSeconds

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyTargetGroup

## Description
Cause 1 of 1: ModifyTargetGroup does not validate a newly supplied health-check interval against the health-check timeout already stored on the target group.
AWS: Smithy: ModifyTargetGroup declares InvalidConfigurationRequestException (awsQueryError code `InvalidConfigurationRequest`, HTTP 400); HealthCheckIntervalSeconds is @range 5-300 and HealthCheckTimeoutSeconds is @range 2-120. AWS additionally requires the interval to exceed the timeout, so lowering only the interval below the group's existing timeout is refused with HTTP 400 and the target group keeps its previous settings.
Emulator: the call succeeds (HTTP 200) and the target group is left with an interval shorter than its timeout.
Why: a client validating a health-check change against the emulator sees it accepted and ships a target-group configuration AWS refuses; worse, the stored configuration describes a probe schedule that cannot be honored (a check can time out after the next check was due).
Fix: the ModifyTargetGroup handler's cross-member health-check validation — compare the effective (request-or-stored) interval and timeout before writing target-group state.
Traces: ec2__elbv2.asghc-target-group-health-check-interval-lowered-below-the-stored-timeout-rejected.

## Evidence
Replay output for ec2__elbv2.asghc-target-group-health-check-interval-lowered-below-the-stored-timeout-rejected:
`AssertionError: the call succeeded; real AWS rejects it, so there is no error code`

Expected: HTTP 400 with error code InvalidConfigurationRequest and a message naming the interval/timeout conflict. Actual: HTTP 200 and the modification applied. (The check's own target-group name/ARN are per-run values, not part of the requirement.)

## Steps to Reproduce
1. Replay the behavioral suite and re-run ec2__elbv2.asghc-target-group-health-check-interval-lowered-below-the-stored-timeout-rejected.
2. Create a target group with HealthCheckTimeoutSeconds set to a value T (e.g. the ASG check's stored timeout).
3. Call ModifyTargetGroup on it supplying only HealthCheckIntervalSeconds with a value <= T.
4. Expect HTTP 400 InvalidConfigurationRequest and the target group unchanged; the emulator returns HTTP 200 with the lowered interval stored.
