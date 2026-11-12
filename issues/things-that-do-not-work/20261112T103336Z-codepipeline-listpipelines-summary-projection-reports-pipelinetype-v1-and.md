Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-list-pipelines-membership-and-pagination
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-list-pipelines-membership-and-pagination
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-afee6f2d"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-afee6f2d"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-afee6f2d"].updated
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-c8ed6f64"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-c8ed6f64"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-c8ed6f64"].updated
Issue-Commit: a4a7ebc706314e7b2de6cf374db3451fa49ea4db

# CodePipeline ListPipelines summary projection reports pipelineType V1 and native-clock timestamps

> _Note: this issue was identified against the code base at commit `a4a7ebc706314e7b2de6cf374db3451fa49ea4db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelines

## Description
The PipelineSummary projection ListPipelines returns diverges from CodePipeline in two independent ways on the same response: the pipeline type it reports, and the precision of the timestamps it serializes. 2 causes.

Cause 1 of 2 - ListPipelines reports pipelineType 'V1' for pipelines the service reports as 'V2' (fixture codepipeline-list-pipelines-membership-and-pagination, step task#8).
AWS: Smithy ListPipelines output.pipelines targets PipelineList of PipelineSummary {name, version, pipelineType, executionMode, created, updated}; PipelineSummary.pipelineType targets the PipelineType enum whose values are exactly V1 and V2, and its documentation states CodePipeline provides pipeline types that differ in characteristics and price, a V2 pipeline being a V1-shaped declaration plus the release-safety and trigger parameters. Recorded behavior: for every pipeline this corpus creates through CreatePipeline, the recorded ListPipelines response summarizes it with pipelineType 'V2'. Hypothesis: the emulator stores a pipeline's type defaulted to V1 rather than the type the service assigns a newly created pipeline, and the summary projection then faithfully reports that wrong stored value.
Emulator: every PipelineSummary in the response reports pipelineType 'V1'.
Why: pipelineType is how a client decides whether the V2-only declaration features (triggers, pipeline-level variables, stage conditions, release safety) may be written to a pipeline, and what it is billed as. A client that enumerates pipelines and branches on the summary's type will refuse to push a V2-only declaration to a pipeline that actually accepts one, or will misreport its fleet's pipeline mix. The pipeline names in the recording are per-run generated names and are not a requirement; the requirement is that a pipeline created through CreatePipeline is summarized as V2.
Fix: the default pipelineType the pipeline state records at creation, and the PipelineSummary projection in the ListPipelines handler that reads it.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8.

Cause 2 of 2 - PipelineSummary.created / .updated are serialized with the internal clock's native precision instead of CodePipeline's millisecond wire form (same fixture and step).
AWS: Smithy PipelineSummary.created and .updated target Timestamp ('the date and time the pipeline was created, in timestamp format'); codepipeline speaks awsJson1_1, whose default timestamp form is epoch-seconds as a JSON number. Recorded behavior: every CodePipeline timestamp in this corpus carries at most three fractional digits, i.e. millisecond resolution (for example a recorded ListActionExecutions lastUpdateTime of 1793700369.272), so millisecond resolution is the wire form. The instant itself is a per-run value and is not a requirement - only the number's fractional precision is.
Emulator: it emits six or seven fractional digits (this run: created/updated 1794479429.1354294 and 1794479429.1544154), a value shape AWS never produces.
Why: a client that round-trips, digests or compares these numbers against AWS-recorded responses, or that parses them with a millisecond-resolution timestamp reader, sees a precision AWS does not emit; equality over recorded payloads fails even where the instant is irrelevant.
Fix: the epoch-seconds timestamp encoding shared by the codepipeline response serializers must truncate/round to millisecond resolution before writing the number.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8 ($.pipelines[*].created, $.pipelines[*].updated).

Related open issues (neither carries this cluster's replay key - the replay reports this ListPipelines cluster as unowned): 20261111T061123Z-codepipeline-listpipelines-reports-pipelinetype-v1-and-timestamps-carry.md describes the same two faults but its recorded replay keys are bound to a previous run's generated pipeline names, so it does not join this cluster; it owns only the PutApprovalResult $.approvedAt key for the timestamp serializer. 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md owns pipelineType V1 on the PipelineDeclaration responses (GetPipeline/CreatePipeline/UpdatePipeline) and not on the PipelineSummary projection.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, data listener accepting). Cluster: service codepipeline, operation ListPipelines, verdict mismatch, expected status 200, actual status 200, affected codepipeline-list-pipelines-membership-and-pagination#task#8, stub_path empty, owned_by empty.

Cause 1 (pipelineType):
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-afee6f2d"].pipelineType: value_mismatch - expected 'V2', got 'V1'.
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-c8ed6f64"].pipelineType: value_mismatch - expected 'V2', got 'V1'.
  Both pipelines were created earlier in the same fixture via CreatePipeline. The 'sim2real-probe-codepipeline-lp-*' names are per-run generated values, not requirements.

Cause 2 (timestamp precision):
  $.pipelines[name="...lp-a-afee6f2d"].created: value_mismatch - recorded value not asserted ('<shaped-not-asserted>'), got 1794479429.1354294 (seven fractional digits).
  $.pipelines[name="...lp-a-afee6f2d"].updated: value_mismatch - got 1794479429.1354294.
  $.pipelines[name="...lp-b-c8ed6f64"].created / .updated: value_mismatch - got 1794479429.1544154.
  Recorded CodePipeline timestamps in this corpus carry at most three fractional digits (e.g. 1793700369.272). The instants above are per-run clock values and are not the requirement; the fractional precision of the serialized epoch-seconds number is.

No status or error-code divergence: the operation answers 200 as recorded, so this is a response-body fidelity fault only.

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json - ListPipelines (output.pipelines: PipelineList; paginated nextToken/maxResults; errors InvalidNextTokenException, ValidationException, both 400), shape PipelineSummary (members name, version, pipelineType, executionMode, created: Timestamp, updated: Timestamp) and enum PipelineType (values V1, V2).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture codepipeline-list-pipelines-membership-and-pagination against it.
3. The fixture creates two pipelines with CreatePipeline, then at step task#8 calls ListPipelines.
4. Read each PipelineSummary in the response: pipelineType comes back 'V1' where the recording reports 'V2' for a pipeline created through CreatePipeline.
5. Read created and updated on the same summaries: they are epoch-seconds numbers with six or seven fractional digits, where every recorded CodePipeline timestamp in the corpus has at most three (millisecond resolution).
