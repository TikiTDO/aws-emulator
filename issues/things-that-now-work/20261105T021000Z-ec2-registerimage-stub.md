Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.image-register-describe-deregister-lifecycle ec2.op-contract-cancel-image-launch-permission ec2.op-contract-delete-snapshot-in-use-rejected ec2.op-contract-deregister-image ec2.op-contract-detach-image-watermark ec2.op-contract-disable-image ec2.op-contract-disable-image-deprecation ec2.op-contract-disable-image-deregistration-protection ec2.op-contract-enableFastLaunch ec2.op-contract-enableImage ec2.op-contract-enableImageDeprecation ec2.op-contract-enableImageDeregistrationProtection ec2.op-contract-export-image ec2.op-contract-list-images-in-recycle-bin ec2.op-contract-register-image ec2.op-contract-replace-image-instance-type-specification ec2.op-contract-reset-image-attribute
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|RegisterImage|NotImplemented
Issue-Commit: a5885c3be3f986c1a94ab18540ac47db2748e09d

# Implement ec2 RegisterImage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a5885c3be3f986c1a94ab18540ac47db2748e09d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterImage

## Description
`RegisterImage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/register_image.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

17 recorded unit(s) fail on their `RegisterImage` call, so each one unblocks only when `RegisterImage` works: `ec2.image-register-describe-deregister-lifecycle`, `ec2.op-contract-cancel-image-launch-permission`, `ec2.op-contract-delete-snapshot-in-use-rejected`, `ec2.op-contract-deregister-image`, `ec2.op-contract-detach-image-watermark`, `ec2.op-contract-disable-image`, `ec2.op-contract-disable-image-deprecation`, `ec2.op-contract-disable-image-deregistration-protection`, `ec2.op-contract-enableFastLaunch`, `ec2.op-contract-enableImage`, `ec2.op-contract-enableImageDeprecation`, `ec2.op-contract-enableImageDeregistrationProtection`, `ec2.op-contract-export-image`, `ec2.op-contract-list-images-in-recycle-bin`, `ec2.op-contract-register-image`, `ec2.op-contract-replace-image-instance-type-specification`, `ec2.op-contract-reset-image-attribute`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/register_image.rs`. Replaying the corpus, the emulator refused the `RegisterImage` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterImage operation: operation RegisterImage is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call RegisterImage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
