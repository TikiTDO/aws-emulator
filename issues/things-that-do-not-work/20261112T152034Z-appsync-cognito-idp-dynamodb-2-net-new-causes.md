Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-idp__dynamodb.appsync-mutation-triggers-subscription-positive appsync__cognito-idp__dynamodb.appsync-subscription-cognito-group-filter-positive appsync__cognito-idp__dynamodb.appsync-subscription-filter-mismatch-no-delivery-negative appsync__cognito-idp__dynamodb.appsync-subscription-payload-format appsync__cognito-idp__dynamodb.appsync-subscription-missing-subscribe-permission-negative
Issue-Key: appsync__cognito-idp__dynamodb.appsync-mutation-triggers-subscription-positive|_graphql_http#10|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cognito-idp__dynamodb.appsync-subscription-cognito-group-filter-positive|_graphql_http#11|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cognito-idp__dynamodb.appsync-subscription-filter-mismatch-no-delivery-negative|_graphql_http#11|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cognito-idp__dynamodb.appsync-subscription-payload-format|_graphql_http#10|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cognito-idp__dynamodb.appsync-subscription-missing-subscribe-permission-negative|_ws_connect#7|WebSocketBadStatusException|ws = websocket.create_connection(routed_url, subprotocols=["graphql-ws"], timeout=15, host=route_headers.get("Host"))
Issue-Commit: b674738a58d9d62ba939f6eca5e8d4760a2144d8

# appsync__cognito-idp__dynamodb: 2 net-new causes — GraphQL/REALTIME uris published on unserved real-AWS hosts, and the realtime WebSocket upgrade is refused 403 for a validly-authenticated Cognito user instead of deferring the @aws_auth group check to subscribe time

> _Note: this issue was identified against the code base at commit `b674738a58d9d62ba939f6eca5e8d4760a2144d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Two independent root causes, grounded in 5 failing behavioral checks. (Cause 1 is the same class of defect as open issue 20261111T061507Z-appsync-cognito-idp-dynamodb-the-graphql-api-s.md, but the checks below are not carried by that issue's replay key, so they are recorded here.)

Cause 1 of 2: CreateGraphqlApi publishes GraphqlApi$uris on real-AWS hostnames the emulator does not serve, so no Cognito-authenticated subscription flow can reach the data plane.
AWS: Smithy — CreateGraphqlApi output GraphqlApi$uris is a map keyed GRAPHQL and REALTIME; AWS's contract is that those values are the addresses the API's data plane is actually served on, and every client dials the published host verbatim. Recorded behavior only for the host form.
Emulator: the published value is `https://<26-char-apiId>.appsync-api.<region>.amazonaws.com/graphql` (and the matching realtime host) — an address the emulator does not listen on. Replay refuses it: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>`. Only the apiId and the `/graphql` + `/graphql/realtime` paths come from state; the host and port must come from the listener the emulator is reached on.
Why: a client gets DNS/connection failure (or egress to real AWS) instead of their API. Every mutation-triggers-subscription, Cognito-group subscription filter, filter-mismatch and subscription payload-format behavior is unreachable.
Fix: the CreateGraphqlApi / GetGraphqlApi handlers and the shared endpoint-URI builder they call (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_graphql_api.rs, .../get_graphql_api.rs, and the URI construction in .../src/state.rs).
Traces: appsync__cognito-idp__dynamodb.appsync-mutation-triggers-subscription-positive, appsync__cognito-idp__dynamodb.appsync-subscription-cognito-group-filter-positive, appsync__cognito-idp__dynamodb.appsync-subscription-filter-mismatch-no-delivery-negative, appsync__cognito-idp__dynamodb.appsync-subscription-payload-format.

