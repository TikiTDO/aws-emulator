Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__appsync.appsync-http-data-source-sigv4-signs-request-to-api-gateway
Issue-Key: apigateway__appsync.appsync-http-data-source-sigv4-signs-request-to-api-gateway|_post_json#38|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# apigateway__appsync: the GraphQL API's published uris.GRAPHQL is a real-AWS hostname, so an HTTP data source signing SigV4 to API Gateway can never be exercised

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
One root cause, grounded in the failing behavioral check of this service pairing.

=== Cause 1 of 1: CreateGraphqlApi publishes GraphqlApi$uris/$dns on hostnames the emulator does not serve ===
AWS: Smithy (appsync): CreateGraphqlApi (POST /v1/apis, code 200) returns GraphqlApi with `uris` and `dns` (both MapOfStringToString, keys GRAPHQL and REALTIME). uris.GRAPHQL is the address a client POSTs its GraphQL document to — on AWS it is the API's live data-plane endpoint, and it is the only address the service publishes for it. An emulator must publish an address it itself answers on.
Emulator: uris.GRAPHQL is emitted as https://<26-char token>.appsync-api.<region>.amazonaws.com/graphql, a host outside the emulator. The check's first data-plane request is refused before it leaves: simparity.published.PublishedUrlRejected: "endpoint_url was given https://<token>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". (The token, host and port differ every run and are not requirements.)
Why: the whole cross-service behavior under test is unreachable — an AWS_IAM-authorized HTTP data source must SigV4-sign AppSync's outbound call to an API Gateway stage so a guarded method admits the signed request and refuses the unsigned one. Because the API's own endpoint is unaddressable, a client cannot drive a single query through the HTTP data source, and the emulator's SigV4 signing of HTTP-data-source egress is never observable.
Fix: the GraphqlApi projection (uris/dns construction) in the CreateGraphqlApi / GetGraphqlApi handlers of the appsync service, plus the GraphQL data-plane listener those URIs must name. NOTE: open issue 20261110T215409Z-appsync-10-net-new-causes-graphql-realtime-endpoints.md (Cause 1) reports the same published-URI defect for appsync-only checks; this cross-service check was not marked owned by it and may be closed by the same fix.
Traces: apigateway__appsync.appsync-http-data-source-sigv4-signs-request-to-api-gateway.

## Evidence
Expected: graphqlApi['uris']['GRAPHQL'] names a host:port the emulator serves (e.g. http://127.0.0.1:<port>/<path>), so a POST of a GraphQL document reaches the API and AppSync in turn signs its HTTP-data-source call to the API Gateway stage with SigV4 (guarded method returns 200 for the signed call, 403 for the unsigned one).
Actual: graphqlApi['uris']['GRAPHQL'] = https://pjfakqvu4pbqyek3f3shwxvnfd.appsync-api.us-west-2.amazonaws.com/graphql; the request is refused by the harness with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34487/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener"). The token/host/port shown are per-run values.

## Steps to Reproduce
1) apigateway: create a REST API with an open method and an AWS_IAM-guarded method, deploy a stage. 2) appsync.create_graphql_api(name=..., authenticationType='API_KEY'), start_schema_creation with an SDL exposing the three fields, create an HTTP data source pointing at the stage's invoke URL with an authorizationConfig of AWS_IAM, create UNIT resolvers. 3) Read graphqlApi['uris']['GRAPHQL'] and POST {"query":"{ guardedViaSigned }"} with header x-api-key. The published URI is a *.appsync-api.<region>.amazonaws.com hostname rather than the emulator's own host:port, so the call never reaches the emulator.
