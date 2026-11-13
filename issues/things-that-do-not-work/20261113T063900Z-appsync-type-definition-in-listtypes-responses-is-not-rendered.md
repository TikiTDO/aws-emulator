Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-graphql-api-and-type-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-graphql-api-and-type-lifecycle appsync-list-types-after-create-type appsync-list-types-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/adahd6e6yapoida52iytmtqti3/types/Blog"].definition
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/anwc6quoyz77zmj2ozykkeubqr/types/Widget"].definition
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/aqnnelu5tpxyelkaio5kuk5fz7/types/Query"].definition
Issue-Commit: f7067607673c78bda9a2a88ab2c9b41127d89615

# Type$definition in ListTypes responses is not rendered from the parsed schema (3 causes)

> _Note: this issue was identified against the code base at commit `f7067607673c78bda9a2a88ab2c9b41127d89615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTypes

## Description
One shared fault, three net-new divergence clusters (all service=appsync, all reported with owned_by [] and stub_path "" in this replay): the ListTypes 200-response projection does not produce Type$definition the way AWS does. AWS re-prints every type from its own parsed schema model in a fixed, newline-terminated multi-line layout; the emulator instead serves either the exact SDL text the client submitted or the output of its own pretty-printer. All three causes must be closed before this issue is submitted.

Related open issues that do NOT own these clusters (the replay reported owned_by [] for all three, so a replay driven from those issues does not exercise these keys and this fix must re-attach to the bindings below; they are named only to avoid duplicated work): 20261112T215152Z-appsync-listtypes-echoes-stored-sdl-text-for-type-definition.md, 20261112T173623Z-appsync-type-definition-is-not-re-rendered-by.md, 20261112T064707Z-appsync-listtypes-serves-a-non-aws-sdl-rendering.md and 20261112T042459Z-appsync-type-projections-diverge-list-responses-serve-the.md all describe this behaviour, but their recorded replay keys embed the per-run AppSync api id inside the matched JSON path ($.types[arn="...apis/<apiId>/types/<Type>"].definition), so they do not join the keys this replay reports. 20261111T092526Z-appsync-getintrospectionschema-returns-a-non-canonical-sdl-the.md covers GetIntrospectionSchema, and 20261112T042459Z-appsync-listtypes-refuses-an-unresolvable-apiid-as-400.md covers ListTypes' not-found refusal, not its 200 body.

Cause 1 of 3 - ListTypes?format=SDL echoes the submitted single-line definition instead of AWS's rendering (fixture appsync-graphql-api-and-type-lifecycle, step task#4).
AWS: recorded behavior only for the text layout. Smithy: ListTypes = GET /v1/apis/{apiId}/types, code 200; required httpLabel apiId, required httpQuery format of enum TypeDefinitionFormat (SDL|JSON), maxResults @range 0..25 (default 0), nextToken @length 1..65536 @pattern ^[\S]+$, paginated {inputToken/outputToken nextToken, items types}; output ListTypesResponse.types: TypeList of Type = {name: ResourceName, description, arn, definition: String (doc "The type definition"), format: TypeDefinitionFormat}. The member definition carries no formatting or serialization trait, so the layout requirement comes from the recordings: AWS emits "type <Name>\n\n{\n", then each field as "<field>: <Type>" followed by a blank line, an extra blank line between consecutive fields, closing with "}\n". The type name and the field names/types are values taken from the request; the multi-line, newline-terminated layout is the fixed part.
Emulator: 200 with every other Type member correct, but types[].definition is the raw request string - expected 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n', got 'type <Name> { <f1>: <T1> <f2>: <T2> }\n'. Recorded instance: expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }\n' - 'Blog'/'id'/'title' are request values, the newline structure is the template.
Why: a client that diffs, hashes or line-parses the SDL ListTypes returns (schema-drift gates in CI, SDL round-trips through the format parameter, codegen that splits a definition into lines) sees a document AWS would never emit, so a comparison against a real account fails although the schema is unchanged.
Fix: the Type response projection behind the ListTypes handler - render Type$definition from the parsed type model for the requested format instead of returning the stored request text.
Traces: appsync-graphql-api-and-type-lifecycle#task#4 (same cluster: #task#5).

Cause 2 of 3 - the same divergence for a CreateType-created type, additionally missing the terminating newline (fixture appsync-list-types-after-create-type, step task#5).
AWS: as Cause 1 - the re-rendered, newline-terminated multi-line document (same Smithy citation; nothing in the model constrains the text, so recorded behavior only).
Emulator: returns the submitted one-line SDL with no trailing newline at all - expected 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n', got 'type <Name> { <f1>: <T1> <f2>: <T2> }'. Recorded instance: expected 'type Widget\n\n{\nid: ID!\n\n\nname: String\n\n}\n', got 'type Widget { id: ID! name: String }'. This is a distinct cluster with its own replay key, so Cause 1's step does not exercise it.
Why: same as Cause 1, and because the document is not newline-terminated even a client that normalises internal whitespace still gets a different byte string and a different hash.
Fix: the same Type projection; the renderer must terminate every emitted type document with a newline.
Traces: appsync-list-types-after-create-type#task#5 (same cluster: #task#6).

Cause 3 of 3 - for a schema-derived type ListTypes emits the emulator's own indented rendering (fixture appsync-list-types-lifecycle, step task#3).
AWS: as Cause 1; for a single-field type the rendered document is 'type <Name>\n\n{\n<field>: <Type>\n\n}\n' (recorded behavior only).
Emulator: emits two-space-indented output of its own printer - expected 'type Query\n\n{\ngetItem: String\n\n}\n', got 'type Query {\n  getItem: String\n}'. 'Query'/'getItem' come from the submitted schema; the indentation and the missing terminating newline are the emulator's. This shows the fault is not merely "echo the request": where the emulator does render, it uses its own layout.
Why: the definition AWS returns is a stable, service-defined rendering, so no ListTypes response - echoed (Causes 1-2) or self-rendered (this one) - is byte-comparable with AWS, and a client cannot tell a real schema change from a formatting difference.
Fix: one shared SDL renderer behind the Type projection, used for types derived from a StartSchemaCreation schema as well as for types created through CreateType.
Traces: appsync-list-types-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true; "/__admin/health answered 200 and the :40995 data listener accepts"). 91 fixtures replayed, 733 steps: 540 passed, 193 mismatched, 0 blocked, 0 errored, score 73.7%, 64 divergence clusters, every one service=appsync. 59 clusters carry owned_by, 0 are stub-blocked, 5 are net-new; the 3 bundled here are the ListTypes family.

1) Cause 1 - fixture appsync-graphql-api-and-type-lifecycle, step task#4, operation ListTypes, expected_status 200, actual_status 200, verdict mismatch, owned_by [], stub_path "". divergence_key: service=appsync|ListTypes|status=200->200|keys=$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/adahd6e6yapoida52iytmtqti3/types/Blog"].definition. detail: value_mismatch (expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }\n'). affected_count 2 (also appsync-graphql-api-and-type-lifecycle#task#5). The api id inside the ARN is a per-run value and is not part of the requirement.

2) Cause 2 - fixture appsync-list-types-after-create-type, step task#5, operation ListTypes, 200 vs 200, verdict mismatch, owned_by [], stub_path "". detail: $.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/anwc6quoyz77zmj2ozykkeubqr/types/Widget"].definition: value_mismatch (expected 'type Widget\n\n{\nid: ID!\n\n\nname: String\n\n}\n', got 'type Widget { id: ID! name: String }'). affected_count 2 (also appsync-list-types-after-create-type#task#6).

3) Cause 3 - fixture appsync-list-types-lifecycle, step task#3, operation ListTypes, 200 vs 200, verdict mismatch, owned_by [], stub_path "". detail: $.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/aqnnelu5tpxyelkaio5kuk5fz7/types/Query"].definition: value_mismatch (expected 'type Query\n\n{\ngetItem: String\n\n}\n', got 'type Query {\n  getItem: String\n}'). affected_count 1.

Model evidence (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json): ListTypes GET /v1/apis/{apiId}/types code 200; required query member format of enum TypeDefinitionFormat (SDL|JSON), maxResults @range 0..25 default 0, nextToken @length 1..65536 @pattern ^[\S]+$, paginated items=types; errors BadRequestException(400), UnauthorizedException(401), NotFoundException(404), ConcurrentModificationException(409), InternalFailureException(500). CreateType POST /v1/apis/{apiId}/types takes required definition + required format and returns Type. No trait on Type$definition constrains its text, so the rendering requirement above is taken from the recorded responses.

## Steps to Reproduce
1. Clone the repo, build/boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture appsync-graphql-api-and-type-lifecycle, step task#4 (its cluster also covers task#5). Chain: CreateGraphqlApi, then CreateType with a single-line SDL object type carrying two fields, then ListTypes with format=SDL. Expect each types[].definition re-rendered as 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n'; the emulator returns the submitted one-line string.
3. Cause 2: replay fixture appsync-list-types-after-create-type, step task#5 (cluster also covers task#6). Same shape; the emulator returns the one-line string and additionally drops the terminating newline.
4. Cause 3: replay fixture appsync-list-types-lifecycle, step task#3. Create the type through StartSchemaCreation instead of CreateType, then ListTypes with format=SDL. Expect AWS's layout; the emulator returns its own two-space-indented rendering with no terminating newline.
5. All three fixture steps must pass before this issue is closed.
