Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# Targets identified by resourceTags (or narrowed by filters) resolve to nothing, so ListExperimentResolvedTargets reports an empty resolution for a tag-based experiment

_Filed: 2026-10-24T00:04:50Z_
_Operation: ListExperimentResolvedTargets | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An experiment's resolved targets are computed once at StartExperiment by `state.rs::resolve_targets`, which only walks `target.resourceArns`. A target that identifies its resources by `resourceTags` contributes NOTHING to the resolution, and `target.filters` are ignored entirely. Expected (real FIS): tag-identified targets are resolved at experiment start against the resources that carry those tags in the account/region, narrowed by the target's `filters` and scoped by `selectionMode`, and ListExperimentResolvedTargets reports one ResolvedTarget per resolved resource (`targetInformation` naming it). Actual: the operation answers 200 with `resolvedTargets: []` for such an experiment, even when the emulator's own stores hold matching resources (the FIS crate already reads another service's store — it resolves the execution role against the IAM store in StartExperiment — so tag resolution against emulated resources is reachable).

A second, consequential symptom: because the resolution is empty, `experimentOptions.emptyTargetResolutionMode` (stored and echoed back, default `fail`) is never applied — the run still reaches `completed`, where FIS fails an experiment whose target resolution came back empty under the `fail` mode. The mode is consulted nowhere in `state.rs` (`run_plan` decides the outcome from the permission check alone).

## Evidence

Smithy model: ListExperimentResolvedTargets -> list<ResolvedTarget> {resourceType, targetName, targetInformation}; ExperimentTemplateTarget carries `resourceTags` and `filters` alongside `resourceArns`; ExperimentOptions.emptyTargetResolutionMode is modelled `fail | skip`. FIS documents a target as "a specific resource ... or one or more resources that match criteria that you specify, for example, resources that have specific tags", and documents emptyTargetResolutionMode=fail as failing the experiment when target resolution is empty.
Handler/state: src/ops/list_experiment_resolved_targets.rs reads `experiment.resolved_targets` only; src/state.rs `resolve_targets` iterates `target.resource_arns` and documents the omission ("A target that identifies its resources by `resourceTags` ... identifies nothing here"); `run_plan` never reads `empty_target_resolution_mode`.
Tests: tests/all/list_experiment_resolved_targets.rs includes a row with `resourceTags` and asserts the emulator's current zero-resolution outcome, i.e. the divergence is pinned by a test rather than fixed; no test asserts tag-based resolution or the fail-on-empty mode.

## Steps to Reproduce

1. Boot the emulator; create an EC2 instance (or an IAM role) carrying tag Stage=test through the corresponding emulated service.
2. create_experiment_template with targets={'web': {'resourceType': 'aws:ec2:instance', 'resourceTags': {'Stage': 'test'}, 'selectionMode': 'ALL'}} and a wait action, then start_experiment.
3. Call list_experiment_resolved_targets(experimentId=...) — observe resolvedTargets == [] instead of one ResolvedTarget per tagged resource.
4. Poll get_experiment until terminal — observe state.status 'completed' although experimentOptions.emptyTargetResolutionMode is 'fail' and the resolution was empty.
