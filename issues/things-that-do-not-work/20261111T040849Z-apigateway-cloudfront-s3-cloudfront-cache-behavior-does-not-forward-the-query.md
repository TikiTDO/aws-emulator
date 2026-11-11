Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive|run#8|AssertionError|assert reflected1["query"] == "alpha", f"expected query 'alpha' forwarded, got {reflected1!r}"
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# CloudFront cache behavior does not forward the query string or allowlisted headers to an API Gateway custom origin

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
One net-new root cause.

Cause 1 of 1: a cache behavior that is configured to forward the query string and an allowlisted header delivers neither to the API Gateway custom origin.
AWS: recorded behavior only — the model has no trait for edge forwarding. The recording shows a distribution whose default cache behavior forwards the query string and an allowlisted request header; each viewer request reaches the API Gateway origin with that per-request query string and header intact, and the origin's response is relayed back unmodified (not a cached copy).
Emulator: the origin receives empty values for all three forwarded items. The echo API reports {'query': '', 'header': '', 'cfHeader': ''} where the request carried query 'alpha' and the allowlisted header.
Why: a client that relies on CloudFront in front of an API (the standard SPA/API pattern) sees every request arrive at the API with its query string and headers stripped, so request-dependent APIs return the wrong answer and cache-key behaviour cannot be exercised at all.
Fix: the edge request path that builds the origin request from the viewer request — it must copy the query string and the headers named by the behavior's forwarding/cache-policy configuration onto the origin request.
Traces: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive.

## Evidence
Check apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive [mismatch]:
AssertionError: expected query 'alpha' forwarded, got {'query': '', 'header': '', 'cfHeader': ''}

Expected: the API Gateway origin observes the viewer request's query string ('alpha') and the allowlisted header value, and the relayed response carries them back.
Actual: all forwarded values arrive empty at the origin.

## Steps to Reproduce
1. Create a REST API that echoes the request query string and a named header.
2. CreateDistribution with that API as a custom origin and a default cache behavior that forwards the query string and allowlists a request header.
3. GET <distribution>/echo?query=alpha with the allowlisted header set.
4. Observe the echoed body: query and header are empty instead of the values sent.
