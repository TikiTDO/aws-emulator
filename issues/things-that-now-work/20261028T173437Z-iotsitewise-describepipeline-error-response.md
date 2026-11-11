Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# DescribePipeline skips the workspace-existence check its sibling ListPipelines documents as the rule for every workspace-scoped read

_Filed: 2026-10-28T17:34:37Z_
_Operation: DescribePipeline | Class: error-response_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribePipeline addresses a pipeline by (workspaceName, pipelineName) through `crate::state::pipeline_key` and refuses with a 404 that names the PIPELINE. It never resolves the workspace, so a request naming a workspace this store does not hold is reported as a missing pipeline instead of a missing workspace. ListPipelines — the other read of the same resource — resolves the workspace first and states that as the crate-wide rule in a comment, and CreatePipeline resolves it too (which is why a pipeline can only ever be stored under a workspace the registry holds). The result is that two reads of the same path disagree about which resource is absent: GET /workspaces/no-such-ws/pipelines answers "Workspace with ID no-such-ws not found" while GET /workspaces/no-such-ws/pipelines/p answers "Pipeline with ID p not found".

UpdatePipeline (src/ops/update_pipeline.rs:174) and DescribePipelineExecution (src/ops/describe_pipeline_execution.rs:210) have the same shape; DeletePipeline (src/ops/delete_pipeline.rs:162-168) too.

## Evidence

The rule, stated and implemented by the other read of the same resource:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_pipelines.rs:165-177
    "// The listing is scoped to ONE workspace, so a workspace this service holds no
     // record of is the modelled refusal rather than an empty page, which would
     // report absence as success. The workspace registry is the one place that says
     // which workspaces exist — a create inside a workspace resolves it there too, so
     // no pipeline can be stored under a workspace the registry does not hold.
     // Resolved BEFORE the page is cut, so a request that is wrong in both ways
     // reports the resource that is absent.
     if !store.workspaces.contains_key(&input.workspace_name) {
         return Err(not_found(crate::state::not_found_message(
             crate::state::RESOURCE_WORKSPACE,
             &input.workspace_name,
         )));
     }"

The same check on the create of the resource:
  src/ops/create_pipeline.rs:199-213 — "// The pipeline is created IN the workspace the path names ... so a workspace this store holds no record of is the same absence every other workspace-scoped operation of this service reports ... if !store.workspaces.contains_key(&input.workspace_name) {"

The handler that skips it:
  src/ops/describe_pipeline.rs:174-181
    "let key = crate::state::pipeline_key(&input.workspace_name, &input.pipeline_name);
     // A read, so nothing here writes ...
     let store = state.lock();
     let record = store
         .pipelines
         .get(&key)
         .ok_or_else(|| not_found(&input.pipeline_name))?;"
  src/ops/describe_pipeline.rs:210-219 — "/// HTTP 404 `ResourceNotFoundException` naming the pipeline the request addressed. ... message: crate::state::not_found_message(crate::state::RESOURCE_PIPELINE, pipeline_name)"

The crate already treats "the absent workspace is reported as the workspace" as its answer for other workspace-scoped entry points: tests/all/refusal_message_rule.rs:274-287 pins "Workspace with ID {UNKNOWN_WORKSPACE} not found" for ListTimeSeries and DeleteTimeSeries. No suite sends DescribePipeline an unknown workspace (tests/all/describe_pipeline_reads.rs, tests/all/pipeline_lifecycle.rs), so nothing pins the current wording.

## Steps to Reproduce

1. CreateWorkspace "plant-a" and CreatePipeline "p" in it.
2. GET /workspaces/no-such-ws/pipelines/p (DescribePipeline) → 404 ResourceNotFoundException "Pipeline with ID p not found".
3. GET /workspaces/no-such-ws/pipelines (ListPipelines) → 404 ResourceNotFoundException "Workspace with ID no-such-ws not found".
4. The two reads of the same unknown workspace disagree about which resource is absent.
