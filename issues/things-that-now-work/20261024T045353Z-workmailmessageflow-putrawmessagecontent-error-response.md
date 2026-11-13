Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1266dfca2a9321742ae6c3c2504880e1e0dae7dd

# PutRawMessageContent never evaluates the S3 bucket policy, so the documented missing/incomplete-policy InvalidContentLocation case answers 200

_Filed: 2026-10-24T04:53:53Z_
_Operation: PutRawMessageContent | Class: error-response_

> _Note: this issue was identified against the code base at commit `1266dfca2a9321742ae6c3c2504880e1e0dae7dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InvalidContentLocation` is documented by this service's own model as covering three distinct misconfigurations of the S3 object named by `content.s3Reference`: a bucket in another region, a bucket owned by another account, and "an incomplete or missing S3 bucket policy". Real WorkMail reads the object as the `workmailmessageflow`/WorkMail service principal, so the bucket must carry a policy granting that principal `s3:GetObject` on the key; without one (the default state of a freshly created bucket, which has no policy at all) the call fails with HTTP 400 `InvalidContentLocation` and the in-transit message keeps its old content.

Expected: PutRawMessageContent naming an object in a bucket that carries no bucket policy (or a policy that does not grant the WorkMail service principal read access to the key) returns HTTP 400 with `x-amzn-errortype: InvalidContentLocation`, and a following GetRawMessageContent still reports the pre-update bytes.

Actual: the handler decides readability from exactly two facts — `object.region != request_region` and `object.owner_account_id != engine::emulator_account_id()` (`resolve_content`, put_raw_message_content.rs:351-376). No policy is consulted, and the handler's own doc comment states the omission is deliberate because "this emulator's S3 holds no bucket policies, so there is nothing to evaluate". That rationale is stale: the S3 emulator does model bucket policies (`Bucket::policy: Option<String>` written by PutBucketPolicy, services/service-com-amazonaws-s3-amazons3/src/state.rs:2036-2039, with PutBucketPolicy/GetBucketPolicy/DeleteBucketPolicy exercised by tests/all/bucket_policy_put_status.rs). The consequence is that the one of the three documented InvalidContentLocation reasons that is hit most often in practice — the customer forgot the bucket policy — is reported as a successful 200 with the message content replaced, i.e. the emulator accepts an update real WorkMail refuses.

This is distinct from the already-open issue "Resolve the S3Reference against the shared S3 emulator state, not a private test-only object map" (PutRawMessageContent / wrong-behavior), which asks only that bucket/key/objectVersion, the bucket's region and the bucket's owner come from the S3 emulator's store; it does not ask for any policy evaluation. Once the reference resolves against real S3 state, the policy check becomes the remaining unimplemented documented failure mode.

## Evidence

Smithy model: `InvalidContentLocation` (httpError 400, declared on PutRawMessageContent) is documented as "WorkMail could not access the updated email content. Possible reasons: You made the request in a region other than your S3 bucket region. The S3 bucket owner is not the same as the calling AWS account. You have an incomplete or missing S3 bucket policy..." — the third reason is an explicit, modeled failure mode. `RawMessageContent.s3Reference` is required and documents a real S3 object.

Handler: services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/put_raw_message_content.rs lines 336-376 — `resolve_content` checks only object presence, `object.region`, and `object.owner_account_id`; lines 348-350 document the policy reason as explicitly not decided. src/state.rs lines 85-123 define `S3Object { content, region, owner_account_id }` with no policy field at all, so the store carries nothing a policy check could read.

Feasibility: the S3 crate already persists bucket policies (services/service-com-amazonaws-s3-amazons3/src/state.rs:2036 `pub policy: Option<String>`, MAX_BUCKET_POLICY_BYTES/is_valid_policy_document at 3743-3759), and cross-crate S3 state access is established practice in this repo (services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/list_s3_buckets.rs:119, services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:1235).

Test coverage: tests/all/raw_message_content.rs covers InvalidContentLocation only for a missing object, a region mismatch and an owner mismatch (`put_with_an_unreadable_object_is_an_invalid_content_location`, `an_object_resolves_only_in_the_region_the_request_was_made_in`). No test exercises a bucket whose policy does not grant WorkMail access, so the third documented reason is entirely unasserted.

## Steps to Reproduce

1. Start the emulator with both the S3 and WorkMailMessageFlow services mounted on the shared engine (core/router registers both).
2. Arrange for an in-transit message to exist under some messageId (today only the in-process seed helper `seed_in_transit_message` can do this).
3. Create an S3 bucket in the request's region owned by the calling account and PutObject a valid MIME body at key `updated/message-mime`. Attach NO bucket policy (or attach one that grants nothing to the WorkMail service principal).
4. Call PutRawMessageContent with that messageId and content={'s3Reference': {'bucket': <bucket>, 'key': 'updated/message-mime'}}.
5. Expected: HTTP 400 with x-amzn-errortype InvalidContentLocation, and a following GetRawMessageContent still returns the pre-update content.
6. Actual: the bucket policy is never consulted — once the reference resolves at all the update is accepted (HTTP 200, empty body) and GetRawMessageContent reports the uploaded bytes.
7. Then attach a bucket policy granting the WorkMail service principal s3:GetObject on that key and repeat: the emulator's answer is identical to step 6, confirming no policy is evaluated in either direction.
