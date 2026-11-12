Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-datasource-function-pipeline-resolver
Issue-Step: task#16
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-datasource-function-pipeline-resolver appsync-datasource-function-resolver-pipeline appsync-resolver-mapping-template-unicode-round-trip appsync-eval-code-error-detail appsync-type-sdl-json-format-fidelity
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|DeleteDataSource|4xx->2xx|$
Issue-Key: appsync|DeleteDataSource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|DeleteDataSource|4xx->2xx|status
Issue-Key: appsync|DeleteFunction|4xx->2xx|$
Issue-Key: appsync|DeleteFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|DeleteFunction|4xx->2xx|status
Issue-Key: appsync|UpdateResolver|2xx->4xx|$.resolver
Issue-Key: appsync|UpdateResolver|2xx->4xx|status
Issue-Key: appsync|DeleteFunction|2xx->4xx|status
Issue-Key: appsync|DeleteDataSource|2xx->4xx|status
Issue-Key: appsync|DeleteDataSource|2xx->4xx|$
Issue-Key: appsync|CreateFunction|4xx->2xx|$.detail
Issue-Key: appsync|CreateFunction|4xx->2xx|$.message
Issue-Key: appsync|CreateFunction|4xx->2xx|$.reason
Issue-Key: appsync|CreateFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateFunction|4xx->2xx|status
Issue-Key: appsync|ListFunctions|2xx->2xx|$.functions[*]
Issue-Key: appsync|CreateResolver|2xx->4xx|$.resolver
Issue-Key: appsync|CreateResolver|2xx->4xx|status
Issue-Key: appsync|GetType|2xx->4xx|$.type
Issue-Key: appsync|GetType|2xx->4xx|status
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# AppSync resolver/function/data-source plane: dependency refusals missing, deletes answer 404, and valid payloads are refused (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDataSource

## Description
The resolver/function/data-source plane diverges in both directions: the emulator accepts deletes AWS refuses because nothing tracks which data sources and functions are still in use, it refuses valid creates/updates (a PIPELINE resolver, a Unicode mapping template, a type read), it cannot resolve functions and data sources by name on the delete path, and it never evaluates APPSYNC_JS code so a syntactically broken function is created. 10 net-new causes.

Cause 1 of 10 — DeleteDataSource does not refuse a data source still referenced by a function.
AWS: 400 BadRequestException with the template "Data source is still in use by functions: [<functionId>, ...]" — fixed text "Data source is still in use by functions: " and a bracketed list of the referencing function ids read from state; x-amzn-errortype BadRequestException (recorded behavior; Smithy: DeleteDataSource errors include BadRequestException httpError 400).
Emulator: 200 with an empty body and no errortype — the data source is deleted out from under the function.
Why: the function is left referencing a data source that no longer exists, a state AWS prevents, and the client never learns it must delete the function first.
Fix: DeleteDataSource validation — consult the function/resolver references in state before deleting.
Traces: appsync-datasource-function-pipeline-resolver#task#16, appsync-datasource-function-resolver-pipeline#task#20.

Cause 2 of 10 — DeleteFunction does not refuse a function still used by a resolver.
AWS: 400 BadRequestException with the template "Cannot delete a function which is currently used by a resolver. Resolver type: <typeName>, field: <fieldName>" — fixed text with the referencing resolver's type and field names from state; x-amzn-errortype BadRequestException (recorded behavior; Smithy: DeleteFunction errors include BadRequestException httpError 400).
Emulator: 200, empty body, no errortype.
Why: a pipeline resolver is left pointing at a deleted function, so invocations fail at runtime instead of the delete failing.
Fix: DeleteFunction validation — consult resolver pipelineConfig.functions before deleting.
Traces: appsync-datasource-function-pipeline-resolver#task#17.

