Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda__s3__stepfunctions.e2e-cloudfront-distribution-origin-error-diagnosis-lifecycle
Issue-Key: cloudfront__dynamodb__lambda__s3__stepfunctions.e2e-cloudfront-distribution-origin-error-diagnosis-lifecycle|phase#11|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: f3f9216a884ddb496f260c89a030775b09cc5916

# cloudfront__dynamodb__lambda__s3__stepfunctions: GetObject for a missing key answers 404 NoSuchKey to a caller without s3:ListBucket instead of 403 AccessDenied

> _Note: this issue was identified against the code base at commit `f3f9216a884ddb496f260c89a030775b09cc5916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: S3 discloses object non-existence to a principal that is only granted s3:GetObject.
AWS: documented S3 authorization behavior — for a GetObject on a key that does not exist, S3 answers 403 with error code `AccessDenied` (message `Access Denied`) when the requester does NOT hold s3:ListBucket on the bucket, and only answers 404 `NoSuchKey` when the requester DOES hold s3:ListBucket. A distribution fronting a private bucket through an Origin Access Control is exactly this case: the bucket policy grants the CloudFront service principal s3:GetObject alone, so a request for a prefix-shaped path with no object behind it (here `/<tenant>/`, with no DefaultRootObject on the distribution) must come back through the edge as 403 with `<Code>AccessDenied</Code>`.
Emulator: the same request is answered 404 with S3's `NoSuchKey` body — `<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message><Key><tenant>/</Key>…</Error>` — so the OAC-signed origin fetch is served without evaluating the bucket policy's action scope, and object existence leaks to an unauthorized caller. The fixed text of the correct answer is HTTP 403, `<Code>AccessDenied</Code>`, `Access Denied`; the Key/RequestId/HostId values are per-run.
Why: the canonical private-bucket-behind-CloudFront diagnosis is inverted. A client debugging a broken tenant URL sees 'key does not exist' instead of AWS's 'access denied', and any check or runbook that distinguishes a missing root object (403 through OAC) from a genuinely missing key is given the wrong signal. It is also an information-disclosure divergence: existence of keys is revealed to a principal with no list permission.
Fix: the s3 GetObject handler's authorization/error selection — when the requester lacks s3:ListBucket on the bucket, a key miss must be reported as 403 AccessDenied rather than 404 NoSuchKey.
Traces: cloudfront__dynamodb__lambda__s3__stepfunctions.e2e-cloudfront-distribution-origin-error-diagnosis-lifecycle (phase 3/5 'solve'; the same phase confirms `<tenant>/index.html` does serve 200 through the edge, so the distribution and OAC path themselves work).

## Evidence
Expected: GET of the stored tenant URL (a prefix with no object, through an OAC distribution whose bucket policy grants only s3:GetObject) returns 403 containing `<Code>AccessDenied</Code>`. Actual: `AssertionError: [phase 3/5 solve] the stored tenant URL 'https://<dist>.cloudfront.net/<tenant>/' returned 404: '<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message><Key><tenant>/</Key><RequestId>…</RequestId><HostId>…</HostId></Error>'`. The distribution domain, tenant id, request id and host id are per-run values.

## Steps to Reproduce
Create a private bucket with objects `<tenant>/index.html` and `<tenant>/app.js`, a CloudFront distribution with an OAC over that bucket and NO DefaultRootObject, and a bucket policy granting cloudfront.amazonaws.com only s3:GetObject. GET `https://<domain>/<tenant>/index.html` (serves 200) and then GET `https://<domain>/<tenant>/`. Observe 404 NoSuchKey where AWS answers 403 AccessDenied. Re-replay cloudfront__dynamodb__lambda__s3__stepfunctions.e2e-cloudfront-distribution-origin-error-diagnosis-lifecycle.
