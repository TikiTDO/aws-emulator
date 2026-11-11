Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action|_ws_connect#9|Timeout|return {"status": status, "head": head[:300]}
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# apigatewayv2__cloudformation: a CFN-deployed WebSocket API with a RouteSelectionExpression never answers, so the by-action dispatch check hangs until the harness budget expires

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApiGatewayV2 WebSocket data plane (RouteSelectionExpression dispatch)

## Description
One net-new root cause (Cause 1 of 1), grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: a WebSocket API deployed from a CloudFormation template never completes a client exchange, so nothing is dispatched by RouteSelectionExpression.
AWS: recorded behavior only for the data plane. Smithy (apigatewayv2) models the control plane this check builds: CreateApi with ProtocolType=WEBSOCKET and Api.RouteSelectionExpression, CreateRoute with RouteKey, CreateStage. The recording completes the whole exchange well inside the harness's 60 s budget: the stage's wss:// execute-api endpoint accepts the WebSocket handshake, and each frame is routed to the route whose RouteKey equals the value the RouteSelectionExpression selects from the frame (here $request.body.action), with the integration's reply delivered on the same connection.
Emulator: the check makes no observable progress and is killed by the harness - the only recorded detail is 'Error: Check timed out after 60.0s'. No handshake completion and no dispatched frame is observed, so the emulator answers nothing on the WebSocket endpoint the stack just published within the time AWS needs to answer everything.
Why: a client of a WebSocket API gets no response at all rather than a wrong one - it cannot connect, cannot send and has nothing to time out against except its own deadline. Any caller (test harness, browser client, CI gate) blocks on the socket, so a stack whose control plane reports CREATE_COMPLETE publishes an endpoint that is dead.
Fix: the apigatewayv2 WebSocket data-plane listener and its route-selection dispatch (the component that terminates wss:// for a stage and maps a frame onto a Route by RouteSelectionExpression/RouteKey), not the CFN resource handlers - the stack itself deployed.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action.

Related but NOT covering this: the open issue 20261106T180247Z-apigatewayv2-a-cfn-provisioned-http-api-with-corsconfiguration.md records a comparable hang on a CFN-provisioned HTTP API's CORS lifecycle; its replay set does not name this WebSocket check, so this recorded unit is not re-replayed by it.

## Evidence
Replay output (behavioral suite, section ### apigatewayv2__cloudformation):
  [mismatch] apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action
  Error: Check timed out after 60.0s

Expected (AWS): the check runs to completion - the CFN stack publishes a WEBSOCKET API whose RouteSelectionExpression is $request.body.action, the client's wss:// handshake succeeds against the stage endpoint, and a frame carrying an 'action' value is answered by the route whose RouteKey is that value.
Actual (emulator): no step after the deploy returns; the check is terminated at the 60 s harness budget with no WebSocket exchange observed. The 60 s value is the harness's budget, not a requirement - the requirement is that the endpoint answers at all.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (apigatewayv2 and cloudformation are both bootstrapped and serving).
2. CreateStack with a template declaring an AWS::ApiGatewayV2::Api with ProtocolType: WEBSOCKET and RouteSelectionExpression: $request.body.action, two AWS::ApiGatewayV2::Route resources whose RouteKeys are the two action values (plus $connect), their integrations, and an AWS::ApiGatewayV2::Stage. Wait for CREATE_COMPLETE.
3. Open a WebSocket connection to the stage's wss:// execute-api endpoint and send a frame whose JSON body is {"action": "<first route key>", ...}.
4. Expect the handshake to complete and the frame to be answered by the route selected from the body's action. Observe that the connection/exchange never completes - the call blocks until the caller's own deadline.
5. Equivalently: re-replay apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-websocket-route-selection-expression-dispatches-by-action.
