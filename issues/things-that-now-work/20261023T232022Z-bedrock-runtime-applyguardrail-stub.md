Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.apply-guardrail-interventions-scope-drops-undetected-policy bedrock-runtime.apply-guardrail-word-filter-state-transition bedrock-runtime.converse-guardrail-intervention-stop-reason bedrock-runtime.guardrail-enforcement-consistent-across-apply-and-converse bedrock__bedrock-runtime.bedrock-runtime-apply-guardrail-without-the-grounding-source-rejected bedrock__bedrock-runtime.bedrock-runtime-guardrail-contextual-grounding-scores-the-answer-against-the-source
Issue-Key: bedrock-runtime|ApplyGuardrail|NotImplemented
Issue-Commit: cd7a0e788c437c5439a918b59f215cef2324a883

# Implement bedrock-runtime ApplyGuardrail: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cd7a0e788c437c5439a918b59f215cef2324a883`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyGuardrail

## Description
`ApplyGuardrail` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/apply_guardrail.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `ApplyGuardrail` call, so each one unblocks only when `ApplyGuardrail` works: `bedrock-runtime.apply-guardrail-interventions-scope-drops-undetected-policy`, `bedrock-runtime.apply-guardrail-word-filter-state-transition`, `bedrock-runtime.converse-guardrail-intervention-stop-reason`, `bedrock-runtime.guardrail-enforcement-consistent-across-apply-and-converse`, `bedrock__bedrock-runtime.bedrock-runtime-apply-guardrail-without-the-grounding-source-rejected`, `bedrock__bedrock-runtime.bedrock-runtime-guardrail-contextual-grounding-scores-the-answer-against-the-source`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/apply_guardrail.rs`. Replaying the corpus, the emulator refused the `ApplyGuardrail` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ApplyGuardrail operation: operation ApplyGuardrail is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-runtime emulator
Call ApplyGuardrail and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
