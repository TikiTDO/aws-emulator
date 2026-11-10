Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach autoscaling__ecs__elbv2.autoscaling-instance-launch-event-triggers-target-group-check
Issue-Key: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach|_error#12|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: autoscaling__ecs__elbv2.autoscaling-instance-launch-event-triggers-target-group-check|run#40|AssertionError|assert state in {"initial", "healthy", "unhealthy"}, (
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# autoscaling__ecs__elbv2: 3 net-new causes — an ALB never forwards to an ECS task ip target, a denied elasticloadbalancing:RegisterTargets call answers 200, and a registered instance reports target health 'unused'

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Three net-new root causes under the autoscaling__ecs__elbv2 heading (Cause 1 of 3 .. Cause 3 of 3).

Cause 1 of 3: an ALB listener whose default forward action points at an ECS-backed target group never forwards a request through to the running task, so the scenario never reaches a verdict.
AWS: Smithy (elasticloadbalancingv2) CreateListener -> DefaultActions[] with Type 'forward' and TargetGroupArn; DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth.State. On AWS, once the ECS service's task is RUNNING and the target reaches State 'healthy', a GET to the load balancer answers with the container's response (HTTP 200).
Emulator: the check is killed by its ceiling — "Check timed out after 60.0s" — while waiting either for the target to reach 'healthy' or for the load balancer to answer 200.
Why: the single thing a load balancer exists for — forwarding a request to a registered, healthy target — is unobservable; every client that provisions ECS behind an ALB and then sends traffic hangs instead of getting a response or an error.
Fix: the ELBv2 data-plane forwarding path (listener default action -> target group -> ECS task address) and the target-health convergence it depends on.
Traces: autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip

Cause 2 of 3: a caller denied elasticloadbalancing:RegisterTargets is still allowed to call RegisterTargets — the call answers HTTP 200 instead of being refused.
AWS: Smithy (elasticloadbalancingv2) RegisterTargets; a principal whose effective identity-based policy does not allow elasticloadbalancing:RegisterTargets is refused with HTTP 403 and error code AccessDenied, message of the form "User: <caller principal ARN> is not authorized to perform: elasticloadbalancing:RegisterTargets on resource: <target group ARN> because no identity-based policy allows the elasticloadbalancing:RegisterTargets action" — the fixed text is the AccessDenied sentence, and the principal ARN, action name and resource ARN are taken from the request and the caller's identity.
Emulator: RegisterTargets returns 200 with an empty body. Replay: AssertionError "expected the call to be denied, but it SUCCEEDED and returned {'ResponseMetadata': {'RequestId': ..., 'HTTPStatusCode': 200, ...}}" (the request id and date are per-run values; the 200 is the defect).
Why: the check's control — a direct RegisterTargets under a denied identity — is what distinguishes Auto Scaling registering through its own service-linked role from the caller registering directly. With authorization never evaluated, every permission-boundary test reads as allowed and a client cannot tell which identity actually performed a registration.
Fix: the ELBv2 request-authorization component that must evaluate the caller's identity-based policy for elasticloadbalancing:RegisterTargets before the RegisterTargets handler mutates state, and emit 403 AccessDenied.
Traces: autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach

Cause 3 of 3: DescribeTargetHealth reports State 'unused' for an instance registered in a target group that is the target of an active forwarding listener.
AWS: Smithy (elasticloadbalancingv2) DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth.{State,Reason,Description}. TargetHealth.Reason documents 'unused' as reachable only for Target.NotRegistered / Target.NotInUse / Target.InvalidState / Target.IpUnusable — i.e. a target that is not registered, or whose target group is not used by any load balancer. A registered target in a group wired to an active forward action is therefore 'initial' (Elb.RegistrationInProgress / Elb.InitialHealthChecking), then 'healthy' or 'unhealthy' with the corresponding reason.
Emulator: the instance is reported State 'unused' while its target group IS the forward target of an active listener. Replay: AssertionError "instance i-00000000000000001 reported target health state 'unused', which is not reachable on a group with an active forwarding listener" — the instance id is a per-run value; the illegal state is the defect.
Why: 'unused' tells a client the target group is not attached to any load balancer, so health-driven logic (wait-for-healthy loops, ASG ELB health checks, deployment gates) concludes the wiring is wrong and gives up, even though the listener is correctly configured.
Fix: the ELBv2 target-health state machine behind DescribeTargetHealth — it must take listener/target-group attachment into account and return initial/healthy/unhealthy with the modeled Reason, reserving 'unused' for unregistered targets and unattached groups.
Traces: autoscaling__ecs__elbv2.autoscaling-instance-launch-event-triggers-target-group-check

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator; service heading autoscaling__ecs__elbv2 7/35 (20%), 28 failed (25 of them OWNED/ALREADY FILED; the three ids in trace_ids survive).

