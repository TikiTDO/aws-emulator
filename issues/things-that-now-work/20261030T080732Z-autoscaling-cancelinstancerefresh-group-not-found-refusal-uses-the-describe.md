Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-group-and-policy-dependent-operations-on-nonexistent-group
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-group-and-policy-dependent-operations-on-nonexistent-group autoscaling-warm-pool-and-instance-refresh-negative
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|CancelInstanceRefresh|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 0c981e6b14c8a03841eea58843a903cb607de4dd

# CancelInstanceRefresh group-not-found refusal uses the describe wording instead of AWS's "Auto Scaling group with name" sentence

> _Note: this issue was identified against the code base at commit `0c981e6b14c8a03841eea58843a903cb607de4dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelInstanceRefresh

## Description
One net-new cause on the Auto Scaling (awsQuery, api version 2011-01-01) client-error path for CancelInstanceRefresh.

Cause 1 of 1 - CancelInstanceRefresh naming an Auto Scaling group that does not exist.

AWS: HTTP 400 with the awsQuery ErrorResponse envelope - Error.Type Sender, Error.Code ValidationError, and Error.Message 'AutoScalingGroup name not found - Auto Scaling group with name <group-name> not found'. The fixed text is 'AutoScalingGroup name not found - Auto Scaling group with name ' followed by the requested AutoScalingGroupName and the trailing ' not found'; the only variable part is the group name taken from the request. Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): CancelInstanceRefresh input CancelInstanceRefreshType.AutoScalingGroupName is required (XmlStringMaxLen255, @length min 1 max 255); its modeled client errors are only ActiveInstanceRefreshNotFoundFault (awsQueryError ActiveInstanceRefreshNotFound, 400) and LimitExceededFault (awsQueryError LimitExceeded, 400), so a group that does not exist is reported through the service-common ValidationError - recorded behavior only for the sentence itself.

Emulator: same status (400) and Code, but the Message is the group-scoped-describe wording 'AutoScalingGroup name not found - AutoScalingGroup: <group-name> not found' - it substitutes 'AutoScalingGroup: ' for AWS's 'Auto Scaling group with name '. This is the same shared group-not-found builder the emulator applies to every group-scoped operation, whereas AWS serves a distinct sentence for this operation.

Why: AWS deliberately serves three different group-not-found message shapes across the instance-refresh operations (the describe form, the bare '- null' form, and this 'Auto Scaling group with name ...' form). Because the Code is the generic ValidationError, the message is the only machine-visible discriminator, so a client or test that matches AWS's sentence for CancelInstanceRefresh never matches, and an operator log scraper keyed on AWS's wording loses the event. Nothing in the difference is per-run, so the mismatch is permanent.

Fix: the CancelInstanceRefresh handler's group-lookup refusal must emit this operation's own message form rather than calling the shared echoing group-not-found builder used by the describes.

Traces: autoscaling-group-and-policy-dependent-operations-on-nonexistent-group#task#7 (also autoscaling-warm-pool-and-instance-refresh-negative#task#4 - same cluster).

Related but not covering: open issue 20261029T211352Z-autoscaling-auto-scaling-client-error-refusals-diverge-emulator-authored.md bundles the analogous DescribeInstanceRefreshes, StartInstanceRefresh and SetDesiredCapacity refusals, but CancelInstanceRefresh is NOT one of its replay keys and its Issue-Key list does not include autoscaling|CancelInstanceRefresh, so this divergence is unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200 and the data listener accepts). Divergence cluster verdict=mismatch, service=autoscaling, operation=CancelInstanceRefresh, expected_status 400 -> actual_status 400, affected_count 2.

$.ErrorResponse.Error.Message: value_mismatch
  expected: 'AutoScalingGroup name not found - Auto Scaling group with name sim2real-probe-autoscaling-asg-f393fafd not found'
  got:      'AutoScalingGroup name not found - AutoScalingGroup: sim2real-probe-autoscaling-asg-f393fafd not found'

The group name 'sim2real-probe-autoscaling-asg-f393fafd' (and its random suffix) is a per-run value supplied by the request and is NOT part of the requirement; the requirement is the fixed text 'AutoScalingGroup name not found - Auto Scaling group with name ' + <requested AutoScalingGroupName> + ' not found'. The HTTP status (400) and the Error.Code (ValidationError) already agree.

Cluster members: autoscaling-group-and-policy-dependent-operations-on-nonexistent-group#task#7, autoscaling-warm-pool-and-instance-refresh-negative#task#4.

Smithy grounding: CancelInstanceRefreshType.AutoScalingGroupName required; modeled errors ActiveInstanceRefreshNotFoundFault / LimitExceededFault / ResourceContentionFault only - no not-found fault, hence service-common ValidationError (Type Sender).

## Steps to Reproduce
1) Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2) POST an awsQuery request with Action=CancelInstanceRefresh&Version=2011-01-01 and AutoScalingGroupName set to a group that does not exist in the emulator's state (as fixture autoscaling-group-and-policy-dependent-operations-on-nonexistent-group does at step task#7).
3) Observe HTTP 400 with ErrorResponse.Error.Code=ValidationError in both cases, then compare ErrorResponse.Error.Message: AWS sends 'AutoScalingGroup name not found - Auto Scaling group with name <name> not found'; the emulator sends 'AutoScalingGroup name not found - AutoScalingGroup: <name> not found'.
4) Re-replay autoscaling-group-and-policy-dependent-operations-on-nonexistent-group#task#7 and autoscaling-warm-pool-and-instance-refresh-negative#task#4 to confirm the fix.
