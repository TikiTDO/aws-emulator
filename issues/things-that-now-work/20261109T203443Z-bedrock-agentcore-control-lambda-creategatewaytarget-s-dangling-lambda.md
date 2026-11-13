Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__lambda.gateway-mcp-lambda-target-requires-existing-function
Issue-Key: bedrock-agentcore-control__lambda.gateway-mcp-lambda-target-requires-existing-function|run#16|AssertionError|assert raised_message == expected_message, f"expected {expected_message!r}, got {raised_message!r}"
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# bedrock-agentcore-control__lambda: CreateGatewayTarget's dangling-Lambda refusal uses a member-path sentence instead of AWS's "Lambda function not found: <arn>. Verify the function ARN is correct and retry."

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayTarget

## Description
This issue has 1 net-new root cause.

Cause 1 of 1 — the ValidationException message for an mcp.lambda target whose lambdaArn names no existing Lambda function is reworded and drops the ARN.
AWS: CreateGatewayTarget with targetConfiguration.mcp.lambda.lambdaArn naming a Lambda function that does not exist in the caller's account and Region is refused with ValidationException (Smithy: CreateGatewayTarget's errors include ValidationException @httpError 400), and nothing is created — the target does not appear in ListGatewayTargets. The message is fixed text "Lambda function not found: " + the lambdaArn exactly as the request supplied it + ". Verify the function ARN is correct and retry." The only variable part is the ARN, taken verbatim from the request; the two sentences around it are constant. Creating that exact function and re-issuing the byte-identical CreateGatewayTarget then converges to READY, so the refusal is specifically about the function's absence.
Emulator: the error code matches (ValidationException) but the message is "targetConfiguration.mcp.lambda.lambdaArn: names a Lambda function that does not exist in this account and Region" — a member-path-prefixed sentence that names the input member instead of the resource and omits the ARN entirely.
Why: a client or operator matching AWS's sentence (or parsing the ARN out of it to report which function is missing) gets nothing usable; the refusal reads like a schema/shape complaint about the member rather than a cross-service existence failure, so a caller cannot tell "this ARN is malformed" from "this function has not been created yet" — and the second is the retryable one. Operator logs lose the identity of the missing function.
Fix: CreateGatewayTarget's cross-service Lambda-existence validation message — the shared Lambda check the handler calls (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_gateway_target.rs and the cross-service Lambda helper in that crate's state) must emit AWS's two-sentence template with the request's lambdaArn interpolated, rather than prefixing the input member path.
Traces: bedrock-agentcore-control__lambda.gateway-mcp-lambda-target-requires-existing-function.

## Evidence
bedrock-agentcore-control__lambda.gateway-mcp-lambda-target-requires-existing-function [mismatch]:
AssertionError: expected 'Lambda function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-bacgw-tool-4ec1858e5af4. Verify the function ARN is correct and retry.', got 'targetConfiguration.mcp.lambda.lambdaArn: names a Lambda function that does not exist in this account and Region'.

The check first proved the premise against the lambda emulator (GetFunction answered ResourceNotFoundException with 'Function not found: <arn>', as AWS does), then issued CreateGatewayTarget on a READY gateway with targetConfiguration.mcp.lambda.lambdaArn set to that same ARN and credentialProviderConfigurations=[{credentialProviderType: GATEWAY_IAM_ROLE}]. The emulator correctly refused with code ValidationException; only $.message diverged, so the check never reached its later assertions.

In the expected message, "Lambda function not found: " and ". Verify the function ARN is correct and retry." are the fixed text; the ARN between them is the value the request supplied. The region, account id and generated function name inside the recorded ARN are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Create an IAM role AgentCore can assume and a gateway (CUSTOM_JWT authorizer); wait for GetGateway status READY.
2. Choose a Lambda function name that does not exist and build its ARN: arn:aws:lambda:<region>:<account>:function:<name>. Confirm lambda GetFunction(FunctionName=<name>) answers ResourceNotFoundException.
3. Call CreateGatewayTarget(gatewayIdentifier=<gatewayId>, name=<target>, targetConfiguration={mcp:{lambda:{lambdaArn:<that ARN>, toolSchema:{inlinePayload:[...]}}}}, credentialProviderConfigurations=[{credentialProviderType:'GATEWAY_IAM_ROLE'}]).
4. Inspect the error: the code is ValidationException as AWS returns, but $.message is 'targetConfiguration.mcp.lambda.lambdaArn: names a Lambda function that does not exist in this account and Region' instead of 'Lambda function not found: <that ARN>. Verify the function ARN is correct and retry.'
5. Replay bedrock-agentcore-control__lambda.gateway-mcp-lambda-target-requires-existing-function to confirm the fix; the check then goes on to create the function and assert the byte-identical CreateGatewayTarget converges to READY.
