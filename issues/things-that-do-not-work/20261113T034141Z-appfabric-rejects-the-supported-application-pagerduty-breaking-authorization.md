Issue-Source: trace-sweep-divergence
Issue-Trace: appfabric-app-authorization-connect-apikey
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appfabric-app-authorization-connect-apikey appfabric-ingestion-and-destination-error-paths
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric|CreateAppAuthorization|2xx->4xx|$.appAuthorization
Issue-Key: appfabric|CreateAppAuthorization|2xx->4xx|status
Issue-Key: appfabric|CreateIngestion|4xx->4xx|$.message
Issue-Key: appfabric|StartUserAccessTasks|2xx->2xx|$.userAccessTasksList[*]
Issue-Commit: 29d660df289a0a2ea7ea951794d8cec859c05e7f

# AppFabric rejects the supported application PAGERDUTY, breaking authorization, ingestion and user-access flows

> _Note: this issue was identified against the code base at commit `29d660df289a0a2ea7ea951794d8cec859c05e7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppAuthorization

## Description
Three causes, all rooted in the emulator's hard-coded supported-application allow-list (crate::state::SUPPORTED_APPLICATIONS / is_supported_app), which omits PAGERDUTY. Every AppFabric flow over a PAGERDUTY tenant therefore fails, and the failures surface in three different operations.

Cause 1 of 3: CreateAppAuthorization refuses a PAGERDUTY app authorization.
AWS: recorded behavior only. The Smithy model types CreateAppAuthorizationRequest.app as the open String255 (@length 1..255) and constrains nothing else; the member's documentation carries a valid-value list (SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE DROPBOX SMARTSHEET CISCO) that the recording shows is stale. The recorded response for app=PAGERDUTY with authType=apiKey is HTTP 201 (the operation's modeled @http code) carrying the required appAuthorization member: app PAGERDUTY, authType apiKey, status PendingConnect, the request's tenant echoed back, the authorization ARN nested under the bundle ARN, and createdAt/updatedAt. So the real service serves PAGERDUTY as a supported application.
Emulator: answers HTTP 400 ValidationException with message "app PAGERDUTY is not supported" and no appAuthorization member at all. Fixed text of that emulator message is "app " / " is not supported"; the only variable part is the app name taken from the request — the whole message is the emulator's own invention for a request AWS accepts, so no AWS wording applies: the correct behavior is a 201 success, not a differently worded error.
Why: a client cannot create any PAGERDUTY app authorization against the emulator, so every PAGERDUTY onboarding path (authorize -> connect -> ingest) is unreachable and later steps of the same flow are blocked for want of a resource the real service would have created.
Fix: the validation step of the CreateAppAuthorization handler, specifically the supported-application set it consults in the service's state module (the allow-list must include PAGERDUTY; it is also what every sibling create consults).
Traces: appfabric-app-authorization-connect-apikey#task#2, appfabric-ingestion-and-destination-error-paths#setup#2.

Cause 2 of 3: CreateIngestion reports the wrong refusal for a PAGERDUTY tenant whose authorization is not connected.
AWS: recorded behavior only (the model declares ValidationException at HTTP 400 for CreateIngestion and says nothing about the message). The recording shows HTTP 400 with message "The app authorization for the specified application and tenant was not connected. Connect the app authorization for the application and tenant before creating an ingestion." That sentence is entirely fixed text — it interpolates no value from the request or from state — and it tells the caller the remediation it still owes (run ConnectAppAuthorization).
Emulator: same status 400 but message "app PAGERDUTY is not supported" (fixed text "app "/" is not supported" with the app name from the request), i.e. it refuses the application outright instead of reporting the unmet connect precondition. The precondition sentence itself already exists in the service's state module, so only the ordering/allow-list is wrong.
Why: a client cannot distinguish "this application is unusable" from "finish connecting this authorization"; retry logic that connects and re-creates the ingestion never fires, and error-handling keyed on the documented precondition sentence breaks.
Fix: the CreateIngestion handler's validation — the supported-application check must not shadow the not-connected precondition for an application the service serves.
Traces: appfabric-ingestion-and-destination-error-paths#task#3.

Cause 3 of 3: StartUserAccessTasks returns an empty userAccessTasksList where AWS reports one item per application tenant.
AWS: recorded behavior only; the model's UserAccessTaskItem requires app and tenantId and allows taskId and error (TaskError{errorCode,errorMessage}), and the operation answers HTTP 201. The recording shows the list carrying one item per (app, tenant) pair the bundle holds: app PAGERDUTY, tenantId the bundle's PagerDuty tenant identifier, no taskId, and error errorCode "notfound" with errorMessage "No authentication token was found for this user. User needs to authorize." — that errorMessage is wholly fixed text, and the variable parts of the item are app and tenantId, taken from the stored authorization.
Emulator: answers 201 with userAccessTasksList empty (userAccessTasksList[0] missing), because the fan-out iterates the bundle's app authorizations and Cause 1 prevented the PAGERDUTY authorization from ever being stored.
Why: a client is handed a shorter list than the number of application tenants in the bundle, so it never learns that the user must authorize; code that iterates the returned items to drive the authorize step silently does nothing.
Fix: the same supported-application allow-list in the service's state module that the create path consults; the StartUserAccessTasks fan-out itself already emits the no-token item for an unconnected authorization.
Traces: appfabric-ingestion-and-destination-error-paths#task#15.

Related open issues (none of which cover these causes): 20261029T200017Z-appfabric-date-time-members-serialize-as-epoch-numbers.md (date-time serialization), 20261103T031921Z-appfabric-duplicate-createappauthorization-conflict-message-is-the-emulator.md (duplicate-create conflict wording and DeleteAppBundle), 20261106T165607Z-appfabric-connectappauthorization-never-validates-the-credential-a-failing.md (connect does not validate credentials).

## Evidence
Cause 1 — fixture appfabric-app-authorization-connect-apikey, step task#2, CreateAppAuthorization (also affects appfabric-ingestion-and-destination-error-paths#setup#2):
  expected: HTTP 201 with $.appAuthorization = {app: PAGERDUTY, authType: apiKey, status: PendingConnect, appAuthorizationArn, appBundleArn, tenant{tenantIdentifier,tenantDisplayName}, createdAt, updatedAt}
  actual:   HTTP 400, $.appAuthorization missing; body message "app PAGERDUTY is not supported"

Cause 2 — fixture appfabric-ingestion-and-destination-error-paths, step task#3, CreateIngestion:
  expected: HTTP 400, $.message = "The app authorization for the specified application and tenant was not connected. Connect the app authorization for the application and tenant before creating an ingestion."
  actual:   HTTP 400, $.message = "app PAGERDUTY is not supported"

Cause 3 — fixture appfabric-ingestion-and-destination-error-paths, step task#15, StartUserAccessTasks:
  expected: HTTP 201, $.userAccessTasksList[0] = {app: PAGERDUTY, tenantId: <bundle's PagerDuty tenant identifier>, error: {errorCode: "notfound", errorMessage: "No authentication token was found for this user. User needs to authorize."}}
  actual:   HTTP 201, $.userAccessTasksList[0] missing (empty list)

Source: services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs lines 164-194 (SUPPORTED_APPLICATIONS, is_supported_app, app_not_supported) and services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_app_authorization.rs lines 268-273 (the is_supported_app refusal, taken before the credential/uniqueness checks).

Smithy: CreateAppAuthorizationRequest.app -> String255 @length(1,255), no enum; CreateAppAuthorization @http code 201, errors include ValidationException (400); CreateIngestion @http code 201, errors include ValidationException (400); StartUserAccessTasks @http code 201, output userAccessTasksList: UserAccessTasksList of UserAccessTaskItem{app*, tenantId*, taskId, error: TaskError{errorCode,errorMessage}}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture appfabric-app-authorization-connect-apikey step task#2 — POST /appbundles/{appBundleIdentifier}/appauthorizations with app=PAGERDUTY, authType=apiKey, an apiKey credential and a PagerDuty tenant. Observe 400 "app PAGERDUTY is not supported" instead of 201 with the appAuthorization. Same request appears as appfabric-ingestion-and-destination-error-paths step setup#2.
3. Cause 2: replay fixture appfabric-ingestion-and-destination-error-paths step task#3 — POST /appbundles/{appBundleIdentifier}/ingestions with app=PAGERDUTY and the PagerDuty tenantId whose authorization is not connected. Observe the 400 message "app PAGERDUTY is not supported" instead of the not-connected precondition sentence.
4. Cause 3: replay fixture appfabric-ingestion-and-destination-error-paths step task#15 — POST /useraccess/start with the bundle identifier and an email. Observe 201 with an empty userAccessTasksList instead of one item per application tenant carrying errorCode notfound.
