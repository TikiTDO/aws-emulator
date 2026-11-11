Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied|_poll_until#40|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected|_poll_until#44|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied|_poll_until#37|Timeout|time.sleep(interval_s)
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# A CloudFront distribution's edge domain never serves, so three edge-enforcement checks never reach a verdict

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 3: the distribution's published edge domain never answers an unsigned GET, so the signed-URL source-IP leg is never graded.
AWS: recorded behavior only - once CreateDistribution reports the distribution Deployed, GETs to the DomainName it published are served by the edge from the configured origin, and the behavior that requires a signed URL answers 403 for an unsigned request while an unsigned public behavior answers 200.
Emulator: polling `<edge>/whoami` never yields a 200 inside the run budget; the step ends `TimeoutError: 200 from the unsigned /whoami behavior: the run budget ended first; not graded`. The edge domain the emulator publishes is never servable.
Why: nothing behind a distribution can be exercised - trusted-key-group signing, IP restrictions, cache behaviors - and a client that deploys a distribution and then fetches its DomainName hangs instead of getting a response or an error.
Fix: the CloudFront data-plane front door that must serve the published DomainName, and the DomainName the CreateDistribution/GetDistribution serializer publishes.
Failing checks: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied

Cause 2 of 3: a cache behavior's AllowedMethods restriction is never exercised - the check never completes.
AWS: recorded behavior only - a cache behavior whose AllowedMethods is GET/HEAD answers a POST with HTTP 403 from the edge, without forwarding it to the origin.
Emulator: the step does not complete and ends `Check timed out after 60.0s`; no POST verdict is ever produced.
Why: a client relying on the edge to reject methods the behavior does not allow gets no answer at all.
Fix: the method gate of the CloudFront edge request pipeline (and the data-plane front door it runs in).
Failing checks: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected

Cause 3 of 3: an origin access control over a Lambda function URL origin is never exercised - the check never completes.
AWS: recorded behavior only - with an OriginAccessControl in SIGV4_ALWAYS mode over a Lambda function URL origin, the edge signs the origin request so the distribution serves the function, while a direct call to the function URL is refused.
Emulator: the step does not complete and ends `Check timed out after 60.0s`.
Why: the documented way to put a function URL behind CloudFront without exposing it cannot be validated.
Fix: the CloudFront origin-request signing path for OriginAccessControl, and the data-plane front door that serves the distribution.
Failing checks: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied

Open issue 20261103T062355Z-cloudfront-lambda-a-deployed-distribution-s-edge-domain-3d1d10.md describes a neighbouring edge-domain failure but carries none of the three replay keys above, so it does not cover them.

Traces: the three check ids named per cause, from the behavioral suite's `cloudfront__lambda` section.

## Evidence
cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied: expected 200 from the unsigned /whoami behavior on the published edge domain; got no 200 at all - 'TimeoutError: 200 from the unsigned /whoami behavior: the run budget ended first; not graded'.
cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected: expected an edge 403 for POST against a GET/HEAD-only behavior; got 'Check timed out after 60.0s'.
cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied: expected the distribution to serve the signed function-URL origin and the direct call to be denied; got 'Check timed out after 60.0s'.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateDistribution with a Lambda function URL origin and two cache behaviors (one public, one requiring a trusted key group); wait for Deployed and read DomainName.
3. GET https://<DomainName>/whoami unsigned - it never returns 200.
4. POST to a behavior whose AllowedMethods is GET/HEAD - expect 403 from the edge.
5. Attach an OriginAccessControl in SIGV4_ALWAYS mode and GET through the distribution, then call the function URL directly.
6. Re-replay exactly the check ids in trace_ids.
