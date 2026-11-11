Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.get-domain-name-access-denied appsync.list-domain-names-access-denied appsync.access-denied-domain-name appsync.access-denied-get-domain-name appsync.put-environment-variables-exceeds-size-limit-rejected appsync.graphql-api-tags-unicode-key-value-round-trip appsync.api-key-default-expiry-seven-days appsync.access-denied-sandbox-eval
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.get-domain-name-access-denied|run#4|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: appsync.list-domain-names-access-denied|run#3|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: appsync.access-denied-domain-name|_poll_until_denied#8|Timeout|time.sleep(interval_s)
Issue-Key: appsync.access-denied-get-domain-name|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: appsync.put-environment-variables-exceeds-size-limit-rejected|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appsync|TagResource|ValidationException
Issue-Key: appsync.api-key-default-expiry-seven-days|run#14|AssertionError|assert abs(key["deletes"] - (key["expires"] + grace_period)) <= tolerance, (
Issue-Key: appsync.access-denied-sandbox-eval|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# AppSync control-plane fidelity: domain-name operations unimplemented, constraint rejections use ValidationException instead of BadRequestException, ApiKey.deletes equals expires, no IAM default-deny

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainName

## Description
Four distinct root causes on the AppSync control plane. 4 causes in this issue; all four must be closed before submitting.

=== Cause 1 of 4: the domain-name operations are scaffold stubs, so neither the authorization error nor the lookup error is ever produced ===
AWS: Smithy (appsync) models GetDomainName (GET /v1/domainnames/{domainName}) with errors AccessDeniedException (403), BadRequestException (400), NotFoundException (404), InternalFailureException (500), and ListDomainNames (GET /v1/domainnames) with AccessDeniedException (403), BadRequestException (400), InternalFailureException (500); CreateDomainName/UpdateDomainName/DeleteDomainName carry the same AccessDeniedException/NotFoundException/BadRequestException set. A principal with no identity-based policy granting the action is rejected with AccessDeniedException/403 before any resource lookup; a permissioned principal naming a domain that was never created gets NotFoundException/404 (recorded message example: "Domain name not found"), and CreateDomainName with a certificate outside us-east-1 gets BadRequestException/400 (recorded message example: "Certificate must be in us-east-1.").
Emulator: every one of these calls answers `NotImplemented` with the message "operation <Op> is scaffolded but not implemented in this emulator" — observed directly for GetDomainName and ListDomainNames. Because the checks that bound IAM credential propagation poll until an AccessDeniedException appears, the constant NotImplemented makes them spin and the check is killed at the 60s harness timeout instead of ever converging.
Why: no client can manage AppSync custom domain names at all, and a client probing authorization sees an unmodeled error code (`NotImplemented`) that is in no SDK's error set for these operations, so error handling written against AWS never matches; the retry-until-settled pattern real SDK users write turns into a hang.
Note: no existing open issue names GetDomainName/ListDomainNames/CreateDomainName/UpdateDomainName/DeleteDomainName — the only filed AppSync stub issue is 20261023T113357Z-appsync-startschemacreation-stub.md (StartSchemaCreation), which does not cover these.
Fix: the domain-name operation handlers (get_domain_name.rs, list_domain_names.rs, create_domain_name.rs, update_domain_name.rs, delete_domain_name.rs) in the AppSync service crate, plus their state and error mapping.
Traces: appsync.get-domain-name-access-denied, appsync.list-domain-names-access-denied, appsync.access-denied-domain-name (60s timeout), appsync.access-denied-get-domain-name (60s timeout).

=== Cause 2 of 4: input-constraint violations are reported as ValidationException instead of the modeled BadRequestException ===
AWS: Smithy (appsync) PutGraphqlApiEnvironmentVariables declares `environmentVariables` -> EnvironmentVariableMap with @length min 0 max 50 and the error set {AccessDeniedException 403, BadRequestException 400, ConcurrentModificationException 409, NotFoundException 404, UnauthorizedException 401, InternalFailureException 500}; TagResource declares `tags` -> TagMap @length 1..50 and the error set {AccessDeniedException 403, BadRequestException 400, LimitExceededException 429, NotFoundException 404, UnauthorizedException 401, InternalFailureException 500}. `ValidationException` is NOT a modeled AppSync error for either operation, so a constraint violation surfaces as BadRequestException with HTTP 400. The message is itself rule-derived (it names the offending member and its constraint), e.g. "1 validation error detected: Value '{...}' at 'environmentVariables' failed to satisfy constraint: Member must have length less than or equal to 50".
Emulator: the generic constraint-validation layer rejects the request with error code `ValidationException` instead of `BadRequestException` — observed for a 60-entry environmentVariables map and for a tag key/value outside the allowed charset (recorded: "1 validation error detected: Value 'val-...' at 'tags.Unicode_Tag_...' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\s\w+-=\.:/@]*$"). Note the validation DECISION is right in both cases; only the error code is wrong.
Why: botocore/SDK clients model AppSync's errors from the Smithy set — `appsync.exceptions.BadRequestException` never matches, so `except BadRequestException` blocks do not catch the rejection and the error escapes as a generic ClientError. Code that branches on the error code takes the wrong branch.
Fix: the AppSync request-validation/error-mapping layer must emit the service's modeled BadRequestException (HTTP 400) for @length/@pattern violations rather than the generic ValidationException.
Traces: appsync.put-environment-variables-exceeds-size-limit-rejected, appsync.graphql-api-tags-unicode-key-value-round-trip.

