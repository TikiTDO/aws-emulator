Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.authorizer-cache-ttl-window apigateway.authorizer-protected-method-invoke-flow apigateway.e2e-authorizer-protected-method-consequence apigateway.test-invoke-authorizer-executes-configured-authorizer apigateway.test-invoke-authorizer-request-type-header-allow-and-deny apigateway.test-invoke-authorizer-returns-policy-and-claims
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.authorizer-cache-ttl-window|run#20|AssertionError|assert deny_resp["clientStatus"] == 0, (
Issue-Key: apigateway.authorizer-protected-method-invoke-flow|run#27|AssertionError|assert denied.get("clientStatus") == 0, f"expected the authorizer function itself to execute cleanly, got clientStatus={denied.get('clientStatus')}"
Issue-Key: apigateway.e2e-authorizer-protected-method-consequence|run#42|AssertionError|assert deny_probe["clientStatus"] == 0, deny_probe.get("clientStatus")
Issue-Key: apigateway.test-invoke-authorizer-executes-configured-authorizer|run#15|AssertionError|assert deny_resp.get("clientStatus") == 0, (
Issue-Key: apigateway.test-invoke-authorizer-request-type-header-allow-and-deny|run#12|AssertionError|assert deny_resp["clientStatus"] == 0, (
Issue-Key: apigateway.test-invoke-authorizer-returns-policy-and-claims|run#30|AssertionError|assert policy["Statement"][0]["Resource"] == expected_resource, (
Issue-Commit: 768db93f7ee9a823eb9c7336f7c11f317ed4b425

# apigateway TestInvokeAuthorizer: a Deny decision is reported as clientStatus 403 instead of 0, the simulated method ARN names 'test-invoke-stage' instead of 'ESTestInvoke-stage', and claims is served for a Lambda authorizer (3 causes)

> _Note: this issue was identified against the code base at commit `768db93f7ee9a823eb9c7336f7c11f317ed4b425`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeAuthorizer

## Description
Three net-new behavioral root causes in apigateway TestInvokeAuthorizer, grounded in 6 failing checks of the simparity-behavioral-aws-v1 replay (apigateway 440/580). Every other apigateway failure in this report is marked OWNED by an open issue or ALREADY FILED and is NOT part of this issue. Related open issues that do NOT carry these replay keys, named for the fixer: 20261029T200808Z-apigateway-api-gateway-authorizer-representation-and-testinvokemethod-execution-9.md (authType serialisation and TestInvokeMethod execution — a different operation and different members) and 20261023T170254Z-apigateway-testinvokeauthorizer-stub.md (that stub is now implemented; these are fidelity defects in the implemented handler, not a 501).

