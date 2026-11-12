Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.delete-document-then-get-fails
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.delete-document-then-get-fails|run#19|AssertionError|assert update_message == "", (
Issue-Commit: e14db27dbe62f4f0b2a12e088b6709fa0d2b11d3

# ssm: UpdateDocument's InvalidDocument for a deleted document carries a message where AWS sends none

> _Note: this issue was identified against the code base at commit `e14db27dbe62f4f0b2a12e088b6709fa0d2b11d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDocument

## Description
One net-new root cause in SSM's UpdateDocument refusal path. The HTTP status (400) and the error code (InvalidDocument) are already correct; only the presence/content of the Message member diverges.

Cause 1 of 1: UpdateDocument on a name whose document has been deleted answers InvalidDocument with a message body, while AWS answers InvalidDocument with NO message at all.

AWS: 400 InvalidDocument (Smithy: UpdateDocument errors include InvalidDocument, errorKind client, httpError 400; InvalidDocument's single member Message targets String and is NOT required, so AWS is free to omit it — recorded behavior shows it omitted for this condition). For the SAME deleted name, GetDocument's InvalidDocument DOES carry a message, and its template is `Document with name <Name> does not exist.` — fixed text around one variable part, <Name>, taken verbatim from the request's Name member, with a trailing full stop. UpdateDocument's InvalidDocument for that same condition carries no Message (the SDK surfaces Error.Message as the empty string). So the requirement is asymmetric by operation: GetDocument messages the condition, UpdateDocument does not. The refusal must also leave the name free — a subsequent CreateDocument with the same Name succeeds.

Emulator: 400 InvalidDocument whose Message is `Document with name <Name> does not exist.` — i.e. the emulator reuses GetDocument's sentence (with the request's Name interpolated) for UpdateDocument, where AWS sends an empty/absent Message. Concretely the replay saw Message 'Document with name sim2real-behav-probe-doc-delete-<suffix> does not exist.' on UpdateDocument; the name is the per-run value filling the template, the defect is that any Message is emitted at all.

Why: a client that distinguishes AWS's two refusal shapes gets the wrong answer. Code that treats a non-empty Error.Message on UpdateDocument as a distinct failure class (e.g. a wrapper that only logs/propagates the message when AWS supplies one, or a test/assertion that pins AWS's exact envelope per operation) diverges, and anything that reconstructs AWS's response byte-for-byte (recorded-response comparison, contract tests, SDK-level envelope assertions) mismatches. The emulator also presents a false uniformity: it suggests every document-not-found refusal in SSM is messaged, which AWS does not do.

Fix: the UpdateDocument handler's document-existence precondition in the SSM service crate (services/service-com-amazonaws-ssm-amazonssm/src/ops/update_document.rs) — it must construct InvalidDocument with no Message member for a name that does not resolve, instead of sharing GetDocument's not-found message formatter. GetDocument's own message is correct and must not change (that assertion passes in the same check).

Note: the open issue 20261105T031534Z-ssm-2-net-new-refusal-message-causes-the.md covers UpdateDocument's DuplicateDocumentContent sentence, a different precondition in the same handler, and does not carry this check id or this cause; it does not cover this work.

Traces: ssm.delete-document-then-get-fails (listed under '### ssm' in the suite report).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout.

[mismatch] ssm.delete-document-then-get-fails
check_ssm_delete_document_then_get_fails.py line 104:
  assert update_message == "", (
      f"expected UpdateDocument's InvalidDocument on a deleted document to carry no message, got {update_message!r}")
AssertionError: expected UpdateDocument's InvalidDocument on a deleted document to carry no message, got 'Document with name sim2real-behav-probe-doc-delete-cbd28401d12b does not exist.'

Expected vs actual for UpdateDocument(Name=<deleted name>, Content=<same JSON 2.2 document>, DocumentVersion="$LATEST"):
  expected: HTTP 400, Error.Code = "InvalidDocument", Error.Message absent (empty string at the SDK)
  actual:   HTTP 400, Error.Code = "InvalidDocument", Error.Message = "Document with name <Name> does not exist."
Fixed text in the emulator's message: "Document with name " + " does not exist."; variable part: <Name>, echoed from the request. The whole message is the divergence — AWS emits none here.

Preceding assertions in the same check PASSED, which localises the defect:
  - CreateDocument then DescribeDocument reaching Status "Active" passed.
  - GetDocument(Name) resolving before the delete passed (positive control).
  - GetDocument after DeleteDocument returning Error.Code "InvalidDocument" passed, AND its Message assertion `message == f"Document with name {name} does not exist."` passed — so SSM's GetDocument not-found message is already right.
  - UpdateDocument's Error.Code assertion ("InvalidDocument") passed.
Only the UpdateDocument Message assertion failed.

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json
  - UpdateDocument: Name required, pattern ^[a-zA-Z0-9_\-.]{3,128}$; Content required; errors include InvalidDocument (errorKind client, httpError 400, awsQueryError code InvalidDocument), InvalidDocumentContent, InvalidDocumentVersion, DuplicateDocumentContent.
  - InvalidDocument: structure, errorKind client, httpError 400; single member Message -> String, NOT required (doc: "The SSM document doesn't exist or the document isn't available to the user. This exception can be issued by various API operations."). The model therefore permits an absent Message; which operations populate it is recorded behavior only.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (the behavioral suite does this).
2. ssm.CreateDocument(Name=<unique, 3-128 chars matching ^[a-zA-Z0-9_\-.]+$>, DocumentType="Command", Content=<minimal schemaVersion 2.2 document with one aws:runShellScript mainStep>). Poll DescribeDocument(Name) until Document.Status == "Active".
3. Positive control: ssm.GetDocument(Name) — expect it to resolve and echo Name.
4. ssm.DeleteDocument(Name=<that name>).
5. ssm.GetDocument(Name=<that name>) — expect 400 InvalidDocument with Message "Document with name <Name> does not exist." (this already matches; keep it matching).
6. ssm.UpdateDocument(Name=<that name>, Content=<the same document content>, DocumentVersion="$LATEST") — expect 400 with Error.Code "InvalidDocument" and NO Error.Message (empty string at the SDK). The emulator currently returns Message "Document with name <Name> does not exist.".
7. ssm.CreateDocument with the same Name again — expect it to succeed (the freed name is reusable) and GetDocument(Name) to resolve.
8. Re-replay exactly the check id in trace_ids (ssm.delete-document-then-get-fails) to confirm the cause is closed.
