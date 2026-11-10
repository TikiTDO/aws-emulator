Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: endusermessaging.tag-resource-rejects-aws-prefixed-key endusermessaging.tag-resource-rejects-duplicate-keys-in-one-request
Issue-Shape-Id: com.amazonaws.endusermessaging#EndUserMessaging
Issue-Key: endusermessaging.tag-resource-rejects-aws-prefixed-key|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Key: endusermessaging|TagResource|NotImplemented
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# Implement endusermessaging TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `endusermessaging.tag-resource-rejects-aws-prefixed-key`, `endusermessaging.tag-resource-rejects-duplicate-keys-in-one-request`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "run.py", line 365, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_endusermessaging_tag_resource_rejects_aws_prefixed_key.py", line 51, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: 'operation TagResource is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the endusermessaging emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