=== Cause 3 of 4: CreateApiKey sets ApiKey.deletes equal to ApiKey.expires, with no 60-day post-expiry retention ===
AWS: Smithy (appsync) ApiKey.deletes is documented "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour" — a DISTINCT instant from ApiKey.expires, because AppSync retains an expired key for a further 60-day grace period before deleting it; recorded behavior gives deletes == expires + 60 days (both hour-rounded). With no explicit `expires` argument, expires defaults to ~7 days out (that part matched).
Emulator: `deletes` is returned identical to `expires` (observed deletes=1793415600 with expires+60d=1798599600, i.e. deletes - expires == 0 instead of 5184000s), from CreateApiKey and consequently from ListApiKeys.
Why: the two fields carry different meanings; a client that schedules rotation or cleanup off `deletes` believes the key record disappears the moment it expires and will purge/rotate 60 days early, and any assertion distinguishing expiry from deletion is wrong.
Fix: the CreateApiKey handler's expiry computation and the ApiKey state record (the same value must be served by ListApiKeys/UpdateApiKey reads).
Traces: appsync.api-key-default-expiry-seven-days.

=== Cause 4 of 4: no identity-based IAM default-deny gate — an unauthorized principal is never answered with AccessDeniedException ===
AWS: Smithy (appsync) models AccessDeniedException (403) on the sandbox operations EvaluateCode and EvaluateMappingTemplate as on the rest of the control plane. A principal whose IAM user has no policy granting appsync:EvaluateCode / appsync:EvaluateMappingTemplate is rejected with AccessDeniedException/403 (implicit default deny), and attaching then detaching an identity policy granting both actions flips the same signed call between success and AccessDeniedException.
Emulator: the call from a freshly created, zero-policy IAM user never yields AccessDeniedException; the check's bounded retry loop never sees that code and the check is killed at the 60s harness timeout. The emulator appears to apply no identity-based authorization decision to AppSync requests at all (the observed code on these sandbox ops is the scaffold `NotImplemented`, which also preempts the authorization gate).
Why: this is an overly-permissive boundary — least-privilege regressions cannot be detected against the emulator, and tests asserting a denial either hang or see the wrong code. Independent of implementing the sandbox operations, the authorization check must fire BEFORE operation dispatch, as it does in AWS.
Note: an open issue covers the EvaluateCode scaffold stub itself; it does not cover the missing authorization gate, which is what this cause is about.
Fix: the request authorization layer in front of AppSync operation dispatch (sigv4 principal -> IAM policy evaluation -> AccessDeniedException/403 with the implicit-deny shape), plus the sandbox op handlers.
Traces: appsync.access-denied-sandbox-eval.

## Evidence
Cause 1 — expected: `AccessDeniedException` (403) for a zero-policy principal, `NotFoundException` (404) for the permissioned principal on a never-created domain. Actual: `AssertionError: expected AccessDeniedException, got 'NotImplemented'` (appsync.get-domain-name-access-denied line 23; same assertion in appsync.list-domain-names-access-denied), and `Check timed out after 60.0s` for appsync.access-denied-domain-name / appsync.access-denied-get-domain-name, whose `_poll_until_denied` loop retries until the harness kills it. Underlying response: `(NotImplemented) ... operation is scaffolded but not implemented in this emulator`.

Cause 2 — expected: `BadRequestException` / HTTP 400. Actual: `AssertionError: expected BadRequestException, got ValidationException` (appsync.put-environment-variables-exceeds-size-limit-rejected line 41) and `botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the TagResource operation: 1 validation error detected: Value 'val-...' at 'tags.Unicode_Tag_...' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\s\w+-=\.:/@]*$` — the check's `except appsync.exceptions.BadRequestException` does not catch it.

Cause 3 — expected: `abs(deletes - (expires + 60*86400)) <= 3700`. Actual: `AssertionError: deletes=1793415600 not within 3700s of expires+60d=1798599600`, i.e. deletes == expires.

Cause 4 — expected: `AccessDeniedException` from EvaluateCode/EvaluateMappingTemplate for an IAM user with no attached policy (and success once a policy granting both actions is attached). Actual: that code never appears; `Check timed out after 60.0s`.

## Steps to Reproduce
1. Cause 1: call appsync GetDomainName(domainName="nonexistent.example.com") and ListDomainNames() — both answer NotImplemented; with credentials of a zero-policy IAM user they still answer NotImplemented instead of AccessDeniedException. Repeat inside a retry-until-AccessDeniedException loop to reproduce the hang.
2. Cause 2: CreateGraphqlApi, then PutGraphqlApiEnvironmentVariables with 60 entries (parameter validation disabled client-side) -> observe error code ValidationException instead of BadRequestException; likewise TagResource on the api ARN with a tag key/value containing non-ASCII characters.
3. Cause 3: CreateGraphqlApi(authenticationType=API_KEY) then CreateApiKey(apiId=...) with no `expires`; compare apiKey.deletes to apiKey.expires + 60 days (also via ListApiKeys).
4. Cause 4: create an IAM user with no attached policies, make an access key, and call EvaluateCode / EvaluateMappingTemplate with those credentials — no AccessDeniedException is ever returned.
