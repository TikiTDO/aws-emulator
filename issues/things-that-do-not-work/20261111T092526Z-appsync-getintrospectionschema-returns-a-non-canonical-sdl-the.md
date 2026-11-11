Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-eval-schema-creation-introspection
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-eval-schema-creation-introspection
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|GetIntrospectionSchema|2xx->2xx|$
Issue-Key: appsync|GetIntrospectionSchema|2xx->2xx|$headers.content-type
Issue-Key: appsync|GetIntrospectionSchema|2xx->2xx|$.data.__schema.directives
Issue-Key: appsync|GetIntrospectionSchema|2xx->2xx|$.data.__schema.types[*]
Issue-Key: appsync|GetIntrospectionSchema|2xx->2xx|$.data.__schema.types[*].description
Issue-Commit: 323587cc72041ca5bad5e7b036e306f86c24b041

# AppSync GetIntrospectionSchema returns a non-canonical SDL, the wrong content-type and an incomplete JSON introspection (2 causes)

> _Note: this issue was identified against the code base at commit `323587cc72041ca5bad5e7b036e306f86c24b041`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIntrospectionSchema

## Description
GetIntrospectionSchema serves the schema the caller uploaded rather than the schema AWS re-renders from its parsed form, and it labels the payload with the wrong media type. Both formats of the operation diverge: SDL is returned verbatim in source order with no `schema` block, and JSON omits the GraphQL introspection meta-types, the built-in scalar descriptions and the directive list.

Smithy basis for both causes — Smithy: GetIntrospectionSchema, GET /v1/apis/{apiId}/schema, http code 200; input apiId (@httpLabel, required), format (OutputType enum, @httpQuery "format", required, values SDL | JSON), includeDirectives (BooleanValue, @httpQuery); output schema (Blob, @httpPayload); errors GraphQLSchemaException(400), NotFoundException(404), UnauthorizedException(401), InternalFailureException(500).

=== Cause 1 of 2: the SDL payload is the uploaded source text, not AWS's canonical rendering, and is served as application/octet-stream (fixture appsync-eval-schema-creation-introspection, step task#4; same cluster also hits task#5 and task#6) ===
AWS: recorded behavior only (the model types the payload as an opaque Blob). AWS parses the schema at StartSchemaCreation and re-renders it for GetIntrospectionSchema, so the returned SDL is canonical and independent of how the caller wrote it: it begins with an explicit root-operation block ("schema {\n  query: Query\n}") followed by a blank line, then the type definitions, then the enum definitions; the fields of each type and the members of each enum are emitted in ascending name order rather than source order; and a directive argument is rendered with a space before its colon ("@deprecated(reason : \"use title\")"). Field/type names, directives and the @deprecated reason are values taken from the uploaded schema; the layout rules above are the fixed part. The response is labelled content-type: application/json.
Emulator: the uploaded SDL is echoed back — no `schema {...}` block, type and enum bodies in the author's source order (e.g. id/title/legacy and RED/GREEN instead of id/legacy/title and GREEN/RED), the enum definition emitted before the second object type instead of after it, and the directive argument rendered "@deprecated(reason: ...)" with no space before the colon. The response is labelled content-type: application/octet-stream.
Why: the SDL from GetIntrospectionSchema is what codegen and schema-diff tooling consumes. Because the emulator returns the caller's own bytes, a client cannot observe the normalization AWS performs — a schema round trip that is stable on AWS keeps changing shape under the emulator, and any diff against an AWS-produced SDL reports spurious changes. The octet-stream label makes HTTP clients and browsers treat a JSON/text payload as an opaque download instead of parsing or displaying it.
Fix: the GetIntrospectionSchema handler's SDL renderer (print from the stored parsed schema with the canonical root-operation block, name ordering and directive-argument spacing) and the response's content-type header.
Traces: appsync-eval-schema-creation-introspection#task#4 (representative), #task#5, #task#6.

