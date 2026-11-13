Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-event-api-crud-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-event-api-crud-lifecycle appsync-event-api-full-lifecycle appsync-event-api-update-error-recovery
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261029T200818Z-appsync-response-projections-omit-handlerconfigs-and-emit-members.md
Issue-Related: 20261029T200818Z-appsync-validation-refusals-are-emulator-authored-wrong-message.md
Issue-Commit: 20f3fe588ec6598ea98761c364c1729f8d379b97

# AppSync Event API name @pattern refusal: wrong error class and value-quoting anchored-regex message

> _Note: this issue was identified against the code base at commit `20f3fe588ec6598ea98761c364c1729f8d379b97`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Two distinct net-new root causes on the AppSync Event-API (v2) control plane, both from wire divergences the replay report marked neither ALREADY FILED (no stub_path) nor OWNED (owned_by empty). 2 causes in this issue; BOTH must be closed before submitting.

=== Cause 1 of 2: CreateApi refuses an Api$name that violates the @pattern with the wrong error code AND a message that quotes the offending value and the anchored regex ===
AWS: Smithy (appsync, com.amazonaws.appsync#AWSDeepdishControlPlaneService) models CreateApi = POST /v2/apis; input member `name` is @required with target ApiName, which carries @length min 1 max 50 and @pattern "^[A-Za-z0-9_\\-\\ ]+$". ValidationException is NOT in CreateApi's modeled error set (modeled: BadRequestException 400, ConcurrentModificationException 409, ServiceQuotaExceededException 402, UnauthorizedException 401, InternalFailureException 500), so for the error CODE this is recorded behavior only: the recording shows AWS answering HTTP 400 with header x-amzn-errortype: ValidationException — the framework-level constraint error — for a name that fails the pattern. The recorded message is AWS's standard single-constraint sentence for a @pattern violation. Its FIXED text is "1 validation error detected: Value at '" ... "' failed to satisfy constraint: Member must satisfy regular expression pattern: " ... ; its VARIABLE parts are the offending member name (here the request member `name`) and the pattern AS THE MODEL WRITES IT MINUS the ^ and $ anchors, i.e. "[A-Za-z0-9_\\-\\ ]+". Note two properties of the template that the emulator gets wrong: AWS does NOT interpolate the rejected value after "Value" for this member (it is "Value at 'name'", not "Value '<the name>' at 'name'"), and AWS renders the pattern unanchored.
Emulator: the same call is refused HTTP 400 — the refusal DECISION and status are right, and no Api is created — but with header x-amzn-errortype: BadRequestException, and with the message "1 validation error detected: Value '<submitted name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_\\-\\ ]+$": it splices the submitted name in after "Value" and emits the pattern with its ^ and $ anchors.
Why: SDK/botocore clients model AppSync's exceptions from the service error set plus the common ValidationException, so code written against AWS catches ValidationException (or branches on Error.Code == "ValidationException") for malformed v2 Api input and takes the wrong branch — or lets the failure escape as a generic ClientError — when the emulator says BadRequestException. Clients and snapshot tests that assert or log the refusal sentence also see text AWS never produced, and a client that parses the pattern out of the message to pre-validate names compiles an anchored regex AWS did not report.
Fix: the CreateApi handler's input validation plus the AppSync constraint-violation message serializer and error-class mapping in the appsync service crate — the ApiName @pattern/@length refusal must be classified as ValidationException (400) and the constraint sentence must omit the offending value for this member and render the pattern with the anchors stripped. Per-operation, not global: the operations whose recordings show BadRequestException must keep it.
Related open issues (neither owns this replay key, so neither covers it): 20261111T140910Z-appsync-3-net-new-causes-implicit-deny-message.md Cause 2 describes the CreateApi code remap from a BEHAVIORAL check only (unicode name, code assertion only, message never compared) and says nothing about the value-quoting or the anchors; 20261029T200818Z-appsync-validation-refusals-are-emulator-authored-wrong-message.md covers other emulator-authored refusal texts.
Traces: appsync-event-api-crud-lifecycle#task#5 (also appsync-event-api-full-lifecycle#task#5).

=== Cause 2 of 2: UpdateApi refuses an Api$name that violates the @pattern the same two wrong ways ===
AWS: Smithy (appsync) models UpdateApi = POST /v2/apis/{apiId}; `name` is @required -> ApiName with the identical @length min 1 max 50 and @pattern "^[A-Za-z0-9_\\-\\ ]+$". ValidationException is likewise not in UpdateApi's modeled error set (modeled: AccessDeniedException 403, BadRequestException 400, ConcurrentModificationException 409, NotFoundException 404, UnauthorizedException 401, InternalFailureException 500), so the CODE is recorded behavior only: the recording shows HTTP 400 with x-amzn-errortype: ValidationException and the same constraint sentence template as Cause 1 — fixed text "1 validation error detected: Value at '" ... "' failed to satisfy constraint: Member must satisfy regular expression pattern: " ... , variables being the member name (`name`) and the unanchored pattern "[A-Za-z0-9_\\-\\ ]+", with no interpolation of the submitted value.
Emulator: HTTP 400 with x-amzn-errortype: BadRequestException and the message "1 validation error detected: Value '<submitted name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_\\-\\ ]+$" — again the value spliced in and the pattern anchored.
Why: same client impact as Cause 1, on the update path: an error-recovery flow that retries after catching ValidationException from a rename never catches it, and the existing Api is left in whatever state the caller assumed the failed rename had reached.
Fix: the same validation/serializer/error-mapping component, reached from the UpdateApi handler — this cause is listed separately because UpdateApi's own validation path and modeled error set differ from CreateApi's, so correcting only CreateApi leaves this one diverging.
Traces: appsync-event-api-update-error-recovery#task#4.

## Evidence
Cause 1 — fixture appsync-event-api-crud-lifecycle, step task#5 (operation CreateApi), cluster also covering appsync-event-api-full-lifecycle#task#5.
Status: expected 400, got 400 (correct).
Expected (AWS, recorded):
  header x-amzn-errortype: ValidationException
  $.message: "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_\\-\\ ]+"
Actual (emulator):
  header x-amzn-errortype: BadRequestException
  $.message: "1 validation error detected: Value 'bad@name' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_\\-\\ ]+$"
Replay detail: "$.message: value_mismatch" and "$headers.x-amzn-errortype: value_mismatch (expected 'ValidationException', got 'BadRequestException')".
Template vs values: the FIXED text is "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern-without-anchors>". 'name' is the request's member name and "[A-Za-z0-9_\\-\\ ]+" is ApiName's modelled @pattern with ^ and $ removed; the literal 'bad@name' in the emulator's text is this one recording's request value and is NOT part of the requirement — the defect is that any value is interpolated there at all, plus the retained ^/$ and the wrong errortype.

Cause 2 — fixture appsync-event-api-update-error-recovery, step task#4 (operation UpdateApi).
Status: expected 400, got 400 (correct).
Expected (AWS, recorded):
  header x-amzn-errortype: ValidationException
  $.message: "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_\\-\\ ]+"
Actual (emulator):
  header x-amzn-errortype: BadRequestException
  $.message: "1 validation error detected: Value 'name#1' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_\\-\\ ]+$"
Replay detail: "$.message: value_mismatch" and "$headers.x-amzn-errortype: value_mismatch (expected 'ValidationException', got 'BadRequestException')". As above, 'name#1' is this recording's request value filling the template, not a requirement.

Model citations used: smithy_query(appsync) — CreateApi POST /v2/apis, input name @required -> ApiName, errors {BadRequestException 400, ConcurrentModificationException 409, ServiceQuotaExceededException 402, UnauthorizedException 401, InternalFailureException 500}; UpdateApi POST /v2/apis/{apiId}, input name @required -> ApiName, errors {AccessDeniedException 403, BadRequestException 400, ConcurrentModificationException 409, NotFoundException 404, UnauthorizedException 401, InternalFailureException 500}; shape ApiName = string, @length min 1 max 50, @pattern "^[A-Za-z0-9_\\-\\ ]+$". ValidationException appears in neither error set, hence "recorded behavior only" for the code.

## Steps to Reproduce
Cause 1 (CreateApi):
1. Point an appsync client at the emulator.
2. Call CreateApi with a name containing a character outside [A-Za-z0-9_- ] (e.g. '@' or '#') and a valid eventConfig: eventConfig={authProviders:[{authType:'API_KEY'}], connectionAuthModes:[{authType:'API_KEY'}], defaultPublishAuthModes:[{authType:'API_KEY'}], defaultSubscribeAuthModes:[{authType:'API_KEY'}]}. Disable client-side parameter validation so the request reaches the service.
3. Observe HTTP 400 (correct) but response header x-amzn-errortype: BadRequestException where AWS sends ValidationException.
4. Observe the message interpolates the submitted name after "Value" and renders the pattern anchored: "... Value '<name>' at 'name' ... pattern: ^[A-Za-z0-9_\\-\\ ]+$". AWS sends "... Value at 'name' ... pattern: [A-Za-z0-9_\\-\\ ]+".
5. ListApis confirms no Api was created — the refusal decision itself is correct; only the code and the message diverge. A compliant ASCII name succeeds, confirming the pattern check fires in the right place.

Cause 2 (UpdateApi):
1. CreateApi with a compliant name; note the returned api.apiId.
2. Call UpdateApi(apiId=<apiId>, name='name#1', eventConfig=<the same valid eventConfig>), again with client-side validation disabled.
3. Observe HTTP 400 with x-amzn-errortype: BadRequestException instead of ValidationException, and the same value-quoting, anchor-retaining message.
4. GetApi(apiId) confirms the Api still carries its original name (correct); only the refusal's code and message text diverge.
