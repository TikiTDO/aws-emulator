Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__firehose__glue__s3.firehose-interval-ceiling-900-echoed-and-size-hint-flushes-through-athena
Issue-Key: athena__firehose__glue__s3.firehose-interval-ceiling-900-echoed-and-size-hint-flushes-through-athena|run#36|AssertionError|assert got == lines_per_key, (
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# athena__firehose__glue__s3: Athena's "$path" per-object row count returns no row for a Firehose-delivered object, so rows are never attributed to the object that holds them

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResults

## Description
Cause 1 of 1: an Athena `SELECT "$path", count(*) FROM "<db>"."<table>" GROUP BY 1` over a Glue table whose StorageDescriptor.Location is a Firehose destination prefix settles SUCCEEDED but its result set contains no row for the object Firehose actually delivered, so the per-object row count is absent. Failing check: athena__firehose__glue__s3.firehose-interval-ceiling-900-echoed-and-size-hint-flushes-through-athena.

AWS: recorded behavior only — the `$path` hidden column is an Athena/Trino engine feature and is not described by the Athena Smithy model (Athena's model describes only the StartQueryExecution/GetQueryResults envelope; the projected column list and values are engine behavior). The recording shows that on a Hive/JSON EXTERNAL_TABLE whose location is the Firehose prefix, Athena scans every object Firehose delivered under that prefix, and the grouped query returns one row per scanned source object: the first column is that object's full `s3://<bucket>/<key>` URI (the same URI ListObjectsV2 reports for it, bucket plus key, no query string or variant spelling) and the second column is the number of records Athena read out of it — for newline-delimited JSON, one row per non-empty line. The requirement is the general one: for every object present under the table location, the result set carries a row whose `$path` is that object's s3:// URI and whose count equals the number of lines in it; object names, counts and the 500-line value in the recording are per-run values that fill that template, not part of it.

Emulator: the query reaches SUCCEEDED (no error code, no refusal) and GetQueryResults returns a result set that does not contain the delivered object at all. The check looks the delivered object's URI up in the returned rows and gets a missing value: S3 lists exactly one object under the stream prefix holding 500 JSON lines, and the emulator's answer for that path is `None` — i.e. no row in the result set has that `$path`. Reported as: "Athena must count every line Firehose delivered in each listed object; S3 holds {'s3://<bucket>/<stream-prefix>/<YYYY>/<MM>/<DD>/<HH>/<object>': 500}, Athena counted {'s3://<bucket>/<stream-prefix>/<YYYY>/<MM>/<DD>/<HH>/<object>': None}". Everything earlier in the same check passed, so this is not the Firehose side: CreateDeliveryStream correctly refused IntervalInSeconds=901 with ValidationException, the 900 s stream reached ACTIVE, DescribeDeliveryStream echoed BufferingHints {SizeInMBs: 1, IntervalInSeconds: 900}, and the size-triggered object was delivered to S3 with the expected ids.

Why: a client relying on AWS's behavior cannot attribute rows to the object they came from. The `$path` column is the standard way to audit a Firehose/Glue/Athena ingest pipeline (reconciling delivered objects against queried rows, finding the object behind a bad row, de-duplicating a re-delivered object). Against the emulator every such lookup yields nothing, and because the query still reports SUCCEEDED the caller sees an empty/zero answer rather than an error, so the pipeline silently appears to have ingested no data.

Fix: the Athena query-execution data plane for Glue-catalog tables — the table-scan planner that enumerates the objects under the Glue table's StorageDescriptor.Location and the projection of the `$path` hidden column (it must emit the scanned object's s3:// URI, and the scan must cover objects delivered under the location's sub-prefixes). This is distinct from the already-open issue 20261104T082656Z-athena-firehose-glue-s3-athena-cannot-read-the.md, which covers a SELECT failing with a SQLite 'no such table'; here the query succeeds and the defect is in which rows/paths come back, so that issue does not cover this check.

Traces: athena__firehose__glue__s3.firehose-interval-ceiling-900-echoed-and-size-hint-flushes-through-athena.

## Evidence
Check athena__firehose__glue__s3.firehose-interval-ceiling-900-echoed-and-size-hint-flushes-through-athena, verdict: mismatch.

Expected (AWS, recorded): for the one object S3 lists under the delivery-stream prefix, the result of `SELECT "$path", count(*) FROM "<db>"."<table>" GROUP BY 1` contains a row whose first column is `s3://<bucket>/<key>` of that object and whose second column equals the number of JSON lines in it (500 in the recording).

Actual (emulator): query state SUCCEEDED, but no result row carries that path — the per-path lookup yields None:
AssertionError: Athena must count every line Firehose delivered in each listed object; S3 holds {'s3://<bucket>/<stream>/<YYYY>/<MM>/<DD>/<HH>/<object>': 500}, Athena counted {'s3://<bucket>/<stream>/<YYYY>/<MM>/<DD>/<HH>/<object>': None}

Preceding assertions in the same check all passed, establishing that the data is in S3 and readable: ids in the delivered object were a subset of the ids put, DescribeDeliveryStream echoed BufferingHints {SizeInMBs: 1, IntervalInSeconds: 900}, and the stream was ACTIVE.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role trusted by firehose.amazonaws.com with s3:PutObject/GetObject/ListBucket on it.
2. Create a Glue database and an EXTERNAL_TABLE with Parameters {classification: json, EXTERNAL: TRUE}, a single int column `id`, StorageDescriptor.Location = s3://<bucket>/<stream-prefix>/, TextInputFormat/HiveIgnoreKeyTextOutputFormat and org.openx.data.jsonserde.JsonSerDe.
3. CreateDeliveryStream (DirectPut, ExtendedS3DestinationConfiguration) with Prefix=<stream-prefix>/, BufferingHints {SizeInMBs: 1, IntervalInSeconds: 900}; wait for ACTIVE.
4. PutRecordBatch enough newline-terminated JSON records ({"id": n, "pad": "x"*8000}) to cross the 1 MiB size hint, then poll ListObjectsV2 under <stream-prefix>/ until an object appears; GetObject it and count its non-empty lines.
5. Through Athena (workgroup with an S3 OutputLocation) run `SELECT "$path", count(*) FROM "<db>"."<table>" GROUP BY 1`, wait for SUCCEEDED and read GetQueryResults.
6. Observe: no returned row's first column equals the s3:// URI of the object listed in step 4, so its line count cannot be found in the result set (expected one row per delivered object with its line count).
