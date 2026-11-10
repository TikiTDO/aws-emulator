Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediatailor.delete-channel-is-idempotent-and-removes-resource-for-dependent-ops mediatailor.tag-apis-reject-deleted-channel-arn-with-distinct-message
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor.delete-channel-is-idempotent-and-removes-resource-for-dependent-ops|run#11|AssertionError|assert code == "NotFoundException", f"GetChannelSchedule on deleted channel gave {code}: {message!r}"
Issue-Key: mediatailor.tag-apis-reject-deleted-channel-arn-with-distinct-message|run#16|AssertionError|assert code == "NotFoundException", f"GetChannelSchedule on deleted channel gave {code}: {message!r}"
Issue-Commit: 04709e89cacddd72127882c68b510e21949bd7d1

# Implement mediatailor GetChannelSchedule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `04709e89cacddd72127882c68b510e21949bd7d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetChannelSchedule

## Description
`GetChannelSchedule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-mediatailor-mediatailor/src/ops/get_channel_schedule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetChannelSchedule` call, so each one unblocks only when `GetChannelSchedule` works: `mediatailor.delete-channel-is-idempotent-and-removes-resource-for-dependent-ops`, `mediatailor.tag-apis-reject-deleted-channel-arn-with-distinct-message`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-mediatailor-mediatailor/src/ops/get_channel_schedule.rs`. Replaying the corpus, the emulator refused the `GetChannelSchedule` call: File "run.py", line 365, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_mediatailor_delete_channel_is_idempotent_and_removes_resource_for_dependent_ops.py", line 48, in run
    assert code == "NotFoundException", f"GetChannelSchedule on deleted channel gave {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: GetChannelSchedule on deleted channel gave NotImplemented: 'operation GetChannelSchedule is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the mediatailor emulator
Call GetChannelSchedule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
