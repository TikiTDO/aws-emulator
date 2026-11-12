Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudwatch.obs-appsync-2-graphql-realtime-lifecycle-counters
Issue-Key: appsync__cloudwatch.obs-appsync-2-graphql-realtime-lifecycle-counters|_fill#39|Timeout|return False
Issue-Commit: b674738a58d9d62ba939f6eca5e8d4760a2144d8

# appsync__cloudwatch: AppSync realtime lifecycle metrics (ConnectSuccess, SubscribeSuccess, PublishDataMessageSuccess, Unsubscribe/DisconnectSuccess, ActiveSubscriptions, ActiveConnections) are never published to AWS/AppSync

> _Note: this issue was identified against the code base at commit `b674738a58d9d62ba939f6eca5e8d4760a2144d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: AppSync serves the realtime WebSocket data plane but emits no AWS/AppSync realtime metrics into CloudWatch, so ListMetrics never catalogues them and GetMetricStatistics never returns a datapoint.
AWS: recorded behavior only (CloudWatch metric emission is not in the Smithy model; AWS documents these in the AppSync monitoring guide). The recording shows that after a realtime subscriber connects, subscribes, receives filtered pushes, unsubscribes and disconnects, the namespace `AWS/AppSync` carries the metric names ConnectSuccess, SubscribeSuccess, PublishDataMessageSuccess, UnsubscribeSuccess, DisconnectSuccess, ActiveSubscriptions and ActiveConnections, each published under exactly the single dimension `GraphQLAPIId` = the api's apiId, at 60-second period. The counters are exact (one connect => ConnectSuccess Sum 1; one subscribe => SubscribeSuccess Sum 1; PublishDataMessageSuccess counts only DELIVERED frames, so a subscription-filter-excluded mutation is not counted), and ActiveSubscriptions/ActiveConnections are per-minute gauges sampled while the socket is held open (ActiveSubscriptions falls to 0 after a stop while ActiveConnections stays >= 1).
Emulator: the realtime handshake, connection_ack and start_ack all succeed, but no AWS/AppSync datapoint ever appears for the api's GraphQLAPIId dimension. The check polls ListMetrics + GetMetricStatistics every 20 s and is still waiting for the first `ActiveSubscriptions` sample when the 600 s check budget expires — reported as `Check timed out after 600.0s`. Nothing is published at any point in the realtime lifecycle.
Why: a client cannot observe or alarm on its AppSync realtime traffic at all. Dashboards read empty, CloudWatch alarms on ConnectSuccess/ActiveConnections never leave INSUFFICIENT_DATA, and any automation that gates on a realtime counter hangs forever instead of seeing a number — exactly the hang this check recorded.
Fix: the AppSync realtime connection/subscription handling must publish these counters and gauges into the CloudWatch emulator's metric store under namespace AWS/AppSync with the single dimension GraphQLAPIId — in the AppSync service's realtime data-plane handling and its CloudWatch metric-emission path (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/), with 60-second periods and delivery-only counting for PublishDataMessageSuccess.
Traces: appsync__cloudwatch.obs-appsync-2-graphql-realtime-lifecycle-counters.

## Evidence
Expected: after one realtime connect + one filtered subscribe + 3 matching and 1 filter-excluded mutation + stop + close, `ListMetrics(Namespace="AWS/AppSync", Dimensions=[{GraphQLAPIId: <apiId>}])` catalogues ConnectSuccess, SubscribeSuccess, PublishDataMessageSuccess, UnsubscribeSuccess, DisconnectSuccess, ActiveSubscriptions and ActiveConnections under exactly the (GraphQLAPIId,) dimension set, and `GetMetricStatistics(Period=60, Statistics=[Sum, Maximum])` yields ConnectSuccess Sum 1, SubscribeSuccess Sum 1, UnsubscribeSuccess Sum 1, DisconnectSuccess Sum 1, PublishDataMessageSuccess Sum 3 (the excluded mutation NOT counted), ActiveSubscriptions Maximum >= 1 while subscribed and 0 after the stop with ActiveConnections Maximum >= 1.

Actual: no datapoint ever arrives. The WebSocket upgrade returned 101, connection_ack and start_ack were received, so the realtime plane is live — yet the first poll loop (waiting only for `ActiveSubscriptions` Maximum >= 1) never satisfies its predicate and the check is killed: `Error: Check timed out after 600.0s`.

## Steps to Reproduce
1) appsync.CreateGraphqlApi(authenticationType="API_KEY"); StartSchemaCreation with a schema exposing Mutation.addPost and Subscription.onAddPost @aws_subscribe(mutations:["addPost"]).
2) CreateDataSource(type="NONE"); CreateResolver for Mutation.addPost (APPSYNC_JS echo) and for Subscription.onAddPost (APPSYNC_JS calling extensions.setSubscriptionFilter on channel == 'alpha'); CreateApiKey.
3) Open the realtime WebSocket to uris.REALTIME with the base64 header/payload query params, send connection_init, then a `start` for `subscription OnAdd { onAddPost { id channel body } }`.
4) Poll `cloudwatch.ListMetrics(Namespace="AWS/AppSync", Dimensions=[{"Name":"GraphQLAPIId","Value":<apiId>}])` and `cloudwatch.GetMetricStatistics(..., MetricName="ActiveSubscriptions", Period=60, Statistics=["Sum","Maximum"])`. No metric name is catalogued and no datapoint is returned, indefinitely.
Replay: appsync__cloudwatch.obs-appsync-2-graphql-realtime-lifecycle-counters.
