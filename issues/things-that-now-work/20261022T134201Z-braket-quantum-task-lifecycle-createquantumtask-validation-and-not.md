Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: braket.braket-quantum-task-bell-circuit-completes-with-correct-measurement-counts braket.braket-cancel-quantum-task-after-completion-conflict-rejected braket.braket-create-quantum-task-invalid-input-rejected braket.braket-resource-not-found-error-semantics-consistent braket.braket-search-devices-invalid-filter-name-rejected
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket.braket-quantum-task-bell-circuit-completes-with-correct-measurement-counts|run#10|AssertionError|assert task["status"] == "COMPLETED", (
Issue-Key: braket.braket-cancel-quantum-task-after-completion-conflict-rejected|run#8|AssertionError|assert task_a["status"] == "COMPLETED", f"task A ended in {task_a['status']}, expected COMPLETED"
Issue-Key: braket.braket-create-quantum-task-invalid-input-rejected|_client_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: braket.braket-resource-not-found-error-semantics-consistent|run#14|AssertionError|assert _get_job_code == "ResourceNotFoundException", (
Issue-Key: braket.braket-search-devices-invalid-filter-name-rejected|run#5|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Braket quantum-task lifecycle, CreateQuantumTask validation and not-found/validation error semantics diverge (4 causes)

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQuantumTask

## Description
Four distinct root causes found in the braket emulator by the behavioral suite. Braket is bootstrapped and answering, so these are fidelity defects, not bootstrap gaps. A fixer must close all FOUR before submitting.

=== Cause 1 of 4: quantum tasks are accepted but never executed; every task ends FAILED ===
AWS: CreateQuantumTask against the SV1 managed simulator (arn:aws:braket:::device/quantum-simulator/amazon/sv1) accepts the task (Smithy: CreateQuantumTask POST /quantum-task, http code 201, output quantumTaskArn) and the service actually dispatches the program; GetQuantumTask (Smithy: output members status (QuantumTaskStatus, required), failureReason, outputS3Directory, shots, endedAt) then transitions the task through to status COMPLETED and the results document is written under the task's outputS3Directory in the caller's output bucket (recorded behavior: a 2-qubit Bell circuit at 100 shots yields a results object whose measurements array has one row per shot). A task that reaches a terminal status is also still visible to ListTagsForResource and SearchQuantumTasks.
Emulator: the task is created, but GetQuantumTask reports status FAILED with failureReason 'This emulator does not execute quantum programs: no device or simulator ran this quantum task, so no result was written to the output S3 location.' No results object is written to S3. Because no task can ever reach COMPLETED, the terminal-state branch of CancelQuantumTask is unreachable too: the cancel-conflict check could not even get to its assertion, failing earlier with 'task A ended in FAILED, expected COMPLETED'. Per the model, cancelling a task in a terminal status must be rejected with ConflictException (Smithy: CancelQuantumTask errors include ConflictException, httpError 409; recorded message example: 'Amazon Braket cannot cancel a quantum task in the COMPLETED status for device ARN arn:aws:braket:::device/quantum-simulator/amazon/sv1.'), while a still-pending task is accepted with 200 and cancellationStatus CANCELLING/CANCELLED.
Why: the Braket data plane is the API plus S3; a client that submits a circuit gets a FAILED task and no measurements, so no SDK/Braket-SDK workflow can run a circuit, read results, or exercise cancel-vs-complete races against the emulator.
Fix: the quantum-task state machine and the simulator execution path behind CreateQuantumTask/GetQuantumTask (the braket task state plus the SV1 simulator backing it), including writing the results document to the output S3 location and the terminal-status rejection branch of CancelQuantumTask.
Traces: braket.braket-quantum-task-bell-circuit-completes-with-correct-measurement-counts, braket.braket-cancel-quantum-task-after-completion-conflict-rejected

