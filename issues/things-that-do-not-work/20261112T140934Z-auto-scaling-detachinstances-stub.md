Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.cli-detach-instance-from-asg-decrements-target-group
Issue-Key: autoscaling__ec2__elbv2.cli-detach-instance-from-asg-decrements-target-group|_run_shell#16|AssertionError|raise AssertionError(
Issue-Commit: 59f14ace25ed5753b46a36483d50334ce01f6026

# Implement auto-scaling DetachInstances: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `59f14ace25ed5753b46a36483d50334ce01f6026`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachInstances

## Description
`DetachInstances` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/detach_instances.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DetachInstances` call, so each one unblocks only when `DetachInstances` works: `autoscaling__ec2__elbv2.cli-detach-instance-from-asg-decrements-target-group`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/detach_instances.rs`. Replaying the corpus, the emulator refused the `DetachInstances` call: File "check_autoscaling__ec2__elbv2_cli_detach_instance_from_asg_decrements_target_group.py", line 104, in _run_aws
    proc = _run_shell(
           ^^^^^^^^^^^
  File "check_autoscaling__ec2__elbv2_cli_detach_instance_from_asg_decrements_target_group.py", line 96, in _run_shell
    raise AssertionError(
AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 'autoscaling', 'detach-instances', '--auto-scaling-group-name', 'sim2real-detach-7cb3389f16e5-asg', '--instance-ids', 'i-00000000000000001', '--should-decrement-desired-capacity']
  stderr: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the DetachInstances operation: operation DetachInstances is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the auto-scaling emulator
Call DetachInstances and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
