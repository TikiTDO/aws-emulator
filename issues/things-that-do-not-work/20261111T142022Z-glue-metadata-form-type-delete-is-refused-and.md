Issue-Source: trace-sweep-divergence
Issue-Trace: glue-formtype-delete-then-not-found
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-formtype-delete-then-not-found glue-not-found-error-paths
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|DeleteFormType|2xx->4xx|$
Issue-Key: glue|DeleteFormType|2xx->4xx|status
Issue-Key: glue|GetFormType|4xx->2xx|$.Message
Issue-Key: glue|GetFormType|4xx->2xx|$.__type
Issue-Key: glue|GetFormType|4xx->2xx|status
Issue-Key: glue|PutAttachment|4xx->4xx|$.Message
Issue-Commit: 18f142fa95962db3f809978042802814a1944299

# Glue metadata form-type delete is refused and asset lookups misorder validation

> _Note: this issue was identified against the code base at commit `18f142fa95962db3f809978042802814a1944299`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFormType

## Description
This bundle carries 3 net-new root causes on Glue's metadata form-type / asset surface. Each cause is numbered below with its own fixture and step; all three must be closed before this issue is resolved.

Cause 1 of 3: DeleteFormType refuses a deletable form type with ConflictException, claiming a dependency that does not block deletion.
AWS: Smithy: DeleteFormType input Identifier (FormTypeId, required, @length 1..256), output is an EMPTY structure, errors AccessDeniedException, ConcurrentModificationException, ConflictException, InternalServiceException, InvalidInputException, ThrottlingException (all client-kind, httpError 400). The recording shows AWS answering 200 with an empty JSON body ({}) for a form type that is still referenced by the asset type it was attached to — the reference recorded here is not a deletion-blocking dependency, so the delete succeeds and the form type is gone.
Emulator: answers 400 with __type 'ConflictException' and Message '{"Message":"Form type <FormTypeName> cannot be deleted because of the following dependencies: gluetype glueAsset#glue_<accountId>#assettype_policy_<suffix>"}' instead of 200 {}.
Why: the form type can never be deleted, so a client cannot tear down or re-create its metadata forms; every delete-then-recreate flow and every cleanup path wedges on a 400 that AWS does not return. It also makes the emulator's dependency rule stricter than AWS's, so a client that handles ConflictException by deleting the named dependency first is sent on a dependency hunt that AWS never asks for.
Fix: DeleteFormType's handler and its dependency check in the form-type state — the reference from an asset type to a form type must not be treated as a deletion-blocking dependency; on success return 200 with an empty body.
Traces: glue-formtype-delete-then-not-found#task#3 (DeleteFormType, expected 200 / actual 400).

