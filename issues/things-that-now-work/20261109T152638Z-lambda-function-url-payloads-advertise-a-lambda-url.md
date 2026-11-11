Issue-Source: trace-sweep-divergence
Issue-Trace: lam-page-list-function-url-configs
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-page-list-function-url-configs lambda-function-url-config-lifecycle
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateFunctionUrlConfig|2xx->2xx|$.FunctionUrl
Issue-Key: lambda|ListFunctionUrlConfigs|2xx->2xx|$.FunctionUrlConfigs[*].FunctionUrl
Issue-Key: lambda|GetFunctionUrlConfig|2xx->2xx|$.FunctionUrl
Issue-Key: lambda|UpdateFunctionUrlConfig|2xx->2xx|$.FunctionUrl
Issue-Related: 20261029T233239Z-lambda-function-url-configs-return-a-default-invokemode.md
Issue-Commit: 8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6

# Lambda function-URL payloads advertise a lambda-url.on.aws host the emulator does not serve

> _Note: this issue was identified against the code base at commit `8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunctionUrlConfig

## Description
Shared fault: every function-URL response member FunctionUrl is minted on the real AWS egress host `https://<32-hex-id>.lambda-url.<region>.on.aws/`, an address the emulator does not serve and cannot be reached on, so the replay flags each of these clusters `host_not_own`. Seven net-new clusters, all on the same serializer. (The InvokeMode/timestamp/NextMarker members of these same responses are a DIFFERENT, already-open fault: 20261029T233239Z-lambda-function-url-configs-return-a-default-invokemode.md. That issue's keys do not cover FunctionUrl, which is why these clusters are unowned; they are filed here only for the host.)

AWS: Smithy FunctionUrlConfig.FunctionUrl / CreateFunctionUrlConfigResponse.FunctionUrl are @required strings with @length min 40 max 100; the model documents them as "The HTTP URL endpoint for your function", i.e. an endpoint that actually answers the invoke. On AWS the host is `<url-id>.lambda-url.<region>.on.aws` and it is served. In an emulator the requirement is only that the announced endpoint is one the emulator itself answers on — the id, the host and the port are environment values, not requirements.
Emulator: the serializer formats the AWS production host literally (e.g. `https://cd3f3a01b5374673be58a4ce558c227e.lambda-url.us-west-2.on.aws/`) on Create, Get, Update and List. The emulator does not own that name, so a client that follows the advertised URL leaves the emulator entirely (DNS/egress), and nothing in the emulator ever answers it.
Why: a client that does the normal thing — create a function URL, then invoke the function over the URL it was handed — never reaches the function. Everything layered on a function URL (an HTTP_PROXY integration, a CloudFront origin, a browser call) is unreachable, and the failure looks like a network problem rather than a configuration one.
Fix: the function-URL serializer/state that mints the URL (CreateFunctionUrlConfig's handler and the shared FunctionUrlConfig projection used by Get/Update/List) must announce the emulator's own data-listener address for the URL id it stored, so the advertised endpoint is the one it is reached on.

Cause 1 of 7: CreateFunctionUrlConfig, 201 == 201 — fixture lam-page-list-function-url-configs step task#5; `$.FunctionUrl: host_not_own`.
Cause 2 of 7: ListFunctionUrlConfigs, 200 == 200 — fixture lam-page-list-function-url-configs step task#7; `$.FunctionUrlConfigs[0].FunctionUrl: host_not_own`.
Cause 3 of 7: ListFunctionUrlConfigs, 200 == 200 — fixture lam-page-list-function-url-configs step task#8; same member on a second config.
Cause 4 of 7: CreateFunctionUrlConfig, 201 == 201 — fixture lambda-function-url-config-lifecycle step task#8; `$.FunctionUrl: host_not_own`.
Cause 5 of 7: GetFunctionUrlConfig, 200 == 200 — fixture lambda-function-url-config-lifecycle step task#9; `$.FunctionUrl: host_not_own`.
Cause 6 of 7: ListFunctionUrlConfigs, 200 == 200 — fixture lambda-function-url-config-lifecycle step task#10; `$.FunctionUrlConfigs[0].FunctionUrl: host_not_own`.
Cause 7 of 7: UpdateFunctionUrlConfig, 200 == 200 — fixture lambda-function-url-config-lifecycle step task#11; `$.FunctionUrl: host_not_own`.

## Evidence
Replay cluster details, status equal to the recorded status in all seven:
- lam-page-list-function-url-configs#task#5 (CreateFunctionUrlConfig 201): `$.FunctionUrl: host_not_own (expected '<shaped-not-asserted>', got 'https://cd3f3a01b5374673be58a4ce558c227e.lambda-url.us-west-2.on.aws/')`
- lam-page-list-function-url-configs#task#7 / #task#8 (ListFunctionUrlConfigs 200): `$.FunctionUrlConfigs[0].FunctionUrl: host_not_own (... 'https://cd3f3a01b5374673be58a4ce558c227e.lambda-url.us-west-2.on.aws/' / '...2393d845cca44276b92eccb5a88ae2e5...')`
- lambda-function-url-config-lifecycle#task#8/#task#9/#task#10/#task#11 (Create/Get/List/Update): `FunctionUrl: host_not_own (... 'https://a72ebd7e3e954c06b2b624c2a1239367.lambda-url.us-west-2.on.aws/')` — note the verdict is host_not_own even where the string equals the recorded one, i.e. the announced host is not the emulator's.
Smithy (com.amazonaws.lambda): FunctionUrlConfig.FunctionUrl @required, @length 40..100, documented as "The HTTP URL endpoint for your function"; CreateFunctionUrlConfig POST /2021-10-31/functions/{FunctionName}/url -> 201.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus) and look at fixture lambda-function-url-config-lifecycle steps task#8..task#11 and lam-page-list-function-url-configs steps task#5/task#7/task#8.
2. By hand: CreateFunction, then CreateFunctionUrlConfig with AuthType=NONE; read FunctionUrl from the 201 body (also GetFunctionUrlConfig / ListFunctionUrlConfigs / UpdateFunctionUrlConfig).
3. Issue an HTTP GET to the returned FunctionUrl from the same client that just talked to the emulator: the host `<id>.lambda-url.<region>.on.aws` is not an address the emulator answers on, so the request never reaches the function.
