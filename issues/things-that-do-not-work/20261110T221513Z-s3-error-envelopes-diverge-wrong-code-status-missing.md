Issue-Source: trace-sweep-divergence
Issue-Trace: s3-b2b-summary-inline-tagging-header-mistakes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-b2b-summary-inline-tagging-header-mistakes s3-object-tagging-acl-and-torrent s3-csvcols-part-number-reads s3-inventory-configuration-id-length-charset-and-case-rules s3-inventory-destination-bucket-must-be-a-regionless-arn s3-inventory-destination-encryption-key-shape s3-inventory-filter-prefix-must-be-non-empty s3-rcsv-encryption-header-mistakes-on-sse-kms-read
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObjectTagging|4xx->4xx|$.Error.Key
Issue-Key: s3|GetObject|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketInventoryConfiguration|4xx->2xx|$headers.content-length
Issue-Key: s3|GetObject|4xx->4xx|$.Error.ActualObjectSize
Issue-Key: s3|GetObject|4xx->4xx|$.Error.RangeRequested
Issue-Related: 20261102T111455Z-s3-inventory-configuration-plane-neither-validates-nor-persists.md
Issue-Related: 20261103T054236Z-s3-object-read-request-validation-diverges-partnumber-sse.md
Issue-Commit: 46b1ef78a2de21b94439ce78901879d208363fd1

# s3 error envelopes diverge: wrong code/status, missing error detail members, and unvalidated inventory writes

