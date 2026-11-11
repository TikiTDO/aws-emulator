Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-list-pipelines-membership-and-pagination
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-list-pipelines-membership-and-pagination
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-492ecd65"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-492ecd65"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-492ecd65"].updated
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-ded4b942"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-ded4b942"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-ded4b942"].updated
Issue-Commit: b37a548368ed88ff8e9aed060c39391f1ed9f4cf

# CodePipeline ListPipelines summarizes a created pipeline as pipelineType V1 and emits sub-millisecond timestamps

> _Note: this issue was identified against the code base at commit `b37a548368ed88ff8e9aed060c39391f1ed9f4cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelines

## Description
The ListPipelines PipelineSummary projection reports the wrong pipeline type for a pipeline the client just created, and the summary's created/updated timestamps are serialized at the emulator clock's native precision instead of CodePipeline's wire resolution. 1 cause (one replay cluster, two fields of the same summary projection).

Cause 1 of 1: ListPipelines reports pipelineType 'V1' for pipelines AWS summarizes as 'V2', and serializes created/updated with six or more fractional digits (fixture codepipeline-list-pipelines-membership-and-pagination, step task#8).
AWS: Smithy ListPipelines output.pipelines targets PipelineList of PipelineSummary {name, version, pipelineType, executionMode, created, updated}; PipelineSummary.pipelineType targets the PipelineType enum whose values are exactly V1 and V2, and its documentation states the pipeline types differ in characteristics and price, a V2 pipeline being the V1 structure plus the release-safety and trigger parameters. PipelineSummary.created/updated target Timestamp; codepipeline speaks awsJson1_1, whose default timestamp form is epoch-seconds as a JSON number. Recorded behavior: for both pipelines the fixture creates with CreatePipeline, the recorded ListPipelines response summarizes pipelineType 'V2', and every recorded CodePipeline timestamp in this corpus carries at most three fractional digits (millisecond resolution), e.g. a recorded lastUpdateTime of 1793700369.272. Hypothesis: the stored pipeline's type defaults to V1 at creation rather than to the type the service assigns a newly created pipeline, and the epoch-seconds encoder writes the clock's full precision.
Emulator: both PipelineSummary entries report pipelineType 'V1' where the recording reports 'V2', and their created/updated come back as epoch-seconds numbers with six or seven fractional digits (observed 1794422900.0217595 and 1794422900.0439074 in this replay; the instants themselves are per-run values and are not requirements, only the value 'V2' and the fractional precision are).
Why: pipelineType is how a client enumerating pipelines decides whether the V2-only declaration features (triggers, pipeline-level variables, stage conditions, release safety) may be written to a pipeline and what it is billed as; a client branching on the summary refuses a V2-only declaration the pipeline actually accepts, or misreports the fleet's pipeline mix. A client that compares or digests these timestamps against AWS-recorded values, or parses them with a millisecond-resolution parser, sees a number shape AWS never emits.
Fix: the pipeline state's default pipelineType at creation and the PipelineSummary projection in the ListPipelines handler that reads it; plus the shared epoch-seconds timestamp serializer used by the codepipeline response serializers, which must truncate to millisecond resolution.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8.

Related open issues (neither owns this replay key — the replay reports this cluster unowned): 20261111T061123Z-codepipeline-listpipelines-reports-pipelinetype-v1-and-timestamps-carry.md describes the same two faults, but its recorded replay keys embed the per-run generated pipeline names from an earlier run (sim2real-probe-codepipeline-lp-a-5faa979c / lp-b-e0960bb3), so they do not join this run's keys (lp-a-492ecd65 / lp-b-ded4b942) and the cluster is reported unowned; and 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md owns pipelineType V1 only on the PipelineDeclaration responses (GetPipeline/CreatePipeline/UpdatePipeline), not on the ListPipelines summary projection. Re-verification of a fix must replay this fixture step.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, /__admin/health answered 200). Cluster: service codepipeline, operation ListPipelines, verdict mismatch, expected status 200, actual status 200, fixture codepipeline-list-pipelines-membership-and-pagination, step task#8, owned_by empty, stub_path empty.

Diverging fields reported by the replay:
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-492ecd65"].pipelineType: value_mismatch - expected 'V2', got 'V1'
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-ded4b942"].pipelineType: value_mismatch - expected 'V2', got 'V1'
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-492ecd65"].created: value_mismatch - recorded value not asserted, got 1794422900.0217595
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-492ecd65"].updated: value_mismatch - recorded value not asserted, got 1794422900.0217595
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-ded4b942"].created: value_mismatch - recorded value not asserted, got 1794422900.0439074
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-ded4b942"].updated: value_mismatch - recorded value not asserted, got 1794422900.0439074

Requirement vs per-run values: the requirement is that a pipeline created through CreatePipeline is summarized with pipelineType 'V2' and that created/updated are epoch-seconds numbers with at most three fractional digits. The pipeline names and the instants above are per-run generated values and are not requirements.

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json - ListPipelines (input nextToken/maxResults, output pipelines: PipelineList + nextToken; errors InvalidNextTokenException, ValidationException; paginated items=pipelines), shape PipelineSummary (members name, version, pipelineType, executionMode, created: Timestamp, updated: Timestamp), shape PipelineType (values V1, V2).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture codepipeline-list-pipelines-membership-and-pagination against it.
3. At step task#8 call ListPipelines after the fixture's CreatePipeline calls have created the two pipelines.
4. Read each PipelineSummary: pipelineType comes back 'V1' where the recording reports 'V2'.
5. Read the same summaries' created/updated: they are epoch-seconds numbers with six or seven fractional digits, where every recorded CodePipeline timestamp in this corpus has at most three.
