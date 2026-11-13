Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-pipeline-artifact-store-encryption-key-preserved
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-pipeline-artifact-store-encryption-key-preserved codepipeline-pipeline-variable-description-empty-vs-absent-preserved codepipeline-update-pipeline-invalid-action-declaration-error codepipeline-update-pipeline-invalid-stage-declaration-error
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|GetPipeline|2xx->2xx|$.pipeline.artifactStore.encryptionKey.__type
Issue-Key: codepipeline|GetPipeline|2xx->2xx|$.pipeline.pipelineType
Issue-Key: codepipeline|GetPipeline|2xx->2xx|$.pipeline.stages[*].actions[*].actionTypeId.__type
Issue-Key: codepipeline|GetPipeline|2xx->2xx|$.pipeline.stages[*].actions[*].configuration
Issue-Key: codepipeline|GetPipeline|2xx->2xx|$.pipeline.stages[*].actions[*].runOrder
Issue-Key: codepipeline|CreatePipeline|2xx->2xx|$.pipeline.stages[*].actions[*].actionTypeId.__type
Issue-Key: codepipeline|CreatePipeline|2xx->2xx|$.pipeline.stages[*].actions[*].configuration
Issue-Key: codepipeline|UpdatePipeline|2xx->2xx|$.pipeline.stages[*].actions[*].actionTypeId.__type
Issue-Key: codepipeline|UpdatePipeline|2xx->2xx|$.pipeline.stages[*].actions[*].configuration
Issue-Key: codepipeline|GetPipeline|2xx->2xx|$.pipeline.stages[*].actions[*]
Issue-Key: codepipeline|GetPipeline|2xx->2xx|$.pipeline.version
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodePipeline PipelineDeclaration responses omit modeled defaults and nested __type and report pipelineType V1

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipeline

## Description
Every operation that returns a PipelineDeclaration (Smithy: CreatePipeline/UpdatePipeline/GetPipeline all have output member pipeline -> PipelineDeclaration) serializes it short in the emulator. Three things are missing or wrong, consistently across create, update and read:
(a) nested structures carry no `__type` discriminator — recorded behavior only: the recorded AWS JSON 1.1 responses include `"__type": "ActionTypeId"` on every ActionDeclaration.actionTypeId and `"__type": "EncryptionKey"` on artifactStore.encryptionKey; the emulator emits the structures without it;
(b) members AWS materializes with a server-side default are dropped instead of echoed — ActionDeclaration.runOrder (Smithy: ActionRunOrder, range 1..999; AWS stores and returns 1 when the declaration omitted it) and ActionDeclaration.configuration (Smithy: ActionConfigurationMap; AWS returns an empty map `{}` for an action declared with no configuration, e.g. a Manual Approval action);
(c) PipelineDeclaration.pipelineType (Smithy: enum PipelineType values [V1, V2]) comes back `V1` where AWS reports `V2` for the same declaration.

Cause 1 of 6 — GetPipeline of a pipeline created with an artifact-store KMS encryptionKey. AWS: 200 with artifactStore.encryptionKey.__type='EncryptionKey', pipelineType='V2', actionTypeId.__type on each action, runOrder=1 on each action and configuration={} on the action declared without one. Emulator: 200 but encryptionKey.__type and both actionTypeId.__type absent, pipelineType='V1', runOrder absent, configuration absent. Why: a client that reads pipelineType to decide whether V2-only features (variables, triggers, stage conditions) are available concludes the pipeline is V1, and a client that round-trips GetPipeline into UpdatePipeline loses each action's runOrder. Fix: the PipelineDeclaration serializer shared by GetPipeline/CreatePipeline/UpdatePipeline, plus the pipelineType defaulting in pipeline state. Traces: codepipeline-pipeline-artifact-store-encryption-key-preserved#task#6.

Cause 2 of 6 — CreatePipeline response. AWS: the echoed declaration carries actionTypeId.__type on both actions and configuration={} on the approval action. Emulator: all three absent. Fix: same serializer, on the CreatePipeline response path. Traces: codepipeline-pipeline-variable-description-empty-vs-absent-preserved#task#5.

