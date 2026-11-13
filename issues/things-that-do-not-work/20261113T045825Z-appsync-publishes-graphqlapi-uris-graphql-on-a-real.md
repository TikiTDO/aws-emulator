Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.e2e-data-source-introspection-driven-resolver-flow
Issue-Key: appsync.e2e-data-source-introspection-driven-resolver-flow|_graphql_request_with_retries#51|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: 39c6dbb2798d7d5c6447151f9d119a36c56109b9

# AppSync publishes GraphqlApi$uris.GRAPHQL on a real-AWS appsync-api.<region>.amazonaws.com host the emulator does not serve, so an introspection-driven RDS resolver can never be exercised (1 cause)

> _Note: this issue was identified against the code base at commit `39c6dbb2798d7d5c6447151f9d119a36c56109b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: the GraphQL data-plane endpoint the emulator publishes for a GraphqlApi is not an address the emulator answers on, so every data-plane call a client makes from the published value leaves the emulator.

AWS: CreateGraphqlApi/GetGraphqlApi return GraphqlApi$uris, a map of endpoint kind -> absolute URL (Smithy: appsync GraphqlApi$uris, documented only as "<p>The URIs.</p>", target MapOfStringToString; the GRAPHQL key carries the API's GraphQL endpoint and REALTIME its WebSocket endpoint). The contract the recording exercises is that POSTing a GraphQL document to the published uris.GRAPHQL with the API key in x-api-key reaches the API's resolvers and returns a GraphQL response envelope {"data": ...} for the live row. On real AWS that URL happens to be https://<apiId>.appsync-api.<region>.amazonaws.com/graphql; the requirement is not that literal hostname but that the URL published by the serving endpoint is served by the serving endpoint. Recorded behavior only for the host form: nothing in the model fixes the host.

Emulator: CreateGraphqlApi returns uris.GRAPHQL = https://<apiId>.appsync-api.<region>.amazonaws.com/graphql — a real-AWS public hostname that this emulator does not serve and whose resolution/egress is outside it. The harness refuses to follow it: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The fixed text of that diagnosis is the mismatch itself; the apiId, region, host and port in it are per-run values. Everything upstream in the check succeeds (CreateDataSource on an RDS_HTTP_ENDPOINT source, StartDataSourceIntrospection/GetDataSourceIntrospection discovering the widgets table's id/name columns, StartSchemaCreation, CreateResolver, and GetResolver echoing dataSourceName) — the divergence is only the published endpoint, which is why the whole introspection-driven resolver round trip is unverifiable.

Why: a client that does exactly what the AWS API tells it to do — take uris.GRAPHQL and POST its query there — is sent to an address the emulator does not own. Against the emulator that call either fails DNS/egress or, worse, would be sent to real AWS. No GraphQL data-plane behavior (resolver execution, APPSYNC_JS request/response functions, RDS_HTTP_ENDPOINT data sources, API-key auth) can be exercised through the documented path, so the emulator's data plane is effectively unaddressable even though the control plane answers 200.

Fix: the GraphqlApi endpoint projection/serializer that fills GraphqlApi$uris (CreateGraphqlApi/GetGraphqlApi/UpdateGraphqlApi/ListGraphqlApis handlers and whatever endpoint-URL builder they share) must publish the emulator's own reachable scheme/host/port and resource path for GRAPHQL (and REALTIME), and the GraphQL data plane must be routed and served on the address it publishes.

Related open issues describe the same symptom for other checks but none carries this check's replay key: 20261110T055930Z-appsync-graphql-api-endpoints-publish-aws-hostnames-the.md, 20261110T153046Z-appsync-logs-appsync-the-graphql-api-s-published-uris-endpoint.md, 20261112T152034Z-appsync-4-net-new-causes-realtime-graphql-endpoint.md.

Traces: appsync.e2e-data-source-introspection-driven-resolver-flow.

## Evidence
Check appsync.e2e-data-source-introspection-driven-resolver-flow [mismatch].

Expected (AWS): api["uris"]["GRAPHQL"] is an endpoint served by the endpoint that published it; POST {"query": "query { getWidget(id: 1) { name } }"} with header x-api-key: <apiKey> returns 200 and a body with no "errors" key and data.getWidget.name equal to the row written through the RDS Data API ('widget-alpha').

Actual (emulator): uris.GRAPHQL = https://<apiId>.appsync-api.<region>.amazonaws.com/graphql — a host the emulator does not serve. Routing the published URL fails before any request is sent:

  File "context.py", line 603, in endpoint_route
    return self._published_target(url, "endpoint_url"), {}
  File "context.py", line 617, in _published_target
    raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: endpoint_url was given https://f2hwgsij6qm34o72y6stoa2h3b.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:36467/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

The apiId f2hwgsij6qm34o72y6stoa2h3b, the region us-west-2 and the port 36467 are per-run values; the defect is the host family (appsync-api.<region>.amazonaws.com) being published by an emulator reached on its own loopback listener.

Smithy cited: appsync GraphqlApi$uris (MapOfStringToString, doc "<p>The URIs.</p>"), returned by CreateGraphqlApi/GetGraphqlApi.

## Steps to Reproduce
1. secretsmanager.create_secret with a JSON {username,password} body; keep ARN.
2. rds.create_db_cluster (Engine aurora-mysql, EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration 0.5/2, DatabaseName=testdb) plus a db.serverless member instance; wait until available and until rds-data.execute_statement SELECT 1 succeeds.
3. Through rds-data.execute_statement: CREATE TABLE widgets (id INT NOT NULL PRIMARY KEY, name VARCHAR(64) NOT NULL) and INSERT (1, 'widget-alpha').
4. iam.create_role trusting appsync.amazonaws.com with rds-data:ExecuteStatement + secretsmanager:GetSecretValue on the cluster/secret.
5. api = appsync.create_graphql_api(name=..., authenticationType="API_KEY")["graphqlApi"]; appsync.create_api_key(apiId=api["apiId"]).
6. appsync.create_data_source(type="RELATIONAL_DATABASE", relationalDatabaseConfig.rdsHttpEndpointConfig -> the cluster ARN, testdb, the secret ARN).
7. appsync.start_data_source_introspection(rdsDataApiConfig={...}); poll get_data_source_introspection until SUCCESS and read the widgets model's field names.
8. start_schema_creation with a schema built from those field names; poll get_schema_creation_status to SUCCESS; create_resolver for Query.getWidget with APPSYNC_JS code using createMySQLStatement.
9. Read api["uris"]["GRAPHQL"] and POST {"query":"query { getWidget(id: 1) { name } }"} to it with x-api-key.
   Observed: the published URI is https://<apiId>.appsync-api.<region>.amazonaws.com/graphql, a host the emulator does not serve, so the request cannot be made at all.
   Expected: the URI is on the emulator's own host/port and returns {"data":{"getWidget":{"name":"widget-alpha"}}}.
