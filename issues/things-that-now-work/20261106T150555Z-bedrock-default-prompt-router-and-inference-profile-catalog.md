Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-catalog-list-prompt-routers
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-catalog-list-prompt-routers bedrock-catalog-get-prompt-router bedrock-catalog-list-inference-profiles
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|ListPromptRouters|2xx->2xx|$.promptRouterSummaries[*]
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.createdAt
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.updatedAt
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.description
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.fallbackModel.modelArn
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.models[*]
Issue-Key: bedrock|ListInferenceProfiles|2xx->2xx|$.inferenceProfileSummaries[*]
Issue-Commit: dd440386ec4fc255614ad32661bffc3e70f095cf

# bedrock default prompt-router and inference-profile catalog seed diverges from AWS

> _Note: this issue was identified against the code base at commit `dd440386ec4fc255614ad32661bffc3e70f095cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPromptRouters

## Description
The emulator's built-in read-only Bedrock catalog for DEFAULT prompt routers and SYSTEM_DEFINED inference profiles is seeded with its own invented content. Every read of that catalog — the two list operations and the per-ARN get — returns 200 with summaries whose descriptions, createdAt/updatedAt timestamps and target-model ARN sets are not the ones AWS publishes. The HTTP status and the response shape are right; the catalog CONTENT is wrong, so the defect is in the seed data (and the serializer's timestamp precision), not in the handlers' control flow. There are 5 causes in this bundle; all 5 must be closed before submitting.

Cause 1 of 5 — ListPromptRouters returns a prompt-router catalog whose every summary diverges (fixture bedrock-catalog-list-prompt-routers, step task#1).
AWS: Smithy bedrock#ListPromptRouters -> GET /prompt-routers, 200, output member promptRouterSummaries: list<PromptRouterSummary>; PromptRouterSummary members promptRouterName, routingCriteria, description (@length 1..200, @sensitive), createdAt/updatedAt (target Timestamp, @timestampFormat date-time), promptRouterArn, models, fallbackModel, status, type. The content of the DEFAULT routers is not in the model, so: recorded behavior only. The recording shows three default routers; for the Nova router AWS reports createdAt and updatedAt as the same date-time with millisecond precision (recorded 2025-04-21T07:00:00.000Z), and for the Claude router description "Routes requests among models in the Claude family" with fallbackModel.modelArn pointing at the us.anthropic.claude-3-5-sonnet-20240620-v1:0 inference profile.
Emulator: all three entries $.promptRouterSummaries[0], [1] and [2] come back with different content — the Nova router's createdAt/updatedAt are 2024-12-03T00:00:00Z (wrong date, and second rather than millisecond precision), and the Claude router's description/fallbackModel/models carry the 20241022-v2:0 Claude 3.5 Sonnet profile and the text "Routes requests among models in the Claude 3.5 family".
Why: a client that enumerates default prompt routers to choose one, or that pins a router by its fallback model, gets a catalog that names different model versions and different creation dates than AWS, so router selection and any assertion on router metadata behaves differently against the emulator than against AWS.
Fix: the bedrock service's built-in default-prompt-router catalog seed (the static table the ListPromptRouters handler serves), plus the timestamp serializer used for PromptRouterSummary.createdAt/updatedAt so date-time values keep the precision AWS emits.
Traces: bedrock-catalog-list-prompt-routers#task#1.

Cause 2 of 5 — GetPromptRouter on the Nova default router reports the wrong creation/update date (fixture bedrock-catalog-get-prompt-router, step task#1).
AWS: Smithy bedrock#GetPromptRouter -> GET /prompt-routers/{promptRouterArn}, 200, output createdAt/updatedAt target Timestamp @timestampFormat date-time. Recorded behavior only for the value: AWS reports the Nova default router's createdAt and updatedAt as one and the same date-time, serialized with millisecond precision.
Emulator: $.createdAt and $.updatedAt both come back as a different, earlier date (recorded run: expected 2025-04-21T07:00:00.000Z, got 2024-12-03T00:00:00Z) — the date itself is wrong and the fractional-second field AWS emits is absent.
Why: a client that reads a default router and displays or compares its age, or that caches on updatedAt, sees a value that never matches AWS.
Fix: the default-prompt-router catalog seed entry for the Nova router and the date-time serializer for GetPromptRouter's createdAt/updatedAt.
Traces: bedrock-catalog-get-prompt-router#task#1.

Cause 3 of 5 — GetPromptRouter on the Claude default router reports the wrong description, fallback model and target models (fixture bedrock-catalog-get-prompt-router, step task#2).
AWS: Smithy bedrock#GetPromptRouter output members description (PromptRouterDescription, @length 1..200, @sensitive), fallbackModel (PromptRouterTargetModel, required), models (PromptRouterTargetModels, required), createdAt/updatedAt (Timestamp, date-time). Recorded behavior only for the values: AWS describes this router as routing "among models in the Claude family" (family-level wording, no version number) and both its fallbackModel.modelArn and its second models[] entry name the us.anthropic.claude-3-5-sonnet-20240620-v1:0 inference profile.
Emulator: $.description is "Routes requests among models in the Claude 3.5 family", and $.fallbackModel.modelArn and $.models[1].modelArn both name the us.anthropic.claude-3-5-sonnet-20241022-v2:0 inference profile instead; $.createdAt/$.updatedAt are also a different date (expected 2024-11-20T00:00:00.000Z, got 2024-10-22T00:00:00Z).
Why: the router's advertised routing set is a different model version from the one AWS routes to, so a client that resolves a default router to its concrete target/fallback model — e.g. to pre-check entitlement or to pick an inference profile to invoke — resolves to the wrong model.
Fix: the default-prompt-router catalog seed entry for the Claude router (description text, fallbackModel and models target list, timestamps).
Traces: bedrock-catalog-get-prompt-router#task#2.

Cause 4 of 5 — GetPromptRouter on the Llama default router reports the wrong description and dates (fixture bedrock-catalog-get-prompt-router, step task#3).
AWS: as Cause 3 (same operation and members). Recorded behavior only for the values: AWS describes this router at family level as routing "among models in the LLaMA family", with createdAt and updatedAt equal and millisecond-precision.
Emulator: $.description is "Routes requests among models in the Llama 3.1 family" (version-qualified, different capitalisation), and $.createdAt/$.updatedAt are a different date (expected 2024-11-20T00:00:00.000Z, got 2024-07-23T00:00:00Z).
Why: same as Cause 3 — the advertised router identity does not match AWS, so metadata-driven client logic diverges.
Fix: the default-prompt-router catalog seed entry for the Llama router (description text and timestamps).
Traces: bedrock-catalog-get-prompt-router#task#3.

Cause 5 of 5 — ListInferenceProfiles serves a system-defined inference-profile catalog whose every summary diverges (fixture bedrock-catalog-list-inference-profiles, step task#1).
AWS: Smithy bedrock#ListInferenceProfiles -> GET /inference-profiles, 200, output inferenceProfileSummaries: list<InferenceProfileSummary>; InferenceProfileSummary members inferenceProfileName, description (@length 1..200, @sensitive), createdAt/updatedAt (Timestamp, date-time), inferenceProfileArn, models (InferenceProfileModels, @length 1..5), inferenceProfileId, status (ACTIVE), type (SYSTEM_DEFINED). The catalog contents are not in the model, so: recorded behavior only. The recording shows AWS describing each SYSTEM_DEFINED profile by the concrete regions it routes to (e.g. for us.anthropic.claude-3-haiku-20240307-v1:0: "Routes requests to Anthropic Claude 3 Haiku in us-east-1 and us-west-2.", a trailing period, exactly two models[] entries — the us-east-1 and us-west-2 foundation-model ARNs — and createdAt/updatedAt 2024-08-26T00:00:00Z).
Emulator: all 93 entries $.inferenceProfileSummaries[0]..[92] diverge. For the same Claude 3 Haiku profile the description is the generic "Routes requests to Anthropic Claude 3 Haiku in US regions" (no region list, no trailing period), models[] carries THREE entries (an extra us-east-2 foundation-model ARN AWS does not list), and createdAt/updatedAt are 2024-03-13T00:00:00Z.
Why: the profile's member region list is part of its contract — a client that reads models[] to learn which regions a cross-region profile can fan out to, or that counts/validates them against the modeled @length 1..5, gets a region set AWS does not route to; and every description and timestamp in the catalog differs, so no catalog assertion holds.
Fix: the bedrock service's built-in system-defined inference-profile catalog seed (per-profile description text, member model ARN list and timestamps) that the ListInferenceProfiles handler serves.
Traces: bedrock-catalog-list-inference-profiles#task#1.

Related but NOT covering these: open issue 20261102T055258Z-bedrock-listfoundationmodels-model-summaries-diverge-converse-metadata-capability-flag.md owns the analogous ListFoundationModels catalog divergence (fixture bedrock-catalog-list-foundation-models#task#1) and is the reason that cluster is excluded here; it says nothing about the prompt-router or inference-profile catalogs.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 162 steps, 106 passed, score 65.4%). All five clusters below are verdict=mismatch with empty stub_path and empty owned_by.

1) bedrock-catalog-list-prompt-routers#task#1 — ListPromptRouters, expected 200 / got 200; diverging keys $.promptRouterSummaries[0], $.promptRouterSummaries[1], $.promptRouterSummaries[2]. Example (Nova router, summaries[0]): expected createdAt/updatedAt '2025-04-21T07:00:00.000Z', got '2024-12-03T00:00:00Z'. Example (Claude router, summaries[1]): expected description 'Routes requests among models in the Claude family' and fallbackModel.modelArn '...:inference-profile/us.anthropic.claude-3-5-sonnet-20240620-v1:0'.

2) bedrock-catalog-get-prompt-router#task#1 — GetPromptRouter, 200/200; $.createdAt: expected '2025-04-21T07:00:00.000Z', got '2024-12-03T00:00:00Z'; $.updatedAt: expected '2025-04-21T07:00:00.000Z', got '2024-12-03T00:00:00Z'.

3) bedrock-catalog-get-prompt-router#task#2 — GetPromptRouter, 200/200; $.description: expected 'Routes requests among models in the Claude family', got 'Routes requests among models in the Claude 3.5 family'; $.fallbackModel.modelArn: expected 'arn:aws:bedrock:us-west-2:000000000000:inference-profile/us.anthropic.claude-3-5-sonnet-20240620-v1:0', got '...us.anthropic.claude-3-5-sonnet-20241022-v2:0'; $.models[1]: same substitution; $.createdAt/$.updatedAt: expected '2024-11-20T00:00:00.000Z', got '2024-10-22T00:00:00Z'.

4) bedrock-catalog-get-prompt-router#task#3 — GetPromptRouter, 200/200; $.description: expected 'Routes requests among models in the LLaMA family', got 'Routes requests among models in the Llama 3.1 family'; $.createdAt/$.updatedAt: expected '2024-11-20T00:00:00.000Z', got '2024-07-23T00:00:00Z'.

5) bedrock-catalog-list-inference-profiles#task#1 — ListInferenceProfiles, 200/200; diverging keys $.inferenceProfileSummaries[0] through $.inferenceProfileSummaries[92] (all 93). Example (summaries[0], inferenceProfileId us.anthropic.claude-3-haiku-20240307-v1:0): expected description 'Routes requests to Anthropic Claude 3 Haiku in us-east-1 and us-west-2.', createdAt/updatedAt '2024-08-26T00:00:00Z', models = [us-east-1 foundation-model ARN, us-west-2 foundation-model ARN]; got description 'Routes requests to Anthropic Claude 3 Haiku in US regions', createdAt/updatedAt '2024-03-13T00:00:00Z', models = [us-east-1, us-east-2, us-west-2].

Fixed text vs variable parts: these are 200 responses, so there is no error code or error message at stake. The fixed text in the diverging payloads is the description sentence template AWS uses — for inference profiles, 'Routes requests to <model display name> in <comma-separated region list>.' with the trailing period; for default prompt routers, 'Routes requests among models in the <family> family' with the family named at family level and no version qualifier. The variable parts, which must come from the catalog entry and not be copied from one recording, are the model display name, the region list, the family name, the member/fallback inference-profile and foundation-model ARNs, and the createdAt/updatedAt date-times. Account id (000000000000) and region (us-west-2) in the ARNs are environment values, not requirements.

Model references used: smithy_query operation=GetPromptRouter,ListPromptRouters,ListInferenceProfiles and shape=PromptRouterSummary,InferenceProfileSummary,Timestamp on services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json. Note the on-disk model declares no toolboxArn member on InferenceProfileSummary and no shadowModeConfig member on PromptRouterSummary; the recorded responses carry those keys as null, so they are reported here as recorded normalisation only and are NOT claimed as a missing-member requirement.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture bedrock-catalog-list-prompt-routers, step task#1 — GET /prompt-routers. Compare $.promptRouterSummaries[*] description, fallbackModel.modelArn, models[] and createdAt/updatedAt against the recorded response.
4. Cause 2: replay fixture bedrock-catalog-get-prompt-router, step task#1 — GET /prompt-routers/{nova default-prompt-router ARN}. Compare $.createdAt and $.updatedAt.
5. Cause 3: replay fixture bedrock-catalog-get-prompt-router, step task#2 — GET /prompt-routers/{claude default-prompt-router ARN}. Compare $.description, $.fallbackModel.modelArn, $.models[1].modelArn, $.createdAt, $.updatedAt.
6. Cause 4: replay fixture bedrock-catalog-get-prompt-router, step task#3 — GET /prompt-routers/{llama default-prompt-router ARN}. Compare $.description, $.createdAt, $.updatedAt.
7. Cause 5: replay fixture bedrock-catalog-list-inference-profiles, step task#1 — GET /inference-profiles. Compare every $.inferenceProfileSummaries[i] description, models[] member ARN list and createdAt/updatedAt.
8. All five steps must pass before the issue is closed; re-replay each listed fixture#step (see Issue-Trace / Issue-Step and Issue-Trace-Ids) rather than only the representative.
