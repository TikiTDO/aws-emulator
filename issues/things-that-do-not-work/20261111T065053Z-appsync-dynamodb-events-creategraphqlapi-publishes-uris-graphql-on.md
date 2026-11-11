Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered
Issue-Key: appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered|_create_api#17|AssertionError|assert not (p := ctx.published_url_mismatch(uri, labels=("appsync-api",), path="/graphql")), p
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# appsync__dynamodb__events: CreateGraphqlApi publishes uris.GRAPHQL on an appsync-api.<region>.amazonaws.com host the emulator does not serve, so the resolver-error-in-a-200 delivery check never reaches a GraphQL call

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: CreateGraphqlApi publishes a GRAPHQL endpoint on a real-AWS host instead of the address the emulator is reached on, so the EventBridge -> AppSync -> DynamoDB pipeline this check builds cannot be driven at all. Failing check: appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered.

AWS: CreateGraphqlApi returns graphqlApi.uris, whose GRAPHQL entry is the endpoint a client (or an EventBridge rule target) sends GraphQL operations to, and on AWS it is a host the serving endpoint itself answers on. Smithy (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice): GraphqlApi$uris is a MapOfStringToString documented only as "The URIs.", so the model states nothing about host or scheme — recorded/structural behavior only: a published endpoint must be an address of the server that published it, which for this emulator means its own listener, i.e. http(s)://127.0.0.1:<port>/<resource path> or http(s)://<own host>:<port>/<path> on a per-resource listener.

Emulator: the published URI is https://<apiId>.appsync-api.<region>.amazonaws.com/graphql — a host outside the emulator. The harness guard ctx.published_url_mismatch(uri, labels=("appsync-api",), path="/graphql") refuses it with: "the emulator published <uri>: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The apiId, region and the harness port vary per run and are not part of the requirement; the invariant defect is that the announced host is an AWS-owned domain rather than the emulator's own.

Why: a client that wires anything to AppSync by reading uris.GRAPHQL configures an off-box target and every request leaves the emulator. Concretely, the whole subject of this check is unobservable: that a resolver error returned inside an HTTP 200 GraphQL response is a SUCCESSFUL EventBridge delivery (not dead-lettered and not counted as FailedInvocations) can never be exercised, because the API is unaddressable before the first mutation is sent.

Fix: the GraphqlApi URI projection in the appsync service crate — render uris (GRAPHQL/REALTIME) and the dns map from the emulator's own listener host and port rather than from an appsync-api.<region>.amazonaws.com template.

Related open issues, named for the fixer (none of them carries this check's replay key, so none gates it): 20261110T155738Z-appsync-dynamodb-events-the-graphqlapi-uri-published-to.md reports the same projection defect for the other three checks of this same group; 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md and 20261110T055930Z-appsync-graphql-api-endpoints-publish-aws-hostnames-the.md report it for other groups. This issue exists because the surviving check above is not covered by any of their replay keys.

Traces: appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered, failing in _create_api on the published-URI guard.

## Evidence
Expected: graphqlApi.uris["GRAPHQL"] on one of the emulator's own hosts, e.g. http(s)://127.0.0.1:<port>/<resource path>.

Actual ([mismatch] appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered):
  File "check_appsync__dynamodb__events_xsvc_appsync_2_resolver_error_in_a_200_response_is_a_successful_delivery_not_dead_lettered.py", line 350, in run
    api_id, uri = _create_api(
  File "...", line 171, in _create_api
    assert not (p := ctx.published_url_mismatch(uri, labels=("appsync-api",), path="/graphql")), p
  AssertionError: the emulator published https://pkd54lkypogbqbvisqmdb4f3h3.appsync-api.us-west-2.amazonaws.com/graphql: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:40335/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

(The apiId pkd54lkypogbqbvisqmdb4f3h3 and the port 40335 are this run's values; the requirement is the host family, not those values.)

Smithy evidence: GraphqlApi$uris documentation is "<p>The URIs.</p>" — the model constrains nothing about the host, so the requirement is the structural one above.

## Steps to Reproduce
1. appsync.CreateGraphqlApi (AWS_IAM auth), add a DynamoDB data source and a unit resolver for the mutation.
2. Read graphqlApi.uris["GRAPHQL"] from the CreateGraphqlApi response (or GetGraphqlApi).
3. Observe the host is <apiId>.appsync-api.<region>.amazonaws.com instead of the emulator's own host:port; a SigV4-signed POST to that URI, or an EventBridge rule target built from it, never reaches the emulator.
4. Replay appsync__dynamodb__events.xsvc-appsync-2-resolver-error-in-a-200-response-is-a-successful-delivery-not-dead-lettered.
