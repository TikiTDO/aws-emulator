Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive apigateway__cloudfront__s3.cloudfront-apigateway-custom-header-passthrough apigateway__cloudfront__s3.cloudfront-apigateway-origin-missing-resource-negative apigateway__cloudfront__s3.cloudfront-apigateway-wrong-origin-path-negative apigateway__cloudfront__s3.cloudfront-apigateway-viewer-protocol-policy-positive apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-positive apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive apigateway__cloudfront__s3.apigw-role-trust-policy-validation
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive|run#6|AssertionError|assert status1 == 200, f"expected 200, got {status1}: {body1!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-custom-header-passthrough|run#7|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-origin-missing-resource-negative|run#7|AssertionError|assert status == 200, f"positive control failed: expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-wrong-origin-path-negative|run#6|AssertionError|assert status == 403, f"expected 403 for a mismatched origin path, got {status}: {body!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-viewer-protocol-policy-positive|run#6|AssertionError|assert redirect_status in (301, 302), f"expected a redirect, got {redirect_status}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive|_poll_http_get#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-positive|_poll_http#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative|_poll_get#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigw-role-trust-policy-validation|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# apigateway__cloudfront__s3: 5 net-new root causes — a distribution domain is answered 404 UnknownService by the service router, the edge never serves an S3/OAI or protocol-mismatched API-Gateway origin, the S3→EventBridge→CreateInvalidation pipeline never runs, and an AWS-service S3 integration never answers

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront edge data-plane GET (distribution DomainName)

## Description
Five distinct net-new root causes, all under the suite's `### apigateway__cloudfront__s3` heading. Each must be closed before this issue is submitted.

=== Cause 1 of 5: a viewer request to a Deployed distribution's DomainName is answered 404 UnknownService by the front-door service router ===
AWS: recorded behavior only — the CloudFront edge (viewer-facing) data plane is not described by the Smithy model. A plain HTTP/HTTPS request to a Deployed distribution's DomainName is routed by its Host header to that distribution, which performs an origin fetch against the behavior's TargetOriginId and relays the origin's response to the viewer: 200 plus the origin body when the origin (here an API Gateway REST stage) answers; 403 when the request maps through an OriginPath/behavior to a resource the origin does not expose; and a redirect (301 Moved Permanently to the same URL over https) when the matched behavior's ViewerProtocolPolicy is `redirect-to-https`. Smithy: CloudFront `ViewerProtocolPolicy` is an enum with values allow-all | https-only | redirect-to-https; `CacheBehavior`/`DefaultCacheBehavior` carry TargetOriginId and the forwarded query-string/header configuration whose values must reach the origin per request.
Emulator: every such viewer request is answered HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}. In that message the error code `UnknownService` and the whole sentence "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)" are fixed text — nothing in it is taken from the request or from state, which is the tell: the router never recognised the distribution domain as a routable target, so no distribution, behavior or origin was ever consulted. Consequently the cases that should differ all collapse to the same 404: the cache-behavior forward check sees 404 instead of 200, the missing-resource negative's positive control sees 404 instead of 200, the wrong-origin-path negative sees 404 instead of 403, and the viewer-protocol-policy check sees 404 instead of a redirect.
Why: a client pointed at a distribution domain can never be served, and every negative edge contract (403 for a mismatched origin path, a redirect for redirect-to-https) is indistinguishable from "service not found". Code that branches on CloudFront's status (retry on 5xx, follow a redirect, treat 403 as an access problem) takes the wrong branch, and a passing control leg can never be established.
Fix: the front door's host-based request router must resolve a distribution DomainName to the CloudFront edge data-plane handler instead of the SigV4/X-Amz-Target service resolver, and that edge handler must implement origin selection, origin fetch and response relay plus ViewerProtocolPolicy redirect handling.
Traces: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive, apigateway__cloudfront__s3.cloudfront-apigateway-custom-header-passthrough, apigateway__cloudfront__s3.cloudfront-apigateway-origin-missing-resource-negative, apigateway__cloudfront__s3.cloudfront-apigateway-wrong-origin-path-negative, apigateway__cloudfront__s3.cloudfront-apigateway-viewer-protocol-policy-positive. Related open issue 20261102T102137Z-apigateway-cloudfront-s3-a-request-to-a-distribution.md describes the same router behavior but does not carry these replay keys.

