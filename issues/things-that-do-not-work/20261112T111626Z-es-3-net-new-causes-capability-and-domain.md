Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-application-capability-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-application-capability-lifecycle opensearch-register-capability-deregister-conflict opensearch-osc-custom-endpoint-fix-input-mistakes opensearch-register-capability-nonexistent-application-error
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DeregisterCapability|4xx->2xx|$.message
Issue-Key: es|DeregisterCapability|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|DeregisterCapability|4xx->2xx|status
Issue-Key: es|UpdateDomainConfig|4xx->4xx|status
Issue-Key: es|RegisterCapability|4xx->4xx|$.message
Issue-Related: 20261102T091103Z-es-request-validation-diverges-refusal-text-on-domain.md
Issue-Commit: fbf28374bba60078ec7e79247fc3d719cafd12c8

# es: 3 net-new causes — capability and domain-config error paths return the wrong status, code and message

> _Note: this issue was identified against the code base at commit `fbf28374bba60078ec7e79247fc3d719cafd12c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeregisterCapability

## Description
Three net-new root causes on the `es` wire surface (OpenSearch/Elasticsearch control plane, endpoint prefix `es`). They share one fault: the refusal paths of the application-capability and domain-config operations answer with the wrong HTTP status, the wrong error class, or the wrong message text, so a client cannot tell a refusal apart from a success, or cannot match the refusal it is given. Each cause below is independent and must be closed separately.

=== Cause 1 of 3: DeregisterCapability does not refuse a deregistration the service cannot take, answering 200 instead of 409 ConflictException ===
AWS: DeregisterCapability (DELETE /2021-01-01/opensearch/application/{applicationId}/capability/deregister/{capabilityName}) models ConflictException among its errors, documented as "An error occurred because the client attempts to remove a resource that is currently in use" (Smithy: com.amazonaws.opensearch#DeregisterCapability errors = AccessDeniedException, ConflictException, DisabledOperationException, InternalException, ResourceNotFoundException, ValidationException; ConflictException httpError 409). Recorded behavior: when the application's AI capability is in a state whose removal the service will not start — the recorded fixtures register the capability and issue the deregistration against it in the same sequence, before the registration has settled — AWS refuses the call with HTTP 409, error code `ConflictException`, and the message `Conflict while deregister AI capability for application <applicationId>`. The fixed text is `Conflict while deregister AI capability for application ` and the only variable part is the `applicationId` from the request path (the recording's value is the probe's application id and is not part of the requirement). Hypothesis: the predicate is that the capability is not in a settled/active state (a registration still in flight), which is what both recordings have in common; the recording states the refusal, not the predicate.
Emulator: the call SUCCEEDS. It answers HTTP 200 with a `status` payload and no `message`/`x-amzn-errortype` at all (replay: status expected 409, got 200; `$.message` missing; `$headers.x-amzn-errortype` missing, expected `ConflictException`). The handler's only refusals are application-not-found and capability-not-found; it then unconditionally starts the removal.
Why: a client relying on AWS's behavior treats a 200 from DeregisterCapability as proof that the service accepted the removal, and treats the 409 as the signal to back off and retry once the capability settles. Against the emulator the conflict never occurs, so retry/backoff code and negative-path tests around capability teardown never execute, and the capability is torn down from a state AWS refuses to tear it down from — leaving the emulator's application in a state the real service cannot reach.
Fix: the DeregisterCapability handler — a conflict gate in its handle body, between the capability lookup and `begin_deregistration`, raising the modeled ConflictException (HTTP 409) with the message above when the held capability's state does not permit a removal to start, and leaving the stored capability untouched in that case.
Traces: opensearch-application-capability-lifecycle#task#3 (representative), opensearch-register-capability-deregister-conflict#task#3. Handler: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/deregister_capability.rs (handle, lines 166-211: only ResourceNotFoundException is raised, at lines 183-199, and line 204 begins the deregistration unconditionally).

=== Cause 2 of 3: UpdateDomainConfig never validates DomainEndpointOptions.CustomEndpoint and resolves the domain first, so a malformed custom endpoint is answered 404 ResourceNotFoundException instead of 400 ValidationException ===
AWS: UpdateDomainConfig (POST /2021-01-01/opensearch/domain/{DomainName}/config) takes DomainEndpointOptions.CustomEndpoint targeting DomainNameFqdn, which is constrained @length min 1 max 255 and @pattern ^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$ — a bare fully-qualified domain name, so a value carrying a scheme (`https://...`) cannot satisfy it; ValidationException (httpError 400) is modeled on the operation (Smithy: com.amazonaws.opensearch#UpdateDomainConfig, DomainEndpointOptions.CustomEndpoint, DomainNameFqdn). Recorded behavior: AWS enforces the modeled constraint as REQUEST validation, ahead of resolving the named domain, and answers HTTP 400 with the service's standard constraint-violation message `1 validation error detected: Value '<value>' at 'domainEndpointOptions.customEndpoint' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$`. The fixed text is `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: ` and the pattern itself as the model writes it, ANCHORS INCLUDED; the variable parts are the rejected value copied from the request and the member path `domainEndpointOptions.customEndpoint`.
Emulator: the pattern is never checked. The handler resolves the domain first and answers HTTP 404 with `x-amzn-errortype: ResourceNotFoundException` and the message `Domain not found: <domainName>` (replay: status expected 400, got 404; `$headers.x-amzn-errortype` got `ResourceNotFoundException`), so the request's malformed member is never reported and the client is told the wrong thing is wrong. No custom-endpoint value check exists anywhere in the refusal chain.
Why: a client relying on AWS's behavior reads a 400 ValidationException naming `domainEndpointOptions.customEndpoint` as "fix this field of my request" and a 404/ResourceNotFoundException as "this domain does not exist". The emulator swaps the two: tooling that walks a malformed-input error back to the offending member gets nothing to point at, retry-on-not-found logic fires on an input error that will never succeed, and a client that sends a scheme-prefixed endpoint (the natural mistake, e.g. pasting a load-balancer URL) is accepted or misdirected instead of being corrected.
Fix: UpdateDomainConfig's input validation — enforce the modeled DomainNameFqdn length/pattern on `DomainEndpointOptions.CustomEndpoint` as request validation, raising the modeled ValidationException (HTTP 400) with the standard constraint message above, and run it BEFORE the domain is resolved so an input error outranks the not-found refusal.
Traces: opensearch-osc-custom-endpoint-fix-input-mistakes#task#1 (cluster representative), opensearch-osc-custom-endpoint-fix-input-mistakes#task#2. Handler: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_domain_config.rs (handle: the domain is resolved at lines 489-496 and raises `domain_not_found`; the refusal chain that follows, lines 522-671, checks VPC placement, master user, security prerequisites, instance type, EBS, warm/cold and off-peak options, but never `domain_endpoint_options.custom_endpoint`).

