Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted
Issue-Key: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation|_drain#24|Timeout|got = sqs.receive_message(
Issue-Key: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted|_poll_until#54|Timeout|time.sleep(interval_s)
Issue-Commit: 2d0b651e448fe6a44fa56d00833f67c02b248ecb

# elbv2__lambda: an ALB with a Lambda target never answers on the data path — neither the malformed-response 502 nor the removed-invoke-permission 502 can be observed (2 causes)

> _Note: this issue was identified against the code base at commit `2d0b651e448fe6a44fa56d00833f67c02b248ecb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ELBv2 ALB data path to a Lambda target (RegisterTargets + lambda:InvokeFunction)

## Description
Two net-new causes, both grounded in checks that reach no verdict at all: each is cut off by the harness at its 60 s budget because a request to the load balancer never produces a response.

Cause 1 of 2: a Lambda target that breaks the ALB response contract never yields the load-balancer-generated 502 after the invocation. Failing check: elbv2__lambda.malformed-lambda-target-response-becomes-a-502-after-the-invocation.
AWS: recorded behavior only (the ALB data path is not modeled in Smithy; elasticloadbalancingv2 models only the control plane). When a registered Lambda target returns a response that violates the ALB contract (missing/invalid statusCode, a body over the documented 1 MB response limit, or an unhandled error), the load balancer answers the client HTTP 502 while the function IS invoked each time — the side channel (the handler's own sqs:SendMessage) proves the invocation happened — and the next well-formed response is a 200 again.
Emulator: Error: Check timed out after 60.0s — no request to the ALB is ever answered, so neither the 502 nor the proof-of-invocation nor the recovery 200 is observed.

Cause 2 of 2: removing lambda:InvokeFunction from a registered Lambda target is never re-checked on the data path. Failing check: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted.
AWS: recorded behavior only. Removing the ALB's lambda:InvokeFunction permission from a registered target makes every request a 502 with the function never invoked and the access log record reading LambdaAccessDenied; re-adding the permission restores 200 with no re-registration, i.e. the permission is evaluated on every invoke rather than only at RegisterTargets.
Emulator: Error: Check timed out after 60.0s — no request is answered, so neither the denied 502 nor the restored 200 nor the LambdaAccessDenied access-log record is observable.

Why (both): a client in front of an ALB with a Lambda target gets a hung connection instead of the load balancer's own 502, so a broken backend contract and a revoked invoke permission are indistinguishable from each other and from a healthy target; the documented 'the ALB answers 502, the function is/was not invoked' contract cannot be relied on.

Fix: the ELBv2 listener data path for Lambda targets — serve requests at the load balancer's published endpoint, invoke the target, validate the target's response against the ALB contract and generate the 502, evaluate lambda:InvokeFunction per invoke, and write the access-log record (LambdaAccessDenied) for the denial.

Related but not covering: open issues 20261106T151148Z-elbv2-lambda-a-malformed-lambda-target-response-never-c548cb.md, 20261105T093535Z-elbv2-lambda-a-lambda-target-that-breaks-the.md and 20261109T153103Z-elbv2-lambda-2-net-new-causes-the-alb.md (the latter reports the published DNSName being an unserved elb.amazonaws.com host — Hypothesis: that is why both of these checks hang) describe this area but carry neither check's replay key.

Traces: the two failing check ids above.

## Evidence
Cause 1 — Expected (AWS): client gets HTTP 502 for each malformed/oversize/raising target response, the SQS side channel shows one invocation per request, and a well-formed response is served 200. Actual (emulator): Error: Check timed out after 60.0s (no response at all).
Cause 2 — Expected (AWS): with lambda:InvokeFunction removed, every request is 502, the function is not invoked, and the access log record reads LambdaAccessDenied; re-adding the permission serves 200 without re-registration. Actual (emulator): Error: Check timed out after 60.0s (no response at all).

## Steps to Reproduce
1. Create a VPC/subnets/security group, an internet-facing ALB, a target group of type lambda, a Lambda target (with a queue for the side channel and a bucket for access logs) and RegisterTargets.
2. Send an HTTP request through the load balancer's published endpoint for each handler mode: well-formed, missing statusCode, >1 MB body, raising.
3. RemovePermission for the ALB's lambda:InvokeFunction and request again; then AddPermission and request once more.
4. Observe that no request through the load balancer is ever answered and both checks are killed at 60 s.
