Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-graphql-api-and-type-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-graphql-api-and-type-lifecycle appsync-list-types-after-create-type appsync-list-types-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/hct5ml3pdeivrglpyenuhpwvpk/types/Blog"].definition
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/c4k72f4siyfokbcu6xombkfknt/types/Widget"].definition
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/myb7xm2bjv3nfczs2if4jb7jam/types/Query"].definition
Issue-Commit: f1963e5ed2264395f5bc9fed00150a8e83d2f518

# ListTypes echoes stored SDL text for Type definition instead of AWS's rendered layout (3 causes)

> _Note: this issue was identified against the code base at commit `f1963e5ed2264395f5bc9fed00150a8e83d2f518`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTypes

## Description
One shared fault, three net-new divergence clusters (all service=appsync, all with owned_by [] and stub_path "" in this replay): the ListTypes 200-response projection does not re-render Type$definition the way AWS does. AWS prints each type from its own parsed schema model in a fixed, newline-terminated multi-line layout; the emulator serves either the SDL text the client submitted or its own pretty-printer's output. All three causes must be closed before this issue is submitted.

Related open issues that do NOT own these clusters (the replay reported owned_by [] for all three, so a replay driven from them does not exercise these keys; they are named here only to avoid duplicated work): 20261112T173623Z-appsync-type-definition-is-not-re-rendered-by.md, 20261112T064707Z-appsync-listtypes-serves-a-non-aws-sdl-rendering.md and 20261112T042459Z-appsync-type-projections-diverge-list-responses-serve-the.md all describe this behaviour, but their recorded replay keys embed the per-run AppSync api id inside the matched JSON path ($.types[arn="...apis/<apiId>/types/<Type>"].definition), so they do not join the keys this replay reports and the fix must re-attach to the keys below. 20261111T092526Z-appsync-getintrospectionschema-returns-a-non-canonical-sdl-the.md covers GetIntrospectionSchema, and 20261112T042459Z-appsync-listtypes-refuses-an-unresolvable-apiid-as-400.md covers ListTypes' not-found refusal, not its 200 body.

Cause 1 of 3 - ListTypes?format=SDL echoes the submitted single-line definition instead of AWS's rendering (fixture appsync-graphql-api-and-type-lifecycle, step task#4).
AWS: recorded behavior only for the text layout. Smithy: ListTypes = GET /v1/apis/{apiId}/types, code 200; required httpLabel apiId, required httpQuery `format` of enum TypeDefinitionFormat (values SDL, JSON), maxResults @range 0..25 (default 0), nextToken @length 1..65536 @pattern ^[\S]+$, paginated {inputToken/outputToken nextToken, items types}; output ListTypesResponse.types: TypeList of Type = {name: ResourceName @length 1..65536 @pattern ^[_A-Za-z][_0-9A-Za-z]*$, description, arn, definition: String (doc "The type definition"), format: TypeDefinitionFormat}. `definition` carries no formatting or serialization trait, so the layout is grounded in the recording: AWS re-prints the parsed type as "type <Name>\n\n{\n", then each field as "<field>: <Type>" followed by a blank line, an extra blank line between consecutive fields, closing with "}\n". The type name and the field names/types are request values; the multi-line, newline-terminated layout is the requirement.
Emulator: 200 with every other Type member correct, but types[].definition is the raw request string - expected 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n', got 'type <Name> { <f1>: <T1> <f2>: <T2> }\n' (recorded instance: expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }\n'; 'Blog'/'id'/'title' are request values, the newline structure is the template).
Why: a client that diffs, hashes or line-parses the SDL ListTypes returns - schema-drift gates in CI, SDL round-trips through the `format` parameter, codegen that splits the definition into lines - sees a document AWS would never emit, so a comparison against a real account fails although the schema is unchanged.
Fix: the Type response projection behind the ListTypes handler - render Type$definition from the parsed type model for the requested format instead of returning the stored request text.
Traces: appsync-graphql-api-and-type-lifecycle#task#4 (same cluster: #task#5).

