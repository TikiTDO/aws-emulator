Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# Resolve the S3Reference against the shared S3 emulator state, not a private test-only object map

_Filed: 2026-10-24T01:57:15Z_
_Operation: PutRawMessageContent | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PutRawMessageContent is supposed to fetch the updated MIME content from the Amazon S3 object the request's `content.s3Reference` names (bucket / key / optional objectVersion) and install it as the in-transit message's content. The emulator instead resolves the reference against a map that lives only inside this service's own state (`ComAmazonawsWorkmailmessageflowGiraffemessageintransitserviceState::s3_objects`), which nothing but the in-process test helper `seed_s3_object` ever writes.

Expected (AWS): a client uploads the MIME bytes to S3 (`PutObject` on a bucket in the request's region, owned by the calling account) and then calls PutRawMessageContent with that bucket/key. The call returns HTTP 200 and the subsequent GetRawMessageContent reports the uploaded bytes.

Actual (emulator): the object the client stored through the S3 emulator is invisible to this handler — `state.resolve_s3_object(reference)` returns `None`, `resolve_content` fails with "no object is stored at that bucket, key and object version", and the call answers HTTP 400 `InvalidContentLocation` with "WorkMail could not access the updated email content at s3://<bucket>/<key>." The message content is never replaced. The operation therefore cannot succeed for any out-of-process caller: the only way to make a reference resolvable is to link this crate and mutate its state directly, which no SDK client can do.

The region check and bucket-owner check are likewise evaluated against the private record rather than the real bucket's location/owner, so the two documented InvalidContentLocation reasons are decided from data a client cannot influence either.

The fix is to resolve the reference through the S3 emulator's own store (bucket -> object, honouring `objectVersion` when given, and taking the bucket's region and owner account from there), the way other service crates in this repo already reach S3 state.

## Evidence

Smithy model: `RawMessageContent.s3Reference` is required and `S3Reference` is documented as "Amazon S3 object representing the updated message content, in MIME format", with bucket (s3BucketIdType) and key (s3KeyIdType) required and objectVersion selecting one version — i.e. a real S3 object, not a service-local record. `InvalidContentLocation` documents its reasons as a bucket in another region, a bucket owned by another account, or an incomplete bucket policy — all properties of a real S3 bucket.

Handler: services/.../src/ops/put_raw_message_content.rs lines 165-215 (`resolve_content(&state, &reference, &request_region)`) and lines 351-376, which read `state.resolve_s3_object(reference)`; src/state.rs lines 156-193 define `s3_objects: HashMap<S3Ref, S3Object>` plus `seed_s3_object`, documented as "the seed helpers that stand in for those producers". No dependency on the S3 crate exists (the crate's Cargo.toml has no `service-com-amazonaws-s3-amazons3` entry).

Feasible and established pattern in this repo: services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:1171 and services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/list_s3_buckets.rs:119 both call `::service_com_amazonaws_s3_amazons3::state::service_state(engine)`; the S3 crate's state exposes `bucket()`, `object()`, `object_version()` and bucket region/owner (services/service-com-amazonaws-s3-amazons3/src/state.rs:2127, 2136, 2882, 3445).

Test coverage: tests/all/raw_message_content.rs exercises the happy path only through `emulator.seed_object(...)` -> `state.seed_s3_object(...)` (test file lines 119-122, 371-480, 724-800). No test puts an object through the S3 service and then calls PutRawMessageContent, so the cross-service path is entirely unasserted.

## Steps to Reproduce

1. Start the emulator with both the S3 and WorkMailMessageFlow services mounted on the shared engine (core/router/src/lib.rs registers both).
2. Arrange for an in-transit message to exist under some messageId (the only producer today is the in-process seed helper; use it so the step under test is the S3 resolution, not message existence).
3. With an SDK client, call S3 CreateBucket on a bucket in the request's region and S3 PutObject with a valid MIME body at key `updated/message-mime`.
4. Call workmailmessageflow PutRawMessageContent with that messageId and content={'s3Reference': {'bucket': <bucket>, 'key': 'updated/message-mime'}}.
5. Expected: HTTP 200 with an empty body, and a following GetRawMessageContent returns the bytes uploaded in step 3.
6. Actual: HTTP 400 with x-amzn-errortype InvalidContentLocation and message "WorkMail could not access the updated email content at s3://<bucket>/updated/message-mime."; GetRawMessageContent still returns the old content.
