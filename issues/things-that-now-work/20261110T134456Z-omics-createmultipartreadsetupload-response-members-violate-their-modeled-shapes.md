Issue-Source: trace-sweep-divergence
Issue-Trace: omics-multipart-read-set-upload-reads-back-uploaded-bytes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: omics-multipart-read-set-upload-reads-back-uploaded-bytes
Issue-Shape-Id: com.amazonaws.omics#Omics
Issue-Key: omics|CreateMultipartReadSetUpload|2xx->2xx|$.creationTime
Issue-Key: omics|CreateMultipartReadSetUpload|2xx->2xx|$.referenceArn
Issue-Commit: 3a275ae8ff7628ae729e6996643a6fe8b123024e

# omics: CreateMultipartReadSetUpload response members violate their modeled shapes — epoch-number creationTime and empty referenceArn

> _Note: this issue was identified against the code base at commit `3a275ae8ff7628ae729e6996643a6fe8b123024e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMultipartReadSetUpload

## Description
CreateMultipartReadSetUpload answers HTTP 200 (the modeled success status) but two members of the JSON body do not have the shape the model binds for them. The status and the operation's semantics are right; the response body is not wire-accurate, so a modeled SDK client cannot read it. Both causes are in the same response-building path for this operation and both must be closed before submitting.

=== Cause 1 of 2: creationTime is serialized as an epoch number instead of a date-time string (fixture omics-multipart-read-set-upload-reads-back-uploaded-bytes, step task#2) ===
AWS: Smithy: CreateMultipartReadSetUploadResponse.creationTime targets Timestamp, @required, timestampFormat = date-time. For the service's REST-JSON protocol that means the member is serialized as an RFC3339/ISO-8601 date-time STRING (e.g. "2024-05-01T12:34:56.789Z"), not as a number. The recorded response for this step carries a date-time-shaped value for $.creationTime (the recording asserts its shape, not a literal instant, because the instant is a per-run value).
Emulator: the member is emitted as a JSON number of epoch seconds with a fractional part — the replay reports "$.creationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794318197.372)". The handler builds the right kind of value (crate::state::timestamp -> aws_smithy_types::DateTime, services/service-com-amazonaws-omics-omics/src/state.rs:1317 and src/ops/create_multipart_read_set_upload.rs:358), so the loss happens when that DateTime is written to the body: the member's date-time format is not honored and the smithy default epoch-seconds encoding is used.
Why: a generated AWS SDK client deserializes this member according to the model, i.e. as a date-time string; a number there is a deserialization failure or a silently wrong instant, so a caller that creates a multipart read set upload cannot read creationTime at all even though the call succeeded. Any client that logs, sorts or compares upload creation times against GetSequenceStore/ListMultipartReadSetUploads output sees an inconsistent representation.
Fix: the operation's output serialization — the date-time timestampFormat binding for creationTime in this operation's generated output schema / the service's REST-JSON timestamp serializer path used by CreateMultipartReadSetUpload (services/service-com-amazonaws-omics-omics/src/ops/create_multipart_read_set_upload.rs, the OUTPUT_SCHEMA it names and the `reported` helper that fills creationTime). Every other omics response member that targets Timestamp with date-time is owed the same encoding.
Traces: omics-multipart-read-set-upload-reads-back-uploaded-bytes#task#2.

=== Cause 2 of 2: referenceArn is reported as an empty string when the request named no reference, instead of being omitted (fixture omics-multipart-read-set-upload-reads-back-uploaded-bytes, step task#2) ===
AWS: Smithy: CreateMultipartReadSetUploadResponse.referenceArn targets ReferenceArn, @length min 1 max 127, @pattern ^arn:.+$ (the request's own referenceArn member is OPTIONAL, same constraints). An empty string satisfies neither the length floor nor the pattern, so AWS cannot and does not emit "" for this member. Recorded behavior: for an upload initiated WITHOUT referenceArn, the recorded response has NO referenceArn key at all — the replay's expectation for $.referenceArn is None (absent).
Emulator: the member is always present and carries the empty string for an upload that named no reference — the replay reports "$.referenceArn: type_mismatch (expected None, got '')". The handler stores the caller's value as-is (Option) and then flattens it with `unwrap_or_default()` when building the response (services/service-com-amazonaws-omics-omics/src/ops/create_multipart_read_set_upload.rs:354, documented in the `reported` doc comment at lines 339-345).
Why: a client that branches on whether a reference genome is attached to the upload (aligned source types BAM/CRAM need one, FASTQ does not) sees a reference ARN that is present-but-empty rather than absent, so an `if referenceArn is not None` / `.referenceArn().isPresent()` test takes the wrong branch, and any code that parses the value as an ARN gets an unparseable empty string. It also means the response advertises a constraint-violating value for a @required member.
Fix: the operation's response construction — the `reported` helper in services/service-com-amazonaws-omics-omics/src/ops/create_multipart_read_set_upload.rs must omit referenceArn when the stored upload holds none rather than substituting the type default, and the output representation must allow its absence; the same rule applies wherever else this crate reports a stored upload's referenceArn (the listing and completion paths that read crate::state::MultipartReadSetUpload).
Traces: omics-multipart-read-set-upload-reads-back-uploaded-bytes#task#2.

Related filed issues: the open omics issues (the StartRun/CreateWorkflowVersion/CloudFormation bundle 20261029T184954Z-omics-4-net-new-root-causes-startrun-refuses, and the CreateShare / DeleteSequenceStore / TagResource / UpdateRunGroup scaffold stubs) concern other operations and do NOT cover either cause above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Fixture: omics-multipart-read-set-upload-reads-back-uploaded-bytes, step task#2, operation CreateMultipartReadSetUpload, verdict mismatch, expected status 200 / actual status 200 (status is correct; the body is not).

Detail reported by the replay:
  $.creationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794318197.372)
  $.referenceArn: type_mismatch (expected None, got '')

Expected (AWS / recorded):
  - $.creationTime: a date-time STRING (Smithy Timestamp, timestampFormat date-time, @required). Shape asserted, not the literal instant, since the instant is per-run.
  - $.referenceArn: ABSENT from the body, because the CreateMultipartReadSetUpload request named no referenceArn. ReferenceArn is @length 1..127 @pattern ^arn:.+$, so "" is not an emittable value.

Actual (emulator):
  - $.creationTime: 1794318197.372 — a JSON number (epoch seconds with milliseconds), the smithy default epoch-seconds encoding rather than the bound date-time format.
  - $.referenceArn: "" — present and empty, produced by `upload.reference_arn.clone().unwrap_or_default()` in `reported` (src/ops/create_multipart_read_set_upload.rs:354).

Source support: handler builds the timestamp correctly as aws_smithy_types::DateTime (src/ops/create_multipart_read_set_upload.rs:358 via crate::state::timestamp, src/state.rs:1317 — whose own doc comment says "as the `date-time` timestamp the model binds"), so the divergence is introduced in the serialization of that value, not in its computation.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay fixture omics-multipart-read-set-upload-reads-back-uploaded-bytes (step task#2).
2. Equivalently, by hand against the omics endpoint: CreateSequenceStore to get a sequence store id, then POST /sequencestore/{sequenceStoreId}/upload with a body naming only the required members — sourceFileType=FASTQ, subjectId, sampleId, name — and NO referenceArn.
3. Inspect the 200 response body:
   - $.creationTime comes back as a JSON number (e.g. 1794318197.372) instead of an RFC3339 date-time string such as "2026-11-10T12:23:17.372Z".
   - $.referenceArn is present as "" instead of being absent from the body.
4. Repeat with referenceArn supplied in the request: the member should echo that ARN; the creationTime encoding is wrong in both cases.
