Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-web.endpoint-latest-revision-mode-rejects-switching-onto-a-non-active-revision
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web.endpoint-latest-revision-mode-rejects-switching-onto-a-non-active-revision|run#25|AssertionError|assert code == "ConflictException", (
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# Implement lambda-web UpdateWebFunctionEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWebFunctionEndpoint

## Description
`UpdateWebFunctionEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/update_web_function_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateWebFunctionEndpoint` call, so each one unblocks only when `UpdateWebFunctionEndpoint` works: `lambda-web.endpoint-latest-revision-mode-rejects-switching-onto-a-non-active-revision`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/update_web_function_endpoint.rs`. Replaying the corpus, the emulator refused the `UpdateWebFunctionEndpoint` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_lambda_web_endpoint_latest_revision_mode_rejects_switching_onto_a_non_active_revision.py", line 276, in run
    assert code == "ConflictException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: switching to LatestRevision while the newest revision is Failed gave NotImplemented: 'operation UpdateWebFunctionEndpoint is scaffolded but not implemented in this emulator', expected ConflictException

## Steps to Reproduce
Boot the lambda-web emulator
Call UpdateWebFunctionEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