Cause 1 of 3: TestInvokeAuthorizer reports a non-zero clientStatus for an authorizer that EXECUTED SUCCESSFULLY and returned a Deny policy.
AWS: Smithy com.amazonaws.apigateway#TestInvokeAuthorizerResponse, member clientStatus (Integer, default 0) — "The HTTP status code that the client would have received. Value is 0 if the authorizer succeeded." The authorizer "succeeded" means its function ran and returned a well-formed authorizer output; the Allow/Deny content of that policyDocument is the authorizer's RESULT, not a failure of it. So for a TOKEN or REQUEST Lambda authorizer that is invoked and returns {principalId, policyDocument:{Statement:[{Action:execute-api:Invoke, Effect:Deny, Resource:<methodArn>}]}}, AWS answers HTTP 200 with clientStatus 0, principalId and policy carrying exactly what the function returned. A non-zero clientStatus is reserved for an authorizer that could not produce a decision (configuration error, failure, timeout) or that refused outright with the documented Unauthorized error.
Emulator: HTTP 200 with clientStatus 403 for the Deny leg, with the log line "Authorizer <name> returned a policy that does not allow execute-api:Invoke on <methodArn>: User is not authorized to access this resource with an explicit deny" followed by "Answered through this API's ACCESS_DENIED gateway response: the client would have received HTTP 403". The policy and principalId are correct; only clientStatus diverges. The handler deliberately renders clientStatus from the gateway response a refused request would have been answered through (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_authorizer.rs lines 401-420), and the Deny policy is turned into a GatewayRefusal (ACCESS_DENIED) in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke/simulated_authorizer.rs lines 204-212.
Why: a client cannot distinguish "the authorizer ran and denied the call" from "the authorizer could not run". A developer testing an authorizer's deny branch — which is precisely what the console/CLI test-invoke is for — sees a non-zero clientStatus and concludes the authorizer is broken; conversely a CI check that asserts clientStatus==0 to mean "the function executed" fails on a perfectly healthy authorizer. The distinction between an executed Deny (clientStatus 0) and an authorizer configuration error (clientStatus 500) is lost.
Fix: the TestInvokeAuthorizer handler's clientStatus derivation — report 0 whenever the authorizer produced a decision (i.e. whenever a principalId/policy was obtained from the function, or a Cognito token was evaluated), and render the gateway-response status only for refusals where no authorizer decision exists (AUTHORIZER_CONFIGURATION_ERROR / AUTHORIZER_FAILURE / the Unauthorized refusal / an identity source the request did not carry). The log line describing the gateway response the client would have been answered through is correct and should stay.
Traces: apigateway.authorizer-cache-ttl-window, apigateway.authorizer-protected-method-invoke-flow, apigateway.e2e-authorizer-protected-method-consequence, apigateway.test-invoke-authorizer-executes-configured-authorizer, apigateway.test-invoke-authorizer-request-type-header-allow-and-deny.

Cause 2 of 3: the method ARN the simulation synthesizes names the wrong reserved stage.
AWS: TestInvokeAuthorizer models no httpMethod and no resourceId, so API Gateway synthesizes the method ARN it hands the authorizer (as event.methodArn) and judges the returned policy against. That ARN names AWS's reserved test-invoke stage, spelled ESTestInvoke-stage: arn:aws:execute-api:<region>:<account-id>:<rest-api-id>/ESTestInvoke-stage/<verb>/<path>. Fixed text: the literal 'ESTestInvoke-stage'; the variable parts are the region, the caller's account id, the restApiId from the request and the verb/path of the simulated request.
Emulator: it synthesizes '.../test-invoke-stage/GET/' — the constant TEST_INVOKE_STAGE = "test-invoke-stage" (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_method.rs line 166), used for the ARN at ops/test_invoke_authorizer.rs line 279 and for the authorizer Pending at service/invoke/simulated_authorizer.rs line 180.
Why: the ARN is what the authorizer function receives and what it echoes into its policy Resource, so a function whose policy is scoped to the real ESTestInvoke-stage ARN (or a lambda:InvokeFunction grant whose aws:SourceArn names it) behaves differently here than on AWS: a policy that AWS would admit is refused, and a developer's assertion on the ARN handed to the authorizer fails. The same spelling is the SourceArn a grant for a console/CLI test invocation must cover, so the permission surface diverges too.
Fix: the one spelling of the reserved test-invoke stage in the apigateway crate (TEST_INVOKE_STAGE), so both TestInvokeMethod and TestInvokeAuthorizer name it as AWS does.
Traces: apigateway.test-invoke-authorizer-returns-policy-and-claims.

Cause 3 of 3: claims is serialized (as an empty map) for a non-Cognito authorizer.
AWS: Smithy com.amazonaws.apigateway#TestInvokeAuthorizerResponse, member claims — "The open identity claims, with any supported custom attributes, returned from the Cognito Your User Pool configured for the API". The member is Cognito-only and is absent from the response for a TOKEN or REQUEST Lambda authorizer; the function's own context map reaches the caller only through the execution log.
Emulator: the handler always emits the member (ops/test_invoke_authorizer.rs lines 441-446 wrap the collected claims in Some(...) unconditionally), so a Lambda-backed authorizer's response carries 'claims': {} — visible in the replayed response dumps of these checks.
Why: a client that tests for the presence of claims to decide whether it is looking at a Cognito authorizer's verdict always sees the member, so the Cognito-only contract is unobservable and an empty map is indistinguishable from "a Cognito authorizer that produced no claims".
Fix: the TestInvokeAuthorizer output serialisation — emit claims only when a COGNITO_USER_POOLS authorizer evaluated an identity token, and leave the member absent otherwise.
Traces: apigateway.test-invoke-authorizer-returns-policy-and-claims (the check asserts the ARN first, so a fixer must close cause 2 before this assertion is reached).

