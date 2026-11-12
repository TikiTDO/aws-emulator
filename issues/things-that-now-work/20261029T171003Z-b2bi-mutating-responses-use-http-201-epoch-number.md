Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-list-operations-and-pagination-errors
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-list-operations-and-pagination-errors b2bi-profile-creation-time-tag-visibility b2bi-tagging-workflow b2bi-partnership-error-paths b2bi-profile-lifecycle b2bi-transformer-job-error-paths b2bi-transformer-lifecycle
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|CreateProfile|2xx->2xx|status
Issue-Key: b2bi|CreateProfile|2xx->2xx|$.createdAt
Issue-Key: b2bi|GetProfile|2xx->2xx|$.createdAt
Issue-Key: b2bi|GetProfile|2xx->2xx|$.modifiedAt
Issue-Key: b2bi|UpdateProfile|2xx->2xx|$.createdAt
Issue-Key: b2bi|UpdateProfile|2xx->2xx|$.modifiedAt
Issue-Key: b2bi|ListProfiles|2xx->2xx|$.profiles[name="sim2real-probe-b2bi-profile-d49b7155-updated"].createdAt
Issue-Key: b2bi|ListProfiles|2xx->2xx|$.profiles[name="sim2real-probe-b2bi-profile-d49b7155-updated"].modifiedAt
Issue-Key: b2bi|CreateTransformer|2xx->2xx|$.createdAt
Issue-Key: b2bi|CreateTransformer|2xx->2xx|$.inputConversion.__type
Issue-Key: b2bi|CreateTransformer|2xx->2xx|$.mapping.__type
Issue-Key: b2bi|CreateTransformer|2xx->2xx|$.transformerId
Issue-Key: b2bi|CreateTransformer|2xx->2xx|status
Issue-Key: b2bi|UpdateTransformer|2xx->2xx|$.createdAt
Issue-Key: b2bi|UpdateTransformer|2xx->2xx|$.inputConversion.__type
Issue-Key: b2bi|UpdateTransformer|2xx->2xx|$.mapping.__type
Issue-Key: b2bi|UpdateTransformer|2xx->2xx|$.modifiedAt
Issue-Key: b2bi|GetTransformer|2xx->2xx|$.createdAt
Issue-Key: b2bi|GetTransformer|2xx->2xx|$.mapping.__type
Issue-Key: b2bi|GetTransformer|2xx->2xx|$.modifiedAt
Issue-Key: b2bi|GetTransformer|2xx->2xx|$.outputConversion.__type
Issue-Key: b2bi|UpdateTransformer|2xx->2xx|$.outputConversion.__type
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# b2bi mutating responses use HTTP 201, epoch-number timestamps, omit modifiedAt and omit nested __type

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProfile

## Description
Ten net-new causes, one shared fault: the emulator's b2bi success responses are serialized wrongly on four counts — a 201 status on create, timestamps as JSON numbers, a missing modifiedAt, and nested structures with no __type.

Shared AWS behavior (Smithy: services/service-com-amazonaws-b2bi-b2bi, protocol awsJson1_0):
- awsJson1_0 has no per-operation HTTP success code; every successful call answers HTTP 200. The recordings confirm 200 for CreateProfile and CreateTransformer.
- CreatedDate/ModifiedDate members carry @timestampFormat("date-time"), so createdAt and modifiedAt are ISO-8601 strings (e.g. of the form 2026-09-06T05:32:39.527Z), never epoch seconds. The recorded instant itself is a per-run value; the FORMAT is the requirement.
- GetProfileResponse/GetTransformerResponse model modifiedAt as an optional member, and the recordings show AWS populating it on a freshly created resource (shaped-not-asserted present), not omitting it until a first update.
- The recorded bodies carry a __type discriminator on nested structure values: com.amazonaws.visby.common#InputConversion, com.amazonaws.visby.common#OutputConversion, com.amazonaws.visby.common#Mapping. The emulator emits those nested objects without __type.

Shared emulator behavior: HTTP 201 from CreateProfile/CreateTransformer; createdAt/modifiedAt as JSON floats (epoch seconds with a fractional part); modifiedAt absent from Get* responses until an update has happened; no __type on nested structures.

Why this matters: an SDK deserializing CreatedDate per @timestampFormat("date-time") either fails to parse a bare number or mis-reads it, so every client that prints or compares b2bi creation/modification times gets garbage; code that reads modifiedAt (change detection, drift reporting) sees an absent member on a resource AWS reports as modified; and an HTTP 201 breaks clients and proxies that treat a non-200 as a redirect/other outcome on a protocol whose success is always 200.

Fix: the b2bi response serializer (status code for the mutating operations, timestamp rendering for CreatedDate/ModifiedDate, nested-structure __type emission) and the create/update state writers that must stamp modifiedAt at creation. Note: the recorded transformerId ('{{transformer_id}}' placeholder vs a generated t-... id) and the literal recorded instants are per-run values and are NOT part of this issue.

