Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted
Issue-Key: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted|_poll_until#54|Timeout|last = probe()
Issue-Commit: f254b8757d5a83e24681f3a32580411f8973ffbe

# elbv2__lambda: removing lambda:InvokeFunction from a registered Lambda target is never re-evaluated on the ALB data path — the request hangs instead of answering 502 LambdaAccessDenied, and re-granting never restores 200

> _Note: this issue was identified against the code base at commit `f254b8757d5a83e24681f3a32580411f8973ffbe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
Cause 1 of 1: an ALB with a `lambda`-type target group never answers a request on its data path, so the per-invoke evaluation of `lambda:InvokeFunction` cannot be observed at all. Failing check: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted.

AWS: recorded behavior only — the ELBv2 listener data path is not modelled (Smithy: the elasticloadbalancingv2 service binds only control-plane operations such as CreateLoadBalancer / CreateTargetGroup / RegisterTargets / DescribeTargetHealth / ModifyLoadBalancerAttributes; there is no modelled operation for serving a request through a load balancer, and the Lambda side is lambda AddPermission / RemovePermission / Invoke). The recorded behavior is: with a Lambda target registered and the `elasticloadbalancing.amazonaws.com` `lambda:InvokeFunction` permission in place, a request through the load balancer's published endpoint is served HTTP 200 from the function's response. After RemovePermission removes that statement, EVERY subsequent request is answered by the load balancer itself with HTTP 502 and the function is NOT invoked, and the ALB access-log record for that request carries `error_reason` = `LambdaAccessDenied` (field 25 of the documented ALB access-log line, with `elb_status_code` 502 and no `target_status_code`). A following AddPermission that restores the same statement makes the next request 200 again with NO re-registration of the target — i.e. the permission is evaluated on every invoke, not only at RegisterTargets time. The 502 body/headers are load-balancer-generated; the fixed text that is a requirement here is the status code 502 and the access-log `error_reason` token `LambdaAccessDenied`; the variable parts (load balancer name/ARN, target group ARN, function ARN, client address, timestamps, trace id) come from the request and the caller's own state.

Emulator: the check never reaches any of those three observations. Replay output is `Error: Check timed out after 600.0s` — the harness kills the check at its 600 s budget while it is inside its own bounded retry (`_poll_until`, which polls a probe of the load balancer endpoint and raises on timeout). No request through the load balancer is ever answered: there is neither the 200 before the permission is removed, nor the 502, nor the `LambdaAccessDenied` access-log record, nor the restored 200. The emulator therefore exposes no status code and no error code at all for this path, rather than a wrong one.

Why: a client in front of an ALB whose target is a Lambda function gets a hung connection instead of the load balancer's own answer. A revoked invoke permission is then indistinguishable from a healthy target, from a broken backend contract, and from a dead load balancer: code that relies on 'the ALB answers 502 and the function is not invoked when lambda:InvokeFunction is missing, and recovers to 200 as soon as the permission is restored' (the standard way an ALB+Lambda deployment is verified and the standard way a least-privilege regression is detected) can never be exercised. Any test or retry/alarm logic keyed on that 502 silently waits forever instead.

Fix: the ELBv2 listener data path for `lambda`-type target groups — it must serve requests at the load balancer's published endpoint, evaluate `lambda:InvokeFunction` for the `elasticloadbalancing.amazonaws.com` principal on EVERY invoke (not only at RegisterTargets), generate the load-balancer 502 without invoking the function when that permission is absent, serve the function's response as 200 once the permission is restored without re-registration, and write the access-log record whose `error_reason` is `LambdaAccessDenied`.

Already-filed issues that do NOT cover this check: open issue 20261110T210703Z-elbv2-lambda-an-alb-with-a-lambda-target.md describes this exact pair of causes but the replay suite does not report this check as owned by it (its recorded replay key pins the old 60 s budget and the `_poll_until#54` frame, while this run fails at 600.0 s), so this check is unowned and is filed here; open issue 20261109T153103Z-elbv2-lambda-2-net-new-causes-the-alb.md reports the published DNSName being an unserved elb.amazonaws.com host and every Lambda target being reported healthy regardless (Hypothesis: that unserved endpoint is why this check hangs) but carries neither this check's replay key; open issue 20261110T143849Z-elbv2-lambda-a-lambda-target-whose-health-path.md covers only target health. Fixed issue 20261105T151413Z-elbv2-lambda-a-malformed-lambda-target-response-never.md claimed the sibling malformed-response cause, a different check id; this permission check still never reaches a verdict.

Traces: elbv2__lambda.xsvc-elbv2-3-lambda-invoke-permission-removed-is-denied-with-502-until-regranted (verdict: mismatch; detail: `Error: Check timed out after 600.0s`).

## Evidence
Expected (AWS, recorded): (1) request through the ALB's published endpoint -> HTTP 200 carrying the Lambda target's response; (2) after RemovePermission of the elasticloadbalancing.amazonaws.com lambda:InvokeFunction statement, every request -> HTTP 502 generated by the load balancer, function NOT invoked, ALB access-log record with elb_status_code 502 and error_reason=LambdaAccessDenied; (3) after AddPermission restores the statement, the next request -> HTTP 200 with no re-registration of the target.

Actual (emulator): no request through the load balancer is answered at any point. The check is cut off inside its own bounded retry loop and the suite reports `Error: Check timed out after 600.0s`. No HTTP status, no error code, and no access-log record are produced for the denied, the healthy, or the restored case.

## Steps to Reproduce
1. Create a VPC with two subnets in distinct AZs, an internet gateway and a security group; create an internet-facing application load balancer with an HTTP listener and a target group of Type=lambda.
2. Create a Lambda function, AddPermission for principal elasticloadbalancing.amazonaws.com with Action=lambda:InvokeFunction scoped to the target group, then RegisterTargets with the function ARN; wait for the load balancer to leave 'provisioning' and become active.
3. GET the load balancer's published endpoint: expect HTTP 200 from the function. (Emulator: nothing answers.)
4. RemovePermission for that statement id and GET again: expect HTTP 502, no invocation of the function, and an access-log record whose error_reason is LambdaAccessDenied.
5. AddPermission again (same statement) and GET once more without re-registering the target: expect HTTP 200.
6. Observe that steps 3-5 never produce a response and the check is killed at its 600 s budget.
