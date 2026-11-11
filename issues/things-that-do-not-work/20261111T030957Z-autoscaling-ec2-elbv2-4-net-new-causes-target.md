Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.alb-health-check-marks-ec2-instance-unhealthy autoscaling__ec2__elbv2.alb-listener-routes-traffic-to-registered-ec2-instance autoscaling__ec2__elbv2.alb-target-group-deregisters-ec2-instance-on-terminate autoscaling__ec2__elbv2.asghc-elb-type-group-replaces-its-unhealthy-instance-while-an-ec2-type-group-keeps-its-own autoscaling__ec2__elbv2.target-group-register-fails-instance-wrong-vpc autoscaling__ec2__elbv2.target-group-registration-denied-without-elasticloadbalancing-permission
Issue-Key: autoscaling__ec2__elbv2.alb-health-check-marks-ec2-instance-unhealthy|_poll_target_state#10|AssertionError|raise AssertionError(f"target never reached {desired_states}, last_state={last_state} reason={last_reason}")
Issue-Key: autoscaling__ec2__elbv2.alb-listener-routes-traffic-to-registered-ec2-instance|_fetch_body_with_retry#13|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.alb-target-group-deregisters-ec2-instance-on-terminate|run#15|Timeout|time.sleep(2.0)
Issue-Key: autoscaling__ec2__elbv2.asghc-elb-type-group-replaces-its-unhealthy-instance-while-an-ec2-type-group-keeps-its-own|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.target-group-register-fails-instance-wrong-vpc|run#9|AssertionError|assert message == expected_message, f"InvalidTarget message {message!r}, expected {expected_message!r}"
Issue-Key: autoscaling__ec2__elbv2.target-group-registration-denied-without-elasticloadbalancing-permission|_error#8|AssertionError|raise AssertionError(f"expected the call to be denied, but it succeeded and returned {result!r}")
Issue-Commit: deeb1944d1d0db8b9764b546c3d5c87bdcf95957

# autoscaling__ec2__elbv2: 4 net-new causes — target health is synthesized 'healthy' with no probe, the EC2/ALB target lifecycle never converges, and RegisterTargets neither words InvalidTarget as AWS does nor enforces IAM

> _Note: this issue was identified against the code base at commit `deeb1944d1d0db8b9764b546c3d5c87bdcf95957`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
Four distinct net-new root causes in the Auto Scaling + EC2 + ELBv2 surface. They are bundled because they share one service heading; a fixer must close all four before submitting.

=== Cause 1 of 4: DescribeTargetHealth reports a constant 'healthy' — the target group never performs the configured HTTP health check ===
AWS: a target group whose HealthCheckPath points at a path the instance's web server answers 404 on fails the matcher. After UnhealthyThresholdCount consecutive failed probes (here 2) spaced HealthCheckIntervalSeconds apart (here 10 s), DescribeTargetHealth reports TargetHealth.State = "unhealthy" with TargetHealth.Reason = "Target.ResponseCodeMismatch" (Smithy: elbv2 DescribeTargetHealth → TargetHealthDescription.TargetHealth; TargetHealthStateEnum includes "unhealthy"; TargetHealthReasonEnum includes "Target.ResponseCodeMismatch"). The verdict cannot legitimately arrive sooner than UnhealthyThresholdCount x HealthCheckIntervalSeconds.
Emulator: the target is reported State="healthy", Reason absent, immediately and permanently, regardless of what the target actually answers on the health path. The check polled for 300 s and never saw any state other than 'healthy'.
Why: a client cannot test any unhealthy-target behaviour — failover, replacement, fail-open, alarming — because every registered target is asserted healthy. Negative controls silently pass and real outages are invisible.
Fix: the target-group health-check scheduler and the DescribeTargetHealth state derivation in the elbv2 service (target state must come from an actual probe of HealthCheckProtocol/Port/Path against the registered target, with HealthyThresholdCount/UnhealthyThresholdCount counting and the Reason enum set from the failure class), not a constant in the DescribeTargetHealth serializer.
Traces: autoscaling__ec2__elbv2.alb-health-check-marks-ec2-instance-unhealthy (AssertionError: target never reached {'unhealthy'}, last_state=healthy reason=None).

=== Cause 2 of 4: the EC2-instance-target lifecycle behind an ALB never reaches any observable step ===
AWS: after RegisterTargets an instance target becomes healthy and the listener forwards HTTP requests to it; terminating the instance moves the target to 'draining' and then removes it from the target group; an Auto Scaling group with HealthCheckType=ELB derives Unhealthy from target health and replaces the instance, while an EC2-health-type group in the same account keeps its own instance. Each of these is an observable state transition that completes in tens of seconds.
Emulator: recorded behavior only — all three checks burn their whole budget and are killed ("Check timed out after 60.0s") without ever reaching their first assertion, so no transition is ever produced: no forwarded response at the load balancer, no deregistration on terminate, no ELB-derived replacement.
Why: every ASG/ELBv2 integration a client builds on (rolling replacement, draining on scale-in, ELB-driven health) hangs instead of converging, so the whole family of behaviours is untestable against the emulator.
Fix: the ELBv2 target registration/deregistration state machine and its data plane, plus the Auto Scaling health-check reconciliation loop that consumes target health. Related open issues describe neighbouring symptoms but do not carry these check keys: 20261107T075601Z-autoscaling-ec2-elbv2-auto-scaling-elbv2-asg-instances-never-register-with.md and 20261108T150952Z-autoscaling-ec2-elbv2-an-elb-health-check-auto.md.
Traces: autoscaling__ec2__elbv2.alb-listener-routes-traffic-to-registered-ec2-instance, autoscaling__ec2__elbv2.alb-target-group-deregisters-ec2-instance-on-terminate, autoscaling__ec2__elbv2.asghc-elb-type-group-replaces-its-unhealthy-instance-while-an-ec2-type-group-keeps-its-own.

