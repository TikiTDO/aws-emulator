Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-list-pipelines-membership-and-pagination
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-list-pipelines-membership-and-pagination
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-281509ce"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-281509ce"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-281509ce"].updated
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-6d5b5895"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-6d5b5895"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-6d5b5895"].updated
Issue-Commit: 0657f445a5e75360273f7d5f2015a3a01d39cdcd

# CodePipeline ListPipelines scalar projection: pipelineType V1 for a V2 pipeline and clock-precision timestamps

> _Note: this issue was identified against the code base at commit `0657f445a5e75360273f7d5f2015a3a01d39cdcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelines

## Description
The PipelineSummary projection ListPipelines returns gets two scalar members wrong: the pipeline type it reports for a pipeline created through CreatePipeline, and the fractional precision of the epoch-seconds timestamps it serializes. Both show on the same replayed step, and both are properties of the summary projection / the service's timestamp encoding rather than of one recording. 2 causes.

Cause 1 of 2: ListPipelines reports pipelineType 'V1' for a pipeline AWS summarizes as 'V2' (fixture codepipeline-list-pipelines-membership-and-pagination, step task#8).
AWS: Smithy (services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json) models ListPipelinesOutput.pipelines as PipelineList of PipelineSummary {name: PipelineName, version: PipelineVersion, pipelineType: PipelineType, executionMode: ExecutionMode, created: Timestamp, updated: Timestamp}; PipelineType is an enum with values V1 and V2, documented as the two pipeline types that 'differ in characteristics and price'. The model does not state which type a newly created pipeline receives, so for that part this is recorded behavior only: for both pipelines the fixture creates with CreatePipeline, the recording's ListPipelines response reports pipelineType 'V2'. Hypothesis: the emulator stores a hard-coded default of V1 on creation, and the summary projection faithfully reports that wrong stored value — the same wrong type appears on the PipelineDeclaration read paths, so the defect is in the stored pipeline, not in this one projection.
Emulator: every PipelineSummary in the response reports pipelineType 'V1'. Status is 200 on both sides; only the member value diverges.
Why: pipelineType is how a client decides whether the V2-only declaration features (triggers, pipeline-level variables, stage conditions, release-safety settings) are usable on a pipeline, and what the pipeline is billed as. A client that enumerates pipelines and branches on the summary's type will refuse to write a V2-only declaration to a pipeline that in fact accepts one, or will misreport its fleet's pipeline mix.
Fix: the default pipelineType the pipeline state records at creation (CreatePipeline's handler/state), and the PipelineSummary projection in the ListPipelines handler that reads it.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8.

Cause 2 of 2: created/updated are serialized at the internal clock's precision instead of CodePipeline's millisecond wire form (same fixture, same step).
AWS: PipelineSummary.created and PipelineSummary.updated both target Timestamp, and codepipeline speaks awsJson1_1, whose default timestamp form is epoch-seconds as a JSON number. The model fixes no fractional precision, so this is recorded behavior only: every timestamp in the corpus's recorded CodePipeline responses carries at most three fractional digits, i.e. millisecond resolution (for example a recorded ListActionExecutions lastUpdateTime of 1793700369.272). Millisecond resolution is the wire form; the instant itself is a per-run value and is not a requirement.
Emulator: it writes out the full precision of its clock — created and updated come back with seven fractional digits. The replay's expected value is masked ('<shaped-not-asserted>'), so what diverges is the serialized number's shape, not the instant. The same over-precise encoding appears on PutApprovalResult.approvedAt and GetPipelineState created/updated/lastStatusChange, so it is one shared serializer and not one operation.
Why: a client that round-trips, digests or compares these numbers against AWS-recorded values, or parses them with a millisecond-resolution decoder, sees a numeric form AWS never emits; equality and hash comparisons over otherwise-identical responses fail even when the instant is irrelevant.
Fix: the service's epoch-seconds timestamp serializer shared by the codepipeline response serializers must truncate/round to millisecond resolution rather than emitting the clock's native precision.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8.

Related open issues (none of which owns this cluster's replay key — the replay reports it unowned, because their recorded Issue-Key entries are pinned to earlier runs' generated pipeline names): 20261111T061123Z-codepipeline-listpipelines-reports-pipelinetype-v1-and-timestamps-carry.md, 20261111T185123Z-codepipeline-listpipelines-summarizes-a-created-pipeline-as-pipelinetype.md, 20261112T032208Z-codepipeline-listpipelines-pipelinesummary-still-reports-v1-for-a-v2.md and 20261112T103336Z-codepipeline-listpipelines-summary-projection-reports-pipelinetype-v1-and.md describe the same two faults; 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md owns the pipelineType V1 fault on the PipelineDeclaration responses (GetPipeline/CreatePipeline/UpdatePipeline) but not on the PipelineSummary projection.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up=true, /__admin/health answered 200; 344 steps, 311 passed, score 90.4%). This cluster: fixture codepipeline-list-pipelines-membership-and-pagination, step task#8, operation ListPipelines, verdict mismatch, expected status 200, actual status 200, owned_by empty (unowned), stub_path empty, affected_count 1.

Cause 1 — pipelineType:
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-281509ce"].pipelineType: value_mismatch — expected 'V2', got 'V1'.
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-6d5b5895"].pipelineType: value_mismatch — expected 'V2', got 'V1'.
  The two pipeline names are per-run generated values and are not requirements; the requirement is that a pipeline created through CreatePipeline is summarized with pipelineType 'V2'.

Cause 2 — timestamp precision:
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-281509ce"].created: value_mismatch — got 1794512901.1034155 against a recorded value that is masked ('<shaped-not-asserted>').
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-281509ce"].updated: value_mismatch — got 1794512901.1034155.
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-6d5b5895"].created: value_mismatch — got 1794512901.124948.
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-6d5b5895"].updated: value_mismatch — got 1794512901.124948.
  Recorded CodePipeline timestamps in this corpus carry at most three fractional digits (e.g. 1793700369.272). Corroborating steps for the same serializer, filed under their own issues for their other faults: codepipeline-s3-source-manual-approval-run-succeeds#task#9 (PutApprovalResult approvedAt 1794512782.9978092) and codepipeline-s3-source-manual-approval-run-succeeds#task#12 (GetPipelineState created/updated 1794512777.9637985, lastStatusChange 1794512778.2813988).
  Per-run values, not requirements: the instants themselves. The requirement is the fractional precision of the serialized epoch-seconds number.

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json — ListPipelines (output.pipelines: PipelineList; nextToken: NextToken @length 1..2048; input.maxResults: MaxPipelines @range 1..1000; paginated inputToken/outputToken nextToken, items pipelines, pageSize maxResults; errors InvalidNextTokenException 400, ValidationException 400), shape PipelineSummary (members name/version/pipelineType/executionMode/created/updated; created and updated target Timestamp), enum PipelineType (values V1, V2).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture codepipeline-list-pipelines-membership-and-pagination from trace set simparity-wire-aws-v1.
3. At step task#8 call ListPipelines and read the PipelineSummary for each pipeline the fixture created earlier with CreatePipeline.
4. Cause 1: each summary reports pipelineType 'V1' where the recording reports 'V2'.
5. Cause 2: each summary's created and updated come back as epoch-seconds numbers with six or more fractional digits (e.g. 1794512901.1034155), where every recorded CodePipeline timestamp carries at most three.
6. Cross-check the shared serializer: replay codepipeline-s3-source-manual-approval-run-succeeds and read approvedAt at step task#9 and created/updated/lastStatusChange at step task#12 — all carry the same clock-native precision.
