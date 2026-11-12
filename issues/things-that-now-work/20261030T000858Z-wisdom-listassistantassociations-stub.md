Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wisdom.assistant-association-client-token-idempotent-conflict-rejected wisdom.assistant-association-delete-transitions-to-not-found wisdom.assistant-association-multiple-distinct-knowledge-bases-allowed wisdom.assistant-knowledge-base-content-session-end-to-end-workflow wisdom.concurrent-assistant-association-race-single-winner
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|ListAssistantAssociations|NotImplemented
Issue-Commit: de414c514180eddfb96a534a3f6304a808cc952a

# Implement wisdom ListAssistantAssociations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `de414c514180eddfb96a534a3f6304a808cc952a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAssistantAssociations

## Description
`ListAssistantAssociations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wisdom-wisdomservice/src/ops/list_assistant_associations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `ListAssistantAssociations` call, so each one unblocks only when `ListAssistantAssociations` works: `wisdom.assistant-association-client-token-idempotent-conflict-rejected`, `wisdom.assistant-association-delete-transitions-to-not-found`, `wisdom.assistant-association-multiple-distinct-knowledge-bases-allowed`, `wisdom.assistant-knowledge-base-content-session-end-to-end-workflow`, `wisdom.concurrent-assistant-association-race-single-winner`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wisdom-wisdomservice/src/ops/list_assistant_associations.rs`. Replaying the corpus, the emulator refused the `ListAssistantAssociations` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListAssistantAssociations operation: operation ListAssistantAssociations is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the wisdom emulator
Call ListAssistantAssociations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
