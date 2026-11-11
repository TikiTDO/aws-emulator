Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__s3.log-edge-producers-alb-access-log-502-record-differs-from-target-status
Issue-Key: elbv2__s3.log-edge-producers-alb-access-log-502-record-differs-from-target-status|_poll_until#34|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: deeb1944d1d0db8b9764b546c3d5c87bdcf95957

# elbv2__s3: the ALB answers HTTP 502 for a target it reports healthy, so the 502-vs-target-status access-log record is never produced

> _Note: this issue was identified against the code base at commit `deeb1944d1d0db8b9764b546c3d5c87bdcf95957`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 1: an ALB with a healthy registered target never serves a successful response — it answers HTTP 502 — so the access-log record that must distinguish a load-balancer-generated 502 from the target's own status is never written.

AWS: once a target is registered and its target group reports TargetHealth.State="healthy" (Smithy: elbv2 DescribeTargetHealth → TargetHealthDescription.TargetHealth, TargetHealthStateEnum includes "healthy"), a request through the listener is forwarded to that target and the target's response is returned to the client (here HTTP 200). HTTP 502 is a load-balancer-generated response, emitted only when the target's response is unusable or the target is unreachable; in that case the ALB access log records elb_status_code=502 with target_status_code="-" and an error_reason, which is exactly how a client tells an ALB-generated 502 apart from a 502 the target itself returned.
Emulator: every request to the load balancer is answered 502 with an empty body even while the target is reported healthy — 131 bytes sent, 103 received, status 502, body '' — for the whole 420 s budget, so the check never gets past warming the healthy path and no access-log record of either kind is produced.
Why: the healthy-target data path is unusable, and the one diagnostic that lets a client attribute a 5xx to the load balancer rather than to its application (elb_status_code vs target_status_code in the access log) can never be observed. Any emulator-based validation of 502 triage or of access-log field semantics is impossible.
Fix: the ALB data plane's forwarding path in the elbv2 service — a request to a listener whose target group holds a healthy target must be proxied to that target and its response returned, with the 502 reserved for genuinely unusable target responses; the access-log writer must then record elb_status_code/target_status_code/error_reason for that outcome. Related open issues record the unserved-DNSName and 502-for-healthy-target symptoms elsewhere (20261110T210333Z-elbv2-s3-an-alb-s-https-listener-refuses.md, 20261109T153103Z-elbv2-s3-2-net-new-causes-modifyloadbalancerattributes-enables.md); neither carries this check key.
Traces: elbv2__s3.log-edge-producers-alb-access-log-502-record-differs-from-target-status.

## Evidence
AssertionError: the ALB serving the healthy target not met within 420s. Last result: {'sent': 131, 'received': 103, 'status': 502, 'body': ''} (check line 501, via _poll_until for r.get("status") == 200).
Expected: HTTP 200 from the healthy target on the warm-up path, and subsequently an access-log line in S3 whose elb_status_code is 502 with target_status_code '-' only for the genuinely failing target, distinguishable from a target-returned 502.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite so the emulator is serving.
2. Create an S3 bucket with the ALB log-delivery policy and enable access logs on an ALB via ModifyLoadBalancerAttributes.
3. Create an HTTP listener forwarding to a target group with a target that serves 200 on the marker path, and wait for DescribeTargetHealth to report 'healthy'.
4. GET /<marker>/warm through the load balancer: it answers 502 with an empty body for the full 420 s budget instead of the target's 200, and no access-log object is produced.
