Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.abort-multipart-nosuchupload-after-complete s3.multipart-list-uploads-then-abort-nonexistent s3.annotation-name-validation-rejected s3.object-annotation-reserved-prefix-rejected s3.object-annotation-validation-errors-rejected s3.put-object-annotation-limits s3.list-object-annotations-invalid-prefix s3.metadata-table-configuration-invalid-schema-rejected s3.bucket-acl-conflicting-grants-rejected s3.copy-object-source-if-match-mismatch-rejected s3.delete-objects-exceeds-max-keys-rejected s3.multipart-upload-part-invalid-number-rejected s3.conc-s3-conditional-writes-if-match-over-delete-marker-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|AbortMultipartUpload|NoSuchUpload
Issue-Key: s3.annotation-name-validation-rejected|run#13|AssertionError|assert code == "InvalidAnnotation", f"expected InvalidAnnotation, got {code}"
Issue-Key: s3.object-annotation-reserved-prefix-rejected|run#9|AssertionError|assert code == "InvalidAnnotation", f"expected InvalidAnnotation for 'aws:'-prefixed name, got {code}"
Issue-Key: s3.object-annotation-validation-errors-rejected|run#15|AssertionError|assert message == NAME_TOO_LONG_MESSAGE, f"unexpected AnnotationNameTooLong message: {message!r}"
Issue-Key: s3.put-object-annotation-limits|run#7|AssertionError|assert code_too_long == ("AnnotationNameTooLong", "Your annotation name is too long."), (
Issue-Key: s3.list-object-annotations-invalid-prefix|run#15|AssertionError|assert err_prefix.get("Code") == "InvalidPrefix", f"expected InvalidPrefix, got {err_prefix!r}"
Issue-Key: s3.metadata-table-configuration-invalid-schema-rejected|run#5|AssertionError|assert code == "InvalidRequest", code
Issue-Key: s3.bucket-acl-conflicting-grants-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: s3.copy-object-source-if-match-mismatch-rejected|run#6|AssertionError|assert refusal == _PRECONDITION_FAILED, f"expected {_PRECONDITION_FAILED!r}, got {refusal!r}"
Issue-Key: s3.delete-objects-exceeds-max-keys-rejected|run#11|AssertionError|assert exc.response.get("Error", {}).get("Message") == (
Issue-Key: s3.multipart-upload-part-invalid-number-rejected|run#7|AssertionError|assert message == "Part number must be an integer between 1 and 10000, inclusive", message
Issue-Key: s3.conc-s3-conditional-writes-if-match-over-delete-marker-rejected|run#16|AssertionError|assert code == "NoSuchKey", f"expected NoSuchKey for If-Match over a marker, got {code!r}"
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# S3 refusal fidelity: AbortMultipartUpload is not a no-op, and nine error codes/messages diverge from AWS

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AbortMultipartUpload

## Description
Ten net-new root causes, all refusals whose status, code or message differs from AWS. A fixer must close all ten.

Cause 1 of 10: AbortMultipartUpload is not idempotent.
AWS (recorded behavior): aborting an upload id that is already gone -- completed, or never present -- answers the operation's success status, HTTP 204 with no body, as a no-op. (Smithy: AbortMultipartUpload http code 204; NoSuchUpload (404) is the only modeled error.)
Emulator: raises NoSuchUpload 'The specified upload does not exist. The upload ID may be invalid, or the upload may have been aborted or completed.' for both the already-completed id and the never-existing id.
Why: cleanup code that aborts unconditionally (the standard pattern after CompleteMultipartUpload or in a finally block) now raises, so teardown paths fail and callers must special-case an error AWS does not return.
Fix: the AbortMultipartUpload handler, which must answer 204 when the upload id is absent.
Traces: s3.abort-multipart-nosuchupload-after-complete, s3.multipart-list-uploads-then-abort-nonexistent

Cause 2 of 10: annotation-name validation uses the wrong error code.
AWS: an annotation name that breaks the name rules -- including one using the reserved 'aws:' prefix -- is refused with code InvalidAnnotation.
Emulator: refuses with InvalidAnnotationName, a code AWS does not emit for this class.
Why: a client branching on InvalidAnnotation never matches, so a well-classified validation failure becomes unhandled.
Fix: the object-annotation name validator's error classification.
Traces: s3.annotation-name-validation-rejected, s3.object-annotation-reserved-prefix-rejected

