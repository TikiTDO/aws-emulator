Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.metrics-lambda-function-url-invoke-meters-the-same-as-the-invoke-api
Issue-Key: cloudwatch__lambda.metrics-lambda-function-url-invoke-meters-the-same-as-the-invoke-api|run#7|AssertionError|assert json.loads(obs["body"]) == {"Message": "Forbidden"}, obs
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# An unsigned AWS_IAM Lambda Function URL request is refused with an emulator-specific AccessDenied body instead of AWS's {"Message":"Forbidden"} (1 cause)

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunctionUrlConfig

## Description
Cause 1 of 1: the Function URL correctly refuses an unsigned request with HTTP 403, but serves the emulator's own error envelope as the body where AWS serves a fixed Function-URL refusal document.

AWS: Smithy: lambda CreateFunctionUrlConfig takes AuthType (AWS_IAM|NONE); AWS documents (https://docs.aws.amazon.com/lambda/latest/dg/urls-auth.html) that a request to an AWS_IAM Function URL that is not SigV4-signed by a principal allowed lambda:InvokeFunctionUrl is rejected at the URL, never reaches the handler, and is answered with HTTP 403 and the JSON body {"Message": "Forbidden"} — that body is entirely fixed text with no variable part, and the Function URL endpoint, not the Lambda API error envelope, serves it. Recorded behavior: both unsigned attempts return 403 with exactly that body, and because the handler never ran, neither attempt contributes to AWS/Lambda Invocations, Errors or Duration.

Emulator: the status is right (403) but the body is {"__type":"AccessDenied","message":"Sign the request with the emulator credentials"} — an AWS-JSON error envelope with a __type discriminator, the code AccessDenied, and a message that names the emulator. Neither the key "Message" nor the value "Forbidden" appears, and the AccessDenied code is not what a Function URL refusal carries.

Why: a client that handles a Function URL refusal by its documented body breaks: code reading response["Message"] == "Forbidden" (the shape AWS's own examples and the SDK-less HTTP clients that call Function URLs branch on) sees a KeyError or a mismatch, and an error-classification layer keyed to the AWS envelope's "__type"/"message" pair gets a code (AccessDenied) that AWS does not use here. Worse, the emulator's message instructs the caller to sign with emulator credentials — guidance that has no counterpart on AWS, so an error path tested here does not transfer.

Fix: the Lambda Function URL request-handling component — the AWS_IAM authorization refusal on the Function URL endpoint must serialize AWS's fixed Function-URL refusal body {"Message":"Forbidden"} with HTTP 403, rather than falling through to the service-wide AWS-JSON error envelope used for Lambda API calls.

Traces: cloudwatch__lambda.metrics-lambda-function-url-invoke-meters-the-same-as-the-invoke-api.

## Evidence
Check cloudwatch__lambda.metrics-lambda-function-url-invoke-meters-the-same-as-the-invoke-api [mismatch]:

AssertionError: {'status': 403, 'body': b'{"__type":"AccessDenied","message":"Sign the request with the emulator credentials"}'}

Expected: HTTP 403 with body exactly {"Message": "Forbidden"} for an unsigned request to an AWS_IAM Function URL (fixed text, no per-request values).
Actual: HTTP 403 with body {"__type":"AccessDenied","message":"Sign the request with the emulator credentials"}.

## Steps to Reproduce
1. Create an execution role and a function.
2. lambda CreateFunctionUrlConfig(FunctionName=<fn>, AuthType="AWS_IAM") and add the lambda:InvokeFunctionUrl permission.
3. Issue a plain HTTP GET to the returned FunctionUrl with no SigV4 signature.
4. AWS answers 403 with body {"Message":"Forbidden"}; the emulator answers 403 with {"__type":"AccessDenied","message":"Sign the request with the emulator credentials"}.
