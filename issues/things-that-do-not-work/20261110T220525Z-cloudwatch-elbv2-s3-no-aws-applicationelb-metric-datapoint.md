Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class
Issue-Key: cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered|_poll#23|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class|_poll#13|Timeout|time.sleep(interval_s)
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# cloudwatch__elbv2__s3: no AWS/ApplicationELB metric datapoint or access-log record ever lands for Lambda-target ALB traffic — all six metric/access-log checks exhaust their 600 s budget

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
One net-new root cause with six grounded checks (Cause 1 of 1).

Cause 1 of 1: an ALB whose target group holds a Lambda target produces neither AWS/ApplicationELB metric datapoints nor S3 access-log records for the requests it serves, so every observability behaviour that distinguishes one failure class from another is unobservable and each check burns its full 600 s ingestion window.
AWS: Smithy (elasticloadbalancingv2) RegisterTargets into a lambda-type TargetGroup; ModifyLoadBalancerAttributes with access_logs.s3.enabled / access_logs.s3.bucket / access_logs.s3.prefix (LoadBalancerAttribute.Key documents these as the access-log attributes, and that the bucket must have a policy granting Elastic Load Balancing permission to write). Smithy (cloudwatch) GetMetricData / GetMetricStatistics over namespace AWS/ApplicationELB. AWS behaviour: each request the load balancer answers is counted into the per-minute series — RequestCount, HTTPCode_Target_5XX_Count for a 5xx the target itself returned, HTTPCode_ELB_5XX_Count for a load-balancer-generated 5xx, and LambdaUserError for an invocation that raised or returned a response the load balancer cannot use — with the LoadBalancer dimension (app/<name>/<id>) and, for target-scoped metrics, the TargetGroup dimension (targetgroup/<name>/<id>); and each request is written as one access-log line whose fields follow the documented order, where a Lambda-target entry carries the function's status and NO target address, and `error_reason` names the failure class (e.g. LambdaUnhandled for a raised handler, LambdaInvalidResponse for a response with no statusCode). The split of 5xx between target-originated and ELB-originated, and the per-target-group narrowing of the target-5xx series, are the requirements; the specific counts and names are request-derived.
Emulator: every one of the six checks is killed by its ceiling — "Check timed out after 600.0s" — i.e. within a 10-minute ingestion window no datapoint and no access-log object ever appears for traffic the load balancer was asked to serve. Nothing distinguishes a crashed invocation from a malformed response from a target 500, and no access-log line exists in which to read the function status, the absent target address or the error reason.
Why: observability is the only way a client learns which side of the load balancer failed. With no metrics and no access logs, a 5xx is indistinguishable from a target error, alarms and metric-math expressions (a 5xx rate derived from target counts) evaluate against empty series and never leave INSUFFICIENT_DATA, and Athena tables built on the documented access-log field order have nothing to read. A client debugging a Lambda-backed ALB has no signal at all.
Fix: the ELBv2 observability components — the AWS/ApplicationELB metric publisher (per-request RequestCount / HTTPCode_Target_5XX_Count / HTTPCode_ELB_5XX_Count / LambdaUserError emission with the LoadBalancer and TargetGroup dimensions, and the target-vs-ELB attribution of a 5xx) and the access-log writer (one documented-field-order line per request, delivered under access_logs.s3.bucket/prefix, with the Lambda-target shape: function status, no target address, and error_reason naming the failure class) — plus the Lambda-target invocation path they both observe.
Related open filings that do NOT carry these replay keys: 20261104T055008Z-cloudwatch-elbv2-s3-5-net-new-root-causes.md and 20261109T153103Z-cloudwatch-elbv2-s3-the-alb-s-published-dnsname.md cover the access-log configuration validation and the earlier published-address defect under this heading; the suite did not mark any of the six checks below OWNED, so they survive as live failures.
Traces: cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx, cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered, cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s, cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts, cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address, cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator; service heading cloudwatch__elbv2__s3 3/16 (19%), 13 failed (7 OWNED; the six ids in trace_ids survive).

All six fail identically — the emulator produced nothing to grade inside the declared ingestion window:
[mismatch] cloudwatch__elbv2__s3.target-500s-count-as-target-5xx-not-elb-5xx — Error: Check timed out after 600.0s
[mismatch] cloudwatch__elbv2__s3.target-5xx-metric-narrows-to-the-target-group-that-answered — Error: Check timed out after 600.0s
[mismatch] cloudwatch__elbv2__s3.lambda-user-error-counts-crashed-and-malformed-invocations-not-target-500s — Error: Check timed out after 600.0s
[mismatch] cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts — Error: Check timed out after 600.0s
[mismatch] cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address — Error: Check timed out after 600.0s
[mismatch] cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class — Error: Check timed out after 600.0s

Each check's own setup: a Lambda handler answering 200 on /health, the function's own 500 on unmatched paths, raising on /crash, and returning a body with no statusCode on /malformed; the function registered as the only target of a lambda-type target group behind an ALB listener; access logs enabled via ModifyLoadBalancerAttributes with a bucket whose policy grants logdelivery.elasticloadbalancing.amazonaws.com; metrics read from AWS/ApplicationELB with _METRIC_DEADLINE_S = 600 and access logs polled with _LOG_DEADLINE_S = 900.

Expected (AWS): /crash and /malformed increment LambdaUserError and HTTPCode_ELB_5XX_Count (not HTTPCode_Target_5XX_Count); the handler's own 500 increments HTTPCode_Target_5XX_Count scoped to the TargetGroup dimension of the group that answered; a metric-math 5xx rate evaluates over those series; and one access-log line per request is delivered under the configured prefix, with the documented field order, the function status, no target:port address, and an error_reason naming the failure class.
Actual (emulator): no metric datapoint and no access-log object appears at all within 600 s.

Smithy consulted (model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): CreateTargetGroup/RegisterTargets for TargetType 'lambda'; LoadBalancerAttribute.Key documentation for access_logs.s3.enabled / access_logs.s3.bucket / access_logs.s3.prefix. Metric names, the target-vs-ELB 5xx split and the access-log field order: recorded behavior plus the ALB CloudWatch-metrics and access-log documentation the checks cite.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; all six ids in trace_ids reproduce.
2. Create a Lambda function whose handler returns 200 on /health, raises on /crash, returns a body with no statusCode on /malformed, and returns statusCode 500 otherwise. Grant elasticloadbalancing.amazonaws.com lambda:InvokeFunction.
3. Create a lambda-type target group, RegisterTargets the function, create an ALB with an HTTP listener forwarding to it, and wait for the load balancer to become active.
4. ModifyLoadBalancerAttributes with access_logs.s3.enabled=true, access_logs.s3.bucket=<bucket with the ELB log-delivery policy>, access_logs.s3.prefix=<prefix>.
5. Send requests to /health, /crash, /malformed and an unmatched path through the load balancer.
6. Read AWS/ApplicationELB RequestCount, HTTPCode_Target_5XX_Count, HTTPCode_ELB_5XX_Count and LambdaUserError for the LoadBalancer (and TargetGroup) dimensions. Expected: the documented attribution per request class. Actual: no datapoints within 600 s.
7. List the bucket under the configured prefix and gunzip the access-log object. Expected: one line per request, documented field order, Lambda-target shape (function status, no target address, error_reason naming the class). Actual: no object is ever written.
8. Re-replay the six ids in trace_ids to confirm the fix.
