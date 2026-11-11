Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb__lambda.deep-appsync-1-appsync-conflict-lambda-handler-reject-surfaces-conflictunhandled-and-item-unchanged
Issue-Key: appsync__dynamodb__lambda.deep-appsync-1-appsync-conflict-lambda-handler-reject-surfaces-conflictunhandled-and-item-unchanged|_gql_client#8|PublishedUrlRejected|target, route = ctx.endpoint_route(uri)
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# appsync__dynamodb__lambda: the published GraphQL endpoint is a real-AWS hostname, so a LAMBDA conflict handler rejecting a write cannot be observed

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
One root cause, grounded in the failing behavioral check of this service pairing.

=== Cause 1 of 1: CreateGraphqlApi publishes GraphqlApi$uris/$dns on a hostname the emulator does not serve ===
AWS: Smithy (appsync): CreateGraphqlApi (POST /v1/apis, code 200) returns GraphqlApi with `uris` and `dns` (MapOfStringToString, keys GRAPHQL and REALTIME). uris.GRAPHQL is the address a client POSTs its mutation to; it is the only address AWS publishes for the API's data plane, so an emulator must publish an address it itself serves.
Emulator: uris.GRAPHQL is emitted as https://<26-char token>.appsync-api.<region>.amazonaws.com/graphql. The data-plane call is refused before it is sent: simparity.published.PublishedUrlRejected: "endpoint_url was given https://<token>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The token, host and port differ per run and are not requirements.
Why: the behavior under test is unreachable — with conflictHandler LAMBDA, a handler that returns a reject decision must surface a ConflictUnhandled error to the caller and leave the DynamoDB item unchanged. Because the API's endpoint is not an address the emulator serves, no mutation can be issued, so neither the ConflictUnhandled error type nor the no-write guarantee is observable to a client.
Fix: the GraphqlApi projection (uris/dns construction) in the CreateGraphqlApi / GetGraphqlApi handlers of the appsync service, plus the GraphQL data-plane listener those URIs must name. NOTE: open issue 20261110T155738Z-appsync-dynamodb-lambda-the-published-graphql-endpoint-is.md reports the same published-URI defect for the sibling check of this pairing; this check was not marked owned by it and may be closed by the same fix.
Traces: appsync__dynamodb__lambda.deep-appsync-1-appsync-conflict-lambda-handler-reject-surfaces-conflictunhandled-and-item-unchanged.

## Evidence
Expected: graphqlApi['uris']['GRAPHQL'] names a host:port the emulator serves, so the stale mutation reaches the API, the LAMBDA conflict handler's reject decision is returned as a ConflictUnhandled error in the GraphQL errors envelope, and the DynamoDB item is unchanged.
Actual: graphqlApi['uris']['GRAPHQL'] = https://cspkeqr54ihrkhx3ubeqpeuanp.appsync-api.us-west-2.amazonaws.com/graphql; the call is refused with simparity.published.PublishedUrlRejected "...its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34503/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener" (host/port/token are per-run values).

## Steps to Reproduce
1) dynamodb.create_table with versioned sync config; lambda.create_function for the conflict handler returning a reject decision; iam roles for both. 2) appsync.create_graphql_api(...), start_schema_creation, create_data_source(type='AMAZON_DYNAMODB', ...) with syncConfig conflictHandler=LAMBDA and the handler's ARN, create_resolver for the mutation. 3) Read graphqlApi['uris']['GRAPHQL'] and POST a mutation carrying a stale _version. The published URI is a *.appsync-api.<region>.amazonaws.com hostname rather than the emulator's own host:port, so the request never reaches the emulator.
