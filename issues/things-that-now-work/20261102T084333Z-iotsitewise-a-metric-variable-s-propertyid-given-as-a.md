Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.sitewise-asset-model-active-waiter-and-a-hand-rolled-poll-agree iotsitewise.sitewise-aws-cli-created-asset-model-is-describable-through-the-sdk iotsitewise.sitewise-create-asset-model-rejects-a-property-name-in-propertyid-with-a-hierarchyid
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.sitewise-asset-model-active-waiter-and-a-hand-rolled-poll-agree|run#16|AssertionError|assert variable["propertyId"] == measurement_id, (
Issue-Key: iotsitewise.sitewise-aws-cli-created-asset-model-is-describable-through-the-sdk|run#21|AssertionError|assert metric_prop["type"]["metric"]["variables"][0]["value"]["propertyId"] == by_name["ActivePower"], (
Issue-Key: iotsitewise.sitewise-create-asset-model-rejects-a-property-name-in-propertyid-with-a-hierarchyid|run#11|AssertionError|assert variable["propertyId"] == by_name["ActivePower"], (
Issue-Commit: 86ceea0f39fca17f15a7090d9d9227e10094079b

# A metric variable's propertyId given as a property NAME is never resolved to the property's server-assigned id, so DescribeAssetModel echoes the name back

> _Note: this issue was identified against the code base at commit `86ceea0f39fca17f15a7090d9d9227e10094079b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssetModel

## Description
Cause 1 of 1: a same-model metric/transform variable whose propertyId is submitted as a property NAME is stored verbatim and read back as that name, instead of being resolved into the referenced property's generated uuid while the model settles.
AWS: Smithy: VariableValue.propertyId (Macro, @length 1..256) documented 'The ID of the property to use as the variable. You can use the property name if it's from the same asset model.' - the name is an accepted INPUT spelling only. Each AssetModelProperty gets a server-assigned id (CustomID, uuid pattern) when the model is created, and by the time the model reaches ACTIVE, DescribeAssetModel reports the metric's variables[].value.propertyId as that resolved uuid, never as the submitted name. This is the state the model's ACTIVE transition - and therefore the asset-model-active waiter - is defined to publish: the waiter returning means the reference is resolved. It holds whichever client wrote the model: the AWS CLI's identical create is described through the SDK with the same resolved uuid.
Emulator: DescribeAssetModel answers 200 for an ACTIVE model whose metric variable still reads propertyId='ActivePower' (the submitted property name) rather than that measurement property's uuid; the waiter returns ACTIVE with the reference unresolved.
Why: a client reads the model back to discover the ids it must address - BatchPutAssetPropertyValue, GetAssetPropertyValue and every downstream variable reference are keyed by property id, and comparisons of the model's dependency graph are made on ids. Getting a name where an id is contracted makes those reads 404 or silently mismatch, and makes a model written by one tool (CLI) un-joinable with the same model read by another (SDK). It also hides whether the reference resolved at all: a name echo is indistinguishable from an unresolvable reference.
Fix: the CreateAssetModel/UpdateAssetModel settle path that assigns property ids must rewrite every VariableValue.propertyId (and hierarchyId) from its input spelling (name, or externalId: prefix) to the resolved id before the model is published as ACTIVE, so the DescribeAssetModel projection reports resolved ids.
Traces: iotsitewise.sitewise-asset-model-active-waiter-and-a-hand-rolled-poll-agree; iotsitewise.sitewise-aws-cli-created-asset-model-is-describable-through-the-sdk; iotsitewise.sitewise-create-asset-model-rejects-a-property-name-in-propertyid-with-a-hierarchyid (this check fails on the same-model resolution assertion before reaching its own refusal assertion).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator from this checkout: 54/87 passed, all 33 failures in iotsitewise. Three checks ground this cause, each creating a model with a measurement named ActivePower plus a metric avg(power) whose variable value is {propertyId: 'ActivePower'}:

1) iotsitewise.sitewise-asset-model-active-waiter-and-a-hand-rolled-poll-agree - AssertionError: the waiter returned before the reference was resolved: the metric variable still reads 'ActivePower', not the measurement's id '<uuid>'.
2) iotsitewise.sitewise-aws-cli-created-asset-model-is-describable-through-the-sdk - AssertionError: the CLI's property-name reference resolved to 'ActivePower', not to the measurement id '<uuid>' the SDK reports.
3) iotsitewise.sitewise-create-asset-model-rejects-a-property-name-in-propertyid-with-a-hierarchyid - AssertionError: the same-model propertyId came back as 'ActivePower'; real AWS resolves the property NAME 'ActivePower' into that property's uuid '<uuid>'.

The uuids quoted are each run's generated value; the requirement is that the described propertyId is the referenced property's server-assigned id - the same value DescribeAssetModel reports as that property's own id - not the submitted name.

Model grounding: VariableValue.propertyId = Macro, documented 'You can use the property name if it's from the same asset model'; AssetModelProperty.id = CustomID with the uuid pattern; AssetModelStatus.state ACTIVE is the settled/published state.

## Steps to Reproduce
1. POST /asset-models with assetModelProperties = [{name: ActivePower, dataType: DOUBLE, type: {measurement: {}}}, {name: ActivePowerAvg, dataType: DOUBLE, type: {metric: {expression: 'avg(power)', variables: [{name: power, value: {propertyId: 'ActivePower'}}], window: {tumbling: {interval: '5m'}}}}}].
2. Wait for the model to reach ACTIVE (SDK waiter or a poll of GET /asset-models/{id}).
3. GET /asset-models/{id} and read the ActivePowerAvg property's type.metric.variables[0].value.propertyId - observe the literal 'ActivePower' instead of the uuid reported as the ActivePower measurement's own id.
4. Re-replay the three checks listed in trace_ids to confirm.
