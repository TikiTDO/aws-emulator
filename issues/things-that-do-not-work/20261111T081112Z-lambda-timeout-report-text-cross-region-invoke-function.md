Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.python-importlib-metadata-reads-vendored-dist-info lambda.python-cpu-bound-handler-within-timeout lambda.handler-working-until-its-remaining-time-floor-completes-before-the-kill lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it lambda.timeout-lowered-kills-a-handler-that-finished-before-rejected lambda.init-phase-is-not-charged-against-the-function-timeout lambda.invoke-refuses-a-function-arn-from-another-region lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected lambda.deep-lambda-3-url-cors-not-enforced-server-side lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler lambda.url-aws-iam-signed-request-429-when-reserved-concurrency-zero lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers lambda.container-image-uri-source-validation-rejected lambda.alias-weighted-routing-splits-traffic
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.python-importlib-metadata-reads-vendored-dist-info|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-cpu-bound-handler-within-timeout|run#6|AssertionError|assert err is None, f"the CPU-bound handler failed within its 20 s timeout: {err!r} {out!r}"
Issue-Key: lambda.handler-working-until-its-remaining-time-floor-completes-before-the-kill|run#16|AssertionError|assert over_body.get("errorMessage") == want, (
Issue-Key: lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it|run#16|AssertionError|assert body.get("errorMessage") == want, (
Issue-Key: lambda.timeout-lowered-kills-a-handler-that-finished-before-rejected|_assert_timed_out#13|AssertionError|assert body.get("errorMessage") == want, (
Issue-Key: lambda.init-phase-is-not-charged-against-the-function-timeout|_init_duration_ms#11|AssertionError|raise AssertionError(f"the cold REPORT line carries no Init Duration: {report!r}")
Issue-Key: lambda.invoke-refuses-a-function-arn-from-another-region|_client_error#17|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected|run#10|AssertionError|assert status == 200 and json.loads(body).get("marker") == MARKER, (
Issue-Key: lambda.deep-lambda-3-url-cors-not-enforced-server-side|run#10|AssertionError|assert status == 200 and json.loads(body).get("marker") == MARKER, (
Issue-Key: lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler|run#12|AssertionError|assert status == 200, f"preflight must be answered 200, got {status!r} {hdrs!r} {body[:200]!r}"
Issue-Key: lambda.url-aws-iam-signed-request-429-when-reserved-concurrency-zero|run#10|AssertionError|assert throttled["headers"].get("x-amzn-errortype") == "TooManyRequestsException", (
Issue-Key: lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers|run#18|AssertionError|assert headers["host"] == host, f"host header must be the URL host, got {headers['host']!r}"
Issue-Key: lambda.container-image-uri-source-validation-rejected|run#9|AssertionError|assert cross_region_uri != good_uri, "the cross-region substitution did not change the host"
Issue-Key: lambda.alias-weighted-routing-splits-traffic|_sample_all_primary#26|Timeout|lam.invoke(FunctionName=f"{fn}:{alias}", Payload=b"{}").get("ExecutedVersion")
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# Lambda timeout/report text, cross-region Invoke, function URL CORS and 10 more invoke-plane causes

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Ten further net-new causes in the Lambda invoke and function-URL planes. (Open issue 20261107T212447Z-lambda-6-net-new-causes-timeout-error-text-oversize.md covers the same timeout-message shape for two OTHER checks but carries none of the replay keys below; 20261107T135301Z-lambda-9-net-new-root-causes-in-durable.md covers neighbouring function-URL causes. Neither covers these.)

Cause 1 of 10: importlib.metadata cannot see a .dist-info vendored in the deployment package.
AWS: recorded behavior only - a package directory plus its `<name>-<version>.dist-info` inside /var/task is on sys.path, so `importlib.metadata.distribution('<name>')` resolves and reports its version.
Emulator: the handler fails with X-Amz-Function-Error: Unhandled, errorType `PackageNotFoundError`, errorMessage `No package metadata was found for <name>` where `<name>` is the distribution the handler asked for.
Why: libraries that read their own or a dependency's metadata at import (a very common pattern) fail only on the emulator.
Fix: the sys.path / package-metadata view the Python execution environment builds over the extracted deployment package.
Failing checks: lambda.python-importlib-metadata-reads-vendored-dist-info

Cause 2 of 10: a CPU-bound handler that fits inside its timeout is killed as timed out.
AWS: recorded behavior only - a handler doing CPU work that finishes inside the configured Timeout returns normally; AWS only times an invoke out past the deadline.
Emulator: with Timeout=20 the invoke comes back with X-Amz-Function-Error: Unhandled and errorType `Sandbox.Timedout`, errorMessage `<requestId> Task timed out after 20.00 seconds`.
Why: compute-bound functions are unusable and a client sees a spurious timeout; it also means the emulator's CPU budget, not the configured Timeout, decides the deadline.
Fix: the invoke deadline/CPU accounting of the Lambda invoke runtime.
Failing checks: lambda.python-cpu-bound-handler-within-timeout