Cause 3 of 10: the AnnotationNameTooLong message text diverges.
AWS: refuses a name over the 512-byte limit with code AnnotationNameTooLong and AWS's own message for that class; the limit (512) is the only variable part.
Emulator: sends 'The annotation name exceeds 512 bytes.', which is not AWS's wording for AnnotationNameTooLong.
Why: the message is a requirement of the error class; a caller logging or matching it sees text AWS never produces.
Fix: the annotation-limit validator's message text.
Traces: s3.object-annotation-validation-errors-rejected, s3.put-object-annotation-limits

Cause 4 of 10: an invalid prefix on the annotation listing crashes with a 500.
AWS: refuses with code InvalidPrefix (a 4xx client error).
Emulator: answers {'Code': '500', 'Message': 'Internal Server Error'} -- an unclassified server fault for a client-side input mistake.
Why: the caller is told the service broke rather than that its prefix was invalid, and SDK retry logic will retry a request that can never succeed.
Fix: the annotation-listing handler's prefix validation, which must refuse InvalidPrefix instead of panicking into a 500.
Traces: s3.list-object-annotations-invalid-prefix

Cause 5 of 10: an invalid metadata-table schema is refused with the wrong code.
AWS: the refusal for a metadata-table configuration whose schema is invalid carries AWS's own code and message for that class, not the generic argument error.
Emulator: answers InvalidArgument.
Why: the caller cannot tell a malformed schema from any other bad argument, so it cannot act on the specific failure.
Fix: the metadata-table-configuration validator's error classification.
Traces: s3.metadata-table-configuration-invalid-schema-rejected

Cause 6 of 10: a bucket ACL carrying both a canned ACL and header grants is refused with the generic placeholder message.
AWS: InvalidRequest with the fixed message 'Specifying both Canned ACLs and Header Grants is not allowed'.
Emulator: the generic "A parameter or header in your request isn't valid. For details, see the description of this API operation."
Why: the message is the only thing that tells the caller which two inputs conflicted; the placeholder tells it nothing and matches no AWS text.
Fix: the PutBucketAcl validation, which must emit the specific conflict message.
Traces: s3.bucket-acl-conflicting-grants-rejected

Cause 7 of 10: the CopyObject precondition refusal appends the header name to AWS's message.
AWS: PreconditionFailed with exactly 'At least one of the pre-conditions you specified did not hold' -- the message is fixed and names no header.
Emulator: 'At least one of the pre-conditions you specified did not hold: x-amz-copy-source-if-match'.
Why: a client comparing the message against AWS's fixed text no longer matches.
Fix: the CopyObject precondition evaluation's message, which must not append the failing header.
Traces: s3.copy-object-source-if-match-mismatch-rejected

Cause 8 of 10: the DeleteObjects over-limit refusal appends its own explanation to AWS's MalformedXML message.
AWS: MalformedXML with exactly 'The XML you provided was not well-formed or did not validate against our published schema' when the Delete element carries more than 1000 Object entries. (Smithy: DeleteObjects models no operation errors; the 1-to-1000 bound and the message are recorded behavior.)
Emulator: appends ': a Delete request carries from 1 to 1000 Object elements, and this one carries 1001' -- the count is a value from the request, not part of AWS's message.
Why: message-matching clients diverge, and the emulator's text is not reproducible as AWS words this error class.
Fix: the DeleteObjects request validation's message.
Traces: s3.delete-objects-exceeds-max-keys-rejected

Cause 9 of 10: the UploadPart invalid-part-number message is the emulator's own sentence.
AWS: refuses PartNumber outside 1..10000 with AWS's own message for that class (Smithy: UploadPart.PartNumber is a required httpQuery integer; the 1..10000 range and its refusal message are recorded behavior).
Emulator: 'Part number must be an integer between 1 and 10000, inclusive: 0' -- AWS neither words it that way nor echoes the offending value in that form.
Why: the refusal message is a requirement of the class; this one cannot be matched or reproduced against AWS.
Fix: the UploadPart part-number validation's message.
Traces: s3.multipart-upload-part-invalid-number-rejected

