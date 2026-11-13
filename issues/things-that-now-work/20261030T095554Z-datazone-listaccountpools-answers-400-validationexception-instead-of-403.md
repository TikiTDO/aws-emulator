Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-domain-scoped-denied-sweep-1
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-domain-scoped-denied-sweep-1
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|ListAccountPools|4xx->4xx|$.message
Issue-Key: datazone|ListAccountPools|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|ListAccountPools|4xx->4xx|status
Issue-Commit: 0e1c1b5eee37df9cd7b5f3886cad9124e3f02059

# datazone ListAccountPools answers 400 ValidationException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `0e1c1b5eee37df9cd7b5f3886cad9124e3f02059`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccountPools

## Description
ONE cause. Cause 1 of 1: ListAccountPools on a domainIdentifier that is syntactically valid but not resolvable/permitted for the caller.

AWS: Smithy (services/service-com-amazonaws-datazone-datazone) binds ListAccountPools to GET /v2/domains/{domainIdentifier}/account-pools with domainIdentifier @required @httpLabel @pattern "^dzd[-_][a-zA-Z0-9_-]{1,36}$", and its modelled errors include AccessDeniedException (errorKind client, httpError 403) and ValidationException (errorKind client, httpError 400). A domainIdentifier matching that pattern violates no modelled input constraint, so an identifier the caller may not use is decided by the authorization stage, not by validation. The recorded response is HTTP 403 with header x-amzn-errortype: AccessDeniedException and message `User is not permitted to perform operation: ListAccountPools`. In that message the fixed text is "User is not permitted to perform operation: " and the only variable part is the operation name taken from the request (here ListAccountPools); no request value, id or timestamp appears in it.

Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException and the hand-written message 'domainIdentifier does not name a domain in this account' — a sentence AWS never emits, classifying an authorization outcome as an input-constraint failure.

Why: 400/ValidationException is a non-retryable client-input error in every AWS SDK, so a caller concludes its own request was malformed and "fixes" a perfectly legal domain identifier, while the real condition (no permission on that domain) is never surfaced. Code that branches on AccessDeniedException — to re-authenticate, assume another role, or report a permissions problem — never fires, and the 403-vs-400 split that drives retry and alarm policy is inverted.

Fix: datazone's request pipeline for ListAccountPools — once domainIdentifier satisfies its modelled pattern, an unresolvable or unauthorized domain must leave the validation stage and be answered by the authorization stage as 403 AccessDeniedException with x-amzn-errortype AccessDeniedException and the "User is not permitted to perform operation: <Operation>" message.

Note: the open issue 20261029T172524Z-datazone-domain-scoped-operations-answer-400-validationexception-instead.md describes the same shared fault for six OTHER operations (ListProjects, ListDomainUnitsForParent, ListEntityOwners, ListPolicyGrants, CreateDomainUnit, CreateAssetType). It does not name ListAccountPools and does not carry this fixture step, so this cause is filed separately; a fixer should expect the same code path.

Traces: datazone-domain-scoped-denied-sweep-1#task#1.

## Evidence
Replay of simparity-wire-aws-v1, divergence_key service=datazone|ListAccountPools|status=403->400|keys=$.message,$headers.x-amzn-errortype,status

Expected (recorded AWS response): status 403; $headers.x-amzn-errortype shaped as AccessDeniedException; $.message = 'User is not permitted to perform operation: ListAccountPools'.
Actual (emulator): status 400; $headers.x-amzn-errortype = 'ValidationException'; $.message = 'domainIdentifier does not name a domain in this account'.

The supplied domainIdentifier matched the modelled pattern ^dzd[-_][a-zA-Z0-9_-]{1,36}$, so no modelled input constraint was violated and ValidationException is the wrong error class. Fixture datazone-domain-scoped-denied-sweep-1, step task#1 (affected: datazone-domain-scoped-denied-sweep-1#task#1).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Inspect fixture datazone-domain-scoped-denied-sweep-1, step task#1: GET /v2/domains/{domainIdentifier}/account-pools (ListAccountPools) with a pattern-valid domainIdentifier the caller is not permitted on.
3. Observe the emulator returns 400 with x-amzn-errortype ValidationException and message 'domainIdentifier does not name a domain in this account'.
4. Expected: 403 with x-amzn-errortype AccessDeniedException and message 'User is not permitted to perform operation: ListAccountPools'.