Cause 3 of 10: the Task-timed-out errorMessage omits the `RequestId: <id> Error: ` prefix.
AWS: recorded behavior only - when an invoke exceeds its Timeout, the payload's errorMessage is `RequestId: <request id> Error: Task timed out after <N>.<NN> seconds`. The fixed text is `RequestId: ` ... ` Error: Task timed out after ` ... ` seconds`; the request id comes from the invocation and the seconds from the function's configured Timeout.
Emulator: the errorMessage is `<request id> Task timed out after <N>.<NN> seconds` - the literal `RequestId: ` and ` Error:` segments are missing.
Why: clients and log scrapers that match on AWS's timeout line (or strip the prefix to recover the request id) mis-parse every emulator timeout.
Fix: the timeout error serializer of the Lambda invoke path.
Failing checks: lambda.handler-working-until-its-remaining-time-floor-completes-before-the-kill, lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it, lambda.timeout-lowered-kills-a-handler-that-finished-before-rejected

Cause 4 of 10: the cold-start REPORT log line carries no Init Duration field.
AWS: recorded behavior only - the REPORT line of a cold invoke is `REPORT RequestId: <id>\tDuration: <d> ms\tBilled Duration: <b> ms\tMemory Size: <m> MB\tMax Memory Used: <u> MB\tInit Duration: <i> ms`, and Init Duration is present exactly on the cold report (init time is not charged against the function Timeout).
Emulator: the cold REPORT line is `REPORT RequestId: <id>\tDuration: <d> ms\tBilled Duration: <b> ms\tMemory Size: <m> MB` - no `Init Duration` (and no `Max Memory Used`) field at all.
Why: a client cannot measure cold-start cost, and because init time is instead folded into Duration the emulator also charges init against the invoke's deadline.
Fix: the REPORT-line formatter and init/invoke time accounting of the Lambda invoke logger.
Failing checks: lambda.init-phase-is-not-charged-against-the-function-timeout

Cause 5 of 10: Invoke accepts a function ARN whose region is not the endpoint's region.
AWS: recorded behavior only; Smithy: Invoke.FunctionName targets NamespacedFunctionName whose pattern carries an optional `<region>:` segment, and Invoke declares ResourceNotFoundException (404) and InvalidParameterValueException (400). Real AWS refuses an Invoke whose FunctionName ARN names another region.
Emulator: the call SUCCEEDS - HTTP 200 with X-Amz-Executed-Version `$LATEST` and the handler's payload.
Why: a client that mis-targets a region gets a silent success against the emulator and a hard failure in production; cross-region isolation is not modelled.
Fix: the ARN parsing/region validation in the Invoke handler.
Failing checks: lambda.invoke-refuses-a-function-arn-from-another-region

Cause 6 of 10: a function URL with a Cors config enforces CORS server-side and refuses cross-origin requests.
AWS: recorded behavior only; Smithy: CreateFunctionUrlConfig takes an optional `Cors` member. A function URL does NOT gate requests on Origin - a request from a disallowed origin is still served by the handler, only without Access-Control-* headers, and a CORS preflight (OPTIONS with Access-Control-Request-Method) is answered 200 with an empty body by the service without invoking the handler.
Emulator: every such request is refused with HTTP 403, header `x-amzn-errortype: AccessDenied` and body `{"__type":"AccessDenied","message":"Requests a browser sends from another site are refused"}`. That status, code and message are the emulator's invention - AWS emits no AccessDenied for a cross-origin function-URL request.
Why: a browser-facing function URL cannot be exercised at all, and a client concludes its CORS config is wrong when AWS would have served the request.
Fix: the function URL front door's CORS handling (it must shape response headers and answer preflights, not authorize on Origin).
Failing checks: lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected, lambda.deep-lambda-3-url-cors-not-enforced-server-side, lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler

Cause 7 of 10: a throttled function URL request carries no x-amzn-ErrorType header.
AWS: recorded behavior only; Smithy: Invoke declares TooManyRequestsException with httpError 429. A function URL request that is throttled (reserved concurrency 0) answers 429 with `x-amzn-ErrorType: TooManyRequestsException`.
Emulator: the response carries no `x-amzn-errortype` header at all (only content-type, content-length, connection, date) with a 28-byte body.
Why: SDKs and clients classify function-URL failures off x-amzn-ErrorType; without it a throttle is indistinguishable from any other error and is not retried as a throttle.
Fix: the error serializer of the function URL front door.
Failing checks: lambda.url-aws-iam-signed-request-429-when-reserved-concurrency-zero

