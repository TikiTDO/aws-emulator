Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb.appsync-iam-signed-mutation-reaches-realtime-subscriber-and-query-reads-it-back
Issue-Key: appsync__dynamodb.appsync-iam-signed-mutation-reaches-realtime-subscriber-and-query-reads-it-back|_connect_realtime#4|AssertionError|raise AssertionError(
Issue-Commit: b674738a58d9d62ba939f6eca5e8d4760a2144d8

# appsync__dynamodb: the realtime endpoint of an AWS_IAM GraphQL API refuses a correctly SigV4-signed WebSocket upgrade with HTTP 403 instead of completing it with 101

> _Note: this issue was identified against the code base at commit `b674738a58d9d62ba939f6eca5e8d4760a2144d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: on an API created with authenticationType AWS_IAM, the realtime endpoint rejects a SigV4-signed WebSocket upgrade with HTTP 403 rather than accepting it.
AWS: recorded behavior only (the realtime handshake is not modeled in the Smithy control plane; CreateGraphqlApi models authenticationType AWS_IAM and publishes uris.REALTIME). The recording shows AppSync's realtime protocol for an AWS_IAM api: the client SigV4-signs a POST of an empty body to the api's `/graphql/connect` path with signing name `appsync`, region = the api's region and `host` = the GraphQL endpoint's netloc, base64-encodes that signed header map into the `header` query parameter (with a base64 `payload`), and opens the WebSocket. A signature the service accepts on the equivalent signed HTTP GraphQL request must also be accepted here: the upgrade completes with HTTP 101 and the socket then answers `connection_init` with `connection_ack`. A 403 is reserved for a signature/credential the service rejects.
Emulator: the upgrade is refused with HTTP 403 even though the same credentials, signing name and region sign the API's HTTP GraphQL requests successfully — the realtime path either does not implement SigV4 verification for AWS_IAM apis or verifies a different canonical request than the one AppSync's realtime protocol specifies (notably the `/graphql/connect` path and the base64 `header` parameter rather than real request headers).
Why: the entire realtime plane of an AWS_IAM-authenticated AppSync API is unusable. A client that signs exactly as the AWS SDKs/amplify do is locked out, so an IAM-signed mutation can never be observed reaching a realtime subscriber, and there is no way to distinguish a genuine authorization failure from this blanket refusal.
Fix: the AppSync realtime handshake authorization must verify the base64 `header` SigV4 material for AWS_IAM apis against the canonical request AppSync's realtime protocol defines (signing name `appsync`, the api's region, host = the GraphQL endpoint netloc, POST of the empty connect body) and complete the upgrade with 101 when it validates — in the AppSync realtime data-plane auth handling under services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/.
Traces: appsync__dynamodb.appsync-iam-signed-mutation-reaches-realtime-subscriber-and-query-reads-it-back.

## Evidence
Expected: WebSocket upgrade to uris.REALTIME, carrying the base64-encoded SigV4-signed header map for a POST to the api's `/graphql/connect` path, returns HTTP 101 (and then `connection_ack` for `connection_init`). Actual: HTTP 403 — `AssertionError: the real-time endpoint refused the IAM-signed upgrade: HTTP 403, expected 101`. The same credentials/signer (`AWS4Auth(..., region, "appsync")`, include_hdrs accept/content-encoding/content-type/host/x-amz-*) are accepted by the emulator on the signed HTTP GraphQL POST, so the refusal is specific to the realtime handshake.

## Steps to Reproduce
1) Create a DynamoDB table and an AppSync service role granting access to it.
2) appsync.CreateGraphqlApi(name=..., authenticationType="AWS_IAM"); StartSchemaCreation; CreateDataSource(name="Notes", type="AMAZON_DYNAMODB", serviceRoleArn=<role>, dynamodbConfig={tableName, awsRegion}); CreateResolver for Mutation.createNote and Query.getNote (APPSYNC_JS).
3) Read uris.GRAPHQL and uris.REALTIME. SigV4-sign a POST of the empty connect body to uris.GRAPHQL's path replaced by the realtime connect path, signing name `appsync`, host = uris.GRAPHQL netloc; base64-encode the resulting header map into the `header` query parameter of uris.REALTIME, with `payload` = base64('{}').
4) Open the WebSocket to that URL. The handshake returns HTTP 403 instead of 101.
Replay: appsync__dynamodb.appsync-iam-signed-mutation-reaches-realtime-subscriber-and-query-reads-it-back.
