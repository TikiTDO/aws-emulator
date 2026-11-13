Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.guardrail-unknown-identifier-not-found bedrock-runtime.guardrail-checks-service-unavailable-distinct-error
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.guardrail-unknown-identifier-not-found|run#6|AssertionError|assert code == "ValidationException", f"got {code!r}"
Issue-Key: bedrock-runtime.guardrail-checks-service-unavailable-distinct-error|run#4|AssertionError|assert codes == ["ValidationException"] * 3, (
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# bedrock-runtime inline-guardrail error taxonomy: ApplyGuardrail and InvokeGuardrailChecks answer the unmodeled code NotImplemented where AWS answers 400 ValidationException

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyGuardrail

## Description
TWO CAUSES on the bedrock-runtime inline-guardrail surface. Both are client-invalid inputs that AWS refuses with a modeled 400 ValidationException, and that the emulator answers with the error code `NotImplemented` — a code that appears nowhere in either operation's modeled error set, so no SDK error class exists for it and every client takes its fall-through branch. Smithy model read for both: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json.

Related already-open issues, named here so a fixer can see the overlap and not re-file: `20261023T232022Z-bedrock-runtime-applyguardrail-stub.md` and `20261023T232022Z-bedrock-runtime-invokeguardrailchecks-stub.md` ask for the two operations to exist at all; neither states the error taxonomy the two causes below pin, which is what this issue adds. Both causes were replayed as `mismatch` (the check caught a ClientError and compared its CODE), not as a blocked-stub error, and neither check is marked ALREADY FILED or OWNED in the suite report.

