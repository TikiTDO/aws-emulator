Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-message-template-attachment-render
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-message-template-attachment-render qconnect-message-template-lifecycle qconnect-message-template-versions
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|CreateMessageTemplate|2xx->2xx|$.messageTemplate.attributeTypes
Issue-Key: wisdom|CreateMessageTemplate|2xx->2xx|$.messageTemplate.channel
Issue-Key: wisdom|CreateMessageTemplate|2xx->2xx|$.messageTemplate.createdTime
Issue-Key: wisdom|CreateMessageTemplate|2xx->2xx|$.messageTemplate.language
Issue-Key: wisdom|CreateMessageTemplate|2xx->2xx|$.messageTemplate.lastModifiedTime
Issue-Key: wisdom|GetMessageTemplate|2xx->2xx|$.messageTemplate.attributeTypes
Issue-Key: wisdom|GetMessageTemplate|2xx->2xx|$.messageTemplate.channel
Issue-Key: wisdom|GetMessageTemplate|2xx->2xx|$.messageTemplate.createdTime
Issue-Key: wisdom|GetMessageTemplate|2xx->2xx|$.messageTemplate.language
Issue-Key: wisdom|GetMessageTemplate|2xx->2xx|$.messageTemplate.lastModifiedTime
Issue-Key: wisdom|GetMessageTemplate|2xx->2xx|$.messageTemplate.content.email.body.plainText.content
Issue-Key: wisdom|GetMessageTemplate|2xx->2xx|$.messageTemplate.content.email.subject
Issue-Key: wisdom|GetMessageTemplate|2xx->2xx|$.messageTemplate.description
Issue-Commit: 64434ac11a54835f0dc0d25437542d6f3223788c

# QConnect message-template responses omit modeled members, emit epoch timestamps and lose updates