Cause 1: [mismatch] autoscaling__ecs__elbv2.alb-listener-forwards-request-to-ecs-task-ip
  Error: Check timed out after 60.0s
  (body waits for DescribeTargetHealth states == ['healthy'], then polls http://<dns>/ for 200)
  Expected: HTTP 200 from the task's container through the listener's forward action. Actual: neither wait ever settles.

Cause 2: [mismatch] autoscaling__ecs__elbv2.autoscaling-group-without-elasticloadbalancing-permission-fails-attach
  File "check_...autoscaling_group_without_elasticloadbalancing_permission_fails_attach.py", line 48, in run -> _error(...)
  AssertionError: expected the call to be denied, but it SUCCEEDED and returned {'ResponseMetadata': {'RequestId': '7b9d1044-feda-4297-ab31-7accef16e684', 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '260', ...}, 'RetryAttempts': 0}}
  Expected: HTTP 403, Error.Code 'AccessDenied', message "User: <principal ARN> is not authorized to perform: elasticloadbalancing:RegisterTargets on resource: <target group ARN> because no identity-based policy allows the elasticloadbalancing:RegisterTargets action". Actual: HTTP 200.

Cause 3: [mismatch] autoscaling__ecs__elbv2.autoscaling-instance-launch-event-triggers-target-group-check
  File "check_...autoscaling_instance_launch_event_triggers_target_group_check.py", line 190, in run -> assert state in {"initial", "healthy", "unhealthy"}
  AssertionError: instance i-00000000000000001 reported target health state 'unused', which is not reachable on a group with an active forwarding listener
  Expected: 'initial' (Reason Elb.RegistrationInProgress or Elb.InitialHealthChecking), then 'healthy'/'unhealthy'. Actual: 'unused'.

Smithy consulted (model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): RegisterTargets; CreateListener DefaultActions[].{Type,TargetGroupArn}; DescribeTargetHealth output TargetHealthDescriptions[].TargetHealth {State, Reason, Description} — the Reason documentation enumerates which reason codes are reachable per state, including the 'unused' set Target.NotRegistered / Target.NotInUse / Target.InvalidState / Target.IpUnusable.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; all three ids in trace_ids reproduce.
2. Cause 1: create an ALB with an HTTP:80 listener whose default action forwards to an ip-type target group, run an awsvpc ECS service with that loadBalancers binding, wait for DescribeTargetHealth state 'healthy', then GET http://<DNSName>/. Expected the container's 200; the emulator never settles.
3. Cause 2: under an identity whose policy does NOT allow elasticloadbalancing:RegisterTargets, call elbv2.RegisterTargets(TargetGroupArn=<tg>, Targets=[{Id:<instance id>}]). Expected 403 AccessDenied with the "...no identity-based policy allows the elasticloadbalancing:RegisterTargets action" message; the emulator answers 200.
4. Cause 3: attach a target group to an ASG, create a listener forwarding to that group, let the ASG launch an instance, then call elbv2.DescribeTargetHealth(TargetGroupArn=<tg>). Expected 'initial'/'healthy'/'unhealthy' with a modeled Reason; the emulator reports 'unused'.
5. Re-replay the three ids in trace_ids to confirm the fix.
