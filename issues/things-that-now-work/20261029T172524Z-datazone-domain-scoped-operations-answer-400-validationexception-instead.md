Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-domain-scoped-denied-sweep-2
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-domain-scoped-denied-sweep-2 datazone-domain-scoped-denied-sweep-1 datazone-domain-scoped-denied-sweep-4
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|ListProjects|4xx->4xx|$.message
Issue-Key: datazone|ListProjects|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|ListProjects|4xx->4xx|status
Issue-Key: datazone|ListDomainUnitsForParent|4xx->4xx|$.message
Issue-Key: datazone|ListDomainUnitsForParent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|ListDomainUnitsForParent|4xx->4xx|status
Issue-Key: datazone|ListEntityOwners|4xx->4xx|$.message
Issue-Key: datazone|ListEntityOwners|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|ListEntityOwners|4xx->4xx|status
Issue-Key: datazone|ListPolicyGrants|4xx->4xx|$.message
Issue-Key: datazone|ListPolicyGrants|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|ListPolicyGrants|4xx->4xx|status
Issue-Key: datazone|CreateDomainUnit|4xx->4xx|$.message
Issue-Key: datazone|CreateDomainUnit|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|CreateDomainUnit|4xx->4xx|status
Issue-Key: datazone|CreateAssetType|4xx->4xx|$.message
Issue-Key: datazone|CreateAssetType|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|CreateAssetType|4xx->4xx|status
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# datazone domain-scoped operations answer 400 ValidationException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListProjects

## Description
SIX causes, one shared fault: these operations receive a domainIdentifier that is syntactically valid for the model's pattern but not resolvable by the caller, and the emulator reports it as an INPUT-CONSTRAINT failure (400 ValidationException) when datazone reports it as an AUTHORIZATION failure (403 AccessDeniedException). A domainIdentifier matching ^dzd[-_][a-zA-Z0-9_-]{1,36}$ satisfies every modelled constraint, so ValidationException is the wrong class for it: resolvability is decided after validation, by the authorization pre-check. Fix all six before submitting.

Shared AWS behavior (Smithy): each of these operations binds AccessDeniedException (httpError 403, doc "You do not have sufficient access to perform this action.", message @required) and ValidationException (httpError 400, doc "The input fails to satisfy the constraints specified by the Amazon Web Services service."); domainIdentifier is @required, @httpLabel, @pattern "^dzd[-_][a-zA-Z0-9_-]{1,36}$". The recorded denial is 403, x-amzn-errortype AccessDeniedException, message `User is not permitted to perform operation: <OperationName>`. Shared emulator behavior: 400, x-amzn-errortype ValidationException, with a hand-written sentence about domainIdentifier. Shared Why: a 400 is a non-retryable client-input error in every SDK, so a caller concludes its request was malformed and "fixes" a perfectly valid identifier, never seeing that it lacks permission; error-type branching on AccessDeniedException never fires. Shared Fix: datazone's request pipeline — once a domainIdentifier satisfies its modelled pattern, an unresolvable/unauthorized domain must leave the validation stage and be answered by the authorization stage as AccessDeniedException.

Cause 1 of 6: ListProjects. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: ListProjects`. Emulator: 400 ValidationException, message 'domainIdentifier <domainIdentifier> does not identify a domain'. Traces: datazone-domain-scoped-denied-sweep-2#task#4.

Cause 2 of 6: ListDomainUnitsForParent. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: ListDomainUnitsForParent`. Emulator: 400 ValidationException, message 'domainIdentifier does not name a domain in this account'. Traces: datazone-domain-scoped-denied-sweep-1#task#9.

Cause 3 of 6: ListEntityOwners. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: ListEntityOwners`. Emulator: 400 ValidationException, message 'domainIdentifier does not name a domain in this account'. Traces: datazone-domain-scoped-denied-sweep-4#task#1.

Cause 4 of 6: ListPolicyGrants. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: ListPolicyGrants`. Emulator: 400 ValidationException, message 'domainIdentifier <domainIdentifier> does not identify a domain'. Traces: datazone-domain-scoped-denied-sweep-4#task#2.

Cause 5 of 6: CreateDomainUnit. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: CreateDomainUnit`. Emulator: 400 ValidationException, message 'domainIdentifier does not name a domain in this account'. Traces: datazone-domain-scoped-denied-sweep-4#task#8.

Cause 6 of 6: CreateAssetType. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: CreateAssetType`. Emulator: 400 ValidationException, message 'domainIdentifier <domainIdentifier> does not identify a domain'. Traces: datazone-domain-scoped-denied-sweep-4#task#12.

## Evidence
Replay report, expected -> actual:
1. ListProjects 403 -> 400; $.message expected 'User is not permitted to perform operation: ListProjects', got 'domainIdentifier dzd_bogus00000000001 does not identify a domain'; x-amzn-errortype expected 'AccessDeniedException', got 'ValidationException'.
2. ListDomainUnitsForParent 403 -> 400; expected 'User is not permitted to perform operation: ListDomainUnitsForParent', got 'domainIdentifier does not name a domain in this account'; errortype got 'ValidationException'.
3. ListEntityOwners 403 -> 400; expected 'User is not permitted to perform operation: ListEntityOwners', got 'domainIdentifier does not name a domain in this account'; errortype got 'ValidationException'.
4. ListPolicyGrants 403 -> 400; expected 'User is not permitted to perform operation: ListPolicyGrants', got 'domainIdentifier dzd_bogus00000000001 does not identify a domain'; errortype got 'ValidationException'.
5. CreateDomainUnit 403 -> 400; expected 'User is not permitted to perform operation: CreateDomainUnit', got 'domainIdentifier does not name a domain in this account'; errortype got 'ValidationException'.
6. CreateAssetType 403 -> 400; expected 'User is not permitted to perform operation: CreateAssetType', got 'domainIdentifier dzd_bogus00000000001 does not identify a domain'; errortype got 'ValidationException'.
In every case the supplied domainIdentifier matched the modelled pattern ^dzd[-_][a-zA-Z0-9_-]{1,36}$, so no modelled input constraint was violated.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 and inspect:
- datazone-domain-scoped-denied-sweep-2: task#4 ListProjects.
- datazone-domain-scoped-denied-sweep-1: task#9 ListDomainUnitsForParent.
- datazone-domain-scoped-denied-sweep-4: task#1 ListEntityOwners, task#2 ListPolicyGrants, task#8 CreateDomainUnit, task#12 CreateAssetType.
Each sends a pattern-valid domainIdentifier the caller is not permitted on and must answer 403 with x-amzn-errortype AccessDeniedException and the "User is not permitted to perform operation: <Operation>" message, not 400 ValidationException.