=== Cause 2 of 4: CreateQuantumTask does not validate the request; an inaccessible output bucket is accepted ===
AWS: CreateQuantumTask validates the request before accepting it and rejects it with ValidationException (Smithy: CreateQuantumTask errors include ValidationException, httpError 400). Recorded behavior: an outputS3Bucket that is syntactically valid but which the caller does not own / which does not exist is rejected with ValidationException and the message "Caller doesn't have access to bucket or it doesn't exist. Learn more on our developer guide: https://docs.aws.amazon.com/braket/latest/developerguide/braket-slr.html" (the rule is request-derived: the named output bucket is resolved against the caller's S3 before the task is created). The same check also expects rejection of an action JSON that is not a braket schema document ('Provided action is not valid. Please refer to the github repo amazon-braket-schemas-python for examples') and of a shots value above the device's own reported shotsRange maximum ('Shot count should be between <min> and <max> on <deviceArn>, but found <n>.'); those two breaks were never reached because the first one was accepted.
Emulator: the request with the never-created output bucket succeeded with HTTP 201 and returned a quantumTaskArn (observed: arn:aws:braket:<region>:<account>:quantum-task/<uuid>), i.e. no output-bucket, action-document or shots-range validation is performed at all.
Why: an overly permissive create lets a client's broken request look successful; code paths that depend on AWS rejecting a bad bucket, a malformed action or an out-of-range shot count (error handling, retry, user feedback) are never exercised, and the task is created in a state where results could never be written.
Fix: CreateQuantumTask's input validation in its handler, ahead of task creation (output-bucket resolution, action-document schema check, shots against the device's shotsRange from GetDevice deviceCapabilities).
Traces: braket.braket-create-quantum-task-invalid-input-rejected

=== Cause 3 of 4: nonexistent-ARN error semantics are not consistent — GetJob answers NotImplemented ===
AWS: read/mutate operations on a well-formed but never-issued ARN report ResourceNotFoundException with HTTP 404 (Smithy: GetQuantumTask/CancelQuantumTask errors include ResourceNotFoundException, httpError 404; the same shape is modelled on the job and spending-limit operations), with ListTagsForResource the documented exception, which uses NotFoundException (recorded behavior only).
Emulator: GetJob on arn:aws:braket:<region>:<account>:job/<ghost> returns error code 'NotImplemented' with the message 'operation GetJob is scaffolded but not implemented in this emulator', so the consistency sweep stops there and the not-found semantics of GetQuantumTask, ListTagsForResource, DeleteSpendingLimit, CancelJob and CancelQuantumTask are left unverified. NOTE: a scaffold issue naming GetJob (services/service-com-amazonaws-braket-braket/src/ops/get_job.rs) is already filed for braket.braket-malformed-job-arn-returns-validation-not-not-found; this cause is the error-semantics consistency requirement across the whole family and should be re-replayed after that stub is implemented.
Why: a client cannot distinguish 'does not exist' from 'server broken' — NotImplemented is not a modelled Braket error, so SDK error handling and retry classification both go wrong where AWS would raise ResourceNotFoundException (404) or NotFoundException for ListTagsForResource.
Fix: the not-found path of the job/task/spending-limit handlers and the service's error mapping, so a well-formed ARN with no backing resource maps to the modelled ResourceNotFoundException (and NotFoundException for ListTagsForResource) rather than a scaffold NotImplemented.
Traces: braket.braket-resource-not-found-error-semantics-consistent

=== Cause 4 of 4: SearchDevices ValidationException message for an unsupported filter name is not AWS's request-derived message ===
AWS: SearchDevices rejects an unsupported filter name with ValidationException (Smithy: SearchDevices errors include ValidationException, httpError 400; input member filters is a required SearchDevicesFilterList). The message is built from the request — it reports the offending member path and the rejected value, recorded as "{'filters': [{0: [{'name': ['unallowed value notARealFilterName']}]}]}" (member 'filters', element index, member 'name', the unallowed value).
Emulator: the status and error code are correct (ValidationException), but the message is the emulator's own prose, 'Unsupported filter name: notARealFilterName. Supported filter names are: deviceArn, deviceName, deviceStatus, deviceType, providerName'. The positive control (filters=[{name: deviceArn, ...}]) was not reached.
Why: clients and test suites that parse Braket's structured per-member validation payload to point at the offending field get an unparseable string instead; the recorded text itself is only an example, the requirement is the request-derived member/index/value structure.
Fix: the ValidationException message construction in SearchDevices' filter validation (serializer/validation layer), so the message is rendered from the rejected member path and value rather than hand-written prose.
Traces: braket.braket-search-devices-invalid-filter-name-rejected

