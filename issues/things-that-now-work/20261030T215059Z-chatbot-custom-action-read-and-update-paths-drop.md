Issue-Source: trace-sweep-divergence
Issue-Trace: chatbot-custom-action-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chatbot-custom-action-lifecycle chatbot-resource-not-found-error-paths
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|GetCustomAction|2xx->2xx|$.CustomAction.AliasName
Issue-Key: chatbot|GetCustomAction|2xx->2xx|$.CustomAction.Attachments
Issue-Key: chatbot|UpdateCustomAction|4xx->4xx|$.Message
Issue-Key: chatbot|UpdateCustomAction|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chatbot|UpdateCustomAction|4xx->4xx|status
Issue-Commit: 11c4079474be1ad931c1a1cc92224a1a5bcf318e

# Chatbot custom-action read and update paths drop fields and misclassify missing actions

> _Note: this issue was identified against the code base at commit `11c4079474be1ad931c1a1cc92224a1a5bcf318e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCustomAction

## Description
The chatbot emulator's custom-action handlers diverge from AWS in two ways: the read path does not return the alias and attachment members that the create/update path accepted, and the update path answers a request addressing a non-existent custom action as an authorization failure instead of the modeled not-found error. Two causes, both in the chatbot service.

Cause 1 of 2: GetCustomAction omits CustomAction.AliasName and CustomAction.Attachments.
AWS: Smithy (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/smithy-model/com-amazonaws-chatbot-wheatleyorchestration-20171011.json): GetCustomAction (POST /get-custom-action, http code 200) returns output member CustomAction targeting structure CustomAction, whose members are CustomActionArn (required), Definition (required), AliasName (CustomActionAliasName, @length 1..30, pattern ^[A-Za-z0-9-_]+$, "The name used to invoke this action in the chat channel"), Attachments (CustomActionAttachmentList of CustomActionAttachment {NotificationType, ButtonText, Criteria, Variables}, "Defines when this custom action button should be attached to a notification") and ActionName. CreateCustomAction and UpdateCustomAction both accept AliasName and Attachments, so AWS persists them on the action and the read returns exactly what was supplied: the recorded 200 response carries CustomAction.AliasName equal to the alias given at create time and CustomAction.Attachments equal to the attachment list given at create time (one entry with its ButtonText, its Criteria list of {Operator, Value, VariableName} and its NotificationType).
Emulator: 200, but the CustomAction object contains only CustomActionArn, Definition and ActionName - AliasName and Attachments are absent (serialized as None/missing), i.e. the values accepted by CreateCustomAction are never stored or never emitted.
Why: a client that creates an action with an alias and notification-button attachments and then reads it back sees an action with no alias and no buttons. Read-modify-write reconciliation (get, change one field, update) silently erases the alias and the attachment definitions because they are missing from the read, and anything that discovers the chat-channel invocation name (`@aws run <alias>`) from the API finds nothing.
Fix: the chatbot service crate's CreateCustomAction/UpdateCustomAction handlers must persist AliasName and Attachments into the custom-action state record, and the GetCustomAction handler/serializer must emit both members from that state.
Traces: chatbot-custom-action-lifecycle#task#5.

Cause 2 of 2: UpdateCustomAction on a custom-action ARN that does not exist returns 403 UnauthorizedException instead of 404 ResourceNotFoundException.
AWS: Smithy: UpdateCustomAction (POST /update-custom-action) models errors InternalServiceError (500), InvalidRequestException (400), ResourceNotFoundException (errorKind client, httpError 404, member `Message`) and UnauthorizedException (403, for authorization failures). Addressing an absent custom action is the ResourceNotFoundException case: the recorded response is 404 with x-amzn-errortype ResourceNotFoundException and body member `Message` reading `Sdk action with id [<ActionName>] does not exist in account [<accountId>]` - the fixed text is `Sdk action with id [` ... `] does not exist in account [` ... `]`, with the action name taken from the CustomActionArn in the request and the account id from the caller's account. Note AWS words this differently from GetCustomAction's not-found text (`sdk action [<ActionName>] not found for account [<accountId>]`), so one shared string cannot satisfy both operations.
Emulator: 403 with x-amzn-errortype UnauthorizedException and no `Message` member at all.
Why: an SDK client maps 403/UnauthorizedException to a credentials or permissions problem, not to a missing resource. Code that implements update-or-create by catching ResourceNotFoundException never sees it and aborts with what looks like an IAM failure; operators chase a non-existent permissions problem instead of a missing action.
Fix: the UpdateCustomAction handler's resource-lookup path in the chatbot service crate must raise the modeled ResourceNotFoundException when the ARN resolves to nothing, and the error serializer must emit status 404, the ResourceNotFoundException errortype header and the `Message` member formatted as above.
Traces: chatbot-resource-not-found-error-paths#task#3.

Related open issues (neither covers these two causes): 20261029T170844Z-chatbot-not-found-and-conflict-errors-use-wrong.md covers the same 403-instead-of-404 class for GetCustomAction, ListTagsForResource and TagResource but not UpdateCustomAction; 20261030T111746Z-chatbot-untagresource-on-a-missing-resource-returns-403.md covers UntagResource; 20261029T170844Z-chatbot-success-responses-diverge-from-recorded-aws-wire.md covers other success-shape divergences but not the dropped CustomAction members.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (chatbot, 8 fixtures, emulator_up=true):

1) chatbot-custom-action-lifecycle#task#5 GetCustomAction - 200 on both sides, body diverges:
   $.CustomAction.AliasName: missing (expected the alias supplied at create time, got None)
   $.CustomAction.Attachments: missing (expected a one-element attachment list [{ButtonText, Criteria: [{Operator, Value, VariableName}], NotificationType, Variables}], got None)
   The emulator's CustomAction object contained only CustomActionArn, Definition.CommandText and ActionName.

2) chatbot-resource-not-found-error-paths#task#3 UpdateCustomAction - status: expected 404, got 403;
   $.Message: missing (expected 'Sdk action with id [<ActionName>] does not exist in account [<accountId>]', got None);
   $headers.x-amzn-errortype: expected 'ResourceNotFoundException', got 'UnauthorizedException'.

Model grounding (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/smithy-model/com-amazonaws-chatbot-wheatleyorchestration-20171011.json): structure CustomAction members = CustomActionArn (required), Definition (required), AliasName, Attachments, ActionName; UpdateCustomAction input accepts AliasName and Attachments and its errors include ResourceNotFoundException (client, httpError 404, member `Message`).

## Steps to Reproduce
1. Clone this checkout, boot the emulator and replay trace set simparity-wire-aws-v1.
2. Cause 1 - fixture chatbot-custom-action-lifecycle: step setup#1 calls POST /create-custom-action with an ActionName, a Definition.CommandText, an AliasName and one Attachments entry; step task#5 then calls POST /get-custom-action with the returned CustomActionArn. Observe the 200 body's CustomAction object: AliasName and Attachments are absent although they were supplied at create time.
3. Cause 2 - fixture chatbot-resource-not-found-error-paths: step task#3 calls POST /update-custom-action with a custom-action ARN that was never created (plus a Definition). Observe 403 with x-amzn-errortype UnauthorizedException and no Message member, instead of 404 with x-amzn-errortype ResourceNotFoundException and Message 'Sdk action with id [<ActionName>] does not exist in account [<accountId>]'.
