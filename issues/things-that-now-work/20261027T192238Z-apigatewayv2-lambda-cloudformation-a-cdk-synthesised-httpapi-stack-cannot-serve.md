Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys|_http_until#37|Timeout|time.sleep(3.0)
Issue-Commit: bbbd5f0b3425ac8a27c3dccbe4215ebaac702410

# apigatewayv2 + lambda + cloudformation: a CDK-synthesised HttpApi stack still cannot serve its $default stage, because the CloudFormation stack creates no Lambda function and no deployment package can run (1 cause)

> _This report CARRIES FORWARD the replay key of
> `20261027T192238Z-apigatewayv2-a-cdk-synthesised-httpapi-stack-never-produces.md`, which is now
> under `issues/things-that-now-work/`. That report asked for three things: the data plane must
> serve a stage of a CloudFormation-deployed api, the api's `CorsConfiguration` must be applied on
> that path, and a stage whose api has been deleted must answer `404 {"message": "Not Found"}`. The
> stage/route dispatch was already implemented
> (`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/invoke.rs`) and the CORS
> half is implemented now (`cors_decoration` there, pinned by
> `tests/all/http_api_cors_data_plane.rs`) — but the check still cannot reach a verdict, for the
> single remaining reason below. The key is re-filed here so it stays attributed to a cause instead
> of being lost._

## Operation
Invoke (execute-api data plane, AWS_PROXY/Lambda integration) — and CloudFormation's
`AWS::Lambda::Function` provisioning

## Description

Cause 1 of 1: the `$default` route of a CloudFormation-deployed HTTP API cannot answer the HTTP 200
the check polls for, because the Lambda function behind it neither exists nor could run.

