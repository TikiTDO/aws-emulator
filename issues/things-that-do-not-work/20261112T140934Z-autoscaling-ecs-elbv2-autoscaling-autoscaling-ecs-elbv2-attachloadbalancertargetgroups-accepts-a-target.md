Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.cli-attach-target-group-wrong-region-fails
Issue-Key: autoscaling__ecs__elbv2.cli-attach-target-group-wrong-region-fails|run#18|AssertionError|assert proc.returncode != 0, "attaching a cross-region target group ARN unexpectedly succeeded"
Issue-Commit: 59f14ace25ed5753b46a36483d50334ce01f6026

# autoscaling (autoscaling__ecs__elbv2): AttachLoadBalancerTargetGroups accepts a target group ARN from another region instead of refusing it with ValidationError

> _Note: this issue was identified against the code base at commit `59f14ace25ed5753b46a36483d50334ce01f6026`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachLoadBalancerTargetGroups

## Description
One net-new root cause under the suite heading `autoscaling__ecs__elbv2` (Cause 1 of 1).

Related open filings that do NOT carry this replay key, so they do not cover it: 20261110T211308Z-autoscaling-ecs-elbv2-2-net-new-causes-registertargets.md and 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md are adjacent (ASG/target-group attachment), but the suite marked this check neither OWNED nor ALREADY FILED. It is the cross-region sibling of the unresolvable-ARN cause filed under the `autoscaling__ec2__elbv2` heading in this same sweep; it is filed separately because an issue carries exactly one service token.

Cause 1 of 1: AttachLoadBalancerTargetGroups accepts a TargetGroupARNs entry whose ARN region is not the endpoint's region.
AWS: Smithy (autoscaling) AttachLoadBalancerTargetGroups requires AutoScalingGroupName and TargetGroupARNs and declares only InstanceRefreshInProgressFault, ResourceContentionFault and ServiceLinkedRoleFailure, so the cross-region refusal is recorded behavior only: an Auto Scaling group can only be attached to target groups in its own region, and AWS refuses an ARN from another region with HTTP 400 and error code `ValidationError` (the message names the ARN that could not be resolved, taken from the request) and registers nothing — DescribeAutoScalingGroups shows TargetGroupARNs unchanged and no instance is registered. The same command with a same-region ARN succeeds and registers the instance.
Emulator: the call is answered successfully — the AWS CLI exits 0 — for a target group ARN whose region segment is a different region from the one the client is calling. Replay: "attaching a cross-region target group ARN unexpectedly succeeded" (the ARN's region values are the run's; the requirement is that an ARN region other than the endpoint's is refused).
Why: a client that builds ARNs from the wrong region variable gets a 200 and an Auto Scaling group that claims an attachment which can never register anything; the regional isolation a multi-region deployment relies on is not enforced, and the ValidationError branch clients handle is unreachable.
Fix: AttachLoadBalancerTargetGroups' input validation in the autoscaling service — parse each TargetGroupARNs entry, compare its region (and account) segment against the request's, and refuse the whole request with ValidationError before mutating the group.
Traces: autoscaling__ecs__elbv2.cli-attach-target-group-wrong-region-fails.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 326/613 passed (53.2%); service heading `autoscaling__ecs__elbv2` 17/35 (49%), 18 failed, of which this is the only survivor after OWNED/ALREADY FILED checks are skipped.

[mismatch] autoscaling__ecs__elbv2.cli-attach-target-group-wrong-region-fails
  File "run.py", line 445, in <lambda> -> body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_autoscaling__ecs__elbv2_cli_attach_target_group_wrong_region_fails.py", line 64, in run
  assert proc.returncode != 0, "attaching a cross-region target group ARN unexpectedly succeeded"
  AssertionError: attaching a cross-region target group ARN unexpectedly succeeded
  Expected: `aws autoscaling attach-load-balancer-target-groups` exits nonzero with HTTP 400 / error code ValidationError, and TargetGroupARNs stays unchanged. Actual: exit 0, the attachment is accepted.

Smithy consulted (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): AttachLoadBalancerTargetGroups input {AutoScalingGroupName (required, length 1-255, pattern ^[\u0020-\uD7FF...]*$), TargetGroupARNs (required list)}, empty output, errors InstanceRefreshInProgressFault (400) / ResourceContentionFault (500) / ServiceLinkedRoleFailure (500) — no cross-region error is modeled, hence recorded behavior for the ValidationError.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the id in trace_ids reproduces.
2. Create an Auto Scaling group from a launch template in the endpoint's region, and create an ELBv2 target group in that region.
3. Run `aws autoscaling attach-load-balancer-target-groups --auto-scaling-group-name <group> --target-group-arns <the target group's ARN with its region segment replaced by a different region>`. Expect a nonzero exit with ValidationError and DescribeAutoScalingGroups showing TargetGroupARNs unchanged; the emulator exits 0 and records the attachment.
4. Run the same command with the real, same-region ARN and confirm it succeeds and the group's instance registers.
5. Re-replay autoscaling__ecs__elbv2.cli-attach-target-group-wrong-region-fails to confirm the fix.