Cause 10 of 10: a conditional write over a delete marker is classified as a precondition failure instead of a missing key.
AWS: PutObject with If-Match on a key whose current version is a delete marker (i.e. the key has no live object) is refused NoSuchKey -- the key does not exist, so there is no ETag to match.
Emulator: refuses PreconditionFailed.
Why: a caller using If-Match for optimistic concurrency cannot tell 'someone else changed it' (PreconditionFailed, retry with a fresh ETag) from 'it is gone' (NoSuchKey, create it), so it retries forever on a deleted key.
Fix: the conditional-write evaluation in the object-write path, which must resolve a delete-marker head to NoSuchKey before evaluating If-Match.
Traces: s3.conc-s3-conditional-writes-if-match-over-delete-marker-rejected

## Evidence
s3.abort-multipart-nosuchupload-after-complete -> AssertionError: expected AbortMultipartUpload on an already-completed upload id to succeed as a no-op, got 'NoSuchUpload': 'The specified upload does not exist. The upload ID may be invalid, or the upload may have been aborted or completed.'
s3.multipart-list-uploads-then-abort-nonexistent -> botocore.errorfactory.NoSuchUpload: An error occurred (NoSuchUpload) when calling the AbortMultipartUpload operation: The specified upload does not exist. ...
s3.annotation-name-validation-rejected -> AssertionError: expected InvalidAnnotation, got InvalidAnnotationName
s3.object-annotation-reserved-prefix-rejected -> AssertionError: expected InvalidAnnotation for 'aws:'-prefixed name, got InvalidAnnotationName
s3.object-annotation-validation-errors-rejected -> AssertionError: unexpected AnnotationNameTooLong message: 'The annotation name exceeds 512 bytes.'
s3.put-object-annotation-limits -> AssertionError: expected AnnotationNameTooLong for a 513-byte name, got ('AnnotationNameTooLong', 'The annotation name exceeds 512 bytes.')
s3.list-object-annotations-invalid-prefix -> AssertionError: expected InvalidPrefix, got {'Code': '500', 'Message': 'Internal Server Error'}
s3.metadata-table-configuration-invalid-schema-rejected -> AssertionError: InvalidArgument
s3.bucket-acl-conflicting-grants-rejected -> AssertionError: expected message 'Specifying both Canned ACLs and Header Grants is not allowed', got "A parameter or header in your request isn't valid. For details, see the description of this API operation."
s3.copy-object-source-if-match-mismatch-rejected -> AssertionError: expected ('PreconditionFailed', 'At least one of the pre-conditions you specified did not hold'), got ('PreconditionFailed', 'At least one of the pre-conditions you specified did not hold: x-amz-copy-source-if-match')
s3.delete-objects-exceeds-max-keys-rejected -> AssertionError: {'Code': 'MalformedXML', 'Message': 'The XML you provided was not well-formed or did not validate against our published schema: a Delete request carries from 1 to 1000 Object elements, and this one carries 1001'}
s3.multipart-upload-part-invalid-number-rejected -> AssertionError: Part number must be an integer between 1 and 10000, inclusive: 0
s3.conc-s3-conditional-writes-if-match-over-delete-marker-rejected -> AssertionError: expected NoSuchKey for If-Match over a marker, got 'PreconditionFailed'

## Steps to Reproduce
Replay the behavioral suite and run the s3 checks in trace_ids. Each makes one call and compares the refusal's status/code/message: AbortMultipartUpload on a completed and on a never-created upload id; PutObjectAnnotation with an invalid and an 'aws:'-prefixed name and with a 513-byte name; ListObjectAnnotations with an invalid prefix; CreateBucketMetadataConfiguration with an invalid schema; PutBucketAcl with both a canned ACL and header grants; CopyObject with a mismatched x-amz-copy-source-if-match; DeleteObjects with 1001 Object entries; UploadPart with PartNumber=0; PutObject with If-Match on a key hidden by a delete marker.
