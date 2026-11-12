Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters appsync__cloudwatch.obs-appsync-2-realtime-connect-rejected-counts-client-error
Issue-Key: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters|_poll_metrics#44|AssertionError|raise AssertionError(
Issue-Key: appsync__cloudwatch.obs-appsync-2-realtime-connect-rejected-counts-client-error|__init__#21|PublishedUrlRejected|_t = urllib.parse.urlsplit(ctx.endpoint_url(url))
Issue-Commit: c9a86881ef96f093104e2802f866de6afbf03a24

# AppSync/CloudWatch observability: ActiveSubscriptions never returns to 0 after unsubscribe, and the realtime endpoint is published on an unreachable AWS host (2 causes)

> _Note: this issue was identified against the code base at commit `c9a86881ef96f093104e2802f866de6afbf03a24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Two net-new causes in the AppSync observability surface, grounded in the two failing checks listed under the appsync__cloudwatch heading. Both must be closed before this issue is submitted.

Cause 1 of 2: the AWS/AppSync ActiveSubscriptions gauge is never decremented when a subscription is unsubscribed on a still-open connection.
AWS: ActiveSubscriptions in namespace AWS/AppSync, dimension [EventAPIId], is a gauge of the subscriptions currently held on an Event API. After a subscriber sends {"type":"unsubscribe"} and the server acknowledges with unsubscribe_success, the minutes at/after that acknowledgement read ActiveSubscriptions 0 while ActiveConnections still reads >= 1 (the connection is deliberately held open), and it stays 0 (recorded behavior only; the model carries no metric definitions).
Emulator: the only minute ever published for the API reads ActiveSubscriptions Maximum 1 — the gauge is published once, at subscribe time, and no minute at/after the acknowledged unsubscribe reads 0 even though ActiveConnections for the same API is still >= 1. The check polled 600s and never saw the drop; ListMetrics does catalogue ActiveSubscriptions/ActiveConnections under [EventAPIId], so the metric exists but its value never follows the subscription lifecycle.
Why: dashboards and alarms built on ActiveSubscriptions read a monotonic, never-falling value, so an operator cannot see subscriptions drain, and any autoscaling/alarm rule keyed on it latches high forever.
Fix: the Event API realtime data plane's metric emission — decrement/republish the ActiveSubscriptions gauge for [EventAPIId] when a subscription ends (unsubscribe and connection close), and keep publishing the gauge for subsequent minutes while the connection is open.
Traces: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters.

Cause 2 of 2: the realtime endpoint a GraphQL API publishes is an AWS hostname the emulator does not answer on, so no realtime (WebSocket) observability flow can run.
AWS: a GraphQL API's realtime endpoint (GraphqlApi$uris REALTIME / GraphqlApi$dns REALTIME, Smithy doc "The URIs.") is the address the API's WebSocket plane is actually served at — on AWS <host-label>.appsync-realtime-api.<region>.amazonaws.com. For an emulator the requirement is that the published address is one the emulator is reached on, exactly as this service already does for an Event API (state::event_api_dns publishes the realtime socket it serves).
Emulator: publishes wss://<label>.appsync-realtime-api.<region>.amazonaws.com/graphql?header=...&payload=..., whose host is not one of the emulator's own hosts; the client dialling it is refused ("a customer of the emulator needs ws(s)://127.0.0.1:<port>/<resource path>, or ws(s)://<own host>:<port>/<path>").
Why: a client that connects where the API says to connect never reaches the emulator, so realtime connect/connection_init behavior and the ConnectRequests/ConnectSuccess/ConnectClientError counters that depend on it are unobservable.
Fix: the GraphQL API endpoint publication (state::api_dns in services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/state.rs, lines ~2989-3000, called from ops/create_graphql_api.rs line 264) must publish the host:port the emulator serves the API's realtime endpoint on. Same root cause as cause 3 of the issue filed for service 'appsync' (op-contract-create-graphql-api); it is repeated here because this check is listed under appsync__cloudwatch and would otherwise carry no replay key.
Traces: appsync__cloudwatch.obs-appsync-2-realtime-connect-rejected-counts-client-error.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (bootstrap_signal false — the emulator serves).

Cause 1 — [mismatch] appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters:
  AssertionError: a minute at/after <unsubscribe minute> reading ActiveSubscriptions == 0 while ActiveConnections >= 1 under [EventAPIId] not met within 600s. Last ListMetrics catalogue for [('EventAPIId', '<apiId>')]: {'ActiveConnections': [('EventAPIId',)], 'ActiveSubscriptions': [('EventAPIId',)], ...}. Last GetMetricStatistics series: {'ActiveSubscriptions': [(<minute>, 1.0, 1.0)], 'ActiveConnections': [(<minute>, 3.0, 1.0)]}
  i.e. after an acknowledged unsubscribe_success on a held-open connection, ActiveSubscriptions stays at Maximum 1 and no later minute is published. (The apiId, minute and port are per-run values.)

Cause 2 — [mismatch] appsync__cloudwatch.obs-appsync-2-realtime-connect-rejected-counts-client-error:
  simparity.published.PublishedUrlRejected: endpoint_url was given wss://<label>.appsync-realtime-api.us-west-2.amazonaws.com/graphql?header=...&payload=... (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs ws(s)://127.0.0.1:<port>/<resource path>, or ws(s)://<own host>:<port>/<path> on a per-resource listener

## Steps to Reproduce
1. Clone the repository and boot the emulator built from it.
2. Replay simparity-behavioral-aws-v1; both checks under Traces fail.
3. Cause 1 by hand: CreateApi (Event API) + CreateChannelNamespace with an onPublish handler, open the realtime endpoint, connection_init, subscribe to a channel, publish a few events, then send {"type":"unsubscribe"} and keep the socket open. Poll cloudwatch GetMetricStatistics for AWS/AppSync ActiveSubscriptions and ActiveConnections with dimension EventAPIId. Expect a minute at/after the unsubscribe where ActiveSubscriptions is 0 while ActiveConnections >= 1, staying 0; the emulator keeps reporting ActiveSubscriptions 1 and publishes no later minute.
4. Cause 2 by hand: CreateGraphqlApi(authenticationType=API_KEY), StartSchemaCreation with a subscription-bearing SDL, read graphqlApi.uris.REALTIME and dial it. The published host is <label>.appsync-realtime-api.<region>.amazonaws.com, which the emulator does not serve; it must publish the emulator's own host:port for that API's realtime endpoint.
