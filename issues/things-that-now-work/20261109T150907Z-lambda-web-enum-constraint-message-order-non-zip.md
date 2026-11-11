Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-web.application-log-level-rejects-an-unknown-value-and-defaults-to-info lambda-web.endpoint-latest-revision-mode-rejects-switching-onto-a-non-active-revision lambda-web.signed-https-get-on-web-function-endpoint-serves-handler-response lambda-web.unsigned-https-get-on-web-function-endpoint-rejected
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web.application-log-level-rejects-an-unknown-value-and-defaults-to-info|run#9|AssertionError|assert message == expected_message, f"unexpected validation message {message!r}"
Issue-Key: lambda-web.endpoint-latest-revision-mode-rejects-switching-onto-a-non-active-revision|run#20|AssertionError|assert rev_b["state"] == "Failed", (
Issue-Key: lambda-web.signed-https-get-on-web-function-endpoint-serves-handler-response|run#2|PublishedUrlRejected|url = ctx.endpoint_url(domain_name)
Issue-Key: lambda-web.unsigned-https-get-on-web-function-endpoint-rejected|run#2|PublishedUrlRejected|url = ctx.endpoint_url(domain_name)
Issue-Commit: 8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6

# lambda-web: enum-constraint message order, non-zip revision builds never reach Failed, and endpoint domainName is unreachable (3 causes)

