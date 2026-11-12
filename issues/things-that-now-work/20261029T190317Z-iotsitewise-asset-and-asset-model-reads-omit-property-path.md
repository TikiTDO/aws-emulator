Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-asset-lifecycle-create-active-delete
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-asset-lifecycle-create-active-delete iotsitewise-create-asset-model-composite-model-lifecycle
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|DescribeAsset|2xx->2xx|$.assetCompositeModelSummaries
Issue-Key: iotsitewise|DescribeAsset|2xx->2xx|$.assetCompositeModels
Issue-Key: iotsitewise|DescribeAsset|2xx->2xx|$.assetProperties[*].notification
Issue-Key: iotsitewise|DescribeAsset|2xx->2xx|$.assetProperties[*].path
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelCompositeModelSummaries[*].path[*]
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelCompositeModels[*]
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelProperties[*].path
Issue-Key: iotsitewise|DescribeAssetModelCompositeModel|2xx->2xx|$.assetModelCompositeModelPath[*]
Issue-Key: iotsitewise|DescribeAssetModelCompositeModel|2xx->2xx|$.assetModelCompositeModelProperties[*].path
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelCompositeModelSummaries
Issue-Key: iotsitewise|DescribeAssetModel|2xx->2xx|$.assetModelCompositeModels
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# Asset and asset-model reads omit property path and composite-model collections

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssetModel

## Description
Four read paths over assets and asset models project an incomplete response shape: the structured `path` members are never emitted, the composite-model collections are omitted when empty instead of serialized as empty lists, the property-level notification block is dropped, and where a path IS emitted it starts at the composite model instead of the root. Related open issue 20261029T020020Z-iotsitewise-describeassetmodel-wrong-behavior covers composite-model PROPERTY RESOLUTION (a component-model-based composite model reporting no properties); it does not cover the missing path/notification members or the empty-collection projection described here.

Cause 1 of 4: DescribeAsset omits assetCompositeModelSummaries, assetCompositeModels and each property's notification and path.
AWS: Smithy: DescribeAsset output members assetCompositeModels (AssetCompositeModels) and assetCompositeModelSummaries (AssetCompositeModelSummaries); assetProperties (AssetProperties, @required) of AssetProperty, whose members include notification (PropertyNotification: topic @required + state @required, PropertyNotificationState) and path (AssetPropertyPath, documented "The structured path to the property from the root of the asset"). AWS serializes the composite-model collections as present-and-empty for an asset whose model has no composite models, reports every asset property's notification block (state DISABLED by default, with the MQTT topic IoT SiteWise would publish to, of the form $aws/sitewise/asset-models/<assetModelId>/assets/<assetId>/properties/<propertyId>), and reports path as the list of {id, name} hops from the asset root down to the property (first hop = the asset's own id and name).
Emulator: answers HTTP 200 but $.assetCompositeModelSummaries and $.assetCompositeModels are absent (expected []), and each property carries neither notification nor path (both None).
Why: a client cannot tell "no composite models" from "field not returned", cannot discover the MQTT topic or notification state for a property without a separate UpdateAssetProperty round trip, and cannot render or address a property by its structured path — which is the only way to disambiguate same-named properties under different composite models.
Fix: the DescribeAsset handler/serializer — project the composite-model collections as empty lists when the asset has none, derive each property's notification (state + topic built from model/asset/property ids) and compute each property's path from the asset root.
Traces: iotsitewise-asset-lifecycle-create-active-delete#task#4.

Cause 2 of 4: DescribeAssetModel emits a composite-model summary path that starts at the composite model instead of the asset model root, omits assetModelProperties[].path, and returns an assetModelCompositeModels entry where AWS returns none.
AWS: Smithy: DescribeAssetModel output assetModelCompositeModelSummaries (AssetModelCompositeModelSummary with path: AssetModelCompositeModelPath, documented "The path that includes all the pieces that make up the composite model") and assetModelProperties (AssetModelProperty with path: AssetModelPropertyPath, "The structured path to the property from the root of the asset model"). A composite-model path's FIRST hop is the root asset model itself ({id: <assetModelId>, name: <assetModelName>}), followed by each composite-model hop; a property's path is the root asset model hop followed by the hops down to the property. Recorded behavior: the summary path begins with the asset model's own {id, name}, each top-level assetModelProperty carries a two-hop path (root model, then the property), and assetModelCompositeModels is not populated with the composite model that assetModelCompositeModelSummaries already describes.
Emulator: answers HTTP 200 but $.assetModelCompositeModelSummaries[0].path[0] is the COMPOSITE model's own {id, name} (the root hop is missing, so every hop is shifted by one), $.assetModelProperties[0].path is absent, and $.assetModelCompositeModels[0] is an extra entry (a fully expanded composite model with its properties) where AWS returned nothing.
Why: a client walking a path to build the fully-qualified property name gets a name that omits the model root, and a client that iterates assetModelCompositeModels sees a composite model twice (once as a summary, once as a full entry) and double-counts it.
Fix: the DescribeAssetModel handler/serializer — build composite-model and property paths rooted at the asset model, and populate assetModelCompositeModels / assetModelCompositeModelSummaries per the service's rule rather than filling both.
Traces: iotsitewise-create-asset-model-composite-model-lifecycle#task#4.