=== Cause 2 of 5: a distribution over a private S3 origin fronted by a legacy Origin Access Identity never serves the object ===
AWS: Smithy: `S3OriginConfig.OriginAccessIdentity` is a required member whose documented value format is `origin-access-identity/cloudfront/ID-of-origin-access-identity`, and an OAI exists so that "viewers can only access objects in an Amazon S3 bucket through CloudFront". Recorded behavior: once the bucket policy grants the OAI's canonical principal s3:GetObject on the key, a GET of that key at the distribution's DomainName answers 200 with the stored object bytes; a direct request to the bucket stays refused.
Emulator: the GET at the distribution domain never reaches 200 inside the check's own poll window, so both OAI checks run to the suite's 60 s cap ("Check timed out after 60.0s") and no object is ever served through the edge.
Why: the whole point of a legacy OAI — private bucket, public distribution — cannot be exercised, so a client that distributes content exclusively through CloudFront gets nothing at all rather than the object.
Fix: the CloudFront edge handler's S3-origin fetch path, plus OAI canonical-principal evaluation in the S3 bucket-policy authorizer.
Traces: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive, apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-positive.

=== Cause 3 of 5: an http-only origin to an HTTPS-only API Gateway origin never surfaces an origin-connect failure, and the https-only control leg never delivers ===
AWS: Smithy: `OriginProtocolPolicy` is an enum with values http-only | match-viewer | https-only. Recorded behavior: with two behaviors over the same API Gateway REST stage, the https-only origin relays the API's 200 response to the viewer, while the http-only origin cannot complete an origin connection to an endpoint that serves only TLS and CloudFront surfaces that to the viewer as 502 Bad Gateway (or 504 Gateway Timeout when the connect stalls) with no origin body leaking through.
Emulator: neither leg ever produces its status — the positive control never answers 200 with the origin payload and the downgraded behavior never answers 502/504, so the check runs to the 60 s cap.
Why: a client cannot tell a protocol-misconfigured origin from a healthy one, and no 5xx is ever raised for an unreachable origin, so origin-health handling and retry logic are never triggered.
Fix: the CloudFront edge handler's per-behavior origin selection and custom-origin connection path, including the OriginProtocolPolicy-driven scheme choice and the 502/504 mapping of a failed origin connection.
Traces: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative.

=== Cause 4 of 5: S3 ObjectCreated → EventBridge rule → Lambda target → CreateInvalidation produces no invalidation ===
AWS: recorded behavior only for the end-to-end wiring. A PutObject into a bucket with EventBridge notifications enabled emits an "Object Created" event; a rule matching that bucket invokes its permitted Lambda target; the function's CreateInvalidation call makes the invalidation visible through ListInvalidations/GetInvalidation on that distribution with Status `InProgress` (later `Completed`) and an InvalidationBatch whose Paths carry exactly the uploaded key path.
Emulator: no invalidation for the uploaded key ever appears on the distribution, so the check polls to the suite's 60 s cap.
Why: a cache-refresh-on-upload pipeline silently does nothing — stale content is served indefinitely and a client that waits for the invalidation to appear blocks forever instead of observing it.
Fix: the S3 notification emitter (EventBridge destination), the EventBridge rule→Lambda target delivery path, and CloudFront invalidation state/listing.
Traces: apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive.

=== Cause 5 of 5: a deployed REST API's AWS-service S3 integration never answers, so an apigateway.amazonaws.com-trusted role can never be shown to work ===
AWS: recorded behavior only for the execute-api data plane. A GET on a deployed stage whose method has an AWS integration with URI `arn:aws:apigateway:<region>:s3:path/<bucket>` and a credentials role whose AssumeRolePolicyDocument trusts `apigateway.amazonaws.com` is served 200 with S3's ListBucket XML — a body containing `<ListBucketResult` and `<Name><bucket></Name>`, where the bucket name is the one from the request's integration URI and the rest is S3's fixed XML envelope.
Emulator: the stage never answers 200 with that XML; the invoke poll never satisfies its predicate and the check runs to the suite's 60 s cap, so the role's functional assumability is never demonstrated even though IAM records the trust document correctly.
Why: a client relying on an API Gateway AWS-service proxy to S3 receives nothing, and a correctly-trusted integration role is indistinguishable from a broken one.
Fix: the execute-api data-plane AWS-service integration handler (credential-role assumption and the signed sub-request to S3) and its response relay.
Traces: apigateway__cloudfront__s3.apigw-role-trust-policy-validation. Related open issue 20261024T182555Z-apigateway-cloudfront-s3-a-deployed-rest-api-s.md describes the same symptom but does not carry this replay key.

