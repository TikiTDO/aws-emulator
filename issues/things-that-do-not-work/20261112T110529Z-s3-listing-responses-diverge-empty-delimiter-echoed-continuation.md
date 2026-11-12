Issue-Source: trace-sweep-divergence
Issue-Trace: s3-lsd-delimiter-parameter-variants
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-lsd-delimiter-parameter-variants s3-lsd-text-output-pagination-loop s3-itc-put-filter-and-id-mistakes
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Delimiter
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.NextContinuationToken
Issue-Key: s3|ListBucketIntelligentTieringConfigurations|2xx->2xx|$.ListIntelligentTieringConfigurationsResult.IntelligentTieringConfiguration
Issue-Commit: 244b37dd3da0cfb1a8c1cd628adb7f39252d7b92

# s3 listing responses diverge: empty Delimiter echoed, continuation token is reversible hex, refused tiering configuration listed

> _Note: this issue was identified against the code base at commit `244b37dd3da0cfb1a8c1cd628adb7f39252d7b92`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
Three net-new causes in the s3 listing projections: a response element AWS omits is emitted, the pagination token is not in AWS's form, and a bucket-configuration listing reports a configuration AWS never recorded.

Cause 1 of 3: ListObjectsV2 echoes a Delimiter element for a request whose delimiter parameter is present but empty.
AWS: Smithy: ListObjectsV2Output$Delimiter is an optional member of ListBucketResult; the recorded response for a listing whose `delimiter=` is empty carries NO Delimiter element at all (an empty delimiter is no delimiter — the listing rolls nothing up and reports Contents flat). Recorded behavior only for the omission rule itself.
Emulator: the response carries $.ListBucketResult.Delimiter as an empty element ('' instead of absent), because the output projection echoes the request member whenever it is present, without treating an empty value as unstated — the same handler's own `stated()` helper already applies that reading on the request side.
Why: a client that keys off the presence of Delimiter to decide whether the listing is hierarchical (or that round-trips the echoed value into its next request) sees a delimiter the listing did not apply, and XML-shape-sensitive clients see an element the AWS schema does not emit here.
Fix: the ListObjectsV2 output projection — map an empty delimiter (and the same for the other echoed scalars) to None rather than Some("").
Traces: s3-lsd-delimiter-parameter-variants#task#4. Code: services/service-com-amazonaws-s3-amazons3/src/ops/list_objects_v2.rs:569 (`delimiter: input.delimiter.clone().map(encode)`).

Cause 2 of 3: NextContinuationToken is lowercase hex of a marker plus the next key, not an AWS-shaped opaque token.
AWS: Smithy: ListObjectsV2Output$NextContinuationToken targets com.amazonaws.s3#NextToken and documents "NextContinuationToken is obfuscated and is not a real key"; the recorded token is asserted by SHAPE (base64-form opaque cursor), not by value, and the emulator's value fails that shape assertion.
Emulator: the token is hex.encode("s3v1 " + first key not returned) — a lowercase hex string that trivially decodes to the emulator's marker and the plaintext object key. It is not in AWS's token form, and it publishes a real key.
Why: a client or test that validates the token's form (or that treats it as an opaque base64 cursor, logs it, or round-trips it through a layer that expects AWS's form) diverges from AWS, and the key names of a bucket leak through a value AWS documents as obfuscated and not a real key.
Fix: the ListObjectsV2 continuation-token encoder — emit a token in AWS's form (base64 of the internal cursor) and keep the decoder symmetric with it; do not embed the key as recoverable plaintext.
Traces: s3-lsd-text-output-pagination-loop#task#5. Code: services/service-com-amazonaws-s3-amazons3/src/ops/list_objects_v2.rs:237-257 (`CONTINUATION_TOKEN_MARKER`, `encode_continuation_token`, `decode_continuation_token`) and :584.

Cause 3 of 3: ListBucketIntelligentTieringConfigurations reports a configuration whose PUT AWS refuses.
AWS: recorded behavior only — the malformed intelligent-tiering document is refused (400 MalformedXML, 'The XML you provided was not well-formed or did not validate against our published schema'), so nothing is recorded and the subsequent listing answers 200 with NO ListIntelligentTieringConfigurationsResult.IntelligentTieringConfiguration element.
Emulator: the listing answers 200 and reports the configuration (its Id, Filter.Prefix/Filter.Tag, Status and both Tiering entries), because the malformed put was accepted and stored, so the read-side listing surfaces state the service should never hold.
Why: a client that checks the bucket's configuration state after a refused write sees the refused document installed, so the refusal and the listing contradict each other and the bucket reports a tiering policy AWS would not be applying.
Fix: the PutBucketIntelligentTieringConfiguration validation must refuse the document before it is written, and the ListBucketIntelligentTieringConfigurations projection should then report nothing for the bucket. The open issue 20261102T111455Z-s3-intelligent-tiering-validation-and-absence-reads-diverge.md covers the PUT refusal and absence reads; it does not carry this listing step's replay key, so this read-side divergence is filed here.
Traces: s3-itc-put-filter-and-id-mistakes#task#8 (the refused put in the same fixture is task#2).

## Evidence
Cause 1 (s3-lsd-delimiter-parameter-variants#task#4): status 200 both sides; $.ListBucketResult.Delimiter: unexpected_present (expected None, got '').
Cause 2 (s3-lsd-text-output-pagination-loop#task#5): status 200 both sides; $.ListBucketResult.NextContinuationToken: value_mismatch (expected a shape-asserted AWS token, got '73337631206d656469612f612e6a7067', which is hex for 's3v1 media/a.jpg' — the emulator's marker plus the next object key in clear).
Cause 3 (s3-itc-put-filter-and-id-mistakes#task#8): status 200 both sides; $.ListIntelligentTieringConfigurationsResult.IntelligentTieringConfiguration: unexpected_present (expected None, got the configuration with Id, Filter {Prefix, Tag}, Status Enabled and two Tiering entries). The earlier put in the same fixture (task#2) is answered 204 instead of 400 MalformedXML.
Model evidence: smithy_query shape ListObjectsV2Output — Delimiter optional; NextContinuationToken doc: "NextContinuationToken is obfuscated and is not a real key"; shapes Delimiter and NextToken are unconstrained strings.

## Steps to Reproduce
1. Cause 1: GET /<bucket>?list-type=2&delimiter= (empty value). The emulator's ListBucketResult contains an empty <Delimiter/> element; AWS omits it (fixture s3-lsd-delimiter-parameter-variants, step task#4).
2. Cause 2: put more objects than max-keys and GET /<bucket>?list-type=2&max-keys=1. Read NextContinuationToken: the emulator returns lowercase hex that decodes to 's3v1 <next key>' rather than an AWS-form opaque token (fixture s3-lsd-text-output-pagination-loop, step task#5).
3. Cause 3: PUT /<bucket>?intelligent-tiering&id=<id> with the malformed configuration document from s3-itc-put-filter-and-id-mistakes task#2 (accepted 204 by the emulator), then GET /<bucket>?intelligent-tiering — the emulator lists the configuration AWS never recorded (step task#8).