=== Cause 3 of 4: RegisterTargets' InvalidTarget message for a cross-VPC instance is worded differently from AWS ===
AWS: registering an EC2 instance that lives in a VPC other than the target group's VPC is refused with HTTP 400 and the awsQuery error code InvalidTarget (Smithy: RegisterTargets errors include InvalidTargetException, awsQueryError code "InvalidTarget", httpResponseCode 400). The message is the fixed text "The following targets are not in the target group VPC '<vpc-id>': '<target-id>'" — fixed parts: "The following targets are not in the target group VPC", the quoting and the colon; variable parts: the target group's VpcId taken from emulator state and the offending Targets[].Id taken from the request (a list, comma-separated when more than one).
Emulator: same code InvalidTarget, but the message is "The target '<instance-id>' is in VPC <vpc-of-instance>, not in VPC <vpc-of-target-group> of the target group" — different fixed text, singular, and it leaks the instance's own VPC which AWS does not name.
Why: clients and CLI wrappers that parse or match on this message (and users reading it) get text AWS never emits; the message is part of the error contract, so a diverging one is a defect even though the code matches.
Fix: the InvalidTarget message construction in the RegisterTargets handler's validation for the cross-VPC case.
Traces: autoscaling__ec2__elbv2.target-group-register-fails-instance-wrong-vpc.

=== Cause 4 of 4: RegisterTargets performs no IAM authorization — a zero-permission role's call is answered 200 OK ===
AWS: elbv2 speaks the awsQuery protocol, so an implicit IAM deny is returned as HTTP 403 with error code "AccessDenied" (not "AccessDeniedException") and the message "User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: elasticloadbalancing:RegisterTargets on resource: <target-group-arn> because no identity-based policy allows the elasticloadbalancing:RegisterTargets action" — fixed text: "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action"; variable parts: the caller's assumed-role ARN (account id, role name, session name) from the signed credentials, the action name, and the TargetGroupArn from the request. No target state is written. (Smithy: RegisterTargets models only InvalidTarget/TargetGroupNotFound/TooManyRegistrationsForTargetId/TooManyTargets; the IAM deny is the protocol-common error, confirmed by the recording.)
Emulator: the call made with credentials for an IAM role that has zero attached policies succeeds — HTTP 200, empty RegisterTargetsOutput — and the target is registered.
Why: every permission-boundary test inverts. Code that is supposed to be blocked by a missing policy silently works against the emulator and then fails in production, and least-privilege policies cannot be validated at all.
Fix: request authorization for the elbv2 awsQuery operations (an identity/authorization layer consulted by the RegisterTargets handler before mutating target state) and the AccessDenied awsQuery error serializer. Note: 20261110T211308Z-autoscaling-ecs-elbv2-2-net-new-causes-registertargets.md reports the same shape of defect under a different service heading and does not carry this check key.
Traces: autoscaling__ec2__elbv2.target-group-registration-denied-without-elasticloadbalancing-permission.

## Evidence
Cause 1 — expected State='unhealthy', Reason='Target.ResponseCodeMismatch' after >= 2 x 10 s of probing; got, for 300 s of polling: AssertionError: target never reached {'unhealthy'}, last_state=healthy reason=None.

Cause 2 — expected the listener to forward to the registered instance / the terminated instance to leave the target group / the ELB-health ASG to replace its instance; got, for all three: "Error: Check timed out after 60.0s" with no assertion reached.

Cause 3 — expected InvalidTarget message "The following targets are not in the target group VPC '<tg-vpc>': '<instance-id>'"; got "The target '<instance-id>' is in VPC <instance-vpc>, not in VPC <tg-vpc> of the target group". (Recorded ids in the replay were i-00000000000000001 / vpc-0000000000000009b / vpc-00000000000000080; those values fill the template and are not part of it.)

Cause 4 — expected ClientError with code 'AccessDenied' (HTTP 403) and the "User: ... is not authorized to perform: elasticloadbalancing:RegisterTargets on resource: <tg-arn> because no identity-based policy allows the elasticloadbalancing:RegisterTargets action" message, and DescribeTargetHealth still empty; got AssertionError: expected the call to be denied, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', ...}}}.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite) so the emulator is built and serving.
2. Cause 1: create a VPC/subnet/SG, launch an instance serving HTTP on /, create a target group with HealthCheckPath=/sim2real-missing-path, HealthCheckIntervalSeconds=10, HealthyThresholdCount=2, UnhealthyThresholdCount=2, create an ALB and an HTTP:80 listener forwarding to it, RegisterTargets the instance, then poll DescribeTargetHealth — it stays State=healthy with no Reason.
3. Cause 2: replay autoscaling__ec2__elbv2.alb-listener-routes-traffic-to-registered-ec2-instance, .alb-target-group-deregisters-ec2-instance-on-terminate and .asghc-elb-type-group-replaces-its-unhealthy-instance-while-an-ec2-type-group-keeps-its-own; each hangs out its budget.
4. Cause 3: create two VPCs, a target group in VPC A and an instance in VPC B, then RegisterTargets the VPC-B instance into the VPC-A group and read the error message.
5. Cause 4: create an IAM role with no attached policies, AssumeRole, build an elbv2 client with those credentials and call RegisterTargets on an existing target group; it returns 200 and the target is registered.
