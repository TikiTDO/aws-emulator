Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chatbot.create-custom-action-idempotent-with-client-token chatbot.custom-action-list-contains-created chatbot.custom-action-list-reflects-create-and-delete chatbot.custom-action-pagination-returns-all-created chatbot.list-custom-actions-maxresults-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|ListCustomActions|NotImplemented
Issue-Commit: baf30d4927ba43bb30691a1e04213021d426cca6

# Implement chatbot ListCustomActions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `baf30d4927ba43bb30691a1e04213021d426cca6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCustomActions

## Description
`ListCustomActions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/list_custom_actions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `ListCustomActions` call, so each one unblocks only when `ListCustomActions` works: `chatbot.create-custom-action-idempotent-with-client-token`, `chatbot.custom-action-list-contains-created`, `chatbot.custom-action-list-reflects-create-and-delete`, `chatbot.custom-action-pagination-returns-all-created`, `chatbot.list-custom-actions-maxresults-out-of-range-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/list_custom_actions.rs`. Replaying the corpus, the emulator refused the `ListCustomActions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListCustomActions operation: operation ListCustomActions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the chatbot emulator
Call ListCustomActions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