> _Note: this issue was identified against the code base at commit `8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebFunction

## Description
Three independent root causes found replaying the behavioral suite against the lambda-web emulator (service-com-amazonaws-lambdaweb-lambdaweb). They are bundled because they all belong to this one service; each must be closed separately.

None of them is covered by an open issue. The nearest open lambda-web issues (20261107T082505Z-lambda-web-resource-not-found-and-reserved-tag.md, 20261107T125554Z-lambda-web-resource-policy-and-tag-refusals-diverge.md, 20261109T041005Z-lambda-web-deleteresourcepolicy-s-stale-revision-conflictexception-reports.md) concern TagResource/ResourcePolicy refusal wording only; nothing below touches those operations.

--- Cause 1 of 3: the ValidationException enum-set message enumerates the enum values in the wrong order ---
Operation: CreateWebFunction (same message shape applies to CreateWebFunctionRevision, which shares the revisionConfig validator).
Failing check: lambda-web.application-log-level-rejects-an-unknown-value-and-defaults-to-info

AWS: Smithy model com-amazonaws-lambdaweb-lambdaweb.json — CreateWebFunction.revisionConfig.serviceConfig.telemetryConfig.loggingConfig.applicationLogLevel targets enum ApplicationLogLevel; the operation models ValidationException. A value outside the enum is refused with ValidationException and AWS's standard constraint sentence:
  "1 validation error detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<enum value set>]"
The fixed text is everything outside the angle brackets, including the literal "1 validation error detected: ", "failed to satisfy constraint: Member must satisfy enum value set: " and the surrounding brackets. The variable parts are <value> (the rejected value, taken verbatim from the request) and <member path> (the dotted path of the offending member in the request shape). The <enum value set> is NOT a per-request value: it is fixed text for a given enum, and the recorded AWS behavior renders ApplicationLogLevel as "[FATAL, TRACE, WARN, ERROR, INFO, DEBUG]" — i.e. the service's own declaration order for that enum, which is NOT the least-to-most-severe order the model's documentation lists. Recorded behavior only for the ordering itself; the model's `values` list is TRACE, DEBUG, INFO, WARN, ERROR, FATAL, so the ordering cannot be derived from the model and must be carried as fixed text.
Emulator: returns the right error code (ValidationException), the right value and the right member path, but renders the value set in Smithy declaration order: "... Member must satisfy enum value set: [TRACE, DEBUG, INFO, WARN, ERROR, FATAL]". The whole message is therefore wrong for this error class.
Why: the enum-set message is the only machine-readable statement of which values are acceptable. A client (or a test, or a user-facing form built from the error) that parses the bracketed set or compares the message against AWS's gets a different string from the emulator than from AWS, so error-message assertions and any golden-file comparison of a refusal diverge even though the refusal itself is correct.
Fix: the request validator that renders enum-constraint messages for lambda-web (the shared ValidationException message builder used by CreateWebFunction's input validation), which should emit the service's fixed value-set text per enum rather than iterating the model's member order.
Traces: lambda-web.application-log-level-rejects-an-unknown-value-and-defaults-to-info (asserts the full message after confirming code == ValidationException).

--- Cause 2 of 3: a revision built from a code object that is not a zip archive reaches Active instead of Failed ---
Operation: CreateWebFunctionRevision (observed through GetWebFunctionRevision).
Failing check: lambda-web.endpoint-latest-revision-mode-rejects-switching-onto-a-non-active-revision

AWS: Smithy — RevisionState enum is [Pending, Active, Failed]; the model documents Pending = "The revision is being built", Active = "ready to serve traffic", Failed = "The revision build failed". CreateWebFunctionRevision is asynchronous (HTTP 202) and the build outcome is reported through GetWebFunctionRevision's state/stateReason. When the S3 object named by buildConfig.codeConfig.s3Object is readable but is not a valid zip archive, the build fails: the revision settles in state Failed with stateReason "Failed to read zip file: zip: not a valid zip file" (recorded behavior; that sentence is fixed text for this failure class, with no variable part).
Emulator: the same revision settles in state Active. The emulator never inspects the fetched code object, so every revision whose code is readable is reported as a successful build; Failed is apparently unreachable, and no stateReason is produced.
Why: a client cannot distinguish a built revision from a broken one. Deployment flows that gate on state (wait for Active, abort on Failed) will route traffic onto a revision whose code was never valid, and the downstream refusal the endpoint should give when asked to serve or auto-deploy a non-Active revision never fires, because the emulator has no non-Active terminal revision to refuse on.
Fix: the revision build state machine behind CreateWebFunctionRevision / CreateWebFunction (the component that transitions a revision out of Pending and populates state/stateReason), which must validate the fetched code object as a zip archive and settle the revision in Failed with the build's reason when it is not.
Traces: lambda-web.endpoint-latest-revision-mode-rejects-switching-onto-a-non-active-revision (revision B is deliberately an S3 object whose body is not a zip; the check asserts state == Failed and then stateReason).

--- Cause 3 of 3: the endpoint's published domainName is a real AWS public hostname that the emulator does not serve ---
Operation: CreateWebFunctionEndpoint (value observed on GetWebFunctionEndpoint.domainName / CreateWebFunction's endpoint).
Failing checks: lambda-web.signed-https-get-on-web-function-endpoint-serves-handler-response, lambda-web.unsigned-https-get-on-web-function-endpoint-rejected

AWS: Smithy — WebFunctionEndpoint carries a domainName property and GetWebFunctionEndpointResponse.domainName is documented as "The domain name assigned to the endpoint." On AWS that domain is the address the endpoint is actually reachable at over HTTPS: a client reads domainName off the endpoint and issues its HTTP request to it, which is the whole data plane of a web function endpoint.
Emulator: once the endpoint reaches Active it publishes a domainName of the form https://<32-hex-id>.lambda-web.<region>.on.aws — AWS's real public domain shape. The emulator does not serve that host; it serves only its own host and port. A client that follows the published domainName is sent off the emulator entirely, and the replay refuses the address with: "endpoint_url was given https://<id>.lambda-web.<region>.on.aws (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". (Host, port and the hex id differ per run and are not part of the requirement; the defect is that the announced host is not an address the emulator answers on.)
Why: the endpoint data plane is unusable. Every behavior that depends on invoking the endpoint is unreachable through the documented path — the handler response for a signed GET on an IamAuth endpoint, and the refusal of an unsigned GET on one — so a client cannot invoke a web function at all, and it cannot discover that from the control-plane responses, which all report the endpoint Active.
Fix: the endpoint address assignment/serialization for CreateWebFunctionEndpoint, UpdateWebFunctionEndpoint and GetWebFunctionEndpoint (the component that computes domainName and the per-endpoint data-plane listener), which must announce an address on a host and port the emulator itself answers on and serve the endpoint's HTTP requests there.
Traces: lambda-web.signed-https-get-on-web-function-endpoint-serves-handler-response, lambda-web.unsigned-https-get-on-web-function-endpoint-rejected (both read endpoint["domainName"] and are refused before any request is sent).

## Evidence
Suite: simparity-behavioral-aws-v1 — lambda-web 7/16 (44%), 9 failed. 2 failures were marked ALREADY FILED (unimplemented CreateWebFunctionEndpoint, ListWebFunctionRevisions) and 3 OWNED by open issues; the 4 checks below are the net-new ones, clustered into the 3 causes above.

Cause 1 — lambda-web.application-log-level-rejects-an-unknown-value-and-defaults-to-info
  Request: CreateWebFunction with revisionConfig.serviceConfig.telemetryConfig.loggingConfig.applicationLogLevel = "VERBOSE".
  Expected (AWS): ValidationException, Message =
    1 validation error detected: Value 'VERBOSE' at 'revisionConfig.serviceConfig.telemetryConfig.loggingConfig.applicationLogLevel' failed to satisfy constraint: Member must satisfy enum value set: [FATAL, TRACE, WARN, ERROR, INFO, DEBUG]
  Actual (emulator): ValidationException, Message =
    1 validation error detected: Value 'VERBOSE' at 'revisionConfig.serviceConfig.telemetryConfig.loggingConfig.applicationLogLevel' failed to satisfy constraint: Member must satisfy enum value set: [TRACE, DEBUG, INFO, WARN, ERROR, FATAL]
  Assertion: AssertionError: unexpected validation message "...[TRACE, DEBUG, INFO, WARN, ERROR, FATAL]" (check line 234). Only the bracketed value set differs; code, value and member path match.

Cause 2 — lambda-web.endpoint-latest-revision-mode-rejects-switching-onto-a-non-active-revision
  Setup: s3.put_object(Key="code-b.zip", Body=b"not a zip archive"); CreateWebFunctionRevision with buildConfig.codeConfig.s3Object = {bucket, key="code-b.zip"}, runtimeConfig.runtime = "nodejs24.x".
  Expected (AWS): GetWebFunctionRevision settles state = "Failed", stateReason = "Failed to read zip file: zip: not a valid zip file".
  Actual (emulator): state = "Active" (no stateReason).
  Assertion: AssertionError: revision B (built from a code object that is not a zip archive) reached 'Active', expected Failed (check line 253/1259).

Cause 3 — lambda-web.signed-https-get-on-web-function-endpoint-serves-handler-response and lambda-web.unsigned-https-get-on-web-function-endpoint-rejected
  Both read endpoint["domainName"] from a GetWebFunctionEndpoint whose state is Active, then GET it.
  Expected (AWS): domainName is the address the endpoint is served at, so the GET reaches the endpoint (signed GET on an IamAuth endpoint -> the handler's 200 response; unsigned GET -> the endpoint's refusal).
  Actual (emulator): domainName = https://30a3d3b458495b83c7b222404a451600.lambda-web.us-west-2.on.aws (signed check) and https://30d92011a1ae0de8a52bcfd8b2397cdc.lambda-web.us-west-2.on.aws (unsigned check) — AWS's real public domain, which the emulator does not serve. The replay refuses it:
    simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>.lambda-web.us-west-2.on.aws (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
  No HTTP request was ever sent, so the handler/auth behavior behind the endpoint is untested as well as unreachable. The hex ids, host and port are per-run values and are not part of the requirement.

## Steps to Reproduce
Boot the emulator from this checkout and point a lambda-web client at it, then:

Cause 1:
1. iam.CreateRole a lambda.amazonaws.com execution role; s3.CreateBucket, enable versioning, s3.PutObject a valid zip at key "code.zip".
2. CreateWebFunction(functionName=<new>, revisionConfig={buildConfig:{codeConfig:{s3Object:{bucket,key:"code.zip"}}, runtimeConfig:{runtime:"nodejs24.x"}}, serviceConfig:{executionRoleArn:<role>, telemetryConfig:{loggingConfig:{applicationLogLevel:"VERBOSE"}}}}, endpointConfig={endpointName:"ep", endpointType:"HomeRegion", authType:"ApplicationManaged"}).
3. Observe the ValidationException Message: the bracketed enum value set is [TRACE, DEBUG, INFO, WARN, ERROR, FATAL] instead of AWS's [FATAL, TRACE, WARN, ERROR, INFO, DEBUG].

Cause 2:
1. Same role/bucket; s3.PutObject(Key="code-a.zip", Body=<valid zip>) and CreateWebFunction with it; poll GetWebFunctionRevision until Active.
2. s3.PutObject(Key="code-b.zip", Body=b"not a zip archive").
3. CreateWebFunctionRevision(functionName, buildConfig={codeConfig:{s3Object:{bucket,key:"code-b.zip"}}, runtimeConfig:{runtime:"nodejs24.x"}}, serviceConfig={executionRoleArn:<role>}).
4. Poll GetWebFunctionRevision on the new revisionId until state is Active or Failed: the emulator reports Active; AWS reports Failed with stateReason "Failed to read zip file: zip: not a valid zip file".

Cause 3:
1. Create a web function with a valid zip and an endpoint (endpointType "HomeRegion"); poll GetWebFunctionEndpoint until state == "Active".
2. Read domainName off the response: it is https://<32-hex>.lambda-web.<region>.on.aws.
3. Issue an HTTPS GET to that domainName (signed with SigV4 for an IamAuth endpoint, unsigned for the rejection case): the host is not one the emulator serves, so the request never reaches the emulator. Expected: domainName names an address on the emulator's own host/port and the GET is served (200 from the handler when signed/ApplicationManaged, the endpoint's refusal when unsigned on IamAuth).
