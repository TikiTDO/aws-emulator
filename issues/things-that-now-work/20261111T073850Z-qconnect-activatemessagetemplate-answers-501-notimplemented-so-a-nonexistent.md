Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: qconnect.activate-nonexistent-template-version-rejected
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: qconnect.activate-nonexistent-template-version-rejected|run#7|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException for a nonexistent version, got {code}"
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# qconnect ActivateMessageTemplate answers 501 NotImplemented, so a nonexistent message-template version is never refused with ResourceNotFoundException (1 cause)

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateMessageTemplate

## Description
One net-new root cause in the qconnect (com.amazonaws.qconnect / WisdomService) emulator, grounded in one failing behavioral check. The replay report did NOT auto-file it and no open issue owns it: the auto-filed stub issues for this service name CreateAIPrompt, CreateMessageTemplateVersion, CreateMessageTemplateAttachment and RenderMessageTemplate, and the open issue 20261103T173243Z-qconnect-ai-agent-version-surface-answers-501-notimplemented.md covers the AI *Agent* version surface plus UpdateContent only — it says nothing about the message-template activation plane.

--- Cause 1 of 1: ActivateMessageTemplate is an unimplemented scaffold stub, so it answers 501 NotImplemented for every request, including one AWS refuses with 404 ResourceNotFoundException ---
AWS: Smithy: ActivateMessageTemplate, POST /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId}/activate, success 200, input {knowledgeBaseId @required @httpLabel, messageTemplateId @required @httpLabel, versionNumber: Version @required @range min 1}, output {messageTemplateArn @required, messageTemplateId @required, versionNumber @required}; modeled errors AccessDeniedException(403), ConflictException(409), ResourceNotFoundException(404), ThrottlingException(400), ValidationException(400). Doc: "Activates a specific version of the Amazon Q in Connect message template. After the version is activated, the previous active version will be deactivated automatically." For a versionNumber that was never created on an existing template, AWS refuses with HTTP 404, Error.Code=ResourceNotFoundException and the recorded message "Message template version does not exist." — that sentence is fixed text for this error class, with no variable part (the ids are not interpolated into it). The refusal must also create nothing: ListMessageTemplateVersions for that template still returns an empty messageTemplateVersionSummaries. 501/NotImplemented is not in the modeled error set at all.
Emulator: every ActivateMessageTemplate call answers HTTP 501 with Error.Code=`NotImplemented` (message of the form "operation ActivateMessageTemplate is scaffolded but not implemented in this emulator"). The handler is still the generated scaffold: `Err(crate::scaffolded::Scaffolded::not_implemented("ActivateMessageTemplate"))`, and the file header still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker. The check's assertion reads `AssertionError: expected ResourceNotFoundException for a nonexistent version, got NotImplemented`.
Why: a client cannot distinguish "this template version does not exist" (a 404 it should handle by creating the version) from "the service is broken": it gets an unmodeled 501 that botocore surfaces as a non-retryable NotImplemented outside the operation's modeled error union, so `except qc.exceptions.ResourceNotFoundException` never matches. Positively, no message-template version can ever be activated, so the $ACTIVE_VERSION qualifier and the implicit deactivation of the previously active version are unreachable, and `isActive` can never become true on any template version.
Fix: the operation's handler, services/service-com-amazonaws-qconnect-wisdomservice/src/ops/activate_message_template.rs — resolve (knowledgeBaseId, messageTemplateId, versionNumber) against the message-template version records in crate::state; refuse an unknown version with ResourceNotFoundException (404, message "Message template version does not exist.") while mutating nothing; on success mark that version active, clear the active flag on the previously active version, and return messageTemplateArn/messageTemplateId/versionNumber. Delete the SCAFFOLDED marker line once implemented. Note the success path additionally needs the already-filed CreateMessageTemplateVersion stub (services/.../src/ops/create_message_template_version.rs) to exist, but the 404 refusal path is entirely this handler's.
Traces: qconnect.activate-nonexistent-template-version-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, qconnect 15/25 passed (60%), 10 failed. Every other failing qconnect check in the report is marked ALREADY FILED (blocked by the CreateAIPrompt / CreateMessageTemplateVersion / CreateMessageTemplateAttachment / RenderMessageTemplate stubs) or OWNED by 20261103T173243Z-qconnect-ai-agent-version-surface-answers-501-notimplemented.md, and all 5 cloudformation__qconnect failures are OWNED by 20261109T223435Z-cloudformation-qconnect-cloudformation-does-not-register-any-aws.md; this check is the only unowned one.

  [mismatch] qconnect.activate-nonexistent-template-version-rejected
    check_qconnect_activate_nonexistent_template_version_rejected.py, line 40, in run
    assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException for a nonexistent version, got {code}"
    AssertionError: expected ResourceNotFoundException for a nonexistent version, got NotImplemented

    Expected: ActivateMessageTemplate(knowledgeBaseId=<kb>, messageTemplateId=<tmpl>, versionNumber=1) on a template that has no versions yet
              -> HTTP 404, Error.Code="ResourceNotFoundException", Error.Message="Message template version does not exist.";
              then ListMessageTemplateVersions(<kb>, <tmpl>) -> messageTemplateVersionSummaries == [] (the refusal created nothing);
              then, after CreateMessageTemplateVersion, ActivateMessageTemplate(versionNumber=<that version>) -> HTTP 200 and
              GetMessageTemplate(messageTemplateId="<tmpl>:<version>") reports isActive == true.
    Actual:   HTTP 501, Error.Code="NotImplemented" for the refusal case (and for every other case).

  Emulator source confirming the cause:
    services/service-com-amazonaws-qconnect-wisdomservice/src/ops/activate_message_template.rs:4   `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented``
    services/service-com-amazonaws-qconnect-wisdomservice/src/ops/activate_message_template.rs:181 `Err(crate::scaffolded::Scaffolded::not_implemented("ActivateMessageTemplate"))`

  Smithy (services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json):
    ActivateMessageTemplate errors = AccessDeniedException(403), ConflictException(409), ResourceNotFoundException(404), ThrottlingException(400), ValidationException(400) — no NotImplemented.
    ListMessageTemplateVersions output.messageTemplateVersionSummaries is @required, so an empty list is the correct "no versions" answer.

  Note: the preceding steps of this check PASS — CreateKnowledgeBase(MESSAGE_TEMPLATES) reaches ACTIVE and CreateMessageTemplate returns a messageTemplateId — so the failure is isolated to the activation operation.

## Steps to Reproduce
1. qconnect CreateKnowledgeBase(name=<unique>, knowledgeBaseType="MESSAGE_TEMPLATES"); poll GetKnowledgeBase until status == ACTIVE.
2. CreateMessageTemplate(knowledgeBaseId=<kb>, name=<unique>, channelSubtype="SMS", content={"sms": {"body": {"plainText": {"content": "No version created yet"}}}}); record messageTemplateId=<tmpl>.
3. ActivateMessageTemplate(knowledgeBaseId=<kb>, messageTemplateId=<tmpl>, versionNumber=1) — no version has been created. Observe HTTP 501 Error.Code="NotImplemented" instead of HTTP 404 Error.Code="ResourceNotFoundException" with Error.Message="Message template version does not exist.".
4. ListMessageTemplateVersions(knowledgeBaseId=<kb>, messageTemplateId=<tmpl>) — messageTemplateVersionSummaries must still be [] (the rejected activation must not create a version).
5. Positive control: CreateMessageTemplateVersion(knowledgeBaseId=<kb>, messageTemplateId=<tmpl>) -> versionNumber=<v>; ActivateMessageTemplate(knowledgeBaseId=<kb>, messageTemplateId=<tmpl>, versionNumber=<v>) must answer HTTP 200 with messageTemplateArn/messageTemplateId/versionNumber, and GetMessageTemplate(knowledgeBaseId=<kb>, messageTemplateId="<tmpl>:<v>") must report isActive == true.