Cause 3 of 4: DescribeAssetModelCompositeModel's path members are likewise not rooted at the asset model.
AWS: Smithy: DescribeAssetModelCompositeModel output assetModelCompositeModelPath (AssetModelCompositeModelPath, @required) and assetModelCompositeModelProperties (AssetModelProperties, each with path). The composite-model path's first hop is the root asset model {id, name}, then the composite model; each composite-model property's path is root asset model -> composite model -> property.
Emulator: answers HTTP 200 but $.assetModelCompositeModelPath[0] is the composite model's own {id, name} (root hop missing) and $.assetModelCompositeModelProperties[0].path is absent entirely.
Why: the same path breakage as cause 2, on the read a client uses specifically to resolve a composite model's fully-qualified property paths; with path absent, nothing in the response states where a composite property sits in the model tree.
Fix: the DescribeAssetModelCompositeModel handler/serializer must build both paths from the shared root-to-node path resolver rather than from the composite node alone.
Traces: iotsitewise-create-asset-model-composite-model-lifecycle#task#5.

Cause 4 of 4: after the composite model is deleted, DescribeAssetModel omits the now-empty composite-model collections and still omits assetModelProperties[].path.
AWS: same DescribeAssetModel members as cause 2. With the model's only composite model deleted, AWS answers HTTP 200 with assetModelCompositeModels and assetModelCompositeModelSummaries present and empty, and the surviving top-level properties still carrying their root-rooted path.
Emulator: answers HTTP 200 with $.assetModelCompositeModelSummaries missing (expected []), $.assetModelCompositeModels missing (expected []) and $.assetModelProperties[0].path missing.
Why: a client that polls the model after a composite-model delete to confirm the deletion cannot distinguish "collection now empty" from "collection not serialized", so the delete appears unconfirmable.
Fix: the DescribeAssetModel serializer must emit the composite-model collections as empty lists rather than omitting them, i.e. the same empty-collection projection rule as cause 1.
Traces: iotsitewise-create-asset-model-composite-model-lifecycle#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (score 72.5%, 11/40 steps mismatched):

1) iotsitewise-asset-lifecycle-create-active-delete#task#4 DescribeAsset — expected 200, got 200; $.assetCompositeModelSummaries: missing (expected []); $.assetCompositeModels: missing (expected []); $.assetProperties[0].notification: missing (expected {state: DISABLED, topic: $aws/sitewise/asset-models/<assetModelId>/assets/<assetId>/properties/<propertyId>}); $.assetProperties[0].path: missing (expected the root asset hop {id: <assetId>, name: <assetName>} followed by the property hop).
2) iotsitewise-create-asset-model-composite-model-lifecycle#task#4 DescribeAssetModel — expected 200, got 200; $.assetModelCompositeModelSummaries[0].path[0]: expected the root asset-model hop {id: <assetModelId>, name: <assetModelName>}, got the composite model's own hop {id: <compositeModelId>, name: 'Motor'}; $.assetModelProperties[0].path: missing (expected root-model hop + property hop); $.assetModelCompositeModels[0]: extra (emulator returned a fully expanded composite model entry where AWS returned none).
3) iotsitewise-create-asset-model-composite-model-lifecycle#task#5 DescribeAssetModelCompositeModel — expected 200, got 200; $.assetModelCompositeModelPath[0]: expected the root asset-model hop, got the composite model's own hop; $.assetModelCompositeModelProperties[0].path: missing (expected root-model hop -> composite-model hop -> property hop).
4) iotsitewise-create-asset-model-composite-model-lifecycle#task#7 DescribeAssetModel (after DeleteAssetModelCompositeModel) — expected 200, got 200; $.assetModelCompositeModelSummaries: missing (expected []); $.assetModelCompositeModels: missing (expected []); $.assetModelProperties[0].path: missing.

The specific ids/names above are this recording's values filling the template; the requirement is the SHAPE — path rooted at the asset/asset model, notification present with its state and derived topic, composite-model collections present-and-empty.

Model grounding: AssetProperty.path = AssetPropertyPath ("structured path to the property from the root of the asset"); AssetProperty.notification = PropertyNotification{topic @required, state @required}; AssetModelProperty.path = AssetModelPropertyPath; AssetModelCompositeModelSummary.path = AssetModelCompositeModelPath; DescribeAssetModelCompositeModel.assetModelCompositeModelPath is @required.

## Steps to Reproduce
1. Replay fixture iotsitewise-asset-lifecycle-create-active-delete; at step task#4 GET /assets/{assetId} (DescribeAsset) for an ACTIVE asset whose model has one measurement property — observe no assetCompositeModels/assetCompositeModelSummaries members and no notification/path on the property.
2. Replay fixture iotsitewise-create-asset-model-composite-model-lifecycle; at step task#4 GET /asset-models/{assetModelId} (DescribeAssetModel) after creating a CUSTOM composite model — observe the summary path's first hop is the composite model rather than the asset model, assetModelProperties[].path is absent, and an extra assetModelCompositeModels entry is returned.
3. Same fixture, step task#5 GET /asset-models/{assetModelId}/composite-models/{compositeModelId} (DescribeAssetModelCompositeModel) — observe assetModelCompositeModelPath[0] is the composite model itself and the composite properties carry no path.
4. Same fixture, step task#7 GET /asset-models/{assetModelId} after deleting the composite model — observe the composite-model collections are omitted instead of serialized as [].
