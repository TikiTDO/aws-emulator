Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune-graph.neptune-graph-delete-graph-deletion-protection-rejected neptune-graph.neptune-graph-create-graph-duplicate-name-conflict-rejected neptune-graph.neptune-graph-start-graph-wrong-status-rejected neptune-graph.import-export-task-nonexistent-not-found neptune-graph.import-task-invalid-role-arn-rejected neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable neptune-graph.neptune-graph-execute-query-during-graph-creation-conflict neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected neptune-graph.create-graph-snapshot-of-graph-in-creating-state-conflict-rejected
Issue-Shape-Id: com.amazonaws.neptunegraph#AmazonNeptuneGraph
Issue-Key: neptune-graph.neptune-graph-delete-graph-deletion-protection-rejected|run#4|AssertionError|assert code == "ValidationException", code
Issue-Key: neptune-graph.neptune-graph-create-graph-duplicate-name-conflict-rejected|run#7|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: neptune-graph.neptune-graph-start-graph-wrong-status-rejected|run#4|AssertionError|assert code == "ConflictException", code
Issue-Key: neptune-graph.import-export-task-nonexistent-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", f"CODE={code!r}"
Issue-Key: neptune-graph.import-task-invalid-role-arn-rejected|run#6|AssertionError|assert code == "AccessDeniedException", f"CODE={code!r}"
Issue-Key: neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: neptune-graph.neptune-graph-execute-query-during-graph-creation-conflict|run#8|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: neptune-graph.create-graph-snapshot-of-graph-in-creating-state-conflict-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException for CreateGraphSnapshot on a CREATING graph, got {code!r}"
Issue-Commit: a91eb31cf888327ed8ad916c03309e3893f84f70

# neptune-graph: 7 refusal-path fidelity causes — wrong error code for deletion protection, non-AWS conflict message, and StartGraph / import-export task / ExecuteQuery / CreateGraphSnapshot refusals answered as 501 NotImplemented

