Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backupsearch.start-search-job-invalid-encryption-key-arn-rejected
Issue-Shape-Id: com.amazonaws.backupsearch#CryoBackupSearchService
Issue-Key: backupsearch.start-search-job-invalid-encryption-key-arn-rejected|run#4|AssertionError|assert message.startswith(
Issue-Commit: a03f808379986133e0f00cdb9a2ead2b78e9a26e

# StartSearchJob with a malformed EncryptionKeyArn drops the relayed KMS error envelope from the ResourceNotFoundException message

> _Note: this issue was identified against the code base at commit `a03f808379986133e0f00cdb9a2ead2b78e9a26e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSearchJob

## Description
One root cause, found by replaying the behavioral suite (simparity-behavioral-aws-v1) against the booted emulator.

Cause 1 of 1: the ResourceNotFoundException raised for an unresolvable EncryptionKeyArn carries only the bare KMS reason, not the relayed-KMS-error envelope AWS wraps it in.

AWS: Smithy StartSearchJob (PUT /search-jobs, restJson1) takes EncryptionKeyArn -> com.amazonaws.backupsearch#EncryptionKeyArn, a string with @arnReference{type: AWS::KMS::Key} and no @pattern/@length constraint, and declares ResourceNotFoundException (httpError 404, @required members message/resourceId/resourceType, documented only as "Request references a resource which does not exist"). Because the member carries no syntactic constraint, a malformed value is not caught by request validation: BackupSearch hands it to KMS, KMS refuses it, and BackupSearch relays KMS's own error text verbatim inside its 404. Recorded behavior is therefore the authority for the message, and the recording shows the relayed form:
  "Invalid keyId '<EncryptionKeyArn as sent>' (Service: Kms, Status Code: 400, Request ID: <KMS's per-call request id>) (SDK Attempt Count: 1)"
Fixed text of that template: the literal prefix "Invalid keyId '", the literal segment "' (Service: Kms, Status Code: 400, Request ID: ", and the literal suffix ") (SDK Attempt Count: 1)". Variable parts, filled from the request and from the downstream call: the quoted key id (exactly the EncryptionKeyArn value the caller sent) and the KMS request id (a per-call value, which is why the check asserts around it rather than on it).

Emulator: same error code and status (ResourceNotFoundException / 404 — the code assertion passed), but the message is only the bare reason "Invalid keyId 'not-an-arn-at-all'". Both literal segments of the relay envelope — "(Service: Kms, Status Code: 400, Request ID: <id>)" and " (SDK Attempt Count: 1)" — are absent. The positive control in the same check (the identical StartSearchJob without EncryptionKeyArn) is accepted, so only the refusal path diverges.

Why: this message is the only human- and machine-readable detail of the refusal (ResourceNotFoundException exposes nothing else about the KMS failure), and the envelope is what tells a caller the refusal came from KMS rather than from BackupSearch's own resource lookup. A client that distinguishes "the KMS key you named is unusable" from "the search job/resource is gone" by matching the "(Service: Kms, Status Code: 400, ...)" marker, or that parses the relayed service/status out of the text for logging and retry decisions, mis-attributes the failure to BackupSearch and loses the downstream status entirely.

Fix: the StartSearchJob handler's EncryptionKeyArn resolution path (where the key is resolved and the modeled ResourceNotFoundException is constructed) — render the message in the relayed-KMS-error form, i.e. the bare reason plus the "(Service: Kms, Status Code: 400, Request ID: <id>)" and " (SDK Attempt Count: 1)" segments, with the key id taken from the request and the request id generated per call. Note the already-open issue 20261022T215802Z-backupsearch-refusal-fidelity-on-the-wire-not-found.md covers different BackupSearch refusal causes (not-found classification for malformed job ids, the fixed "Resource is not found" text, list-enum and @pattern validation message rendering, TagResource 204) and carries no key for this EncryptionKeyArn relay path; it does not cover this cause.

Traces: behavioral check backupsearch.start-search-job-invalid-encryption-key-arn-rejected.

## Evidence
Behavioral suite run against the booted emulator: 23/24 passed, 1 mismatched, 0 errored, 0 skipped, bootstrap_signal false.

[mismatch] backupsearch.start-search-job-invalid-encryption-key-arn-rejected
  AssertionError: unexpected message: "Invalid keyId 'not-an-arn-at-all'"
  raised by the assertion
    message.startswith("Invalid keyId 'not-an-arn-at-all' (Service: Kms, Status Code: 400, Request ID: ")
  (the companion assertion message.endswith(") (SDK Attempt Count: 1)") fails for the same reason).

Expected (AWS, template): Error.Code = "ResourceNotFoundException", HTTP 404, Error.Message = "Invalid keyId '<EncryptionKeyArn as sent>' (Service: Kms, Status Code: 400, Request ID: <per-call KMS request id>) (SDK Attempt Count: 1)".
Actual (emulator): Error.Code = "ResourceNotFoundException", HTTP 404, Error.Message = "Invalid keyId '<EncryptionKeyArn as sent>'" — nothing after the closing quote.
The code/status assertions passed; only the message diverges, and the divergence is the two fixed literal segments of the envelope, not the per-call request id inside it.
Positive control in the same check (StartSearchJob with the same Name/SearchScope and no EncryptionKeyArn) succeeded and returned a SearchJobIdentifier.

## Steps to Reproduce
Boot the emulator and point a botocore backupsearch client (restJson1) at it.
1. Call start_search_job(Name=<any name <=500 chars>, SearchScope={"BackupResourceTypes": ["S3"]}, EncryptionKeyArn="not-an-arn-at-all") — any value that is not a resolvable KMS key ARN works.
2. Catch the ClientError and inspect response["Error"]["Code"] and response["Error"]["Message"]. Code is ResourceNotFoundException in both AWS and the emulator; the emulator's message stops at "Invalid keyId 'not-an-arn-at-all'", while AWS appends " (Service: Kms, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)".
3. Positive control: repeat the same call without EncryptionKeyArn — it is accepted and returns SearchJobIdentifier (then stop_search_job on it to clean up).