Cause 2 of 3: after the form type is deleted, GetFormType still answers 200 instead of refusing EntityNotFoundException.
AWS: Smithy: GetFormType input Identifier (FormTypeId, required), output Id/Name/Schema, errors AccessDeniedException, EntityNotFoundException (client, httpError 400), InternalServiceException, InvalidInputException, ThrottlingException. The recording shows AWS answering 400 with __type 'EntityNotFoundException' and Message '{"Message":"Metadata form type <FormTypeName> not found."}'. Fixed text: the JSON wrapper {"Message":"..."} plus 'Metadata form type ' and ' not found.'; the only variable part is the form-type name taken from the request (the recorded name is one run's fill-in, not part of the template).
Emulator: answers 200 with the form type's body — no __type and no Message at all (replay: 'status: expected 400, got 200', '$.__type: missing', '$.Message: missing'). Hypothesis: this is downstream of Cause 1 — the delete was refused, so the entity still exists — but it must be re-verified after Cause 1 is fixed, because a delete that mutates no state would produce the same reading.
Why: a deleted form type stays readable, so a client cannot observe deletion at all; a caller that polls GetFormType to confirm a teardown loops forever, and a caller that relies on EntityNotFoundException to decide whether to create never takes the create branch.
Fix: the form-type state's delete path (so the entry is actually removed) and GetFormType's lookup/refusal path (so a missing identifier raises EntityNotFoundException with AWS's wording).
Traces: glue-formtype-delete-then-not-found#task#4 (GetFormType, expected 400 / actual 200).

Cause 3 of 3: PutAttachment validates FormTypeId before the asset identifier, so a missing asset is reported as a missing form type.
AWS: Smithy: PutAttachment input AssetIdentifier (AssetId, required, @length 1..1087, pattern ^[a-zA-Z0-9\\-\\:\\/\\.\\_\\*]+$), AttachmentName (required), Content (required), FormTypeId (required), plus IterableFormName/ItemIdentifier/ClientToken; errors AccessDeniedException, ConcurrentModificationException, EntityNotFoundException (client, httpError 400), InternalServiceException, InvalidInputException, ThrottlingException. The recording shows AWS refusing 400 and naming the ASSET first, with Message '{"Message":"Asset not found: <assetIdentifier>"}'. Fixed text: the JSON wrapper {"Message":"..."} plus 'Asset not found: '; the variable part is the asset identifier copied from the request's AssetIdentifier. So when both the asset and the form type are absent, AWS resolves the asset first and the asset miss is the reported one.
Emulator: answers 400 at the right status but with Message '{"Message":"Metadata form type <FormTypeId> not found."}' — it reports the form-type miss, naming the request's FormTypeId instead of its AssetIdentifier.
Why: the refusal names the wrong resource and the wrong class of problem, so a client that parses the message (or branches on which identifier it names) remediates the wrong input: it goes and creates/looks up a form type when the real fault is that the asset does not exist. Error-message assertions and retry logic keyed on the asset identifier never match.
Fix: PutAttachment's input validation ordering — resolve and refuse on AssetIdentifier before FormTypeId, and author AWS's 'Asset not found: <assetIdentifier>' message for that case.
Traces: glue-not-found-error-paths#task#7 (PutAttachment, expected 400 / actual 400).

Related open issue (does not own these replay keys, so these causes are net-new): '20261108T010218Z-glue-getasset-reads-omit-the-asset-type-and.md' covers omitted GetAsset members on the same asset/form surface.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 83.5%, 66 divergence clusters) — these three clusters have empty owned_by and empty stub_path.

Cause 1 — glue-formtype-delete-then-not-found#task#3, DeleteFormType, verdict mismatch, expected_status 200, actual_status 400:
  status: expected 200, got 400
  $: value_mismatch (expected {}, got {'__type': 'ConflictException', 'Message': '{"Message":"Form type FormTypeRetention0d613640 cannot be deleted because of the following dependencies: gluetype glueAsset#glue_000000000000#assettype_policy_0d613640"}'})

Cause 2 — glue-formtype-delete-then-not-found#task#4, GetFormType, verdict mismatch, expected_status 400, actual_status 200:
  status: expected 400, got 200
  $.__type: missing (expected 'EntityNotFoundException', got None)
  $.Message: missing (expected '{"Message":"Metadata form type FormTypeRetention0d613640 not found."}', got None)

Cause 3 — glue-not-found-error-paths#task#7, PutAttachment, verdict mismatch, expected_status 400, actual_status 400:
  $.Message: value_mismatch (expected '{"Message":"Asset not found: asset-does-not-exist-0000"}', got '{"Message":"Metadata form type FormTypeMissing0000 not found."}')

Model evidence (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): DeleteFormType output is an empty structure and lists ConflictException among its errors; GetFormType and PutAttachment both list EntityNotFoundException (client, httpError 400); PutAttachment declares AssetIdentifier and FormTypeId both required.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Causes 1 and 2: replay fixture glue-formtype-delete-then-not-found. At step task#3 call DeleteFormType on the created form type and assert 200 with an empty JSON body (no ConflictException). At step task#4 call GetFormType on the same identifier and assert 400 with __type 'EntityNotFoundException' and Message '{"Message":"Metadata form type <name> not found."}', with <name> taken from the request.
3. Cause 3: replay fixture glue-not-found-error-paths step task#7 — call PutAttachment with an AssetIdentifier that does not exist and a FormTypeId that also does not exist; assert the 400 Message is '{"Message":"Asset not found: <assetIdentifier>"}' naming the request's AssetIdentifier, not a form-type miss.
4. Re-replay both fixtures end to end after fixing Cause 1, so Cause 2 is confirmed against a form type that was genuinely removed.