Cause 3 of 10 — UpdateResolver refuses a valid PIPELINE resolver update with 400.
AWS: 200, returning the resolver with kind PIPELINE, pipelineConfig.functions populated and dataSourceName absent — a PIPELINE resolver legitimately has no data source of its own (Smithy: UpdateResolver output member resolver; Resolver.kind enum, Resolver.pipelineConfig).
Emulator: 400 with no resolver in the body.
Why: pipeline resolvers cannot be updated at all, so the whole pipeline-resolver workflow is unreachable.
Fix: UpdateResolver validation — do not require dataSourceName when kind is PIPELINE.
Traces: appsync-datasource-function-pipeline-resolver#task#14, appsync-resolver-mapping-template-unicode-round-trip#task#6.

Cause 4 of 10 — DeleteFunction answers 404 for a function the emulator created.
AWS: 200.
Emulator: 404.
Why: a client's teardown cannot remove functions, so function state leaks and names cannot be reused.
Fix: the function index used by DeleteFunction must resolve the functionId minted by CreateFunction.
Traces: appsync-datasource-function-pipeline-resolver#teardown#19.

Cause 5 of 10 — DeleteDataSource answers 404 for a data source the emulator created.
AWS: 200.
Emulator: 404.
Fix: the data-source index used by DeleteDataSource must resolve by the name CreateDataSource stored.
Traces: appsync-datasource-function-pipeline-resolver#teardown#20.

Cause 6 of 10 — the same DeleteDataSource 404 occurs mid-flow, with a not-found body where AWS returns an empty 200.
AWS: 200 with an empty body (Smithy: DeleteDataSourceResponse has no members).
Emulator: 404 with {"__type": "NotFoundException", "message": "Data source <name> not found"}.
Why: the delete is unusable even immediately after a successful create, so no flow can reach its cleanup phase.
Fix: same lookup defect as cause 5, in the DeleteDataSource handler.
Traces: appsync-datasource-function-resolver-pipeline#task#25.

Cause 7 of 10 — CreateFunction accepts APPSYNC_JS code that does not parse.
AWS: 400 BadRequestException with reason CODE_ERROR, message "The code contains one or more errors." and a detail.codeErrors list whose entries carry errorType (e.g. PARSE_ERROR), the compiler's own diagnostic as value, and a location {line, column, span}. The fixed text is the "The code contains one or more errors." message and the CODE_ERROR reason; the variable parts are the compiler diagnostic and the location, both derived from the submitted code (recorded behavior).
Emulator: 200 — the function is created, with no reason, detail, message or x-amzn-errortype.
Why: a client that relies on AppSync rejecting bad resolver code at deploy time ships a broken function, and its CODE_ERROR handling (which surfaces line/column to the developer) is unreachable.
Fix: CreateFunction validation — parse the APPSYNC_JS code and raise the CODE_ERROR BadRequestException with the codeErrors detail.
Traces: appsync-eval-code-error-detail#task#4, appsync-eval-code-error-detail#task#5.

Cause 8 of 10 — ListFunctions returns the wrong inventory, consequent to cause 7 and to a function-identity defect.
AWS: the listing holds exactly the functions that were accepted, each with its own functionId/functionArn, and emits the optional members (description, requestMappingTemplate, responseMappingTemplate, cachingConfig, functionVersion, syncConfig) AWS projects.
Emulator: lists the functions that should have been refused, repeats the same function name under two different functionIds, and omits the optional members from the projection.
Why: the API's inventory does not reflect what the client successfully created, so reconciliation and idempotent re-deploys diverge.
Fix: the function state keyed by name plus the ListFunctions serializer's member projection; the refusal in cause 7 must also keep the rejected function out of state.
Traces: appsync-eval-code-error-detail#task#8.

Cause 9 of 10 — CreateResolver refuses a mapping template containing non-ASCII characters.
AWS: 200, storing and echoing the requestMappingTemplate byte-for-byte including non-ASCII text, emoji, escaped quotes and backslashes (Smithy: Resolver.requestMappingTemplate / .responseMappingTemplate, MappingTemplate string — no character-class constraint).
Emulator: 400 with no resolver in the body.
Why: any resolver whose template carries a comment or literal outside ASCII cannot be created, which is a routine case for localized schemas.
Fix: CreateResolver validation/deserialization — stop restricting mapping-template characters (and preserve the template verbatim through the serializer).
Traces: appsync-resolver-mapping-template-unicode-round-trip#task#4.

