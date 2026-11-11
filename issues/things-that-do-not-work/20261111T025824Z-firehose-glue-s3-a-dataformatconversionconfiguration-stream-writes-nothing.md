Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error
Issue-Key: firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error|_poll_until#15|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# firehose__glue__s3: a DataFormatConversionConfiguration stream writes nothing at all — no Parquet object under Prefix and no format-conversion-failed object under ErrorOutputPrefix

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
Cause 1 of 1: an ExtendedS3Destination delivery stream with DataFormatConversionConfiguration (Enabled, OpenXJsonSerDe in / ParquetSerDe out, SchemaConfiguration pointing at a Glue table) accepts records with FailedPutCount 0 but never writes any object — neither the converted object under `Prefix` nor the failed-record object under `ErrorOutputPrefix` — so the MalformedData-vs-ParseError error-code differential is unobservable. Failing check: firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error.

AWS: Smithy (com.amazonaws.firehose): ExtendedS3DestinationConfiguration.DataFormatConversionConfiguration — "The serializer, deserializer, and schema for converting data from the JSON format to the Parquet or ORC format before writing it to Amazon S3"; DataFormatConversionConfiguration — "Specifies that you want Firehose to convert data from the JSON format to the Parquet or ORC format before writing it to Amazon S3. Firehose uses the serializer and deserializer that you specify, in addition to the column information from the Amazon Web Services Glue table", with Enabled defaulting to true and SchemaConfiguration/InputFormatConfiguration/OutputFormatConfiguration required when enabled; OutputFormatConfiguration.Serializer = ParquetSerDe (Compression UNCOMPRESSED here); ExtendedS3DestinationConfiguration.ErrorOutputPrefix — "A prefix that Firehose evaluates and adds to failed records before writing them to S3". Recorded behavior for the rest: with BufferingHints {SizeInMBs: 64, IntervalInSeconds: 60}, the interval expires and Firehose writes exactly one UNCOMPRESSED Parquet object under `Prefix` holding only the records that converted, while every record that could not be converted is diverted to `ErrorOutputPrefix` with `!{firehose:error-output-type}` resolved to the conversion-failure type; all failed records of one flush window share ONE newline-delimited object whose final key segment carries no file extension, one JSON line per record with `rawData` = base64 of the ORIGINAL pre-conversion bytes and `lastErrorCode` = `DataFormatConversion.MalformedData` for a record whose value violates the Glue column type (here a string where a nested array<struct<sku:string,qty:int>> declares int) and `DataFormatConversion.ParseError` for a record that is not parseable JSON at all. Those two codes are fixed text; the keys, markers and timestamps in any one recording are per-run values.

Emulator: PutRecordBatch answers 200 with FailedPutCount 0 for all three records, and then nothing is ever written. ListObjectsV2 under the stream's `Prefix` (`pq/`) returns an empty list on every poll for 420 s ("a converted Parquet object lands under pq/ not met within 420s. Last result: []"), so the error-prefix object is never reached either. No status code or error code diverges — the divergence is the complete absence of the delivered and the diverted object.

Why: a client relying on AWS's behavior loses both halves of the contract. The conforming record never becomes queryable Parquet (so a Glue/Athena table over the prefix stays empty), and an unconvertible record produces no error artifact, so there is no `rawData` to replay and no `lastErrorCode` to distinguish a schema-type mismatch from unparseable JSON. Because PutRecordBatch reports FailedPutCount 0, the producer believes every record was durably accepted while the data silently disappears.

Fix: the Firehose delivery pipeline for ExtendedS3Destination when DataFormatConversionConfiguration is enabled — the buffer flush that converts the buffered JSON through the Glue SchemaConfiguration and writes the Parquet object under `Prefix`, and the error-output diversion that writes unconvertible records (with rawData and the DataFormatConversion.* lastErrorCode) to the resolved `ErrorOutputPrefix`. Two open issues are adjacent but do not cover this check (the suite did not mark it owned): 20261025T065545Z-firehose-glue-s3-a-delivery-stream-with-dataformatconversionconfiguration.md (raw JSON written instead of Parquet) and 20261029T175617Z-firehose-glue-s3-a-record-that-does-not.md (a non-conforming record written into the Parquet object instead of being diverted) — both presuppose an object is written, whereas here no object is written at all.

Traces: firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error.

## Evidence
Check firehose__glue__s3.format-conversion-error-code-distinguishes-malformed-data-from-parse-error, verdict: mismatch.

Setup: Glue table columns [marker string, n int, tags array<struct<sku:string,qty:int>>]; DirectPut stream with ExtendedS3DestinationConfiguration Prefix="pq/", ErrorOutputPrefix="err/!{firehose:error-output-type}/", CompressionFormat UNCOMPRESSED, BufferingHints {SizeInMBs: 64, IntervalInSeconds: 60}, DataFormatConversionConfiguration {Enabled: true, OpenXJsonSerDe, ParquetSerDe(UNCOMPRESSED), SchemaConfiguration -> that Glue table, VersionId LATEST}. Stream reached ACTIVE. PutRecordBatch of three records (one conforming, one with a string in the nested int field, one that is not JSON) returned FailedPutCount 0.

Expected (AWS): within the 60 s buffering interval one UNCOMPRESSED Parquet object appears under pq/ containing only the conforming record; the two bad records share one extensionless newline-delimited object under err/<conversion-failure-type>/, each line carrying rawData (base64 of the original bytes) and lastErrorCode = DataFormatConversion.MalformedData (type mismatch) and DataFormatConversion.ParseError (unparseable JSON) respectively.

Actual (emulator): no object is ever written under pq/ —
AssertionError: a converted Parquet object lands under pq/ not met within 420s. Last result: []
The error-prefix assertions were therefore never reached; nothing exists under err/ either.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role trusted by firehose.amazonaws.com with s3:PutObject/GetObject/ListBucket/AbortMultipartUpload on it.
2. Create a Glue database and table with columns marker:string, n:int, tags:array<struct<sku:string,qty:int>> and the Parquet serde.
3. CreateDeliveryStream (DeliveryStreamType=DirectPut) with ExtendedS3DestinationConfiguration: Prefix="pq/", ErrorOutputPrefix="err/!{firehose:error-output-type}/", CompressionFormat=UNCOMPRESSED, BufferingHints={SizeInMBs:64, IntervalInSeconds:60}, DataFormatConversionConfiguration={Enabled:true, InputFormatConfiguration:{Deserializer:{OpenXJsonSerDe:{}}}, OutputFormatConfiguration:{Serializer:{ParquetSerDe:{Compression:UNCOMPRESSED}}}, SchemaConfiguration:{RoleARN, DatabaseName, TableName, Region, VersionId:LATEST}}; wait for ACTIVE.
4. PutRecordBatch three newline-terminated records: {"marker":"<good>","n":1,"tags":[{"sku":"a","qty":1}]}, {"marker":"<mismatch>","n":2,"tags":[{"sku":"b","qty":"not-an-int"}]}, and the raw bytes `not-json-at-all <marker>`. FailedPutCount is 0.
5. Poll ListObjectsV2 with Prefix="pq/" for several minutes: it stays empty (expected: one UNCOMPRESSED Parquet object containing only the good record).
6. Poll ListObjectsV2 with Prefix="err/": also empty (expected: one extensionless newline-delimited object with two JSON lines carrying rawData and lastErrorCode DataFormatConversion.MalformedData / DataFormatConversion.ParseError).
