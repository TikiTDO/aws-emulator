Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity|run#29|AssertionError|assert in_flight_snapshots, (
Issue-Commit: 181b4db8a49f9f99cf56c1b2f4aacf19bf82375e

# opensearch: DescribeDomainChangeProgress never reports ChangeProgressStages for an in-flight domain configuration change (1 cause)

> _Note: this issue was identified against the code base at commit `181b4db8a49f9f99cf56c1b2f4aacf19bf82375e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomainChangeProgress

## Description
One cause.

Cause 1 of 1: a blue/green domain configuration change (here a scale-out via UpdateDomainConfig) is accepted and eventually completes, but DescribeDomainChangeProgress reports an EMPTY ChangeProgressStages list for the entire time the change is in flight, and an empty list when it finishes.

AWS: Smithy model (opensearch, DescribeDomainChangeProgress, GET /2021-01-01/opensearch/domain/{DomainName}/progress) returns ChangeProgressStatusDetails, whose member `ChangeProgressStages` targets ChangeProgressStageList and is documented as "The specific stages that the domain is going through to perform the configuration change"; each ChangeProgressStage carries Name (@length 1..64), Status (@length 1..256), Description and LastUpdated, and the sibling `TotalNumberOfStages` is documented as "The total number of stages required for the configuration change". So while a change is in flight AWS reports a non-empty stage list whose entries advance in Status and LastUpdated as the blue/green deployment proceeds.
Emulator: every poll taken while the change was in flight returned ChangeProgressStages as an empty list, and the final poll also reported no stages — the replay recorded "no poll observed a non-empty ChangeProgressStages while the change was in flight (1 in-flight polls); final stages ()". The operation itself answers 200; only the stage list is empty.
Why: a client that drives a change to completion by watching per-stage progress (the documented purpose of this operation) has nothing to watch: it cannot tell which phase a blue/green deployment is in, cannot surface progress to a user, and cannot distinguish 'stages not yet started' from 'this change has no stages'. Code that waits for a specific stage Name never advances and times out even though the change succeeds.
Fix: the DescribeDomainChangeProgress handler and the domain change-progress state it reads — record the stage sequence when a configuration change is admitted, advance each stage's Status/LastUpdated as the change progresses, and serialize ChangeProgressStages (with TotalNumberOfStages consistent with it) in ChangeProgressStatusDetails.
Traces: opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity.

## Evidence
Expected (Smithy ChangeProgressStatusDetails): while the change is in flight, ChangeProgressStages is a non-empty list of ChangeProgressStage entries, each with Name, Status, Description and LastUpdated, consistent with TotalNumberOfStages.

Actual (emulator), verbatim from the replay:
  AssertionError: no poll observed a non-empty ChangeProgressStages while the change was in flight (1 in-flight polls); final stages ()

DescribeDomainChangeProgress answered 200 on every poll; ChangeProgressStages was empty on all of them, including the terminal one. The number of polls and the domain/change ids are per-run values, not part of the requirement.

## Steps to Reproduce
1. CreateDomain (EngineVersion OpenSearch_2.11, single t3.small.search data node, gp3 10 GiB) and wait for Processing=false.
2. Index a few documents through the domain's data plane so the change has data to carry.
3. Call UpdateDomainConfig to scale the cluster out (e.g. raise ClusterConfig.InstanceCount), triggering a blue/green change.
4. Poll DescribeDomainChangeProgress(DomainName=<domain>) repeatedly while DescribeDomain still reports Processing=true.
5. Observe ChangeProgressStages is an empty list on every in-flight poll and on the final poll, instead of a non-empty advancing stage list.
