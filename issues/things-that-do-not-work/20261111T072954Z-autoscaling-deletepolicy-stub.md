Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.delete-policy-nonexistent-noop autoscaling.op-contract-deletePolicy autoscaling.scaling-policy-delete-removes-policy
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: auto-scaling|DeletePolicy|NotImplemented
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# Implement autoscaling DeletePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePolicy

## Description
`DeletePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeletePolicy` call, so each one unblocks only when `DeletePolicy` works: `autoscaling.delete-policy-nonexistent-noop`, `autoscaling.op-contract-deletePolicy`, `autoscaling.scaling-policy-delete-removes-policy`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_policy.rs`. Replaying the corpus, the emulator refused the `DeletePolicy` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_autoscaling_delete_policy_nonexistent_noop.py", line 44, in run
    assert code == "ValidationError", f"expected ValidationError, got {code}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationError, got NotImplemented

## Steps to Reproduce
Boot the autoscaling emulator
Call DeletePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
