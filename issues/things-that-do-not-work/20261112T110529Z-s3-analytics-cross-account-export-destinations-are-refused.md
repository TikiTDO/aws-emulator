Issue-Source: trace-sweep-divergence
Issue-Trace: s3-aaxe-configuration-id-mistakes
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-aaxe-configuration-id-mistakes s3-aaxe-data-export-schema-mistakes s3-aaxe-destination-arn-mistakes s3-aaxe-destination-values-stored-unvalidated s3-aaxe-filter-shapes-beside-the-foreign-export s3-aaxe-read-mistakes-on-the-per-bucket-probe s3-aaxe-reference-walk-partitions-foreign-own-and-unset-exports s3-aaxe-reput-replaces-and-deleting-the-foreign-arm
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketAnalyticsConfiguration|2xx->4xx|status
Issue-Key: s3|ListBucketAnalyticsConfigurations|2xx->2xx|$.ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration
Issue-Key: s3|GetBucketAnalyticsConfiguration|2xx->2xx|$.AnalyticsConfiguration.StorageClassAnalysis.DataExport.Destination.S3BucketDestination.BucketAccountId
Issue-Key: s3|GetBucketAnalyticsConfiguration|2xx->4xx|$.AnalyticsConfiguration
Issue-Key: s3|GetBucketAnalyticsConfiguration|2xx->4xx|status
Issue-Key: s3|GetBucketAnalyticsConfiguration|2xx->2xx|$.ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration
Issue-Commit: 244b37dd3da0cfb1a8c1cd628adb7f39252d7b92

# s3 analytics cross-account export destinations are refused and never recorded (5 causes)

> _Note: this issue was identified against the code base at commit `244b37dd3da0cfb1a8c1cd628adb7f39252d7b92`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketAnalyticsConfiguration

## Description
Storage-class-analysis configurations whose export destination names an owning account (S3BucketDestination.BucketAccountId) cannot be written to this emulator, and every read of such a configuration therefore diverges too. 5 net-new causes, all on the s3 analytics configuration plane.

Cause 1 of 5: PutBucketAnalyticsConfiguration refuses a stated BucketAccountId that is not a 12-digit account id.
AWS: Smithy: AnalyticsS3BucketDestination$BucketAccountId targets com.amazonaws.s3#AccountId, which is a plain string with NO @length, @pattern or other constraint, and the member documents only "The account ID that owns the destination S3 bucket. If no account ID is provided, the owner is not validated before exporting data" plus a recommendation to set it. Nothing in the model or the member's documentation makes the value's FORM part of the request contract, and the recording confirms it: the put is answered 204 No Content with an empty body and the value is stored verbatim.
Emulator: the put is answered 400 with the fixed envelope Error.Code 'MalformedXML' and Error.Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text, no variable part), because the handler requires the stated account id to satisfy an internal 12-digit account-id test before the configuration is written. 14 recorded steps across 8 fixtures are refused this way, including every setup step that seeds a cross-account export.
Why: a client that configures storage class analysis exporting to a bucket in another account — the documented cross-account export case — gets a schema refusal for a document AWS accepts, so the configuration is never created and the whole subsequent workflow (read, re-put, delete) operates on a bucket that holds nothing.
Fix: the PutBucketAnalyticsConfiguration handler's destination validation — drop the account-id form gate for BucketAccountId and store the stated value as the payload carries it, the way the handler already stores Bucket and Prefix.
Traces: s3-aaxe-configuration-id-mistakes#task#8 (representative); same cluster also s3-aaxe-data-export-schema-mistakes#task#6, s3-aaxe-destination-arn-mistakes#task#9, s3-aaxe-destination-values-stored-unvalidated#task#2..#5, s3-aaxe-filter-shapes-beside-the-foreign-export#task#2..#5, s3-aaxe-read-mistakes-on-the-per-bucket-probe#setup#2, s3-aaxe-reference-walk-partitions-foreign-own-and-unset-exports#setup#3, s3-aaxe-reput-replaces-and-deleting-the-foreign-arm#setup#2. Code: services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_analytics_configuration.rs:305-325 (the `crate::state::is_account_id` gate that returns `malformed_analytics_configuration()`).

Cause 2 of 5: ListBucketAnalyticsConfigurations reports no configuration at all for a bucket that has one.
AWS: recorded behavior only — ListBucketAnalyticsConfigurations answers 200 with one ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration entry per recorded configuration, each carrying its Id and StorageClassAnalysis.DataExport (OutputSchemaVersion, Destination.S3BucketDestination with Bucket, Format and BucketAccountId).
Emulator: the same call answers 200 with the AnalyticsConfiguration element absent entirely, because the write that would have recorded it was refused by cause 1.
Why: a client enumerating a bucket's analytics configurations sees an empty listing and concludes the bucket is unconfigured, so it will re-create configurations in a loop or report the wrong state of the bucket.
Fix: follows from cause 1 (the analytics configuration state is never populated); verify the ListBucketAnalyticsConfigurations projection emits one entry per stored configuration once the put is accepted.
Traces: s3-aaxe-configuration-id-mistakes#task#9 (representative); same cluster also s3-aaxe-destination-values-stored-unvalidated#task#8, s3-aaxe-filter-shapes-beside-the-foreign-export#task#9, s3-aaxe-read-mistakes-on-the-per-bucket-probe#task#7.

