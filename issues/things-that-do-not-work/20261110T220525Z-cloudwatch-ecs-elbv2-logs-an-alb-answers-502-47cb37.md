Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets cloudwatch__ecs__elbv2__logs.alb-request-count-metric-in-cloudwatch cloudwatch__ecs__elbv2__logs.alb-target-response-time-metric-in-cloudwatch cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets|run#29|AssertionError|assert status == 200, f"expected 200 from first reachable response, got {status}"
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-request-count-metric-in-cloudwatch|run#14|AssertionError|assert ok >= 20, f"expected most of 40 requests to succeed against a healthy ALB, got {ok} OK"
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-target-response-time-metric-in-cloudwatch|run#15|AssertionError|assert len(latencies) >= 10, f"too few successful requests against the ALB to measure latency: {latencies}"
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# cloudwatch__ecs__elbv2__logs: an ALB answers 502 (or nothing at all) for the ECS targets it reports healthy, so no request ever succeeds and no ApplicationELB request/latency metric accrues

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One net-new root cause with four grounded checks (Cause 1 of 1).

Cause 1 of 1: an ALB fronting an awsvpc ECS service never serves a successful HTTP response from its registered target — the request is answered with a load-balancer-generated HTTP 502, or the target never converges and the check exhausts its budget. Because no request succeeds, the AWS/ApplicationELB RequestCount and TargetResponseTime series that the metric checks read stay empty.
AWS: Smithy (elasticloadbalancingv2) CreateListener -> DefaultActions[] {Type 'forward', TargetGroupArn}; DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth {State, Reason, Description}. On AWS, once the ECS task is RUNNING and its ip target reaches State 'healthy', a GET to the load balancer is forwarded to the container and the container's response (HTTP 200 for nginx/httpd on "/") is returned verbatim; a 502 is reserved for the case where the load balancer could not get a well-formed response from the target (Reason class Target.FailedHealthChecks / a malformed or refused target response). Per the ALB CloudWatch metrics documentation, each forwarded request increments RequestCount and publishes a TargetResponseTime sample under AWS/ApplicationELB with the LoadBalancer (and TargetGroup) dimensions.
Emulator: the first reachable response through the ALB is HTTP 502 ("expected 200 from first reachable response, got 502"); in the metric checks none of 40 requests succeed ("expected most of 40 requests to succeed against a healthy ALB, got 0 OK") and no latency sample is collected ("too few successful requests against the ALB to measure latency: []"); and the health-convergence check never settles at all ("Check timed out after 60.0s"). The counts 40/20/10 are the checks' own thresholds; the defect is that the success count is zero.
Why: a client that stands up the canonical ALB+ECS topology gets a 502 for every request even though the control plane reported the target healthy, so the service appears broken with no diagnosable cause; and every downstream behaviour built on real traffic — RequestCount, TargetResponseTime, alarms on them, health-based routing, access logs — is unobservable. A 502 that the target never produced also mislabels the failure: the client blames its container.
Fix: the ELBv2 data-plane forwarding path — the listener/rule action evaluation that resolves a target group to a registered target's address and proxies the request/response — together with the target-health state machine that must only report 'healthy' for a target the load balancer can actually reach, and the AWS/ApplicationELB metric publisher that must emit RequestCount/TargetResponseTime per forwarded request.
Related open filings that do NOT carry these replay keys: 20261108T191814Z-cloudwatch-ecs-elbv2-logs-an-alb-answers-502.md and 20261105T143800Z-cloudwatch-ecs-elbv2-logs-3-net-new-causes.md describe adjacent ALB/ECS target defects under this heading, and 20261109T143515Z-cloudwatch-ecs-elbv2-logs-an-alb-s-dnsname-is-published-as-name.md covers the earlier published-address defect; the suite did not mark any of the four checks below OWNED, so they survive as live failures.
Traces: cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets (the 502), cloudwatch__ecs__elbv2__logs.alb-request-count-metric-in-cloudwatch and cloudwatch__ecs__elbv2__logs.alb-target-response-time-metric-in-cloudwatch (0 successful requests, no latency samples), cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health (never converges).

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator; service heading cloudwatch__ecs__elbv2__logs 2/22 (9%), 19 failed (15 OWNED/ALREADY FILED; the four ids in trace_ids survive).

[mismatch] cloudwatch__ecs__elbv2__logs.alb-routes-traffic-only-to-healthy-ecs-targets
  File "check_...alb_routes_traffic_only_to_healthy_ecs_targets.py", line 76, in run -> assert status == 200
  AssertionError: expected 200 from first reachable response, got 502
  Expected: HTTP 200 with the healthy task's body; the unreachable sibling target excluded from routing. Actual: HTTP 502 from the load balancer.

[mismatch] cloudwatch__ecs__elbv2__logs.alb-request-count-metric-in-cloudwatch
  File "check_...alb_request_count_metric_in_cloudwatch.py", line 37, in run -> assert ok >= 20
  AssertionError: expected most of 40 requests to succeed against a healthy ALB, got 0 OK
  Expected: the requests succeed and AWS/ApplicationELB RequestCount accrues for the LoadBalancer dimension. Actual: zero successes.

[mismatch] cloudwatch__ecs__elbv2__logs.alb-target-response-time-metric-in-cloudwatch
  File "check_...alb_target_response_time_metric_in_cloudwatch.py", line 38, in run -> assert len(latencies) >= 10
  AssertionError: too few successful requests against the ALB to measure latency: []
  Expected: measured latencies and a TargetResponseTime series reflecting them. Actual: no successful request at all.

[mismatch] cloudwatch__ecs__elbv2__logs.alb-health-check-reflects-ecs-task-health
  Error: Check timed out after 60.0s
  (waits for the ip target to turn 'healthy' once the container answers the health-check path, then for the ALB to serve the container's content)

Smithy consulted (model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): CreateListener DefaultActions[].{Type,TargetGroupArn}; DescribeTargetHealth output TargetHealthDescriptions[].TargetHealth {State, Reason, Description} (Reason documents Target.FailedHealthChecks / Target.Timeout / Target.ResponseCodeMismatch as the unhealthy reason codes). ALB metric semantics (RequestCount, TargetResponseTime under AWS/ApplicationELB with the LoadBalancer/TargetGroup dimensions): recorded behavior plus the ALB CloudWatch metrics documentation the checks cite.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; all four ids in trace_ids reproduce.
2. Create an ALB (2 subnets, SG open on 80), an ip-type target group on port 80 with health-check path "/", and an HTTP:80 listener forwarding to it.
3. Register an awsvpc Fargate ECS service running nginx/httpd into that target group and wait for DescribeTargetHealth State 'healthy'.
4. GET http://<load balancer address>/. Expected: HTTP 200 with the container's body. Actual: HTTP 502.
5. Send ~40 requests, then read AWS/ApplicationELB RequestCount and TargetResponseTime for the LoadBalancer dimension over the covering 60 s periods. Expected: a nonzero RequestCount sum and latency samples. Actual: no request succeeds, so both series are empty.
6. Register a second, permanently unreachable target in the same group and confirm it is reported unhealthy and excluded from routing while the healthy task serves every request.
7. Re-replay the four ids in trace_ids to confirm the fix.
