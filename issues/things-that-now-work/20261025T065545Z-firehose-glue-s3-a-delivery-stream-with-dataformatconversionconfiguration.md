Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__s3.firehose-parquet-conversion-rejects-a-schema-mismatched-record-to-the-error-prefix
Issue-Key: firehose__glue__s3.firehose-parquet-conversion-rejects-a-schema-mismatched-record-to-the-error-prefix|run#16|AssertionError|assert delivered[:4] == b"PAR1" and delivered[-4:] == b"PAR1", (
Issue-Commit: a1aff8283ff7a4c3e09b44355afd91a57915fcd2

# firehose__glue__s3: a delivery stream with DataFormatConversionConfiguration writes raw JSON instead of Parquet, so neither conversion nor the ErrorOutputPrefix path can be observed (1 cause)

> _Note: this issue was identified against the code base at commit `a1aff8283ff7a4c3e09b44355afd91a57915fcd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Cause 1 of 1: the delivery stream's DataFormatConversionConfiguration is stored but never applied to delivered records.
AWS: an ExtendedS3 destination configured with DataFormatConversionConfiguration (SchemaConfiguration pointing at a Glue table, InputFormatConfiguration=OpenX/Hive JSON deserializer, OutputFormatConfiguration=ParquetSerDe) converts each buffered record to Parquet against the Glue table's column schema before writing it to S3. The Smithy model describes the configuration members but not the conversion behaviour, so the behaviour here is recorded behavior only: the recording shows the delivered object being a Parquet file — the byte stream begins and ends with the Parquet magic 'PAR1' — and a record whose fields do not match the Glue schema being diverted to the stream's ErrorOutputPrefix rather than landing in the converted output.
Emulator: records delivered to the destination prefix are the raw JSON payloads the producer put, with no conversion applied. The delivered object starts with b'{\"ki' and ends with b'ebc\\n' instead of 'PAR1'...'PAR1', so it is not a Parquet file at all. Because nothing is converted, the schema-mismatch arm cannot be exercised either: there is no conversion step to fail and therefore no ErrorOutputPrefix record.
Why: Parquet conversion is the whole reason this destination shape exists — it is what makes the delivered data queryable by a columnar reader against the Glue table. A client that lands data through it and then reads the destination prefix with a Parquet reader (Athena, Spark, pandas) fails to parse the object; a client that relies on malformed records being quarantined under ErrorOutputPrefix instead finds them mixed into the main output, silently corrupting the dataset.
Fix: the Firehose delivery/flush path for an ExtendedS3 destination — the stage that must apply DataFormatConversionConfiguration (resolve SchemaConfiguration against the Glue catalog, deserialize with the configured input deserializer, serialize with ParquetSerDe) before writing the object, and route records that fail conversion to ErrorOutputPrefix.
Traces: firehose__glue__s3.firehose-parquet-conversion-rejects-a-schema-mismatched-record-to-the-error-prefix.

Related but not covering: the open issue 20261024T005347Z-firehose-glue-s3-firehose-parquet-conversion-against-a-glue-schema-produces.md describes the case where NOTHING is written at all. Here objects ARE now written to the destination prefix — they are simply unconverted raw JSON — so the defect that remains is the missing conversion stage, not the missing delivery.

## Evidence
Expected: the object delivered under the destination prefix is a Parquet file — its first four bytes and last four bytes are b'PAR1' — and the record that does not match the Glue table's columns appears under the stream's ErrorOutputPrefix instead of in the converted output. Actual: the delivered object is the raw JSON payload: AssertionError 'the delivered object is not a Parquet file: head b\'{\"ki\', tail b\'ebc\\n\'' at check_firehose__glue__s3_firehose_parquet_conversion_rejects_a_schema_mismatched_record_to_the_error_prefix.py line 136. No ErrorOutputPrefix object is produced for the mismatched record.

## Steps to Reproduce
Create a Glue database and table with a typed column schema; create an S3 bucket; CreateDeliveryStream with an ExtendedS3DestinationConfiguration whose DataFormatConversionConfiguration has Enabled=true, SchemaConfiguration naming that Glue database/table, InputFormatConfiguration={'Deserializer':{'OpenXJsonSerDe':{}}} and OutputFormatConfiguration={'Serializer':{'ParquetSerDe':{}}}, plus an ErrorOutputPrefix; PutRecord one schema-conforming JSON record and one record whose fields contradict the schema; wait for the buffer to flush and GetObject the delivered key. The emulator returns raw JSON bytes; AWS returns a 'PAR1'-framed Parquet object and quarantines the mismatched record under ErrorOutputPrefix.

Replay: firehose__glue__s3.firehose-parquet-conversion-rejects-a-schema-mismatched-record-to-the-error-prefix.
