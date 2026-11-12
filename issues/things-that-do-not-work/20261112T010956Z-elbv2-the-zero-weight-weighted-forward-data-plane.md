Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.zero-weight-target-group-is-never-selected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.zero-weight-target-group-is-never-selected|_http#28|Timeout|except urllib.error.HTTPError as exc:
Issue-Commit: e817c6ed64be07871cff8a8e76b425c5a00c0c98

# elbv2: the zero-weight weighted-forward data-plane check reaches no verdict — an ALB with two Lambda target groups never answers at its DNSName within the check budget

> _Note: this issue was identified against the code base at commit `e817c6ed64be07871cff8a8e76b425c5a00c0c98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyListener

## Description
Cause 1 of 1: an ALB whose HTTP:80 listener has a weighted forward action over two lambda-type target groups never serves a response at the load balancer's published DNSName, so the whole weighted-routing scenario (weights 1/0, then 0/1, then the all-zero-weight refusal of ModifyListener) never becomes observable and the check burns its entire 60 s budget without reaching any assertion.

AWS: Smithy (elasticloadbalancingv2, elasticloadbalancing-v10): Action.ForwardConfig -> ForwardActionConfig.TargetGroups is a list of TargetGroupTuple{TargetGroupArn, Weight}; TargetGroupTuple.Weight is documented as "The weight. The range is 0 to 999." Real AWS distributes a forward action's requests across the listed target groups in proportion to their weights, and a target group whose Weight is 0 is never selected while another group in the same forward action has a non-zero weight — so with weights 1/0 every request is answered by group A's target and with 0/1 every request is answered by group B's target. Real AWS also refuses a ModifyListener whose forward action gives every target group Weight 0: the call is rejected with HTTP 400 and error code ValidationError, and the listener keeps its previous (working) configuration. The error message for that class is AWS's fixed ValidationError text naming the offending member/condition; its variable parts are taken from the request (the listener ARN and the submitted weights), never from one recorded run. For the data-plane half of the behaviour the model says nothing (it is wire behaviour of the ALB itself): recorded behaviour only — a GET to http://<DNSName>/ returns HTTP 200 with the body produced by the Lambda registered in the non-zero-weight target group.

Emulator: the check never gets an answer. The replay verdict is the bare harness message `Check timed out after 60.0s`, recorded before the first weighted tally assertion (`_wait_for_body(ctx, url, "TG-A")` after the 1/0 listener is created) could succeed. The control steps that precede it are the ones real AWS also satisfies instantly (both Lambda functions return their marker body on a direct Invoke, the load balancer goes ACTIVE, both lambda targets register), so the step that does not complete is the HTTP request through the load balancer: the ALB data path produces neither a 200 with the Lambda body nor any other response the harness could assert on, and in consequence the all-zero-weight ModifyListener refusal is never exercised either.

Why: a client that shifts traffic between two target groups by weight — the standard blue/green and canary lever — cannot observe any routing at all: zero-weight exclusion cannot be verified, a weight shift appears to have no effect because nothing is ever served, and the guard that refuses an all-zero forward action is unreachable, so a caller can leave a listener in a configuration AWS would have rejected and get no signal that traffic is going nowhere.

Fix: the ELBv2 data plane for the load balancer's published endpoint — the listener's request-serving path and its forward-action target selection (weighted choice over ForwardActionConfig.TargetGroups, including lambda-type target invocation), plus the ModifyListener validation that refuses a forward action in which every TargetGroupTuple.Weight is 0. Related open issues describe neighbouring symptoms of the same data plane but do not carry this check's replay key: 20261110T220525Z-elbv2-5-net-new-data-plane-causes-dnsname.md (which owns elbv2.forward-weighted-and-sticky-target-groups-data-plane) and 20261110T210703Z-elbv2-lambda-an-alb-with-a-lambda-target.md; neither covers elbv2.zero-weight-target-group-is-never-selected, which is why it is filed here.

Traces: behavioral check elbv2.zero-weight-target-group-is-never-selected (suite heading `### elbv2`).

## Evidence
Expected (real AWS): after creating an internet-facing ALB with an HTTP:80 listener whose default action forwards to two lambda-type target groups with weights 1 and 0, a GET to http://<DNSName>/ returns HTTP 200 with the body of the Lambda in the weight-1 group, and 20 consecutive GETs return only that body; after ModifyListener flips the weights to 0/1 the same 20 GETs return only the other group's body; a ModifyListener whose forward action sets every weight to 0 is refused with HTTP 400 / ValidationError and the previous configuration keeps serving.

Actual (emulator): no response is ever served at the load balancer's DNSName, so the check never reaches its first tally and the suite records only `Check timed out after 60.0s` for elbv2.zero-weight-target-group-is-never-selected. The preceding control steps (direct lambda:Invoke of both functions returning their marker bodies, the load balancer reaching ACTIVE, both lambda targets reading back as registered) are the only part of the scenario that completes, so the divergence is in serving the request through the listener and in the never-exercised all-zero-weight ModifyListener refusal — not in the Lambda artifacts.

## Steps to Reproduce
1. Create a security group allowing TCP 80 in the shared VPC and two Lambda functions that each return {statusCode: 200, body: "TG-A"} / "TG-B" in the ALB Lambda-target response shape; confirm both with a direct lambda:Invoke.
2. elbv2:CreateLoadBalancer (Scheme=internet-facing, two AZ subnets, that security group) and wait for State=active.
3. elbv2:CreateTargetGroup twice with TargetType=lambda; grant elasticloadbalancing.amazonaws.com lambda:InvokeFunction on each function for its target group ARN; elbv2:RegisterTargets each function ARN into its target group.
4. elbv2:CreateListener Port=80 Protocol=HTTP with DefaultActions=[forward -> [{TG-A, Weight:1}, {TG-B, Weight:0}]].
5. GET http://<DNSName>/ 20 times — expect HTTP 200 with body "TG-A" every time. Observed: no response is served; the scenario times out.
6. elbv2:ModifyListener to weights 0/1 and repeat step 5 — expect only "TG-B".
7. elbv2:ModifyListener with both weights 0 — expect HTTP 400 ValidationError and the 0/1 configuration still serving.
