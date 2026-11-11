Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-resolver-mapping-template-unicode-round-trip
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-resolver-mapping-template-unicode-round-trip
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|GetResolver|2xx->2xx|$.resolver.responseMappingTemplate
Issue-Related: 20261110T162826Z-appsync-resolver-updates-are-not-persisted-getresolver-and.md
Issue-Commit: 323587cc72041ca5bad5e7b036e306f86c24b041

# AppSync UpdateResolver does not persist the mapping templates: GetResolver serves the pre-update text (1 cause)

> _Note: this issue was identified against the code base at commit `323587cc72041ca5bad5e7b036e306f86c24b041`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResolver

## Description
=== Cause 1 of 1: an UpdateResolver that changes requestMappingTemplate/responseMappingTemplate is accepted but not stored, so the following GetResolver returns the templates the resolver was created with (fixture appsync-resolver-mapping-template-unicode-round-trip, step task#7) ===
AWS: Smithy: UpdateResolver (POST /v1/apis/{apiId}/types/{typeName}/resolvers/{fieldName}) takes requestMappingTemplate and responseMappingTemplate (MappingTemplate, @length 1..65536, @pattern ^.*$) and returns the updated Resolver; GetResolver (GET /v1/apis/{apiId}/types/{typeName}/resolvers/{fieldName}) returns Resolver. Recorded behavior: after a successful UpdateResolver, GetResolver returns exactly the template strings that were just submitted, byte for byte — the templates are opaque text, so non-ASCII characters (accented Latin, CJK, Cyrillic, emoji), embedded double quotes and backslashes all round-trip unchanged, and a responseMappingTemplate supplied on the update replaces the previous one.
Emulator: GetResolver returns the templates from the original CreateResolver — both requestMappingTemplate and responseMappingTemplate are the pre-update values; the update's text (including its own Unicode content) never appears. The update call itself reports success.
Why: a client that updates a resolver and then reads it back sees its change silently discarded, so it cannot tell a persisted write from a lost one; any configuration-management flow that writes-then-verifies either loops forever or reports convergence on state the service never stored. Because the templates are what the data plane executes, the resolver keeps running the old logic after a successful update.
Fix: the UpdateResolver handler's state write (persist requestMappingTemplate/responseMappingTemplate, preserving the submitted bytes verbatim) and/or the resolver state the GetResolver handler reads.
NOTE: open issue 20261110T162826Z-appsync-resolver-updates-are-not-persisted-getresolver-and.md describes the same not-persisted family for other resolver members; the replay did NOT mark this cluster as owned by it (owned_by was empty), so it is filed here and may be closed by the same fix. Open stub issue 20261110T215409Z-appsync-updateresolver-stub.md concerns an unimplemented UpdateResolver scaffold; here the update is served, it just does not persist.
Traces: appsync-resolver-mapping-template-unicode-round-trip#task#7.

## Evidence
Mismatch cluster from trace set simparity-wire-aws-v1 (service appsync, operation GetResolver), no stub_path and no owned_by. Fixture appsync-resolver-mapping-template-unicode-round-trip, step task#7, expected 200 / got 200:
  $.resolver.requestMappingTemplate value_mismatch — expected '## updated: ñøπ 🚀 üñíçødé "double\\"quote" back\\\\slash\n{\n  "version": "2018-05-29",\n  "operation": "Invoke",\n  "payload": $util.toJson($context.arguments)\n}', got the create-time value '## comment: héllo 世界 🎉 "quoted" \\backslash\\ Ω≈ç√\n{\n  "version": "2018-05-29",\n  "operation": "Invoke",\n  "payload": $util.toJson($context.arguments)\n}'.
  $.resolver.responseMappingTemplate value_mismatch — expected '## updated response ключ\n$util.toJson($context.result)', got the create-time value '$util.toJson($context.result)'.
The two actual values are precisely the strings submitted at CreateResolver earlier in the same fixture, which is what identifies this as a lost write rather than a mangled encoding.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), fixture appsync-resolver-mapping-template-unicode-round-trip, step task#7. By hand: create_graphql_api -> start_schema_creation -> create_data_source -> create_resolver(typeName, fieldName, requestMappingTemplate=<template A, containing non-ASCII text, quotes and backslashes>, responseMappingTemplate='$util.toJson($context.result)') -> update_resolver(same apiId/typeName/fieldName, requestMappingTemplate=<template B>, responseMappingTemplate=<template C>) -> get_resolver(apiId, typeName, fieldName). Expect resolver.requestMappingTemplate == template B and resolver.responseMappingTemplate == template C; observe template A and the original response template.
