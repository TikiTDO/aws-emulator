Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-code-binding-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-code-binding-lifecycle schemas-registry-tagging
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|GetCodeBindingSource|2xx->2xx|$.__wire_length__
Issue-Key: schemas|TagResource|2xx->2xx|$headers.content-length
Issue-Commit: 9821f2ab76525a0c0a9a7ed388e3f955f1962573

# schemas response payloads diverge from AWS: GetCodeBindingSource serves a truncated archive and the TagResource 204 omits Content-Length

> _Note: this issue was identified against the code base at commit `9821f2ab76525a0c0a9a7ed388e3f955f1962573`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCodeBindingSource

## Description
Two net-new causes in how the schemas service frames and fills its HTTP response payloads. Both are response-serialization defects of the same service and each must be re-replayed after the fix.

Cause 1 of 2: GetCodeBindingSource serves a code-binding archive far smaller than the one AWS generates.
AWS: Smithy (services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json) models GetCodeBindingSource as GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/language/{Language}/source, http code 200, output_shape GetCodeBindingSourceResponse whose single member Body is a blob bound with @httpPayload. The model does not describe the archive's contents, so this is recorded behavior only: AWS answers 200 with the ZIP archive of the source files its code generator produced for that (schema version, language) pair, and for the schema version this fixture builds that archive is 3060 bytes of payload. The archive AWS publishes carries a declaration per type the document implies — including the inline/nested object types a named schema's members declare — not only the document's top-level named schemas.
Emulator: the status is right (200 expected, 200 actual) and the body is a ZIP archive, but it is 400 bytes where AWS's is 3060 — the replay reports $.__wire_length__: value_mismatch (expected 3060, got 400). The payload is compared by length here, so what is pinned is that the archive is a small fraction of the published one, i.e. it is missing generated source.
Hypothesis: services/service-com-amazonaws-schemas-schemas/src/ops/get_code_binding_source.rs `declared_structures` enumerates ONLY the document's top-level named schemas (components/schemas for OpenApi3, the root schema plus `definitions` for JSONSchemaDraft4) and skips any member whose declaration is an inline object, so one file is emitted where AWS emits a file per implied type; `code_binding_archive` then zips just those files.
Why: a client that unpacks the archive and compiles against it (the whole point of a code binding) gets a package missing most of the types its handler needs, and a client that merely checks the payload size or entry list against what AWS returns sees a different artifact. The response is not interchangeable with AWS's.
Fix: the code-generation/serialization path of this operation's handler — the archive builder and the structure enumeration it feeds on (the `code_binding_archive` / `declared_structures` pair in the GetCodeBindingSource handler), which must emit the full set of generated source files for the stored document rather than only its top-level named schemas.
Traces: fixture schemas-code-binding-lifecycle, step task#6 (divergence_key service=schemas|GetCodeBindingSource|status=200->200|keys=$.__wire_length__).

