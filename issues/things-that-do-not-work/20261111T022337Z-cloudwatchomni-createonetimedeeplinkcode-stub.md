Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.deep-link-code-rejects-foreign-redirect-and-long-ttl cloudwatchomni.deep-link-url-carries-its-one-time-code-and-domain
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.deep-link-code-rejects-foreign-redirect-and-long-ttl|run#4|AssertionError|assert (code, message) == ("ValidationException", "Invalid redirect URI: unrecognized domain"), (
Issue-Key: cloudwatchomni|CreateOneTimeDeepLinkCode|NotImplemented
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# Implement cloudwatchomni CreateOneTimeDeepLinkCode: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOneTimeDeepLinkCode

## Description
`CreateOneTimeDeepLinkCode` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_one_time_deep_link_code.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateOneTimeDeepLinkCode` call, so each one unblocks only when `CreateOneTimeDeepLinkCode` works: `cloudwatchomni.deep-link-code-rejects-foreign-redirect-and-long-ttl`, `cloudwatchomni.deep-link-url-carries-its-one-time-code-and-domain`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_one_time_deep_link_code.rs`. Replaying the corpus, the emulator refused the `CreateOneTimeDeepLinkCode` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cloudwatchomni_deep_link_code_rejects_foreign_redirect_and_long_ttl.py", line 77, in run
    assert (code, message) == ("ValidationException", "Invalid redirect URI: unrecognized domain"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected answer to a foreign redirect: NotImplemented: 'operation CreateOneTimeDeepLinkCode is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the cloudwatchomni emulator
Call CreateOneTimeDeepLinkCode and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
