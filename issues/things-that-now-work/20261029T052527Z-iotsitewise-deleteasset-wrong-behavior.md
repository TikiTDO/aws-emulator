Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8d0d74e15d09000e4789d5e17c7af92005cbf7c1

# DeleteAsset leaves every action/execution `resolveTo` pointing at the removed asset, the dangling reference ExecuteAction refuses to store and forget_asset's own rule drops for projects

_Filed: 2026-10-29T05:25:27Z_
_Operation: DeleteAsset | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8d0d74e15d09000e4789d5e17c7af92005cbf7c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteAsset` removes the asset through `crate::state::forget_asset`, which drops everything the state module declares to be "keyed off" an asset — its external id, its property series, its property aliases, its hierarchy associations and its project memberships — but NOT the `resolveTo` reference an executed action and its execution carry.

Those references are read back by `ListComputationModelResolveToResources` (through `computation_model_resolved_assets`), and by `DescribeExecution` / `ListExecutions`, straight off the stored records with no existence filter. So after deleting an asset that a computation model's action resolved to, `GET /computation-models/{id}/resolve-to-resources` keeps reporting that asset id as a resource the model "resolves to", and the execution records keep naming it — an asset nothing in the service can address any more.

That is exactly the state the crate refuses to be put into by a write: `ExecuteAction` validates `resolveTo.assetId` against the asset store and answers 404 for an id no asset holds, stating that the refusal is "the refusal the listings scoped by that same reference give". And it is the state `forget_asset` explicitly prevents for the OTHER reference to an asset it knows about — a project's membership — with a comment that states the rule in general terms. The sibling `forget_computation_model` does the symmetric cleanup for actions/executions on the other side (their TARGET), citing `forget_asset` as its precedent, so only the `resolveTo` side is left dangling.

## Evidence

The delete, services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/delete_asset.rs:245-247:
```
        // The asset and everything keyed off it (its property series, its aliases, its
        // hierarchy associations) go away, so no later read can still see it.
        store.forget_asset(&id);
```

What `forget_asset` actually drops — src/state.rs:5359-5386 — and the rule it states for the one asset reference it does clean up:
```
    /// Forget an asset and everything keyed off it: its external-id entry, its
    /// property series, its property aliases, its hierarchy associations and its
    /// membership of every IoT SiteWise Monitor project.
    pub fn forget_asset(&mut self, asset_id: &str) -> Option<AssetRecord> {
        ...
        // A project's membership names assets, so an asset this service no longer
        // holds is in no project: dropping the entry here is what keeps
        // `ListProjectAssets` from reporting an asset nothing can address.
        for project in self.projects.values_mut() {
            project.asset_ids.remove(asset_id);
        }
```
(nothing in the body touches `self.actions` or `self.executions`.)

The sibling forget that DOES drop action/execution records, and names forget_asset as its precedent — src/state.rs:5547-5578:
```
    /// An action and an execution each NAME the target resource they were taken
    /// on, ... so a model this service no longer holds can leave none of them behind
    /// for a listing to report or for the [`MAX_ACTIONS`] budget to count — the
    /// same reason [`Self::forget_asset`] drops what is keyed off an asset.
    pub fn forget_computation_model(...) {
        ...
        self.actions.retain(|_, action| { !action.targets(TARGET_RESOURCE_TYPE_COMPUTATION_MODEL, computation_model_id) });
        self.executions.retain(|_, execution| { !execution.targets(...) });
```

The write that refuses the same dangling reference — src/ops/execute_action.rs:314-324:
```
        // "The detailed resource this action resolves to" is a reference to one
        // asset, so an id no asset of this store holds resolves to nothing — the
        // refusal the listings scoped by that same reference give.
        if let Some(resolve_to) = input.resolve_to.as_ref() {
            if !store.assets.contains_key(&resolve_to.asset_id) {
                return Err(not_found(crate::state::not_found_message(
                    crate::state::RESOURCE_ASSET,
                    &resolve_to.asset_id,
                )));
```

The read that then reports it unfiltered — src/state.rs:3135-3156:
```
    pub fn computation_model_resolved_assets(&self, computation_model_id: &str) -> Vec<String> {
        let from_executions = self.executions.values().filter(...).filter_map(|record| record.resolve_to.as_ref());
        let from_actions = self.actions.values().filter(...).filter_map(|record| record.resolve_to.as_ref());
        from_executions.chain(from_actions).map(|resolve_to| resolve_to.asset_id.clone()).collect::<BTreeSet<String>>()...
```
and its caller, src/ops/list_computation_model_resolve_to_resources.rs:186 (`let resolved = store.computation_model_resolved_assets(&input.computation_model_id);`), which maps each id straight into `ResolveTo { asset_id }` at lines 213-215.

Test coverage: nothing pins the current behaviour. tests/all/computation_model_resolve_to_resources.rs never deletes an asset (grep for `delete` in that file returns no DeleteAsset call); its live-API test `an_action_executed_through_the_api_shows_in_that_models_resolved_resources` (line 659) only executes actions and lists. tests/all/hierarchies_etags_and_refusals.rs covers DeleteAsset's hierarchy refusals, not its reference cleanup.

## Steps to Reproduce

1. CreateAssetModel + CreateAsset, and wait for the asset to report ACTIVE.
2. CreateComputationModel with a configuration whose computation type publishes an action definition; wait for it to report ACTIVE.
3. POST /actions with {"targetResource": {"computationModelId": <model>}, "actionDefinitionId": <definition>, "actionPayload": {...}, "resolveTo": {"assetId": <asset>}} -> 202 (ExecuteAction accepts it only because the asset exists, src/ops/execute_action.rs:317-324).
4. GET /computation-models/{model}/resolve-to-resources -> 200, the summary list contains that assetId.
5. DELETE /assets/{asset} -> 200 (no guard refuses it, src/ops/delete_asset.rs:224-247).
6. GET /assets/{asset} -> 404, the asset is gone.
7. GET /computation-models/{model}/resolve-to-resources again -> 200 and STILL reports resolveTo.assetId = the deleted asset; GET /executions/{executionId} likewise reports it. Expected, by the rule forget_asset states for the other asset reference it holds (src/state.rs:5379-5384: "an asset this service no longer holds is in no project: dropping the entry here is what keeps ListProjectAssets from reporting an asset nothing can address") and by forget_computation_model's symmetric cleanup: the deleted asset is reported by no listing.
