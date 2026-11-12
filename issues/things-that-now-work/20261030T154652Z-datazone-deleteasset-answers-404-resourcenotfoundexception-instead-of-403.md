Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-domain-scoped-denied-sweep-9
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-domain-scoped-denied-sweep-9
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|DeleteAsset|4xx->4xx|$.message
Issue-Key: datazone|DeleteAsset|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|DeleteAsset|4xx->4xx|status
Issue-Commit: 9b1913dfb1a265094321f560fd11d2f4ba8c2a4f

# datazone DeleteAsset answers 404 ResourceNotFoundException instead of 403 AccessDeniedException on a denied domain

> _Note: this issue was identified against the code base at commit `9b1913dfb1a265094321f560fd11d2f4ba8c2a4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAsset

## Description
ONE cause. Cause 1 of 1: DeleteAsset, fixture datazone-domain-scoped-denied-sweep-9, step task#2.

AWS: DeleteAsset (Smithy: com.amazonaws.datazone#DataZone, DELETE /v2/domains/{domainIdentifier}/assets/{identifier}, errors AccessDeniedException httpError 403 and ResourceNotFoundException httpError 404) evaluates the caller's authorization on the domain BEFORE it resolves the domain or the asset. When the caller is not permitted on the domain, the answer is HTTP 403 with header x-amzn-errortype: AccessDeniedException and message `User is not permitted to perform operation: DeleteAsset`. In that message the fixed text is `User is not permitted to perform operation: ` and the only variable part is the operation name of the request (here DeleteAsset); nothing is taken from the request path, so the domain id and asset id of the recording are not part of the template. Status/code/message taken from the recorded response.

Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and message 'Domain not found' — it resolves the domain in state first and reports non-existence instead of denying the call.

Why: a delete that is actually forbidden is reported as "already gone". Idempotent-delete and cleanup logic treats 404 on DELETE as success, so a client relying on AWS's behavior silently loses the permissions signal and believes an asset it may not touch has been removed; code that branches on AccessDeniedException (re-auth, role escalation, surfacing a policy error) never runs, and the 404 is additionally indistinguishable from a genuinely missing asset.

Fix: datazone's domain-scoped authorization pre-check must run ahead of the DeleteAsset handler's state lookups, and must construct AccessDeniedException (403, x-amzn-errortype AccessDeniedException) with AWS's message template rather than falling through to the ResourceNotFoundException path. This is the same ordering defect already reported for the sibling delete/list/project operations in open issue 20261029T172524Z-datazone-domain-scoped-delete-list-and-project-operations.md, which does NOT carry DeleteAsset's replay key (datazone-domain-scoped-denied-sweep-9#task#2) and therefore does not cover this operation; fix them together if convenient, but this cause must be closed on its own replay.

Traces: datazone-domain-scoped-denied-sweep-9#task#2 (DeleteAsset).

## Evidence
Replay report, fixture datazone-domain-scoped-denied-sweep-9 step task#2, operation DeleteAsset, verdict mismatch:
- status: expected 403, got 404
- $.message: expected 'User is not permitted to perform operation: DeleteAsset', got 'Domain not found'
- $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'ResourceNotFoundException'
Smithy model (services/service-com-amazonaws-datazone-datazone/smithy-model/com-amazonaws-datazone-datazone.json): DeleteAsset binds AccessDeniedException (httpError 403) and ResourceNotFoundException (httpError 404); success is HTTP 204.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 against the emulator and inspect fixture datazone-domain-scoped-denied-sweep-9, step task#2: DELETE /v2/domains/{domainIdentifier}/assets/{identifier} issued by a caller that is not permitted on the domain. Expected: 403, x-amzn-errortype AccessDeniedException, message 'User is not permitted to perform operation: DeleteAsset'. Actual: 404, x-amzn-errortype ResourceNotFoundException, message 'Domain not found'.