> _Note: this issue was identified against the code base at commit `46b1ef78a2de21b94439ce78901879d208363fd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObjectTagging

## Description
Four net-new causes, all in the s3 object/bucket-subresource error path: the emulator decides the wrong failure (or no failure at all) and, where it does fail, serves an error envelope whose code, status, message and detail members are not S3's. All four share one root shape — the request-level validation/precedence that produces S3's REST error document is either absent or implemented with the emulator's own vocabulary instead of S3's. 4 causes, each with its own fixture and step.

=== Cause 1 of 4: GetObjectTagging NoSuchKey reports a truncated <Key> ===
AWS: Smithy: GetObjectTagging http GET /{Bucket}/{Key+}?tagging, Key -> ObjectKey @required @length(min 1), Bucket -> BucketName @httpLabel. The modeled error set for the operation is empty, so the 404 envelope is recorded behavior only: S3's REST error document for a missing object carries Code=NoSuchKey and a <Key> element that echoes the object key as it was addressed in the request, verbatim and in full. The fixed text of that envelope is the Code and Message; the <Key> element is a value taken from the request, so the requirement is "the key the client asked for", not any one recording's string.
Emulator: the 404 is produced with the right status, but $.Error.Key has had its leading path segment removed — the recorded key is '<prefix-segment>/reports/summary.json' and the emulator answers 'reports/summary.json'. Hypothesis: the handler resolves the bucket from the Host header for a virtual-hosted-style request and then ALSO strips the first path segment as if the request were path-style, so the first segment of the key is consumed twice and never reaches the error document.
Why: a client that keys retry/lookup logic off Error.Key (SDK NoSuchKey handling, S3 inventory/repair tooling, any code that logs the missing key and re-requests it) is handed a key that does not exist at that bucket and will re-request the wrong object or report the wrong path to its operator. The same truncation would corrupt every other place the key is echoed back.
Fix: the key extraction shared by the object-level route (bucket/key split for addressing style) and the GetObjectTagging handler's NoSuchKey error construction — the error document must be built from the fully resolved request key.
Traces: s3-b2b-summary-inline-tagging-header-mistakes#task#6, s3-object-tagging-acl-and-torrent#task#13.

=== Cause 2 of 4: GetObject partNumber parse failure leaks a Rust deserializer message as ValidationException ===
AWS: Smithy: GetObject.PartNumber -> PartNumber, @httpQuery("partNumber"), type integer. A partNumber query value that is not a valid part number is refused 400 with Code=InvalidArgument and Message 'Part number must be an integer between 1 and 10000, inclusive' (fixed text), plus the InvalidArgument detail members <ArgumentName> and <ArgumentValue> — ArgumentName is the fixed member name 'partNumber' and ArgumentValue is the rejected value copied from the request's query string. Status, code and the detail members are as recorded; the bounds 1..10000 are S3's documented part-number range.
Emulator: answers 400 but with Code 'ValidationException' and Message 'invalid i32 `one`: invalid digit found in string', and emits neither ArgumentName nor ArgumentValue. 'ValidationException' is not an S3 error code at all (S3 serves REST-XML <Error><Code>), and the message is the internal integer-parser's text, which exposes the host language's type name and parser wording to the client.
Why: an SDK matching on InvalidArgument (or on the ArgumentName/ArgumentValue pair to tell the caller which parameter it got wrong) sees an unknown code and no detail members, so the failure is surfaced as a generic/unmodeled error instead of a parameter error, and the caller is never told which argument was bad. The leaked parser text also changes with the emulator's implementation rather than with the API.
Fix: GetObject's request validation — parse partNumber as an optional integer and, on a parse failure or an out-of-range value, raise the S3 InvalidArgument error (with ArgumentName/ArgumentValue) instead of letting the generic query-deserializer error surface. The RequestId formatting that also differs on this step is already owned by 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md and is not part of this cause.
Traces: s3-csvcols-part-number-reads#task#7.

=== Cause 3 of 4: PutBucketInventoryConfiguration accepts malformed inventory configurations with an empty 204 ===
AWS: Smithy: PutBucketInventoryConfiguration http PUT /{Bucket}?inventory with code 200; Id -> InventoryId @httpQuery("id") @required; InventoryConfiguration @httpPayload @required. An InventoryConfiguration payload that does not validate against the published schema is refused 400 with Code=MalformedXML and Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text, no variable parts), with the RequestId/HostId members of the standard S3 error envelope and a Content-Length for that body. The recorded corpus exercises four distinct malformations and AWS refuses all four the same way: an Id outside the permitted length/charset/case rules, a destination bucket that is not a regionless S3 ARN, a malformed destination SSE-KMS key shape, and an empty filter prefix.
Emulator: every one of those four is accepted — status 204 with an entirely empty body and no Content-Length, so the invalid configuration is stored and no error document is produced at all. (204 is also not this operation's success status; the model binds code 200.)
Why: a client that relies on S3 to reject an unusable inventory configuration gets a success, so the bucket ends up carrying a configuration that could never produce a report, and the error surfaces much later (or never) instead of at the PUT. Any negative test or IaC pre-flight that asserts MalformedXML passes silently against the emulator.
Fix: PutBucketInventoryConfiguration's request validation/deserializer — validate the InventoryConfiguration payload (Id rules, Destination bucket ARN form, destination encryption key shape, non-empty filter prefix) and serve the MalformedXML error envelope on failure; also return the modeled success status rather than 204.
Traces: s3-inventory-configuration-id-length-charset-and-case-rules#task#4, s3-inventory-destination-bucket-must-be-a-regionless-arn#task#3, s3-inventory-destination-bucket-must-be-a-regionless-arn#task#4, s3-inventory-destination-encryption-key-shape#task#3, s3-inventory-filter-prefix-must-be-non-empty#task#3.

=== Cause 4 of 4: a ranged GetObject that is both out of range and carries SSE-C headers answers 400 InvalidRequest instead of 416 InvalidRange ===
AWS: Smithy: GetObject.Range -> @httpHeader("Range"); SSECustomerAlgorithm/SSECustomerKey/SSECustomerKeyMD5 -> the x-amz-server-side-encryption-customer-* headers; GetObject's modeled errors are only InvalidObjectState(403)/NoSuchKey(404), so this envelope is recorded behavior only. For a GET whose Range cannot be satisfied by the object, S3 answers HTTP 416 with Code=InvalidRange and Message 'The requested range is not satisfiable' (fixed text) plus the InvalidRange detail members <RangeRequested>, echoing the request's Range header value, and <ActualObjectSize>, the stored object's length in bytes — both values from the request and from state, not literals. The recording shows this refusal winning even though the same request also carries SSE-C headers against an SSE-KMS object: range satisfiability is decided for this request, and the encryption-parameter objection is not what the client is told.
Emulator: answers 400 with Code 'InvalidRequest' and Message 'The encryption parameters are not applicable to this object.', and emits neither RangeRequested nor ActualObjectSize. So the status class (400 vs 416), the code, the message and both detail members all differ.
Why: 416 is the status range-aware clients branch on — a resumable downloader or byte-range reader treats 416 as "re-probe the object length" (and reads ActualObjectSize to do it) while a 400 is an unretryable request error. Against the emulator such a client aborts the transfer instead of recovering, and it is additionally told the wrong reason for the failure.
Fix: GetObject's validation ordering and its InvalidRange error construction — evaluate range satisfiability for the request as S3 does, and serve the 416 InvalidRange envelope populated with the requested range and the object's actual size instead of collapsing the failure into the encryption-parameter InvalidRequest.
Traces: s3-rcsv-encryption-header-mistakes-on-sse-kms-read#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (296 fixtures, 3264 steps, 3045 passed, score 93.3%, emulator_up=true, bootstrap_signal=false). The four clusters below are the only divergences in the run that are neither stub-blocked nor owned by an already-open issue.

Cause 1 — fixture s3-b2b-summary-inline-tagging-header-mistakes, step task#6, GetObjectTagging, verdict mismatch, expected 404 / actual 404:
  $.Error.Key: value_mismatch (expected 'sim2real-probe-b2b-tagh-48a3cc39/reports/summary.json', got 'reports/summary.json')
  cluster also covers s3-object-tagging-acl-and-torrent#task#13 (affected_count 2)
  divergence_key: service=s3|GetObjectTagging|status=404->404|keys=$.Error.Key

Cause 2 — fixture s3-csvcols-part-number-reads, step task#7, GetObject, verdict mismatch, expected 400 / actual 400:
  $.Error.Code: value_mismatch (expected 'InvalidArgument', got 'ValidationException')
  $.Error.Message: value_mismatch (expected 'Part number must be an integer between 1 and 10000, inclusive', got 'invalid i32 `one`: invalid digit found in string')
  $.Error.ArgumentName: missing (expected 'partNumber', got None)
  $.Error.ArgumentValue: missing (expected 'one', got None)
  divergence_key: service=s3|GetObject|status=400->400|keys=$.Error.ArgumentName,$.Error.ArgumentValue,$.Error.Code,$.Error.Message,$.Error.RequestId
  (the RequestId leg of this cluster is owned by 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md and is excluded)

Cause 3 — fixture s3-inventory-configuration-id-length-charset-and-case-rules, step task#4, PutBucketInventoryConfiguration, verdict mismatch, expected 400 / actual 204:
  status: expected 400, got 204
  $: empty_body (expected {'Error': {'Code': 'MalformedXML', 'Message': 'The XML you provided was not well-formed or did not validate against our published schema', 'RequestId': ..., 'HostId': ...}}, got {})
  $headers.content-length: missing (expected '280', got None)
  cluster also covers s3-inventory-destination-bucket-must-be-a-regionless-arn#task#3, s3-inventory-destination-bucket-must-be-a-regionless-arn#task#4, s3-inventory-destination-encryption-key-shape#task#3, s3-inventory-filter-prefix-must-be-non-empty#task#3 (affected_count 5)
  divergence_key: service=s3|PutBucketInventoryConfiguration|status=400->204|keys=$,$headers.content-length,status

Cause 4 — fixture s3-rcsv-encryption-header-mistakes-on-sse-kms-read, step task#5, GetObject, verdict mismatch, expected 416 / actual 400:
  status: expected 416, got 400
  $.Error.Code: value_mismatch (expected 'InvalidRange', got 'InvalidRequest')
  $.Error.Message: value_mismatch (expected 'The requested range is not satisfiable', got 'The encryption parameters are not applicable to this object.')
  $.Error.RangeRequested: missing (expected 'bytes=1000-2000', got None)
  $.Error.ActualObjectSize: missing (expected '58', got None)
  divergence_key: service=s3|GetObject|status=416->400|keys=$.Error.ActualObjectSize,$.Error.Code,$.Error.Message,$.Error.RangeRequested,status

Model grounding used: smithy_query query=operation on com-amazonaws-s3-amazons3 for GetObject (PartNumber @httpQuery partNumber integer; Range @httpHeader Range; SSECustomerAlgorithm/Key/KeyMD5 headers; errors InvalidObjectState 403, NoSuchKey 404), GetObjectTagging (GET /{Bucket}/{Key+}?tagging, Key @required ObjectKey @length min 1, no modeled errors) and PutBucketInventoryConfiguration (PUT /{Bucket}?inventory code 200, Id @httpQuery id @required, InventoryConfiguration @httpPayload @required). Where the model is silent on the refusal, the status/code/message above are taken from the recorded responses and labelled recorded behavior only in the description.

Related open issues that do NOT cover these causes: 20261110T134656Z-s3-object-level-requests-are-mis-evaluated-and.md (object-level error envelopes, different clusters), 20261110T210333Z-s3-6-net-new-causes-in-the-sse.md (SSE-C write plane), 20261104T021914Z-s3-analytics-configuration-writes-are-accepted-unvalidated-and.md (analytics, not inventory). Each of the four clusters above reports owned_by == [] in this replay.

## Steps to Reproduce
Boot the emulator from this checkout and replay the corpus (replay_wire_corpus against trace set simparity-wire-aws-v1), or drive the four steps directly against the emulator's S3 data listener:

1. Cause 1 — fixture s3-b2b-summary-inline-tagging-header-mistakes, step task#6. Create the probe bucket, do NOT write the object, then GET the object's tagging subresource (GET /<bucket>/<multi-segment-key>?tagging) for a key whose first segment is itself a path segment. Expect 404 with <Error><Code>NoSuchKey</Code> and <Key> equal to the full requested key; observe <Key> with its leading segment stripped. Same divergence at s3-object-tagging-acl-and-torrent#task#13.
2. Cause 2 — fixture s3-csvcols-part-number-reads, step task#7. GET an existing object with a non-numeric partNumber query value (e.g. ?partNumber=one). Expect 400 InvalidArgument / 'Part number must be an integer between 1 and 10000, inclusive' with <ArgumentName>partNumber</ArgumentName> and <ArgumentValue> echoing the supplied value; observe 400 with Code ValidationException and the internal i32 parse message, no ArgumentName/ArgumentValue.
3. Cause 3 — fixture s3-inventory-configuration-id-length-charset-and-case-rules, step task#4 (and the sibling steps s3-inventory-destination-bucket-must-be-a-regionless-arn#task#3 / #task#4, s3-inventory-destination-encryption-key-shape#task#3, s3-inventory-filter-prefix-must-be-non-empty#task#3). PUT /<bucket>?inventory&id=<id> with an InventoryConfiguration that breaks one of: the Id length/charset/case rules, the regionless-ARN form of the destination bucket, the destination SSE-KMS key shape, or a non-empty filter prefix. Expect 400 MalformedXML with the standard S3 error envelope and a Content-Length; observe 204 with an empty body and the configuration stored (a following GetBucketInventoryConfiguration returns it).
4. Cause 4 — fixture s3-rcsv-encryption-header-mistakes-on-sse-kms-read, step task#5. Write a small object (58 bytes in the recording) under SSE-KMS, then GET it with a Range header beyond the object's length (bytes=1000-2000) while also sending x-amz-server-side-encryption-customer-* headers. Expect 416 InvalidRange / 'The requested range is not satisfiable' with <RangeRequested> echoing the Range header and <ActualObjectSize> equal to the stored object's byte length; observe 400 InvalidRequest / 'The encryption parameters are not applicable to this object.' with neither detail member.

Re-replay all four fixture#step bindings (the representative plus the three in trace_ids) to confirm a fix; each expands to its whole cluster, so the two GetObjectTagging steps and the five inventory steps are covered.
