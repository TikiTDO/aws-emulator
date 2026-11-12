Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-runtime.invoke-endpoint-missing-permission-rejected
Issue-Shape-Id: com.amazonaws.sagemakerruntime#AmazonSageMakerRuntime
Issue-Key: sagemaker-runtime.invoke-endpoint-missing-permission-rejected|run#11|AssertionError|assert denied_code == "AccessDeniedException", f"expected AccessDeniedException, got {denied_code}: {denied_msg}"
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# sagemaker-runtime InvokeEndpoint evaluates no caller authorization: a principal with zero permissions is answered 400 ValidationError (endpoint-not-found) instead of 403 AccessDeniedException (1 cause)

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeEndpoint

## Description
Cause 1 of 1: InvokeEndpoint performs resource lookup without ever evaluating the caller's identity-based permissions, so an unauthorized principal is indistinguishable from a permitted one. Failing check: sagemaker-runtime.invoke-endpoint-missing-permission-rejected.

AWS: real AWS evaluates sigv4 authorization at the front door, BEFORE the endpoint is resolved. A principal holding no identity-based grant for sagemaker:InvokeEndpoint is refused with HTTP 403 and error code AccessDeniedException, with a message built from the request/identity of the form "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: sagemaker:InvokeEndpoint on resource: <endpoint ARN> because no identity-based policy allows the sagemaker:InvokeEndpoint action" (that construction rule is the requirement; the exact recorded text is only an example). A PERMITTED caller naming an endpoint that does not exist gets the modeled ValidationError instead — Smithy (services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json): InvokeEndpoint errors = InternalDependencyException(530), InternalFailure(500), ModelError(424), ModelNotReadyException(429), ServiceUnavailable(503), ValidationError(client, httpError 400); AccessDeniedException is NOT modeled on the operation because it is produced by the authorization layer, not the operation. The two outcomes are therefore distinct and ordered: authorization first (403 AccessDeniedException), resource existence second (400 ValidationError).

Emulator: both callers get the identical refusal. The check's positive control (default, permitted caller against a nonexistent endpoint name) correctly gets ValidationError, and then the SAME call issued with credentials for a freshly created role carrying zero attached policies ALSO gets 400 ValidationError, e.g. "Endpoint <name> of account 000000000000 not found." (recorded message, example only). No AccessDeniedException is ever produced, which shows authorization is not evaluated at all on this operation — not merely ordered differently.

Why: a client cannot tell a permissions failure from a missing resource. ValidationError is a non-retryable "fix your request" signal, while AccessDeniedException tells the caller to fix its IAM policy or assume a different role; SDK/caller error handling, alerting, and least-privilege regression tests all branch on that distinction. Worse, any test suite that uses this emulator to prove an endpoint is NOT reachable by an under-privileged principal passes vacuously: the unauthorized invocation is accepted as far as endpoint lookup, so a real deployment of the same code would be denied where the emulator said it was fine, and a policy omission is invisible until production.

Fix: belongs in the InvokeEndpoint operation's request authorization step — the caller's identity (the sigv4/assumed-role principal from the request context) must be checked against its identity-based policy for sagemaker:InvokeEndpoint on the target endpoint ARN before the endpoint is resolved in the control-plane store, and refused with 403/AccessDeniedException when no grant matches. This is the only operation of the service's data plane, so the gate lives in its handler (and in whatever shared IAM evaluation the emulator's IAM service exposes), not in the endpoint store.

Traces: sagemaker-runtime.invoke-endpoint-missing-permission-rejected ([mismatch] "expected AccessDeniedException, got ValidationError: Endpoint sim2real-smrt-never-created-<id> of account 000000000000 not found."). Handler: services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/src/ops/invoke_endpoint.rs (its header documents endpoint resolution, TargetVariant, EnableExplanations and SessionId refusals, but no authorization check).

## Evidence
Expected (real AWS): calling InvokeEndpoint(EndpointName=<never-created name>, Body=b"denied-caller") with credentials of an assumed role that has NO attached policies -> HTTP 403, Error.Code = "AccessDeniedException", message naming the principal ARN, the action sagemaker:InvokeEndpoint and the resource. The same call from the default permitted caller -> HTTP 400, Error.Code = "ValidationError" (endpoint does not exist).

Actual (emulator): both calls -> HTTP 400, Error.Code = "ValidationError", message e.g. "Endpoint sim2real-smrt-never-created-2f527c24b0e9 of account 000000000000 not found." Check assertion that failed: `assert denied_code == "AccessDeniedException"` -> AssertionError: expected AccessDeniedException, got ValidationError.

Model evidence: Smithy InvokeEndpoint declares ValidationError with httpError 400 and does not declare AccessDeniedException (authorization-layer error, HTTP 403).

## Steps to Reproduce
1. Boot the emulator and replay check sagemaker-runtime.invoke-endpoint-missing-permission-rejected.
2. Baseline/positive control: with the default credentials, call sagemaker-runtime InvokeEndpoint with EndpointName set to a name that was never created and Body=b"x". Observe 400 ValidationError (correct).
3. Create an IAM role with a trust policy allowing sts:AssumeRole by the caller and attach NO policies to it; AssumeRole into it.
4. With those zero-permission credentials, issue the identical InvokeEndpoint call against the same nonexistent endpoint name.
5. Observe HTTP 400 with Error.Code "ValidationError" ("Endpoint ... not found.") instead of HTTP 403 with Error.Code "AccessDeniedException"; the unauthorized principal is treated exactly like the permitted one.
