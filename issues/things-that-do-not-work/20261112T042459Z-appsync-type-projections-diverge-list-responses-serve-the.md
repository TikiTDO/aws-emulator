Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-graphql-api-and-type-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-graphql-api-and-type-lifecycle appsync-list-types-after-create-type appsync-list-types-lifecycle appsync-list-types-by-association-json-format appsync-list-types-by-association-lifecycle appsync-schema-type-management
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/dqfnsivb6gwenfnbhi2aaypfad/types/Blog"].definition
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/ouulaz5o4yyw3gc6vkk3rrwywu/types/Widget"].definition
Issue-Key: appsync|ListTypes|2xx->2xx|$.types[arn="arn:aws:appsync:us-west-2:000000000000:apis/j7pcjzludc3heihxhy4c6us3ku/types/Query"].definition
Issue-Key: appsync|ListTypesByAssociation|2xx->2xx|$.types[*]
Issue-Key: appsync|UpdateType|2xx->2xx|$.type.name
Issue-Commit: 97bcd61576aa0f22fd1da9220056da036e0a415a

# AppSync Type projections diverge: list responses serve the stored definition instead of AWS's rendered form, and UpdateType emits a member AWS omits (5 causes)

> _Note: this issue was identified against the code base at commit `97bcd61576aa0f22fd1da9220056da036e0a415a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTypes

## Description
The emulator's AppSync type-listing and type-update responses do not build Type bodies the way AWS does. AWS re-renders Type$definition from its own parsed type model for the requested TypeDefinitionFormat; the emulator instead serves whatever text it stored (the client's raw SDL, or its own pretty-printer's output) and, on UpdateType, adds a member AWS leaves absent. Five independent causes, all service=appsync, all net-new (none stub-blocked, none owned by an open issue). All five must be closed before this issue is submitted.

Related open issues that do NOT cover these: 20261108T071728Z-appsync-type-operations-diverge-deleted-types-stay-readable.md Cause 2 covers the same non-rendered definition on **GetType** only (its replay keys are appsync|GetType|...), so the ListTypes / ListTypesByAssociation / UpdateType projections below are uncovered; 20261029T200818Z-appsync-response-projections-omit-handlerconfigs-and-emit-members.md covers absent/extra members on other appsync shapes, not Type; 20261111T092526Z-appsync-getintrospectionschema-returns-a-non-canonical-sdl-the.md covers GetIntrospectionSchema, a different operation.

Cause 1 of 5 - ListTypes (format=SDL) returns the submitted single-line definition verbatim instead of AWS's re-rendered SDL (fixture appsync-graphql-api-and-type-lifecycle, step task#4).
AWS: recorded behavior only. Smithy: ListTypes = GET /v1/apis/{apiId}/types with required query `format` (TypeDefinitionFormat = SDL|JSON), output `types: TypeList` + `nextToken`; Type members are name/description/arn/definition/format and Type$definition is documented only as "The type definition" String with no formatting trait. The recording shows AWS does not echo the client's text: it re-prints the parsed type, one token group per line, as `type <Name>\n\n{\n` then each field as `<field>: <Type>` followed by a blank line, consecutive fields separated by an extra blank line, then `}\n` - i.e. the document is newline-terminated and never single-line. The type and field names in any recording are request values; the layout is the requirement.
Emulator: status 200 and every other Type member correct, but `$.types[<type>].definition` is the raw request string - expected `type <Name>\n\n{\n<field1>: <Type1>\n\n\n<field2>: <Type2>\n\n}\n`, got `type <Name> { <field1>: <Type1> <field2>: <Type2> }\n` (recorded instance: expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }\n').
Why: a client that diffs, hashes or line-parses the schema returned by ListTypes - schema-drift detection in CI, SDL round-trips through the `format` parameter, codegen that splits the definition into lines - sees a document AWS would never emit, so comparison against a real account fails although nothing changed.
Fix: the Type response projection used by the ListTypes handler (shared with CreateType/UpdateType/GetType) - render Type$definition from the parsed type model for the requested format instead of returning the stored request string.
Traces: appsync-graphql-api-and-type-lifecycle#task#4, appsync-graphql-api-and-type-lifecycle#task#5.

Cause 2 of 5 - the same ListTypes SDL divergence on a type created through CreateType in a second fixture (fixture appsync-list-types-after-create-type, step task#5).
AWS: as Cause 1 - the re-rendered, newline-terminated multi-line SDL document.
Emulator: returns the submitted one-line SDL and, here, without even a trailing newline: expected 'type Widget\n\n{\nid: ID!\n\n\nname: String\n\n}\n', got 'type Widget { id: ID! name: String }' ('Widget', 'id', 'name' are request values). This is a distinct cluster/replay key from Cause 1 and will not be exercised by replaying Cause 1's step.
Why: same as Cause 1; additionally the missing trailing newline means even a client that normalises internal whitespace still sees a different document.
Fix: same projection as Cause 1; the renderer must terminate the document with a newline for every type it emits.
Traces: appsync-list-types-after-create-type#task#5, appsync-list-types-after-create-type#task#6.

Cause 3 of 5 - ListTypes serves the emulator's own pretty-printed SDL for a schema-derived type, which is also not AWS's layout (fixture appsync-list-types-lifecycle, step task#3).
AWS: as Cause 1. For a single-field type the rendered document is `type <Name>\n\n{\n<field>: <Type>\n\n}\n`.
Emulator: emits its own two-space-indented printer output instead - expected 'type Query\n\n{\ngetItem: String\n\n}\n', got 'type Query {\n  getItem: String\n}' ('Query', 'getItem' are schema values). This shows the fault is not only "echo the request": where the emulator does render, it renders with a layout of its own and still without the terminating newline.
Why: the definition AWS returns is a stable, service-defined rendering; a client cannot match either the echoed form (Causes 1-2) or this indented form, so no ListTypes response is byte-comparable with AWS.
Fix: one shared SDL renderer behind the Type projection, used for types derived from a StartSchemaCreation schema as well as for types created via CreateType.
Traces: appsync-list-types-lifecycle#task#3.

Cause 4 of 5 - ListTypesByAssociation (format=JSON) returns a JSON introspection document of the wrong shape (fixture appsync-list-types-by-association-json-format, step task#7).
AWS: recorded behavior only. Smithy: ListTypesByAssociation = GET /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}/types with required query `format`, output `types: TypeList`; nothing in the model constrains the JSON text. The recording shows AWS emits a COMPACT (no newlines, no indentation) JSON object per type whose members are, in order, `name`, `kind`, `fields`, `interfaces`, `directives`; each field entry carries `name`, `type`, `args`, `directives`, `isDeprecated`; a field's `type` carries only the members that have values (e.g. `{"kind":"NON_NULL","ofType":{"name":"ID"}}`). AWS omits absent members entirely rather than emitting them as null, and always includes the `directives` arrays.
Emulator: status 200, but the type element does not compare equal - the definition is a pretty-printed, two-space-indented document with a different member set: it adds explicit nulls AWS omits (`description`, `deprecationReason`, `inputFields`, `enumValues`, `possibleTypes`, `"name": null` inside a NON_NULL wrapper), adds `"kind": "SCALAR"` on leaf types, and omits the `directives` arrays AWS includes. (The replay reports `$.types[0]: missing` / `$.types[0]: extra` because the whole element differs; the ARN inside the element also differs only by the per-run api id, which is NOT part of the requirement.)
Why: a client that parses the JSON introspection per type - schema stitching for merged APIs, tooling that feeds the document back into a GraphQL introspection consumer - gets null-valued members where AWS says absent and loses the `directives` lists entirely, so merged-API schema inspection behaves differently against the emulator than against AWS.
Fix: the JSON branch of the Type definition serializer behind ListTypesByAssociation (shared with ListTypes/GetType format=JSON) - emit compact JSON with AWS's member set and skip members that have no value.
Traces: appsync-list-types-by-association-json-format#task#7, appsync-list-types-by-association-json-format#task#8, appsync-list-types-by-association-lifecycle#task#7.

Cause 5 of 5 - UpdateType's response includes Type$name, which AWS leaves absent (fixture appsync-schema-type-management, step task#4).
AWS: recorded behavior only. Smithy: UpdateType = POST /v1/apis/{apiId}/types/{typeName}, output `type: Type`; Type$name is an optional member (ResourceName, @length 1..65536, @pattern ^[_A-Za-z][_0-9A-Za-z]*$). The recording shows AWS's UpdateType response projects the Type WITHOUT `name` - the member is absent from the body (GetType, by contrast, does carry it).
Emulator: answers 200 with every other member matching, but splices the request's typeName in as `$.type.name` - expected None (member absent), got the updated type's name (recorded instance: 'Blog', which is the request's typeName, not part of any template).
Why: a client that uses member presence to tell responses apart, or that round-trips the returned Type object back into a request, sees a member AWS never sends; strict response validators treat the extra member as an unexpected field.
Fix: the UpdateType handler's Type response projection - omit `name` from the UpdateType output, matching the operation's recorded projection rather than reusing GetType's.
Traces: appsync-schema-type-management#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, data listener accepting). 733 steps, 499 passed, 224 mismatched, score 68.1%, 75 divergence clusters; 69 of them are owned by open issues and none is stub-blocked, leaving these 5 net-new clusters (all service=appsync):

1) appsync-graphql-api-and-type-lifecycle#task#4 - ListTypes, expected 200, actual 200. divergence_key service=appsync|ListTypes|status=200->200|keys=$.types[...].definition. detail: $.types[arn="...types/Blog"].definition: value_mismatch (expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }\n'). affected_count 2: also appsync-graphql-api-and-type-lifecycle#task#5.

2) appsync-list-types-after-create-type#task#5 - ListTypes, 200 vs 200. detail: $.types[arn="...types/Widget"].definition: value_mismatch (expected 'type Widget\n\n{\nid: ID!\n\n\nname: String\n\n}\n', got 'type Widget { id: ID! name: String }'). affected_count 2: also appsync-list-types-after-create-type#task#6.

3) appsync-list-types-lifecycle#task#3 - ListTypes, 200 vs 200. detail: $.types[arn="...types/Query"].definition: value_mismatch (expected 'type Query\n\n{\ngetItem: String\n\n}\n', got 'type Query {\n  getItem: String\n}'). affected_count 1.

4) appsync-list-types-by-association-json-format#task#7 - ListTypesByAssociation, 200 vs 200. divergence_key service=appsync|ListTypesByAssociation|status=200->200|keys=$.types[0]. detail: $.types[0]: missing / extra - expected definition '{"name":"Order","kind":"OBJECT","fields":[{"name":"id","type":{"kind":"NON_NULL","ofType":{"name":"ID"}},"args":[],"directives":[],"isDeprecated":false},{"name":"total","type":{"name":"Float"},"args":[],"directives":[],"isDeprecated":false}],"interfaces":[],"directives":[]}' (format JSON), got a pretty-printed document with "description": null, "deprecationReason": null, "inputFields": null, "enumValues": null, "possibleTypes": null, "kind": "SCALAR"/"name": null members and no "directives" arrays. affected_count 3: also appsync-list-types-by-association-json-format#task#8, appsync-list-types-by-association-lifecycle#task#7. The differing api id inside the element ARN is a per-run value and is not asserted as a requirement here.

5) appsync-schema-type-management#task#4 - UpdateType, 200 vs 200. divergence_key service=appsync|UpdateType|status=200->200|keys=$.type.name. detail: $.type.name: type_mismatch (expected None, got 'Blog'). affected_count 1.

Model evidence (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json): ListTypes GET /v1/apis/{apiId}/types, required format query (TypeDefinitionFormat=SDL|JSON), paginated on types/nextToken/maxResults(0..25), errors BadRequestException(400)/NotFoundException(404)/ConcurrentModificationException(409)/UnauthorizedException(401)/InternalFailureException(500). ListTypesByAssociation GET /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}/types, same format query and error set. UpdateType POST /v1/apis/{apiId}/types/{typeName}, output type: Type. Type = {name: ResourceName, description, arn, definition: String "The type definition", format: TypeDefinitionFormat}. No formatting, serialization or required-member trait on Type$definition or Type$name, so Causes 1-5 are grounded in recorded behavior.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay appsync-graphql-api-and-type-lifecycle step task#4 (and task#5). CreateGraphqlApi, CreateType with a single-line SDL object type, then ListTypes?format=SDL. Expect each types[].definition re-rendered as 'type <Name>\n\n{\n<field>: <Type>\n\n...\n}\n'; the emulator returns the submitted one-line string.
3. Cause 2: replay appsync-list-types-after-create-type step task#5 (and task#6). Same shape; the emulator additionally drops the terminating newline.
4. Cause 3: replay appsync-list-types-lifecycle step task#3. Create the type through a schema, then ListTypes?format=SDL. Expect AWS's layout; the emulator returns its own two-space-indented rendering.
5. Cause 4: replay appsync-list-types-by-association-json-format step task#7 (and task#8, appsync-list-types-by-association-lifecycle#task#7). Associate a source API with a merged API, then ListTypesByAssociation?format=JSON. Expect one compact JSON introspection object per type with members name/kind/fields/interfaces/directives and absent members omitted; the emulator returns a pretty-printed document with explicit nulls and no directives arrays.
6. Cause 5: replay appsync-schema-type-management step task#4. CreateType then UpdateType for that typeName. Expect the response's type object to have NO name member; the emulator includes name.
7. All five steps must pass before this issue is closed.