## Evidence
Cause 1 — expected vs actual (all four replayed verbatim from the suite):
  cloudfront-apigateway-cache-behavior-forward-positive: expected 200 with the API's echo JSON; actual 404 b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'
  cloudfront-apigateway-custom-header-passthrough: expected 200; actual the same 404 UnknownService body.
  cloudfront-apigateway-origin-missing-resource-negative: "positive control failed: expected 200, got 404" with the same UnknownService body.
  cloudfront-apigateway-wrong-origin-path-negative: "expected 403 for a mismatched origin path, got 404" with the same UnknownService body.
  cloudfront-apigateway-viewer-protocol-policy-positive: "expected a redirect, got 404".

Cause 2 — both OAI checks: "Error: Check timed out after 60.0s". The check creates a private bucket, PutObject, CreateCloudFrontOriginAccessIdentity, a distribution whose S3OriginConfig.OriginAccessIdentity is origin-access-identity/cloudfront/<id>, puts the OAI bucket policy, waits for Deployed and then polls GET <distribution-domain>/<key> for status 200 (120 s, 10 s interval). Expected: 200 with the object bytes. Actual: the predicate is never satisfied; the run hits the suite cap.

Cause 3 — cloudfront-apigw-origin-protocol-mismatch-negative: "Error: Check timed out after 60.0s". Expected: the https-only behavior answers 200 with the origin payload (origin_label), then the http-only behavior answers 502 or 504 with no origin body. Actual: neither status is ever produced.

Cause 4 — s3-object-created-eventbridge-cloudfront-invalidation-positive: "Error: Check timed out after 60.0s". Expected: after PutObject, an invalidation for /<key> is listed on the distribution with Status in (InProgress, Completed) and InvalidationBatch.Paths.Items == ["/<key>"]. Actual: _wait_for_effect("CloudFront invalidation created for the uploaded S3 key") never finds one.

Cause 5 — apigw-role-trust-policy-validation: "Error: Check timed out after 60.0s". IAM GetRole correctly returns Statement[0] Effect=Allow, Action=sts:AssumeRole, Principal.Service=apigateway.amazonaws.com. Expected: GET <invoke-url>/listing answers 200 with a body containing "<ListBucketResult" and "<Name><bucket></Name>". Actual: the poll "ListBucket via the apigateway.amazonaws.com-assumed role" is never satisfied.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check ids in trace_ids.

Cause 1: create a REST API with a MOCK /echo GET that reflects the query string and X-Custom-Header, deploy it to a stage, CreateDistribution with that stage as a custom origin (OriginPath /<stage>, a behavior forwarding the query string and X-Custom-Header), wait for Deployed, then GET https://<distribution-domain>/echo?q=alpha with X-Custom-Header: header-alpha. Observe 404 UnknownService instead of 200 with the reflected values. Repeat with a mismatched OriginPath (expect 403) and with ViewerProtocolPolicy=redirect-to-https over http (expect a 301 redirect).

Cause 2: PutObject into a private bucket, CreateCloudFrontOriginAccessIdentity, CreateDistributionWithTags using S3OriginConfig.OriginAccessIdentity=origin-access-identity/cloudfront/<oai-id>, PutBucketPolicy granting that OAI s3:GetObject, wait for Deployed, then GET https://<distribution-domain>/<key>. Observe that 200 never arrives.

Cause 3: create a Lambda-proxy REST API, deploy it, then create one distribution with two origins over the same stage domain — one OriginProtocolPolicy=https-only (default behavior) and one http-only (behavior pattern broken/*). GET /probe (expect 200 with the origin payload) and GET /broken/probe (expect 502 or 504). Observe neither.

Cause 4: create a bucket with EventBridge notifications enabled and a distribution over it, create a Lambda that calls CreateInvalidation for the event's key, create an EventBridge rule for that bucket with the Lambda as a permitted target, then PutObject. Poll ListInvalidations on the distribution for a path /<key>. Observe none ever appears.

Cause 5: create a role trusting apigateway.amazonaws.com with s3:ListBucket on the bucket, create a REST API with a /listing GET AWS integration to arn:aws:apigateway:<region>:s3:path/<bucket> using that role as credentials, deploy, then GET <invoke-url>/listing. Observe 200 with <ListBucketResult> never arrives.
