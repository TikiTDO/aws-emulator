Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.x-amz-crc32-matches-body-on-success-and-error dynamodb.sigv4-query-string-presigned-list-tables-accepted dynamodb.response-requestid-format-and-content-length-match-body-on-error dynamodb.sigv4-auth-missing-forms-give-missing-authentication-token dynamodb.incomplete-sigv4-signature-lists-missing-components dynamodb.sigv4-query-string-presigned-list-tables-tampered-signature-rejected dynamodb.describetable-authorization-header-and-x-amz-algorithm-query-conflict-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.sigv4-query-string-presigned-list-tables-accepted|run#13|AssertionError|assert crc == str(binascii.crc32(raw) & 0xFFFFFFFF), (
Issue-Key: dynamodb.response-requestid-format-and-content-length-match-body-on-error|last#10|AssertionError|assert _REQUEST_ID_RE.match(request_id), (
Issue-Key: dynamodb.sigv4-auth-missing-forms-give-missing-authentication-token|run#16|AssertionError|assert status == 400, f"{label}: expected status 400, got {status!r}: {raw_body!r}"
Issue-Key: dynamodb.incomplete-sigv4-signature-lists-missing-components|run#17|AssertionError|assert status == 400, f"a bare 'AWS4-' Authorization header gave status {status!r}: {body!r}"
Issue-Key: dynamodb.sigv4-query-string-presigned-list-tables-tampered-signature-rejected|run#15|AssertionError|assert resp.status == 400, f"a tampered query-string signature gave {resp.status!r}: {body!r}"
Issue-Key: dynamodb.describetable-authorization-header-and-x-amz-algorithm-query-conflict-rejected|run#25|AssertionError|assert err_type == _ISE, (
Issue-Related: 20261029T180026Z-dynamodb-partiql-resource-policy-arn-addressing-crc32-header.md
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: the HTTP/SigV4 layer omits x-amz-crc32, serves a non-AWS request id, and answers 403 UnrecognizedClientException for every malformed-auth form (5 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
Five net-new root causes below the operation handlers, in the emulator's awsJson1.0 HTTP and SigV4 layer. They apply to every DynamoDB operation, not only the ones the checks happened to call. Smithy: service protocol awsJson1_0, auth scheme aws.auth#sigv4 (sigv4Name dynamodb); the model carries no header or signing text, so the recorded wire behaviour is the contract here.

Cause 1 of 5 — no x-amz-crc32 response header. AWS returns x-amz-crc32 on every DynamoDB response, success and error alike, holding the CRC32 of the response body; SDKs verify it and retry on mismatch. Emulator: the header is absent from 200s (headers seen: content-length, content-type, date, x-amzn-requestid) and from presigned-request responses. Why: a client with response-checksum validation enabled either fails the call or silently loses its integrity check. Fix: the awsJson1.0 response serializer / HTTP layer. Traces: x-amz-crc32-matches-body-on-success-and-error, sigv4-query-string-presigned-list-tables-accepted.

Cause 2 of 5 — x-amzn-requestid is a lowercase hyphenated UUID. AWS DynamoDB request ids are 52 uppercase alphanumeric characters (^[0-9A-Z]{52}$); the emulator serves e.g. a v4 UUID. The open issue 20261031T122518Z-cloudformation-dynamodb-lambda-dynamodb-responses-carry-a-lowercase-uuid-as-x.md records the same defect under a different service heading and does not carry this check's replay key. Fix: request-id generation in the HTTP layer (and keep content-length consistent with the body on errors). Traces: response-requestid-format-and-content-length-match-body-on-error.

Cause 3 of 5 — a missing or structurally incomplete Authorization header answers 403 UnrecognizedClientException. AWS: a request with NO Authorization header (and no presigned query credentials) is 400 MissingAuthenticationToken with the message 'Missing Authentication Token'; a header present but missing SigV4 components is refused with a message that names the missing components. The emulator answers 403 with {"__type":"UnrecognizedClientException","message":"The security token included in the request is invalid."} for both. Why: a client cannot tell 'you did not sign' from 'your credentials are not recognised', and the status is wrong, so retry/credential-refresh logic takes the wrong path. Fix: the SigV4 request-parsing stage, before credential lookup. Traces: sigv4-auth-missing-forms-give-missing-authentication-token, incomplete-sigv4-signature-lists-missing-components.

Cause 4 of 5 — a tampered presigned query-string signature is refused with the wrong status. AWS refuses a bad query-string signature as 400 InvalidSignatureException 'The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method. Consult the service documentation for details.'; the emulator serves that code and message but with HTTP 403. Fix: the status mapping for signature-mismatch in the SigV4 verifier. Traces: sigv4-query-string-presigned-list-tables-tampered-signature-rejected.

Cause 5 of 5 — a request carrying BOTH an Authorization header and X-Amz-Algorithm in the query string is not detected as a conflicting-auth request. AWS refuses it as InvalidSignatureException; the emulator proceeds to dispatch and answers __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' — i.e. the request was executed. Fix: the conflicting-auth-form check in the SigV4 verifier. Traces: describetable-authorization-header-and-x-amz-algorithm-query-conflict-rejected.

## Evidence
Expected vs actual, verbatim from the replay:
- x-amz-crc32-matches-body-on-success-and-error: 'CreateTable 200: no x-amz-crc32 header among ['content-length', 'content-type', 'date', 'x-amzn-requestid']'.
- sigv4-query-string-presigned-list-tables-accepted: 'x-amz-crc32 None is not the CRC32 of the 200 response body'.
- response-requestid-format-and-content-length-match-body-on-error: "CreateTable 200: x-amzn-requestid 'de74fa61-c59c-4868-b3f2-7b290c847d44' does not match '^[0-9A-Z]{52}$'" (that UUID is this run's value; the 52-character uppercase format is the requirement).
- sigv4-auth-missing-forms-give-missing-authentication-token: 'no_auth_header: expected status 400, got 403: b'{"__type":"UnrecognizedClientException","message":"The security token included in the request is invalid."}''.
- incomplete-sigv4-signature-lists-missing-components: "a bare 'AWS4-' Authorization header gave status 403: {'__type': 'UnrecognizedClientException', 'message': 'The security token included in the request is invalid.'}".
- sigv4-query-string-presigned-list-tables-tampered-signature-rejected: 'a tampered query-string signature gave 403' with the InvalidSignatureException body; AWS uses 400.
- describetable-authorization-header-and-x-amz-algorithm-query-conflict-rejected: "a request carrying both Authorization and X-Amz-Algorithm gave __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException', expected 'InvalidSignatureException'".

## Steps to Reproduce
1. CreateTable with a valid signed request; inspect the response headers for x-amz-crc32 and check x-amzn-requestid against ^[0-9A-Z]{52}$. Repeat on an error response (e.g. DescribeTable of a missing table).
2. POST a well-formed DynamoDB body with NO Authorization header — expect 400 MissingAuthenticationToken.
3. POST the same body with Authorization: 'AWS4-' only — expect a 400-class refusal naming the missing SigV4 components.
4. Presign a ListTables GET, alter one character of X-Amz-Signature, send it — expect 400 InvalidSignatureException.
5. Send a DescribeTable request carrying both a valid Authorization header and an X-Amz-Algorithm query parameter — expect InvalidSignatureException, not the operation's result.
