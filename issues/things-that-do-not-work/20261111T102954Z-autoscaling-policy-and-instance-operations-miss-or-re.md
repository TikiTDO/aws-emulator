Issue-Source: trace-sweep-divergence
Issue-Trace: asg-err-scaling-policy-adjustments
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asg-err-scaling-policy-adjustments autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals autoscaling-group-dependent-operations-on-nonexistent-group autoscaling-instance-level-operations-negative
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|PutScalingPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|PutScalingPolicy|4xx->2xx|status
Issue-Key: autoscaling|CancelInstanceRefresh|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|SetInstanceProtection|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|TerminateInstanceInAutoScalingGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# autoscaling policy and instance operations miss or re-word AWS validation refusals (4 causes)

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Four argument refusals on autoscaling policy- and instance-level operations diverge: one is not raised at all, and three carry emulator-authored message text instead of AWS's. 4 net-new causes, all on autoscaling. The autoscaling Smithy model is not present in this checkout (multi-service root, no services/service-com-amazonaws-autoscaling-autoscaling/smithy-model/*.json), so each cause is grounded in recorded behavior only; the fixed text of each message and its variable parts are named per cause.

Cause 1 of 4 — PutScalingPolicy accepts more StepAdjustments than the service allows.
AWS: recorded behavior only. A StepScaling policy with more than 20 step adjustments is refused HTTP 400 with Error.Type 'Sender', Error.Code 'ValidationError' and the fixed Message 'Your policies can have at most 20 StepAdjustments.' (no variable part — 20 is the service limit).
Emulator: HTTP 200, the policy is created and no ErrorResponse is produced.
Why: a client building step policies programmatically has its over-sized policy accepted here and rejected in AWS, so the emulator validates nothing and the defect only appears in production; worse, the stored policy has a shape AWS cannot hold.
Fix: PutScalingPolicy input validation (StepAdjustments cardinality bound) in the operation's validation layer.
Traces: asg-err-scaling-policy-adjustments#task#11.

Cause 2 of 4 — CancelInstanceRefresh's no-active-refresh refusal is re-worded.
AWS: recorded behavior only. With no refresh to cancel the call is refused HTTP 400 and the Message is the fixed 'No in progress, baking, pending, or cancelling Instance Refresh found for Auto Scaling group <group-name>.' — the group name is the only variable part, and the enumeration of cancellable states plus the trailing period are fixed text.
Emulator: HTTP 400 with 'No active Instance Refresh found for Auto Scaling group <group-name>' — different wording, no state enumeration, no trailing period.
Why: the refusal text is what tells an operator which refresh states are cancellable; clients that assert on AWS's message (or parse the state list out of it) do not match the emulator's.
Fix: the CancelInstanceRefresh handler's not-found error message.
Traces: autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#7.

Cause 3 of 4 — SetInstanceProtection's non-member refusal is re-worded.
AWS: recorded behavior only. Naming an instance that is not in the group is refused HTTP 400 ValidationError with Message 'The instance <instance-id> is not part of Auto Scaling group <group-name>.' — instance id and group name are the variable parts.
Emulator: 'Trying to update Instances that are not part of the group: <instance-id>' — a different sentence that never names the group.
Why: the message no longer identifies which group rejected the instance, so multi-group callers cannot attribute the failure, and text-matching error handling breaks.
Fix: the SetInstanceProtection handler's membership-check error message.
Traces: autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#14 (cluster also covers autoscaling-group-dependent-operations-on-nonexistent-group#task#21, autoscaling-instance-level-operations-negative#task#2).

Cause 4 of 4 — TerminateInstanceInAutoScalingGroup refuses an unknown instance id with the wrong message and leaks a null group name.
AWS: recorded behavior only. An instance id that exists in no group is refused HTTP 400 ValidationError with Message 'Instance Id not found - No managed instance found for instance ID: <instance-id>' — the fixed 'Instance Id not found - No managed instance found for instance ID: ' prefix plus the requested id.
Emulator: 'Instance <instance-id> is not part of Auto Scaling group null.' — a different message that also serializes the literal 'null' where a group name would go.
Why: AWS distinguishes 'this instance is unknown' from 'this instance belongs to another group'; the emulator collapses the two and emits a message naming a nonexistent group 'null', so a client cannot tell which condition it hit.
Fix: the TerminateInstanceInAutoScalingGroup handler's instance lookup and its not-found error message.
Traces: autoscaling-group-dependent-operations-on-nonexistent-group#task#24 (cluster also covers autoscaling-instance-level-operations-negative#task#3).

Related but not covering: open issues 20261110T100847Z-autoscaling-refusal-fidelity-wrong-codes-and-emulator-authored.md and 20261107T071147Z-autoscaling-auto-scaling-argument-validation-refusals-use-the-wrong.md own other autoscaling refusal steps; the replay reported owned_by empty for all four clusters above.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. All four clusters reported owned_by empty and stub_path empty.

Cause 1 — asg-err-scaling-policy-adjustments#task#11, PutScalingPolicy expected 400 got 200:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Your policies can have at most 20 StepAdjustments.'}})

Cause 2 — autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#7, CancelInstanceRefresh 400 vs 400:
  $.ErrorResponse.Error.Message: expected 'No in progress, baking, pending, or cancelling Instance Refresh found for Auto Scaling group <group>.', got 'No active Instance Refresh found for Auto Scaling group <group>'

Cause 3 — autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#14, SetInstanceProtection 400 vs 400:
  $.ErrorResponse.Error.Message: expected 'The instance i-0123456789abcdef0 is not part of Auto Scaling group <group>.', got 'Trying to update Instances that are not part of the group: i-0123456789abcdef0'

Cause 4 — autoscaling-group-dependent-operations-on-nonexistent-group#task#24, TerminateInstanceInAutoScalingGroup 400 vs 400:
  $.ErrorResponse.Error.Message: expected 'Instance Id not found - No managed instance found for instance ID: i-0aaaaaaaaaaaaaaaa', got 'Instance i-0aaaaaaaaaaaaaaaa is not part of Auto Scaling group null.'

## Steps to Reproduce
1. Cause 1: PutScalingPolicy with PolicyType StepScaling and 21 StepAdjustments — answered 200 instead of 400 ValidationError.
2. Cause 2: CancelInstanceRefresh on a group with no refresh in progress — 400 with re-worded message.
3. Cause 3: SetInstanceProtection naming an instance id that is not a member of the group — 400 with re-worded message that omits the group name.
4. Cause 4: TerminateInstanceInAutoScalingGroup with an instance id unknown to every group — 400 with 'not part of Auto Scaling group null.' instead of AWS's 'Instance Id not found - ...'.
