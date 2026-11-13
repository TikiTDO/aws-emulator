Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appfabric.create-app-authorization-invalid-app-name-rejected
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric.create-app-authorization-invalid-app-name-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Commit: 09276ae2a06db22058973d2ee0c9c36d823e0e87

# CreateAppAuthorization accepts an unsupported app name and creates the authorization instead of refusing with ValidationException

> _Note: this issue was identified against the code base at commit `09276ae2a06db22058973d2ee0c9c36d823e0e87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppAuthorization

## Description
Cause 1 of 1: CreateAppAuthorization performs no validation of the `app` member against AppFabric's fixed set of supported applications, so an arbitrary string is accepted and a real AppAuthorization resource is created.

AWS: Smithy (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json): CreateAppAuthorization.app targets String255 (@required, @length min 1 max 255) — the model does not express the supported-application list, so the enum is enforced server-side only; CreateAppAuthorization models ValidationException (client error, httpError 400). Recorded behavior: AWS refuses an app name outside the supported-application set with HTTP 400, error code `ValidationException`, and message `app <app> is not supported` — the fixed text is `app ` ... ` is not supported`, with the variable part being the `app` value copied verbatim from the request. No resource is created: a subsequent ListAppAuthorizations on the bundle returns an empty appAuthorizationSummaryList.

Emulator: the call succeeds. It answers HTTP 201 with a fully-populated `appAuthorization` whose `app` field echoes the unsupported name (e.g. app="NOT_A_REAL_SUPPORTED_APP", status="PendingConnect", a minted appAuthorizationArn under the bundle, createdAt/updatedAt, persona and authUrl), and the authorization is persisted — the check's `_error()` helper raised `AssertionError: expected the call to be refused, but it succeeded and returned {...'HTTPStatusCode': 201, ... 'app': 'NOT_A_REAL_SUPPORTED_APP'...}`.

Why: the emulator is overly permissive on a rejection class. A client that validates its app-name handling (a UI that enumerates supported apps, a config pipeline that catches typos, or a test asserting a bad app name is refused) sees a successful 201 and a live authorization ARN where AWS returns 400 ValidationException. Worse, the fake authorization then occupies the bundle and shifts the result of every later ListAppAuthorizations/GetAppAuthorization, and the per-tenant/authType dedup state, so downstream assertions diverge too.

Fix: input validation for CreateAppAuthorization in the appfabric crate's CreateAppAuthorization handler — reject any `app` value outside the supported-application set before any state mutation, returning ValidationException (HTTP 400) with the message `app <app> is not supported`, and leave the bundle's authorization set untouched. The same check should be applied by UpdateAppAuthorization-style paths that accept an app name.

Traces: behavioral check appfabric.create-app-authorization-invalid-app-name-rejected (group overly-permissive). The check's positive control (app="SLACK") and its terminal GetAppAuthorization read must keep passing after the fix.

## Evidence
Check: appfabric.create-app-authorization-invalid-app-name-rejected [mismatch]

Request: CreateAppAuthorization appBundleIdentifier=<bundle arn>, app="NOT_A_REAL_SUPPORTED_APP", authType="oauth2", credential.oauth2Credential={clientId, clientSecret}, tenant={tenantIdentifier=<generated>, tenantDisplayName="sim2real behavioral tenant"}

Expected (AWS): HTTP 400, Error.Code = "ValidationException", Error.Message = "app NOT_A_REAL_SUPPORTED_APP is not supported" (fixed text "app " + the request's app value + " is not supported"); afterwards ListAppAuthorizations(appBundleIdentifier=<bundle>) -> appAuthorizationSummaryList == [].

Actual (emulator): HTTP 201 with appAuthorization = {appAuthorizationArn: arn:aws:appfabric:us-east-1:<acct>:appbundle/<bundle-id>/appauthorization/<auth-id>, app: 'NOT_A_REAL_SUPPORTED_APP', authType: 'oauth2', status: 'PendingConnect', tenant: {...}, persona: 'admin', authUrl: ...}; the authorization is persisted in the bundle.

Assertion raised: AssertionError: expected the call to be refused, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'appAuthorization': {... 'app': 'NOT_A_REAL_SUPPORTED_APP' ...}} (check_appfabric_create_app_authorization_invalid_app_name_rejected.py line 22, via line 101).

## Steps to Reproduce
1. Point a botocore appfabric client at the emulator, region us-east-1.
2. CreateAppBundle(clientToken=<uuid>, tags=[{key:'sim2real', value:'create-app-authorization-invalid-app-name-rejected'}]) and keep appBundle.arn.
3. Call CreateAppAuthorization(appBundleIdentifier=<bundle arn>, app='NOT_A_REAL_SUPPORTED_APP', authType='oauth2', credential={'oauth2Credential': {'clientId': 'placeholder-client-id', 'clientSecret': 'placeholder-client-secret'}}, tenant={'tenantIdentifier': 'sim2real-tenant-<rand>', 'tenantDisplayName': 'sim2real behavioral tenant'}).
4. Observe HTTP 201 and an appAuthorization echoing app='NOT_A_REAL_SUPPORTED_APP'; AWS returns HTTP 400 ValidationException 'app NOT_A_REAL_SUPPORTED_APP is not supported'.
5. Call ListAppAuthorizations(appBundleIdentifier=<bundle arn>) — the emulator lists the bogus authorization; AWS lists none.