Cause 1 of 2 — ApplyGuardrail with a well-formed but unresolvable guardrailIdentifier is not refused as a client validation error.
AWS: Smithy ApplyGuardrail -> POST /guardrail/{guardrailIdentifier}/version/{guardrailVersion}/apply; guardrailIdentifier is @required @httpLabel, @length max 2048, @pattern ^(|([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$. Modeled errors are AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(400), ServiceUnavailableException(503), ThrottlingException(429), ValidationException(400, doc: "The input fails to satisfy the constraints specified by Amazon Bedrock"). `NotImplemented` is NOT a modeled error of this operation. Recorded behavior only for the class choice: an ARN that is correctly shaped and owned by the caller but names a guardrail id that was never created is resolved during request validation and refused with ValidationException (400), not ResourceNotFoundException (404); recorded message example 'The guardrail identifier or version provided in the request does not exist.' (the message is an example — the requirement is the status and the code).
Emulator: the call is refused with code 'NotImplemented' ('operation ApplyGuardrail is scaffolded but not implemented in this emulator').
Why: a client cannot distinguish a bad guardrail reference from an unsupported API. botocore raises a bare ClientError instead of the modeled ValidationException, so `except client.exceptions.ValidationException` never fires; retry/alarm logic keyed on 4xx client-validation vs server/unsupported takes the wrong branch, and no negative-path test of guardrail identifier handling can be written against the emulator.
Fix: the ApplyGuardrail handler (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/apply_guardrail.rs) — resolve guardrailIdentifier/guardrailVersion against control-plane guardrail state during input validation and map an unresolvable one to ValidationException/400.
Traces: bedrock-runtime.guardrail-unknown-identifier-not-found.

Cause 2 of 2 — InvokeGuardrailChecks with an EMPTY `checks` object is not refused as a client validation error, and the refusal is non-deterministic in class.
AWS: Smithy InvokeGuardrailChecks -> POST /guardrail-checks/invoke; input members `messages` (@required, @length min 1) and `checks` (@required -> GuardrailChecksConfig). GuardrailChecksConfig's own doc states the rule: "The configuration for inline guardrail checks. Specify one or more check types to run against the messages." — so `checks={}`, with none of contentFilter/promptAttack/sensitiveInformation populated, violates a documented requirement and is refused with ValidationException (400, modeled on this operation), deterministically on every attempt and never as one of the operation's 5xx server classes (InternalServerException 500 / ServiceUnavailableException 503). `NotImplemented` is NOT a modeled error of this operation.
Emulator: three identical attempts all returned code 'NotImplemented' ('operation InvokeGuardrailChecks is scaffolded but not implemented in this emulator'); the well-formed positive control in the same check (contentFilter over HATE/VIOLENCE) is never reached, so no structured per-category `results`/`usage` is produced either.
Why: the ValidationException/success boundary of the inline-checks taxonomy collapses into one undifferentiated unsupported-API answer, so a client that branches on "my request was malformed" vs "the backend is faulting" cannot work, and a safety gate a client believes it is running is simply absent.
Fix: the InvokeGuardrailChecks handler (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/invoke_guardrail_checks.rs) — validate that at least one of contentFilter/promptAttack/sensitiveInformation is present and reject with ValidationException/400 before evaluation; serve the populated case with GuardrailChecksResults + GuardrailChecksUsageResults.
Traces: bedrock-runtime.guardrail-checks-service-unavailable-distinct-error.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1; bedrock-runtime 11/63 passed, so the service is up and answering (other checks on this service returned 200s and modeled 4xx).

Cause 1 — bedrock-runtime.guardrail-unknown-identifier-not-found:
  check_bedrock_runtime_guardrail_unknown_identifier_not_found.py line 39:
    assert code == "ValidationException", f"got {code!r}"
  AssertionError: got 'NotImplemented'
  Request: ApplyGuardrail(guardrailIdentifier='arn:aws:bedrock:<region>:<own-account>:guardrail/abcdef123456', guardrailVersion='DRAFT', source='INPUT', content=[{'text': {'text': 'Is this content safe?'}}]).
  Expected: 400 ValidationException (message example 'The guardrail identifier or version provided in the request does not exist.'); observed: code 'NotImplemented', message 'operation ApplyGuardrail is scaffolded but not implemented in this emulator'.

Cause 2 — bedrock-runtime.guardrail-checks-service-unavailable-distinct-error:
  check_bedrock_runtime_guardrail_checks_service_unavailable_distinct_error.py line 39:
    assert codes == ["ValidationException"] * 3, (...)
  AssertionError: an empty checks object must deterministically be ValidationException on every attempt, got ['NotImplemented', 'NotImplemented', 'NotImplemented']
  Request (x3): InvokeGuardrailChecks(messages=[{'role':'user','content':[{'text':'Hello, how has your day been so far?'}]}], checks={}).
  Expected: 400 ValidationException on every attempt; observed: code 'NotImplemented' on every attempt. The positive control that follows — checks={'contentFilter': {'categories': [{'category':'HATE'},{'category':'VIOLENCE'}]}} asserting 2 results, severityScore in [0,1], usage.contentFilter.textUnits > 0 — was never reached.

Model evidence: smithy_query query=operation operation=ApplyGuardrail,InvokeGuardrailChecks and query=shape shape=GuardrailChecksConfig,ValidationException on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json. ApplyGuardrail errors: AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(400), ServiceUnavailableException(503), ThrottlingException(429), ValidationException(400). InvokeGuardrailChecks errors: AccessDeniedException(403), InternalServerException(500), ServiceUnavailableException(503), ThrottlingException(429), ValidationException(400). Neither models `NotImplemented`. GuardrailChecksConfig doc: "Specify one or more check types to run against the messages."

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay the check ids in trace_ids.
3. Cause 1: call ApplyGuardrail with guardrailIdentifier='arn:aws:bedrock:<region>:<caller-account>:guardrail/abcdef123456' (correct shape, account-owned, never created), guardrailVersion='DRAFT', source='INPUT', content=[{'text':{'text':'Is this content safe?'}}]. Expected a 400 whose Error.Code is 'ValidationException'; observed Error.Code 'NotImplemented'. Repeat with a guardrail the caller just created and published as the positive control — it must succeed with required usage/action/outputs/assessments.
4. Cause 2: call InvokeGuardrailChecks three times with messages=[{'role':'user','content':[{'text':'Hello, how has your day been so far?'}]}] and checks={}. Expected Error.Code 'ValidationException' with HTTP 400 on all three attempts and never 'InternalServerException'/'ServiceUnavailableException'; observed Error.Code 'NotImplemented' on all three.
5. Still in Cause 2, call InvokeGuardrailChecks with checks={'contentFilter': {'categories': [{'category':'HATE'},{'category':'VIOLENCE'}]}}. Expected 200 with results.contentFilter.results of length 2 covering {HATE, VIOLENCE}, each severityScore in [0.0, 1.0], and usage.contentFilter.textUnits > 0.