Cause 3 of 5: GetBucketAnalyticsConfiguration serves a configuration whose destination has lost its BucketAccountId.
AWS: Smithy: AnalyticsS3BucketDestination$BucketAccountId; the read reports the configuration exactly as the last accepted put recorded it, so a destination written with an owning account id reports that member back.
Emulator: the read answers 200 and the configuration is present, but $.AnalyticsConfiguration.StorageClassAnalysis.DataExport.Destination.S3BucketDestination.BucketAccountId is missing. Hypothesis: the put that carried the member was refused by cause 1, so the bucket still holds the earlier configuration, which never carried it; the read handler itself clones the stored configuration faithfully.
Why: a client reading back its own configuration cannot tell that destination-owner validation is not configured, and a re-put built from the read silently drops the owner guard the member exists to provide.
Fix: follows from cause 1 — the PutBucketAnalyticsConfiguration handler must record BucketAccountId so the read has it to report.
Traces: s3-aaxe-data-export-schema-mistakes#task#7.

Cause 4 of 5: GetBucketAnalyticsConfiguration answers NoSuchConfiguration for a configuration that was set.
AWS: recorded behavior only — after the configuration is set, the read answers 200 with the AnalyticsConfiguration (Id, StorageClassAnalysis.DataExport with the S3BucketDestination's Bucket, Format, BucketAccountId and Prefix).
Emulator: the read answers 404 with the not-there refusal and no AnalyticsConfiguration, because the put was refused by cause 1 and nothing was recorded.
Why: the configuration appears never to have existed, so a client cannot read, re-put or delete it; a workflow that waits for the configuration to become readable never converges.
Fix: follows from cause 1; no change is needed in the GetBucketAnalyticsConfiguration handler, which reports whatever the store holds.
Traces: s3-aaxe-destination-arn-mistakes#task#10 (representative); same cluster also s3-aaxe-destination-values-stored-unvalidated#task#7, s3-aaxe-reference-walk-partitions-foreign-own-and-unset-exports#task#10, s3-aaxe-reput-replaces-and-deleting-the-foreign-arm#task#4.

Cause 5 of 5: the per-bucket analytics read reports no configuration on the single-configuration probe.
AWS: recorded behavior only — the read answers 200 and reports the bucket's one configuration (Id, StorageClassAnalysis.DataExport with Destination.S3BucketDestination carrying Bucket, Format and BucketAccountId).
Emulator: answers 200 with the AnalyticsConfiguration element absent, again because the seeding put was refused by cause 1.
Why: same client-visible effect as cause 2 on the single-configuration path: the bucket reads as unconfigured immediately after it was configured.
Fix: follows from cause 1; confirm the read/listing projection reports the stored configuration once the write lands.
Traces: s3-aaxe-read-mistakes-on-the-per-bucket-probe#task#6.

## Evidence
Cause 1 (s3-aaxe-configuration-id-mistakes#task#8, 14 affected steps): expected status 204 with empty body; actual 400 with Error.Code 'MalformedXML', Error.Message 'The XML you provided was not well-formed or did not validate against our published schema'. The refused documents' destinations all state BucketAccountId (e.g. a non-numeric owner token) which the handler tests with `is_account_id`.
Cause 2 (s3-aaxe-configuration-id-mistakes#task#9, 4 affected steps): $.ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration: missing — expected an entry {Id, StorageClassAnalysis.DataExport.{OutputSchemaVersion: V_1, Destination.S3BucketDestination.{Bucket: an s3 bucket ARN, Format: CSV, BucketAccountId: the stated owner}}}, got None.
Cause 3 (s3-aaxe-data-export-schema-mistakes#task#7): $.AnalyticsConfiguration.StorageClassAnalysis.DataExport.Destination.S3BucketDestination.BucketAccountId: missing (expected the stated owner account token, got None); status 200 both sides.
Cause 4 (s3-aaxe-destination-arn-mistakes#task#10, 4 affected steps): status expected 200, got 404; $.AnalyticsConfiguration missing (expected the configuration with Destination.S3BucketDestination.{Bucket, Format: CSV, BucketAccountId, Prefix}).
Cause 5 (s3-aaxe-read-mistakes-on-the-per-bucket-probe#task#6): $.ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration missing (expected the probe's single configuration with Destination.S3BucketDestination.{Bucket, Format: CSV, BucketAccountId}), status 200 both sides.
Model evidence: smithy_query shape AnalyticsS3BucketDestination — BucketAccountId -> com.amazonaws.s3#AccountId, a string with no constraint traits; doc states only that an unprovided account id means the owner is not validated.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus) and look at the s3 analytics clusters.
2. Cause 1: on an existing bucket, PUT ?analytics&id=<id> with an AnalyticsConfiguration whose StorageClassAnalysis.DataExport.Destination.S3BucketDestination carries Bucket=<bucket ARN>, Format=CSV and a BucketAccountId that is not twelve digits. The emulator answers 400 MalformedXML; AWS answers 204 with an empty body (fixture s3-aaxe-configuration-id-mistakes, step task#8).
3. Cause 2/5: GET ?analytics on that bucket — the emulator's ListBucketAnalyticsConfigurationsResult has no AnalyticsConfiguration element (s3-aaxe-configuration-id-mistakes#task#9, s3-aaxe-read-mistakes-on-the-per-bucket-probe#task#6).
4. Cause 4: GET ?analytics&id=<id> — the emulator answers 404 NoSuchConfiguration instead of 200 with the configuration (s3-aaxe-destination-arn-mistakes#task#10).
5. Cause 3: where a configuration without an owning account was seeded first and a second put adds BucketAccountId, GET ?analytics&id=<id> returns 200 but the S3BucketDestination has no BucketAccountId (s3-aaxe-data-export-schema-mistakes#task#7).
