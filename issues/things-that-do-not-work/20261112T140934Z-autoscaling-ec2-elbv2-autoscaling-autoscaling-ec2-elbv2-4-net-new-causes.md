Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asghc-attach-a-nonexistent-target-group-rejected autoscaling__ec2__elbv2.cli-target-group-attach-fails-with-invalid-arn autoscaling__ec2__elbv2.asg-target-group-attachment-denied-without-permission autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered autoscaling__ec2__elbv2.asg-registration-fails-with-mismatched-target-group-vpc autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted autoscaling__ec2__elbv2.asg-target-group-registration-serves-http-and-feeds-back-health autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted
Issue-Key: autoscaling__ec2__elbv2.asghc-attach-a-nonexistent-target-group-rejected|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: autoscaling__ec2__elbv2.cli-target-group-attach-fails-with-invalid-arn|run#13|AssertionError|assert bad_proc.returncode != 0, "expected malformed target group ARN to be rejected"
Issue-Key: autoscaling__ec2__elbv2.asg-target-group-attachment-denied-without-permission|_error#10|AssertionError|raise AssertionError(f"expected AccessDenied, but the call SUCCEEDED and returned {result!r}")
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered|_error#15|AssertionError|raise AssertionError(f"expected AccessDenied, but the call SUCCEEDED and returned {result!r}")
Issue-Key: autoscaling__ec2__elbv2.asg-registration-fails-with-mismatched-target-group-vpc|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asg-target-group-registration-serves-http-and-feeds-back-health|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted|run#17|AssertionError|assert health == {iid: ("unhealthy", "Target.FailedHealthChecks")}, (
Issue-Commit: 59f14ace25ed5753b46a36483d50334ce01f6026

# autoscaling (autoscaling__ec2__elbv2): 4 net-new causes — AttachLoadBalancerTargetGroups validates nothing and authorizes nothing, ASG target-group lifecycles never converge, and an NLB target reports healthy with no ingress

> _Note: this issue was identified against the code base at commit `59f14ace25ed5753b46a36483d50334ce01f6026`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachLoadBalancerTargetGroups

## Description
Four net-new root causes under the suite heading `autoscaling__ec2__elbv2` (Cause 1 of 4 .. Cause 4 of 4). All four must be closed before this issue is submitted; each carries its own failing check ids.

Related open filings that do NOT carry these replay keys and therefore do not cover these failures: 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md and 20261111T030957Z-autoscaling-ec2-elbv2-4-net-new-causes-target.md (adjacent to Causes 3 and 4), and 20261104T042502Z-autoscaling-ec2-elbv2-4-net-new-root-causes.md (adjacent to Cause 2). The suite marked none of the ids below OWNED.

Cause 1 of 4: AttachLoadBalancerTargetGroups accepts a TargetGroupARNs entry that does not resolve — a nonexistent target group and a syntactically malformed ARN are both answered 200.
AWS: Smithy (autoscaling) AttachLoadBalancerTargetGroups input requires AutoScalingGroupName and TargetGroupARNs and declares InstanceRefreshInProgressFault, ResourceContentionFault and ServiceLinkedRoleFailure; the query protocol's common `ValidationError` (HTTP 400) is what AWS answers for an unresolvable or malformed member, so for this condition: recorded behavior only. AWS refuses the whole call with HTTP 400 / error code `ValidationError`, whose message names the ARNs that could not be found (fixed text from AWS, the ARN list taken from the request), and attaches nothing — TargetGroupARNs on the group is unchanged afterwards. A malformed ARN is refused the same way before any lookup.
Emulator: the call returns HTTP 200 with an empty AttachLoadBalancerTargetGroupsResult body for a target group ARN that was never created, and the AWS CLI exits 0 for a malformed ARN. Replay: "the call succeeded; real AWS rejects it, so there is no error code" and "expected malformed target group ARN to be rejected".
Why: a client's typo or stale ARN is silently accepted, so the group reports an attachment to a target group that cannot exist and nothing ever registers there; the error path clients handle (ValidationError, attach nothing) is unreachable, and the operation cannot be used to validate configuration.
Fix: AttachLoadBalancerTargetGroups' input validation in the autoscaling service — parse each TargetGroupARNs entry, resolve it against ELBv2 target-group state, and refuse the whole request with ValidationError naming the unresolved ARNs before mutating the group.
Traces: autoscaling__ec2__elbv2.asghc-attach-a-nonexistent-target-group-rejected, autoscaling__ec2__elbv2.cli-target-group-attach-fails-with-invalid-arn.

