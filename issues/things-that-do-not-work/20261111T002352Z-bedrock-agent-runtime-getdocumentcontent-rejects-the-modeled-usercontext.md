Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-get-document-content-both-output-formats
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-get-document-content-both-output-formats
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|GetDocumentContent|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|GetDocumentContent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|GetDocumentContent|4xx->4xx|status
Issue-Commit: 4fbb8d92928479b0d33d95ed4fefcdea254a8e5f

# bedrock-agent-runtime: GetDocumentContent rejects the modeled userContext member with an unmodeled ValidationException, pre-empting the unknown-knowledge-base 404

> _Note: this issue was identified against the code base at commit `4fbb8d92928479b0d33d95ed4fefcdea254a8e5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDocumentContent

## Description
One net-new cause (1 cause) on bedrock-agent-runtime GetDocumentContent (POST /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/documents/{documentId}/content). The emulator refuses any GetDocumentContent that carries the modeled optional userContext member with a 400 ValidationException of its own authorship, so the operation can never be served for an identity-scoped read and the service's ResourceNotFoundException refusal for a knowledge base that does not exist is never reached. The same divergence occurs for both values of outputFormat (RAW and EXTRACTED), i.e. the refusal is raised before any output-format handling.

=== Cause 1 of 1: a GetDocumentContent carrying the modeled userContext member is refused 400 ValidationException instead of being served, so the unknown-knowledge-base 404 ResourceNotFoundException is unobservable ===
AWS: Smithy (services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model): GetDocumentContent = POST /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/documents/{documentId}/content, @readonly, success 200, output {mimeType @required, presignedUrl @required @sensitive, documentContentLength}. Its input GetDocumentContentRequest defines an OPTIONAL member userContext -> UserContext (@sensitive structure, @required userId), documented "Contains information about the user making the request. This is used for access control filtering to ensure that results only include documents the user is authorized to access." It also defines the optional enum outputFormat -> DocumentOutputFormat (values RAW, EXTRACTED). userContext is therefore a modeled, accepted input member; the modeled ValidationException (client, httpError 400, doc "Input validation failed. Check your request parameters and retry the request.") is for input that violates the model's constraints, not for a member the model defines. The modeled ResourceNotFoundException (client, httpError 404, doc "The specified resource Amazon Resource Name (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.") is what a well-formed knowledgeBaseId naming no knowledge base draws, and the recorded response shows resource resolution happening BEFORE any read-shaping input is interpreted: HTTP 404, x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/', $.message "Knowledge Base with id <knowledgeBaseId> does not exist". That template is the requirement: the fixed text is "Knowledge Base with id " ... " does not exist", and the only variable part is the knowledgeBaseId taken verbatim from the request path (the recording's KBNOTFND01 is one filled-in value, not part of the requirement). userContext itself draws no rejection at all.
Emulator: answers HTTP 400 with x-amzn-errortype 'ValidationException' and the message "userContext: this service applies no per-document access control to a content read, so a read for one user cannot be narrowed to the documents that user may read; omit userContext to read an ingested document". That whole sentence is fixed emulator prose (only the leading member name "userContext" comes from the request) and corresponds to no AWS error class, status or wording. Because it is raised ahead of knowledge-base resolution, the 404 ResourceNotFoundException branch is unreachable for any request that sets userContext, and it is raised for both outputFormat=RAW and outputFormat=EXTRACTED.
Why: a client doing the documented access-control-filtered document read gets its request rejected outright rather than served, so identity-scoped document retrieval cannot run against the emulator at all; and because the refusal pre-empts existence checking, error handling keyed on ResourceNotFoundException never fires — a typo'd, deleted or not-yet-created knowledgeBaseId is reported as an input-validation failure (400 ValidationException) instead of a missing resource (404 ResourceNotFoundException), breaking retry/fallback and provisioning-wait logic that distinguishes the two, and showing the user a message AWS never emits.
Fix: the GetDocumentContent operation's input validation in the bedrock-agent-runtime crate must stop rejecting the modeled userContext member, and the operation's handler must resolve/validate knowledgeBaseId (then dataSourceId and documentId) before interpreting read-shaping input, emitting ResourceNotFoundException with httpError 404, x-amzn-errortype ResourceNotFoundException and the service's "Knowledge Base with id <knowledgeBaseId> does not exist" template formatted from the request path when the knowledge base does not exist; document-content state should accept userContext and, where no per-document ACL data exists, serve the ingested document rather than refuse.
Traces: bedrock-agent-runtime-get-document-content-both-output-formats#task#1 (outputFormat one) and #task#2 (the other outputFormat) — one cluster, same cause.

Related open issues that do NOT cover this: 20261103T030007Z-bedrock-agent-runtime-getdocumentcontent-stub.md describes the earlier unimplemented/NotImplemented scaffold symptom, not this unmodeled 400 ValidationException (the operation now answers, and answers wrongly); 20261109T032116Z-bedrock-agent-runtime-retrieve-s-unknown-knowledge-base.md covers the same shape of fault on Retrieve only, and carries Retrieve's replay keys, not GetDocumentContent's. Neither carries the replay key of the step below.

## Evidence
Cause 1 — fixture bedrock-agent-runtime-get-document-content-both-output-formats, step task#1 (GetDocumentContent; request carries userContext and a well-formed but unprovisioned knowledgeBaseId); step task#2 is the same call with the other outputFormat and diverges identically (cluster affected_count=2).
  Expected: HTTP 404, headers x-amzn-errortype = 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/', $.message = "Knowledge Base with id KBNOTFND01 does not exist" (fixed text "Knowledge Base with id " / " does not exist"; the only variable part is the request path's knowledgeBaseId).
  Actual: HTTP 400, headers x-amzn-errortype = 'ValidationException', $.message = "userContext: this service applies no per-document access control to a content read, so a read for one user cannot be narrowed to the documents that user may read; omit userContext to read an ingested document" (entirely fixed emulator prose apart from the leading member name).
  Divergence key: service=bedrock-agent-runtime|GetDocumentContent|status=404->400|keys=$.message,$headers.x-amzn-errortype,status; verdict=mismatch, stub_path empty, owned_by empty.

Replay summary: trace set simparity-wire-aws-v1, emulator up (health 200, data listener accepting), 49 fixtures replayed, 215 steps, 170 passed, score 79.1%.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS SDK/CLI bedrock-agent-runtime client at its data listener.
2. Call GetDocumentContent(knowledgeBaseId="KBNOTFND01" — any well-formed 10-character id that was never created, dataSourceId=<any well-formed id>, documentId=<any id>, outputFormat="RAW", userContext={"userId": "<any user id>"}).
3. Observe HTTP 400 with x-amzn-errortype ValidationException and the emulator-authored userContext message, instead of HTTP 404 with x-amzn-errortype ResourceNotFoundException and $.message "Knowledge Base with id KBNOTFND01 does not exist".
4. Repeat with outputFormat="EXTRACTED": the same 400 is returned, showing the refusal precedes output-format handling.
5. Repeat against an EXISTING knowledge base / data source / ingested document with userContext set: the same 400 is returned instead of a 200 carrying mimeType and presignedUrl.
6. Re-replay to confirm: fixture bedrock-agent-runtime-get-document-content-both-output-formats, steps task#1 and task#2.
