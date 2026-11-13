Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked
Issue-Key: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked|_http#18|Timeout|e.read().decode("utf-8", "replace"),
Issue-Commit: 4a88e8365a42250444744c30ae29769cc0d2c461

# apigateway__dynamodb__lambda: the request-validator rejection check reaches no verdict — a deployed stage's AWS_PROXY Lambda integration never answers 200, so the run exhausts the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `4a88e8365a42250444744c30ae29769cc0d2c461`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane, AWS_PROXY Lambda integration)

## Description
Cause 1 of 1: a deployed REST stage whose POST method carries a request validator plus a JSON-Schema request model and an AWS_PROXY Lambda integration never answers the invoke URL, so neither the validator's 400 refusal nor the "integration not invoked" absence window is ever evaluated.

AWS: the control-plane shapes the check configures are modeled — Smithy (com-amazonaws-apigateway-backplanecontrolservice.json): CreateRequestValidator with validateRequestBody/validateRequestParameters (both @default false), and PutMethod with requestValidatorId and requestModels (map of content type -> model name). The execute-api data plane itself is not in the Smithy model, so for it this is recorded behavior only: POST https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/orders with Content-Type: application/json and a body satisfying the method's model is handed to the AWS_PROXY integration at arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function-arn}/invocations, and API Gateway serves the function's own proxy response — HTTP 200 with the handler's body — after which the handler's DynamoDB PutItem is visible via GetItem. A body that omits a property the model declares required is refused by API Gateway BEFORE the integration runs: HTTP 400 with body {"message":"Invalid request body"} — the fixed sentence is "Invalid request body" and this message has no variable part — and because the integration is never invoked, no item ever appears for the handler's sentinel partition key.

Emulator: the very first (control) request never succeeds. The check polls the invoke URL to the condition status == 200 and the harness kills the run first: the only replay output is `Error: Check timed out after 60.0s`. Nothing downstream is reached — the 400 "Invalid request body" assertion, the absence window on the sentinel key, and the "control item must survive the rejected request" assertion all go unevaluated. The failure is a non-answering data plane, not a wrong answer.

Why: a client that deploys a REST stage with a Lambda proxy integration gets no response at all where AWS returns the function's 200 proxy response, and a client that relies on API Gateway's request validator to keep malformed bodies away from its backend gets no evidence either way — the emulator can neither confirm the 400 nor prove the backend was spared. Request validation is a security/ingress control; silence makes it untestable.

Fix: the execute-api data-plane invoke path for AWS_PROXY Lambda integrations on a deployed stage (it must resolve the integration, invoke the function and serialize the function's proxy response), and the request-validator/request-model gate that must run ahead of it and short-circuit with 400 {"message":"Invalid request body"}. Note: open issues 20261029T235159Z-apigateway-dynamodb-lambda-2-causes-the-malformed-proxy.md (same service, deployed stage never answers) and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describe adjacent symptoms, but the replay report does not mark this check as owned by either, so it is filed here; a fixer should re-replay this check's id explicitly.

Traces: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked (verdict: mismatch, `Check timed out after 60.0s`).

## Evidence
Expected (AWS): control POST with body {"id": "<generated>", "note": "valid"} -> HTTP 200, body {"stored": "<id>"}, and a DynamoDB item under that key; then POST with body {"note": "no id here"} (required property `id` missing) -> HTTP 400 with body containing AWS's "Invalid request body" envelope, and NO DynamoDB item under the sentinel key for the whole absence window.

Actual (emulator): the control POST never returns 200. The check's bounded poll on the invoke URL never satisfies its accept condition and the harness terminates the run: `Error: Check timed out after 60.0s`. No HTTP status, error code or body was ever obtained from the deployed stage, so none of the validator assertions were reached.

Grounding: failing check apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked in the behavioral suite report; it carries neither an "ALREADY FILED" nor an "OWNED by" marker.

## Steps to Reproduce
1. Create a DynamoDB table and a Lambda function whose handler writes payload.id (or a fixed sentinel when id is absent) to that table and returns {"statusCode": 200, "body": {"stored": <pk>}}.
2. CreateRestApi; CreateModel(contentType="application/json", schema=a draft-04 object schema with required ["id","note"], additionalProperties=false).
3. CreateRequestValidator(validateRequestBody=true, validateRequestParameters=false).
4. CreateResource(pathPart="orders"); PutMethod(httpMethod="POST", authorizationType="NONE", requestValidatorId=<validator>, requestModels={"application/json": <model>}).
5. PutIntegration(type="AWS_PROXY", integrationHttpMethod="POST", uri="arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{fn-arn}/invocations"); grant lambda:InvokeFunction to apigateway.amazonaws.com for arn:aws:execute-api:{region}:{account}:{api-id}/*/*.
6. CreateDeployment(stageName="test").
7. POST the schema-valid body to https://{api-id}.execute-api.{region}.amazonaws.com/test/orders. Expected 200 with the function's proxy body; observed: no 200 is ever returned and the request path never settles.
8. POST the body missing `id`. Expected 400 {"message":"Invalid request body"} and no sentinel item; observed: unreachable because step 7 never converges.