## Evidence
Suite: 3/16 passed (18.8%); braket 3/16, 11 failed. Two clusters were marked ALREADY FILED (unimplemented CreateSpendingLimit, unimplemented GetJob) and are skipped.

Cause 1:
- braket.braket-quantum-task-bell-circuit-completes-with-correct-measurement-counts — AssertionError: task ended in FAILED, failureReason='This emulator does not execute quantum programs: no device or simulator ran this quantum task, so no result was written to the output S3 location.' Expected: status COMPLETED, then results.json under task['outputS3Directory'] with 100 measurement rows of 2 correlated bits.
- braket.braket-cancel-quantum-task-after-completion-conflict-rejected — AssertionError: task A ended in FAILED, expected COMPLETED. Expected after completion: CancelQuantumTask -> ConflictException (409), message example 'Amazon Braket cannot cancel a quantum task in the COMPLETED status for device ARN arn:aws:braket:::device/quantum-simulator/amazon/sv1.', and the task's status unchanged; a pending task -> 200 with cancellationStatus in (CANCELLING, CANCELLED).

Cause 2:
- braket.braket-create-quantum-task-invalid-input-rejected — AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'quantumTaskArn': 'arn:aws:braket:us-west-2:000000000000:quantum-task/204942b6-62b2-4f85-ba04-e2465a6e8a8a'}. Expected: ValidationException (400) for an outputS3Bucket the caller has no access to.

Cause 3:
- braket.braket-resource-not-found-error-semantics-consistent — AssertionError: expected ResourceNotFoundException, got 'NotImplemented' (on get_job(jobArn='arn:aws:braket:<region>:<account>:job/<ghost>')). Expected: ResourceNotFoundException (404).

Cause 4:
- braket.braket-search-devices-invalid-filter-name-rejected — AssertionError: expected message "{'filters': [{0: [{'name': ['unallowed value notARealFilterName']}]}]}", got 'Unsupported filter name: notARealFilterName. Supported filter names are: deviceArn, deviceName, deviceStatus, deviceType, providerName'. Error code ValidationException matched.

## Steps to Reproduce
Boot the emulator and point a botocore braket client at it in us-west-2.

Cause 1: s3.create_bucket(Bucket='amazon-braket-<uniq>'); braket.create_quantum_task(clientToken=<uniq>, deviceArn='arn:aws:braket:::device/quantum-simulator/amazon/sv1', action=json.dumps({'braketSchemaHeader': {'name': 'braket.ir.openqasm.program', 'version': '1'}, 'source': 'OPENQASM 3.0; bit[2] b; qubit[2] q; h q[0]; cnot q[0], q[1]; b[0] = measure q[0]; b[1] = measure q[1];'}), shots=100, outputS3Bucket=<bucket>, outputS3KeyPrefix='results'); poll braket.get_quantum_task(quantumTaskArn=...) until terminal -> observe FAILED instead of COMPLETED, and no results object under outputS3Directory. Then, on a task that reached a terminal status, braket.cancel_quantum_task(...) -> expect ConflictException 409.

Cause 2: same create call but with outputS3Bucket set to a bucket that was never created -> observe HTTP 201 + quantumTaskArn instead of ValidationException 400. Then repeat with action=json.dumps({'foo':'bar'}) and with shots = (GetDevice deviceCapabilities service.shotsRange max + 1).

Cause 3: braket.get_job(jobArn='arn:aws:braket:us-west-2:000000000000:job/never-created') -> observe Code 'NotImplemented' instead of ResourceNotFoundException; repeat for get_quantum_task / list_tags_for_resource / delete_spending_limit / cancel_job / cancel_quantum_task on well-formed nonexistent ARNs.

Cause 4: braket.search_devices(filters=[{'name': 'notARealFilterName', 'values': ['x']}]) -> ValidationException whose Message is the emulator's prose instead of the request-derived per-member structure.