> _Note: this issue was identified against the code base at commit `a91eb31cf888327ed8ad916c03309e3893f84f70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGraph

## Description
The neptune-graph emulator is serving (CreateGraph/GetGraph/DeleteGraph work and graphs reach AVAILABLE), but seven distinct refusal/error-semantics causes diverge from AWS. All seven are in the neptune-graph service. Smithy model consulted: services/service-com-amazonaws-neptunegraph-amazonneptunegraph/smithy-model/com-amazonaws-neptunegraph-amazonneptunegraph.json.

=== Cause 1 of 7: DeleteGraph on a deletion-protected graph answers ConflictException instead of ValidationException ===
AWS: DeleteGraph on a graph with deletionProtection=true is refused with ValidationException (Smithy: DeleteGraph errors include ValidationException @httpError 400 and ConflictException @httpError 409; the recorded AWS response for this case is ValidationException/400 with the request-derived message 'Graph [<graphId>] cannot be deleted because deletion protection is enabled.'). The graph is left intact (status AVAILABLE, deletionProtection still true) and the delete succeeds after UpdateGraph clears protection.
Emulator: returns ConflictException (409). services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/delete_graph.rs builds a ConflictException in the deletion-protection branch.
Why: a client that distinguishes a permanent input/configuration problem (400 ValidationException) from a retryable state conflict (409 ConflictException) will treat the protected-graph refusal as a transient conflict and retry forever, and SDK retry/error-classification logic keyed on the error shape takes the wrong branch.
Fix: the deletion-protection refusal branch of DeleteGraph's handler (delete_graph.rs, the `if graph.deletion_protection` branch at lines 187-196) must raise the ValidationException variant with the message template naming the graph id.
Traces: neptune-graph.neptune-graph-delete-graph-deletion-protection-rejected (AssertionError: expected 'ValidationException', got 'ConflictException').

=== Cause 2 of 7: CreateGraph duplicate-name ConflictException does not use AWS's request-derived message template ===
AWS: CreateGraph with an existing graphName is refused with ConflictException (Smithy: CreateGraph errors include ConflictException @httpError 409) and the message is built from the request's graphName: 'Graph with name <graphName> already exists!'. The code and status match in the emulator; only the message rule diverges, and the rule is known because the message is derived from the request member graphName.
Emulator: same code/status, but the message is "a graph named '<graphName>' already exists with id <graphId>" — an emulator-authored sentence that additionally leaks the colliding graph's id, which AWS does not disclose on this path.
Why: clients and operators that match on the documented message text (and tests/log scrapers built against AWS) do not recognize the refusal, and the extra graph id discloses a resource the caller was not told about.
Fix: the duplicate-name branch of CreateGraph's handler (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/create_graph.rs) must format the ConflictException message from the requested graphName only.
Traces: neptune-graph.neptune-graph-create-graph-duplicate-name-conflict-rejected (AssertionError: expected message 'Graph with name <name> already exists!', got "a graph named '<name>' already exists with id g-...").

=== Cause 3 of 7: StartGraph cannot refuse a graph that is not STOPPED — answers 501 NotImplemented ===
AWS: StartGraph on a graph already in AVAILABLE is refused with ConflictException (Smithy: StartGraph errors ConflictException @httpError 409, ValidationException 400, ResourceNotFoundException 404; recorded message 'Unable to start the graph because it is in the AVAILABLE state.'), and the graph's status is unchanged. StartGraph succeeds only from STOPPED.
Emulator: StartGraph is a scaffold stub (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_graph.rs returns Scaffolded::not_implemented("StartGraph"), HTTP 501 'NotImplemented'), so no state gate exists at all.
Why: a caller driving the stop/start lifecycle gets an unmodeled 501 instead of the modeled 409, so it can neither start a stopped graph nor detect that a start was inapplicable.
Fix: implement StartGraph in start_graph.rs: resolve the graph, refuse any non-STOPPED status with the modeled ConflictException naming the current state, otherwise transition STOPPED -> STARTING -> AVAILABLE in the service state.
Traces: neptune-graph.neptune-graph-start-graph-wrong-status-rejected (AssertionError: expected 'ConflictException', got 'NotImplemented').

=== Cause 4 of 7: the import/export task read and cancel operations cannot report a nonexistent task — 501 instead of ResourceNotFoundException ===
AWS: GetImportTask, CancelImportTask, GetExportTask and CancelExportTask each refuse an unknown but well-formed taskIdentifier with ResourceNotFoundException (Smithy: all four model ResourceNotFoundException @httpError 404; taskIdentifier pattern ^t-[a-z0-9]{10}$, so 't-0000000000' passes shape validation and must reach resource lookup). Recorded messages are request/account-derived, e.g. 'No import task found for customer [<account>] with import task id [<taskId>]' and 'No export task found for customer [<account>] with export task id [<taskId>]'.
Emulator: all four answer HTTP 501 'NotImplemented' (scaffold stubs under services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/{get_import_task,cancel_import_task,get_export_task,cancel_export_task}.rs).
Why: the whole import/export task family is unusable, and a client polling a task id cannot distinguish 'task gone' (404) from an unmodeled server-side 501, so poll loops never terminate.
Fix: implement the four task lookup/cancel handlers with a task registry in crate::state and raise the modeled ResourceNotFoundException (404) with the account/task-id message template when the id is unknown.
Traces: neptune-graph.import-export-task-nonexistent-not-found (AssertionError: CODE='NotImplemented' where ResourceNotFoundException was required).

=== Cause 5 of 7: StartImportTask validates roleArn shape instead of authorizing it first ===
AWS: recorded behavior only — StartImportTask authorizes roleArn before any other validation, so BOTH a non-ARN string ('not-an-arn') and a well-formed foreign-account role ARN are refused with AccessDeniedException (HTTP 403). AccessDeniedException is NOT in StartImportTask's modeled error list (Smithy: ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400), i.e. AWS answers with the service's common denial shape ahead of the modeled shape checks; recorded example messages are 'User: <callerArn> is not authorized to perform: null' and 'Cross-account pass role is not allowed.'. The requirement here is the status/code and the ordering, not the recorded message text.
Emulator: answers ValidationException (400) — the modeled roleArn @pattern (^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$) is enforced by the generic pre-handler shape validation, so the authorization gate never runs.
Why: a client that retries or re-prompts for credentials on 403 and treats 400 as a permanent bad-input bug diagnoses an authorization problem as a malformed request; a cross-account pass-role attempt is reported as a syntax error.
Fix: StartImportTask's handler (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_import_task.rs) must perform the roleArn authorization decision ahead of shape validation for this member, returning the 403 AccessDenied shape.
Traces: neptune-graph.import-task-invalid-role-arn-rejected (AssertionError: CODE='ValidationException' where AccessDeniedException was required).

=== Cause 6 of 7: ExecuteQuery's three modeled refusal paths are all unreachable ===
AWS: ExecuteQuery (Smithy: errors AccessDeniedException @httpError 403, ConflictException 409, UnprocessableException 422, ValidationException 400, ThrottlingException 429; endpoint hostPrefix '{graphIdentifier}.') refuses (a) a syntactically malformed openCypher query with ValidationException/400 before query processing — recorded example message "Invalid input 'R': expected whitespace, comment, COPY, node labels, a property map, ')', MapLiteral or a relationship pattern (line 1, column 10 (offset: 9))"; (b) a query against a graph still CREATING with ResourceNotFoundException (the per-graph data-plane endpoint is not registered yet); (c) a caller whose IAM principal has no neptune-graph permissions with AccessDeniedException/403 whose message is built from the caller and resource ARNs ('User: <principalArn> is not authorized to perform: neptune-graph:DeleteDataViaQuery, neptune-graph:ReadDataViaQuery, neptune-graph:WriteDataViaQuery on resource: <graphArn>').
Emulator: every ExecuteQuery call answers HTTP 501 'NotImplemented' (scaffold stub services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/execute_query.rs), so none of these three decisions exists. In the authorization check the 501 also makes the harness's poll loop never converge, and the check is killed at the 60 s cap. NOTE: a stub issue naming ExecuteQuery and execute_query.rs is already auto-filed for the 8 checks that abort on the raw 501; this cause records the error semantics those three checks additionally require, which 'make the operation return something' does not cover — close them together.
Why: a client cannot tell a bad query (400/422, do not retry) from a not-yet-ready graph (404, retry) from a permission problem (403, fix IAM); and because an unauthorized caller is not refused, authorization is not enforced on the data plane at all.
Fix: implement ExecuteQuery in execute_query.rs with (1) openCypher syntax validation raising the modeled ValidationException, (2) a graph-status/endpoint gate raising ResourceNotFoundException while the graph is CREATING, and (3) an authorization check raising the 403 AccessDenied shape with the principal/resource-derived message.
Traces: neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable (got 'NotImplemented', expected ValidationException); neptune-graph.neptune-graph-execute-query-during-graph-creation-conflict (got 'NotImplemented', expected ResourceNotFoundException); neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected (check timed out after 60.0s polling for AccessDeniedException while every call answered 501).

=== Cause 7 of 7: CreateGraphSnapshot does not refuse a snapshot of a non-AVAILABLE graph ===
AWS: CreateGraphSnapshot against a graph still in CREATING is refused with ValidationException (Smithy: CreateGraphSnapshot errors ValidationException @httpError 400, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402), no snapshot is created (ListGraphSnapshots shows none with that name), and the identical request succeeds once the graph is AVAILABLE. Recorded example message: 'Graph [<graphId>] is not available for snapshot. You can only create a snapshot for an AVAILABLE Graph.'
Emulator: answers HTTP 501 'NotImplemented' (scaffold stub services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/create_graph_snapshot.rs). NOTE: a stub issue naming CreateGraphSnapshot and that file is already auto-filed for the 2 checks that abort on the raw 501; this cause records the graph-status gate and 400 shape that check additionally requires — close them together.
Why: a client that snapshots right after CreateGraph gets an unmodeled 501 rather than the modeled 400, so it cannot learn it must wait for AVAILABLE, and cannot rely on the 'no snapshot created' guarantee.
Fix: implement CreateGraphSnapshot in create_graph_snapshot.rs with a graph-status gate that raises the modeled ValidationException (naming the graph id) for any non-AVAILABLE source graph, leaving the snapshot store untouched.
Traces: neptune-graph.create-graph-snapshot-of-graph-in-creating-state-conflict-rejected (AssertionError: expected ValidationException for CreateGraphSnapshot on a CREATING graph, got 'NotImplemented').

## Evidence
Behavioral suite: 11/35 passed (31.4%); neptune-graph 11/35, 24 failed. Net-new (non-auto-filed) failures and their verdicts:

1. [mismatch] neptune-graph.neptune-graph-delete-graph-deletion-protection-rejected — expected ValidationException (400) for DeleteGraph on a deletionProtection=true graph; got ConflictException (409). AssertionError: ConflictException.
2. [mismatch] neptune-graph.neptune-graph-create-graph-duplicate-name-conflict-rejected — code ConflictException matches; message diverges. Expected 'Graph with name sim2real-ng-10a02098637f already exists!', got "a graph named 'sim2real-ng-10a02098637f' already exists with id g-54t2qk16z6".
3. [mismatch] neptune-graph.neptune-graph-start-graph-wrong-status-rejected — expected ConflictException (409) 'Unable to start the graph because it is in the AVAILABLE state.'; got 'NotImplemented' (HTTP 501).
4. [mismatch] neptune-graph.import-export-task-nonexistent-not-found — expected ResourceNotFoundException (404) from GetImportTask('t-0000000000'); got 'NotImplemented'. Same for CancelImportTask / GetExportTask / CancelExportTask.
5. [mismatch] neptune-graph.import-task-invalid-role-arn-rejected — StartImportTask(roleArn='not-an-arn') expected AccessDeniedException (403); got ValidationException (400).
6. [mismatch] neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable — 'MATCH (n RETURN n' expected ValidationException (400); got 'NotImplemented'.
   [mismatch] neptune-graph.neptune-graph-execute-query-during-graph-creation-conflict — expected ResourceNotFoundException while the graph is CREATING; got 'NotImplemented'.
   [mismatch] neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected — expected AccessDeniedException (403) for a policy-less IAM user; every ExecuteQuery answered 501, the harness poll never converged, 'Check timed out after 60.0s'.
7. [mismatch] neptune-graph.create-graph-snapshot-of-graph-in-creating-state-conflict-rejected — expected ValidationException (400) 'Graph [g-...] is not available for snapshot. You can only create a snapshot for an AVAILABLE Graph.'; got 'NotImplemented'.

Source evidence in the checkout:
- services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/delete_graph.rs:187-196 returns DeleteGraphErrorModeled::ConflictException for the deletion-protection refusal (ValidationException is in the same modeled enum, lines 107-113).
- services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_graph.rs:182 returns Scaffolded::not_implemented("StartGraph").
- Smithy model (com-amazonaws-neptunegraph-amazonneptunegraph.json): DeleteGraph/StartGraph/CreateGraphSnapshot each model ValidationException 400 and ConflictException 409; GetImportTask/CancelImportTask/GetExportTask/CancelExportTask each model ResourceNotFoundException 404; ExecuteQuery models AccessDeniedException 403, UnprocessableException 422, ValidationException 400; StartImportTask models NO AccessDeniedException (its 403 is recorded behavior only) and constrains roleArn with pattern ^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$.

Auto-filed stub failures excluded from this issue: ExecuteQuery (8 checks), CreateGraphSnapshot (2), ListTagsForResource (2), ListQueries (1), StopGraph (1), CreatePrivateGraphEndpoint (1).

## Steps to Reproduce
Boot the emulator from this checkout and point a botocore neptune-graph client at it.

Cause 1: g = create_graph(graphName=<unique>, provisionedMemory=16, deletionProtection=True, publicConnectivity=True); wait for GetGraph status AVAILABLE; delete_graph(graphIdentifier=g['id'], skipSnapshot=True) -> observe ConflictException/409, expected ValidationException/400 with message 'Graph [<id>] cannot be deleted because deletion protection is enabled.'
Cause 2: create a graph, then create_graph(graphName=<same name>, provisionedMemory=32, deletionProtection=False, publicConnectivity=True) -> ConflictException whose message is the emulator's own sentence including the existing graph id, instead of 'Graph with name <name> already exists!'.
Cause 3: create a graph and wait for AVAILABLE; start_graph(graphIdentifier=<id>) -> 501 NotImplemented, expected ConflictException/409 'Unable to start the graph because it is in the AVAILABLE state.'
Cause 4: get_import_task(taskIdentifier='t-0000000000'), cancel_import_task(...), get_export_task(...), cancel_export_task(...) -> each 501 NotImplemented, expected ResourceNotFoundException/404.
Cause 5: start_import_task(graphIdentifier='g-0000000000', source='s3://any/prefix/', roleArn='not-an-arn') -> ValidationException/400, expected AccessDeniedException/403; repeat with roleArn='arn:aws:iam::111111111111:role/x' -> also expected 403.
Cause 6: create a graph and wait for AVAILABLE; execute_query(graphIdentifier=<id>, queryString='MATCH (n RETURN n', language='OPEN_CYPHER') -> 501, expected ValidationException/400. Immediately after CreateGraph (status CREATING), execute_query(... 'RETURN 1 AS one') -> 501, expected ResourceNotFoundException. With credentials of a policy-less IAM user, execute_query on an AVAILABLE graph -> 501, expected AccessDeniedException/403 naming the principal and graph ARN.
Cause 7: create_graph(...); while GetGraph reports CREATING, create_graph_snapshot(graphIdentifier=<id>, snapshotName=<unique>) -> 501, expected ValidationException/400 and no snapshot in list_graph_snapshots.
