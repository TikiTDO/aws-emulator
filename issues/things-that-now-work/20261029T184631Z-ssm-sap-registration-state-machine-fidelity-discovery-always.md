Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-sap.register-application-discovery-status-transition ssm-sap.register-application-synchronous-response-state ssm-sap.start-application-nonexistent-rejected ssm-sap.concurrent-application-refresh-contention ssm-sap.stop-application-on-registering-app-is-accepted ssm-sap.start-configuration-checks-error-paths ssm-sap.list-components-nonexistent-application-resource-not-found ssm-sap.register-application-sap-abap-credentials-rejected ssm-sap.register-application-quota-limit-rejected
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap.register-application-discovery-status-transition|run#14|AssertionError|assert final_app["Status"] == "FAILED", final_app
Issue-Key: ssm-sap.register-application-synchronous-response-state|run#21|AssertionError|assert terminal["Status"] == "ERROR", (
Issue-Key: ssm-sap.start-application-nonexistent-rejected|_error#17|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ssm-sap.concurrent-application-refresh-contention|run#20|AssertionError|assert len(errors) == len(results), (
Issue-Key: ssm-sap|StopApplication|ConflictException
Issue-Key: ssm-sap.start-configuration-checks-error-paths|run#16|AssertionError|assert err_not_running.get("Message") == expected_not_running, (
Issue-Key: ssm-sap.list-components-nonexistent-application-resource-not-found|run#14|AssertionError|assert not resp.get("Components"), (
Issue-Key: ssm-sap.register-application-sap-abap-credentials-rejected|_error#7|AssertionError|raise AssertionError(f"expected {expect} to be rejected, but it SUCCEEDED with {result!r}")
Issue-Key: ssm-sap.register-application-quota-limit-rejected|run#10|AssertionError|raise AssertionError(f"never hit the applications quota after {_MAX_ATTEMPTS} registrations")
Issue-Commit: 1f4ddd4c47b0507bb1e50344cb9c3bb105203334

# ssm-sap registration/state-machine fidelity: discovery always succeeds, start/stop/refresh/configuration-check preconditions wrong, components fabricated, SAP_ABAP credentials and the applications quota unenforced

