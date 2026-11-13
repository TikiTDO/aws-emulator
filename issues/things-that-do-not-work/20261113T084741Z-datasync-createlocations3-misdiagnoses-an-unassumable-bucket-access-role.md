Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-s3-location-iam-role-validation
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-s3-location-iam-role-validation
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|CreateLocationS3|4xx->4xx|$.datasyncErrorCode
Issue-Key: datasync|CreateLocationS3|4xx->4xx|$.errorCode
Issue-Related: 20261029T180920Z-datasync-refusals-omit-errorcode-datasyncerrorcode-and-use-emulator.md
Issue-Commit: 25060605044cad8473b79fae67a906480ef14ab5

# DataSync CreateLocationS3 misdiagnoses an unassumable bucket-access role as an s3:ListObjectsV2 access denial

> _Note: this issue was identified against the code base at commit `25060605044cad8473b79fae67a906480ef14ab5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLocationS3

## Description
One root cause, grounded in one wire divergence cluster.

=== Cause 1 of 1: a bucket-access role whose trust policy does not let DataSync assume it is refused with the wrong error code and the wrong message ===

AWS: Smithy (services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json) models CreateLocationS3 with required members S3BucketArn and S3Config, whose required member S3Config.BucketAccessRoleArn is "the ARN of the IAM role that DataSync uses to access your S3 bucket", and declares errors [InternalException (server, 500), InvalidRequestException (client, 400)]. InvalidRequestException carries three members: message, errorCode and datasyncErrorCode. During CreateLocationS3, AWS first attempts sts:AssumeRole on the supplied BucketAccessRoleArn, before it ever probes the bucket. When that assume-role attempt fails -- the role exists but its trust policy does not allow datasync.amazonaws.com to assume it -- AWS refuses with HTTP 400, __type InvalidRequestException and errorCode == datasyncErrorCode == "StsAssumeRoleInvalidRequest". The message for that error class is the fixed text "Invalid IAM role, " + <the BucketAccessRoleArn exactly as submitted> + ". DataSync service was unable to perform sts:AssumeRole." -- the two sentences are fixed; the only variable part is the role ARN taken from the request (the recording's arn:aws:iam::000000000000:role/sim2real/sim2real-probe-datasync-notrust-43af4972 is one filling of that slot, not part of the requirement). The failure is attributed to the trust relationship, not to any bucket-level permission, because no bucket call is reachable without the assumed credentials.

Emulator: the same request is refused with HTTP 400 but with no errorCode and no datasyncErrorCode member at all, and with emulator-authored message text that blames a bucket data-plane permission instead: "DataSync location access test failed: could not perform s3:ListObjectsV2 on bucket <bucket>. Access denied. Ensure bucket access role has s3:ListBucket permission." So the status is right, the two error-code members are absent, and the message names the wrong failure (a missing s3:ListBucket grant on the bucket policy) for a role that cannot be assumed in the first place.

Why: a client cannot branch on the refusal. Tooling and the AWS SDKs key remediation off datasyncErrorCode/errorCode -- StsAssumeRoleInvalidRequest means "fix the role's trust policy", while a ListObjectsV2 access denial means "fix the bucket policy or the role's permission policy". Against the emulator the discriminating field is missing entirely, so any code reading it sees null and falls through to a generic handler; and an operator reading the message is sent to edit the wrong document, so the location never gets created. Retry/classification logic that distinguishes assume-role setup races from permission errors cannot work.

Fix: CreateLocationS3's validation, in its bucket-access-role check: the assume-role check must run before the bucket access probe and emit its own InvalidRequestException populated with errorCode/datasyncErrorCode = StsAssumeRoleInvalidRequest and the "Invalid IAM role, <roleArn>. DataSync service was unable to perform sts:AssumeRole." message; the existing s3:ListObjectsV2 wording must stay reserved for the case where the role IS assumable but the bucket probe is denied. The error serializer must also carry errorCode and datasyncErrorCode onto the wire for this error.

Related but not covering this: open issue 20261029T180920Z-datasync-refusals-omit-errorcode-datasyncerrorcode-and-use-emulator.md records the same family of defect (missing error-code members plus emulator-authored message text) for other DataSync operations and steps, and the replay report lists this cluster as unowned -- its causes do not include the CreateLocationS3 assume-role refusal, and in particular not the mis-attribution of an unassumable role to a bucket-listing denial.

Traces: datasync-s3-location-iam-role-validation#task#7

## Evidence
Fixture datasync-s3-location-iam-role-validation, step task#7, operation CreateLocationS3, service datasync, verdict mismatch. Expected status 400, actual status 400 (status matches; body diverges).

  $.datasyncErrorCode: missing (expected 'StsAssumeRoleInvalidRequest', got None)
  $.errorCode: missing (expected 'StsAssumeRoleInvalidRequest', got None)
  $.message: value_mismatch (expected 'Invalid IAM role, arn:aws:iam::000000000000:role/sim2real/sim2real-probe-datasync-notrust-43af4972. DataSync service was unable to perform sts:AssumeRole.', got 'DataSync location access test failed: could not perform s3:ListObjectsV2 on bucket sim2real-probe-datasync-iam-60ca440f. Access denied. Ensure bucket access role has s3:ListBucket permission.')

Message anatomy for the expected error: fixed text "Invalid IAM role, " and ". DataSync service was unable to perform sts:AssumeRole."; variable part = the S3Config.BucketAccessRoleArn value copied from the request.

Model references: CreateLocationS3 input {S3BucketArn required, S3Config required (S3Config.BucketAccessRoleArn required, pattern ^arn:(aws|...):iam::[0-9]{12}:role/.*$)}; errors = [InternalException(500), InvalidRequestException(400)]; InvalidRequestException members = {message, errorCode, datasyncErrorCode}.

## Steps to Reproduce
Replay fixture datasync-s3-location-iam-role-validation and inspect step task#7.

Manually: create an S3 bucket; create an IAM role whose trust policy does NOT name datasync.amazonaws.com as a principal (and which therefore cannot be assumed by DataSync), regardless of what it is allowed to do on the bucket; call CreateLocationS3 with that bucket's ARN and S3Config.BucketAccessRoleArn set to that role. Observe HTTP 400 whose JSON body has no errorCode/datasyncErrorCode and a message about s3:ListObjectsV2 / s3:ListBucket on the bucket, instead of errorCode = datasyncErrorCode = StsAssumeRoleInvalidRequest with message "Invalid IAM role, <roleArn>. DataSync service was unable to perform sts:AssumeRole."
