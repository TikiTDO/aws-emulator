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
Issue-Commit: b5518325ad9936970d86f9a3541971c01b6fb226

# schemas code-binding archive is truncated and the TagResource 204 reaches the client with no Content-Length

> _Note: this issue was identified against the code base at commit `b5518325ad9936970d86f9a3541971c01b6fb226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCodeBindingSource

## Description
Two net-new causes in how the schemas service fills and frames its HTTP response payloads. Both are response-serialization defects of the same service, and each must be re-replayed after the fix. NOTE: the resolved issue 20261110T135450Z-schemas-response-payloads-diverge-from-aws-getcodebindingsource-serves.md described these same two replay keys, but it now sits in issues/things-that-now-work/ while BOTH keys still diverge on a replay of the current checkout (the replay reports them as unowned). A resolved issue covers nothing, so they are re-filed here with their fixture bindings.

Cause 1 of 2: GetCodeBindingSource serves a code-binding archive that is a small fraction of the one AWS publishes.
AWS: Smithy (services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json) models GetCodeBindingSource as GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/language/{Language}/source, http code 200, output_shape GetCodeBindingSourceResponse whose single member Body is a blob bound with @httpPayload (errors BadRequestException 400, UnauthorizedException 401, ForbiddenException 403, NotFoundException 404, TooManyRequestsException 429, InternalServerErrorException 500). The model says nothing about the archive's contents, so this is recorded behavior only: AWS answers 200 with the ZIP of everything its code generator emits for that (schema version, language) pair — a declaration per type the document implies PLUS the language's supporting files that make the binding compile and (de)serialize — and the step compares the payload by length, pinning that archive at 3060 bytes for the schema version this fixture builds.
Emulator: the status is right (200 expected, 200 actual) and the body is a well-formed ZIP, but it is 400 bytes where AWS's is 3060 — the replay reports "$.__wire_length__: value_mismatch (expected 3060, got 400)". What the key pins is that the served archive is a small fraction of the published one, i.e. the generated binding is missing most of its source.
Hypothesis: the generator emits exactly ONE file per structure it reads and nothing else — code_binding_archive() zips render() of each declared_structures() entry and adds no supporting/marshalling file, no package metadata and no per-member serialization helper, so even with every implied (inline/nested/list-item) type now hoisted and declared the archive stays a few hundred bytes of plain model declarations.
Why: a client that unpacks the archive and compiles against it — the entire purpose of a code binding — gets a source tree missing the files AWS's binding ships, so code written against the real binding does not build; a client that compares the artifact it downloaded against AWS's (size, entry list, or a checksum) sees a different artifact. The response is not interchangeable with AWS's.
Fix: the code-generation/serialization path of this operation's handler — the archive builder and the structure enumeration it feeds on, which must emit the full set of generated source files for the stored document rather than one declaration file per structure.
Traces: fixture schemas-code-binding-lifecycle, step task#6 (divergence_key service=schemas|GetCodeBindingSource|status=200->200|keys=$.__wire_length__).

Cause 2 of 2: the empty 204 TagResource answers reaches the client with no Content-Length header.
AWS: Smithy models TagResource as POST /tags/{ResourceArn} with input {ResourceArn @httpLabel @required, Tags jsonName 'tags' @required map}, output_shape Unit and http code 204; no output member is bound to a header, so the header is pure framing of the empty body. The recording shows the 204 carrying content-length with the fixed value '0' — the value has no variable part.
Emulator: status is right (204 expected, 204 actual) and the body is empty, but the response carries no content-length header at all — the replay reports "$headers.content-length: missing (expected '0', got None)".
Why: an HTTP client or proxy that frames a response by its declared length sees an unframed empty reply instead of an explicitly zero-length one; clients differ in how they treat a 204 with neither Content-Length nor Transfer-Encoding and some log or retry it as truncated, so the reply is not byte-interchangeable with AWS's.
Fix: the HTTP/1.1 response writer, not this service crate's serializer. The service already states the header at its own seam — EmulatedService::override_protocol in services/service-com-amazonaws-schemas-schemas/src/service.rs returns a protocol whose serialize_output calls frame_empty_body(), inserting content-length: 0 on every empty body (that file's module docs say so explicitly) — and the replay still observes the header absent on the socket, so what strips it is the transport's writer, which drops content-length on a 204. That writer is where the remaining half of this cause lives: it must preserve an explicitly serialized content-length: 0 on a 204 instead of removing it.
Related open issue: 20261029T203553Z-schemas-empty-body-204-responses-omit-the-content-length.md describes the same framing gap, but its replay key covers only UntagResource (schemas-registry-tagging#task#5), DeleteSchemaVersion and DeleteSchema (schemas-schema-lifecycle#task#8 / #task#10); TagResource's step is NOT carried by it and would be closed unrun, which is why it is filed here.
Traces: fixture schemas-registry-tagging, step task#3 (divergence_key service=schemas|TagResource|status=204->204|keys=$headers.content-length).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 13 fixtures replayed, 289 steps (75 passed, 214 mismatched, 0 errored, 0 blocked), score_pct 26.0, 58 divergence clusters. 56 clusters are owned by open issues (ListSchemas catalog, etc.); the two below are the only unowned ones and no cluster carries a stub_path.

Cause 1 — service=schemas, operation GetCodeBindingSource, fixture schemas-code-binding-lifecycle step task#6:
  expected_status 200, actual_status 200
  detail: $.__wire_length__: value_mismatch (expected 3060, got 400)
  divergence_key: service=schemas|GetCodeBindingSource|status=200->200|keys=$.__wire_length__
  affected: schemas-code-binding-lifecycle#task#6 (1 step), owned_by: [] , stub_path: ""
  Smithy: GetCodeBindingSource http {method GET, uri /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/language/{Language}/source, code 200}; output member Body target Body, httpPayload, type blob; input Language/RegistryName/SchemaName @httpLabel @required plus SchemaVersion @httpQuery('schemaVersion'); errors BadRequestException 400, UnauthorizedException 401, ForbiddenException 403, NotFoundException 404, TooManyRequestsException 429, InternalServerErrorException 500.
  Emulator source: services/service-com-amazonaws-schemas-schemas/src/ops/get_code_binding_source.rs — code_binding_archive() (lines ~431-462) zips exactly one entry per declared_structures() item via render(); declared_structures() (lines ~483-586) now hoists inline/nested and list-item types as well as the document's named schemas, yet the archive is still 400 bytes, so the shortfall is the per-structure file set itself, not the structure enumeration.

Cause 2 — service=schemas, operation TagResource, fixture schemas-registry-tagging step task#3:
  expected_status 204, actual_status 204
  detail: $headers.content-length: missing (expected '0', got None)
  divergence_key: service=schemas|TagResource|status=204->204|keys=$headers.content-length
  affected: schemas-registry-tagging#task#3 (1 step), owned_by: [] , stub_path: ""
  Smithy: TagResource input {ResourceArn @httpLabel @required, Tags jsonName 'tags' @required map}, output_shape Unit, http {method POST, uri /tags/{ResourceArn}, code 204}; no header-bound output member.
  Emulator source: services/service-com-amazonaws-schemas-schemas/src/service.rs — frame_empty_body() (lines 105-113) inserts content-length: 0 on an empty body and is applied in serialize_output()/serialize_error() (lines 145-180) through override_protocol() (line 252); the module docs at lines 62-69 record that the vendored HTTP writer removes content-length on a 204. The replay confirms the header is absent on the wire.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it; replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture schemas-code-binding-lifecycle up to step task#6 — CreateRegistry, CreateSchema, PutCodeBinding for the language the fixture asks for, poll DescribeCodeBinding until CREATE_COMPLETE, then GET /v1/registries/name/<registry>/schemas/name/<schema>/language/<language>/source. Observed: 200 with a ZIP body of 400 bytes; AWS's recorded body for the same step is 3060 bytes. Unzip both and compare the entry list: the emulator's archive holds only the plain model declaration files.
3. Cause 2: replay fixture schemas-registry-tagging step task#3 — POST /tags/<registry ARN> with a tags map, over HTTP/1.1. Inspect the raw response headers: status 204, empty body, and NO content-length header; AWS sends 'content-length: 0'.
4. After the fix, re-replay BOTH schemas-code-binding-lifecycle#task#6 and schemas-registry-tagging#task#3 — they are served by two different paths (the code-binding archive generator and the empty-response framing on the HTTP writer) and neither closes the other.