Cause 2 of 3 - the same divergence for a CreateType-created type, additionally missing the terminating newline (fixture appsync-list-types-after-create-type, step task#5).
AWS: as Cause 1 - the re-rendered, newline-terminated multi-line document (same Smithy citation; nothing in the model constrains the text, so this is recorded behavior only).
Emulator: returns the submitted one-line SDL with no trailing newline at all - expected 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n', got 'type <Name> { <f1>: <T1> <f2>: <T2> }' (recorded instance: expected 'type Widget\n\n{\nid: ID!\n\n\nname: String\n\n}\n', got 'type Widget { id: ID! name: String }'). This is a distinct cluster with a distinct replay key, so Cause 1's step does not exercise it.
Why: same as Cause 1, and because the document is not newline-terminated even a client that normalises internal whitespace still gets a different byte string and a different hash.
Fix: the same Type projection; the renderer must terminate every emitted type document with a newline.
Traces: appsync-list-types-after-create-type#task#5 (same cluster: #task#6).

Cause 3 of 3 - for a schema-derived type ListTypes emits the emulator's own indented rendering (fixture appsync-list-types-lifecycle, step task#3).
AWS: as Cause 1; for a single-field type the rendered document is 'type <Name>\n\n{\n<field>: <Type>\n\n}\n' (recorded behavior only).
Emulator: emits two-space-indented output of its own printer - expected 'type Query\n\n{\ngetItem: String\n\n}\n', got 'type Query {\n  getItem: String\n}' ('Query'/'getItem' come from the submitted schema; the indentation and the missing terminating newline are the emulator's). This shows the fault is not only "echo the request": where the emulator does render, it uses its own layout.
Why: the definition AWS returns is a stable, service-defined rendering, so no ListTypes response - echoed (Causes 1-2) or self-rendered (this one) - is byte-comparable with AWS, and a client cannot tell a real schema change from a formatting difference.
Fix: one shared SDL renderer behind the Type projection, used for types derived from a StartSchemaCreation schema as well as for types created through CreateType.
Traces: appsync-list-types-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health answered 200 and the data listener accepts). 733 steps: 522 passed, 201 mismatched, 10 blocked, 0 errored, score 71.2%, 65 divergence clusters, every one service=appsync; 62 clusters carry owned_by, 0 are stub-blocked, 3 are net-new - exactly the three bundled here.

1) Cause 1 - fixture appsync-graphql-api-and-type-lifecycle, step task#4, operation ListTypes, expected_status 200, actual_status 200, verdict mismatch, owned_by [], stub_path "". divergence_key: service=appsync|ListTypes|status=200->200|keys=$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/hct5ml3pdeivrglpyenuhpwvpk/types/Blog"].definition. detail: value_mismatch (expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }\n'). affected_count 2 (also appsync-graphql-api-and-type-lifecycle#task#5). The api id inside the ARN is a per-run value and is not part of the requirement.

2) Cause 2 - fixture appsync-list-types-after-create-type, step task#5, operation ListTypes, 200 vs 200, verdict mismatch, owned_by [], stub_path "". detail: $.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/c4k72f4siyfokbcu6xombkfknt/types/Widget"].definition: value_mismatch (expected 'type Widget\n\n{\nid: ID!\n\n\nname: String\n\n}\n', got 'type Widget { id: ID! name: String }'). affected_count 2 (also appsync-list-types-after-create-type#task#6).

3) Cause 3 - fixture appsync-list-types-lifecycle, step task#3, operation ListTypes, 200 vs 200, verdict mismatch, owned_by [], stub_path "". detail: $.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/myb7xm2bjv3nfczs2if4jb7jam/types/Query"].definition: value_mismatch (expected 'type Query\n\n{\ngetItem: String\n\n}\n', got 'type Query {\n  getItem: String\n}'). affected_count 1.

Model evidence (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json): ListTypes GET /v1/apis/{apiId}/types code 200; required query member format of enum TypeDefinitionFormat (SDL|JSON), maxResults @range 0..25 default 0, nextToken @length 1..65536 @pattern ^[\S]+$, paginated items=types; errors BadRequestException(400), UnauthorizedException(401), NotFoundException(404), ConcurrentModificationException(409), InternalFailureException(500). Shape Type members: name (ResourceName @length 1..65536 @pattern ^[_A-Za-z][_0-9A-Za-z]*$), description, arn, definition (String, doc "The type definition"), format (TypeDefinitionFormat). No trait on definition constrains its text, so the rendering requirement above is taken from the recorded responses.

## Steps to Reproduce
1. Clone the repo, build/boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture appsync-graphql-api-and-type-lifecycle, step task#4 (its cluster also covers task#5). Chain: CreateGraphqlApi, then CreateType with a single-line SDL object type carrying two fields, then ListTypes with format=SDL. Expect each types[].definition re-rendered as 'type <Name>\n\n{\n<f1>: <T1>\n\n\n<f2>: <T2>\n\n}\n'; the emulator returns the submitted one-line string.
3. Cause 2: replay fixture appsync-list-types-after-create-type, step task#5 (cluster also covers task#6). Same shape; the emulator returns the one-line string and additionally drops the terminating newline.
4. Cause 3: replay fixture appsync-list-types-lifecycle, step task#3. Create the type through StartSchemaCreation instead of CreateType, then ListTypes with format=SDL. Expect AWS's layout; the emulator returns its own two-space-indented rendering with no terminating newline.
5. All three fixture steps must pass before this issue is closed.
