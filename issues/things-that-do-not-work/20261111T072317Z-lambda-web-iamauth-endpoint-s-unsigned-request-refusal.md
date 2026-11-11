Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-web.unsigned-https-get-on-web-function-endpoint-rejected
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web.unsigned-https-get-on-web-function-endpoint-rejected|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# lambda-web IamAuth endpoint's unsigned-request refusal serves a JSON {"Message":"Forbidden"} body and no lambda-web-request-id header, not AWS's 403 Forbidden page (1 cause)

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWebFunctionEndpoint

## Description
Cause 1 of 1: the 403 a web function endpoint answers an unsigned request with is the wrong document and omits the response header that identifies it. Failing check: lambda-web.unsigned-https-get-on-web-function-endpoint-rejected.

AWS: an endpoint whose authType is IamAuth "uses IAM to authorize requests, which must be signed with Signature Version 4 (SigV4)" (Smithy: AuthType, one of ApplicationManaged | IamAuth, on GetWebFunctionEndpointResponse$authType / CreateWebFunctionEndpointRequest$authType). The data plane an endpoint publishes (GetWebFunctionEndpointResponse$domainName, "The domain name assigned to the endpoint") is plain HTTP and is not modelled, so this is recorded behavior only: the recording shows an HTTPS request to the published domainName carrying no SigV4 credential refused with HTTP 403 whose body is the Forbidden HTML page and whose headers carry lambda-web-request-id. Its FIXED text is, exactly:

<!DOCTYPE html>\n<html>\n<head>\n    <title>HTTP Error 403</title>\n</head>\n<body>\n    <h1>403 Forbidden</h1>\n    <p>You are not authorized to access this endpoint.</p>\n    <p>Request ID: {request-id}</p>\n</body>\n</html>

The only VARIABLE part is {request-id}: the id of that very response, which must equal the value of that response's own lambda-web-request-id header (every run produces a different one; no recorded id is part of the requirement). There is no error code on this hop — it is an HTTP status and a document, not a restJson1 error envelope.

Emulator: the published address answers such a request 403 with content-type application/json and the body {"Message":"Forbidden"} (the FORBIDDEN_DOCUMENT constant), and sets no lambda-web-request-id header at all, so a client has no id for the refusal and the body is neither the documented page nor HTML.

Why: a client that shows or logs the service's refusal page gets a JSON blob instead of the 403 Forbidden document AWS serves, and any client or test that correlates a refusal by its lambda-web-request-id has no id to correlate on — the header is simply absent, so the refusal is unattributable.

Fix: the endpoint data plane's refusal path — the IamAuth branch of the endpoint listener's request handler in the lambda-web service crate's endpoint-plane state module, which must build the HTML Forbidden document from the per-response request id it also emits as the lambda-web-request-id response header (content-type text/html), rather than returning a fixed JSON constant.

Related, already filed, NOT re-filed here: the same check cannot reach this assertion today because its positive control (a correctly signed GET that must be served the handler's 200) is answered 501 by the endpoint plane; that cause is open as issues/things-that-do-not-work/20261110T090000Z-lambda-web-endpoint-serves-no-handler-response.md, which carries both endpoint-invocation check ids. That issue explicitly describes the current 403 {"Message":"Forbidden"} answer as correct, so closing it alone leaves this check failing on the 403 body and the missing header.

Traces: lambda-web.unsigned-https-get-on-web-function-endpoint-rejected (currently reported as "Check timed out after 60.0s": its control poll loops on the 501 before the 403 assertions run).

## Evidence
Check lambda-web.unsigned-https-get-on-web-function-endpoint-rejected asserts, after a signed control request succeeds:
  status == 403 for an unsigned GET to the Active IamAuth endpoint's domainName;
  request_id = headers.get("lambda-web-request-id") must be non-empty ("a 403 response must carry a lambda-web-request-id header");
  text == "<!DOCTYPE html>\n<html>\n<head>\n    <title>HTTP Error 403</title>\n</head>\n<body>\n    <h1>403 Forbidden</h1>\n    <p>You are not authorized to access this endpoint.</p>\n    <p>Request ID: {request_id}</p>\n</body>\n</html>" with that same response's request id interpolated.
Emulator instead answers 403 with content-type application/json and body {"Message":"Forbidden"}, with no lambda-web-request-id header (lambda-web endpoint-plane refusal constant). The check is reported as a 60s timeout because its signed positive control is answered 501 and it polls for 200, so the 403 assertions are never reached — both must be served correctly for the check to pass.

## Steps to Reproduce
1. CreateWebFunction with a valid .zip deployment package in a versioned S3 bucket and endpointConfig {endpointName: "ep", endpointType: "HomeRegion", authType: "IamAuth"}.
2. Poll GetWebFunctionEndpoint(functionName, endpointName="ep") until state == "Active" and read domainName.
3. Issue an HTTPS GET to that domain carrying NO Authorization header and no X-Amz-Credential query parameter.
4. Observed: 403, content-type application/json, body {"Message":"Forbidden"}, no lambda-web-request-id response header. Expected: 403 with the HTML Forbidden page above and a lambda-web-request-id header whose value is the id interpolated into the page's "Request ID:" line.