AWS: recorded behavior only (execute-api data plane; not in the Smithy model). The stack reaches
CREATE_COMPLETE, `GetStages` lists exactly `$default` with `AutoDeploy` true, and an unsigned HTTPS
GET on the stage endpoint answers 200 with the integration Lambda's JSON body (`{"marker": <the
marker the request carried>, "path": "/", "method": "GET"}`). After `DeleteStack` the same URL
answers 404 with body exactly `{"message": "Not Found"}`, `GetApi` on the deleted api id raises
`NotFoundException` and lambda `GetFunction` raises `ResourceNotFoundException`.

Emulator: two gaps stack up on that one path.

1. CloudFormation provisions no `AWS::Lambda::*` resource type at all. `provisioner_for`
   (`services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs`) maps
   every `AWS::ApiGatewayV2::*` type to a real create against this emulator's apigatewayv2 service,
   but no `AWS::Lambda::Function` / `AWS::Lambda::Permission` arm exists, so those resources are
   left declared-only: the stack publishes a function ARN and a `FunctionName` output for a
   function the lambda service does not hold, and `GetFunction` on it answers
   `ResourceNotFoundException` BEFORE the stack is ever deleted. The CDK-synthesised template also
   carries the function's code inline (`Code: {ZipFile: <source text>}`), which CloudFormation
   itself packages into a .zip archive before calling `CreateFunction` — so the provisioner that
   creates it has to do that packaging too.
2. Even for a function created directly, running its deployment package is not implemented: lambda
   `Invoke` answers HTTP 501 for `RequestResponse`
   (`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs` — "there is no
   in-process runtime for it, and no response payload is returned that the function did not
   produce"), so the matched `AWS_PROXY` route meets the "integration produced no response" class
   and is answered `500 {"message":"Internal Server Error"}`.

The check's serve poll (`_http_until`, `accept = (status == 200)`) therefore never converges and the
check is killed at the suite's 60 s cap, reported as `Check timed out after 60.0s`.

Why: this is the normal way an HTTP API with a Lambda default route is shipped, so nothing a client
does with such a stack is observable — not the 200 and the handler's body, not the CORS decoration
ON that answer (the decoration itself is implemented and tested, but the only status it can be
observed on today is the `500`), and not the `404 {"message": "Not Found"}` that proves the API
stopped serving after teardown.

Fix: (a) an `AWS::Lambda::Function` provisioner in CloudFormation — `CreateFunction` with the
declaration's `Runtime`, `Handler`, `Role`, `MemorySize`, `Timeout` and `Environment`, and its
inline `Code.ZipFile` packaged into the .zip archive the API expects — plus the
`AWS::Lambda::Permission` the CDK template carries; (b) an in-process runtime for a Lambda
deployment package, owned by the open issues
`20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` and
`20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`;
and (c) the `AWS_PROXY` dispatch half of the apigatewayv2 data plane — build the proxy event in the
shape the integration's `PayloadFormatVersion` selects and map the function's
`statusCode`/`headers`/`cookies`/`body`/`isBase64Encoded` onto the HTTP answer, owned by
`20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md`, which
carries its own three keys. Nothing short of a function actually running can close this: a status
or body invented for a backend that never ran would be indistinguishable from a real one.

What a fixer should know about the in-process interpreter route, so the work is not restarted from
scratch: `rustpython-vm` 0.6.0 DOES compile in this workspace, once the `rustpython-ruff_python_ast`
/ `ruff_python_parser` / `ruff_python_trivia` / `ruff_source_file` / `ruff_text_size` crates are
pinned to 0.16.5 (the newest 0.16.10 resolves two `get_size2` versions and fails to build), while
0.4.0 pulls the unmaintained `atty`. What it does not bring is a Python standard library: that
lives in `rustpython-pylib`, which declares no license at all (`cargo info rustpython-pylib@0.6.0`
reports `license: unknown`), so the dependency-license gate cannot admit it and `import json` —
which the recorded handler does — resolves nowhere. Either a licensed stdlib source or native
modules bound to crates this workspace already has is the part that still needs deciding.

The remaining third leg of the original report — a stage whose api has been DELETED answering
`404 {"message": "Not Found"}` — needs a decision that one service cannot make alone: answering it
means claiming `execute-api` requests for api ids this service does NOT hold, and the REST data
plane (`service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`) is
addressed at the same host and path shape, so two claimants make the router answer
`500 AmbiguousService` for every such request. The claim rule has to be settled across both
services (by protocol of the id's shape, or by one owner for unknown ids) before either can answer.

Traces: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys.

## Evidence
Failing check apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys
— verdict [mismatch], detail: `Error: Check timed out after 60.0s`.

Expected (AWS): stack CREATE_COMPLETE; GetStages == {$default, AutoDeploy=true}; GET
<stage-endpoint>/ -> 200 with the Lambda body; the same GET with an allow-listed `Origin` -> 200 +
`access-control-allow-origin`; an OPTIONS preflight -> 200 + that header and
`access-control-allow-methods`; a preflight from a non-allow-listed origin or for a
non-allow-listed method -> 200 with neither header; after DeleteStack: GET -> 404 body exactly
{"message": "Not Found"}, GetApi -> NotFoundException, lambda GetFunction -> ResourceNotFoundException.

Actual (emulator): the stack's apigatewayv2 resources are created and the stage resolves, but the
`$default` route's `AWS_PROXY` integration is answered `500 {"message":"Internal Server Error"}`
(the honest answer for an integration that produced no response) — now carrying the api's CORS
decoration — so the serve poll never accepts. The function the integration names was never created
by the stack at all.

## Steps to Reproduce
1. `cdk synth` (LegacyStackSynthesizer, no bootstrap) an app containing: a python3.12 inline-code
   Lambda, an `apigatewayv2.HttpApi` with
   `corsPreflight={allow_origins:["https://example.test"], allow_methods:[GET]}` and
   `default_integration=HttpLambdaIntegration(fn)`, plus the CDK-generated AWS::Lambda::Permission.
2. CreateStack with that template (Capabilities=[CAPABILITY_IAM]) and poll DescribeStacks for
   CREATE_COMPLETE.
3. lambda GetFunction on the stack's `FunctionName` output: expect the function, observe
   `ResourceNotFoundException` — the stack created none.
4. Unsigned GET on `<ApiEndpoint>/`: expect 200 with the Lambda's JSON body, observe
   `500 {"message":"Internal Server Error"}`.
5. Replay apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys.