Cause 2 of 2: TagResource's empty 204 carries no Content-Length header.
AWS: Smithy models TagResource as POST /tags/{ResourceArn} with output_shape Unit and http code 204; no output member is bound to a header, so the header is pure framing of the empty body. The recording shows the 204 carrying content-length with the fixed value '0' — there is no variable part to that value.
Emulator: status is right (204 expected, 204 actual) and the body is empty, but no content-length header is emitted at all — the replay reports $headers.content-length: missing (expected '0', got None).
Why: an HTTP client or proxy that frames a response by its declared length sees an unframed empty response instead of an explicitly zero-length one; clients differ in how they treat a 204 with neither Content-Length nor Transfer-Encoding and some log or retry it as truncated, so the reply is not byte-interchangeable with AWS's.
Fix: the response serializer for an operation whose output shape is Unit — it must emit Content-Length: 0 on the empty 204. Note the open issue 20261029T203553Z-schemas-empty-body-204-responses-omit-the-content-length.md describes the same framing gap, but its replay key covers only UntagResource (schemas-registry-tagging#task#5), DeleteSchemaVersion and DeleteSchema (schemas-schema-lifecycle#task#8 / #task#10); TagResource's step is NOT carried by it and would be closed unrun, which is why it is filed here.
Traces: fixture schemas-registry-tagging, step task#3 (divergence_key service=schemas|TagResource|status=204->204|keys=$headers.content-length).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score_pct 23.6).

Cause 1 — service=schemas, operation GetCodeBindingSource, fixture schemas-code-binding-lifecycle step task#6:
  expected_status 200, actual_status 200
  detail: $.__wire_length__: value_mismatch (expected 3060, got 400)
  divergence_key: service=schemas|GetCodeBindingSource|status=200->200|keys=$.__wire_length__
  affected: schemas-code-binding-lifecycle#task#6 (1 step)
  Smithy: GetCodeBindingSource http {method GET, uri /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/language/{Language}/source, code 200}; output member Body target Body, httpPayload, type blob; errors BadRequestException 400, ForbiddenException 403, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401, InternalServerErrorException 500.
  Emulator source: services/service-com-amazonaws-schemas-schemas/src/ops/get_code_binding_source.rs — handle() returns code_binding_archive(target, &schema.name, &version.r#type, &version.content) (line ~243); code_binding_archive zips one file per entry of declared_structures (line ~434-453); declared_structures collects only components/schemas (OpenApi3) or the root schema + `definitions` (draft-04) and skips declarations with no `properties` object (line ~469-529).

Cause 2 — service=schemas, operation TagResource, fixture schemas-registry-tagging step task#3:
  expected_status 204, actual_status 204
  detail: $headers.content-length: missing (expected '0', got None)
  divergence_key: service=schemas|TagResource|status=204->204|keys=$headers.content-length
  affected: schemas-registry-tagging#task#3 (1 step)
  Smithy: TagResource input {ResourceArn httpLabel required, Tags jsonName 'tags' required map}, output_shape Unit, http {method POST, uri /tags/{ResourceArn}, code 204}; no header-bound output member.
  The sibling empty-204 operations (UntagResource, DeleteSchemaVersion, DeleteSchema) diverge identically and are already owned by 20261029T203553Z-schemas-empty-body-204-responses-omit-the-content-length.md; TagResource is reported unowned by the replay and is the net-new cause filed here.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it; replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture schemas-code-binding-lifecycle up to step task#6 — create the registry and schema, PutCodeBinding for the language the fixture asks for, wait for the binding to report CREATE_COMPLETE, then GET /v1/registries/name/<registry>/schemas/name/<schema>/language/<language>/source. The response is 200 with a ZIP body of 400 bytes; AWS's recorded body for the same step is 3060 bytes. Unzip both: the emulator's archive contains only the files generated for the document's top-level named schemas.
3. Cause 2: replay fixture schemas-registry-tagging step task#3 — POST /tags/<registry ARN> with a tags map. Inspect the response headers: status 204 with an empty body and NO content-length header; AWS sends 'content-length: 0'.
4. After the fix, re-replay BOTH schemas-code-binding-lifecycle#task#6 and schemas-registry-tagging#task#3 — they are served by two different paths (the code-binding archive generator and the empty-204 response serializer) and neither closes the other.

## Resolution

Cause 1 (the truncated archive): the generator now renders one source file per type the
stored document IMPLIES, not only per type it names. `declared_structures` in
`services/service-com-amazonaws-schemas-schemas/src/ops/get_code_binding_source.rs` reads
the document's named schemas as before and then walks their members: a member whose
declaration is an object carrying members of its own declares a nested structure the
document never names, so it is given a name (the member's own, qualified by the structure
it belongs to where that name is already some type's, numbered after that) and read as a
structure in its turn — through the `items` of a list as well, and however deeply the
nesting goes. The member then holds that type rather than the language's unconstrained
one. An object that declares no member of its own is still free-form and still gets no
file. The walk is a queue rather than a recursion, so a deeply nested document does not
grow the stack. `tests/all/code_binding_implied_types.rs` asserts the rule over a table of
documents (inline under a member, under a list's items, three levels deep, a shadowed
name, one member name in two structures, draft-04, and the free-form boundary) in every
language the service generates for.

CAVEAT: the replay key for this cause compares the archive's BYTE LENGTH against the
recording's. AWS publishes the bindings, not the generator that writes them, and the model
describes nothing about the archive's contents, so the exact length of a proprietary code
generator's output is not reproducible by any rule — only by fitting that one recorded
number, which is not a fix. What the key pins beyond that number — that the archive was a
small fraction of the published one because it was missing generated source — is what the
change above addresses.

Cause 2 (`content-length: 0` on the empty 204): already served at the one seam a service
owns for it. `Service::override_protocol` in `src/service.rs` hands every modelled response
of this service through a protocol that frames an answer it serializes with an empty body
as `content-length: 0`, which covers every operation of this model whose output is `Unit`,
`TagResource` included; `tests/all/resource_tag_application.rs` now carries `TagResource`'s
framing row beside the sibling rows in `tests/all/resource_tag_removal.rs`,
`tests/all/empty_response_framing.rs` and `tests/all/empty_result_statuses.rs`. CAVEAT: a
client reached over HTTP/1.1 still does not see the header on a `204` — the HTTP writer the
vendored transport serves on (`vendor/server/src/lib.rs`, the hyper connection builder)
removes it there, and that writer is under the read-only vendored tree, so the wire-level
half of this cause is a platform limitation rather than something a service crate can state
its way out of. The suite's wire probe pins what the socket really delivers.
