Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-table-replica-autoscaling
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-table-replica-autoscaling dynamodb-ttl-contributor-insights-and-replica-autoscaling-config ddbctl-export-time-window-and-token-conflict dynamodb-import-lifecycle dynamodb-catalog-describe-endpoints dynamodb-table-control-plane-lifecycle dynamodb-table-lifecycle-core dynamodb-table-tagging-lifecycle dynamodb-scan-return-consumed-capacity dynamodb-trajwf-parallel-scan-segment-contract
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DescribeTableReplicaAutoScaling|4xx->2xx|$.__type
Issue-Key: dynamodb|DescribeTableReplicaAutoScaling|4xx->2xx|$.message
Issue-Key: dynamodb|DescribeTableReplicaAutoScaling|4xx->2xx|status
Issue-Key: dynamodb|UpdateTableReplicaAutoScaling|4xx->2xx|$.__type
Issue-Key: dynamodb|UpdateTableReplicaAutoScaling|4xx->2xx|$.message
Issue-Key: dynamodb|UpdateTableReplicaAutoScaling|4xx->2xx|status
Issue-Key: dynamodb|ImportTable|2xx->2xx|$.ImportTableDescription.S3BucketSource.S3BucketOwner
Issue-Key: dynamodb|ListImports|2xx->2xx|$.ImportSummaryList[*]
Issue-Key: dynamodb|DescribeImport|2xx->2xx|$.ImportTableDescription.CloudWatchLogGroupArn
Issue-Key: dynamodb|DescribeImport|2xx->2xx|$.ImportTableDescription.ProcessedSizeBytes
Issue-Key: dynamodb|DescribeImport|2xx->2xx|$.ImportTableDescription.S3BucketSource.S3BucketOwner
Issue-Key: dynamodb|DescribeEndpoints|2xx->2xx|$.Endpoints[*].Address
Issue-Key: dynamodb|Scan|2xx->2xx|$.ConsumedCapacity.CapacityUnits
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*]
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].id.S
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].status.S
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].timestamp.S
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].workflowId.S
Issue-Related: 20261109T055139Z-dynamodb-export-surface-8-net-new-causes-export.md
Issue-Related: 20261110T232401Z-dynamodb-validation-and-not-found-refusal-messages-are.md
Issue-Commit: b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca

# dynamodb response state and shape diverge on replica autoscaling, import, export, endpoints, scan capacity and parallel scan