Cause 1 of 10: CreateProfile answers 201. AWS: 200. Emulator: 201. Fix: response status for CreateProfile. Trace: b2bi-list-operations-and-pagination-errors#setup#1 (also affects b2bi-profile-creation-time-tag-visibility#setup#1, #setup#2 and b2bi-tagging-workflow#setup#1, #setup#2).

Cause 2 of 10: CreateProfile answers 201 AND renders createdAt as an epoch number. AWS: 200 with createdAt as an ISO-8601 date-time string. Emulator: 201 with createdAt as a float. Fix: status plus timestamp serialization. Trace: b2bi-partnership-error-paths#setup#1 (also b2bi-profile-lifecycle#setup#1).

Cause 3 of 10: GetProfile renders createdAt as an epoch number and omits modifiedAt on a profile that has not yet been updated. AWS: both present as ISO-8601 date-time strings. Fix: timestamp serialization plus stamping modifiedAt at create time. Trace: b2bi-profile-lifecycle#task#2 (also #task#4).

Cause 4 of 10: UpdateProfile renders createdAt and modifiedAt as epoch numbers. AWS: ISO-8601 date-time strings. Fix: timestamp serialization. Trace: b2bi-profile-lifecycle#task#3.

Cause 5 of 10: ListProfiles renders each profile summary's createdAt/modifiedAt as epoch numbers. AWS: ISO-8601 date-time strings in every list entry. Fix: timestamp serialization on the list projection too. Trace: b2bi-profile-lifecycle#task#5.

Cause 6 of 10: CreateTransformer answers 201, renders createdAt as an epoch number, and emits inputConversion and mapping with no __type. AWS: 200, ISO-8601 createdAt, nested __type com.amazonaws.visby.common#InputConversion and com.amazonaws.visby.common#Mapping. Fix: status, timestamp serialization, nested __type. Trace: b2bi-transformer-job-error-paths#setup#1.

Cause 7 of 10: UpdateTransformer renders createdAt/modifiedAt as epoch numbers and emits inputConversion and mapping with no __type. AWS: ISO-8601 strings and the nested __type discriminators. Fix: as cause 6, minus the status. Trace: b2bi-transformer-job-error-paths#setup#2.

Cause 8 of 10: CreateTransformer answers 201. AWS: 200. Fix: response status for CreateTransformer. Trace: b2bi-transformer-lifecycle#setup#1.

Cause 9 of 10: GetTransformer renders createdAt as an epoch number, omits modifiedAt on a transformer not yet updated, and emits mapping/outputConversion with no __type. AWS: ISO-8601 createdAt and modifiedAt plus com.amazonaws.visby.common#Mapping / com.amazonaws.visby.common#OutputConversion. Fix: timestamp serialization, modifiedAt stamping, nested __type. Trace: b2bi-transformer-lifecycle#task#2 (also #task#4).

Cause 10 of 10: UpdateTransformer renders createdAt/modifiedAt as epoch numbers and emits mapping/outputConversion with no __type. AWS: ISO-8601 strings and the nested __type discriminators. Fix: as cause 9. Trace: b2bi-transformer-lifecycle#task#3.

## Evidence
Replay of simparity-wire-aws-v1. b2bi-list-operations-and-pagination-errors#setup#1 CreateProfile: status expected 200, got 201 (5 affected steps). b2bi-partnership-error-paths#setup#1 CreateProfile: status expected 200 got 201; $.createdAt expected an ISO-8601 date-time string ('2026-09-06T05:32:39.527Z' in the recording) got 1793293513.5182936. b2bi-profile-lifecycle#task#2 GetProfile: $.createdAt expected <shaped-not-asserted> got 1793293514.929524; $.modifiedAt missing (expected <shaped-not-asserted>). #task#3 UpdateProfile: createdAt/modifiedAt both epoch floats. #task#5 ListProfiles: $.profiles[...].createdAt/.modifiedAt epoch floats. b2bi-transformer-job-error-paths#setup#1 CreateTransformer: status expected 200 got 201; $.createdAt epoch float; $.inputConversion.__type missing (expected 'com.amazonaws.visby.common#InputConversion'); $.mapping.__type missing (expected 'com.amazonaws.visby.common#Mapping'). #setup#2 UpdateTransformer: same body defects plus epoch modifiedAt. b2bi-transformer-lifecycle#setup#1 CreateTransformer: status expected 200 got 201. #task#2 GetTransformer: epoch createdAt, missing modifiedAt, missing $.mapping.__type and $.outputConversion.__type. #task#3 UpdateTransformer: epoch createdAt/modifiedAt, missing nested __type.

## Steps to Reproduce
1. Boot the emulator from mainline. 2. Replay simparity-wire-aws-v1. 3. Replay b2bi-list-operations-and-pagination-errors#setup#1 and each step in trace_ids. 4. Observe HTTP 201 on the create steps, createdAt/modifiedAt serialized as JSON numbers rather than ISO-8601 date-time strings, modifiedAt absent from GetProfile/GetTransformer, and nested inputConversion/outputConversion/mapping objects without a __type discriminator.
