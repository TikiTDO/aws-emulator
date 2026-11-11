Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-message-template-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-message-template-lifecycle qconnect-message-template-versions
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|UpdateMessageTemplate|2xx->2xx|$.messageTemplate.attributeTypes
Issue-Key: wisdom|UpdateMessageTemplate|2xx->2xx|$.messageTemplate.channel
Issue-Key: wisdom|UpdateMessageTemplate|2xx->2xx|$.messageTemplate.createdTime
Issue-Key: wisdom|UpdateMessageTemplate|2xx->2xx|$.messageTemplate.language
Issue-Key: wisdom|UpdateMessageTemplate|2xx->2xx|$.messageTemplate.lastModifiedTime
Issue-Key: wisdom|CreateMessageTemplateVersion|2xx->2xx|$.messageTemplate.attributeTypes
Issue-Key: wisdom|CreateMessageTemplateVersion|2xx->2xx|$.messageTemplate.channel
Issue-Key: wisdom|CreateMessageTemplateVersion|2xx->2xx|$.messageTemplate.createdTime
Issue-Key: wisdom|CreateMessageTemplateVersion|2xx->2xx|$.messageTemplate.language
Issue-Key: wisdom|CreateMessageTemplateVersion|2xx->2xx|$.messageTemplate.lastModifiedTime
Issue-Key: wisdom|ListMessageTemplateVersions|2xx->2xx|$.messageTemplateVersionSummaries[name="sim2real-probe-qconnect-mtv-055acc0c"].channel
Issue-Commit: 23e4ac40a56978a5a155c920fdf4b093f96724d4

# QConnect message-template write and version paths omit channel/language/attributeTypes and emit epoch timestamps

