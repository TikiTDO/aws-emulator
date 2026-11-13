Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.eks-anywhere-subscription-read-semantics eks.op-eks-anywhere-mutations-unknown-subscription
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|ListEksAnywhereSubscriptions|NotImplemented
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# Implement eks ListEksAnywhereSubscriptions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEksAnywhereSubscriptions

## Description
`ListEksAnywhereSubscriptions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/list_eks_anywhere_subscriptions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListEksAnywhereSubscriptions` call, so each one unblocks only when `ListEksAnywhereSubscriptions` works: `eks.eks-anywhere-subscription-read-semantics`, `eks.op-eks-anywhere-mutations-unknown-subscription`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/list_eks_anywhere_subscriptions.rs`. Replaying the corpus, the emulator refused the `ListEksAnywhereSubscriptions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListEksAnywhereSubscriptions operation: operation ListEksAnywhereSubscriptions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the eks emulator
Call ListEksAnywhereSubscriptions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
