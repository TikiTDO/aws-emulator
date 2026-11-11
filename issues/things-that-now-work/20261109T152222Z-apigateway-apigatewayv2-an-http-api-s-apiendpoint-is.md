Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__apigatewayv2.httpapi-stage-default-throttle-returns-default-too-many-requests
Issue-Key: apigateway__apigatewayv2.httpapi-stage-default-throttle-returns-default-too-many-requests|run#21|PublishedUrlRejected|url = f"{ctx.stage_url(api['ApiEndpoint'], '$default')}/"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__apigatewayv2: an HTTP API's ApiEndpoint is published as <apiId>.execute-api.<region>.amazonaws.com, a host the emulator does not serve

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: the HTTP API's published invoke endpoint names an AWS production host instead of the emulator's own host.
AWS: CreateApi/GetApi return ApiEndpoint, the URL a client uses to invoke the API; every stage of the API is reachable at that URL. (Recorded behavior only for the host form the emulator must publish: a value published by the emulator has to be an address the emulator is reached on.)
Emulator: ApiEndpoint is returned as "https://<apiId>.execute-api.us-west-2.amazonaws.com". The emulator announced an address other than the one it is reached on, so a client that takes ApiEndpoint and issues a request to it is refused at egress before any HTTP request is made: "endpoint_url was given https://<apiId>.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".
Why: the documented way to invoke an HTTP API — read ApiEndpoint, append the route — cannot work. Every behaviour behind the endpoint (here, the stage's default route throttle answering 429 with the default "Too Many Requests" gateway response) is unreachable, so nothing about the HTTP API data plane can be exercised.
Fix: the apigatewayv2 CreateApi/GetApi/GetApis serializers (the ApiEndpoint member) must publish a host the emulator serves, and the HTTP API data plane must answer on it.
Traces: apigateway__apigatewayv2.httpapi-stage-default-throttle-returns-default-too-many-requests

## Evidence
Expected: ApiEndpoint names an emulator-served host (e.g. http(s)://127.0.0.1:<port>/...), so a request to it reaches the HTTP API data plane and, with a default-route throttle configured, is answered 429 with the default TooManyRequests gateway response.
Actual: "simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:36089/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener" — the request never left the client.

## Steps to Reproduce
1. apigatewayv2 CreateApi(Name=..., ProtocolType='HTTP') with a route and a Lambda/HTTP integration; CreateStage with default-route throttle settings; CreateDeployment.
2. Read ApiEndpoint from CreateApi/GetApi.
3. Issue an HTTP GET to ApiEndpoint + the route path.
Expected: the request reaches the emulator's HTTP API data plane. Actual: ApiEndpoint names *.execute-api.us-west-2.amazonaws.com, which the emulator does not serve, so the call cannot be made.
