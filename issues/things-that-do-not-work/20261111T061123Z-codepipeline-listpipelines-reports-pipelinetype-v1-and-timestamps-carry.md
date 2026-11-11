Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-list-pipelines-membership-and-pagination
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-list-pipelines-membership-and-pagination codepipeline-s3-source-manual-approval-run-succeeds
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-5faa979c"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-5faa979c"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-a-5faa979c"].updated
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-e0960bb3"].created
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-e0960bb3"].pipelineType
Issue-Key: codepipeline|ListPipelines|2xx->2xx|$.pipelines[name="sim2real-probe-codepipeline-lp-b-e0960bb3"].updated
Issue-Key: codepipeline|PutApprovalResult|2xx->2xx|$.approvedAt
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# CodePipeline ListPipelines reports pipelineType V1 and timestamps carry sub-millisecond precision

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelines

## Description
Two scalar-serialization faults in codepipeline responses: the pipeline summary projection reports the wrong pipeline type, and every timestamp the service emits carries more fractional precision than CodePipeline's wire form. 2 causes.

Cause 1 of 2 - ListPipelines reports pipelineType V1 for pipelines AWS reports as V2 (fixture codepipeline-list-pipelines-membership-and-pagination, step task#8).
AWS: Smithy ListPipelines output.pipelines is a PipelineList of PipelineSummary {name, version, pipelineType (PipelineType enum, values V1|V2), executionMode, created, updated}. The pipelineType member is documented as distinguishing V1 pipelines ('a JSON structure that contains standard pipeline, stage, and action-level parameters') from V2 pipelines ('the same structure as a V1 type, along with additional parameters for release safety and trigger configuration'), with the note that including V2 parameters when creating or updating a pipeline results in a V2 pipeline. Recorded behavior: for both pipelines the corpus creates with CreatePipeline, the recorded ListPipelines response reports pipelineType 'V2'. Hypothesis: the emulator defaults the stored pipeline's type to V1 instead of to the type the service assigns a newly created pipeline, so the summary projection faithfully reports a wrong stored value.
Emulator: both summaries report pipelineType 'V1'.
Why: pipelineType is how a client decides whether the V2-only declaration features (triggers, pipeline-level variables, stage conditions, release safety) are available on a pipeline and what it is billed as. A client that enumerates pipelines and branches on the summary's type will refuse to write a V2-only declaration to a pipeline that actually accepts one, or will misreport the fleet's pipeline mix.
Fix: the pipeline state's default pipelineType on creation, and the PipelineSummary projection in the ListPipelines handler that reads it.
Traces: codepipeline-list-pipelines-membership-and-pagination#task#8.

Cause 2 of 2 - timestamps are serialized with sub-millisecond precision (fixture codepipeline-s3-source-manual-approval-run-succeeds, step task#9, PutApprovalResult).
AWS: Smithy PutApprovalResult output.approvedAt targets Timestamp; codepipeline speaks awsJson1_1, whose default timestamp form is epoch-seconds as a JSON number. Recorded behavior: every timestamp in the corpus's recorded CodePipeline responses carries at most three fractional digits, i.e. millisecond resolution (for example a recorded ListActionExecutions lastUpdateTime of 1793700369.272). Millisecond resolution is therefore the wire form of a CodePipeline timestamp; the instant itself is a per-run value and is not a requirement.
Emulator: it emits the full precision of its internal clock - approvedAt 1794376957.3722878 (seven fractional digits). The same fault shows on ListPipelines created/updated (1794377075.45266, 1794377075.4737728) and on GetPipelineState created/updated/lastStatusChange (1794376952.3382232, 1794376952.6549876, 1794376957.3722878), so it is one serializer, not one operation. Hypothesis: a float seconds-since-epoch is written straight out instead of being truncated to milliseconds.
Why: a client that round-trips or compares these numbers against AWS-recorded values, or that formats them with a millisecond-resolution parser, sees a value shape AWS never emits; equality and digest comparisons over recorded responses fail even when the instant is irrelevant.
Fix: the service's timestamp serializer (the epoch-seconds encoding shared by the codepipeline response serializers) must round/truncate to millisecond resolution, matching the wire form, rather than emitting the clock's native precision.

Related open issue: 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md reports pipelineType V1 on the PipelineDeclaration responses (GetPipeline/CreatePipeline/UpdatePipeline) and owns those replay keys; it does not cover the PipelineSummary projection in ListPipelines, nor the timestamp precision fault, and the replay reports both clusters here as unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, /__admin/health 200). Both clusters: verdict mismatch, expected status 200, actual status 200.

Cause 1 - codepipeline-list-pipelines-membership-and-pagination#task#8, ListPipelines.
  $.pipelines[name="sim2real-probe-codepipeline-lp-a-5faa979c"].pipelineType: value_mismatch - expected 'V2', got 'V1'.
  $.pipelines[name="sim2real-probe-codepipeline-lp-b-e0960bb3"].pipelineType: value_mismatch - expected 'V2', got 'V1'.
  Same step, Cause 2's fault: .created / .updated reported as value_mismatch against a non-asserted recorded value, got 1794377075.45266 and 1794377075.4737728 (6-7 fractional digits).
  The pipeline names above are per-run generated names, not requirements; the requirement is that a pipeline created through CreatePipeline is summarized as V2.

Cause 2 - codepipeline-s3-source-manual-approval-run-succeeds#task#9, PutApprovalResult.
  $.approvedAt: value_mismatch - got 1794376957.3722878, seven fractional digits, against a recorded timestamp whose value is not asserted. Recorded CodePipeline timestamps in this corpus carry at most three fractional digits (e.g. 1793700369.272).
  Corroborating steps for the same serializer (filed under their own issues for their other faults): codepipeline-list-pipelines-membership-and-pagination#task#8 created/updated, codepipeline-s3-source-manual-approval-run-succeeds#task#12 created/updated/lastStatusChange.
  Per-run values, not requirements: the instants themselves. The requirement is the fractional precision of the serialized epoch-seconds number.

Model consulted: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json - ListPipelines (output.pipelines: PipelineList, paginated nextToken/maxResults; errors InvalidNextTokenException, ValidationException), PutApprovalResult (output.approvedAt: Timestamp; errors ActionNotFoundException, ApprovalAlreadyCompletedException, InvalidApprovalTokenException, PipelineNotFoundException, StageNotFoundException, ValidationException), shapes PipelineSummary (member pipelineType doc) and PipelineType (values V1, V2).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture codepipeline-list-pipelines-membership-and-pagination. At step task#8 call ListPipelines: each PipelineSummary for a pipeline the fixture created with CreatePipeline reports pipelineType 'V1' where the recording reports 'V2', and created/updated come back with six or more fractional digits.
3. Replay fixture codepipeline-s3-source-manual-approval-run-succeeds. At step task#9 call PutApprovalResult with the open approval's token and read approvedAt: it is an epoch-seconds number with seven fractional digits, where CodePipeline emits at most three.
4. Compare any timestamp in the emulator's codepipeline responses (GetPipelineState created/updated/lastStatusChange, ListPipelines created/updated, PutApprovalResult approvedAt) with the recorded ones: the recorded values are millisecond-resolution, the emulator's are not.
