Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-sap.start-application-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap.start-application-nonexistent-rejected|_error#22|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: f515581569b98a9dcbd2e686c12f912fdce51bba

# DeregisterApplication leaves the application resolvable in DELETING, so GetApplication succeeds instead of refusing with ValidationException "No registered application found..."

> _Note: this issue was identified against the code base at commit `f515581569b98a9dcbd2e686c12f912fdce51bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeregisterApplication

## Description
One root cause, one failing check.

Cause 1 of 1: a successful DeregisterApplication does not take the application out of the read path, so a caller that reads the id straight afterwards is told the application still exists (failing check: ssm-sap.start-application-nonexistent-rejected).

AWS: recorded behavior only. The Smithy model fixes the error families but not the lifecycle: DeregisterApplication (POST /deregister-application) models ValidationException (400), UnauthorizedException (401) and InternalServerException (500) and returns an EMPTY output — it hands back no OperationId, unlike RegisterApplication/StartApplication/StopApplication, which do. GetApplication models only ValidationException (400) and InternalServerException (500) — no ResourceNotFoundException. The recording the check replays shows that once DeregisterApplication has returned 200 for an ApplicationId, the very next GetApplication for that same id is REFUSED with HTTP 400, error code ValidationException and message "No registered application found for account: <account-id>, and application: <application-id>" — fixed text "No registered application found for account: ", ", and application: ", with the account id taken from the account the lookup runs in and the application id taken verbatim from the request. The recording further shows that a subsequent StartApplication on the same deregistered id is refused with HTTP 404 ResourceNotFoundException and message "Application <application-id> doesn't exist." (fixed text "Application " / " doesn't exist.", the id taken from the request) — i.e. the deregistered id is gone for readers and mutators alike as soon as the deregistration is accepted, and the two operations answer that same condition in their own error families.

Emulator: DeregisterApplication accepts the call and then models the removal as a DELAYED change owed by the clock: it sets the application's Status/DiscoveryStatus to DELETING and stores a pending Removal due one TRANSITION_SPAN later, leaving the record in the Region's applications map until a later request settles it. The application therefore stays resolvable, and the immediately following GetApplication returns HTTP 200 with Application.Status = "DELETING", Application.DiscoveryStatus = "DELETING", the full Arn/AppRegistryArn/Tags of the still-stored record, instead of the 400 ValidationException. For the same window, a StartApplication on that id resolves an existing application rather than reporting the 404 ResourceNotFoundException "Application <id> doesn't exist." the recording shows.

Why: a client relying on AWS's behavior treats a returned DeregisterApplication as the point the registration stops existing — teardown helpers, idempotent cleanup loops and "is it gone yet" polls all key off the not-found refusal (code ValidationException with the "No registered application found" message for GetApplication, ResourceNotFoundException for the mutating calls). Against the emulator those clients instead observe a live application with Status DELETING, so cleanup loops keep polling or re-deregistering, assertions that the resource is gone fail, and mutations that AWS refuses as not-found are accepted against a record AWS has already dropped. The window is also wall-clock dependent, so the same client code passes or fails depending on how fast it issues the next call.

Fix: DeregisterApplication's handler and the application lifecycle in this service's state — the deregistration must make the application unresolvable to the read/mutate path at the instant it is accepted (drop it, or mark it so that resolve_application_id no longer answers for it), rather than parking a pending Removal and leaving the record visible in DELETING. The refusal wording already exists in state (no_registered_application for GetApplication's 400, application_not_found_message for the 404 family), so the fix is in when the record stops resolving, not in the message builders.

Traces: ssm-sap.start-application-nonexistent-rejected (fails at the GetApplication-after-DeregisterApplication step: expected a ClientError, got 200 with Application.Status "DELETING"). File/line citations: services/service-com-amazonaws-ssmsap-ssmsap/src/ops/deregister_application.rs lines 176-210 (pending Removal, status set to DELETING, record re-inserted), services/service-com-amazonaws-ssmsap-ssmsap/src/ops/get_application.rs lines 168-199 (resolve + project the still-stored record), services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs lines 376 (TRANSITION_SPAN), 851-908 (StoredApplication::settle leaves a due removal in place) and 1946+ (RegionState::settle is what finally forgets it).

## Evidence
Check ssm-sap.start-application-nonexistent-rejected, step: client.deregister_application(ApplicationId=app_id) returns 200, then client.get_application(ApplicationId=app_id).

Expected (AWS, as recorded): ClientError, HTTP 400, Error.Code = "ValidationException", Error.Message = "No registered application found for account: <account-id>, and application: <application-id>" (fixed text as quoted; account id from the calling account, application id from the request).

Actual (emulator): HTTP 200, no error. Response body:
  {'Application': {'Id': '<app-id>', 'Type': 'HANA', 'Arn': 'arn:aws:ssm-sap:<region>:<account>:application/<app-id>', 'AppRegistryArn': 'arn:aws:servicecatalog:<region>:<account>:/applications/<uuid>', 'Status': 'DELETING', 'DiscoveryStatus': 'DELETING', 'Components': [], 'LastUpdated': <timestamp>}, 'Tags': {'sim2real': '1'}}
Harness assertion: "expected a ClientError, but the call SUCCEEDED and returned {...'Status': 'DELETING'...}". (Ids, ARNs, account, region and timestamp above are per-run values, not part of the requirement.)

Same cause, next step of the same check: StartApplication on that deregistered id must be HTTP 404 ResourceNotFoundException with message "Application <application-id> doesn't exist."; the emulator resolves the still-stored DELETING record instead.

## Steps to Reproduce
1. Boot the emulator and point a botocore ssm-sap client at it.
2. RegisterApplication with ApplicationId=<id>, ApplicationType=HANA, one Instances entry, Sid=HDB, SapInstanceNumber=00 and one ADMIN credential; wait until GetApplication reports a Status other than REGISTERING.
3. Call DeregisterApplication(ApplicationId=<id>) and observe it returns 200.
4. Immediately call GetApplication(ApplicationId=<id>).
   Expected: ClientError 400 ValidationException, message "No registered application found for account: <account-id>, and application: <id>".
   Actual: 200 with Application.Status = "DELETING" and the full stored record.
5. Immediately call StartApplication(ApplicationId=<id>).
   Expected: 404 ResourceNotFoundException, message "Application <id> doesn't exist."
   Actual: the DELETING record is resolved instead of a not-found refusal.
Or simply replay check ssm-sap.start-application-nonexistent-rejected.
