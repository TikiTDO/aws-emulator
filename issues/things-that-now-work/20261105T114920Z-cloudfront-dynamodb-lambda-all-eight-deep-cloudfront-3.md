Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-cache-hit-leaves-origin-invocation-count-unchanged cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-cache-hit-leaves-origin-invocation-count-unchanged|warm#1|Timeout|out = lam.invoke(FunctionName=function_name,
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Commit: 9fc0802c1fad78cd4c44ae6731253718bd889678

# cloudfront__dynamodb__lambda: all eight deep-cloudfront-3 origin-behavior checks never complete — a distribution over a Lambda function-URL origin never becomes usable at its edge domain

> _Note: this issue was identified against the code base at commit `9fc0802c1fad78cd4c44ae6731253718bd889678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Cause 1 of 1: every check in the deep-cloudfront-3 origin family is killed at the 60 s per-check cap before it can make a single observation of the edge. Failing checks: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding, deep-cloudfront-3-cf-origin-cache-hit-leaves-origin-invocation-count-unchanged, deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto, deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees, deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key, deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes, deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin, deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge.

AWS: a CreateDistribution is accepted with Status `InProgress` and, after propagation, GetDistribution reports Distribution.Status == `Deployed`; from that point a request to the distribution's DomainName is served by the edge, relayed to the configured origin (here an AWS_IAM-protected Lambda function URL fronted by an Origin Access Control), and the response carries the origin's body plus the edge's own cache behavior — `x-cache: Miss from cloudfront` / `Hit from cloudfront` according to the cache policy's cache key, CloudFront-generated request headers such as `CloudFront-Viewer-Country` and `CloudFront-Forwarded-Proto` delivered to the origin, viewer-supplied `CloudFront-*` headers stripped at the edge, Origin Path prefixed onto the path the origin sees, and headers/cookies/query strings not named by the origin request policy withheld from the origin. Each of these eight checks first waits for the distribution to reach Deployed (GetDistribution polled every 10 s) and then drives requests through the edge; none of them ever got past that stage.

Emulator: recorded behavior only — the checks produce no observation at all and end with `Error: Check timed out after 60.0s`; no edge response, cache-status header, or origin invocation count is ever obtained. Hypothesis: the distribution never leaves a non-Deployed Status so the first bounded poll never returns, and/or a request to the distribution DomainName is not routed to the deployed distribution at all — the sibling API-Gateway/S3 edge findings in this repository record that shape as a 404 `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` from the emulator's service router. The origin function in these checks also imports boto3, which the emulator's python Lambda runtime does not provide (filed separately for the Lambda runtime), so even a reachable edge would relay a faulted origin.

Why: a client cannot exercise any CloudFront caching or request-shaping behavior against this emulator. Cache-policy cache keys, origin request policies, compression negotiation, Origin Path rewriting and edge header spoof-stripping are all invisible, so a configuration that would break in production looks indistinguishable from a correct one.

Fix: the CloudFront distribution lifecycle (GetDistribution must settle Status to `Deployed`) and the edge data plane that serves a request addressed to a distribution's DomainName and relays it to the configured origin — the CloudFront service's distribution state and its edge/request-routing component, plus the service router that currently fails to resolve a distribution-domain request.

Related open issues describe a CloudFront edge that serves nothing under other groupings (20261102T102137Z-cloudfront-a-deployed-distribution-s-edge-domain-answers.md, 20261102T102137Z-cloudfront-lambda-a-deployed-distribution-over-a-lambda.md, 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md); none of them carries the replay keys of these eight checks.

Traces: see trace_ids.

## Evidence
Expected (AWS): GetDistribution(Id) eventually reports Distribution.Status == 'Deployed'; a GET on https://<DomainName>/<path> then returns 200 with the origin's JSON body and an `x-cache` header, and the origin's DynamoDB invocation counters advance exactly once per Miss and not at all on a Hit.

Actual (emulator): all eight checks are terminated by the harness before any edge response is obtained — `Error: Check timed out after 60.0s` for each of deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding, -cache-hit-leaves-origin-invocation-count-unchanged, -edge-generates-viewer-country-and-forwarded-proto, -path-prefixes-the-path-the-origin-sees, -query-string-allexcept-and-all-shape-the-cache-key, -request-policy-forwards-what-the-cache-key-excludes, -unlisted-header-cookie-query-blocked-from-origin, -viewer-spoofed-edge-headers-rejected-at-the-edge. The checks' own helper would have raised `AssertionError: timed out waiting for distribution <id> to reach Deployed after 900.0s`, so the stall occurs before the 900 s budget, i.e. inside the first poll loop.

## Steps to Reproduce
1. CreateFunction (python3.x) with a function URL (AuthType AWS_IAM) and an Origin Access Control of OriginAccessControlOriginType `lambda`.
2. CreateDistribution with that function URL as the origin, a cache policy with MinTTL=DefaultTTL=MaxTTL=3600 and an origin request policy.
3. Poll GetDistribution for Distribution.Status == 'Deployed'.
4. GET https://<DomainName>/items?a=1 — observe that no edge response is ever served.
5. Re-replay the eight checks in trace_ids to confirm.
