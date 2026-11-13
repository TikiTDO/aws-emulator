Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-asghc-traj-ip-target-group-attach-refused-instance-group-accepted
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-asghc-traj-ip-target-group-attach-refused-instance-group-accepted autoscaling-group-configuration-and-topology-negative autoscaling-group-dependent-operations-on-nonexistent-group
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|AttachLoadBalancerTargetGroups|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|AttachLoadBalancerTargetGroups|4xx->2xx|status
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.TargetGroupARNs.member
Issue-Key: autoscaling|AttachLoadBalancerTargetGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261022T112459Z-autoscaling-group-round-trip-create-skips-min-max.md
Issue-Commit: f7067607673c78bda9a2a88ab2c9b41127d89615

# autoscaling: AttachLoadBalancerTargetGroups skips target-type validation, group target-group state is wrong, and its not-found refusal is re-worded

> _Note: this issue was identified against the code base at commit `f7067607673c78bda9a2a88ab2c9b41127d89615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachLoadBalancerTargetGroups

## Description
Three net-new wire causes on the Auto Scaling load-balancer-attachment surface (Cause 1 of 3 .. Cause 3 of 3). All three must be closed before this issue is submitted; each names its own fixture and step.

Related open filings that do NOT carry these replay keys and therefore do not cover these failures: 20261112T140934Z-autoscaling-ec2-elbv2-autoscaling-autoscaling-ec2-elbv2-4-net-new-causes.md (covers a nonexistent/malformed ARN and the IAM AccessDenied gate, not the target-TYPE check), 20261112T140934Z-autoscaling-ecs-elbv2-autoscaling-autoscaling-ecs-elbv2-attachloadbalancertargetgroups-accepts-a-target.md (cross-region ARN), 20261110T160612Z-autoscaling-ec2-describeautoscalinggroups-omits-loadbalancernames-targetgrouparns-and-suspendedprocesses.md and 20261111T093414Z-autoscaling-ec2-elbv2-2-net-new-causes-attachloadbalancertargetgroups.md. The replay marked none of the clusters below OWNED.

Cause 1 of 3: AttachLoadBalancerTargetGroups accepts a target group whose target type is not 'instance'.
AWS: Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json) AttachLoadBalancerTargetGroups input {AutoScalingGroupName (required, XmlStringMaxLen255, length 1-255), TargetGroupARNs (required list)}, empty output, modeled errors InstanceRefreshInProgressFault / ResourceContentionFault / ServiceLinkedRoleFailure; the target-type precondition itself is recorded behavior only (the awsQuery common ValidationError). An Auto Scaling group can only register EC2 instances, so AWS refuses the whole request with HTTP 400, Error.Type 'Sender', Error.Code 'ValidationError' and the message "Provided Target Group '<target group arn>' has invalid target type. Please ensure all provided Target Groups have target type of instance." — the sentences "Provided Target Group '...' has invalid target type." and "Please ensure all provided Target Groups have target type of instance." are AWS's fixed text, and the quoted ARN is the offending entry taken from the request's TargetGroupARNs. Nothing is attached. The same call for an instance-type target group is accepted with 200.
Emulator: the request for an ip-target-type target group is answered HTTP 200 with an empty AttachLoadBalancerTargetGroupsResult and no ErrorResponse element at all, i.e. the attachment is performed.
Why: a client that attaches an ALB/NLB target group created for awsvpc/ip targets to a group gets a success it would never get on AWS, so the misconfiguration is only discovered later (or never) as instances that never register; the ValidationError path clients branch on is unreachable, and the operation cannot be used to validate a deployment's target-group wiring.
Emulator: see above.
Fix: AttachLoadBalancerTargetGroups' input validation in the autoscaling service — resolve each TargetGroupARNs entry against ELBv2 target-group state, check its TargetType is 'instance', and refuse the whole request with 400/ValidationError before mutating group state.
Traces: autoscaling-asghc-traj-ip-target-group-attach-refused-instance-group-accepted#task#8.

Cause 2 of 3: after the attach calls, DescribeAutoScalingGroups reports the group's target-group state wrongly — it lists the target group AWS refused, and TrafficSources is empty.
AWS: Smithy DescribeAutoScalingGroups -> AutoScalingGroups[].{TargetGroupARNs, TrafficSources}. A group reports in TargetGroupARNs exactly the target groups that were successfully attached (so the refused non-instance target group is absent), and reports one TrafficSources entry per attached source, each {Identifier: <the target group ARN>, Type: 'elbv2'}.
Emulator: TargetGroupARNs contains BOTH ARNs (the refused ip-type one and the accepted instance-type one) where only the accepted one is expected, and TrafficSources is serialized as an empty value instead of a member list containing {Identifier: <attached target group arn>, Type: 'elbv2'}.
Why: the group's read-back contradicts what AWS's own refusal would have left behind, and TrafficSources — the modern read clients and CloudFormation drift detection use — reports no traffic source for a group that has one, so load-balancer attachment is unobservable through either member.
Fix: the autoscaling group state for traffic sources plus the DescribeAutoScalingGroups serializer — only record an attachment that passed validation, and project every recorded attachment into TrafficSources as {Identifier, Type='elbv2'} alongside TargetGroupARNs.
Traces: autoscaling-asghc-traj-ip-target-group-attach-refused-instance-group-accepted#task#12.

Cause 3 of 3: AttachLoadBalancerTargetGroups' group-not-found refusal is re-worded with emulator-authored detail.
AWS: recorded behavior only for the message text (the awsQuery common ValidationError; no not-found fault is modeled on this operation). For a nonexistent AutoScalingGroupName AWS answers HTTP 400 / Error.Code 'ValidationError' with the message 'AutoScalingGroup name not found - null' — the whole string is AWS's fixed text for this operation and it carries NO variable part (AWS does not interpolate the requested group name here).
Emulator: the same status and code but the message 'AutoScalingGroup name not found - AutoScalingGroup <requested name> not found', i.e. the suffix after the dash is emulator-authored and interpolates the request's group name.
Why: clients and test suites that match on the refusal sentence (and the many SDK/CLI users who assert on the exact AWS string) see a message AWS never emits; the divergence is per-operation, so a blanket "name the group" message is wrong here even though other operations do name it.
Fix: the not-found refusal text in the autoscaling AttachLoadBalancerTargetGroups handler's validation — emit this operation's exact AWS sentence rather than a shared emulator-authored not-found formatter.
Traces: autoscaling-group-configuration-and-topology-negative#task#11 (also autoscaling-group-dependent-operations-on-nonexistent-group#task#13).

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200): 716/1044 steps passed (68.6%), 78 divergence clusters. These three clusters carry no owning issue and no stub.

Cause 1 — fixture autoscaling-asghc-traj-ip-target-group-attach-refused-instance-group-accepted, step task#8, operation AttachLoadBalancerTargetGroups, verdict mismatch:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': "Provided Target Group 'arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/<name>-ip/<suffix>' has invalid target type. Please ensure all provided Target Groups have target type of instance."}, 'RequestId': '<shaped-not-asserted>'}, got None)
  The target-group name/ARN suffix and RequestId in the recording are per-run values; the requirement is 400 / Sender / ValidationError with AWS's fixed sentences and the request's ARN.

Cause 2 — same fixture, step task#12, operation DescribeAutoScalingGroups, verdict mismatch:
  $...AutoScalingGroups.member.TargetGroupARNs.member: type_mismatch (expected the instance-type target group ARN only, got a list of BOTH the ip-type and the instance-type ARNs)
  $...AutoScalingGroups.member.TrafficSources: type_mismatch (expected {'member': {'Identifier': '<instance-type target group arn>', 'Type': 'elbv2'}}, got '')
  (The same step also shows the missing CapacityReservationSpecification / AvailabilityZoneIds / InstanceLifecyclePolicy / ServiceLinkedRoleARN / AvailabilityZoneDistribution members already owned by 20261022T112459Z-autoscaling-group-round-trip-create-skips-min-max.md; this cluster is net-new for the TargetGroupARNs/TrafficSources members.)

Cause 3 — fixture autoscaling-group-configuration-and-topology-negative, step task#11, operation AttachLoadBalancerTargetGroups, verdict mismatch (expected 400, got 400):
  $.ErrorResponse.Error.Message: value_mismatch (expected 'AutoScalingGroup name not found - null', got 'AutoScalingGroup name not found - AutoScalingGroup sim2real-probe-autoscaling-missing-asg not found')
  affected also: autoscaling-group-dependent-operations-on-nonexistent-group#task#13. The group name is the request's value; the fixed text is everything else.

Smithy consulted: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json — AttachLoadBalancerTargetGroups input {AutoScalingGroupName required length 1-255, TargetGroupARNs required list}, empty output, errors InstanceRefreshInProgressFault/ResourceContentionFault/ServiceLinkedRoleFailure (no not-found fault modeled, so the refusal is the query-protocol ValidationError).

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay trace set simparity-wire-aws-v1; the fixtures/steps named under Traces reproduce.
2. Cause 1: create an ELBv2 target group with TargetType='ip' and another with TargetType='instance' in the group's VPC, create an Auto Scaling group from a launch template, then call autoscaling AttachLoadBalancerTargetGroups with the ip-type ARN. Expect HTTP 400, Error.Type Sender, Error.Code ValidationError, message "Provided Target Group '<arn>' has invalid target type. Please ensure all provided Target Groups have target type of instance." and nothing attached; the emulator answers 200. Repeat with the instance-type ARN and expect 200.
3. Cause 2: after step 2, call DescribeAutoScalingGroups for the group. Expect TargetGroupARNs to list only the instance-type ARN and TrafficSources to list one {Identifier: <that ARN>, Type: 'elbv2'}; the emulator lists both ARNs and an empty TrafficSources.
4. Cause 3: call AttachLoadBalancerTargetGroups with a group name that does not exist. Expect HTTP 400 / ValidationError with the message exactly 'AutoScalingGroup name not found - null'; the emulator appends its own 'AutoScalingGroup <name> not found' detail.
5. Re-replay autoscaling-asghc-traj-ip-target-group-attach-refused-instance-group-accepted#task#8 and #task#12 and autoscaling-group-configuration-and-topology-negative#task#11 to confirm the fix.
