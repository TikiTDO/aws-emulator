Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.flush-stage-authorizers-cache-bypasses-ttl
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.flush-stage-authorizers-cache-bypasses-ttl|_poll_for_fresh_invocation#24|Timeout|last_result = _get_json_with_retries(url, headers, timeout_s=10.0)
Issue-Commit: c4fcf6952896a9081c9de16adffed799f1736bf9

# apigateway: FlushStageAuthorizersCache's TTL bypass is never observable — the check consumes its whole budget without a verdict

> _Note: this issue was identified against the code base at commit `c4fcf6952896a9081c9de16adffed799f1736bf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FlushStageAuthorizersCache

## Description
One net-new root cause in the apigateway emulator, grounded in a failing behavioral check that no open issue carries the replay key for.

Cause 1 of 1: a REST stage whose method is guarded by a TOKEN (CUSTOM) Lambda authorizer never completes the FlushStageAuthorizersCache round trip, so the flush-bypasses-TTL contract is unobservable.
Failing check: apigateway.flush-stage-authorizers-cache-bypasses-ttl

AWS: FlushStageAuthorizersCache is modelled (Smithy: BackplaneControlService#FlushStageAuthorizersCache, DELETE /restapis/{restApiId}/stages/{stageName}/cache/authorizers, success http code 202, input restApiId and stageName both @required @httpLabel, errors BadRequestException 400, ConflictException 409, LimitExceededException 429, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401). Behaviorally (recorded behavior only — the model states the status and the errors, not the cache semantics): while an authorizer's authorizerResultTtlInSeconds window is open, two identical requests to the deployed stage with the same identity-source header share ONE authorizer invocation, so a value the authorizer returns in its `context` and the integration response template renders (here $context.authorizer.count) is identical on both calls; after FlushStageAuthorizersCache is accepted with 202, a subsequent request on that stage re-invokes the authorizer even though the TTL is far from expiring, so that counter advances. The requirement is the pair of states (identical inside the TTL, advanced after the flush) and the 202; how many seconds propagation takes, and the concrete counter values, are not requirements.
Emulator: the check reaches no verdict at all. The only detail the replay produces is `Error: Check timed out after 60.0s` — the 60 s budget is consumed before any of the sequence completes, so neither the cached pair of GETs through the stage URL, nor the 202 from FlushStageAuthorizersCache, nor the post-flush re-invocation is ever observed. Nothing here is a runner defect: no tool was missing, no name resolution or TLS failure is reported, and the assertion at stake is on the emulator's own served responses.
Hypothesis (not stated by the model or by the replay output): the stalled step is the execute-api data plane — the stage URL for a CUSTOM-authorized method over a MOCK integration does not become servable, which is the same symptom already reported for sibling stage checks — rather than the control-plane FlushStageAuthorizersCache handler itself.
Why: a client that must force its authorizer to be re-evaluated (a revoked token, a changed policy) calls FlushStageAuthorizersCache and then relies on the very next request being re-authorized. Against this emulator it gets no usable signal at all: the stage does not answer in the window, so neither the cached behaviour inside the TTL nor the flush's effect can be relied on, and a deployment that depends on invalidating an authorizer decision silently keeps serving the stale one.
Fix: the execute-api data-plane path for a deployed stage whose method has authorizationType=CUSTOM (TOKEN authorizer invocation, its result cache keyed by identity source with authorizerResultTtlInSeconds, and the $context.authorizer.* bindings the integration response template renders), plus the FlushStageAuthorizersCache handler that must invalidate that cache and answer 202.
Related open issues that do NOT carry this check's replay key: 20261107T222601Z-apigateway-2-net-new-causes-a-deployed-stage.md (covers apigateway.flush-caches-bypass-ttl and notes in passing that FlushStageAuthorizersCache is unreachable there) and 20261023T210711Z-apigateway-lambda-four-integration-authorizer-behaviour-checks-hang.md (authorizer checks that hang in apigateway__lambda).
Traces: apigateway.flush-stage-authorizers-cache-bypasses-ttl.

## Evidence
Cause 1 — apigateway.flush-stage-authorizers-cache-bypasses-ttl
  Expected: (a) two consecutive GETs of https://<api-id>.execute-api.<region>.<host>/<stage>/authtest with the same `Authorization` identity-source header, inside an authorizer configured with authorizerResultTtlInSeconds=300, return the SAME rendered $context.authorizer.count (one authorizer invocation, cached); (b) FlushStageAuthorizersCache(restApiId, stageName) answers HTTP 202; (c) a following GET returns a count strictly greater than the cached one (the authorizer was re-invoked despite the 300 s TTL).
  Actual: the replay reports only `Error: Check timed out after 60.0s`. The 60 s budget elapses with none of (a), (b) or (c) observed — the emulator produced no completed response for the stage GET loop (`_get_json_with_retries`, 90 s window) within the check's budget, so the flush contract reaches no verdict.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay apigateway.flush-stage-authorizers-cache-bypasses-ttl.
3. Or by hand: CreateRole (lambda trust) and CreateFunction a python3.12 TOKEN-authorizer function that increments a module-level counter, Allows on `authorizationToken == "let-me-in"`, and returns that counter in its `context`.
4. CreateRestApi; CreateAuthorizer(type=TOKEN, authorizerUri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations, identitySource=method.request.header.Authorization, authorizerResultTtlInSeconds=300); lambda AddPermission for principal apigateway.amazonaws.com scoped to arn:aws:execute-api:<region>:<account>:<api-id>/authorizers/<authorizer-id>.
5. CreateResource 'authtest'; PutMethod GET (authorizationType=CUSTOM, authorizerId=<id>); PutIntegration type=MOCK requestTemplates {'application/json': '{"statusCode": 200}'}; PutMethodResponse 200; PutIntegrationResponse 200 responseTemplates {'application/json': '{"count": "$context.authorizer.count"}'}; CreateDeployment(stageName='test').
6. GET <stage-url>/authtest twice with header Authorization: let-me-in — both must return the same count. Observe the stage does not answer.
7. Call FlushStageAuthorizersCache(restApiId, stageName='test') — must be 202 — then GET again and observe the count must advance.