> _Note: this issue was identified against the code base at commit `23e4ac40a56978a5a155c920fdf4b093f96724d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMessageTemplate

## Description
The emulator's QConnect (wisdom) message-template serializers drop modeled optional members and write Timestamp members in the wrong wire form. The already-open issue "QConnect message-template responses omit modeled members, emit epoch timestamps and lose updates" (20261111T121353Z-wisdom-qconnect-message-template-responses-omit-modeled-members-emit.md) covers only CreateMessageTemplate and GetMessageTemplate; the three causes below are on the UPDATE and VERSION paths (UpdateMessageTemplate, CreateMessageTemplateVersion, ListMessageTemplateVersions), which that issue does not carry replay keys for and therefore does not cover. This bundle carries 3 causes.

Cause 1 of 3 - UpdateMessageTemplate response drops channel/language/attributeTypes and serializes createdTime/lastModifiedTime as epoch numbers (fixture qconnect-message-template-lifecycle, step task#6).
AWS: Smithy (com.amazonaws.qconnect) UpdateMessageTemplate (POST /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId}) -> UpdateMessageTemplateResponse.messageTemplate : MessageTemplateData. MessageTemplateData declares `channel` (target Channel, string @length 1..10, @sensitive), `channelSubtype` (required, ChannelSubtype EMAIL|SMS|WHATSAPP|PUSH), `language` (LanguageCode, @length 2..5), `attributeTypes` (MessageTemplateAttributeTypeList, @uniqueItems), and `createdTime` / `lastModifiedTime` as REQUIRED Timestamp members both carrying @timestampFormat("date-time"). So the update response returns `channel` for the template's channel, echoes the effective `language`, returns `attributeTypes` (an empty list when the content declares no attributes) and renders both timestamps as RFC 3339 / ISO-8601 date-time strings. Only the FORMAT is the requirement: the instants come from the template's creation time and from the time the update was applied, so a recorded literal such as '2026-09-06T19:23:22.874Z' is a filled-in value, not part of the contract.
Emulator: status 200, but `$.messageTemplate.channel` is absent (expected 'EMAIL'), `$.messageTemplate.language` is absent (expected 'en_US'), `$.messageTemplate.attributeTypes` is absent (expected []), and `$.messageTemplate.createdTime` / `$.messageTemplate.lastModifiedTime` are emitted as bare epoch-seconds floats (observed 1794401631.519813 and 1794401631.543926) instead of date-time strings.
Why: an SDK client deserializing these two required members against @timestampFormat("date-time") either fails outright or silently yields a wrong instant, so change detection, sorting and display on the write path break. A client that reads `channel` to select a rendering pipeline or `language` to localize sees null for a template that has both, and code that iterates `attributeTypes` to learn which attribute categories must be supplied at render time cannot distinguish "no attributes" from "not returned".
Fix: the UpdateMessageTemplate handler's response serializer for MessageTemplateData in the qconnect/wisdom service crate - populate channel (from the stored/resolved channelSubtype), language and attributeTypes, and route both timestamp members through the date-time timestamp serializer instead of emitting the raw epoch value. The same MessageTemplateData/ExtendedMessageTemplateData serializer is shared with the create/read paths, so one correction should cover all of them.
Traces: qconnect-message-template-lifecycle#task#6.

Cause 2 of 3 - CreateMessageTemplateVersion response drops the same members and emits the same epoch timestamps (fixture qconnect-message-template-versions, step task#4).
AWS: Smithy CreateMessageTemplateVersion (POST /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId}/versions) -> CreateMessageTemplateVersionResponse.messageTemplate : ExtendedMessageTemplateData. ExtendedMessageTemplateData declares the same `channel`, `language` and `attributeTypes` members and the same REQUIRED `createdTime` / `lastModifiedTime` Timestamp members with @timestampFormat("date-time") (plus attachments/isActive/versionNumber). Snapshotting a template into a version therefore returns the template's channel, its language and its attributeTypes, with both timestamps as date-time strings.
Emulator: status 200 with `$.messageTemplate.channel` missing (expected 'CHAT' for the chat-subtype template under test), `$.messageTemplate.language` missing (expected 'en_US'), `$.messageTemplate.attributeTypes` missing (expected []), and createdTime/lastModifiedTime again epoch floats (observed 1794401630.63035) where a date-time string is required.
Why: the version snapshot is the artifact clients archive and later render from, so a version record that lacks channel and language is unusable for choosing a renderer or a locale, and the unparseable timestamps make version history ordering wrong or fatal to deserialize.
Fix: the CreateMessageTemplateVersion handler's response serializer for ExtendedMessageTemplateData in the qconnect/wisdom service crate - copy channel, language and attributeTypes from the template record being versioned and emit both timestamps via the date-time serializer.
Traces: qconnect-message-template-versions#task#4.

Cause 3 of 3 - ListMessageTemplateVersions summaries omit `channel` (fixture qconnect-message-template-versions, step task#5).
AWS: Smithy ListMessageTemplateVersions (GET /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId}/versions) -> ListMessageTemplateVersionsResponse.messageTemplateVersionSummaries : MessageTemplateVersionSummaryList of MessageTemplateVersionSummary. MessageTemplateVersionSummary declares `channel` (target Channel, @length 1..10, @sensitive) beside the required messageTemplateArn/messageTemplateId/knowledgeBaseArn/knowledgeBaseId/name/channelSubtype/isActive/versionNumber. Each listed version therefore carries the channel of the template it was cut from.
Emulator: status 200, but the summary for the listed version has `channel` absent (expected 'CHAT' for the chat-subtype template under test); every other asserted summary member matches.
Why: a client that lists versions to pick one to render has to know each version's channel from the listing; with the member absent it must fall back to a per-version Get (or guess), and any UI or routing decision keyed on the summary's channel silently degrades.
Fix: the ListMessageTemplateVersions handler's serializer for MessageTemplateVersionSummary in the qconnect/wisdom service crate - populate channel for each summary from the stored version record (same derivation as the full data shapes above).
Traces: qconnect-message-template-versions#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up=true, score_pct 57.9, steps_total 114, steps_mismatched 36).

Cause 1 - fixture qconnect-message-template-lifecycle step task#6, UpdateMessageTemplate, expected 200 / actual 200:
  $.messageTemplate.attributeTypes: missing (expected [], got None)
  $.messageTemplate.channel: missing (expected 'EMAIL', got None)
  $.messageTemplate.language: missing (expected 'en_US', got None)
  $.messageTemplate.createdTime: value_mismatch (expected '2026-09-06T19:23:22.468Z', got 1794401631.519813)
  $.messageTemplate.lastModifiedTime: value_mismatch (expected '2026-09-06T19:23:22.874Z', got 1794401631.543926)
  divergence_key: service=wisdom|UpdateMessageTemplate|status=200->200|keys=$.messageTemplate.attributeTypes,$.messageTemplate.channel,$.messageTemplate.createdTime,$.messageTemplate.language,$.messageTemplate.lastModifiedTime
  (the two expected timestamp literals are that recording's instants; the requirement is the ISO-8601 date-time FORM, with lastModifiedTime stamped at the update)

Cause 2 - fixture qconnect-message-template-versions step task#4, CreateMessageTemplateVersion, expected 200 / actual 200:
  $.messageTemplate.attributeTypes: missing (expected [], got None)
  $.messageTemplate.channel: missing (expected 'CHAT', got None)
  $.messageTemplate.language: missing (expected 'en_US', got None)
  $.messageTemplate.createdTime: value_mismatch (expected '2026-09-06T19:23:24.229Z', got 1794401630.63035)
  $.messageTemplate.lastModifiedTime: value_mismatch (expected '2026-09-06T19:23:24.229Z', got 1794401630.63035)
  divergence_key: service=wisdom|CreateMessageTemplateVersion|status=200->200|keys=$.messageTemplate.attributeTypes,$.messageTemplate.channel,$.messageTemplate.createdTime,$.messageTemplate.language,$.messageTemplate.lastModifiedTime

Cause 3 - fixture qconnect-message-template-versions step task#5, ListMessageTemplateVersions, expected 200 / actual 200:
  $.messageTemplateVersionSummaries[name="<template-name>"].channel: missing (expected 'CHAT', got None)
  divergence_key: service=wisdom|ListMessageTemplateVersions|status=200->200|keys=$.messageTemplateVersionSummaries[...].channel
  (the generated probe template name in the selector is per-run and not a requirement)

Smithy consulted (services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json): UpdateMessageTemplate output MessageTemplateData; CreateMessageTemplateVersion output ExtendedMessageTemplateData; ListMessageTemplateVersions output MessageTemplateVersionSummaryList/MessageTemplateVersionSummary; shapes MessageTemplateData and ExtendedMessageTemplateData (members channel: Channel @length 1..10 @sensitive, channelSubtype required ChannelSubtype EMAIL|SMS|WHATSAPP|PUSH, language: LanguageCode @length 2..5, attributeTypes: MessageTemplateAttributeTypeList @uniqueItems, createdTime/lastModifiedTime required Timestamp @timestampFormat("date-time")); MessageTemplateVersionSummary (member channel: Channel).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture qconnect-message-template-lifecycle, step task#6 - POST /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId} (UpdateMessageTemplate) for the EMAIL template created earlier in the fixture. Inspect the response messageTemplate: channel, language and attributeTypes are absent, and createdTime/lastModifiedTime are epoch-seconds numbers rather than ISO-8601 date-time strings.
3. Cause 2: replay fixture qconnect-message-template-versions, step task#4 - POST /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId}/versions (CreateMessageTemplateVersion) for the CHAT-subtype template created earlier in that fixture. The response messageTemplate shows the same three missing members and the same two epoch-number timestamps.
4. Cause 3: replay fixture qconnect-message-template-versions, step task#5 - GET /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId}/versions (ListMessageTemplateVersions). The returned messageTemplateVersionSummaries entry for that template has no `channel` member.
5. All three steps return HTTP 200, so the divergence is visible only in the response bodies.
