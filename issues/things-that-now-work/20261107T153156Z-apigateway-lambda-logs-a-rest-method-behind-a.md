Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation
Issue-Key: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation|_stays_absent#22|Timeout|time.sleep(interval_s)
Issue-Commit: afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b

# apigateway__lambda__logs: a REST method behind a TOKEN Lambda authorizer never answers 200/401/403 on its deployed stage, so an explicit Deny is never observable (1 cause)

> _Note: this issue was identified against the code base at commit `afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
One net-new behavioral root cause grounded in a failing check of the simparity-behavioral-aws-v1 replay (apigateway__lambda__logs 0/5; the other four failures are OWNED by open issues and are not part of this one). Related open issue, named for the fixer: 20261029T182816Z-apigateway-lambda-logs-proxy-integrations-answer-500.md records REST proxy integrations answering 500; this check is not marked OWNED by it and exercises the authorizer gate in front of the integration.

Cause 1 of 1 — the stage's invoke URL never produces an answer the authorizer decision can be read from.
AWS: Smithy com.amazonaws.apigateway#CreateAuthorizer (type TOKEN, identitySource method.request.header.Authorization, authorizerUri naming the authorizer Lambda) plus a method whose authorizationType is CUSTOM and whose authorizerId names it, deployed to a stage. A request to https://{restApiId}.execute-api.{region}.amazonaws.com/{stage}/{resource} carrying a token whose authorizer returns a policyDocument with Effect "Deny" for the method ARN is answered 403 by the gateway, with the explicit-deny message body (fixed text "User is not authorized to access this resource with an explicit deny"), the backend integration is NOT invoked, and the stage's access log records that 403 with $context.requestId resolved and $context.path equal to the stage-qualified "/{stage}/{resource}". The same request with an Allow token reaches the integration and gets its 200. A malformed/absent token yields 401; so AWS always answers one of 200/401/403 for this wiring.
Emulator: nothing acceptable comes back. The check exhausts the whole 60 s budget on its FIRST probe of the stage invoke URL (it polls until the status is one of 200, 401 or 403), so it never reaches the deny assertions, the access-log assertions or the Allow-token control. The authorizer's decision, the 403 status, the explicit-deny message and the "backend never invoked" guarantee are all unobservable.
Why: a client cannot use a Lambda authorizer at all. Nothing distinguishes "the authorizer denied" from "the gateway is broken", and the over-permissiveness this scenario exists to catch (the backend running for a denied request) cannot be ruled out either, because neither leg of the differential ever answers.
Fix: the execute-api REST stage request path in the apigateway crate — the authorizer invocation and its policy-document evaluation must run and produce a 403 (or 401) or hand the request to the integration, instead of leaving the stage request unanswered.
Traces: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[mismatch] apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation
Error: Check timed out after 60.0s.

The check creates a TOKEN authorizer (Allow for the token "allow-me", Deny otherwise), a backend Lambda that prints "BACKEND-INVOKED <marker>", a CUSTOM-authorized method, a deployment and a stage with accessLogSettings, and then issues:
  deny_status, _, deny_text = _poll_http(url, accept=lambda s: s in (200, 401, 403), headers={"Authorization": "deny-me"}, ...)
It never gets a status in {200, 401, 403} within the budget, so the next assertions never run:
  assert deny_status == 403, "expected 403 for a request the Lambda authorizer explicitly Denied ..."
  assert "not authorized" in deny_text.lower(), "expected API Gateway's explicit-deny message ..."
Expected (AWS): 403 with the explicit-deny message, no BACKEND-INVOKED line in the backend's log group, a 403 access-log record whose $context.path is "/test/secure" and whose $context.requestId is non-empty, and 200 with the backend marker for the "allow-me" token.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a REST API with a /secure resource and a GET method, a Lambda authorizer function returning Allow only for the token "allow-me", and a backend Lambda proxy integration.
3. aws apigateway create-authorizer --type TOKEN --identity-source method.request.header.Authorization --authorizer-uri <authorizer invoke arn>, then set the method's authorizationType to CUSTOM with that authorizerId.
4. aws apigateway create-deployment --stage-name test and enable accessLogSettings on the stage.
5. curl -H 'Authorization: deny-me' https://<restApiId>.execute-api.<region>.amazonaws.com/test/secure
   Observed: no 200/401/403 answer within 60 s. AWS: 403 with the explicit-deny message and no backend invocation.
6. Re-replay the check id in trace_ids.
