Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-cache-parameter-group-workflow
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-cache-parameter-group-workflow elasticache-catalog-describe-cache-engine-versions elasticache-catalog-describe-reserved-cache-nodes-offerings elasticache-catalog-describe-service-updates
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter[*]
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Marker
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*]
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].AllowedValues
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ChangeType
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].DataType
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].Description
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].IsModifiable
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].MinimumEngineVersion
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ParameterName
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ParameterValue
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].AllowedValues
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].ChangeType
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].DataType
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].Description
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].IsModifiable
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].MinimumEngineVersion
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].ParameterName
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].ParameterValue
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*].Source
Issue-Key: elasticache|DescribeCacheEngineVersions|2xx->2xx|$.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion[*]
Issue-Key: elasticache|DescribeReservedCacheNodesOfferings|2xx->2xx|$.DescribeReservedCacheNodesOfferingsResponse.DescribeReservedCacheNodesOfferingsResult.ReservedCacheNodesOfferings.ReservedCacheNodesOffering[*]
Issue-Key: elasticache|DescribeServiceUpdates|2xx->2xx|$.DescribeServiceUpdatesResponse.DescribeServiceUpdatesResult.ServiceUpdates.ServiceUpdate[*]
Issue-Related: 20261030T130325Z-elasticache-parameter-catalog-is-incomplete-describecacheparameters-omits-cachenodetypespecificparameters.md
Issue-Commit: 4839448a699fa31c23507e6b5ab2cdd919186720

# ElastiCache reference catalogs diverge from AWS across five Describe surfaces (6 causes)

