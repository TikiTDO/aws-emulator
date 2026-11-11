Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.create-then-list-request-immediately-visible cloudcontrol.list-resource-requests-and-get-status-agree-on-one-request cloudcontrol.list-resource-requests-chronological-ordering cloudcontrol.list-resource-requests-honors-status-filter cloudcontrol.list-resource-requests-invalid-next-token-rejected cloudcontrol.list-resource-requests-operation-type-filter-preserves-order cloudcontrol.list-resource-requests-ordered-by-requested-time-desc cloudcontrol.list-resource-requests-pagination-no-duplicates cloudcontrol.list-resource-requests-reflects-create cloudcontrol.list-resource-requests-time-window-filter cloudcontrol.request-visible-in-list-before-completion cloudcontrol.resource-request-status-terminal-after-ttl-expiry cloudcontrol.server-side-progress-events-tracked-per-request-dup-removed
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol|ListResourceRequests|NotImplemented
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# Implement cloudcontrol ListResourceRequests: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceRequests

## Description
`ListResourceRequests` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/list_resource_requests.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `ListResourceRequests` call, so each one unblocks only when `ListResourceRequests` works: `cloudcontrol.create-then-list-request-immediately-visible`, `cloudcontrol.list-resource-requests-and-get-status-agree-on-one-request`, `cloudcontrol.list-resource-requests-chronological-ordering`, `cloudcontrol.list-resource-requests-honors-status-filter`, `cloudcontrol.list-resource-requests-invalid-next-token-rejected`, `cloudcontrol.list-resource-requests-operation-type-filter-preserves-order`, `cloudcontrol.list-resource-requests-ordered-by-requested-time-desc`, `cloudcontrol.list-resource-requests-pagination-no-duplicates`, `cloudcontrol.list-resource-requests-reflects-create`, `cloudcontrol.list-resource-requests-time-window-filter`, `cloudcontrol.request-visible-in-list-before-completion`, `cloudcontrol.resource-request-status-terminal-after-ttl-expiry`, `cloudcontrol.server-side-progress-events-tracked-per-request-dup-removed`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/list_resource_requests.rs`. Replaying the corpus, the emulator refused the `ListResourceRequests` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListResourceRequests operation: operation ListResourceRequests is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudcontrol emulator
Call ListResourceRequests and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