> _Note: this issue was identified against the code base at commit `1f4ddd4c47b0507bb1e50344cb9c3bb105203334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterApplication

## Description
Eight distinct root causes in the ssm-sap application lifecycle and its control-call preconditions. All eight are in one service and were clustered from the behavioral replay. (Related but NOT the same as the open issue 20261024T151725Z-ssm-sap-registerapplication-validation.md, which is about SYNCHRONOUS refusal of reserved aws: tag keys and unresolvable instance/database references at RegisterApplication; the causes below are about the ASYNCHRONOUS outcome of registration, the preconditions of the other control calls, component synthesis, the SAP_ABAP conditional-member rule and the account quota.)

Cause 1 of 8 — a registration against an EC2 instance that cannot be discovered settles SUCCESSFUL instead of failing.
AWS: RegisterApplication is asynchronous: it answers 200 with Application.Status=REGISTERING / DiscoveryStatus=REGISTERING and an OperationId. Discovery then runs by sending an SSM document to each instance in Instances. When the instance does not exist (or carries no SSM Agent/SAP software), discovery cannot complete: the application converges to Status=FAILED with DiscoveryStatus=REGISTRATION_FAILED, and the tracked REGISTER_APPLICATION operation converges to Status=ERROR carrying the relayed SSM refusal in StatusMessage. Smithy: ApplicationStatus enum = [ACTIVATED, STARTING, STOPPED, STOPPING, FAILED, REGISTERING, DELETING, UNKNOWN]; ApplicationDiscoveryStatus enum = [SUCCESS, REGISTRATION_FAILED, REFRESH_FAILED, REGISTERING, DELETING]; OperationStatus enum = [INPROGRESS, SUCCESS, ERROR]. The StatusMessage text is recorded behavior only; it is the relayed inner-SDK error, of the form "Error executing SSM document - AccessDeniedException User: arn:aws:sts::<account>:assumed-role/AWSServiceRoleForAWSSSMForSAP/ssm-sap is not authorized to perform: ssm:SendCommand on resource: arn:aws:ec2:<region>:<account>:instance/<instance-id> because no identity-based policy allows the ssm:SendCommand action (Service: Ssm, Status Code: 400, Request ID: <request-id>) (SDK Attempt Count: <n>)" — everything but <account>, <region>, <instance-id>, <request-id> and <n> is fixed text; <account>/<region>/<instance-id> come from the request and the signing context, and <request-id>/<n> are per-run values a client must not depend on.
Emulator: the registration always settles as if discovery had succeeded. GetApplication after the settle returns Status='ACTIVATED', DiscoveryStatus='SUCCESS', and the tracked operation reaches Status='SUCCESS' with no StatusMessage. Nothing in the emulator can ever produce FAILED / REGISTRATION_FAILED / ERROR for a registration.
Why: a client that registers an application and then waits for the registration to settle (the normal onboarding flow) is told the SAP application was discovered when no instance was ever contacted. Error-handling paths keyed on FAILED/REGISTRATION_FAILED/ERROR are dead code against the emulator, and every downstream precondition that keys off "registration failed" (Causes 2 and 3) is unreachable.
Fix: the settle/clock logic for a registration in the service's state (the REGISTER_APPLICATION pending transition and the application/operation terminal status it writes), plus the operation record's StatusMessage.
Traces: ssm-sap.register-application-discovery-status-transition, ssm-sap.register-application-synchronous-response-state.

Cause 2 of 8 — StartApplication does not refuse an application whose registration did not complete successfully.
AWS: StartApplication on a registered application whose registration ended in FAILED is refused with ValidationException (HTTP 400), message "The registration of application <ApplicationId> is failed." — fixed text with <ApplicationId> taken from the request. (StartApplication on a never-registered or deregistered id is ResourceNotFoundException "Application <ApplicationId> doesn't exist.", which the emulator already gets right.) Smithy: StartApplication errors = ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ValidationException(400, service-common).
Emulator: StartApplication against a registered application that never completed discovery answers HTTP 200 with a fresh OperationId, silently starting an application with no working backend. No "registration is failed" precondition exists in the handler.
Why: a client gets an OperationId for a start it can never observe, instead of the modeled 400 telling it the application must be re-registered.
Fix: the StartApplication handler's precondition check (src/ops/start_application.rs) — it must reject on the application's registration outcome, not only on existence. Depends on Cause 1 producing a FAILED application at all.
Traces: ssm-sap.start-application-nonexistent-rejected.

Cause 3 of 8 — StartApplicationRefresh is accepted (non-uniformly, under concurrency) on an application whose registration did not complete.
AWS: StartApplicationRefresh against an application whose registration ended in FAILED is refused with ConflictException (409) for EVERY caller, deterministically; concurrent callers all get the same error family and none of them files an operation record. Smithy: StartApplicationRefresh errors = ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), UnauthorizedException, ValidationException(400).
Emulator: five concurrent StartApplicationRefresh calls against the same settled application return a MIX — one HTTP 200 with an OperationId and four ConflictException. The accept/refuse decision is driven only by whether another operation is currently pending on the application (first caller wins, the rest conflict), never by the registration outcome, so the outcome depends on request interleaving rather than on the application's state.
Why: a client racing refreshes sees an inconsistent, interleaving-dependent result set, and one caller is handed an operation that AWS would never have created; the state the refresh writes is then based on a registration that never succeeded.
Fix: the StartApplicationRefresh handler's precondition check (src/ops/start_application_refresh.rs) — gate on the application's registration outcome before the pending-operation gate, so the refusal is the same for all callers regardless of order.
Traces: ssm-sap.concurrent-application-refresh-contention.

Cause 4 of 8 — StopApplication refuses a REGISTERING application that AWS accepts.
AWS: StopApplication on an application that is still REGISTERING (never ACTIVATED) is ACCEPTED: HTTP 200 with a tracked OperationId, exactly as for a running application, and the application resource stays fully usable afterwards (TagResource/ListTagsForResource still work against its ARN). Recorded behavior only — the model declares ConflictException for the operation but does not say REGISTERING is a conflict.
Emulator: StopApplication answers ConflictException (409) with message "Application <ApplicationId> is REGISTERING and cannot be stopped until that completes" — a refusal AWS does not make, and a message AWS never emits.
Why: a client that stops an application immediately after registering it (a common teardown path) gets a 409 it must special-case, and never receives the OperationId it needs to track the stop.
Fix: the StopApplication handler's precondition gate (src/ops/stop_application.rs) — REGISTERING must not be treated as an in-progress-operation conflict for this operation.
Traces: ssm-sap.stop-application-on-registering-app-is-accepted.

Cause 5 of 8 — StartConfigurationChecks' ConflictException message for a not-successfully-registered application is the emulator's own wording.
AWS: StartConfigurationChecks against a registered application that has not reached a successful registration is refused with ConflictException (409) and the exact message "The specified application has not been registered successfully" — entirely fixed text, with no values interpolated from the request or state. (The never-registered case is ResourceNotFoundException "Application <ApplicationId> doesn't exist.", which the emulator already gets right.) Smithy: StartConfigurationChecks errors = ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ValidationException(400).
Emulator: the code is right but the message is "Application <ApplicationId> has an operation in progress and configuration checks cannot be started until that operation completes" (src/ops/start_configuration_checks.rs, the `application.pending.is_some()` branch at ~lines 187-196) — different fixed text, and it interpolates an ApplicationId that AWS's message does not contain.
Why: a client or test that matches on the error message (the only way to distinguish "not registered successfully" from the other ConflictException cases this operation raises, since both use code ConflictException) misclassifies the failure.
Fix: the message helper used by the StartConfigurationChecks conflict branch (src/ops/start_configuration_checks.rs with crate::state::conflict) — the not-successfully-registered case needs its own AWS-worded message, distinct from the "check already in progress" one.
Traces: ssm-sap.start-configuration-checks-error-paths.

Cause 6 of 8 — RegisterApplication fabricates a component for an application registered with no ComponentsInfo.
AWS: a HANA application registered without ComponentsInfo (and whose discovery never completed) has NO components: ListComponents for it returns an empty Components list. Components are produced by discovery of the actual SAP landscape, not derived from the request's Sid/ApplicationType. Smithy: ListComponents output Components (ComponentSummaryList); RegisterApplication input ComponentsInfo is optional.
Emulator: ListComponents on a freshly registered application returns one synthesized component derived from the request: [{ApplicationId: <app-id>, ComponentId: "<Sid>-<ApplicationType>" (e.g. "HDB-HANA"), ComponentType: "HANA", Arn: "arn:aws:ssm-sap:<region>:<account>:component/<app-id>/<Sid>-<ApplicationType>"}]. The same fabricated component also appears in the Application.Components list of RegisterApplication/GetApplication.
Why: a client enumerating components to drive per-component operations (GetComponent, backups, configuration checks) is handed a component id that corresponds to nothing, and cannot distinguish "discovery has not produced components yet" from "one component exists".
Fix: the component derivation in RegisterApplication / the service state (src/ops/register_application.rs and the component records in src/state.rs) — components must come from ComponentsInfo and from a completed discovery, not be synthesized from Sid + ApplicationType.
Traces: ssm-sap.list-components-nonexistent-application-resource-not-found.

Cause 7 of 8 — RegisterApplication accepts a SAP_ABAP registration carrying Credentials.
AWS: Credentials are meaningful only for a HANA registration. A RegisterApplication call with ApplicationType=SAP_ABAP that carries Credentials is refused with ValidationException (HTTP 400) and the message "The SID and database ARN of ASCS or Application Server must be specified for SAP applications with type SAP_ABAP." — entirely fixed text for this error class, with no values from the request. The rejected call leaves nothing behind: GetApplication for that ApplicationId then answers ValidationException "No registered application found for account: <account>, and application: <ApplicationId>". Smithy: RegisterApplication errors = ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ValidationException(400, service-common); the SAP_ABAP conditional-member rule is documented prose, not a model constraint.
Emulator: the call SUCCEEDS with HTTP 200, returning a fully registered SAP_ABAP application (Status=REGISTERING, DiscoveryStatus=REGISTERING, Components=["ABC-HANA"]) and an OperationId — a type/credentials combination AWS never admits, and it persists state.
Why: a client's malformed SAP_ABAP onboarding is silently accepted and leaves a bogus application registered in the account, so the client never learns it must supply the ASCS/Application Server SID and database ARN instead of Credentials.
Fix: the conditional-member validation in the RegisterApplication handler (src/ops/register_application.rs), which today validates ApplicationType and some type-conditional members but not the SAP_ABAP/Credentials exclusion.
Traces: ssm-sap.register-application-sap-abap-credentials-rejected.

Cause 8 of 8 — no per-account Applications quota is enforced by RegisterApplication.
AWS: an account may register only up to its ssm-sap Applications quota (observed to be on the order of 10). The registration that would exceed it is refused with ValidationException (HTTP 400) and the message "Resource Limit Reached. Account <account> is only allotted <quota> applications. Currently has <count> applications" — the fixed text is "Resource Limit Reached. Account " / " is only allotted " / " applications. Currently has " / " applications", with <account> the caller's account id and <quota>/<count> the account's allotment and its current registered-application count (equal at the boundary). The refused application is not created (GetApplication answers ValidationException "No registered application found for account: <account>, and application: <ApplicationId>"), the applications that reached the limit stay registered, and deregistering exactly one lets the identical call succeed — i.e. the rejection tracks the live count, not the request. Recorded behavior only for the quota value and message; the model declares no quota constraint.
Emulator: 20 consecutive RegisterApplication calls all return HTTP 200; no registration is ever refused for the account limit, so the ValidationException is unreachable.
Why: a client cannot exercise or handle the quota boundary at all, and capacity-planning/back-pressure code paths are dead against the emulator.
Fix: an account/region-scoped applications-count limit checked in the RegisterApplication handler before any state is written (src/ops/register_application.rs plus the applications map in src/state.rs), driven by the live registered count so a deregistration frees a slot.
Traces: ssm-sap.register-application-quota-limit-rejected.

## Evidence
Replay output, per cause.

Cause 1 — ssm-sap.register-application-discovery-status-transition:
  expected: Application.Status == 'FAILED', DiscoveryStatus == 'REGISTRATION_FAILED', operation Status == 'ERROR'
  actual:   {'Id': 'sim2real-sap-ca7d809686d1', 'Type': 'HANA', 'Arn': 'arn:aws:ssm-sap:us-west-2:000000000000:application/sim2real-sap-ca7d809686d1', 'AppRegistryArn': '...', 'Status': 'ACTIVATED', 'DiscoveryStatus': 'SUCCESS', 'Components': ['HDB-HANA'], 'LastUpdated': ...}
  ssm-sap.register-application-synchronous-response-state:
  expected: the registration Operation ends in ERROR with StatusMessage "Error executing SSM document - AccessDeniedException User: arn:aws:sts::<account>:assumed-role/AWSServiceRoleForAWSSSMForSAP/ssm-sap is not authorized to perform: ssm:SendCommand on resource: arn:aws:ec2:<region>:<account>:instance/<instance-id> because no identity-based policy allows the ssm:SendCommand action (Service: Ssm, Status Code: 400, Request ID: <request-id>) (SDK Attempt Count: <n>)"
  actual:   AssertionError: expected the registration Operation to end in ERROR, got 'SUCCESS'

Cause 2 — ssm-sap.start-application-nonexistent-rejected:
  expected: ValidationException (400), Message "The registration of application <ApplicationId> is failed."
  actual:   AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'OperationId': 'f3e0f74c-aee0-4b95-99bf-26ed3a2f3663'}
  (the earlier never-registered assertion in the same check passed: ResourceNotFoundException "Application <id> doesn't exist.")

Cause 3 — ssm-sap.concurrent-application-refresh-contention:
  expected: all 5 concurrent StartApplicationRefresh calls rejected with ConflictException, and no new operation records
  actual:   AssertionError: expected every racer to be rejected the same way, got 1 unexpected successes among [('ok', '99141175-82e3-4f40-8153-ad91b7c25a9c'), ('error', 'ConflictException'), ('error', 'ConflictException'), ('error', 'ConflictException'), ('error', 'ConflictException')]

Cause 4 — ssm-sap.stop-application-on-registering-app-is-accepted:
  expected: HTTP 200 with an OperationId for StopApplication on a REGISTERING application
  actual:   botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the StopApplication operation: Application sim2real-sap-af3e43108957 is REGISTERING and cannot be stopped until that completes

Cause 5 — ssm-sap.start-configuration-checks-error-paths:
  expected message: 'The specified application has not been registered successfully'
  actual message:   'Application sim2real-sap-b7bb1e60c033 has an operation in progress and configuration checks cannot be started until that operation completes'
  (code ConflictException matched; the never-registered ResourceNotFoundException assertion earlier in the check passed)

Cause 6 — ssm-sap.list-components-nonexistent-application-resource-not-found:
  expected: no components on a freshly registered app with no ComponentsInfo
  actual:   [{'ApplicationId': 'sim2real-sap-c1fe76c3d5a2', 'ComponentId': 'HDB-HANA', 'ComponentType': 'HANA', 'Arn': 'arn:aws:ssm-sap:us-west-2:000000000000:component/sim2real-sap-c1fe76c3d5a2/HDB-HANA'}]
  (the ResourceNotFoundException "Application <id> doesn't exist." assertion earlier in the same check passed)

Cause 7 — ssm-sap.register-application-sap-abap-credentials-rejected:
  expected: ValidationException (400), Message 'The SID and database ARN of ASCS or Application Server must be specified for SAP applications with type SAP_ABAP.'
  actual:   AssertionError: expected a SAP_ABAP RegisterApplication call carrying Credentials to be rejected, but it SUCCEEDED with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Application': {'Id': 'sim2real-sap-abap-e897acca195c', 'Type': 'SAP_ABAP', 'Status': 'REGISTERING', 'DiscoveryStatus': 'REGISTERING', 'Components': ['ABC-HANA'], ...}, 'OperationId': '29061cce-c2e0-4587-adda-6b2d9428b932'}

Cause 8 — ssm-sap.register-application-quota-limit-rejected:
  expected: some registration refused with ValidationException (400), Message 'Resource Limit Reached. Account <account> is only allotted <quota> applications. Currently has <count> applications'
  actual:   AssertionError: never hit the applications quota after 20 registrations

## Steps to Reproduce
Cause 1: POST /register-application {"ApplicationId":"APP1","ApplicationType":"HANA","Instances":["i-0deadbeefdeadbeef"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"CredentialType":"ADMIN","DatabaseName":"HDB/HDB","SecretId":"any"}]}; note the returned OperationId; poll POST /get-application {"ApplicationId":"APP1"} until Status != REGISTERING -> observe ACTIVATED/SUCCESS where AWS gives FAILED/REGISTRATION_FAILED; POST /get-operation {"OperationId":"<id>"} -> observe SUCCESS where AWS gives ERROR with the relayed SendCommand AccessDeniedException StatusMessage.

Cause 2: with APP1 settled as above, POST /start-application {"ApplicationId":"APP1"} -> observe 200 + OperationId; AWS answers 400 ValidationException "The registration of application APP1 is failed."

Cause 3: with APP1 settled, fire 5 concurrent POST /start-application-refresh {"ApplicationId":"APP1"} -> observe 1x 200 + OperationId and 4x 409 ConflictException; AWS refuses all 5 with ConflictException and files no operation.

Cause 4: POST /register-application {"ApplicationId":"APP2",...} and immediately (while Status==REGISTERING) POST /stop-application {"ApplicationId":"APP2"} -> observe 409 ConflictException "Application APP2 is REGISTERING and cannot be stopped until that completes"; AWS answers 200 with an OperationId.

Cause 5: POST /register-application {"ApplicationId":"APP3",...} and immediately POST /start-configuration-checks {"ApplicationId":"APP3"} -> observe 409 ConflictException with message "Application APP3 has an operation in progress and configuration checks cannot be started until that operation completes"; AWS answers 409 ConflictException "The specified application has not been registered successfully".

Cause 6: POST /register-application {"ApplicationId":"APP4","ApplicationType":"HANA","Sid":"HDB",...} with no ComponentsInfo, then POST /list-components {"ApplicationId":"APP4"} -> observe one component ComponentId "HDB-HANA"; AWS returns an empty Components list.

Cause 7: POST /register-application {"ApplicationId":"ABAP1","ApplicationType":"SAP_ABAP","Instances":["i-0deadbeefdeadbeef"],"Sid":"ABC","Credentials":[{"CredentialType":"ADMIN","DatabaseName":"HDB/HDB","SecretId":"any"}]} -> observe 200 and a registered application; AWS answers 400 ValidationException "The SID and database ARN of ASCS or Application Server must be specified for SAP applications with type SAP_ABAP."

Cause 8: POST /register-application 20 times with distinct ApplicationIds in one account/Region -> observe all 20 answer 200; AWS refuses at the account's Applications quota with 400 ValidationException "Resource Limit Reached. Account <account> is only allotted <quota> applications. Currently has <count> applications", leaves the refused application uncreated, and accepts the identical call again after one deregistration.