> _Note: this issue was identified against the code base at commit `4839448a699fa31c23507e6b5ab2cdd919186720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEngineDefaultParameters

## Description
SIX net-new root causes, all service=elasticache, all verdict=mismatch, all with stub_path empty and owned_by empty in this replay (so none of them is filed elsewhere). Each one is a read-only Describe surface that answers the right HTTP status but serves the emulator's own seeded reference data instead of the catalog AWS returns: engine default parameters, a custom parameter group's parameters before and after a modify, the cache engine version catalog, the reserved-node offering catalog and the service-update catalog. The fault they share is the elasticache crate's seeded reference catalogs (and how the per-node-type/parameter lists are projected into the response), so a fixer must close all SIX causes before submitting. Two open issues are adjacent but cover neither these fixtures nor these replay keys: 20261030T130325Z-elasticache-parameter-catalog-is-incomplete-describecacheparameters-omits-cachenodetypespecificparameters.md is bound to fixture elasticache-catalog-describe-cache-parameters (DescribeCacheParameters on the *default* groups), and 20261102T083323Z-elasticache-cache-subnet-group-creation-and-engine-version.md covers only the single valkey8 CacheEngineVersionDescription/CacheEngineDescription pair in fixture elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes.

Cause 1 of 6: DescribeEngineDefaultParameters serves neither the family's default parameter set nor its node-type-specific parameters (fixture elasticache-cache-parameter-group-workflow, step task#3).
AWS: Smithy: DescribeEngineDefaultParameters(CacheParameterGroupFamily required) -> DescribeEngineDefaultParametersResult{EngineDefaults}; EngineDefaults{CacheParameterGroupFamily, Marker, Parameters: ParametersList, CacheNodeTypeSpecificParameters: CacheNodeTypeSpecificParametersList}; errors InvalidParameterValueException (400/InvalidParameterValue), InvalidParameterCombinationException (400/InvalidParameterCombination); paginated with outputToken EngineDefaults.Marker and items EngineDefaults.Parameters. The catalog CONTENT is recorded behavior only: for the requested family AWS answers 200 with the family's complete engine-default Parameters list (each member carrying ParameterName, ParameterValue, Description, Source, DataType, AllowedValues, IsModifiable, MinimumEngineVersion and, where applicable, ChangeType) plus a populated CacheNodeTypeSpecificParameters list whose members carry AllowedValues, MinimumEngineVersion and a CacheNodeTypeSpecificValues list with one CacheNodeType/Value pair per supported node type, and a Marker consistent with that page.
Emulator: 200, but CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter[0] is a different member than the family's (value_mismatch), EngineDefaults.Marker diverges, and the Parameters members diverge field by field (AllowedValues, DataType, Description, MinimumEngineVersion, ParameterName, ParameterValue, IsModifiable, ChangeType) from index 0 upward.
Why: a client that reads engine defaults to decide which parameters it may modify, what their allowed ranges are and which minimum engine version they need, gets a different parameter universe than AWS's; node-type-aware sizing has no data at all, and the diverging Marker makes a paginating client walk a page sequence that does not exist on AWS.
Fix: the elasticache crate's seeded engine-default parameter catalog and the DescribeEngineDefaultParameters handler's projection of EngineDefaults (Parameters, CacheNodeTypeSpecificParameters, Marker).
Traces: elasticache-cache-parameter-group-workflow#task#3.

Cause 2 of 6: DescribeCacheParameters on a freshly created custom parameter group returns a shorter, differently-valued parameter set (same fixture, step task#5).
AWS: Smithy: DescribeCacheParameters -> CacheParameterGroupDetails{Marker, Parameters: ParametersList, CacheNodeTypeSpecificParameters: CacheNodeTypeSpecificParametersList}; errors CacheParameterGroupNotFoundFault (404/CacheParameterGroupNotFound), InvalidParameterValueException (400/InvalidParameterValue). Recorded behavior only for content: a new custom group inherits its family's complete default parameter set, so AWS answers 200 with that full Parameters list and a populated CacheNodeTypeSpecificParameters list (the recording's first node-type-specific member carries AllowedValues '0-' and MinimumEngineVersion '7.0.0' with one CacheNodeType/Value pair per supported node type).
Emulator: 200, but CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter[0] value_mismatch and the Parameters list diverges across indexes 0..41 while indexes 42..63 are absent entirely - the inherited set is truncated and its metadata (AllowedValues, DataType, Description, IsModifiable, MinimumEngineVersion, ChangeType, ParameterName, ParameterValue) does not match the family's.
Why: a client that creates a custom group and then enumerates it to pick the parameters it wants to override never sees most of the family's parameters, so a subsequent ModifyCacheParameterGroup targets names the emulator does not list, and tuning tools that diff against the family default report a different delta than on AWS.
Fix: same component as cause 1 - the seeded family parameter catalog that CreateCacheParameterGroup copies from, plus the DescribeCacheParameters handler's list projection.
Traces: elasticache-cache-parameter-group-workflow#task#5.

Cause 3 of 6: after ModifyCacheParameterGroup, DescribeCacheParameters still serves the wrong parameter set and the wrong Source attribution (same fixture, step task#7).
AWS: as cause 2; recorded behavior only for content - a parameter the caller has modified comes back with its new ParameterValue and its Source reflecting that it was set by the user, while every unmodified member keeps the family default, and the node-type-specific list is unchanged.
Emulator: 200, with the same CacheNodeTypeSpecificParameter[0] value_mismatch and the same truncated/divergent Parameters list as cause 2, and in addition the Source member of the modified entries (Parameter[24] and Parameter[41] in this cluster) diverges from the recording.
Why: a client cannot tell which parameters it has overridden from which are engine defaults, so an idempotent configure-then-verify loop re-applies modifications or reports drift that does not exist.
Fix: same component as cause 1, plus the state the ModifyCacheParameterGroup handler writes back into the group's parameter entries (value and Source).
Traces: elasticache-cache-parameter-group-workflow#task#7.

Cause 4 of 6: DescribeCacheEngineVersions serves a legacy engine-version catalog instead of AWS's (fixture elasticache-catalog-describe-cache-engine-versions, step task#1).
AWS: Smithy: DescribeCacheEngineVersions -> CacheEngineVersionMessage{Marker, CacheEngineVersions: CacheEngineVersionList}, no modeled errors; CacheEngineVersion{Engine, EngineVersion, CacheParameterGroupFamily, CacheEngineDescription, CacheEngineVersionDescription}. Recorded behavior only for content: AWS returns the engine versions actually offered, including the current Redis OSS generations mapped onto their parameter-group families (for example EngineVersion '6.0'/'6.2' -> family 'redis6.x', '7.0'/'7.1' -> family 'redis7') with a CacheEngineVersionDescription of the form 'redis version <x.y.z>'.
Emulator: 200, but the members from index 11 through index 32 are not AWS's: where AWS lists the 6.x/7.x entries the emulator lists legacy 2.8.x entries (Engine 'redis', EngineVersion '2.8.19'/'2.8.22'/'2.8.23'/'2.8.24', family 'redis2.8'), so the modern generations are missing from the catalog.
Why: version discovery is the first call a client makes before CreateCacheCluster/CreateReplicationGroup; against the emulator it concludes that only long-retired engine versions exist, picks one AWS no longer offers, and never finds the family names (redis6.x, redis7) it needs to create a parameter group for a current cluster.
Fix: the elasticache crate's seeded cache-engine-version catalog read by the DescribeCacheEngineVersions handler.
Traces: elasticache-catalog-describe-cache-engine-versions#task#1.

Cause 5 of 6: DescribeReservedCacheNodesOfferings mints synthetic offerings whose ids and price/offering-type combinations are not AWS's (fixture elasticache-catalog-describe-reserved-cache-nodes-offerings, step task#1; the same cluster covers task#2 and task#3).
AWS: Smithy: DescribeReservedCacheNodesOfferings -> ReservedCacheNodesOfferingMessage{Marker, ReservedCacheNodesOfferings}; ReservedCacheNodesOffering{ReservedCacheNodesOfferingId, CacheNodeType, Duration:Integer, FixedPrice:Double, UsagePrice:Double, ProductDescription, OfferingType, RecurringCharges}; errors ReservedCacheNodesOfferingNotFoundFault (404/ReservedCacheNodesOfferingNotFound), InvalidParameterValueException (400/InvalidParameterValue). Recorded behavior only for content: the offering catalog is a stable, enumerable set in which each OfferingType is internally consistent - a 'No Upfront' offering has FixedPrice 0.0 and a non-zero hourly RecurringCharge, an 'All Upfront' offering has a non-zero FixedPrice and a 0.0 recurring charge - and Double members are serialised in decimal form.
Emulator: 200, but offerings [0]..[5] are all different members than the recording's: the ReservedCacheNodesOfferingId values are per-run invented ids rather than catalog ids, the OfferingType/price pairing is inverted relative to AWS for the same node type and duration (the 'All Upfront' member carries the recurring-charge shape and the 'No Upfront' member carries the fixed price), and UsagePrice/FixedPrice come back as '0' where AWS serialises '0.0'.
Why: a client pricing a reservation reads a cost model that contradicts itself, and because the offering id is not a catalog id it cannot be carried into PurchaseReservedCacheNodesOffering or compared across calls. (The concrete ids and prices in the recording are one run's fill-ins; the requirement is a stable catalog with self-consistent OfferingType/FixedPrice/RecurringCharges triples.)
Fix: the elasticache crate's seeded reserved-cache-node offering catalog and the DescribeReservedCacheNodesOfferings handler/serializer that renders its Double members.
Traces: elasticache-catalog-describe-reserved-cache-nodes-offerings#task#1 (cluster also covers task#2, task#3).

Cause 6 of 6: DescribeServiceUpdates serves invented service updates with per-run timestamps instead of AWS's published update set (fixture elasticache-catalog-describe-service-updates, step task#1).
AWS: Smithy: DescribeServiceUpdates -> ServiceUpdatesMessage{Marker, ServiceUpdates}; ServiceUpdate{ServiceUpdateName, ServiceUpdateReleaseDate:TStamp, ServiceUpdateEndDate:TStamp, ServiceUpdateSeverity:enum, ServiceUpdateRecommendedApplyByDate:TStamp, ServiceUpdateStatus:enum, ServiceUpdateDescription, ServiceUpdateType:enum, Engine, EngineVersion, AutoUpdateAfterRecommendedApplyByDate, EstimatedUpdateTime}; errors ServiceUpdateNotFoundFault (404), InvalidParameterValueException (400/InvalidParameterValue). Recorded behavior only for content: AWS returns the published service updates for the account's engines - named updates such as the Redis OSS v6 EOL updates, with ServiceUpdateType 'engine-major-version-update', a severity, a 'available' status, and release/recommended-apply-by/end dates that are the update's published dates.
Emulator: 200, but every member from index 0 through index 72 diverges: the emulator answers its own synthetic set (for example a memcached 'security-update' named after the engine, ServiceUpdateStatus 'expired') whose ServiceUpdateReleaseDate/RecommendedApplyByDate/EndDate are generated relative to the emulator's clock, so neither the names, the types, the severities, the statuses nor the dates correspond to the catalog AWS serves.
Why: a client that enumerates service updates to decide what to apply (or to assert that nothing is pending) sees updates that do not exist, in a status AWS does not report for them, so patch-compliance automation acts on phantom work and never finds the real engine-version updates by name. (The specific dates in the recording are fill-ins; the requirement is a stable published catalog, not clock-derived values.)
Fix: the elasticache crate's seeded service-update catalog read by the DescribeServiceUpdates handler.
Traces: elasticache-catalog-describe-service-updates#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, 27 fixtures replayed, 182/246 steps passed (score 74.0%), 41 divergence clusters. The six clusters below are every cluster in that run with verdict=mismatch, stub_path empty AND owned_by empty; the other 35 are either owned by an open issue or blocked by a filed stub.

Cause 1 - fixture elasticache-cache-parameter-group-workflow, step task#3, operation DescribeEngineDefaultParameters, expected 200, actual 200:
  $.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter[0]: value_mismatch (expected a member with AllowedValues '0-', MinimumEngineVersion '7.0.0' and a CacheNodeTypeSpecificValues list with one CacheNodeType/Value pair per supported node type)
  $...EngineDefaults.Marker: value_mismatch
  $...EngineDefaults.Parameters.Parameter[0..].AllowedValues / .DataType / .Description / .MinimumEngineVersion / .ParameterName / .ParameterValue / .IsModifiable / .ChangeType: value_mismatch across the list.

Cause 2 - same fixture, step task#5, operation DescribeCacheParameters, expected 200, actual 200:
  $.DescribeCacheParametersResponse.DescribeCacheParametersResult.CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter[0]: value_mismatch (expected AllowedValues '0-', MinimumEngineVersion '7.0.0', per-node-type values)
  $...Parameters.Parameter[0]..[41]: per-member field mismatches (AllowedValues, DataType, Description, IsModifiable, MinimumEngineVersion, ChangeType, ParameterName, ParameterValue)
  $...Parameters.Parameter[42]..[63]: missing entirely.

Cause 3 - same fixture, step task#7, operation DescribeCacheParameters, expected 200, actual 200: same CacheNodeTypeSpecificParameter[0] value_mismatch and the same truncated Parameters list as cause 2, plus $...Parameters.Parameter[24].Source and $...Parameters.Parameter[41].Source divergent after the preceding ModifyCacheParameterGroup.

Cause 4 - fixture elasticache-catalog-describe-cache-engine-versions, step task#1, operation DescribeCacheEngineVersions, expected 200, actual 200:
  $.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion[13]: missing (expected {EngineVersion '6.0', CacheParameterGroupFamily 'redis6.x', CacheEngineVersionDescription 'redis version 6.0.5', Engine 'redis', CacheEngineDescription 'Redis'}, got {Engine 'redis', EngineVersion '2.8.19', CacheParameterGroupFamily 'redis2.8', ...})
  [14]: expected '6.2'/'redis6.x'/'redis version 6.2.6', got '2.8.22'/'redis2.8'
  [15]: expected '7.0'/'redis7'/'redis version 7.0.7', got '2.8.23'/'redis2.8'
  [16]: expected '7.1'/'redis7'/'redis version 7.1.0', got '2.8.24'/'redis2.8'
  divergence_key lists CacheEngineVersion[11] through [32] as divergent.

Cause 5 - fixture elasticache-catalog-describe-reserved-cache-nodes-offerings, step task#1 (affects task#1, task#2, task#3), operation DescribeReservedCacheNodesOfferings, expected 200, actual 200:
  $...ReservedCacheNodesOfferings.ReservedCacheNodesOffering[0]: missing (expected a 'No Upfront' cache.r6g.large/31536000 offering with FixedPrice '0.0', UsagePrice '0.0' and an hourly RecurringCharge; got an 'All Upfront' offering with a run-generated ReservedCacheNodesOfferingId, FixedPrice '1261.44', UsagePrice '0' and RecurringChargeAmount '0')
  [1]: expected 'All Upfront' with FixedPrice '1146.0' and recurring '0.0'; got 'No Upfront' with FixedPrice '0' and recurring '0.18'
  ReservedCacheNodesOffering[0]..[5] all reported divergent.

Cause 6 - fixture elasticache-catalog-describe-service-updates, step task#1, operation DescribeServiceUpdates, expected 200, actual 200:
  $...ServiceUpdates.ServiceUpdate[0]: missing (expected {ServiceUpdateName 'elasticache-redis-oss-v6-eol-replication-groups', Engine 'redis', EngineVersion 'redis 6.x', ServiceUpdateType 'engine-major-version-update', ServiceUpdateSeverity 'important', ServiceUpdateStatus 'available', published release/recommended-apply-by/end dates}, got {ServiceUpdateName 'elasticache-memcached-security-update-001', ServiceUpdateType 'security-update', Engine 'memcached', ServiceUpdateStatus 'expired', clock-derived dates})
  [1]: expected 'elasticache-redis-oss-v6-eol-cache-clusters'; got another synthetic memcached update
  divergence_key lists ServiceUpdate[0] through [72] as divergent.

Model evidence: smithy_query(query='operation', operation='DescribeEngineDefaultParameters,DescribeCacheEngineVersions,DescribeReservedCacheNodesOfferings,DescribeServiceUpdates', model_path='services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json') and smithy_query(query='shape', shape='EngineDefaults,CacheEngineVersion,ReservedCacheNodesOffering,ServiceUpdate') for the output shapes, members and modeled errors quoted above. The models define the SHAPES only; the catalog contents are recorded behavior. Concrete ids, prices, node types and timestamps quoted from the recording are fill-ins, not requirements - the requirement is that each catalog holds AWS's entries with self-consistent, stable, non-clock-derived values.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and look at the elasticache clusters with owned_by empty.
3. Cause 1-3: replay fixture elasticache-cache-parameter-group-workflow. At step task#3 call DescribeEngineDefaultParameters(CacheParameterGroupFamily=<the redis7 family the fixture uses>) and compare EngineDefaults.Parameters, EngineDefaults.CacheNodeTypeSpecificParameters and EngineDefaults.Marker with the fixture's expected body. At step task#5 call DescribeCacheParameters on the group created earlier in the fixture: Parameters must hold the family's full inherited set (the fixture expects entries up to index 63) and CacheNodeTypeSpecificParameters must be populated. At step task#7, after the fixture's ModifyCacheParameterGroup, re-call DescribeCacheParameters and check the modified members' ParameterValue and Source.
4. Cause 4: replay fixture elasticache-catalog-describe-cache-engine-versions step task#1 - DescribeCacheEngineVersions must list the redis 6.0/6.2 (family redis6.x) and 7.0/7.1 (family redis7) entries; observe redis 2.8.x entries in their place.
5. Cause 5: replay fixture elasticache-catalog-describe-reserved-cache-nodes-offerings steps task#1..task#3 - DescribeReservedCacheNodesOfferings must return the stable offering catalog with self-consistent OfferingType/FixedPrice/RecurringCharges triples and decimal Double serialisation; observe run-generated offering ids and inverted price/offering-type pairings.
6. Cause 6: replay fixture elasticache-catalog-describe-service-updates step task#1 - DescribeServiceUpdates must return the published update set (e.g. the Redis OSS v6 EOL engine-major-version-update entries, status 'available'); observe synthetic memcached security updates with clock-derived dates.
7. Re-replay all five fixtures after the fix and confirm every step passes.
