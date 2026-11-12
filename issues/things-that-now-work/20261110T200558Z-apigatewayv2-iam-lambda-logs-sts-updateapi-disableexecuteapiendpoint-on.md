Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin
Issue-Key: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin|_do_handshake_attempt#30|Timeout|return status, body, sock
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__iam__lambda__logs__sts: UpdateApi(DisableExecuteApiEndpoint) on a manual stage plus its WebSocket twin never answers — the check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._
>
> _Fixed: the `DisableExecuteApiEndpoint` gate now ANSWERS on every address an api's default `execute-api` endpoint is reached at, on both of this service's data planes — `403` `{"message":"Forbidden"}` while the member is set, and the route's or the handshake's own answer again as soon as it is cleared, with no redeployment of a stage in between (`src/service/data_plane.rs::serves_default_endpoint`, read by `src/service/invoke.rs`, `src/service/websocket.rs` and `src/service/websocket/transport.rs`; driven over every addressing of both protocols and both directions of the transition in `tests/all/disabled_execute_api_endpoint.rs`). Before it, a request to a disabled endpoint was left UNCLAIMED and the front door answered `404 UnknownService`, and a WebSocket handshake to one was answered as though the endpoint were still on._
>
> _What still keeps this check from reaching a verdict is NOT the gate: its handshake helper waits for `101 Switching Protocols` at the endpoint under test, which is the emulator's one buffered front door, and nothing in the vendored path hands a service the connection (two probes against this checkout show the check retrying to the 60 s cap whatever the front door answers — `403`, `418` or `501`). That capability is owned by `issues/things-that-do-not-work/20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md` and `20261107T120000Z-apigatewayv2-the-front-door-cannot-switch-a-websocket-handshake-to-the-apis-own-socket.md`; this check's replay key is re-filed with it as `issues/things-that-do-not-work/20261112T190000Z-apigatewayv2-the-disableexecuteapiendpoint-check-still-hangs-on-the-front-door-upgrade.md`, so it is not recorded as working._

## Operation
UpdateApi

## Description
Cause 1 of 1: the DisableExecuteApiEndpoint contract cannot be observed because neither the manually created HTTP stage nor its WEBSOCKET twin answers.
  AWS: recorded behavior only for the data plane (the execute-api data plane is not in the Smithy model). In the recording, with DisableExecuteApiEndpoint=true a request to the default execute-api endpoint is refused promptly with HTTP 403 and the gateway envelope {"message":"Forbidden"} (fixed text; no variable part), and a WebSocket upgrade to the same default endpoint is likewise refused instead of upgrading; after setting it back to false both serve again. Every leg answers within seconds.
  Emulator: the check reaches no verdict — 'Check timed out after 60.0s'. The signed request/upgrade neither succeeds nor is refused, so the enable/disable transition is ungradeable.
  Why: a client relying on DisableExecuteApiEndpoint to shut the default endpoint sees a hang instead of a 403 Forbidden, so it cannot tell a disabled endpoint from an unreachable one and no retry or failover decision is possible.
  Fix: the apigatewayv2 execute-api front door for manually created stages and WEBSOCKET twins — the DisableExecuteApiEndpoint gate must answer (403 {"message":"Forbidden"}) or serve, never leave the request outstanding. Related open issues 20261110T153046Z-apigatewayv2-iam-lambda-logs-sts-updateapi-disableexecuteapiendpoint-true.md and 20261109T143515Z-apigatewayv2-iam-lambda-logs-sts-a-signed-websocket-handshake-to-a-manually-created.md describe earlier shapes of this area and do not carry this check's replay key.
  Traces: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin.

## Evidence
expected: with DisableExecuteApiEndpoint=true the default execute-api endpoint answers 403 {"message":"Forbidden"} for the HTTP stage and refuses the WebSocket upgrade; with false it serves again — each leg within seconds.
actual: Error: Check timed out after 60.0s — no status, no error code, no refusal envelope.

## Steps to Reproduce
1. Create an HTTP API and a WEBSOCKET API, create stages manually and deploy them.
2. UpdateApi(DisableExecuteApiEndpoint=true) on both.
3. Send a SigV4-signed request to the HTTP default execute-api endpoint and an RFC 6455 upgrade to the WebSocket one; neither answers within 60 s.
4. Replay apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin.
