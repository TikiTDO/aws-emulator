Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-web.signed-https-get-on-web-function-endpoint-serves-handler-response lambda-web.unsigned-https-get-on-web-function-endpoint-rejected
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb

# lambda-web: a web function endpoint serves no handler response, because no engine runs the revision's deployment package (1 cause)

> _Filed by the fixer of
> `issues/things-that-now-work/20261109T150907Z-lambda-web-enum-constraint-message-order-non-zip.md`,
> whose Cause 3 fixed the ADDRESS half: an endpoint now publishes a `domainName` this
> emulator answers on and serves the endpoint's own HTTP requests there. This is the
> remaining half of that data plane, kept as its own issue because it is a layer of its
> own: running caller-supplied code in process._

## Operation
CreateWebFunction (observed on the data plane the endpoint publishes)

## Description
Cause 1 of 1: an authorized request to a web function endpoint's published address is
answered `501` instead of the web function's own HTTP response, because nothing runs the
revision's code.

AWS: a web function endpoint IS an HTTP data plane — `GetWebFunctionEndpointResponse$domainName`
is "The domain name assigned to the endpoint", a client reads it and issues its request
there, and what comes back is whatever the revision's code answered. The revision names its
code as a .zip deployment package in Amazon S3
(`buildConfig.codeConfig.s3Object`) for the runtime `buildConfig.runtimeConfig.runtime`
names (a Node.js runtime identifier in the recorded calls), so the response a client gets
is produced by that package's own handler.

Emulator: the endpoint's address is served
(`services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/endpoint_plane.rs`): a
request for an endpoint that is not serving traffic is `404`, and an `IamAuth` endpoint
refuses a request presenting no SigV4 credential with Lambda's `403 {"Message":"Forbidden"}`.
An AUTHORIZED request is answered `501` with a sentence saying no web function code ran.
Nothing is synthesised in the handler's name — no body, no status and no duration for code
nothing executed — which is deliberate: a plausible invented body would be worse than the
honest refusal.

Why: every behaviour that depends on what a web function ANSWERS is unreachable — the
handler's `200` for a signed GET on an endpoint, any routing between revisions by weight,
and anything a client does with the response. The control plane reports the endpoint
`Active` and the address answers, so the gap is visible only at the last hop.

Fix: run the revision's deployment package in process, through the guidance for an
operation that must execute caller-supplied code (a contained, bounded JavaScript engine;
the package is read and judged as an archive already, in
`src/state/code_package.rs`). The work belongs in the lambda-web service crate and layers:
the engine and its containment first, then its bounds, then the request/response bridge
from `endpoint_plane::serve_request` to the handler. The emulated Lambda service
(`services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/`) already
runs handler code for its own invocations and is the closest reference; anything the two
services share belongs in a shared crate under `services/` rather than in a second copy.

## Evidence
Checks `lambda-web.signed-https-get-on-web-function-endpoint-serves-handler-response` and
`lambda-web.unsigned-https-get-on-web-function-endpoint-rejected`: both read
`endpoint["domainName"]` from a `GetWebFunctionEndpoint` whose state is `Active` and then
GET it. The published address is now accepted and dialled (the earlier
`PublishedUrlRejected` refusal is gone), and the signed GET is answered `501` where AWS
answers the handler's `200`.

## Steps to Reproduce
1. Create a web function with a valid .zip package and an endpoint
   (`endpointType` "HomeRegion"); poll `GetWebFunctionEndpoint` until `state == "Active"`.
2. Read `domainName` off the response and issue an HTTPS GET to it, signed with SigV4.
3. The answer is `501` with "This web function endpoint is not implemented: the emulator
   runs no web function code, so the handler's own response cannot be served."; expected is
   the response the deployment package's handler produces.
