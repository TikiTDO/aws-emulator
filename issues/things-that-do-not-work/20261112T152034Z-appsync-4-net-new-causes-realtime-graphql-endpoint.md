Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.appsync-deleted-api-key-subscription-rejected appsync.appsync-subscription-delivers-mutation-to-realtime-subscriber appsync.appsync-subscription-filter-excludes-nonmatching-mutation appsync.deep-appsync-3-appsync-rt-invalidation-completes-only-matching-subscriber appsync.e2e-schema-merge-cross-source-query-flow appsync.op-contract-delete-function appsync.op-contract-list-resolvers
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.appsync-deleted-api-key-subscription-rejected|_post_json#47|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.appsync-subscription-delivers-mutation-to-realtime-subscriber|_post_json#28|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.appsync-subscription-filter-excludes-nonmatching-mutation|_post_json#29|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.deep-appsync-3-appsync-rt-invalidation-completes-only-matching-subscriber|_post_json#22|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync.e2e-schema-merge-cross-source-query-flow|run#26|AssertionError|assert "greetingOne" in merged_query_def, merged_query_def
Issue-Key: appsync.op-contract-delete-function|run#26|AssertionError|assert code_msg == "Function not found.", f"unexpected message: {code_msg!r}"
Issue-Key: appsync.op-contract-list-resolvers|_error#20|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: b674738a58d9d62ba939f6eca5e8d4760a2144d8

# appsync: 4 net-new causes — realtime/GraphQL endpoint URIs published on unserved real-AWS hosts, merged-API schema merge clobbers the sibling source's Query fields, DeleteFunction and ListResolvers not-found messages diverge

> _Note: this issue was identified against the code base at commit `b674738a58d9d62ba939f6eca5e8d4760a2144d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi, StartSchemaMerge, DeleteFunction, ListResolvers

## Description
Four independent root causes in the AppSync emulator, grounded in 7 failing behavioral checks. (Cause 1 is the same class of defect described by open issues 20261111T065707Z-appsync-3-net-new-causes-graphqlapi-uris-dns.md and 20261109T150433Z-appsync-4-net-new-causes-apicache-creating-state.md, but the checks below are not carried by either issue's replay key, so they are recorded here.)

Cause 1 of 4: CreateGraphqlApi/GetGraphqlApi publish GraphqlApi$uris on real-AWS hostnames the emulator does not serve, so the GraphQL and REALTIME data planes are unreachable.
AWS: Smithy — CreateGraphqlApi output GraphqlApi$uris is a MapOfStringToString keyed GRAPHQL and REALTIME; GetGraphqlApi returns the same map. AWS's contract is that those URIs are the addresses at which the API's data plane is actually served, and every client (and the AWS SDKs' AppSync realtime protocol) dials exactly the host the control plane published. Recorded behavior only for the host form: the recording shows a customer dialing the published uris value verbatim.
Emulator: the published value names an `<apiId>.appsync-api.<region>.amazonaws.com` host (e.g. `https://<26-char-apiId>.appsync-api.us-west-2.amazonaws.com/graphql`) — an address the emulator does not listen on. Replay rejects it with `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.appsync-api.<region>.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>`. The URI must be built from the listener the emulator is actually reached on (host and port from the request/serving configuration), not from a hard-coded AWS DNS template; the `/graphql` and `/graphql/realtime` paths and the apiId are the only parts that come from state.
Why: no client can ever reach the API. Every API-key subscription, realtime delivery, subscription-filter and cache-invalidation behavior is unverifiable and unusable — a customer gets DNS/connection failure (or, worse, a call out to real AWS) instead of their emulated API.
Fix: the CreateGraphqlApi / GetGraphqlApi handlers and the shared endpoint-URI construction they call (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_graphql_api.rs, .../get_graphql_api.rs and the URI builder in .../src/state.rs) must derive uris.GRAPHQL and uris.REALTIME from the emulator's own listener.
Traces: appsync.appsync-deleted-api-key-subscription-rejected, appsync.appsync-subscription-delivers-mutation-to-realtime-subscriber, appsync.appsync-subscription-filter-excludes-nonmatching-mutation, appsync.deep-appsync-3-appsync-rt-invalidation-completes-only-matching-subscriber.

Cause 2 of 4: StartSchemaMerge on a second source API replaces the merged API's Query type instead of composing both sources' fields into it.
AWS: Smithy — StartSchemaMerge(associationId, mergedApiIdentifier) returns sourceApiAssociationStatus, and a MERGE_SUCCESS means the source API's types have been composed into the MERGED API's served schema. GetType(apiId=<mergedApiId>, typeName="Query", format="SDL") then returns the merged API's own composed Query type, which must carry the fields contributed by EVERY association that reached MERGE_SUCCESS. Recorded behavior only for the composition rule's exact SDL layout.
Emulator: both associations report MERGE_SUCCESS and each ListTypesByAssociation shows its own source's field, but GetType on the merged API returns only the LAST-merged source's contribution — `type Query {\n  greetingTwo: String\n}` — with the first source's field (`greetingOne`) absent. The second merge overwrote the stored Query type rather than unioning its fields.
Why: a client that merges N source APIs loses every source but the last. Queries spanning two sources are rejected with "Cannot query field", so the whole merged-API feature silently produces a wrong schema while reporting MERGE_SUCCESS.
Fix: the StartSchemaMerge handler's schema-composition step and the merged-API schema it writes to state (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/start_schema_merge.rs and the merged-schema field in .../src/state.rs) must union each association's type contributions per type name, not assign.
Traces: appsync.e2e-schema-merge-cross-source-query-flow.