> _Note: this issue was identified against the code base at commit `64434ac11a54835f0dc0d25437542d6f3223788c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMessageTemplate

## Description
The emulator's QConnect (wisdom) message-template path serializes an incomplete MessageTemplateData/ExtendedMessageTemplateData structure, writes timestamps in the wrong wire form, and never applies UpdateMessageTemplate mutations to the stored template. This bundle carries 3 causes.

Cause 1 of 3 - CreateMessageTemplate response drops channel/language/attributeTypes and serializes timestamps as epoch numbers (fixture qconnect-message-template-attachment-render, step setup#3; same cluster also hits qconnect-message-template-lifecycle#task#3 and qconnect-message-template-versions#task#3).
AWS: Smithy (com.amazonaws.qconnect) CreateMessageTemplate -> CreateMessageTemplateResponse.messageTemplate : MessageTemplateData. MessageTemplateData declares `channel` (target Channel, @length 1..10, @sensitive), `channelSubtype` (required, ChannelSubtype enum EMAIL|SMS|WHATSAPP|PUSH), `language` (LanguageCode, @length 2..5), `attributeTypes` (MessageTemplateAttributeTypeList, @uniqueItems, members SYSTEM|AGENT|CUSTOMER_PROFILE|CUSTOM), and `createdTime`/`lastModifiedTime` as required Timestamp members carrying @timestampFormat("date-time"). So AWS returns `channel` alongside the requested channelSubtype, echoes the request's `language`, returns `attributeTypes` (an empty list when the content declares no attributes), and renders both timestamps as RFC 3339 / ISO-8601 date-time strings such as "2026-09-06T19:23:22.468Z" - the instant comes from the request's processing time, only the FORMAT is the requirement.
Emulator: status 200, but `$.messageTemplate.channel` is absent (expected 'EMAIL'), `$.messageTemplate.language` is absent (expected 'en_US', the value sent on the request), `$.messageTemplate.attributeTypes` is absent (expected []), and `$.messageTemplate.createdTime` / `$.messageTemplate.lastModifiedTime` are emitted as bare epoch-seconds floats (e.g. 1794399117.6518328) instead of a date-time string.
Why: an SDK client deserializing createdTime/lastModifiedTime against @timestampFormat("date-time") either fails to parse the number or silently yields a wrong instant, so any client that sorts, displays or compares template timestamps breaks. A client that reads `channel` to pick a rendering pipeline, or `language` to localize, sees null for a template that was created with those values, and code that iterates `attributeTypes` to discover which attribute categories must be supplied at render time sees null instead of a (possibly empty) list and cannot distinguish "no attributes" from "not returned".
Fix: the CreateMessageTemplate handler's response serializer for MessageTemplateData in the qconnect/wisdom service crate - populate channel (derived from the resolved channelSubtype), language (echoed from the request), and attributeTypes (empty list when content declares no attributes), and route both timestamp members through the date-time timestamp serializer rather than emitting the raw epoch value.
Traces: qconnect-message-template-attachment-render#setup#3, plus qconnect-message-template-lifecycle#task#3 and qconnect-message-template-versions#task#3 in the same cluster.

Cause 2 of 3 - GetMessageTemplate response drops the same members and emits the same epoch timestamps (fixture qconnect-message-template-lifecycle, step task#4).
AWS: Smithy GetMessageTemplate -> GetMessageTemplateResponse.messageTemplate : ExtendedMessageTemplateData, which declares the identical `channel`, `language`, `attributeTypes` members and the identical required `createdTime`/`lastModifiedTime` Timestamp members with @timestampFormat("date-time"). A read-back of a freshly created template therefore returns the create-time values in those members, with both timestamps as date-time strings.
Emulator: status 200 with `$.messageTemplate.channel` missing (expected 'EMAIL'), `$.messageTemplate.language` missing (expected 'en_US'), `$.messageTemplate.attributeTypes` missing (expected []), and createdTime/lastModifiedTime again epoch floats (e.g. 1794399119.3603733) where the recording holds an ISO-8601 date-time string.
Why: the read path is the one clients poll after create/update, so the parse failure and the null members are hit by every describe/list-then-get flow, not just by the create call. A round-trip through the emulator loses the channel and language a template was created with.
Fix: the GetMessageTemplate handler's response serializer for ExtendedMessageTemplateData - same two defects as Cause 1 (member population and date-time timestamp formatting); the fix most likely belongs in one shared MessageTemplateData/ExtendedMessageTemplateData serializer used by both operations.
Traces: qconnect-message-template-lifecycle#task#4.

Cause 3 of 3 - UpdateMessageTemplate mutations are not persisted; GetMessageTemplate keeps serving the create-time snapshot (fixture qconnect-message-template-lifecycle, step task#8).
AWS: Smithy UpdateMessageTemplate (POST /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId}) accepts `content` (MessageTemplateContentProvider), `language`, `sourceConfiguration` and `defaultAttributes` and returns the updated MessageTemplateData; MessageTemplateData.lastModifiedTime is documented as "The timestamp when the message template data was last modified." A subsequent GetMessageTemplate therefore reflects the updated content (email subject and plainText body), the updated description where the lifecycle updated it, and a lastModifiedTime strictly later than createdTime. Recorded behavior confirms it: after the update the fixture reads back the new subject, the new plain-text body, the new description, and a lastModifiedTime distinct from createdTime.
Emulator: status 200 but the pre-update state is returned - `$.messageTemplate.content.email.subject` is 'Your order has shipped' where the recording has 'Your order has shipped!', `$.messageTemplate.content.email.body.plainText.content` is 'Hi, your order is on its way.' where the recording has 'Hi, great news -- your order shipped today.', `$.messageTemplate.description` is 'Order confirmation email' where the recording has 'Order shipped notification email', and lastModifiedTime equals createdTime (identical epoch float 1794399119.3603733) instead of advancing. The literal strings here are this recording's values, not requirements: the requirement is that the read-back equals the values most recently written by the update.
Why: a client that updates a template and re-reads it (or that renders from the stored template) keeps getting the superseded content indefinitely, and lastModifiedTime never advances, so change detection and optimistic-concurrency checks based on it never fire. Silent data loss on write is worse than an error: the update returns 200, so nothing signals the failure.
Fix: the UpdateMessageTemplate handler's state mutation in the qconnect/wisdom service crate - apply the request's content/language/defaultAttributes (and description where supplied) to the stored template record and stamp lastModifiedTime at the time of the update, so the read path observes them. Related but NOT covering: the open issue "QConnect GetAIAgent read path ignores AI Agent lifecycle mutations and serves the create-time snapshot" (20261031T001959Z-wisdom-qconnect-getaiagent-read-path-ignores-ai-agent-lifecycle.md) reports the same class of stale-read defect for AI agents, a different resource and different handlers.
Traces: qconnect-message-template-lifecycle#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, score 56.1%).

Cause 1 - fixture qconnect-message-template-attachment-render step setup#3, CreateMessageTemplate, expected 200 / actual 200:
  $.messageTemplate.attributeTypes: missing (expected [], got None)
  $.messageTemplate.channel: missing (expected 'EMAIL', got None)
  $.messageTemplate.language: missing (expected 'en_US', got None)
  $.messageTemplate.createdTime: value_mismatch (expected <date-time string>, got 1794399117.6518328)
  $.messageTemplate.lastModifiedTime: value_mismatch (expected <date-time string>, got 1794399117.6518328)
  affected (3): qconnect-message-template-attachment-render#setup#3, qconnect-message-template-lifecycle#task#3, qconnect-message-template-versions#task#3

Cause 2 - fixture qconnect-message-template-lifecycle step task#4, GetMessageTemplate, expected 200 / actual 200:
  $.messageTemplate.attributeTypes: missing (expected [], got None)
  $.messageTemplate.channel: missing (expected 'EMAIL', got None)
  $.messageTemplate.language: missing (expected 'en_US', got None)
  $.messageTemplate.createdTime: value_mismatch (expected '2026-09-06T19:23:22.468Z', got 1794399119.3603733)
  $.messageTemplate.lastModifiedTime: value_mismatch (expected '2026-09-06T19:23:22.468Z', got 1794399119.3603733)

Cause 3 - fixture qconnect-message-template-lifecycle step task#8, GetMessageTemplate (after UpdateMessageTemplate), expected 200 / actual 200:
  $.messageTemplate.content.email.subject: value_mismatch (expected 'Your order has shipped!', got 'Your order has shipped')
  $.messageTemplate.content.email.body.plainText.content: value_mismatch (expected 'Hi, great news -- your order shipped today.', got 'Hi, your order is on its way.')
  $.messageTemplate.description: value_mismatch (expected 'Order shipped notification email', got 'Order confirmation email')
  $.messageTemplate.lastModifiedTime: value_mismatch (expected '2026-09-06T19:23:22.874Z', got 1794399119.3603733 - unchanged from createdTime)
  $.messageTemplate.attributeTypes / channel / language: missing, as in Causes 1 and 2

Smithy consulted: com.amazonaws.qconnect CreateMessageTemplate / GetMessageTemplate / UpdateMessageTemplate; shapes MessageTemplateData, ExtendedMessageTemplateData (members channel, channelSubtype, language, attributeTypes, createdTime/lastModifiedTime with @timestampFormat("date-time")), ChannelSubtype (EMAIL|SMS|WHATSAPP|PUSH), MessageTemplateAttributeType (SYSTEM|AGENT|CUSTOMER_PROFILE|CUSTOM).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture qconnect-message-template-attachment-render, step setup#3 - POST /knowledgeBases/{knowledgeBaseId}/messageTemplates with channelSubtype=EMAIL, language=en_US and email content. Inspect the response messageTemplate: channel, language and attributeTypes are absent and createdTime/lastModifiedTime are epoch numbers rather than date-time strings.
3. Cause 2: replay fixture qconnect-message-template-lifecycle, step task#4 - GET /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId} for the template just created. The same three members are absent and the same two timestamps are epoch numbers.
4. Cause 3: within fixture qconnect-message-template-lifecycle, POST the UpdateMessageTemplate at the step preceding task#8 (new email subject, new plainText body, new description), then replay step task#8's GetMessageTemplate. The response still carries the create-time subject, body and description and a lastModifiedTime identical to createdTime.
