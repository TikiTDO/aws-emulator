Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-list-pipelines-membership-and-pagination
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-list-pipelines-membership-and-pagination
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-4bccae28"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-4bccae28"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-4bccae28"].updated
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-ab3029b8"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-ab3029b8"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-ab3029b8"].updated
Issue-Commit: 03f1d5047411e59a2cbababc00e134c537b99208

# ListPipelines PipelineSummary still reports V1 for a V2-created pipeline, and serializes created/updated at clock precision

> _Note: this issue was identified against the code base at commit `03f1d5047411e59a2cbababc00e134c537b99208`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelines

## Description
The ListPipelines PipelineSummary projection diverges from AWS in two ways on the same response. 2 causes, both carried by the single replay cluster codepipeline-list-pipelines-membership-and-pagination#task#8 (ListPipelines), so one replay of that step exercises both and BOTH must be closed.

Cause 1 of 2: ListPipelines summarizes a pipeline created through CreatePipeline as pipelineType 'V1' where CodePipeline summarizes it as 'V2' (fixture codepipeline-list-pipelines-membership-and-pagination, step task#8).
AWS: Smithy ListPipelines output.pipelines targets PipelineList of PipelineSummary {name, version, pipelineType, executionMode, created, updated}; PipelineSummary.pipelineType targets the PipelineType enum whose values are exactly V1 and V2, and whose documentation distinguishes the two pipeline types by characteristics and price. Recorded behavior: for each of the two pipelines the fixture creates with CreatePipeline, the recorded ListPipelines response reports pipelineType 'V2'. Hypothesis: the emulator stores a default pipelineType of V1 on creation (or the summary projection hard-codes V1), so the enumeration faithfully reports a value AWS never assigns to these pipelines.
Emulator: both PipelineSummary entries report pipelineType 'V1'.
Why: pipelineType is how a client decides whether the V2-only declaration surface (triggers, pipeline-level variables, stage conditions, release-safety parameters) is usable on a pipeline, and what the pipeline is billed as. A client that enumerates pipelines and branches on the summary's type refuses to write a V2-only declaration to a pipeline that would accept one, and misreports the fleet's pipeline mix.
Fix: the pipeline state's pipelineType assignment at creation, and the PipelineSummary projection in the ListPipelines handler that reads it.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8.

Cause 2 of 2: PipelineSummary.created / PipelineSummary.updated are serialized with the emulator clock's native sub-millisecond precision (same fixture, same step).
AWS: Smithy PipelineSummary.created and .updated target Timestamp; codepipeline speaks awsJson1_1, whose default timestamp form is epoch-seconds as a JSON number. Recorded behavior only for the precision: every timestamp in the recorded CodePipeline responses of this corpus carries at most three fractional digits, i.e. millisecond resolution; the instants themselves are per-run values and are not requirements.
Emulator: it emits seven fractional digits (the replay reports .created and .updated as value_mismatch against the recording's non-asserted timestamp, with values carrying full clock precision).
Why: a client that compares, digests or round-trips these numbers against AWS-recorded responses, or parses them with a millisecond-resolution reader, sees a value shape AWS never emits, so equality over otherwise-identical responses fails even when the instant is irrelevant.
Fix: the shared epoch-seconds timestamp serializer used by the codepipeline response serializers must truncate to millisecond resolution rather than emitting the clock's native precision.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8.

Related open issues (neither owns this replay key — the replay reports this cluster as unowned, because their recorded keys name per-run generated pipeline names from an earlier run): 20261111T061123Z-codepipeline-listpipelines-reports-pipelinetype-v1-and-timestamps-carry.md and 20261111T185123Z-codepipeline-listpipelines-summarizes-a-created-pipeline-as-pipelinetype.md describe the same two faults; 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md owns pipelineType V1 only on the PipelineDeclaration responses (GetPipeline/CreatePipeline/UpdatePipeline), not on the PipelineSummary projection. This issue carries the replay key that currently diverges.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, 60 fixtures replayed, 344 steps, 306 passed, score 89.0%).

Cluster: fixture codepipeline-list-pipelines-membership-and-pagination, step task#8, operation ListPipelines, verdict mismatch, expected status 200, actual status 200, owned_by: [] (unowned), stub_path: "".

Diverging members reported by the replay:
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-4bccae28"].pipelineType: value_mismatch — expected 'V2', got 'V1'
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-ab3029b8"].pipelineType: value_mismatch — expected 'V2', got 'V1'
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-4bccae28"].created: value_mismatch — expected '<shaped-not-asserted>', got 1794453536.7594137
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-4bccae28"].updated: value_mismatch — expected '<shaped-not-asserted>', got 1794453536.7594137
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-ab3029b8"].created: value_mismatch — expected '<shaped-not-asserted>', got 1794453536.7783482
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-ab3029b8"].updated: value_mismatch — expected '<shaped-not-asserted>', got 1794453536.7783482

Per-run values that are NOT requirements: the generated pipeline names (sim2real-probe-codepipeline-lp-a-*, -lp-b-*) and the instants themselves. The requirements are (a) a pipeline created through CreatePipeline is summarized with pipelineType 'V2', and (b) the serialized epoch-seconds timestamp carries at most millisecond (three fractional digit) resolution, as every recorded CodePipeline timestamp in this corpus does (e.g. a recorded lastUpdateTime of 1793700369.272).

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json — ListPipelines (input nextToken/maxResults, output pipelines: PipelineList + nextToken, errors InvalidNextTokenException, ValidationException, paginated items=pipelines), shape PipelineSummary (members name, version, pipelineType, executionMode, created: Timestamp, updated: Timestamp) and enum PipelineType (values V1, V2).

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator.
2. Replay fixture codepipeline-list-pipelines-membership-and-pagination from trace set simparity-wire-aws-v1.
3. At step task#8 the fixture calls ListPipelines after creating two pipelines with CreatePipeline. Read each PipelineSummary: pipelineType comes back 'V1' where the recording reports 'V2'.
4. In the same response read created and updated: they are epoch-seconds numbers with seven fractional digits, where every recorded CodePipeline timestamp carries at most three.
5. Re-replay codepipeline-list-pipelines-membership-and-pagination#task#8 after a fix: the step must report pipelineType 'V2' for both created pipelines AND millisecond-resolution created/updated before this issue is closed.
