Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.authorizer-protected-method-invoke-flow apigateway.e2e-authorizer-protected-method-consequence
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.authorizer-protected-method-invoke-flow|run#38|AssertionError|assert _json(denied_body) == _DENY_BODY, f"unexpected explicit-deny body {denied_body!r}"
Issue-Key: api-gateway|DeleteAuthorizer|ConflictException
Issue-Commit: eb01f0cbd4059754f5704e86e7da8eaac7554275

# apigateway: 2 net-new causes — the explicit-deny 403 body truncates AWS's policy-reason clause, and DeleteAuthorizer stays refused 409 ConflictException after UpdateMethod sets authorizationType=NONE

> _Note: this issue was identified against the code base at commit `eb01f0cbd4059754f5704e86e7da8eaac7554275`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAuthorizer, execute-api Invoke

## Description
Two distinct net-new root causes in the apigateway service, grounded in failing checks of this suite that are neither ALREADY FILED nor OWNED by an open issue.

=== Cause 1 of 2: the explicit-deny 403 body drops the clause naming WHY authorization failed ===
AWS: when a CUSTOM (TOKEN) Lambda authorizer returns a policy document that explicitly Denies execute-api:Invoke for the invoked method ARN, the deployed stage answers HTTP 403 with x-amzn-errortype: AccessDeniedException and the body {"Message":"User is not authorized to access this resource with an explicit deny in an identity-based policy"}. That whole sentence is FIXED text — it contains no request-, state- or run-derived value — and it is deliberately distinct from the no-matching-Allow sentence ("... because no identity-based policy allows the execute-api:Invoke action"), because the two tell a caller whether a Deny matched or no Allow matched. Smithy: CreateAuthorizer.type -> AuthorizerType; the data-plane refusal is not modelled, so this sentence is recorded behavior; the recording shows status 403 with the full sentence.
Emulator: the status (403) is correct, but the body is truncated to the prefix {"Message":"User is not authorized to access this resource with an explicit deny"} — the trailing " in an identity-based policy" is missing.
Why: a client or operator that matches on the message to distinguish an explicit Deny from a missing Allow (or any tooling asserting AWS's documented sentence) mis-diagnoses the authorizer policy, and the two different denial causes become indistinguishable.
Fix: the execute-api data-plane serializer that renders the AccessDeniedException body for a Lambda-authorizer policy evaluation; it must emit AWS's complete sentence for the explicit-deny outcome. NOTE: an open issue, 20261107T191316Z-apigateway-lambda-3-net-new-causes-non-proxy-d346f3.md, describes the same truncation under the apigateway__lambda heading (its Cause 2); this check is listed under the apigateway heading and is not marked OWNED by it, so it is filed here and that issue is named for cross-reference — fixing one serializer should close both.
Traces: apigateway.authorizer-protected-method-invoke-flow.

=== Cause 2 of 2: an authorizer detached by UpdateMethod is still treated as referenced, so DeleteAuthorizer is refused 409 ===
AWS: Smithy: UpdateMethod (PATCH /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}) returns Method with members authorizationType and authorizerId; patching /authorizationType to NONE detaches the authorizer from the method, after which DeleteAuthorizer (DELETE /restapis/{restApiId}/authorizers/{authorizerId}, success code 202) succeeds. ConflictException (409) is modelled on DeleteAuthorizer and is the refusal AWS uses only while the authorizer is STILL referenced by a method; once no method references it, the delete must be accepted.
Emulator: after a successful UpdateMethod replacing /authorizationType with NONE, DeleteAuthorizer is still refused with 409 ConflictException and the message "Cannot delete authorizer '<authorizerName>', is referenced in method: <httpMethod>//<resourcePath>" (fixed text "Cannot delete authorizer '", "', is referenced in method: "; the authorizer name, HTTP method and resource path are values from the request and the emulator's state). The delete-gating check evidently keys off the method's stored authorizerId (or an index entry) that UpdateMethod never clears when authorizationType becomes NONE.
Why: an authorizer can never be deleted through the documented detach-then-delete sequence: the only escape is deleting the whole REST API. Every teardown/replace workflow (including CloudFormation-style authorizer replacement) wedges on a permanent 409.
Fix: UpdateMethod's patch handling for /authorizationType must clear the method's authorizerId when the type becomes NONE (and the DeleteAuthorizer reference guard must consult the live method state rather than a stale reference index).
Traces: apigateway.e2e-authorizer-protected-method-consequence.

## Evidence
Cause 1 — expected body: {"Message": "User is not authorized to access this resource with an explicit deny in an identity-based policy"} with status 403. Actual: status 403 (correct) with body b'{"Message":"User is not authorized to access this resource with an explicit deny"}'. Failure: AssertionError: unexpected explicit-deny body b'{"Message":"User is not authorized to access this resource with an explicit deny"}'. Every word of the expected sentence is fixed text; none of it is request- or state-derived.

Cause 2 — the check calls UpdateMethod(patchOperations=[{"op":"replace","path":"/authorizationType","value":"NONE"}]) (which succeeds) and then DeleteAuthorizer, which real AWS accepts. Actual: botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the DeleteAuthorizer operation: Cannot delete authorizer 'sim2real-e2e-authorizer-a68554c8b99b', is referenced in method: GET//secure. The authorizer name and the "GET//secure" method identifier are values from the request/state; the surrounding sentence is the emulator's fixed text. Expected: a 202 empty response, followed by NotFoundException/"Invalid Authorizer identifier specified" on a subsequent GetAuthorizer/UpdateAuthorizer for that id.

## Steps to Reproduce
Cause 1:
1. CreateRestApi; CreateResource(pathPart="secure"); CreateAuthorizer(type="TOKEN", identitySource="method.request.header.Authorization") backed by a Lambda that returns an IAM policy with Effect=Deny on the invoked methodArn for an invalid token.
2. PutMethod(GET, authorizationType="CUSTOM", authorizerId=<id>); PutIntegration(type="MOCK"); PutMethodResponse/PutIntegrationResponse 200; CreateDeployment(stageName="test").
3. GET the deployed stage URL /secure with header Authorization: deny-me -> 403 with body {"Message":"User is not authorized to access this resource with an explicit deny"} instead of AWS's "... with an explicit deny in an identity-based policy".

Cause 2:
1. Same REST API with a CUSTOM TOKEN authorizer attached to GET /secure.
2. UpdateMethod(restApiId, resourceId, httpMethod="GET", patchOperations=[{"op":"replace","path":"/authorizationType","value":"NONE"}]) -> succeeds.
3. DeleteAuthorizer(restApiId, authorizerId) -> 409 ConflictException "Cannot delete authorizer '<name>', is referenced in method: GET//secure" instead of 202 success.