=== Cause 3 of 3: RegisterCapability's missing-application refusal uses the wrong message wording ===
AWS: RegisterCapability (POST /2021-01-01/opensearch/application/{applicationId}/capability/register) models ResourceNotFoundException, documented as "An exception for accessing or deleting a resource that doesn't exist" (Smithy: com.amazonaws.opensearch#RegisterCapability errors include ResourceNotFoundException). Recorded behavior: registering a capability on an application id that names no application is refused with error code `ResourceNotFoundException` and the message `Application <applicationId> does not exist` — fixed text `Application ` and ` does not exist`, with the `applicationId` from the request path as the only variable part. The recorded status for this refusal is HTTP 404 (the emulator already matches the status).
Emulator: the same refusal is worded `Application <applicationId> not found` (replay: `$.message` value_mismatch, expected `Application app000nonexistent000 does not exist`, got `Application app000nonexistent000 not found`). Status and error code match; only the message text diverges, because the handler reuses a generic `application_not_found_named` helper whose wording is not RegisterCapability's.
Why: an error message is part of the contract. A client or test that matches on the service's wording — log scrapers, assertion suites, SDK retry/classification wrappers keyed on message text — fails against the emulator for a refusal it otherwise got right, and a user reading the emulator's message is being shown text AWS never emits.
Fix: the RegisterCapability handler's not-found refusal message — give the application-not-found path for this operation AWS's wording (`Application <applicationId> does not exist`) rather than the shared `not found` phrasing. The helper is shared by other application sub-resource operations whose recorded wording is different (GetCapability records `Capability <name> not found for application <id>`), so change RegisterCapability's message rather than the helper's wording for every caller.
Traces: opensearch-register-capability-nonexistent-application-error#task#1. Handler: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/register_capability.rs (handle, lines 206-213: the application lookup raises `ResourceNotFoundException(application_not_found_named(&input.application_id))`).

Related but NOT covering: the open issue 20261112T105051Z-opensearch-registercapability-accepts-any-well-formed-capabilityname-instead.md is about RegisterCapability not restricting `capabilityName` to `ai-capability`; it says nothing about the three causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (98 fixtures, 486 steps, 308 passed, 178 mismatched, score 63.4%). All three clusters below are reported with empty `stub_path` and empty `owned_by` — they are net-new.

Cause 1 — DeregisterCapability, fixture opensearch-application-capability-lifecycle step task#3 (cluster also covers opensearch-register-capability-deregister-conflict#task#3):
  Expected (AWS recording): HTTP 409, x-amzn-errortype = ConflictException,
    message = "Conflict while deregister AI capability for application <applicationId>"
  Actual (emulator): HTTP 200, no message, no x-amzn-errortype header.
  Replay detail: status: expected 409, got 200 / $.message: missing / $headers.x-amzn-errortype: missing (expected 'ConflictException')
  Smithy: DeregisterCapability errors include ConflictException (httpError 409), doc "An error occurred because the client attempts to remove a resource that is currently in use."

Cause 2 — UpdateDomainConfig, fixture opensearch-osc-custom-endpoint-fix-input-mistakes steps task#1 and task#2:
  Expected (AWS recording): HTTP 400, a ValidationException whose message is
    "1 validation error detected: Value 'https://<lb-host>.us-west-2.elb.amazonaws.com' at 'domainEndpointOptions.customEndpoint' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$"
  Actual (emulator): HTTP 404, x-amzn-errortype = ResourceNotFoundException,
    message = "Domain not found: <domainName>"
  Replay detail: status: expected 400, got 404 / $.message: value_mismatch / $headers.x-amzn-errortype: shape_mismatch (got 'ResourceNotFoundException')
  (The domain name inside each message is the fixture's per-run probe name and is not part of the requirement; the requirement is the status, the error class and the message template.)
  Smithy: DomainEndpointOptions.CustomEndpoint -> DomainNameFqdn, length 1..255, pattern ^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$; UpdateDomainConfig errors include ValidationException (httpError 400).

Cause 3 — RegisterCapability, fixture opensearch-register-capability-nonexistent-application-error step task#1:
  Expected (AWS recording): HTTP 404, message = "Application app000nonexistent000 does not exist"
  Actual (emulator): HTTP 404, message = "Application app000nonexistent000 not found"
  Replay detail: $.message: value_mismatch (expected 'Application app000nonexistent000 does not exist', got 'Application app000nonexistent000 not found')
  Smithy: RegisterCapability errors include ResourceNotFoundException, doc "An exception for accessing or deleting a resource that doesn't exist."

## Steps to Reproduce
Replay the whole corpus: replay_wire_corpus() against an emulator built from this checkout, then look at the three clusters below (each is reported with stub_path="" and owned_by=[]).

Cause 1 (DeregisterCapability conflict) — replay fixture opensearch-application-capability-lifecycle, step task#3 (and opensearch-register-capability-deregister-conflict, step task#3). By hand:
  1. CreateApplication(name=<unique lowercase name>) on the emulator's es/opensearch endpoint; keep the returned id as <app>.
  2. RegisterCapability(applicationId=<app>, capabilityName="ai-capability", capabilityConfig={"aiConfig": {}}).
  3. Immediately DeregisterCapability(applicationId=<app>, capabilityName="ai-capability"), before the registration settles.
     Expected: HTTP 409 ConflictException, message "Conflict while deregister AI capability for application <app>", and the capability still stored unchanged.
     Observed: HTTP 200 with a status payload; the removal is started.

Cause 2 (UpdateDomainConfig custom endpoint) — replay fixture opensearch-osc-custom-endpoint-fix-input-mistakes, steps task#1 and task#2. By hand:
  1. UpdateDomainConfig(DomainName=<any syntactically valid domain name>, DomainEndpointOptions={CustomEndpointEnabled: true, CustomEndpoint: "https://internal-example-1234567890.us-west-2.elb.amazonaws.com"}) — a value with a scheme, which cannot match DomainNameFqdn's pattern.
     Expected: HTTP 400 ValidationException, message "1 validation error detected: Value '<the value sent>' at 'domainEndpointOptions.customEndpoint' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$" — regardless of whether the named domain exists.
     Observed: HTTP 404 ResourceNotFoundException "Domain not found: <domainName>".
  2. Repeat with an existing domain to confirm the pattern is still not enforced (the malformed endpoint is accepted instead of refused).

Cause 3 (RegisterCapability wording) — replay fixture opensearch-register-capability-nonexistent-application-error, step task#1. By hand:
  1. RegisterCapability(applicationId="app000nonexistent000", capabilityName="ai-capability", capabilityConfig={"aiConfig": {}}) against an application id that does not exist.
     Expected: ResourceNotFoundException with message "Application app000nonexistent000 does not exist".
     Observed: ResourceNotFoundException with message "Application app000nonexistent000 not found".