Cause 2 of 2: the realtime endpoint refuses the WebSocket upgrade with HTTP 403 for a user whose Cognito user-pool token IS valid for the API's AMAZON_COGNITO_USER_POOLS auth, when the only thing missing is the Cognito group a field's @aws_auth directive requires.
AWS: recorded behavior only (the realtime handshake is not in the Smithy control-plane model). The recording shows AppSync separates CONNECTION authorization from SUBSCRIPTION authorization: any identity that satisfies the API's configured authentication mode completes the upgrade with HTTP 101 and receives `connection_ack`; a field-level `@aws_auth(cognito_groups: [...])` denial is then reported per-subscription, as a `start`-time message of type `error` carrying the Unauthorized refusal, leaving the connection usable. Only an identity that fails the API's auth mode outright is refused at the handshake (401/403).
Emulator: the upgrade itself is refused — `websocket._exceptions.WebSocketBadStatusException: Handshake status 403 Forbidden -+-+- {'content-type': 'application/json'} -+-+- None` — for an authenticated, confirmed user-pool user who is merely not in the `Editors` group. The group membership is being evaluated at connect time instead of at subscribe time.
Why: a client cannot observe or test field-level group authorization at all: it never gets a socket, so it never sees the per-subscription `error` frame AWS sends, and a single denied field takes down the whole connection rather than just that subscription. It also breaks the restore path — after AdminAddUserToGroup and re-authentication the same identity must be admitted by the identical subscription call, which is unobservable while the handshake 403s.
Fix: the AppSync realtime handshake authorization must admit any identity that satisfies the API's authentication mode (here: a valid user-pool token for the configured user pool) and move the `@aws_auth(cognito_groups:)` field check into the subscription-registration (`start`) path, answering a denial with a `start`-time `error` message instead of a handshake status — in the AppSync realtime data-plane auth handling under services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/.
Traces: appsync__cognito-idp__dynamodb.appsync-subscription-missing-subscribe-permission-negative.

## Evidence
Cause 1 — expected: uris.GRAPHQL / uris.REALTIME on a host:port the emulator serves (replay reports the customer needs `http(s)://127.0.0.1:<port>/<resource path>`). Actual: `https://pudfx6gk2gtnijjyz4amo3267y.appsync-api.us-west-2.amazonaws.com/graphql`, rejected with `simparity.published.PublishedUrlRejected: ... its host is not one of the emulator's own hosts`. The identical rejection occurs on 4 checks with 4 different apiIds, so the host template — not any one recorded value — is the defect.

Cause 2 — expected: WebSocket upgrade to uris.REALTIME with the outsider's valid user-pool Authorization header -> HTTP 101 + `connection_ack`, then the `start` for `subscription OnCreatePostForEditors { onCreatePostForEditors { id content } }` answered with a message of type `error` (the @aws_auth(cognito_groups:["Editors"]) denial); the connection stays open and delivers nothing. Actual: the handshake is refused — `WebSocketBadStatusException: Handshake status 403 Forbidden -+-+- {'content-type': 'application/json'} -+-+- None` — so no socket, no connection_ack and no per-subscription error frame.

## Steps to Reproduce
Shared setup: create a DynamoDB `Post` table, an AppSync service role with dynamodb access, and an AppSync API with authenticationType AMAZON_COGNITO_USER_POOLS over a Cognito user pool; schema declares Mutation.createPost (DynamoDB PutItem VTL resolver) and `Subscription.onCreatePostForEditors @aws_subscribe(mutations:["createPost"]) @aws_auth(cognito_groups:["Editors"])`.

Cause 1: read graphqlApi.uris["GRAPHQL"] / ["REALTIME"] and POST a mutation / open the realtime socket at that URI — the host is `<apiId>.appsync-api.<region>.amazonaws.com`, which the emulator does not serve. Replay: appsync__cognito-idp__dynamodb.appsync-mutation-triggers-subscription-positive.

Cause 2: create two confirmed pool users, one added to the `Editors` group and one not; authenticate both; open the realtime WebSocket to uris.REALTIME with the NON-Editors user's ID token in the Authorization header. The upgrade returns HTTP 403 instead of 101. Replay: appsync__cognito-idp__dynamodb.appsync-subscription-missing-subscribe-permission-negative.