> _Note: this issue was identified against the code base at commit `b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTableReplicaAutoScaling

## Description
This bundle carries 10 net-new causes. The shared fault is response fidelity: the emulator serves a success where AWS refuses, or serves a 200 whose body omits modeled members / carries values it did not compute from state. Each cause must be closed individually.

Cause 1 of 10 — DescribeTableReplicaAutoScaling answers 200 for a table that is not a global table (fixture dynamodb-table-replica-autoscaling step task#3).
AWS: Smithy: DescribeTableReplicaAutoScaling errors = ResourceNotFoundException (client, HTTP 400) and InternalServerError. Recorded refusal: HTTP 400, __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException', message "Global table with name: '<tableName>' does not exist." Fixed text: 'Global table with name: ' / "' does not exist." (note the global-table wording and the trailing period); the variable part is the table name from the request.
Emulator: HTTP 200 with a TableAutoScalingDescription body and no error at all.
Why: the operation is global-table-scoped. A client probing whether replica auto scaling exists gets a success for a table that has no replicas, so it proceeds to configure/assert replica scaling that cannot exist; the ResourceNotFoundException branch of every caller is dead code under the emulator.
Fix: the DescribeTableReplicaAutoScaling handler — resolve the table as a global table in state and refuse with ResourceNotFoundException and the global-table sentence when it is not one.

Cause 2 of 10 — UpdateTableReplicaAutoScaling likewise answers 200 for a non-global table (fixture dynamodb-table-replica-autoscaling step task#4).
AWS: Smithy: UpdateTableReplicaAutoScaling errors include ResourceNotFoundException (client, HTTP 400). Same refusal as cause 1: 400 / 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' / "Global table with name: '<tableName>' does not exist."
Emulator: HTTP 200, the update is accepted.
Why: worse than cause 1 — an auto-scaling configuration is recorded against a table that cannot host it, so later reads describe settings AWS would never have stored and the caller's infrastructure code believes a replica scaling policy is in force.
Fix: the UpdateTableReplicaAutoScaling handler's table resolution, same global-table precondition as cause 1.
Note: open issue '20261110T191942Z-dynamodb-reopens-dynamodb-describetablereplicaautoscaling-the-not-found-refusal-for.md' concerns the wording of a not-found refusal that IS emitted; these two clusters emit no refusal at all and the replay reports no owner for them.

Cause 3 of 10 — an ExportTime outside the PITR window is refused with the wrong error class (ExportTableToPointInTime; fixture ddbctl-export-time-window-and-token-conflict step task#4).
AWS: Smithy: ExportTableToPointInTime errors include InvalidExportTimeException ('The specified ExportTime is outside of the point in time recovery window', client, HTTP 400) and PointInTimeRecoveryUnavailableException. Recorded refusal: 400, __type 'com.amazonaws.dynamodb.v20120810#InvalidExportTimeException', message 'Export Time is invalid' (entirely fixed text).
Emulator: 400 with __type 'com.amazonaws.dynamodb.v20120810#PointInTimeRecoveryUnavailableException' and message "Point in time recovery is not enabled for table '<tableName>'".
Why: the two classes mean opposite things — 'your timestamp is out of range, pick another' versus 'enable PITR on this table first'. A client told PITR is unavailable turns PITR on (or fails the run) instead of correcting its ExportTime, and the emulator asserts a state fact (PITR disabled) that is false.
Fix: ExportTableToPointInTime's precondition ordering/state read — check PITR enablement and the export-time window separately, and raise InvalidExportTimeException for an out-of-window ExportTime on a PITR-enabled table.

Cause 4 of 10 — ImportTable's response omits S3BucketSource.S3BucketOwner (fixture dynamodb-import-lifecycle step task#1).
AWS: Smithy: ImportTableDescription.S3BucketSource targets S3BucketSource, whose members are S3Bucket (required), S3KeyPrefix and S3BucketOwner (pattern ^[0-9]{12}$, 'the account number of the S3 bucket that is being imported from'). AWS echoes the resolved bucket owner — the requester's account when the request omitted it.
Emulator: 200 with S3BucketOwner absent.
Why: a client reading back the import's source to confirm which account's bucket it is reading from finds nothing, and round-tripping the description into a retry drops the owner.
Fix: the ImportTable handler's ImportTableDescription serializer/state — record and emit the resolved bucket owner account.

Cause 5 of 10 — ListImports summaries omit modeled members (fixture dynamodb-import-lifecycle step task#2).
AWS: Smithy: ImportSummary members include ImportArn, ImportStatus, TableArn, S3BucketSource, CloudWatchLogGroupArn ('the ARN of the Cloudwatch Log Group associated with this import task'), InputFormat, StartTime, EndTime. AWS populates CloudWatchLogGroupArn for every import task (a log-group ARN under /aws-dynamodb/imports).
Emulator: 200, but the summary carries no CloudWatchLogGroupArn.
Why: the log group is where an import's per-row errors are reported; a caller that follows the ARN to diagnose a FAILED import has nothing to follow.
Fix: import state + the ListImports serializer — assign a log-group ARN when the import is created and project it into ImportSummary.
(The same cluster also shows per-run StartTime/EndTime values differing; those are run-dependent and are not part of this cause.)

Cause 6 of 10 — DescribeImport omits CloudWatchLogGroupArn, ProcessedSizeBytes and S3BucketSource.S3BucketOwner (fixture dynamodb-import-lifecycle step task#3).
AWS: Smithy: ImportTableDescription.CloudWatchLogGroupArn, .ProcessedSizeBytes ('the total size of data processed from the source file, in Bytes', LongObject) and .S3BucketSource.S3BucketOwner are all modeled and populated by AWS; a terminal import reports ProcessedSizeBytes (0 when nothing was processed) rather than omitting it.
Emulator: 200 with all three absent.
Why: ProcessedSizeBytes absent is indistinguishable from 'not reported', so progress/After-the-fact accounting code cannot tell a zero-byte import from an unreported one; the missing log-group ARN again strands failure diagnosis.
Fix: the DescribeImport projection of import state — emit all three members.

Cause 7 of 10 — DescribeEndpoints announces an address the emulator is not reachable on (fixture dynamodb-catalog-describe-endpoints step task#1).
AWS: Smithy: DescribeEndpoints output Endpoints is required; Endpoint.Address is a required string ('IP address of the endpoint') and CachePeriodInMinutes is required. The contract is that the returned Address is an address of a host that serves this API — AWS returns its regional service endpoint, and the operation exists so a client can re-target subsequent calls at it.
Emulator: 200 with $.Endpoints[0].Address 'dynamodb.us-west-2.localhost:0' — the replay classifies this as host_not_own: it is neither the regional endpoint nor the host:port the emulator is actually reached on (port 0 is not a listenable port).
Why: endpoint discovery is the one operation whose whole value is the address it returns. A client (or any SDK with endpoint discovery enabled, which the model marks for this service) that honours the announced Address sends every following request to a port nothing listens on and fails with a connection error. Host and port values are environment-specific, so the requirement is only that the announced address be the emulator's own serving address — not this particular string.
Fix: the DescribeEndpoints handler — return the data listener's own reachable host and port (and a real CachePeriodInMinutes), never a hard-coded name with port 0.

Cause 8 of 10 — Scan reports a constant ConsumedCapacity instead of one computed from the data read (fixture dynamodb-scan-return-consumed-capacity step task#7).
AWS: Smithy: Scan.ReturnConsumedCapacity / output ConsumedCapacity.CapacityUnits ('the total number of capacity units consumed by the operation'); ConsumedCapacity is returned only when the request asks for it. AWS computes a Scan's read capacity from the total size of the items examined, rounded up to 4 KB units — one unit per 4 KB for a strongly consistent read and half a unit per 4 KB for an eventually consistent read — so the value grows with the amount of data scanned.
Emulator: 200 with CapacityUnits 0.5 where the recording reports 2.0 — the minimum eventually-consistent single-unit charge, independent of how much was scanned.
Why: capacity reporting is what callers use to size provisioned throughput, implement client-side rate limiting and test backoff; a constant floor makes every scan look free, so throughput-planning and capacity-assertion logic is validated against a value that cannot be right.
Fix: Scan's consumed-capacity computation (shared with Query) — derive units from the summed size of scanned items, the 4 KB rounding and the ConsistentRead flag.
Note: open issue '20261109T055139Z-dynamodb-throughput-and-capacity-reporting-diverges-across-updatetable.md' covers UpdateTable/DescribeTable/PutItem capacity reporting; this Scan cluster is reported with no owner.

Cause 9 of 10 — a parallel Scan segment returns the wrong subset of items (Scan; fixture dynamodb-trajwf-parallel-scan-segment-contract step task#6).
AWS: Smithy: Scan.Segment (0..999999) and Scan.TotalSegments (1..1000000). Recorded behavior only: each (Segment, TotalSegments) pair owns a fixed, disjoint share of the table determined by the items' key hashes, so a given segment returns a specific set of items and the union of all segments covers the table exactly once.
Emulator: 200, but the segment's first item is a different item than the recording's (the recording's segment returns the item keyed web-003; the emulator returns web-001).
Why: a client that parallel-scans by splitting Segment across workers and asserts per-segment contents — or that resumes one segment from a LastEvaluatedKey — sees a different partition than AWS produces, so per-worker results are not reproducible and a resumed segment can re-read or skip items. Hypothesis: the emulator slices items by insertion/scan order rather than by key hash.
Fix: Scan's segment partitioning in the scan/state path — assign items to segments by a hash of the item's key, stably and independently of insertion order.

Cause 10 of 10 — the adjacent parallel-scan segment is likewise mis-assigned (Scan; fixture dynamodb-trajwf-parallel-scan-segment-contract step task#7).
AWS: as cause 9; the recording's other segment returns the item keyed web-002 (workflowId wf-beta, status processing).
Emulator: 200 returning the item keyed web-003 (workflowId wf-alpha, status completed) as that segment's first item.
Why: same as cause 9, and it confirms the misassignment is a shift of the whole partition rather than one stray item — so no segment of a parallel scan matches AWS. The specific item ids here are fixture data, not a requirement; the requirement is the disjoint, key-hash-determined partition.
Fix: same component as cause 9; this cluster is listed separately because it is a distinct replay step a fix must be confirmed against.

## Evidence
Cause 1 (DescribeTableReplicaAutoScaling, dynamodb-table-replica-autoscaling#task#3, 2 affected steps): expected 400 / __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' / message "Global table with name: '<tableName>' does not exist."; actual 200 with no __type and no message.
Cause 2 (UpdateTableReplicaAutoScaling, dynamodb-table-replica-autoscaling#task#4, 2 affected): same expected refusal; actual 200.
Cause 3 (ExportTableToPointInTime, ddbctl-export-time-window-and-token-conflict#task#4, 1 affected): expected 400 / 'com.amazonaws.dynamodb.v20120810#InvalidExportTimeException' / 'Export Time is invalid'; actual 400 / 'com.amazonaws.dynamodb.v20120810#PointInTimeRecoveryUnavailableException' / "Point in time recovery is not enabled for table '<tableName>'".
Cause 4 (ImportTable, dynamodb-import-lifecycle#task#1): $.ImportTableDescription.S3BucketSource.S3BucketOwner missing (expected the owning account id, got None).
Cause 5 (ListImports, dynamodb-import-lifecycle#task#2): $.ImportSummaryList[0] lacks CloudWatchLogGroupArn (expected an arn:aws:logs:<region>:<account>:log-group:/aws-dynamodb/imports:* value).
Cause 6 (DescribeImport, dynamodb-import-lifecycle#task#3): $.ImportTableDescription.CloudWatchLogGroupArn missing, $.ImportTableDescription.ProcessedSizeBytes missing (expected 0, got None), $.ImportTableDescription.S3BucketSource.S3BucketOwner missing.
Cause 7 (DescribeEndpoints, dynamodb-catalog-describe-endpoints#task#1, 4 affected): $.Endpoints[0].Address verdict host_not_own — expected the service endpoint host, got 'dynamodb.us-west-2.localhost:0', which is not the host:port the emulator serves on.
Cause 8 (Scan, dynamodb-scan-return-consumed-capacity#task#7, 2 affected): $.ConsumedCapacity.CapacityUnits expected 2.0, got 0.5.
Cause 9 (Scan, dynamodb-trajwf-parallel-scan-segment-contract#task#6): $.Items[0] expected the item keyed id=web-003 (workflowId wf-alpha, status completed), got the item keyed id=web-001.
Cause 10 (Scan, dynamodb-trajwf-parallel-scan-segment-contract#task#7): $.Items[0].workflowId.S expected 'wf-beta', got 'wf-alpha'; $.Items[0].id.S expected 'web-002', got 'web-003'; $.Items[0].status.S expected 'processing', got 'completed'.

## Steps to Reproduce
1. Create a plain (non-replicated) table, then call DescribeTableReplicaAutoScaling and UpdateTableReplicaAutoScaling on it (fixture dynamodb-table-replica-autoscaling steps task#3 and task#4): both must answer 400 ResourceNotFoundException with the global-table sentence, not 200.
2. Enable PITR on a table and call ExportTableToPointInTime with an ExportTime outside the recovery window (ddbctl-export-time-window-and-token-conflict step task#4): compare $.__type and $.message.
3. Run the import lifecycle in dynamodb-import-lifecycle (steps task#1, task#2, task#3) and inspect S3BucketSource.S3BucketOwner, CloudWatchLogGroupArn and ProcessedSizeBytes in the ImportTable / ListImports / DescribeImport responses.
4. Call DescribeEndpoints (dynamodb-catalog-describe-endpoints step task#1) and attempt to connect to the returned $.Endpoints[0].Address.
5. Scan a table with ReturnConsumedCapacity=TOTAL over several items (dynamodb-scan-return-consumed-capacity step task#7) and compare $.ConsumedCapacity.CapacityUnits against the 4 KB-rounded computation.
6. Scan the fixture table twice with TotalSegments=2 and Segment=0 then Segment=1 (dynamodb-trajwf-parallel-scan-segment-contract steps task#6 and task#7) and compare each segment's item set.
7. Re-replay the corpus: all ten clusters (16 affected steps) must pass.
