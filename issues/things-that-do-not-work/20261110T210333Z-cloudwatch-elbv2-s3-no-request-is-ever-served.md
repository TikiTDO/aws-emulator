Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm
Issue-Key: cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: 41bd6c476a5fe40a474aadc7e47c434277d677d7

# cloudwatch__elbv2__s3: no request is ever served through an ALB with a Lambda target group, so neither the access-log records nor the AWS/ApplicationELB target metrics derived from those requests ever appear

> _Note: this issue was identified against the code base at commit `41bd6c476a5fe40a474aadc7e47c434277d677d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
Cause 1 of 1: the ALB data plane never answers a request on the load balancer's announced address, so every signal that is produced BY a served request — access-log records and the AWS/ApplicationELB request/target metrics — stays absent and each of these checks exhausts its whole budget without reaching a verdict.
AWS: recorded behavior. An ALB fronting a Lambda target group leaves `provisioning` for `active`, answers a warm-up `GET /health` with HTTP 200 through the target, and from then on each class of Lambda outcome is reported consistently in two places. In the delivered access log (objects under `AWSLogs/<account>/elasticloadbalancing/<region>/`, one record per request): a handler that raises logs elb_status_code 502, target_status_code `-` and error_reason `LambdaUnhandled`; a handler returning a reply with no statusCode logs 502, `-` and `LambdaInvalidResponse`; a handler returning its own 500 logs 500, 500 and `-`; a Lambda-target record carries the function status and no target address. In CloudWatch: `HTTPCode_Target_5XX_Count` counts the target-returned 500s (and not ELB-generated 5xx), narrows to the target group that actually answered, `LambdaUserError` counts the crashed and malformed invocations rather than target 500s, and a metric-math 5xx rate derived from those counts and `RequestCount` moves with them (an alarm on the rate staying OK while a count alarm fires).
Emulator: all four of these checks run their full 600 s budget and time out with the condition never satisfied. They get past the load balancer's provisioning poll and then stall waiting for the load balancer to answer a request and for log objects / metric datapoints to appear — nothing is ever served, so no access-log object and no AWS/ApplicationELB datapoint is produced. The sibling check elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni shows the same load balancer endpoint refusing TCP connections outright (ConnectionRefusedError(111) for 420 s), i.e. the same missing data plane reached through a different listener protocol.
Why: nothing that depends on an ALB serving traffic can be observed. Access-log based diagnosis of a failing target, and every ApplicationELB/Lambda-target metric and alarm built on request counts, are permanently empty rather than wrong — a client cannot distinguish a healthy deployment from a broken one, and an alarm that should fire never leaves INSUFFICIENT_DATA.
Fix: the ELBv2 listener/target-group data plane first (serve the address the load balancer announces, invoke the Lambda target, and classify its reply into the ELB/target status pair and the error_reason), then the access-log writer and the AWS/ApplicationELB metric publication fed by those served requests. Adjacent open issues 20261109T153103Z-cloudwatch-elbv2-s3-the-alb-s-published-dnsname.md, 20261107T183213Z-cloudwatch-elbv2-s3-enabling-alb-access-logs-writes.md and 20261104T055008Z-cloudwatch-elbv2-s3-5-net-new-root-causes.md describe this area but do not carry these checks' replay keys.
Traces: the four check ids in trace_ids.

## Evidence
All four checks end with `Error: Check timed out after 600.0s` and no response recorded:
- cloudwatch__elbv2__s3.access-log-error-reason-names-the-lambda-target-failure-class — expects the delivered log to hold 6 records for the run with (path, elb_status, target_status, error_reason) = ('/crash','502','-','LambdaUnhandled')×2, ('/malformed','502','-','LambdaInvalidResponse')×2, ('/answered','500','500','-')×2.
- cloudwatch__elbv2__s3.access-log-lambda-target-entries-carry-the-function-status-and-no-target-address.
- cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts.
- cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm.
The precondition each of them waits on — `_poll(... describe_load_balancers ... State.Code in ('active','failed'), timeout_s=420)` followed by warm-up `GET /health` returning 200 through the load balancer — is the same one the sibling elbv2__s3 check reports as `ConnectionRefusedError(111, 'Connection refused')` for 420 s.

## Steps to Reproduce
1. Create an ALB in two subnets, a Lambda target group, and register a handler whose `/crash` path raises, whose `/malformed` path returns a reply with no statusCode, and whose other paths return their own 500; enable access logs to an S3 bucket with the log-delivery bucket policy.
2. Wait for the load balancer to report State.Code `active`, then send `GET /health` to its DNS name.
   Expect: HTTP 200 served through the Lambda target.
   Observe: no response at all — the budget is exhausted waiting (and a direct TCP connect to the same endpoint is refused, see elbv2__s3.log-edge-producers-alb-https-access-log-records-negotiated-tls-and-sni).
3. Send two requests each to /crash, /malformed and /answered, then list the access-log prefix AWSLogs/<account>/elasticloadbalancing/<region>/ — expect six records with the elb/target status pair and error_reason above; observe no objects.
4. Query AWS/ApplicationELB HTTPCode_Target_5XX_Count / LambdaUserError / RequestCount for the load balancer and target group — expect datapoints matching the requests sent; observe none.
5. Re-replay the four check ids in trace_ids to confirm.
