Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.create-portal-access-denied-without-permission apigatewayv2.integration-response-key-format-validation
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.create-portal-access-denied-without-permission|_error_code#6|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2.integration-response-key-format-validation|_error#7|AssertionError|raise AssertionError(
Issue-Commit: 5d61882ef55489b75c810fe19d4c281a0249999c

# apigatewayv2: CreatePortal performs no authorization and CreateIntegrationResponse performs no IntegrationResponseKey validation (2 causes)

> _Note: this issue was identified against the code base at commit `5d61882ef55489b75c810fe19d4c281a0249999c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePortal, CreateIntegrationResponse

## Description
Two distinct root causes in the apigatewayv2 control plane, both grounded in failing behavioral checks of suite simparity-behavioral-ga-prod-v1. Both operations used to answer the off-model code 'NotImplemented'; they now ANSWER, and they answer 201 for requests AWS refuses. A fixer must close BOTH causes before submitting; they are numbered below and each carries its own failing check ids.

Related open issue 20261023T151412Z-apigatewayv2-four-negative-path-control-plane-behaviours-are.md lists these two check ids, but its replay keys are the old `got 'NotImplemented'` assertions; the failures have since moved to different assertions (an unexpected SUCCESS), and the replay report marked neither check ALREADY FILED nor OWNED, so no open issue carries their current replay keys. The per-operation scaffold issues 20261023T125319Z-apigatewayv2-createportal-stub.md and 20261023T114649Z-apigatewayv2-createintegrationresponse-stub.md are already marked fixed and do not cover the semantics below. Neither cause here is a refusal, so neither reopens those fixed issues.

=== Cause 1 of 2: CreatePortal does not authorize the caller, so a principal carrying an explicit Deny is served instead of refused ===
AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json) CreatePortal -> POST /v2/portals, success code 201, modeled errors AccessDeniedException (httpError 403), BadRequestException (400), TooManyRequestsException (429). A principal whose identity-based policy carries an explicit Deny on apigateway:CreatePortal must be refused with error code AccessDeniedException (HTTP 403) and no portal must be created; the same session must still succeed at an action the Deny does not name, and an unrestricted principal must succeed with 201. The AWS message is of the documented authorization form built from the request principal, action and resource ('User: ... is not authorized to perform: apigateway:CreatePortal ... with an explicit deny in an identity-based policy') — that rule is the requirement, the literal text is an example.
Emulator: the restricted principal's CreatePortal SUCCEEDS with HTTP 201 and a fully populated portal (observed PublishStatus 'DISABLED', a PortalArn and a PortalId), identical to the unrestricted path. No IAM evaluation happens for this operation at all.
Why: a client cannot rely on the emulator to model least-privilege at all for portals — IAM-policy regression tests that assert a denied principal is refused pass against the emulator and fail on AWS, and an agent or test harness that probes permissions by attempting the call concludes it is authorized when AWS would deny it.
Fix: the CreatePortal handler's authorization path (the operation's handler in services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_portal.rs) must evaluate the caller's identity-based policy for apigateway:CreatePortal and return AccessDeniedException/403 without mutating portal state.
Traces: apigatewayv2.create-portal-access-denied-without-permission.

=== Cause 2 of 2: CreateIntegrationResponse stores any IntegrationResponseKey, so the key grammar is never enforced ===
AWS: Smithy CreateIntegrationResponse -> POST /v2/apis/{ApiId}/integrations/{IntegrationId}/integrationresponses, success code 201, modeled errors BadRequestException (httpError 400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429); IntegrationResponseKey targets SelectionKey and is required. Recorded behavior: for a non-Lambda (MOCK) integration the key is validated in two independent stages — a value that is neither '$default' nor wrapped '/like/this/' is refused with BadRequestException 400 (example message, naming the member and the offending value: "IntegrationResponseKey must be a regular expression pattern of format '/<pattern>/' or have a value of $default. Current value is badkey"), and a correctly wrapped key whose inner pattern is not status-code shaped is refused by a second, more specific BadRequestException 400 naming 'non-Lambda Integration'. A wrapped, status-code-shaped key such as '/2\\d\\d/' is accepted with 201 and round-trips through GetIntegrationResponse. The requirement is the 400/BadRequestException verdict for each stage plus the acceptance and round-trip of a status-code-shaped key; the exact strings are examples.
Emulator: CreateIntegrationResponse(IntegrationResponseKey='badkey') SUCCEEDS with HTTP 201 and returns the bogus key verbatim (observed {'HTTPStatusCode': 201, 'IntegrationResponseKey': 'badkey', 'ResponseParameters': {}, 'ResponseTemplates': {}}), minting an integration response AWS never creates.
Why: a client building MOCK/WebSocket integration responses gets no feedback that its selection key can never match anything; the misconfiguration surfaces as silently-unmatched responses at invoke time rather than as a 400 at configure time, and negative tests of the key grammar pass against the emulator and fail on AWS.
Fix: IntegrationResponseKey validation in the CreateIntegrationResponse handler (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_integration_response.rs) — the wrapper-format check and, for non-Lambda integrations, the status-code-shape check, each returning BadRequestException/400 and persisting nothing. The same grammar belongs on UpdateIntegrationResponse.
Traces: apigatewayv2.integration-response-key-format-validation.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against this checkout: 18/194 passed.

Cause 1 — apigatewayv2.create-portal-access-denied-without-permission [mismatch]: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'Authorization': {'None': {}}, 'IncludedPortalProductArns': [], 'PortalArn': 'arn:aws:apigateway:<region>:<account>:/portals/<id>', 'PortalContent': {'DisplayName': 'sim2real-portal-denied-...'}, 'PortalId': '<id>', 'PublishStatus': 'DISABLED', 'Tags': {}}.
Expected: error code AccessDeniedException, HTTP 403, and no portal created.
Actual: HTTP 201 with a created portal, for a session assuming a role with an explicit Deny on apigateway:CreatePortal.

Cause 2 — apigatewayv2.integration-response-key-format-validation [mismatch]: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'IntegrationResponseId': '<id>', 'IntegrationResponseKey': 'badkey', 'ResponseParameters': {}, 'ResponseTemplates': {}}.
Expected: error code BadRequestException, HTTP 400, for IntegrationResponseKey='badkey' on a MOCK integration of a WEBSOCKET API; then a second, distinct BadRequestException 400 for '/error.*/'; then 201 for '/2\\d\\d/'.
Actual: HTTP 201 for 'badkey', so the later legs were never reached.

Neither check carried an ALREADY FILED or OWNED-by marker in the replay report.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore apigatewayv2 client at it.
2. Cause 1: create an IAM role whose inline policy carries an explicit Deny on 'apigateway:CreatePortal' (and Allow on everything else), assume it, and call CreatePortal(Authorization={'None': {}}, EndpointConfiguration={'None': {}}, PortalContent={'DisplayName': '<name>', ...}) with those credentials. Expect AccessDeniedException / HTTP 403 and no portal in ListPortals; observe HTTP 201 and a created portal. Verify the unrestricted principal still gets 201.
3. Cause 2: CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); CreateIntegration(ApiId=api, IntegrationType='MOCK'); then CreateIntegrationResponse(ApiId=api, IntegrationId=integ, IntegrationResponseKey='badkey'). Expect BadRequestException / HTTP 400 and GetIntegrationResponses empty; observe HTTP 201 with IntegrationResponseKey 'badkey'. Then repeat with IntegrationResponseKey='/error.*/' (expect a second, distinct 400) and with '/2\\d\\d/' (expect 201 and a byte-identical read-back).
4. Re-replay the two check ids in trace_ids.