Cause 8 of 10: the function URL event's Host header is the emulator's internal listener address.
AWS: recorded behavior only - in the payload-v2 event a function URL delivers, `headers.host` is the host of the function URL the caller used, and the handler can reconstruct the URL from it.
Emulator: `headers.host` is the internal listener socket address (an address other than the function URL host the request was made to).
Why: handlers that build absolute links, enforce a host allow-list, or sign callbacks from the Host header produce the wrong value. The requirement is that Host equals the URL's host; the particular address in any one run is not part of it.
Fix: the request-to-event mapper of the function URL front door (it must forward the public URL host, not the bound socket).
Failing checks: lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers

Cause 9 of 10: the ECR repositoryUri the emulator publishes has no region in its host, so no cross-region image URI exists.
AWS: recorded behavior only - an ECR repositoryUri is `<account>.dkr.ecr.<region>.amazonaws.com/<repo>`, so substituting another region yields a different host, and CreateFunction/UpdateFunctionCode refuses an image URI whose registry is in another region.
Emulator: substituting the region into the published repositoryUri leaves the host unchanged - the host carries no region segment - so the cross-region refusal cannot even be posed.
Why: a client reading repositoryUri from the emulator gets a registry host that does not match the documented ECR form, and the cross-region image guard is unverifiable.
Fix: the repositoryUri the ECR CreateRepository/DescribeRepositories response serializer publishes, and the image-URI region validation in the Lambda CreateFunction/UpdateFunctionCode handler.
Failing checks: lambda.container-image-uri-source-validation-rejected

Cause 10 of 10: weighted alias routing never resolves - the check never reaches a verdict.
AWS: recorded behavior only - an alias with RoutingConfig.AdditionalVersionWeights splits invokes between the primary and the additional version, and a client observes both versions across repeated invokes.
Emulator: the step does not complete and ends `Check timed out after 60.0s`; no distribution is ever observable.
Why: alias-based canary deployment cannot be exercised, and a hang gives a client no error to act on.
Fix: the alias routing resolution in the Invoke handler / the alias state it reads.
Failing checks: lambda.alias-weighted-routing-splits-traffic

Traces: the per-cause failing-check lists above, all from the behavioral suite's `lambda` section.

## Evidence
Cause 1: expected importlib.metadata.distribution('mypkg') to resolve; got errorType 'PackageNotFoundError', errorMessage 'No package metadata was found for mypkg'.
Cause 2: expected a clean 200 inside a 20 s Timeout; got errorType 'Sandbox.Timedout', errorMessage '<requestId> Task timed out after 20.00 seconds'.
Cause 3: expected errorMessage 'RequestId: <id> Error: Task timed out after 3.00 seconds'; got '<id> Task timed out after 3.00 seconds' (same shape at Timeout=2 and Timeout=3).
Cause 4: expected an Init Duration field on the cold REPORT; got 'REPORT RequestId: <id>\tDuration: 6140.02 ms\tBilled Duration: 6141 ms\tMemory Size: 128 MB'.
Cause 5: expected a refusal; the Invoke SUCCEEDED with HTTPStatusCode 200, x-amz-executed-version '$LATEST'.
Cause 6: expected 200 (handler served / preflight answered with no Access-Control-* headers and an empty body); got 403, x-amzn-errortype 'AccessDenied', body '{"__type":"AccessDenied","message":"Requests a browser sends from another site are refused"}'.
Cause 7: expected x-amzn-ErrorType 'TooManyRequestsException'; response headers were {'content-type': 'application/json', 'content-length': '28', 'connection': 'close', 'date': ...} with no x-amzn-errortype.
Cause 8: expected the event's host header to equal the function URL host; got the internal listener address.
Cause 9: 'the cross-region substitution did not change the host' - the published repositoryUri host contains no region.
Cause 10: 'Check timed out after 60.0s'.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 3/4: CreateFunction with a small Timeout and a handler that sleeps past it; Invoke with LogType=Tail and compare the payload errorMessage and the REPORT line against AWS's text.
3. Cause 5: Invoke with FunctionName set to the function's ARN rewritten to a different region.
4. Cause 6/7/8: CreateFunctionUrlConfig with AuthType=AWS_IAM and a Cors config, then (a) GET with a disallowed Origin, (b) OPTIONS with Access-Control-Request-Method, (c) GET with reserved concurrency 0, (d) GET and echo the event's headers.host.
5. Cause 9: CreateRepository in ECR and read repositoryUri; try to form a same-account URI in another region.
6. Cause 1/2/10: as named in the check ids.
7. Re-replay exactly the check ids in trace_ids to confirm.