Cause 2 of 4: AttachLoadBalancerTargetGroups succeeds for a caller whose identity is not permitted to perform it.
AWS: recorded behavior only for the authorization decision (IAM is not in the Smithy model). A caller assuming a role without autoscaling:AttachLoadBalancerTargetGroups (or without the elasticloadbalancing permissions the attachment needs) is refused with HTTP 403 and error code `AccessDenied`, whose message has AWS's fixed shape — `User: <caller arn> is not authorized to perform: <action> on resource: <resource arn>` — with the caller ARN, action and resource taken from the request and the signing identity; nothing is attached.
Emulator: the identical call returns HTTP 200 (content-type text/xml, empty result body) for the restricted principal. Replay: "expected AccessDenied, but the call SUCCEEDED and returned {'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}" (the RequestId, date and content-length in that dict are per-run values).
Why: every least-privilege test inverts — a client validating that a deployment role CANNOT attach target groups sees it succeed, and the permission-boundary behavior a CloudFormation stack depends on (instances NOT registered when the stack's role lacks the ELB permission) cannot be observed at all.
Fix: the authorization seam in front of the autoscaling AttachLoadBalancerTargetGroups handler — evaluate the signing identity's permissions and refuse with 403/AccessDenied, and apply the same gate on the path CloudFormation uses to attach a target group on the stack role's behalf.
Traces: autoscaling__ec2__elbv2.asg-target-group-attachment-denied-without-permission, autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered.

Cause 3 of 4: an Auto Scaling group's target-group lifecycle never converges — these checks exhaust their whole budget without the group ever reaching an observable state.
AWS: recorded behavior only. On AWS a group with MinSize/MaxSize/DesiredCapacity=1 brings an instance to InService within tens of seconds; that instance is registered in the group's attached target groups when they are in the same VPC and is NOT registered in one from another VPC; a SetDesiredCapacity after the attached target group has been deleted out of band fails the scale-out ("target group not found") and launches nothing; and with HealthCheckType=ELB the load balancer's verdict appears as InService/Healthy on the group's instance.
Emulator: all three checks end with "Check timed out after 60.0s", so none of launch, registration, the cross-VPC refusal or the ELB health feedback is ever observable.
Why: the entire ASG-to-target-group integration is unverifiable; a client cannot tell a converging group from a stuck one, and a test suite hangs instead of reporting.
Fix: the autoscaling group state machine that launches instances and drives them to InService, and the registration seam that pushes/withholds those instances in the group's attached ELBv2 target groups (including the VPC match check and the ELB health-check feedback into instance HealthStatus).
Traces: autoscaling__ec2__elbv2.asg-registration-fails-with-mismatched-target-group-vpc, autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted, autoscaling__ec2__elbv2.asg-target-group-registration-serves-http-and-feeds-back-health.

Cause 4 of 4: an NLB target is reported healthy although no security-group rule admits the health-check port, so no probe could have reached it.
AWS: Smithy (elasticloadbalancingv2) DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth {State, Reason, Description}. AWS reports 'healthy' only after HealthyThresholdCount consecutive successful probes from the load balancer nodes to the target's health-check port. With no ingress rule admitting that port the probes cannot complete, so AWS reports State 'unhealthy' with Reason 'Target.FailedHealthChecks' (recorded behavior for the dependency on the security-group rule); once the subnet CIDR is admitted the target becomes 'healthy' with no Reason.
Emulator: the target is reported {'State': 'healthy'} with Reason None while nothing admits the health-check port. Replay: "with no address rule for :4000 the NLB's checks must fail: {'i-00000000000000001': ('healthy', None)}" (the instance id and port 4000 are the run's values; the requirement is that an unprobeable target is unhealthy with Reason Target.FailedHealthChecks).
Why: health is synthesized from registration rather than derived from a probe, so every security-group, listener or container misconfiguration reads as healthy and all health-driven routing, failover and ASG replacement logic built on DescribeTargetHealth silently does the wrong thing.
Fix: the ELBv2 target-health probe and state machine behind DescribeTargetHealth, as reached through an ASG-attached NLB target group — derive State/Reason from an actual probe that respects the target's security-group ingress rules.
Traces: autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 326/613 passed (53.2%); service heading `autoscaling__ec2__elbv2` 23/60 (38%), 37 failed. Most are OWNED by open issues or ALREADY FILED against unimplemented stubs; the 8 ids in trace_ids are the survivors clustered here.

Cause 1 — [mismatch] autoscaling__ec2__elbv2.asghc-attach-a-nonexistent-target-group-rejected:
  File "check_...asghc_attach_a_nonexistent_target_group_rejected.py", line 96, in run -> got = _error(lambda: asg.attach_load_balancer_target_groups(...))
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
  [mismatch] autoscaling__ec2__elbv2.cli-target-group-attach-fails-with-invalid-arn:
  AssertionError: expected malformed target group ARN to be rejected  (aws autoscaling attach-load-balancer-target-groups exited 0)
  Expected: HTTP 400, error code ValidationError, TargetGroupARNs unchanged. Actual: HTTP 200 / CLI exit 0.

Cause 2 — [mismatch] autoscaling__ec2__elbv2.asg-target-group-attachment-denied-without-permission:
  AssertionError: expected AccessDenied, but the call SUCCEEDED and returned {'ResponseMetadata': {'RequestId': 'c89aec0b-75bd-4465-b41e-3718b6ac3c0d', 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '311', ...}}}
  [mismatch] autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered:
  AssertionError: expected AccessDenied, but the call SUCCEEDED and returned {'ResponseMetadata': {'RequestId': '6692b88a-09b4-4509-a04c-7da8912b3278', 'HTTPStatusCode': 200, ...}}
  Expected: HTTP 403, error code AccessDenied, nothing attached. Actual: HTTP 200. (RequestId/date/content-length are per-run values and are not part of the requirement.)

Cause 3 — Error: Check timed out after 60.0s for autoscaling__ec2__elbv2.asg-registration-fails-with-mismatched-target-group-vpc, autoscaling__ec2__elbv2.asg-scale-up-blocked-after-target-group-deleted and autoscaling__ec2__elbv2.asg-target-group-registration-serves-http-and-feeds-back-health.

Cause 4 — [mismatch] autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted:
  File "check_...asghc_nlb_target_stays_unhealthy....py", line 122, in run -> assert health == {iid: ("unhealthy", "Target.FailedHealthChecks")}
  AssertionError: with no address rule for :4000 the NLB's checks must fail: {'i-00000000000000001': ('healthy', None)}
  Expected: ('unhealthy', 'Target.FailedHealthChecks'). Actual: ('healthy', None).

Smithy consulted: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json — AttachLoadBalancerTargetGroups input {AutoScalingGroupName (required, length 1-255), TargetGroupARNs (required list)}, empty output, errors InstanceRefreshInProgressFault / ResourceContentionFault / ServiceLinkedRoleFailure. services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/... — DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth {State, Reason, Description}.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; every id in trace_ids reproduces.
2. Cause 1: create an ASG from a launch template, then call autoscaling.attach_load_balancer_target_groups(AutoScalingGroupName=<group>, TargetGroupARNs=[<a well-formed ARN for a target group that was never created>]). Expect HTTP 400 / ValidationError naming the ARN and DescribeAutoScalingGroups to show TargetGroupARNs unchanged; the emulator answers 200. Repeat with a syntactically malformed ARN through `aws autoscaling attach-load-balancer-target-groups`; expect a nonzero exit.
3. Cause 2: assume a role whose policy omits autoscaling:AttachLoadBalancerTargetGroups and issue the same, otherwise valid, attach call. Expect HTTP 403 / AccessDenied; the emulator answers 200. Then deploy the CFN stack whose role lacks the ELB permission and confirm the ASG's instances are NOT registered in the target group.
4. Cause 3: create an ASG (min=max=desired=1) with a same-VPC target group attached and poll DescribeAutoScalingGroups for an InService instance, then (a) attach a target group from another VPC and confirm nothing registers there, (b) delete the attached target group out of band and SetDesiredCapacity higher, expecting the scale-out to fail and launch nothing, (c) set HealthCheckType=ELB and confirm the load balancer's verdict appears on the instance. Each check times out at 60 s instead.
5. Cause 4: register the ASG's instance in an NLB target group whose health check is TCP/HTTP on a port that NO security-group rule admits, and poll DescribeTargetHealth. Expect ('unhealthy', 'Target.FailedHealthChecks'); the emulator reports ('healthy', None). Then admit the subnet CIDR on that port and expect 'healthy'.
6. Re-replay the ids in trace_ids to confirm the fix.
