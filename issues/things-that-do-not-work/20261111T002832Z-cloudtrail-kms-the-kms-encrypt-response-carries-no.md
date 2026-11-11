Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__kms.kms-encryption-context-and-call-count-are-observable-only-at-the-trail
Issue-Key: cloudtrail__kms.kms-encryption-context-and-call-count-are-observable-only-at-the-trail|run#20|KeyError|sdk_request_id = last["ResponseMetadata"]["RequestId"]
Issue-Commit: 0dd76f1da95766ea213c05d03a6e5ff9181cf064

# cloudtrail__kms: the KMS Encrypt response carries no request-id header, so ResponseMetadata.RequestId is absent and an Encrypt can never be joined to its KMS last-usage or CloudTrail record

> _Note: this issue was identified against the code base at commit `0dd76f1da95766ea213c05d03a6e5ff9181cf064`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Encrypt

## Description
Cause 1 of 1: the emulator's KMS Encrypt response omits the protocol request-id header, so botocore builds a response with NO ResponseMetadata.RequestId key at all.

AWS: KMS speaks awsJson1.1 (Smithy: kms Encrypt, input KeyId/Plaintext/EncryptionContext, output CiphertextBlob/KeyId/EncryptionAlgorithm). Every AWS response — success or error — carries the service's request-id header (x-amzn-RequestId for KMS), which botocore surfaces as response['ResponseMetadata']['RequestId'] on EVERY call. That one minted id is the join key the rest of the audit surface is defined on: Smithy kms#KeyLastUsageData$KmsRequestId is the request id of the last successful cryptographic operation and kms#KeyLastUsageData$CloudTrailEventId is documented as "The CloudTrail eventId associated with the last successful cryptographic operation", and the CloudTrail record for that call carries the same string in its requestID field. The id is per-request and opaque; its VALUE is never a requirement, only its PRESENCE on every response and its identity across the three surfaces.

Emulator: the key is missing, not merely different. The check dies at `sdk_request_id = last["ResponseMetadata"]["RequestId"]` with `KeyError: 'RequestId'` on the response to a plain Encrypt (KeyId = a key CreateKey just returned, Plaintext present, EncryptionContext a 3-entry map). The call itself returned 200 with a CiphertextBlob; only the header is absent. Because the key is absent the check cannot even reach its GetKeyLastUsage / LookupEvents assertions, so KmsRequestId, CloudTrailEventId and the per-call Encrypt count are untested behind this one defect.

Why: a client cannot correlate a cryptographic operation with anything. 'Which trail record is this Encrypt?' and 'which request did KMS last serve for this key?' are exactly the KmsRequestId/CloudTrailEventId joins, and both are unreachable. Any SDK code that logs or retries on ResponseMetadata.RequestId raises KeyError instead of reading an id, and botocore's own retry/throttling diagnostics lose their handle.

Note: open issue 20261105T112044Z-cloudtrail-lambda-s3-the-lambda-invoke-response-carries.md reports the same SHAPE of defect for the Lambda Invoke response; it names Lambda's handler and does not cover KMS, so this is a separate root cause in the KMS response path.

Fix: the KMS response serialization / outbound-header layer for the kms service (the component that stamps the awsJson1.1 response headers for every KMS operation, not Encrypt alone) must emit the request-id header on every response, and the same minted id must be what GetKeyLastUsage reports as KeyLastUsage.KmsRequestId and what the CloudTrail record for that call reports as requestID.

Traces: cloudtrail__kms.kms-encryption-context-and-call-count-are-observable-only-at-the-trail (verdict error, KeyError: 'RequestId').

## Evidence
Expected: kms.encrypt(KeyId=<key>, Plaintext=<bytes>, EncryptionContext={...}) returns a response whose ResponseMetadata contains a RequestId (non-empty opaque string), and that same string appears as GetKeyLastUsage -> KeyLastUsage.KmsRequestId and as the CloudTrail record's requestID for that Encrypt.

Actual: the Encrypt response has no RequestId in ResponseMetadata. Replay detail:
  File "check_cloudtrail__kms_kms_encryption_context_and_call_count_are_observable_only_at_the_trail.py", line 150, in run
    sdk_request_id = last["ResponseMetadata"]["RequestId"]
  KeyError: 'RequestId'
The response did carry CiphertextBlob, so the operation succeeded; only the request-id header is missing. The missing key is the requirement here — the id's literal value in any one run is not.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. kms.create_key(Description=..., Tags=[{"TagKey":"sim2real","TagValue":"..."}]) and take KeyMetadata.KeyId.
3. resp = kms.encrypt(KeyId=<that key id>, Plaintext=b"hello", EncryptionContext={"a":"b"}).
4. Read resp["ResponseMetadata"]["RequestId"] -> raises KeyError: 'RequestId'. On AWS it is a non-empty id.
5. Follow-on (blocked by step 4): kms.get_key_last_usage(KeyId=<key id>) should report KeyLastUsage.KmsRequestId equal to that id and a CloudTrailEventId that cloudtrail.lookup_events returns as an EventId.
