Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach autoscaling__ecs__elbv2.autoscaling-instance-launch-event-triggers-target-group-check
Issue-Key: autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach|_error#12|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: autoscaling__ecs__elbv2.autoscaling-instance-launch-event-triggers-target-group-check|run#40|AssertionError|assert state in {"initial", "healthy", "unhealthy"}, (
Issue-Commit: dd8fce8e7ad2da3c1b7c1776d58c99e163a1e8cd

# autoscaling__ecs__elbv2: 2 net-new causes — RegisterTargets ignores an explicit IAM deny (200 OK), and DescribeTargetHealth reports 'unused' for a target behind an active forwarding listener

> _Note: this issue was identified against the code base at commit `dd8fce8e7ad2da3c1b7c1776d58c99e163a1e8cd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets, DescribeTargetHealth

## Description
Two independent, net-new ELBv2 root causes surfaced by the autoscaling__ecs__elbv2 behavioral checks. Neither is covered by an open issue: the replay report marked neither check ALREADY FILED nor OWNED. Related but distinct open issues that do NOT cover these: 20261104T042502Z-autoscaling-ec2-elbv2-4-net-new-root-causes.md (IAM authorization never evaluated, but for autoscaling/ec2 operations under a different service heading) and 20261107T183213Z-autoscaling-ec2-elbv2-2-net-new-causes-an.md (empty/'unused' target health, a different service heading and a different wiring).

Cause 1 of 2: elasticloadbalancing:RegisterTargets is not authorized — an explicitly denied caller still succeeds.
AWS: RegisterTargets is authorized against the IAM action elasticloadbalancing:RegisterTargets on the target group resource. When the calling identity's identity-based policy contains an explicit Deny for that action, AWS refuses the call before any modeled validation, with HTTP 403 and an awsQuery Error envelope whose Type is Sender and whose Code is `AccessDenied`. The message has the fixed form `User: <caller-arn> is not authorized to perform: elasticloadbalancing:RegisterTargets on resource: <target-group-arn> with an explicit deny in an identity-based policy` — the fixed text is "User: ", " is not authorized to perform: ", " on resource: " and " with an explicit deny in an identity-based policy"; the variable parts are the assumed-role ARN carried by the signing credentials, the IAM action name, and the TargetGroupArn taken from the request. Smithy: RegisterTargets models only InvalidTargetException/TargetGroupNotFoundException/TooManyRegistrationsForTargetIdException/TooManyTargetsException (all HTTP 400, awsQuery codes InvalidTarget/TargetGroupNotFound/TooManyRegistrationsForTargetId/TooManyTargets); AccessDenied is the protocol-common authorization failure and is not in the operation's error list, so the exact wording above is recorded behavior only.
Emulator: the denied identity's RegisterTargets call returns HTTP 200 with an empty RegisterTargetsResult body (content-type text/xml, no Error element) instead of a 403/AccessDenied. The deny is demonstrably in force on that identity in the same run: the check's control probe, autoscaling:DescribeTags under the same assumed role, IS denied, so the gap is specific to the ELBv2 RegisterTargets authorization step rather than to IAM policy propagation.
Why: a client that relies on AWS refusing a least-privilege violation sees a false success. Permission-boundary assertions pass vacuously, code that branches on the AccessDenied error code never takes that branch, and a target is actually registered that real AWS would have left unregistered — so subsequent target-health and routing state diverges from AWS for the rest of the scenario.
Fix: the authorization step of the ELBv2 RegisterTargets handler (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/register_targets.rs) must consult the request's identity against the IAM policy state and emit the common AccessDenied refusal before performing the registration, the same way the autoscaling DescribeTags path already does.
Traces: autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach.

Cause 2 of 2: DescribeTargetHealth reports State 'unused' for a target in a group that an active ALB listener forwards to.
AWS: Smithy TargetHealthStateEnum = initial | healthy | unhealthy | unhealthy.draining | unused | draining | unavailable. Per the modeled documentation of TargetHealth.Reason, `unused` is only reported with reason Target.NotRegistered, Target.NotInUse ("the target group is not used by any load balancer, or the target is in an Availability Zone that is not enabled for its load balancer"), Target.InvalidState or Target.IpUnusable. A target that is registered in a target group which is the DefaultActions forward target of an ENABLED HTTP:80 listener on an ACTIVE application load balancer, with HealthCheckEnabled=true and the target's AZ among the load balancer's subnets, is therefore never `unused`: it reports `initial` (reason Elb.RegistrationInProgress, then Elb.InitialHealthChecking) until the required number of health checks has completed, and then `healthy` or `unhealthy`.
Emulator: immediately after the ASG scale-out registers its instance into the attached target group, DescribeTargetHealth(TargetGroupArn, Targets=[{Id: <instance-id>}]) returns one TargetHealthDescription whose TargetHealth.State is `unused`, even though the group is the listener's forward target on an active ALB and health checks are enabled. Observed verbatim: "instance i-00000000000000001 reported target health state 'unused', which is not reachable on a group with an active forwarding listener" (the instance id is a per-run value, the state is the requirement).
Why: `unused` tells a client the group is wired to no load balancer. Waiters and deployment gates that poll for initial -> healthy (CloudFormation/CodeDeploy rollouts, ASG ELB-type health checks, autoscaling target-group attachment verification) never progress, and operators reading the state conclude the listener/target-group wiring is broken when the recorded AWS behavior is a normal initial -> healthy transition.
Fix: the state derivation in the ELBv2 DescribeTargetHealth handler (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_target_health.rs) must derive the state from whether any listener action on an active load balancer forwards to the group and from the registration/health-check lifecycle, instead of falling back to `unused` for a registered target of a forwarded-to group; the accompanying Reason must come from the documented set for the state it reports.
Traces: autoscaling__ecs__elbv2.autoscaling-instance-launch-event-triggers-target-group-check.

## Evidence
Cause 1 — check autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach [mismatch]:
  Expected: elbv2.RegisterTargets under a role with an explicit Deny on elasticloadbalancing:RegisterTargets -> HTTP 403, Error.Code 'AccessDenied', Error.Type 'Sender', message 'User: <caller-arn> is not authorized to perform: elasticloadbalancing:RegisterTargets on resource: <target-group-arn> with an explicit deny in an identity-based policy'.
  Actual: AssertionError: expected the call to be denied, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '260', ...}}} (request id and date are per-run values).
  Control in the same run: autoscaling:DescribeTags under the same assumed role IS denied, so the deny was active when RegisterTargets was accepted.

Cause 2 — check autoscaling__ecs__elbv2.autoscaling-instance-launch-event-triggers-target-group-check [mismatch]:
  Wiring: CreateTargetGroup(Protocol=HTTP, Port=80, TargetType=instance, HealthCheckEnabled=True) -> CreateLoadBalancer(Type=application, Scheme=internet-facing) polled to State.Code='active' -> CreateListener(Protocol=HTTP, Port=80, DefaultActions=[{Type: forward, TargetGroupArn: tg}]) -> CreateAutoScalingGroup(TargetGroupARNs=[tg]) -> SetDesiredCapacity(1).
  Expected: DescribeTargetHealth -> TargetHealth.State in {initial, healthy, unhealthy} (AWS: 'initial' with Reason Elb.RegistrationInProgress / Elb.InitialHealthChecking, then healthy/unhealthy).
  Actual: AssertionError: instance i-00000000000000001 reported target health state 'unused', which is not reachable on a group with an active forwarding listener.
  Model: TargetHealth.Reason documents 'unused' only for Target.NotRegistered / Target.NotInUse / Target.InvalidState / Target.IpUnusable.

## Steps to Reproduce
Cause 1:
1. Create a VPC-backed target group and an ASG with one running instance.
2. Create an IAM role whose policy explicitly Denies elasticloadbalancing:RegisterTargets (and autoscaling:DescribeTags as a side-effect-free probe) and assume it.
3. Poll autoscaling:DescribeTags under that role until it is denied, confirming the deny is in effect.
4. Call elbv2:RegisterTargets(TargetGroupArn=<tg>, Targets=[{Id: <instance-id>, Port: 80}]) under the same role.
5. Observe HTTP 200 / empty RegisterTargetsResult. AWS answers HTTP 403 with Error.Code 'AccessDenied'.

Cause 2:
1. CreateTargetGroup(Protocol=HTTP, Port=80, TargetType=instance, VpcId=<vpc>, HealthCheckEnabled=true).
2. CreateLoadBalancer(Type=application, Scheme=internet-facing, Subnets=<two AZs>) and wait for State.Code='active'.
3. CreateListener(Protocol=HTTP, Port=80, DefaultActions=[{Type: forward, TargetGroupArn: <tg>}]).
4. CreateAutoScalingGroup(LaunchTemplate=..., MinSize=0, MaxSize=2, DesiredCapacity=0, TargetGroupARNs=[<tg>], VPCZoneIdentifier=<the two subnets>).
5. SetDesiredCapacity(DesiredCapacity=1) and wait for the instance to appear.
6. Call elbv2:DescribeTargetHealth(TargetGroupArn=<tg>, Targets=[{Id: <instance-id>}]).
7. Observe TargetHealth.State='unused'. AWS reports 'initial' (Elb.RegistrationInProgress / Elb.InitialHealthChecking) and then 'healthy' or 'unhealthy'.