=== Cause 2 of 2: the format=JSON introspection result omits the introspection meta-types, built-in scalar descriptions and __schema.directives (fixture appsync-eval-schema-creation-introspection, step task#7) ===
AWS: recorded behavior only. For format=JSON the payload is a standard GraphQL introspection document, {"data":{"__schema":{...}}}, that includes (i) descriptions on the built-in scalars ("Built-in ID", "Built-in String", and the equivalents for the other built-ins), (ii) the full set of introspection meta-types — __Schema, __Type, __Field, __InputValue, __EnumValue, __Directive and the related enums — each with its standard description and fields, and (iii) a populated __schema.directives list. The names, descriptions and field shapes of the meta-types are fixed GraphQL introspection text; only the user-defined types come from the uploaded schema. The response is labelled content-type: application/json.
Emulator: only the user-defined types plus bare built-in scalars are emitted: $.data.__schema.types[2].description and types[3].description are absent where AWS gives 'Built-in ID' / 'Built-in String', types[5] through types[13] (the __Schema/__Type/__Field/__InputValue/__EnumValue/__Directive meta-types and their enums) are missing entirely, and $.data.__schema.directives is absent. The payload is again labelled application/octet-stream.
Why: introspection documents are consumed by code generators, GraphQL IDEs and client-side schema validators, all of which read __schema.types for the meta-types and __schema.directives. Against the emulator those tools see a schema with no introspection system and no directives, so generation or validation fails or silently drops directive handling — while the same call against AWS succeeds.
Fix: the GetIntrospectionSchema handler's JSON introspection builder (emit the full introspection meta-type set, built-in scalar descriptions and the directives list) and the response's content-type header.
Traces: appsync-eval-schema-creation-introspection#task#7.

## Evidence
Both are mismatch clusters from trace set simparity-wire-aws-v1 (service appsync, operation GetIntrospectionSchema), neither carrying stub_path or owned_by.

Cause 1 — appsync-eval-schema-creation-introspection#task#4 (cluster also covers #task#5 and #task#6), expected 200 / got 200:
  $ value_mismatch. Expected body:
    schema {\n  query: Query\n}\n\ntype Note @aws_api_key @aws_iam {\n  id: ID!\n  legacy: String @deprecated(reason : "use title")\n  title: String\n}\n\ntype Query {\n  colors: [Color]\n  getNote(id: ID!): Note @aws_iam\n}\n\nenum Color {\n  GREEN\n  RED\n}\n
  Actual body:
    type Note @aws_api_key @aws_iam {\n  id: ID!\n  title: String\n  legacy: String @deprecated(reason: "use title")\n}\n\nenum Color {\n  RED\n  GREEN\n}\n\ntype Query {\n  getNote(id: ID!): Note @aws_iam\n  colors: [Color]\n}\n
  So: the leading `schema { query: Query }` block is absent; fields are in source order (id/title/legacy) not name order (id/legacy/title); enum members are RED/GREEN not GREEN/RED; the enum precedes `type Query` instead of following it; `@deprecated(reason: ...)` lacks the space before the colon.
  $headers.content-type value_mismatch (expected 'application/json', got 'application/octet-stream').

Cause 2 — appsync-eval-schema-creation-introspection#task#7, expected 200 / got 200. Diverging keys reported by the replay: $.data.__schema.types[2].description (expected 'Built-in ID', got None), $.data.__schema.types[3].description (expected 'Built-in String', got None), $.data.__schema.types[5] through types[13] missing — types[5] expected the OBJECT '__Schema' with description 'A GraphQL Introspection defines the capabilities of a GraphQL server. It exposes all available types and directives on the server, the entry points for query, mutation, and subscription operations.' and its types/queryType/mutationType/directives/subscriptionType fields — $.data.__schema.directives missing, and $headers.content-type diverging as in Cause 1.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).

Cause 1: replay appsync-eval-schema-creation-introspection step task#4 (also task#5, task#6) — create_graphql_api, start_schema_creation with the fixture's SDL (a type whose fields are not in name order, an enum whose members are not in name order, a @deprecated directive), then get_introspection_schema(apiId=..., format='SDL'). Compare the returned bytes with the recorded canonical SDL and inspect the response's content-type header.
Cause 2: replay the same fixture's step task#7 — get_introspection_schema(apiId=..., format='JSON', includeDirectives=True) on the same API and check data.__schema.types for the __Schema/__Type/__Field/__InputValue/__EnumValue/__Directive meta-types, the built-in scalar descriptions, and data.__schema.directives; also inspect the content-type header.