Cause 3 of 4: DeleteFunction on an unknown functionId returns GetFunction's not-found message instead of AWS's DeleteFunction message.
AWS: Smithy — DeleteFunction errors include NotFoundException (client, HTTP 404). For DeleteFunction the message is the fixed text `Function not found.` — it has NO variable parts (the functionId is not interpolated). Note this is deliberately different from GetFunction, whose message does interpolate the id (`function id <functionId> not found`, with <functionId> taken from the request); the emulator already gets GetFunction right.
Emulator: a DeleteFunction for an already-deleted functionId returns NotFoundException with message `function id jz5pgbleljg2bctcybwkkk7kav not found` — i.e. the GetFunction template, with the request's functionId interpolated — instead of the fixed `Function not found.`
Why: clients and tests that branch or assert on the refusal text of a double-delete see the wrong message; the error class is right but the wording is not AWS's, so message-matching retry/idempotency logic diverges.
Fix: the DeleteFunction handler's not-found refusal path (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_function.rs) must emit its own message rather than reusing the GetFunction one.
Traces: appsync.op-contract-delete-function.

Cause 4 of 4: ListResolvers on a typeName that does not exist in the schema returns 200 with an empty list instead of refusing with NotFoundException.
AWS: Smithy — ListResolvers(apiId, typeName) is GET /v1/apis/{apiId}/types/{typeName}/resolvers and models NotFoundException (client, HTTP 404) among its errors; the typeName is a path label naming an existing type, so an unknown type is a 404, not an empty page. The message is the fixed text `Type not found.` (no variable parts).
Emulator: `ListResolvers(apiId=<api>, typeName="TotallyUnknownType404")` returns HTTP 200 with body `{"resolvers": []}`.
Why: a client cannot distinguish "this type has no resolvers" from "this type does not exist", so typos and stale type names are silently swallowed; paginators and provisioning code that rely on the 404 to detect a missing type proceed on a false premise.
Fix: the ListResolvers handler must look the typeName up in the API's schema before paging and refuse an unknown one (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/list_resolvers.rs).
Traces: appsync.op-contract-list-resolvers.

## Evidence
Cause 1 — expected: uris.GRAPHQL / uris.REALTIME on a host:port the emulator serves (replay reports it needs `http(s)://127.0.0.1:<port>/<resource path>`). Actual: `https://jxywxr4ffbqhyhry24eydrjo2j.appsync-api.us-west-2.amazonaws.com/graphql`, rejected by replay with `simparity.published.PublishedUrlRejected: ... its host is not one of the emulator's own hosts`. Same rejection on 4 checks with 4 different apiIds, so the host template — not one recorded value — is the defect.

Cause 2 — expected: `GetType(apiId=<mergedApiId>, typeName="Query", format="SDL")` definition contains BOTH `greetingOne` (source 1) and `greetingTwo` (source 2) after both associations report MERGE_SUCCESS. Actual: `type Query {\n  greetingTwo: String\n}` — AssertionError `assert "greetingOne" in merged_query_def`. Each association's own ListTypesByAssociation view was correct, so the loss is in the merged API's composed schema.

Cause 3 — expected: DeleteFunction on a deleted functionId -> NotFoundException, Message exactly `Function not found.`. Actual: NotFoundException, Message `function id jz5pgbleljg2bctcybwkkk7kav not found` (fixed text `function id ` / ` not found` around the request's functionId — GetFunction's template). AssertionError: `unexpected message: 'function id jz5pgbleljg2bctcybwkkk7kav not found'`.

Cause 4 — expected: ListResolvers with an unknown typeName -> ClientError, Code `NotFoundException`, Message `Type not found.`, HTTP 404. Actual: HTTP 200, body `{'resolvers': []}` (ResponseMetadata HTTPStatusCode 200, content-length 16). AssertionError: `expected a ClientError, but the call SUCCEEDED and returned {... 'resolvers': []}`.

## Steps to Reproduce
Cause 1: appsync.CreateGraphqlApi(name=..., authenticationType="API_KEY") with a subscription-bearing schema, then read graphqlApi.uris["GRAPHQL"] / ["REALTIME"] and POST a GraphQL request (or open the realtime WebSocket) to that URI. The host is `<apiId>.appsync-api.<region>.amazonaws.com`, which the emulator does not serve. Replay: appsync.appsync-subscription-delivers-mutation-to-realtime-subscriber.

Cause 2: create two API_KEY source APIs, one exposing `Query.greetingOne` and the other `Query.greetingTwo`; create a MERGED api with mergedApiExecutionRoleArn; AssociateSourceGraphqlApi + StartSchemaMerge for each; wait for both associations to reach MERGE_SUCCESS; then GetType(apiId=<mergedApiId>, typeName="Query", format="SDL"). Only the second source's field is present. Replay: appsync.e2e-schema-merge-cross-source-query-flow.

Cause 3: CreateDataSource(type="NONE"), CreateFunction, DeleteFunction (succeeds), then DeleteFunction again with the same functionId; inspect Error.Message. Replay: appsync.op-contract-delete-function.

Cause 4: create an API whose schema declares only `type Query { getPost: String getComments: String }`, then ListResolvers(apiId=<api>, typeName="TotallyUnknownType404"). Replay: appsync.op-contract-list-resolvers.
