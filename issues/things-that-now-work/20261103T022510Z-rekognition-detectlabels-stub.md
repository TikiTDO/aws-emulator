Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.bucket-versioning-status-key-absent-until-versioning-is-enabled dynamodb__lambda__rekognition__s3.detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format dynamodb__lambda__rekognition__s3.detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object dynamodb__lambda__rekognition__s3.detect-labels-on-an-s3-object-narrows-by-maxlabels-and-minconfidence
Issue-Key: dynamodb__lambda__rekognition__s3.bucket-versioning-status-key-absent-until-versioning-is-enabled|run#18|AssertionError|assert stale_version == (
Issue-Key: dynamodb__lambda__rekognition__s3.detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format|run#9|AssertionError|assert got == expected, (
Issue-Key: dynamodb__lambda__rekognition__s3.detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object|run#9|AssertionError|assert got == expected, (
Issue-Key: rekognition|DetectLabels|NotImplemented
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# Implement rekognition DetectLabels: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectLabels

## Description
`DetectLabels` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/detect_labels.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `DetectLabels` call, so each one unblocks only when `DetectLabels` works: `dynamodb__lambda__rekognition__s3.bucket-versioning-status-key-absent-until-versioning-is-enabled`, `dynamodb__lambda__rekognition__s3.detect-labels-on-a-jpg-named-object-holding-text-rejected-invalid-image-format`, `dynamodb__lambda__rekognition__s3.detect-labels-on-a-missing-s3-object-rejected-invalid-s3-object`, `dynamodb__lambda__rekognition__s3.detect-labels-on-an-s3-object-narrows-by-maxlabels-and-minconfidence`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/detect_labels.rs`. Replaying the corpus, the emulator refused the `DetectLabels` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_dynamodb__lambda__rekognition__s3_bucket_versioning_status_key_absent_until_versioning_is_enabled.py", line 90, in run
    assert stale_version == (
           ^^^^^^^^^^^^^^^^^^
AssertionError: DetectLabels given a Version on a never-versioned bucket must fail the same way as a missing key — there is no such version to fetch; got ('NotImplemented', 'operation DetectLabels is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the rekognition emulator
Call DetectLabels and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
