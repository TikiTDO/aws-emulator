Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: endusermessaging.brand-profile-tag-resource-overwrites-and-untag-removes-only-named-keys endusermessaging.brand-profile-tags-are-unreachable-after-profile-delete endusermessaging.list-tags-for-resource-on-arn-of-wrong-resource-type-answers-not-found endusermessaging.list-tags-for-resource-rejects-arn-from-another-region
Issue-Shape-Id: com.amazonaws.endusermessaging#EndUserMessaging
Issue-Key: endusermessaging|ListTagsForResource|NotImplemented
Issue-Key: endusermessaging.list-tags-for-resource-on-arn-of-wrong-resource-type-answers-not-found|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}: {message!r}"
Issue-Key: endusermessaging.list-tags-for-resource-rejects-arn-from-another-region|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# Implement endusermessaging ListTagsForResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
`ListTagsForResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/list_tags_for_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `ListTagsForResource` call, so each one unblocks only when `ListTagsForResource` works: `endusermessaging.brand-profile-tag-resource-overwrites-and-untag-removes-only-named-keys`, `endusermessaging.brand-profile-tags-are-unreachable-after-profile-delete`, `endusermessaging.list-tags-for-resource-on-arn-of-wrong-resource-type-answers-not-found`, `endusermessaging.list-tags-for-resource-rejects-arn-from-another-region`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/list_tags_for_resource.rs`. Replaying the corpus, the emulator refused the `ListTagsForResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the endusermessaging emulator
Call ListTagsForResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
