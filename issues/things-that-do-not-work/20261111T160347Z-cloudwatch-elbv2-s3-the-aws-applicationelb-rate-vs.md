Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm
Issue-Key: cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm|_call#17|Timeout|return 0, ""
Issue-Commit: 0e97705e9904239bd55bed92c4be71546a783c93

# cloudwatch__elbv2__s3: the AWS/ApplicationELB rate-vs-count alarm differential never becomes observable — a low target-5xx error rate produces no datapoints, so neither alarm ever transitions

> _Note: this issue was identified against the code base at commit `0e97705e9904239bd55bed92c4be71546a783c93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData / DescribeAlarms (AWS/ApplicationELB HTTPCode_Target_5XX_Count, RequestCount)

## Description
Cause 1 of 1: an ALB that serves a mix of 200s and target 500s publishes no usable AWS/ApplicationELB datapoints, so a metric-math 5xx-RATE alarm and a 5xx-COUNT alarm never reach their distinct states and the check burns its whole 600 s budget without a verdict.
Failing check: cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm.

AWS: recorded behavior only — the CloudWatch and ELBv2 Smithy models describe the metric-data and alarm APIs but state nothing about which datapoints an ALB emits. The recording the check encodes is: an ALB with a Lambda target group whose handler answers 200 on /health and POST /jobInstance/updateStatus and 500 on every other route is driven with a known mix of requests; AWS then publishes AWS/ApplicationELB RequestCount and HTTPCode_Target_5XX_Count datapoints at period 60 for the LoadBalancer (and TargetGroup) dimensions, a 1-of-1 period-60 COUNT alarm on HTTPCode_Target_5XX_Count transitions to ALARM, and a metric-math RATE alarm (5xx / requests) over the same window stays OK because the error RATE is below its threshold. The check treats its 420 s active deadline and 600 s alarm deadline as ceilings to FAIL at, never as asserted latencies, so no per-run timing value is part of the requirement.

Emulator: the check never reaches either assertion and ends "Check timed out after 600.0s". The same family of ALB/Lambda metric and access-log checks in this group fails for one documented reason — no AWS/ApplicationELB datapoint is ever produced for ALB traffic (and the ALB's published DNSName is not an address the emulator serves, so the traffic that would produce them never lands). The open issues 20261110T220525Z-cloudwatch-elbv2-s3-no-aws-applicationelb-metric-datapoint.md ("all six metric/access-log checks") and 20261110T210333Z-cloudwatch-elbv2-s3-no-request-is-ever-served.md describe that behavior for their own checks; NEITHER carries this check id, so this seventh check in the family is not covered by them and is recorded here.

Why: a client relying on AWS's behavior builds its ALB error alarming on the rate/count distinction — a count alarm catches any target 5xx while a rate alarm tolerates a low background error rate — and then gates deployments, rollbacks and pager routing on those two alarms disagreeing. Against the emulator neither alarm ever leaves its initial state, so every such gate silently reads "nothing wrong": a real 5xx burst is invisible and the alarm configuration can never be validated.

Fix: the ALB data path and the AWS/ApplicationELB metric emission behind it — the request path that must serve the load balancer's published endpoint, and the per-request publication of RequestCount / HTTPCode_Target_5XX_Count (with the LoadBalancer and TargetGroup dimensions, at period 60) that CloudWatch's metric-math and alarm evaluation then read. Alarm evaluation itself must transition on those datapoints rather than remaining at INSUFFICIENT_DATA.

Traces: cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm.

## Evidence
cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm [mismatch]: "Error: Check timed out after 600.0s". The check (GROUP alb-metrics, SERVICES cloudwatch/elbv2/s3, namespace _NS = "AWS/ApplicationELB") drives a Lambda-target ALB whose handler answers 200 for POST /jobInstance/updateStatus and /health and 500 for any other route, waits up to _ACTIVE_DEADLINE_S = 420 for the load balancer to leave provisioning and up to _ALARM_DEADLINE_S = 600 for its period-60, 1-of-1 alarms to evaluate — both declared as ceilings to FAIL at, not asserted latencies. Expected on AWS: AWS/ApplicationELB RequestCount and HTTPCode_Target_5XX_Count datapoints for the driven traffic, the COUNT alarm in ALARM and the metric-math RATE alarm still OK. Actual on the emulator: no verdict at all within 600 s — the expected datapoints and alarm transitions never appear. Related open issues covering the same no-datapoint behavior for OTHER checks (neither carries this check id): 20261110T220525Z-cloudwatch-elbv2-s3-no-aws-applicationelb-metric-datapoint.md, 20261110T210333Z-cloudwatch-elbv2-s3-no-request-is-ever-served.md, 20261109T153103Z-cloudwatch-elbv2-s3-the-alb-s-published-dnsname.md.

## Steps to Reproduce
1. Clone the repository and boot the emulator built from it.
2. Replay suite simparity-behavioral-aws-v1 and look at the cloudwatch__elbv2__s3 group.
3. Re-replay cloudwatch__elbv2__s3.target-5xx-rate-alarm-stays-ok-under-a-low-error-rate-that-fires-a-count-alarm: it creates a Lambda function that answers 200 on /health and POST /jobInstance/updateStatus and 500 elsewhere, fronts it with an ALB Lambda target group, drives a mix of succeeding and failing requests at the load balancer's published endpoint, then creates a period-60 1-of-1 alarm on AWS/ApplicationELB HTTPCode_Target_5XX_Count and a metric-math alarm on the 5xx RATE.
4. Observe that the check never reaches either alarm assertion and ends "Check timed out after 600.0s": query GetMetricData for AWS/ApplicationELB RequestCount and HTTPCode_Target_5XX_Count over the driven window and see no datapoints, and DescribeAlarms to see both alarms still in their initial state.