Cause 10 of 10 — GetType refuses the format value AWS accepts on this operation.
AWS: 200, returning the type with its definition and the format echoed as requested. The recorded response shows AWS serving GetType for a format value outside the TypeDefinitionFormat enum and echoing it in Type.format (recorded behavior; Smithy: TypeDefinitionFormat enum values are SDL and JSON, and GetType.format is @required @httpQuery — the model is stricter than the recorded service, so the recording governs).
Emulator: 400 with no type in the body.
Why: a client reading a type back with the format value AWS tolerates gets a hard refusal, so the type-format fidelity flow cannot complete.
Fix: GetType's format handling in the operation's validation/deserialization.
Traces: appsync-type-sdl-json-format-fidelity#task#21.

## Evidence
Replay of simparity-wire-aws-v1.

1) appsync-datasource-function-pipeline-resolver#task#16 DeleteDataSource: status expected 400 got 200; expected body message 'Data source is still in use by functions: [<functionId>]' and errortype 'BadRequestException', got {} and no header.
2) appsync-datasource-function-pipeline-resolver#task#17 DeleteFunction: status expected 400 got 200; expected 'Cannot delete a function which is currently used by a resolver. Resolver type: Query, field: getItem' with errortype 'BadRequestException', got {}.
3) appsync-datasource-function-pipeline-resolver#task#14 UpdateResolver: status expected 200 got 400; $.resolver (kind PIPELINE, pipelineConfig.functions populated, dataSourceName None) expected, got None.
4) appsync-datasource-function-pipeline-resolver#teardown#19 DeleteFunction: status expected 200, got 404.
5) appsync-datasource-function-pipeline-resolver#teardown#20 DeleteDataSource: status expected 200, got 404.
6) appsync-datasource-function-resolver-pipeline#task#25 DeleteDataSource: status expected 200 got 404; body expected {} got {'__type': 'NotFoundException', 'message': 'Data source <name> not found'}.
7) appsync-eval-code-error-detail#task#4 CreateFunction: status expected 400 got 200; $.reason expected 'CODE_ERROR' got None; $.detail expected {'codeErrors': [{'errorType': 'PARSE_ERROR', 'value': "code.js(7,1): error TS1005: '}' expected.\n", 'location': {'line': 7, 'column': 1, 'span': 0}}]} got None; $.message expected 'The code contains one or more errors.' got None.
8) appsync-eval-code-error-detail#task#8 ListFunctions 200: $.functions[0] expected the accepted function (name ..._fn_ok_..., with description/requestMappingTemplate/responseMappingTemplate/cachingConfig/functionVersion/syncConfig members), got the rejected one (name ..._fn_bad_...); $.functions[1] expected absent, got a second entry with the same name under a different functionId.
9) appsync-resolver-mapping-template-unicode-round-trip#task#4 CreateResolver: status expected 200 got 400; $.resolver with requestMappingTemplate '## comment: héllo 世界 🎉 "quoted" \\backslash\\ Ω≈ç√ ...' expected, got None.
10) appsync-type-sdl-json-format-fidelity#task#21 GetType: status expected 200 got 400; $.type (definition 'enum Color ... {RED GREEN BLUE}', format 'YAML') expected, got None.

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay simparity-wire-aws-v1.
2. Fixtures appsync-datasource-function-pipeline-resolver, appsync-datasource-function-resolver-pipeline, appsync-eval-code-error-detail, appsync-resolver-mapping-template-unicode-round-trip and appsync-type-sdl-json-format-fidelity mismatch at the steps under Traces.
3. By hand: create a data source, a function on it and a PIPELINE resolver referencing the function, then DeleteDataSource (cause 1) and DeleteFunction (cause 2); UpdateResolver on the PIPELINE resolver (cause 3); DeleteFunction/DeleteDataSource by the ids and names the creates returned (causes 4-6); CreateFunction with APPSYNC_JS code missing a closing brace, then ListFunctions (causes 7-8); CreateResolver with a non-ASCII requestMappingTemplate (cause 9); GetType with format=YAML (cause 10).
