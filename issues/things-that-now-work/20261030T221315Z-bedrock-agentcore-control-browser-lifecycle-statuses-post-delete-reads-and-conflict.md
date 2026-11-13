Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-browser-duplicate-name-conflict
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-browser-duplicate-name-conflict bedrock-agentcore-control-browser-lifecycle bedrock-agentcore-control-multi-browser-isolation
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateBrowser|2xx->2xx|$.status
Issue-Key: bedrock-agentcore-control|CreateBrowser|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|DeleteBrowser|2xx->2xx|$.status
Issue-Key: bedrock-agentcore-control|GetBrowser|2xx->2xx|$.status
Issue-Key: bedrock-agentcore-control|GetBrowser|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|GetBrowser|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetBrowser|4xx->2xx|status
Issue-Commit: 468023723f0688d7e48e7cd568bb3e02439c9d0a

# Browser lifecycle statuses, post-delete reads and conflict message diverge from AWS

> _Note: this issue was identified against the code base at commit `468023723f0688d7e48e7cd568bb3e02439c9d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBrowser

## Description
The emulator's custom-browser surface (CreateBrowser / GetBrowser / DeleteBrowser) models browser state as a permanently pending resource: it reports the transient status for every read, never settles the resource, never removes it on delete, and words its ConflictException differently from AWS. This bundle carries 5 CAUSES; all five must be closed before submitting.

Cause 1 of 5: CreateBrowser reports CREATING instead of READY.
AWS: Smithy: CreateBrowser http 202 PUT /browsers; output member `status` targets enum BrowserStatus (values CREATING, CREATE_FAILED, READY, DELETING, DELETE_FAILED, DELETED) and is @required. Recorded behavior: a CreateBrowser of a custom browser answers 202 with status READY — a browser created from a network configuration alone is serviceable immediately, so AWS does not park it in the transient CREATING state.
Emulator: answers 202 with $.status = 'CREATING'.
Why: a client that creates a browser and waits for status READY before calling StartBrowserSession (the SDK waiter pattern) never observes readiness and spins until its own timeout; code that branches on CREATING takes the "still provisioning" path forever.
Fix: the CreateBrowser handler's state transition — a created browser must be stored in, and serialized with, its settled status rather than the transient one.
Traces: bedrock-agentcore-control-browser-duplicate-name-conflict#setup#1 (cluster also covers bedrock-agentcore-control-browser-lifecycle#setup#1, bedrock-agentcore-control-multi-browser-isolation#setup#1, bedrock-agentcore-control-multi-browser-isolation#setup#2).

Cause 2 of 5: CreateBrowser duplicate-name ConflictException carries the wrong message.
AWS: Smithy: CreateBrowser error ConflictException (client, httpError 409, member `message` targets NonBlankString); doc: "This exception is thrown when there is a conflict performing an operation". Recorded behavior: 409 with message "Browser with name '<name>' already exists in this account". Fixed text: "Browser with name '" ... "' already exists in this account"; the only variable part is <name>, taken verbatim from the request's `name` member (recorded run: sim2real_probe_browserdup_6d8c19f2). AWS does NOT disclose the existing browserId in this message.
Emulator: 409 with message "a browser named <name> already exists (browserId <name>-<suffix>)" — lower-cased prose, no quoting of the name, and an extra parenthetical carrying the colliding browserId.
Why: the status and error code match but the text does not, so clients and tests that match on the AWS message (or surface it to users) mis-handle the collision, and the emulator leaks an identifier AWS keeps out of the error.
Fix: the CreateBrowser handler's conflict/validation path — emit AWS's ConflictException message template.
Traces: bedrock-agentcore-control-browser-duplicate-name-conflict#task#2.

Cause 3 of 5: DeleteBrowser reports DELETING instead of DELETED.
AWS: Smithy: DeleteBrowser http 202 DELETE /browsers/{browserId}; output `status` @required, enum BrowserStatus. Recorded behavior: 202 with status DELETED — deletion of a custom browser completes synchronously.
Emulator: answers 202 with $.status = 'DELETING'.
Why: a client that deletes a browser and waits for DELETED before releasing its name or finishing teardown never sees completion; idempotent re-delete logic keyed on DELETING loops.
Fix: the DeleteBrowser handler's state transition — settle the record to its terminal status and serialize that.
Traces: bedrock-agentcore-control-browser-duplicate-name-conflict#teardown#3 (cluster also covers bedrock-agentcore-control-browser-lifecycle#teardown#3, bedrock-agentcore-control-multi-browser-isolation#task#5, bedrock-agentcore-control-multi-browser-isolation#teardown#8).

Cause 4 of 5: GetBrowser reports CREATING for a created browser instead of READY.
AWS: Smithy: GetBrowser http 200 GET /browsers/{browserId}; output `status` @required, enum BrowserStatus. Recorded behavior: reading back a just-created custom browser answers 200 with status READY.
Emulator: answers 200 with $.status = 'CREATING'.
Why: this is the read a waiter polls, so the readiness poll never terminates even though the resource is fully usable; it is the same unsettled state as Cause 1 observed through the read path, and must be fixed in the state the read serves, not patched in the create response alone.
Fix: the GetBrowser handler's projection of stored browser state (and the stored status it reads).
Traces: bedrock-agentcore-control-browser-lifecycle#task#2 (cluster also covers bedrock-agentcore-control-multi-browser-isolation#task#3, bedrock-agentcore-control-multi-browser-isolation#task#4).

Cause 5 of 5: GetBrowser on a DELETED browser answers 200 with the browser instead of 404 ResourceNotFoundException.
AWS: Smithy: GetBrowser errors include ResourceNotFoundException (client, httpError 404, member `message` targets NonBlankString); doc: "This exception is thrown when a resource referenced by the operation does not exist". Recorded behavior: after DeleteBrowser, GetBrowser answers 404 with header x-amzn-errortype: ResourceNotFoundException and message "Browser with ID <browserId> does not exist" — fixed text "Browser with ID " ... " does not exist", the single variable part being the <browserId> from the request URI label (recorded run: sim2real_probe_browser_7b492160-zmhk9yn8m9).
Emulator: answers 200 with the full browser body; $.message and the x-amzn-errortype header are absent.
Why: a deleted browser stays visible, so teardown verification, delete-then-recreate flows and any client that treats a successful GET as existence proof all read stale state and never see the resource disappear.
Fix: the DeleteBrowser/GetBrowser state layer — deletion must remove the browser (or mark it unreadable) so the read path raises ResourceNotFoundException with the 404 status, the x-amzn-errortype header and AWS's message text.
Traces: bedrock-agentcore-control-browser-lifecycle#teardown#4.

Not covered by existing open issues: the open issue "bedrock-agentcore-control: the service-provided built-in browser aws.browser.v1 is absent, so GetBrowser answers 404 instead of a READY built-in" (20261030T215654Z-bedrock-agentcore-control-the-service-provided-built-in.md) concerns the missing built-in browser aws.browser.v1, the opposite direction of Cause 5 and unrelated to customer-created browser state; the GetBrowser/ListBrowsers stub issues predate the handler now answering 200.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

Cause 1 — CreateBrowser, expected 202 / actual 202: "$.status: value_mismatch (expected 'READY', got 'CREATING')"; fixture bedrock-agentcore-control-browser-duplicate-name-conflict step setup#1; 4 affected steps.

Cause 2 — CreateBrowser, expected 409 / actual 409: "$.message: value_mismatch (expected \"Browser with name 'sim2real_probe_browserdup_6d8c19f2' already exists in this account\", got 'a browser named sim2real_probe_browserdup_6d8c19f2 already exists (browserId sim2real_probe_browserdup_6d8c19f2-rixr205czp)')"; fixture bedrock-agentcore-control-browser-duplicate-name-conflict step task#2.

Cause 3 — DeleteBrowser, expected 202 / actual 202: "$.status: value_mismatch (expected 'DELETED', got 'DELETING')"; fixture bedrock-agentcore-control-browser-duplicate-name-conflict step teardown#3; 4 affected steps.

Cause 4 — GetBrowser, expected 200 / actual 200: "$.status: value_mismatch (expected 'READY', got 'CREATING')"; fixture bedrock-agentcore-control-browser-lifecycle step task#2; 3 affected steps.

Cause 5 — GetBrowser, expected 404 / actual 200: "status: expected 404, got 200; $.message: missing (expected 'Browser with ID sim2real_probe_browser_7b492160-zmhk9yn8m9 does not exist', got None); $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException', got None)"; fixture bedrock-agentcore-control-browser-lifecycle step teardown#4.

Model grounding (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json): BrowserStatus enum = [CREATING, CREATE_FAILED, READY, DELETING, DELETE_FAILED, DELETED]; CreateBrowser 202 PUT /browsers with @required status and ConflictException(409); DeleteBrowser 202 DELETE /browsers/{browserId} with @required status, ConflictException(409), ResourceNotFoundException(404); GetBrowser 200 GET /browsers/{browserId} with @required status and ResourceNotFoundException(404). The recorded names, ids and suffixes above are per-run values, not requirements.

## Steps to Reproduce
1. git clone the repo and boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: fixture bedrock-agentcore-control-browser-duplicate-name-conflict, step setup#1 — PUT /browsers with a name and networkConfiguration; response status field is CREATING, expected READY.
4. Cause 2: same fixture, step task#2 — PUT /browsers again with the same name; 409 body message text differs from AWS's "Browser with name '<name>' already exists in this account".
5. Cause 3: same fixture, step teardown#3 — DELETE /browsers/{browserId}; response status field is DELETING, expected DELETED.
6. Cause 4: fixture bedrock-agentcore-control-browser-lifecycle, step task#2 — GET /browsers/{browserId} for the created browser; status is CREATING, expected READY.
7. Cause 5: fixture bedrock-agentcore-control-browser-lifecycle, step teardown#4 — after DELETE, GET /browsers/{browserId}; emulator answers 200 with the browser body, expected 404 ResourceNotFoundException with x-amzn-errortype header and message "Browser with ID <browserId> does not exist".
