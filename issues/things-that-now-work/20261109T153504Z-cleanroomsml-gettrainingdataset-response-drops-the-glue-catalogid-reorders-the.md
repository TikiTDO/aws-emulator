Issue-Source: trace-sweep-divergence
Issue-Trace: cleanroomsml-training-dataset-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanroomsml-training-dataset-lifecycle
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml|GetTrainingDataset|2xx->2xx|$.createTime
Issue-Key: cleanroomsml|GetTrainingDataset|2xx->2xx|$.trainingData[*].inputConfig.dataSource.glueDataSource.catalogId
Issue-Key: cleanroomsml|GetTrainingDataset|2xx->2xx|$.trainingData[*].inputConfig.schema[*].columnName
Issue-Key: cleanroomsml|GetTrainingDataset|2xx->2xx|$.trainingData[*].inputConfig.schema[*].columnTypes[*]
Issue-Key: cleanroomsml|GetTrainingDataset|2xx->2xx|$.updateTime
Issue-Commit: c204ffd13545bba78014bc80d4ce821b8ce22cdd

# GetTrainingDataset response drops the Glue catalogId, reorders the schema column list, and serializes timestamps as epoch numbers

> _Note: this issue was identified against the code base at commit `c204ffd13545bba78014bc80d4ce821b8ce22cdd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrainingDataset

## Description
On the happy path, GetTrainingDataset returns HTTP 200 but its body does not round-trip what CreateTrainingDataset was given: the Glue data source loses a member, the schema column list comes back in a different order, and the two required timestamps are serialized in the wrong wire form. This issue carries ONE cause with three symptoms in the same read-back projection, all in the GetTrainingDataset output serializer / the training-dataset state record.

Cause 1 of 1 - GetTrainingDataset does not faithfully project the stored training dataset.
AWS: Smithy: GetTrainingDataset -> GET /training-dataset/{trainingDatasetArn}, output GetTrainingDatasetResponse with required members createTime, updateTime (both target Timestamp with @timestampFormat date-time), trainingDatasetArn, name, trainingData (DatasetList), status and roleArn. trainingData[].inputConfig is DatasetInputConfig, whose required `schema` is DatasetSchemaList (@length 1..100) of ColumnSchema {columnName required, columnTypes required @length 1..1} and whose required `dataSource` is DataSource { glueDataSource required }; GlueDataSource members are tableName (required), databaseName (required) and catalogId (AccountId, @length 12..12, pattern ^[0-9]{12}$, doc "The Glue catalog that contains the training data."). AWS therefore (a) echoes catalogId when the create request supplied one, (b) preserves the schema list in the order it was submitted - a Smithy list is ordered and is not a set, so element i of the response schema is element i of the request schema - and (c) serializes createTime/updateTime as ISO-8601 date-time strings because of @timestampFormat date-time, not as the restJson1 default epoch-seconds number.
Emulator: for a dataset created with a three-column schema and a glueDataSource carrying catalogId, the read-back returns 200 but: `trainingData[0].inputConfig.dataSource.glueDataSource.catalogId` is absent; the three ColumnSchema entries appear rotated relative to the submitted order (the submitted 2nd, 3rd and 1st columns are returned as the 1st, 2nd and 3rd), with each entry's columnTypes rotated with it; and createTime/updateTime are emitted as JSON numbers (epoch seconds with a fractional part) rather than date-time strings.
Why: a client cannot trust the read-back. The missing catalogId silently drops the account that owns the Glue catalog, so a caller that re-submits the retrieved inputConfig (a common update/clone flow) targets the default catalog instead of the one originally configured. The reordered schema breaks every positional consumer - column-to-type alignment, UI column tables, and any assertion that the dataset it created is the dataset it reads back - and because the rotation moves columnTypes with the name the mapping is self-consistent but wrong relative to the request. The numeric timestamps are not merely a formatting nit: an SDK generated from this model parses these members as @timestampFormat date-time strings, so deserialization of a required member fails or yields a wrong instant.
Fix: the GetTrainingDataset output serializer and the training-dataset state record it reads - preserve glueDataSource.catalogId when storing/projecting the create input, preserve the submitted schema list order rather than re-deriving or re-sorting it, and emit createTime/updateTime with the date-time timestamp format.
Traces: cleanroomsml-training-dataset-lifecycle#task#8. The concrete column names, ARNs, account id and timestamp values in that recording are per-run/per-request data filling the shape and are not themselves requirements; the requirements are catalogId presence, order preservation and the date-time timestamp form.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture cleanroomsml-training-dataset-lifecycle, step task#8, operation GetTrainingDataset, status 200 expected and 200 got. Field-level divergences reported by the replay:

  $.trainingData[0].inputConfig.dataSource.glueDataSource.catalogId: missing (expected '000000000000', got None)
  $.trainingData[0].inputConfig.schema[0].columnName: expected 'item_id', got 'user_id'
  $.trainingData[0].inputConfig.schema[0].columnTypes[0]: expected 'ITEM_ID', got 'USER_ID'
  $.trainingData[0].inputConfig.schema[1].columnName: expected 'ts', got 'item_id'
  $.trainingData[0].inputConfig.schema[1].columnTypes[0]: expected 'TIMESTAMP', got 'ITEM_ID'
  $.trainingData[0].inputConfig.schema[2].columnName: expected 'user_id', got 'ts'
  $.trainingData[0].inputConfig.schema[2].columnTypes[0]: expected 'USER_ID', got 'TIMESTAMP'
  $.createTime: expected a date-time-shaped value ('<shaped-not-asserted>'), got the number 1794238305.7858725
  $.updateTime: expected a date-time-shaped value ('<shaped-not-asserted>'), got the number 1794238305.7858725

The schema rows show a rotation, not random noise: the recorded order is [item_id, ts, user_id] and the emulator returns [user_id, item_id, ts]. The catalogId '000000000000' and the epoch value 1794238305.78... are this run's values; the divergences are the absent member, the changed order, and the number-vs-date-time-string form.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus) and look at fixture cleanroomsml-training-dataset-lifecycle, step task#8.
3. Equivalently by hand: POST /training-dataset with a trainingData[0].inputConfig whose dataSource.glueDataSource sets tableName, databaseName AND catalogId, and whose schema lists three ColumnSchema entries in a known order.
4. GET /training-dataset/{trainingDatasetArn} for the returned ARN.
5. Observe in the 200 response: glueDataSource.catalogId is absent; the three schema entries are returned rotated relative to the order submitted in step 3; and createTime/updateTime are JSON numbers instead of ISO-8601 date-time strings as @timestampFormat date-time requires.
