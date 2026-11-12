Issue-Source: trace-sweep-divergence
Issue-Trace: braket-sv1-deterministic-circuit-results-in-s3
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-sv1-deterministic-circuit-results-in-s3
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: s3|GetObject|2xx->2xx|$.taskMetadata.deviceParameters
Issue-Related: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md
Issue-Commit: c068b04c9070a7dffb3e8c7bd93f77709d5e2d9a

# Braket quantum-task results object served by S3 diverges: application/json content type and taskMetadata without deviceParameters

> _Note: this issue was identified against the code base at commit `c068b04c9070a7dffb3e8c7bd93f77709d5e2d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Two root causes on the S3 read that ends the Braket workflow: the result object a completed quantum task leaves under its outputS3Directory is now present and served 200 (good), but S3 serves it with the WRONG Content-Type and its body's taskMetadata document is missing a member. Both were found by replaying the wire corpus (fixture braket-sv1-deterministic-circuit-results-in-s3, step task#5, service s3, operation GetObject, verdict mismatch, expected 200 / actual 200). A fixer must close BOTH before submitting.

Note on already-open issues: 20261110T003300Z-s3-getobject-404-for-the-braket-quantum-task.md describes an EARLIER state of this same step, when GetObject answered 404 because no task execution ever wrote the key; the code base has since drifted (the object is written and the step now answers 200), and the replay reports owned_by EMPTY for this cluster, so that issue does not cover the two divergences below. 20261110T003300Z-braket-quantum-task-create-read-wire-shape-diverges.md owns the braket-side API divergences for the same fixture (steps task#3/task#4), including deviceParameters not being STORED at create time; the cause below is a separate defect in the result-document WRITER, which emits no deviceParameters member at all whatever is stored. Both are named only so a fixer can fix them together.

=== Cause 1 of 2: the result object is stored and served as application/json instead of binary/octet-stream ===
AWS: recorded behavior only for the media type. Amazon Braket writes a finished gate-model task's result document into the caller's output bucket, and reading that key with S3 GetObject returns it with content-type 'binary/octet-stream' — S3's content type for an object whose write declared none (Smithy, s3: GetObject output member ContentType binds httpHeader Content-Type and is echoed from what PutObject's ContentType member recorded for the object; PutObject input member ContentType binds httpHeader Content-Type and is optional). The bucket name, key, etag, last-modified and checksum values are per-run and are not requirements; the media type is.
Emulator: GetObject answers the object with 'content-type: application/json' — replay: "$headers.content-type: value_mismatch (expected 'binary/octet-stream', got 'application/json')". The stored value is the emulator's own choice: braket's writer explicitly sets a JSON content type on the internal PutObject, so S3 faithfully echoes a content type AWS's own write never records.
Why: a client that dispatches on the object's Content-Type (a download helper choosing text vs binary handling, a browser/CDN fetch of the results key, or a test asserting the response headers of a Braket results read) sees a different media type than against AWS, and any code that treats 'binary/octet-stream' as the marker of a Braket-written result object misclassifies it.
Fix: the Braket result-document write path (the content type it declares when putting the results object through the in-process S3 client), and/or S3's stored-object content-type handling so the object GetObject serves carries the media type AWS records for this write. No change is needed in GetObject's header serialization itself — it is echoing what was stored.
Traces: braket-sv1-deterministic-circuit-results-in-s3#task#5 (s3 GetObject). Code: services/service-com-amazonaws-braket-braket/src/state/task_results.rs:44 (const RESULTS_CONTENT_TYPE = "application/json") and :113 (.content_type(RESULTS_CONTENT_TYPE) on the PutObject that writes the object).

=== Cause 2 of 2: the result document's taskMetadata omits deviceParameters ===
AWS: recorded behavior only for the document shape. The object body is a braket.task_result.gate_model_task_result document whose nested taskMetadata (braket.task_result.task_metadata) reports the task as it was submitted — braketSchemaHeader, id (the task ARN), shots, deviceId, createdAt, endedAt, status AND deviceParameters, the device-parameters document the task was created with, echoed verbatim. The braket API models the same value as a required read-back member (Smithy, braket: GetQuantumTask output member deviceParameters, JsonValue, REQUIRED, mediaType application/json — 'The parameters for the device on which the quantum task ran'), so both the API read and the result document report it. The CONTENTS of that document are whatever the request sent; its presence is the requirement.
Emulator: taskMetadata is assembled without any deviceParameters member, so the member is absent from the served body — replay: '$.taskMetadata.deviceParameters: missing (expected {"braketSchemaHeader": {"name": "braket.device_schema.simulators.gate_model_simulator_device_parameters", "version": "1"}, "paradigmParameters": {"braketSchemaHeader": {"name": "braket.device_schema.gate_model_parameters", "version": "1"}, "qubitCount": 2, "disableQubitRewiring": false}}, got None)'. This is independent of the create-time storage defect filed on the braket side: the writer builds no such member even when a document is held.
Why: the Amazon Braket SDK parses this object into its result type and reads taskMetadata to report what the task ran under; a consumer that reads result.task_metadata.deviceParameters (to verify the qubit count / rewiring settings a run used, or to re-submit an identical task) gets nothing where AWS gives the submitted document, and a strict schema validation of the result document sees a taskMetadata missing a member AWS populates.
Fix: the Braket result-document builder — the taskMetadata object it assembles must carry the task's stored device-parameters document (and the result context it is built from must carry that document through from the task record).
Traces: braket-sv1-deterministic-circuit-results-in-s3#task#5 (s3 GetObject). Code: services/service-com-amazonaws-braket-braket/src/state/task_results.rs:169-180 (the "taskMetadata" object in gate_model_result, which lists id/deviceId/shots/createdAt/endedAt/status and no deviceParameters) and :128-141 (ResultContext, which carries no device-parameters field to put there).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 39 steps total, 30 passed, 9 mismatched, 0 errored, 0 blocked, score 76.9%, 7 divergence clusters. Six clusters are braket clusters already owned by open issues (owned_by non-empty); this is the one cluster with owned_by EMPTY.

Fixture braket-sv1-deterministic-circuit-results-in-s3, step task#5, service s3, operation GetObject, verdict mismatch, expected_status 200, actual_status 200, divergence_key 'service=s3|GetObject|status=200->200|keys=$.taskMetadata.deviceParameters,$headers.content-type', affected_count 1:
  $.taskMetadata.deviceParameters: missing (expected {'braketSchemaHeader': {'name': 'braket.device_schema.simulators.gate_model_simulator_device_parameters', 'version': '1'}, 'paradigmParameters': {'braketSchemaHeader': {'name': 'braket.device_schema.gate_model_parameters', 'version': '1'}, 'qubitCount': 2, 'disableQubitRewiring': False}}, got None)
  $headers.content-type: value_mismatch (expected 'binary/octet-stream', got 'application/json')

Everything else about the step matches: the object is served 200 with its gate_model_task_result body (braketSchemaHeader, measurements, measuredQubits, resultTypes, additionalMetadata and the remaining taskMetadata members all pass), so the two deltas above are the whole divergence.

Models consulted:
  smithy_query(query='operation', operation='GetObject,PutObject', model_path='services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json') — GetObject output ContentType binds httpHeader 'Content-Type' (echoed from the stored object); PutObject input ContentType binds httpHeader 'Content-Type' and is optional, so a write that declares none leaves S3 to report its default.
  smithy_query(query='operation', operation='GetQuantumTask', model_path='services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json') — deviceParameters is a REQUIRED output member, JsonValue, mediaType application/json: 'The parameters for the device on which the quantum task ran'.

Code read (commit of this checkout): services/service-com-amazonaws-braket-braket/src/state/task_results.rs:44 and :113 declare and apply content type 'application/json' on the internal PutObject of the results object; :169-180 build taskMetadata with no deviceParameters member; :128-141 define ResultContext with no field to carry it.

## Steps to Reproduce
Boot the emulator and replay fixture braket-sv1-deterministic-circuit-results-in-s3 (replay_wire_corpus), or by hand with botocore clients in us-west-2:

1. s3.create_bucket(Bucket='amazon-braket-<uniq>').
2. braket.create_quantum_task(clientToken=<uniq>, deviceArn='arn:aws:braket:::device/quantum-simulator/amazon/sv1', shots=10, outputS3Bucket=<bucket>, outputS3KeyPrefix='results', action=json.dumps({'braketSchemaHeader': {'name': 'braket.ir.openqasm.program', 'version': '1'}, 'source': 'OPENQASM 3.0; bit[2] b; qubit[2] q; x q[0]; b[0] = measure q[0]; b[1] = measure q[1];'}), deviceParameters=json.dumps({'braketSchemaHeader': {'name': 'braket.device_schema.simulators.gate_model_simulator_device_parameters', 'version': '1'}, 'paradigmParameters': {'braketSchemaHeader': {'name': 'braket.device_schema.gate_model_parameters', 'version': '1'}, 'qubitCount': 2, 'disableQubitRewiring': False}})).
3. t = braket.get_quantum_task(quantumTaskArn=<arn from step 2>); poll until the status is terminal.
4. r = s3.get_object(Bucket=t['outputS3Bucket'], Key=t['outputS3Directory'] + '/results.json')
   -> Cause 1: inspect r['ContentType'] (or the raw response's Content-Type header): it is 'application/json'; AWS serves this object as 'binary/octet-stream'.
   -> Cause 2: doc = json.loads(r['Body'].read()); doc['taskMetadata'] has no 'deviceParameters' key. Against AWS it carries the device-parameters document submitted in step 2.
