Issue-Source: trace-sweep-divergence
Issue-Trace: s3-cfg-configure-before-create
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-cfg-configure-before-create s3-cfg-tags-at-create s3-create-bucket-namespace-header-and-account-mismatches s3-oc-create-bucket-ownership-header-mistakes s3-multipart-upload-lifecycle s3-object-copy-and-rename s3w-multipart-complete-errors s3w-multipart-entity-too-small
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CreateBucket|2xx->2xx|$headers.location
Issue-Key: s3|CompleteMultipartUpload|2xx->2xx|$.CompleteMultipartUploadResult.Location
Issue-Commit: f0b2a1a4859373de2ddca21e0cf7a4e6a954ae11

# s3 publishes response URLs on the real AWS S3 host instead of the endpoint it serves

> _Note: this issue was identified against the code base at commit `f0b2a1a4859373de2ddca21e0cf7a4e6a954ae11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucket

## Description
Two S3 responses that hand the client a URL for the resource it just created build that URL against the public AWS S3 domain rather than the address the request arrived on. The replay flags both as host_not_own: the emulator announces a host it does not serve.

Cause 1 of 2 — CreateBucket's Location response header names <bucket>.s3.amazonaws.com.
AWS: Smithy: CreateBucketOutput$Location is bound to httpHeader Location. Against real S3 the value is the created bucket's own endpoint; the requirement for an emulator is that the announced host be the endpoint serving the request (scheme, host and port taken from the request's Host/endpoint), with the bucket addressed in the style the request used. A hardcoded public-AWS host is not a per-environment detail the client can ignore — it is an address that resolves away from the emulator.
Emulator: emits Location: http://<bucket>.s3.amazonaws.com/ — a real AWS host the emulator does not answer on — for every CreateBucket, regardless of the endpoint the PUT was received on.
Why: a client that follows the Location it was handed (SDK redirect handling, CLI tooling, or any code that treats Location as the new bucket's base URL) leaves the emulator and talks to public AWS, where the bucket does not exist; in a sandbox with no egress it simply fails to connect. The create looks successful but the returned handle is unusable.
Fix: CreateBucket's response serializer — construct Location from the request's own endpoint instead of a constant s3.amazonaws.com host.
Traces: s3-cfg-configure-before-create#task#3 (cluster also covers s3-cfg-tags-at-create#task#1, s3-create-bucket-namespace-header-and-account-mismatches#task#6, s3-oc-create-bucket-ownership-header-mistakes#task#4).

Cause 2 of 2 — CompleteMultipartUpload's Location element names <bucket>.s3.<region>.amazonaws.com.
AWS: Smithy: CompleteMultipartUploadOutput$Location (XML element Location inside CompleteMultipartUploadResult) is the URL of the newly assembled object. Same requirement: the host must be the endpoint serving the request; only the bucket/key path portion is determined by the request.
Emulator: serializes Location as https://<bucket>.s3.<region>.amazonaws.com/<key>, a real AWS host it does not serve.
Why: a client that uses the completed object's Location to fetch or hand off the object (a common post-upload step) is pointed at public AWS and gets a failure or, worse, an unrelated object; the multipart upload appears to have completed with an address that does not work.
Fix: the CompleteMultipartUpload result serializer — build Location from the serving endpoint plus the bucket and key.
Traces: s3-multipart-upload-lifecycle#task#8 (cluster also covers s3-object-copy-and-rename#task#7, s3w-multipart-complete-errors#task#8, s3w-multipart-entity-too-small#task#6).

Related open issues report the same class of defect for other services' S3-facing URLs (e.g. 20261109T164637Z-polly-synthesis-task-outputuri-announces-the-real-aws.md, 20261109T210043Z-lambda-s3-getlayerversion-publishes-content-location-on-an-awslambda-layers.md) but none of them covers S3's own CreateBucket Location or CompleteMultipartUpload Location.

## Evidence
Replay of the wire corpus (service s3):

- CreateBucket, fixture s3-cfg-configure-before-create step task#3, status 200 -> 200: $headers.location: host_not_own — the emulator returned 'http://<bucket>.s3.amazonaws.com/', a host it does not serve. Same cluster: s3-cfg-tags-at-create#task#1, s3-create-bucket-namespace-header-and-account-mismatches#task#6, s3-oc-create-bucket-ownership-header-mistakes#task#4 (4 steps).
- CompleteMultipartUpload, fixture s3-multipart-upload-lifecycle step task#8, status 200 -> 200: $.CompleteMultipartUploadResult.Location: host_not_own — the emulator returned 'https://<bucket>.s3.<region>.amazonaws.com/<key>'. Same cluster: s3-object-copy-and-rename#task#7, s3w-multipart-complete-errors#task#8, s3w-multipart-entity-too-small#task#6 (4 steps).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Fixture s3-cfg-configure-before-create step task#3: read the Location header of the CreateBucket 200 — it names an s3.amazonaws.com host, not the emulator's endpoint.
3. Fixture s3-multipart-upload-lifecycle step task#8: read CompleteMultipartUploadResult/Location — it names an s3.<region>.amazonaws.com host.
4. By hand: PUT a bucket against the emulator's endpoint and inspect Location; run a multipart upload to completion and inspect the Location element. Attempt to GET either URL — it does not resolve to the emulator.
