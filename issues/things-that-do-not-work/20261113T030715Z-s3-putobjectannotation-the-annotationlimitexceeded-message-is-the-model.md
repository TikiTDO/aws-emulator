Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.object-annotation-validation-errors-rejected s3.put-object-annotation-limits
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.object-annotation-validation-errors-rejected|run#30|AssertionError|assert message == LIMIT_EXCEEDED_MESSAGE, f"unexpected AnnotationLimitExceeded message: {message!r}"
Issue-Key: s3.put-object-annotation-limits|run#23|AssertionError|assert code_over_cap == ("AnnotationLimitExceeded", (
Issue-Commit: 00fa4fcb7857a2daae640c829f8c16fc60943323

# S3 PutObjectAnnotation: the AnnotationLimitExceeded message is the model's doc prose instead of AWS's counted message

> _Note: this issue was identified against the code base at commit `00fa4fcb7857a2daae640c829f8c16fc60943323`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObjectAnnotation

## Description
One net-new root cause in the s3 group, found by replaying the behavioral suite.

Cause 1 of 1: when an object version already holds the maximum number of annotations, `PutObjectAnnotation` is correctly refused with HTTP 400 and error code `AnnotationLimitExceeded`, but the error MESSAGE is wrong — the emulator serves the Smithy shape's documentation sentence rather than the message AWS puts on the wire.

AWS: Smithy (s3): `PutObjectAnnotation` declares the error `AnnotationLimitExceeded` as a client error with `httpError: 400`; the shape's `@documentation` is the prose "The request would exceed the maximum number of annotations allowed per object." — that is the API-reference description of the error, NOT its wire message. Recorded behavior: the refusal's `Error.Message` is the counted form

    The object annotation count (<count>) exceeds the limit (<limit>).

where the fixed text is `The object annotation count (` … `) exceeds the limit (` … `).` and the two variable parts come from the request and the emulator's own state: `<count>` is the annotation count the write WOULD produce (the number already stored on that object version plus one) and `<limit>` is the per-object-version annotation cap in force (1000 in the recordings). Both recorded checks assert this same templated message, one of them computing it from the cap rather than hard-coding it.

Emulator: HTTP 400 and code `AnnotationLimitExceeded` are right, but `Error.Message` is the literal doc string `The request would exceed the maximum number of annotations allowed per object.` — no counts, and no dependence on the request or state.

Why: the error message is part of the contract. A client that parses or logs the refusal to report "you are at N of M annotations" (or that asserts on AWS's wording in a test) sees a different sentence, and the two values AWS reports — how many annotations the write would make and what the ceiling is — are simply absent, so the caller cannot tell how far over the cap it is or what the cap is. It also mis-signals the shape of the whole annotation error family: the neighbouring refusals this operation serves (`AnnotationNameTooLong` → "Your annotation name is too long.", `InvalidAnnotation` → "Your annotation name cannot start with 'aws' or 's3'." / "Your annotation name contains invalid characters.", `NoSuchAnnotation` → "The specified annotation does not exist.") all match today, so this one message is the lone outlier.

Fix: the `PutObjectAnnotation` handler's cap validation / its error serializer in the s3 service — the `AnnotationLimitExceeded` message must be formatted from the resulting annotation count and the cap in force, not taken from the shape's documentation string. Nothing else about the refusal needs to change: status, code, and the no-side-effect behaviour (the rejected annotation is absent and no existing annotation is evicted) already hold.

Traces: s3.object-annotation-validation-errors-rejected, s3.put-object-annotation-limits

## Evidence
s3.object-annotation-validation-errors-rejected:
  AssertionError: unexpected AnnotationLimitExceeded message: 'The request would exceed the maximum number of annotations allowed per object.'
  expected (LIMIT_EXCEEDED_MESSAGE): 'The object annotation count (1001) exceeds the limit (1000).'

s3.put-object-annotation-limits:
  AssertionError: expected AnnotationLimitExceeded at the 1,001st annotation, got ('AnnotationLimitExceeded', 'The request would exceed the maximum number of annotations allowed per object.')
  expected: ('AnnotationLimitExceeded', 'The object annotation count (1001) exceeds the limit (1000).')

The literals 1001 and 1000 in these recordings are the template's variable parts for a cap of 1000 with 1000 annotations already stored — `<count>` = stored + 1, `<limit>` = the cap; the requirement is the fixed text around them, not those two numbers. The emulator's string is byte-identical to the Smithy `AnnotationLimitExceeded` shape documentation (`<p>The request would exceed the maximum number of annotations allowed per object.</p>`), which is what identifies the source of the defect. Both checks confirm the rest of the refusal is correct: the code is `AnnotationLimitExceeded`, the over-cap annotation does not appear in `ListObjectAnnotations`, no existing annotation was evicted, and after `DeleteObjectAnnotation` frees one slot the identical `PutObjectAnnotation` succeeds.

## Steps to Reproduce
1. Create a general-purpose bucket and `PutObject` a key.
2. Call `PutObjectAnnotation` on that key with distinct `AnnotationName`s until the object version holds the per-object cap (1000 annotations); confirm with `ListObjectAnnotations(MaxAnnotationResults=1000)` that 1000 are present and the listing is not truncated.
3. Call `PutObjectAnnotation` once more with a new name (e.g. `one-too-many`).
4. Expected: HTTP 400, `Error.Code = AnnotationLimitExceeded`, `Error.Message = "The object annotation count (1001) exceeds the limit (1000)."` — i.e. the counted template filled from the resulting count and the cap.
   Actual: HTTP 400 and the right code, but `Error.Message = "The request would exceed the maximum number of annotations allowed per object."`.
