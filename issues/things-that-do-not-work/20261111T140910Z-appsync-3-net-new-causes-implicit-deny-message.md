Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.access-denied-graphql-api appsync.graphql-api-unicode-name-round-trip appsync.put-environment-variables-exceeds-size-limit-rejected appsync.put-environment-variables-size-limit-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.access-denied-graphql-api|run#16|AssertionError|assert message == expected, f"{label}: expected message {expected!r}, got {message!r}"
Issue-Key: appsync.graphql-api-unicode-name-round-trip|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: appsync.put-environment-variables-exceeds-size-limit-rejected|run#10|AssertionError|assert message == (
Issue-Key: appsync.put-environment-variables-size-limit-rejected|run#15|AssertionError|assert rejected_message == (
Issue-Commit: 92964a3563e046b76198eacfa7b4e785275ebde9

# appsync: 3 net-new causes — implicit-deny message names the api ARN for environmentVariables sub-resource ops, CreateApi name-pattern refusal is BadRequestException instead of ValidationException, and the environmentVariables length-violation message re-orders the request map

> _Note: this issue was identified against the code base at commit `92964a3563e046b76198eacfa7b4e785275ebde9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGraphqlApiEnvironmentVariables

## Description
Three distinct net-new root causes on the AppSync control plane, all grounded in failing behavioral checks that the replay report did NOT mark ALREADY FILED or OWNED. 3 causes in this issue; all three must be closed before submitting.

=== Cause 1 of 3: the AccessDeniedException implicit-deny message names the GraphqlApi ARN instead of the operation's own resource for Get/PutGraphqlApiEnvironmentVariables ===
Failing check ids: appsync.access-denied-graphql-api
AWS: Smithy (appsync, com.amazonaws.appsync#AWSDeepdishControlPlaneService) models GetGraphqlApiEnvironmentVariables as GET /v1/apis/{apiId}/environmentVariables and PutGraphqlApiEnvironmentVariables as PUT /v1/apis/{apiId}/environmentVariables, both with AccessDeniedException (httpError 403) in their error set. For a principal with no identity-based policy granting the action, AWS answers 403 AccessDeniedException whose message is the standard implicit-deny sentence. Its FIXED text is: "User: " … " is not authorized to perform: appsync:" … " on resource: " … " because no identity-based policy allows the appsync:" … " action". Its VARIABLE parts come from the request and from IAM state: the caller's principal ARN, the action name (the operation name), and the RESOURCE the action is evaluated against. Recorded behavior: for the api-level actions (GetGraphqlApi/UpdateGraphqlApi/DeleteGraphqlApi) the resource is the GraphqlApi ARN (arn:aws:appsync:<region>:<account>:apis/<apiId>) — those three sub-checks PASS. For the two environmentVariables actions the resource is the operation's REST URI path hung off the service ARN prefix: arn:aws:appsync:<region>:<account>:/v1/apis/<apiId>/environmentVariables (leading slash and the /v1 prefix are part of it; <region>, <account> and <apiId> are the request's/state's values).
Emulator: for GetGraphqlApiEnvironmentVariables (and likewise PutGraphqlApiEnvironmentVariables) it reuses the api-level ARN form, emitting "… on resource: arn:aws:appsync:<region>:<account>:apis/<apiId> because …" — the /v1/apis/<apiId>/environmentVariables resource path is replaced by the parent api ARN. Code (AccessDeniedException) and status (403) are correct; only the resource segment of the message diverges.
Why: the implicit-deny message is the only place AWS tells a client WHICH resource the denial was evaluated against. A client (or a least-privilege tool) that parses the resource out of the message to build the IAM policy statement it is missing derives a policy for the api resource rather than for the environmentVariables sub-resource, so the policy it writes does not lift the denial; any assertion on the denial text fails.
Fix: the AppSync authorization/error layer that renders the implicit-deny AccessDeniedException message — the resource it formats must be derived per-operation from the action's resource (the modeled HTTP URI path for the environment-variable operations), not defaulted to the GraphqlApi ARN. Related open issue 20261025T022306Z-appsync-graphql-api-control-plane-operations-are-not.md covers the MISSING deny gate on GetGraphqlApi/UpdateGraphqlApi/DeleteGraphqlApi; it does not cover the resource rendered in the message, and those three sub-checks now pass.
Traces: appsync.access-denied-graphql-api (sub-case label "GetGraphqlApiEnvironmentVariables").

=== Cause 2 of 3: CreateApi refuses a name violating the @pattern with BadRequestException, where AWS answers ValidationException ===
Failing check ids: appsync.graphql-api-unicode-name-round-trip
AWS: Smithy (appsync) CreateApi = POST /v2/apis, input member name @required, target ApiName with @length min 1 max 50 and @pattern ^[A-Za-z0-9_\-\ ]+$. ValidationException is NOT in CreateApi's modeled error set (modeled: BadRequestException 400, ConcurrentModificationException 409, ServiceQuotaExceededException 402, UnauthorizedException 401, InternalFailureException 500), so for the error CODE this is recorded behavior only: the recording shows AWS rejecting a name carrying non-ASCII characters with error code ValidationException (the framework-level constraint error) and the message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_\-\ ]+". The fixed text is "1 validation error detected: Value at '" … "' failed to satisfy constraint: Member must satisfy regular expression pattern: " … ; the variable parts are the offending member name ('name') and the pattern from the model.
Emulator: the same call is refused with error code BadRequestException. The validation DECISION is right (the unicode name is refused and no Api is created); only the code is wrong, so the check stops at the code assertion and the message was not compared — the message must be verified against the recorded sentence above once the code is corrected.
Why: botocore/SDK clients model AppSync's exceptions from the service error set plus the common ValidationException; code written against AWS catches ValidationException (or branches on Error.Code == "ValidationException") for malformed input to the v2 Api operations and takes the wrong branch — or lets the error escape as a generic ClientError — when the emulator says BadRequestException.
Context (not a reopen): the fixed issue 20261024T032443Z-appsync-control-plane-fidelity-domain-name-operations-unimplemented.md, Cause 2, required constraint violations on PutGraphqlApiEnvironmentVariables and TagResource to carry the modeled BadRequestException instead of ValidationException. That remap now applies to CreateApi too, where the recording shows AWS using ValidationException; the mapping has to be per-operation rather than global.
Fix: the AppSync request-validation/error-mapping layer and the CreateApi handler's input validation — the @pattern/@length refusal for CreateApi's name must be classified as ValidationException (400) with AWS's constraint sentence, while the operations whose recordings show BadRequestException keep it.
Traces: appsync.graphql-api-unicode-name-round-trip.

=== Cause 3 of 3: the environmentVariables @length refusal renders the offending map in sorted key order instead of the order the request carried ===
Failing check ids: appsync.put-environment-variables-exceeds-size-limit-rejected, appsync.put-environment-variables-size-limit-rejected
AWS: Smithy (appsync) PutGraphqlApiEnvironmentVariables = PUT /v1/apis/{apiId}/environmentVariables, input environmentVariables @required -> EnvironmentVariableMap with @length min 0 max 50; error set includes BadRequestException (400). A map over the 50-pair limit is refused 400 BadRequestException with the constraint sentence "1 validation error detected: Value '{<rendered map>}' at 'environmentVariables' failed to satisfy constraint: Member must have length less than or equal to 50". Fixed text: "1 validation error detected: Value '{" … "}' at 'environmentVariables' failed to satisfy constraint: Member must have length less than or equal to 50". Variable part: <rendered map>, which is the REQUEST's map rendered as "key=value" pairs joined by ", " IN THE ORDER THE REQUEST CARRIED THEM (recorded behavior: the pairs appear in the submitted order, e.g. VAR0, VAR1, VAR2, … VAR59 for a request built in that order — the keys/values themselves are request values, not part of the template).
Emulator: status 400 and code BadRequestException are correct, but the rendered map is re-ordered into lexicographic key order before being interpolated — "{VAR0=x, VAR1=x, VAR10=x, VAR11=x, …, VAR19=x, VAR2=x, VAR20=x, …}" instead of the request's "{VAR0=x, VAR1=x, VAR2=x, …}" — so the message text diverges for every request whose key order is not already lexicographic. Both affected checks fail on exactly this message comparison, with 60-entry and 51-entry maps respectively.
Why: the message is a requirement of this error class; a client that logs, diffs or asserts on the refusal sentence (and any test or snapshot written against AWS) sees text AWS never produced, and a client that parses the rendered map back out to find which pairs it submitted recovers them in an order the service invented.
Context (not a reopen): the fixed issue 20261024T032443Z-appsync-control-plane-fidelity-domain-name-operations-unimplemented.md, Cause 2, corrected the error CODE for this call (ValidationException -> BadRequestException) and that part now holds — the surviving divergence is purely the ordering of the request map inside the message, which that issue did not describe.
Fix: the validation-message serializer for map-typed @length violations in the AppSync service crate (the component that renders the offending value for PutGraphqlApiEnvironmentVariables) must preserve the request's member order when formatting the map rather than iterating a sorted/ordered collection.
Traces: appsync.put-environment-variables-exceeds-size-limit-rejected, appsync.put-environment-variables-size-limit-rejected.

## Evidence
Cause 1 — appsync.access-denied-graphql-api, sub-case GetGraphqlApiEnvironmentVariables.
Expected (AWS): HTTP 403, code AccessDeniedException, message
  'User: arn:aws:iam::<account>:user/<denied-user> is not authorized to perform: appsync:GetGraphqlApiEnvironmentVariables on resource: arn:aws:appsync:<region>:<account>:/v1/apis/<apiId>/environmentVariables because no identity-based policy allows the appsync:GetGraphqlApiEnvironmentVariables action'
Actual (emulator): same status and code, but the resource segment is the api ARN:
  '… on resource: arn:aws:appsync:<region>:<account>:apis/<apiId> because no identity-based policy allows the appsync:GetGraphqlApiEnvironmentVariables action'
Replay assertion: check_appsync_access_denied_graphql_api.py:103 `assert message == expected` -> 'GetGraphqlApiEnvironmentVariables: expected message … got …'. The GetGraphqlApi/UpdateGraphqlApi/DeleteGraphqlApi sub-cases, which use the GraphqlApi ARN, pass — so the divergence is scoped to the environmentVariables operations' resource.

Cause 2 — appsync.graphql-api-unicode-name-round-trip.
Call: CreateApi(name=<ascii prefix> + '-café-测试-😀', eventConfig={API_KEY auth providers/modes}).
Expected (AWS, recorded): code ValidationException, message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_\-\ ]+".
Actual (emulator): code BadRequestException.
Replay assertion: check_appsync_graphql_api_unicode_name_round_trip.py:34 `assert code == "ValidationException"` -> 'AssertionError: expected ValidationException, got BadRequestException'. The refusal itself is correct (no Api is created), so only the code (and the un-compared message) is at issue.

Cause 3 — appsync.put-environment-variables-exceeds-size-limit-rejected and appsync.put-environment-variables-size-limit-rejected.
Call: PutGraphqlApiEnvironmentVariables(apiId=<api>, environmentVariables={VAR0.., 60 pairs / 51 pairs}).
Expected (AWS): 400 BadRequestException, message "1 validation error detected: Value '{VAR0=x, VAR1=x, VAR2=x, … VAR59=x}' at 'environmentVariables' failed to satisfy constraint: Member must have length less than or equal to 50" — the pairs in the order the request carried them.
Actual (emulator): 400 BadRequestException, message "1 validation error detected: Value '{VAR0=x, VAR1=x, VAR10=x, VAR11=x, VAR12=x, … VAR19=x, VAR2=x, VAR20=x, …, VAR9=x}' at 'environmentVariables' failed to satisfy constraint: Member must have length less than or equal to 50" — the pairs sorted lexicographically by key.
Replay assertions: check_appsync_put_environment_variables_exceeds_size_limit_rejected.py:39 and check_appsync_put_environment_variables_size_limit_rejected.py:43, both `assert <message> == (…)` -> 'unexpected BadRequestException message …'. (The specific key names/values above are the checks' request values filling the template, not part of the requirement.)

## Steps to Reproduce
Cause 1:
1. CreateGraphqlApi(name=<unique>, authenticationType='API_KEY') with a permissioned caller; note apiId.
2. As the permissioned caller, PutGraphqlApiEnvironmentVariables(apiId, {'PROBE':'1'}) and read it back (positive control).
3. Create an IAM user with ZERO attached policies, mint an access key, and build an appsync client with those credentials.
4. With that client call GetGraphqlApiEnvironmentVariables(apiId=<apiId>) and PutGraphqlApiEnvironmentVariables(apiId=<apiId>, environmentVariables={'FOO':'bar'}).
5. Inspect Error.Message: the resource is arn:aws:appsync:<region>:<account>:apis/<apiId> instead of arn:aws:appsync:<region>:<account>:/v1/apis/<apiId>/environmentVariables. Compare with GetGraphqlApi under the same credentials, whose resource (the api ARN) is correct.

Cause 2:
1. Call CreateApi(name='<ascii-prefix>-café-测试-😀', eventConfig={authProviders:[{authType:'API_KEY'}], connectionAuthModes:[{authType:'API_KEY'}], defaultPublishAuthModes:[{authType:'API_KEY'}], defaultSubscribeAuthModes:[{authType:'API_KEY'}]}).
2. Observe Error.Code == 'BadRequestException'; AWS answers 'ValidationException' with "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_\-\ ]+".
3. ListApis confirms no Api was created (that part is correct), and the same call with a compliant ASCII name succeeds.

Cause 3:
1. CreateGraphqlApi(name=<unique>, authenticationType='API_KEY'); note apiId.
2. With client-side parameter validation disabled, call PutGraphqlApiEnvironmentVariables(apiId, {f'VAR{i}': 'x' for i in range(60)}) — a map built in VAR0, VAR1, VAR2 … order.
3. Read Error.Message: the rendered map inside "Value '{…}'" comes back in lexicographic key order (VAR0, VAR1, VAR10, VAR11 …) instead of the order the request carried (VAR0, VAR1, VAR2 …).
4. Repeat with 51 pairs (plain client) for the second check; GetGraphqlApiEnvironmentVariables confirms nothing was persisted/overwritten, which is correct.