Not filed from these checks: the per-run timestamps and the measured latency inside the log text, and the generated api/authorizer/function ids — those are per-run values, not requirements.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

Cause 1 — five checks, same shape:
[mismatch] apigateway.test-invoke-authorizer-executes-configured-authorizer
  AssertionError: expected clientStatus 0 (the Lambda authorizer itself executed successfully, regardless of its Allow/Deny decision), got 403
[mismatch] apigateway.authorizer-cache-ttl-window
  AssertionError: expected clientStatus 0 (the Lambda executed successfully and returned Deny, not an invocation failure), got 403
[mismatch] apigateway.authorizer-protected-method-invoke-flow
  AssertionError: expected the authorizer function itself to execute cleanly, got clientStatus=403
[mismatch] apigateway.e2e-authorizer-protected-method-consequence
  AssertionError: 403  (assert deny_probe["clientStatus"] == 0)
[mismatch] apigateway.test-invoke-authorizer-request-type-header-allow-and-deny
  AssertionError: the authorizer invocation with a disallowed header value must still run (not be rejected client-side), got {... 'clientStatus': 403, 'principalId': 'sim2real-request-user', 'policy': '{"Version":"2012-10-17","Statement":[{"Action":"execute-api:Invoke","Effect":"Deny","Resource":"arn:aws:execute-api:<region>:<account>:<apiId>/test-invoke-stage/GET/"}]}', 'authorization': {}, 'claims': {}}
  log tail: "... returned a policy that does not allow execute-api:Invoke on <methodArn>: User is not authorized to access this resource with an explicit deny" / "Answered through this API's ACCESS_DENIED gateway response: the client would have received HTTP 403"
Expected (AWS): HTTP 200, clientStatus 0, with principalId and policy carrying the function's Deny document.
Actual: HTTP 200, clientStatus 403 (the same policy/principalId).

Cause 2:
[mismatch] apigateway.test-invoke-authorizer-returns-policy-and-claims
  AssertionError: expected the synthesized test-invoke method ARN 'arn:aws:execute-api:<region>:<account>:<apiId>/ESTestInvoke-stage/GET/', got 'arn:aws:execute-api:<region>:<account>:<apiId>/test-invoke-stage/GET/'
The Resource is the ARN the emulator handed the function as event['methodArn'] and the function echoed back, so the divergence is in the synthesized ARN, not in the function. Only the stage token differs; region/account/apiId are per-run values taken from the request and state.

Cause 3:
Same check asserts `"claims" not in resp`. Every replayed TestInvokeAuthorizer response in this suite carries 'claims': {} for TOKEN/REQUEST Lambda authorizers (see the response dumps quoted above), where AWS omits the member.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateRestApi; CreateFunction (python3.x) whose handler returns {"principalId":"u","policyDocument":{"Version":"2012-10-17","Statement":[{"Action":"execute-api:Invoke","Effect":"Allow" if event.get("authorizationToken")=="allow-me" else "Deny","Resource":event.get("methodArn","*")}]}}; CreateAuthorizer type=TOKEN (and once type=REQUEST) with authorizerUri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations and identitySource=method.request.header.Authorization; lambda AddPermission for apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<apiId>/authorizers/<authorizerId>.
3. apigateway test-invoke-authorizer --rest-api-id <apiId> --authorizer-id <authorizerId> --headers Authorization=deny-me
   Observed: clientStatus 403. AWS: clientStatus 0 (cause 1).
4. Same call with Authorization=allow-me and inspect policy.Statement[0].Resource.
   Observed: .../test-invoke-stage/GET/. AWS: .../ESTestInvoke-stage/GET/ (cause 2).
5. In the same response, observe the member `claims` present as {}; AWS omits it for a non-Cognito authorizer (cause 3).
6. Re-replay the six check ids listed in trace_ids.