Cause 3 of 6 — GetPipeline on the same pipeline (read-back, two steps). Same omissions as Cause 2. Fix: same serializer, GetPipeline path. Traces: codepipeline-pipeline-variable-description-empty-vs-absent-preserved#task#6.

Cause 4 of 6 — UpdatePipeline response. Same omissions as Cause 2. Fix: same serializer, UpdatePipeline path. Traces: codepipeline-pipeline-variable-description-empty-vs-absent-preserved#task#7.

Cause 5 of 6 — GetPipeline after an UpdatePipeline that AWS refuses. AWS: the update is rejected (see the declaration-validation issue), so GetPipeline still returns the ORIGINAL declaration — version 1, one action in the Source stage, pipelineType V2, actionTypeId.__type / runOrder / configuration present. Emulator: version 2 with the rejected duplicate-named action persisted as stages[0].actions[1], pipelineType V1 and the same missing members. Why: a client reading back after a refused update sees a pipeline AWS would never have stored, with a bumped version. Fix: the UpdatePipeline validation gate must reject before mutating state, plus the serializer above. Traces: codepipeline-update-pipeline-invalid-action-declaration-error#task#5.

Cause 6 of 6 — GetPipeline after an UpdatePipeline refused for a stage-declaration reason. AWS: pipelineType V2, actionTypeId.__type, runOrder=1, configuration={}. Emulator: pipelineType V1 and all of those absent. Fix: same serializer and the same pipelineType default. Traces: codepipeline-update-pipeline-invalid-stage-declaration-error#task#7.

Related but not covering: open issue 20261029T183239Z-codepipeline-declaration-validation-and-response-shapes-10-net.md; the replay reports owned_by=[] for all six clusters, so none of them is carried by it.

## Evidence
codepipeline-pipeline-artifact-store-encryption-key-preserved#task#6 (GetPipeline, 200->200): '$.pipeline.artifactStore.encryptionKey.__type: missing (expected EncryptionKey, got None)', '$.pipeline.pipelineType: value_mismatch (expected V2, got V1)', '$.pipeline.stages[0].actions[0].actionTypeId.__type: missing', '$.pipeline.stages[0].actions[0].runOrder: missing (expected 1, got None)', '$.pipeline.stages[1].actions[0].configuration: missing (expected {}, got None)', '$.pipeline.stages[1].actions[0].runOrder: missing (expected 1, got None)'.
codepipeline-pipeline-variable-description-empty-vs-absent-preserved#task#5 / #task#6 / #task#7 (CreatePipeline, GetPipeline x2, UpdatePipeline, all 200->200): actionTypeId.__type missing on both stages' actions and stages[1].actions[0].configuration missing (expected {}).
codepipeline-update-pipeline-invalid-action-declaration-error#task#5 (GetPipeline, 200->200): '$.pipeline.version: value_mismatch (expected 1, got 2)', '$.pipeline.stages[0].actions[1]: extra (got the duplicate-named SourceAction declaration)', pipelineType V2 vs V1, runOrder/__type/configuration missing.
codepipeline-update-pipeline-invalid-stage-declaration-error#task#7 (GetPipeline, 200->200): pipelineType V2 vs V1, actionTypeId.__type / runOrder / configuration missing.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus.
2. CreatePipeline with a Source stage (AWS/S3) and an Approval stage whose action declares no configuration and no runOrder, with an artifactStore encryptionKey.
3. Inspect the CreatePipeline response and then GetPipeline/UpdatePipeline on the same pipeline: pipelineType comes back V1, no nested __type members, and runOrder / configuration are absent.
4. Re-replay codepipeline-pipeline-artifact-store-encryption-key-preserved#task#6, codepipeline-pipeline-variable-description-empty-vs-absent-preserved#task#5/#task#6/#task#7, codepipeline-update-pipeline-invalid-action-declaration-error#task#5 and codepipeline-update-pipeline-invalid-stage-declaration-error#task#7.
