Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-catalog-describe-cache-engine-versions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-catalog-describe-cache-engine-versions elasticache-catalog-describe-cache-parameter-groups elasticache-catalog-describe-reserved-cache-nodes-offerings elasticache-catalog-describe-service-updates elasticache-cache-parameter-group-workflow
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeCacheEngineVersions|2xx->2xx|$.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion[*]
Issue-Key: elasticache|DescribeCacheParameterGroups|2xx->2xx|$.DescribeCacheParameterGroupsResponse.DescribeCacheParameterGroupsResult.CacheParameterGroups.CacheParameterGroup[*]
Issue-Key: elasticache|DescribeReservedCacheNodesOfferings|2xx->2xx|$.DescribeReservedCacheNodesOfferingsResponse.DescribeReservedCacheNodesOfferingsResult.ReservedCacheNodesOfferings.ReservedCacheNodesOffering[*]
Issue-Key: elasticache|DescribeServiceUpdates|2xx->2xx|$.DescribeServiceUpdatesResponse.DescribeServiceUpdatesResult.ServiceUpdates.ServiceUpdate[*]
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.CacheNodeTypeSpecificParameters
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
Issue-Related: 20261030T130325Z-elasticache-parameter-catalog-is-incomplete-describecacheparameters-omits-cachenodetypespecificparameters.md
Issue-Commit: 232d6cb39af83c8f3e5d0960e0729eaeb270e0f7

# ElastiCache reference catalogs diverge from AWS: engine versions, default parameter groups, reserved-node offerings, service updates and engine defaults

> _Note: this issue was identified against the code base at commit `232d6cb39af83c8f3e5d0960e0729eaeb270e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCacheEngineVersions

## Description
SEVEN net-new root causes, all in the elasticache crate: every read-only reference catalog the service publishes answers 200 with the emulator's own invented inventory instead of AWS's. The shared fault is the seeded catalog data (and, for the parameter describes, the empty CacheNodeTypeSpecificParameters wrapper), so a fixer must close all seven before submitting. None of the four open elasticache issues names these catalogs or carries any of the replay keys below; the companion issue on DescribeCacheParameters owns the per-family parameter sets of the elasticache-catalog-describe-cache-parameters fixture, and this issue owns the rest.

Cause 1 of 7: DescribeCacheEngineVersions publishes a different engine-version inventory (fixture elasticache-catalog-describe-cache-engine-versions, step task#1).
AWS: Smithy: DescribeCacheEngineVersions -> CacheEngineVersionMessage {Marker, CacheEngineVersions: CacheEngineVersionList}; the member fields are Engine, EngineVersion, CacheParameterGroupFamily, CacheEngineDescription, CacheEngineVersionDescription. The inventory itself is recorded behavior only: AWS returns every supported engine/version pair, each bound to its parameter-group family, ordered by engine then version — the recording includes memcached 1.4.5 (family memcached1.4) and redis 6.0/6.2/7.0 (families redis6.x, redis6.x, redis7), i.e. both the oldest memcached line and the current redis lines.
Emulator: 200, but the list is a different, shorter inventory: at the index where AWS has memcached 1.4.5 the emulator has memcached 1.5.10, where AWS has redis 6.0/6.2/7.0 the emulator has redis 2.8.22/2.8.23/2.8.24, and 22 member positions in total diverge.
Why: this is the discovery call a client makes before CreateCacheCluster / CreateReplicationGroup; a caller that picks an EngineVersion from it (or maps a version to its CacheParameterGroupFamily) chooses versions AWS does not offer and misses the versions it does.
Fix: the elasticache crate's seeded engine-version catalog read by the DescribeCacheEngineVersions handler.
Traces: elasticache-catalog-describe-cache-engine-versions#task#1.

Cause 2 of 7: DescribeCacheParameterGroups omits the valkey9 default groups (fixture elasticache-catalog-describe-cache-parameter-groups, step task#1).
AWS: Smithy: DescribeCacheParameterGroups -> CacheParameterGroupsMessage {Marker, CacheParameterGroups}; each CacheParameterGroup carries CacheParameterGroupName, CacheParameterGroupFamily, Description, IsGlobal and ARN. Recorded behavior for the inventory: every supported family has its default group, and the cluster-mode families additionally have the '.cluster.on' variant — the recording's last two entries are default.valkey9 (family valkey9, Description 'Default parameter group for valkey9') and default.valkey9.cluster.on (Description 'Customized default parameter group for valkey9 with cluster mode on'). The ARN is arn:aws:elasticache:<region>:<accountId>:parametergroup:<name> — region and account id come from the request's endpoint and the caller's identity, not from the recording.
Emulator: 200 with the two valkey9 entries absent ('got None' at both positions).
Why: a client that enumerates families from this call never sees the current engine generation, and the matching DescribeCacheParameters call for default.valkey9 404s (reported in the companion issue); together the newest family does not exist on the emulator.
Fix: seed the valkey9 default parameter groups in the elasticache crate's parameter-group catalog read by the DescribeCacheParameterGroups handler.
Traces: elasticache-catalog-describe-cache-parameter-groups#task#1.

Cause 3 of 7: DescribeReservedCacheNodesOfferings invents offerings instead of returning AWS's (fixture elasticache-catalog-describe-reserved-cache-nodes-offerings, step task#1; the cluster also covers task#2 and task#3).
AWS: Smithy: DescribeReservedCacheNodesOfferings -> ReservedCacheNodesOfferingMessage {Marker, ReservedCacheNodesOfferings}; each offering carries ReservedCacheNodesOfferingId, CacheNodeType, Duration, FixedPrice, UsagePrice, ProductDescription, OfferingType and RecurringCharges. Recorded behavior for the inventory: for one CacheNodeType/ProductDescription there is a fixed set of offerings, one per (Duration, OfferingType) combination — the recording shows, for cache.r6g.large/redis, a 31536000s 'No Upfront' offering with FixedPrice 0.0 and an hourly RecurringCharge, a 31536000s 'All Upfront' offering with a non-zero FixedPrice and a 0.0 recurring charge, and 94608000s variants. The offering ids are service-assigned and differ per environment; the REQUIREMENT is the set of (Duration, OfferingType) combinations and the internally consistent pricing shape (All Upfront => FixedPrice > 0 and RecurringChargeAmount 0; No Upfront => FixedPrice 0 and RecurringChargeAmount > 0).
Emulator: 200, but six offering positions diverge and the pricing shape is permuted — at position 0 it returns an 'All Upfront' offering where AWS has 'No Upfront', at position 1 a 'No Upfront' where AWS has 'All Upfront'.
Why: a client pricing or purchasing a reservation (DescribeReservedCacheNodesOfferings then PurchaseReservedCacheNodesOffering) reads an offering mix AWS does not sell, and the OfferingType/price relationship it depends on is inverted.
Fix: the elasticache crate's seeded reserved-node offering catalog read by the DescribeReservedCacheNodesOfferings handler.
Traces: elasticache-catalog-describe-reserved-cache-nodes-offerings#task#1 (cluster also covers task#2, task#3).

Cause 4 of 7: DescribeServiceUpdates publishes invented updates (fixture elasticache-catalog-describe-service-updates, step task#1).
AWS: Smithy: DescribeServiceUpdates -> ServiceUpdatesMessage {Marker, ServiceUpdates}; each update carries ServiceUpdateName, ServiceUpdateReleaseDate, ServiceUpdateEndDate, ServiceUpdateSeverity, ServiceUpdateRecommendedApplyByDate, ServiceUpdateStatus, ServiceUpdateDescription, ServiceUpdateType, Engine, EngineVersion, AutoUpdateAfterRecommendedApplyByDate, EstimatedUpdateTime. Recorded behavior for the inventory: AWS returns the real published update catalog — e.g. the engine-major-version-update entries named elasticache-redis-oss-v6-eol-replication-groups and elasticache-redis-oss-v6-eol-cache-clusters, Engine redis, EngineVersion 'redis 6.x', ServiceUpdateStatus 'available', ServiceUpdateType 'engine-major-version-update'. 73 member positions are compared.
Emulator: 200 with a wholly different list: position 0 is a fabricated 'elasticache-memcached-security-update-001' (type security-update, status expired, Engine memcached) and all 73 positions diverge; the emulator's release/end/apply-by dates are also run-relative timestamps rather than the catalog's fixed dates.
Why: a client that lists updates and then calls BatchApplyUpdateAction/DescribeUpdateActions names a ServiceUpdateName that does not exist on AWS, and update-compliance tooling reads a status and severity mix AWS never publishes. (Per-run timestamps are not themselves a requirement; the update NAMES, types, engines, statuses and the presence of the real entries are.)
Fix: the elasticache crate's seeded service-update catalog read by the DescribeServiceUpdates handler.
Traces: elasticache-catalog-describe-service-updates#task#1.

Cause 5 of 7: DescribeEngineDefaultParameters returns an empty CacheNodeTypeSpecificParameters wrapper and the wrong EngineDefaults payload (fixture elasticache-cache-parameter-group-workflow, step task#3).
AWS: Smithy: DescribeEngineDefaultParameters -> EngineDefaults {CacheParameterGroupFamily, Marker, Parameters, CacheNodeTypeSpecificParameters}; paginated {inputToken Marker, outputToken EngineDefaults.Marker, items EngineDefaults.Parameters}. Recorded behavior for content: EngineDefaults.CacheNodeTypeSpecificParameters is a wrapper holding a CacheNodeTypeSpecificParameter list (recording's first member: AllowedValues '0-', MinimumEngineVersion '7.0.0', one CacheNodeType/Value pair per supported node type), EngineDefaults.Marker is set when the family's parameter list is truncated, and Parameters holds the family's full default set.
Emulator: 200, but the replay reports type_mismatch at EngineDefaults.CacheNodeTypeSpecificParameters (the node is not the member-wrapped list AWS returns), a divergent EngineDefaults.Marker, and divergent fields on the Parameters members (AllowedValues, DataType, Description, IsModifiable, MinimumEngineVersion, ParameterName, ParameterValue).
Why: this is the call a client uses to learn a family's defaults before creating a group; with no node-type-specific parameters and wrong per-parameter metadata it cannot validate a ModifyCacheParameterGroup request, and the absent Marker breaks pagination of the default set.
Fix: the elasticache crate's engine-default catalog and the DescribeEngineDefaultParameters handler's serialisation of the CacheNodeTypeSpecificParameters wrapper and Marker.
Traces: elasticache-cache-parameter-group-workflow#task#3.

Cause 6 of 7: DescribeCacheParameters on a freshly created customer group returns the stub catalog (fixture elasticache-cache-parameter-group-workflow, step task#5).
AWS: as in cause 5 for content; DescribeCacheParameters returns the inherited family defaults for a newly created group, including the node-type-specific list.
Emulator: 200 with type_mismatch at CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter and divergent fields across the Parameters members.
Why: a client reading back the group it just created sees a catalog that does not match the family it asked for, so parameter-tuning flows (read AllowedValues, then modify) cannot run.
Fix: same component as cause 5 (the catalog the per-group describe inherits from the family defaults).
Traces: elasticache-cache-parameter-group-workflow#task#5.

Cause 7 of 7: the same divergence after ModifyCacheParameterGroup, i.e. the catalog stays wrong across a modify/read-back round trip (fixture elasticache-cache-parameter-group-workflow, step task#7).
AWS: as cause 6; after a modify, the response carries the family's full parameter set with the modified members' Source changed and the node-type-specific list still populated.
Emulator: 200 with the same CacheNodeTypeSpecificParameter type_mismatch and divergent Parameters members (the replay also flags the Source member on some entries).
Why: the read-back a client uses to confirm its own modification reports a catalog whose members and metadata do not match AWS, so it cannot tell which parameters are user-set.
Fix: same component as cause 5.
Traces: elasticache-cache-parameter-group-workflow#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, 84/139 steps passed). Seven clusters, all verdict=mismatch, service=elasticache, stub_path empty, owned_by empty.

Cause 1 — elasticache-catalog-describe-cache-engine-versions#task#1, DescribeCacheEngineVersions, expected 200, actual 200:
  $...CacheEngineVersions.CacheEngineVersion[4]: missing (expected {Engine memcached, EngineVersion 1.4.5, CacheParameterGroupFamily memcached1.4, ...}, got {Engine memcached, EngineVersion 1.5.10, CacheParameterGroupFamily memcached1.5, ...})
  [13]: expected redis 6.0 / family redis6.x, got redis 2.8.22 / family redis2.8
  [14]: expected redis 6.2 / family redis6.x, got redis 2.8.23 / family redis2.8
  [15]: expected redis 7.0 / family redis7, got redis 2.8.24 / family redis2.8
  plus positions 10,11,12,16..23,25..31 divergent.
Cause 2 — elasticache-catalog-describe-cache-parameter-groups#task#1, DescribeCacheParameterGroups, expected 200, actual 200:
  CacheParameterGroup[19]: missing (expected {CacheParameterGroupName default.valkey9, CacheParameterGroupFamily valkey9, IsGlobal false, Description 'Default parameter group for valkey9', ARN arn:aws:elasticache:<region>:<account>:parametergroup:default.valkey9}, got None)
  CacheParameterGroup[20]: missing (expected default.valkey9.cluster.on, Description 'Customized default parameter group for valkey9 with cluster mode on', got None)
Cause 3 — elasticache-catalog-describe-reserved-cache-nodes-offerings#task#1 (affects task#1..task#3), expected 200, actual 200:
  ReservedCacheNodesOffering[0]: expected {Duration 31536000, CacheNodeType cache.r6g.large, OfferingType 'No Upfront', FixedPrice 0.0, UsagePrice 0.0, RecurringCharge {Hourly, 0.141}, ProductDescription redis}, got {OfferingType 'All Upfront', FixedPrice 1261.44, RecurringCharge {Hourly, 0}}
  [1]: expected OfferingType 'All Upfront' with FixedPrice 1146.0 and RecurringChargeAmount 0.0, got OfferingType 'No Upfront' with FixedPrice 0 and RecurringChargeAmount 0.18
  [2]..[5] likewise divergent (expected includes Duration 94608000 offerings). Offering ids differ per environment and are not the requirement.
Cause 4 — elasticache-catalog-describe-service-updates#task#1, DescribeServiceUpdates, expected 200, actual 200:
  ServiceUpdate[0]: expected {ServiceUpdateName 'elasticache-redis-oss-v6-eol-replication-groups', Engine redis, EngineVersion 'redis 6.x', ServiceUpdateType 'engine-major-version-update', ServiceUpdateStatus 'available', ServiceUpdateSeverity 'important'}, got {ServiceUpdateName 'elasticache-memcached-security-update-001', Engine memcached, EngineVersion 1.6.22, ServiceUpdateType 'security-update', ServiceUpdateStatus 'expired', ServiceUpdateSeverity 'critical'}
  ServiceUpdate[1]: expected 'elasticache-redis-oss-v6-eol-cache-clusters'; positions [0]..[72] all divergent.
Cause 5 — elasticache-cache-parameter-group-workflow#task#3, DescribeEngineDefaultParameters, expected 200, actual 200:
  $.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.CacheNodeTypeSpecificParameters: type_mismatch (expected {CacheNodeTypeSpecificParameter: [ {AllowedValues '0-', MinimumEngineVersion '7.0.0', CacheNodeTypeSpecificValues: [...per node type...]}, ... ]})
  plus EngineDefaults.Marker and EngineDefaults.Parameters.Parameter[*].{AllowedValues,DataType,Description,IsModifiable,MinimumEngineVersion,ParameterName,ParameterValue} divergent.
Cause 6 — elasticache-cache-parameter-group-workflow#task#5, DescribeCacheParameters, expected 200, actual 200:
  $...CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter: type_mismatch (expected the populated list), plus Parameters.Parameter[*] field divergences.
Cause 7 — elasticache-cache-parameter-group-workflow#task#7, DescribeCacheParameters, expected 200, actual 200: same shape as cause 6, additionally flagging Parameter[24].Source and Parameter[41].Source.

Expected / actual in one line per cause: every one of these calls must answer HTTP 200 with AWS's published catalog for the requested scope; the emulator answers HTTP 200 with a different inventory (causes 1-4) or with an unpopulated CacheNodeTypeSpecificParameters wrapper and wrong per-parameter metadata (causes 5-7).

Model evidence: smithy_query(query="operation", operation="DescribeCacheEngineVersions,DescribeCacheParameterGroups,DescribeReservedCacheNodesOfferings,DescribeServiceUpdates,DescribeEngineDefaultParameters,DescribeCacheParameters", model_path="services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json") -> DescribeEngineDefaultParameters output EngineDefaults, paginated on EngineDefaults.Marker/EngineDefaults.Parameters, errors InvalidParameterValueException 400/InvalidParameterValue and InvalidParameterCombinationException 400/InvalidParameterCombination; DescribeCacheParameters output CacheParameterGroupDetails {Marker, Parameters, CacheNodeTypeSpecificParameters}. The catalogs' contents are not modelled, so they are cited as recorded behavior.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: DescribeCacheEngineVersions with no filter. Expect AWS's full engine inventory (memcached 1.4.5 with family memcached1.4; redis 6.0 and 6.2 with family redis6.x; redis 7.0 with family redis7). Observe memcached 1.5.10 and redis 2.8.x entries at those positions.
3. Cause 2: DescribeCacheParameterGroups with no filter. Expect default.valkey9 and default.valkey9.cluster.on among the groups. Observe them absent.
4. Cause 3: DescribeReservedCacheNodesOfferings(CacheNodeType=cache.r6g.large, ProductDescription=redis). Expect one offering per (Duration, OfferingType) combination with All Upfront => FixedPrice>0/RecurringChargeAmount 0 and No Upfront => FixedPrice 0/RecurringChargeAmount>0. Observe the OfferingType/price pairing permuted.
5. Cause 4: DescribeServiceUpdates with no filter. Expect the published updates including elasticache-redis-oss-v6-eol-replication-groups and elasticache-redis-oss-v6-eol-cache-clusters (Engine redis, type engine-major-version-update, status available). Observe fabricated memcached security updates instead.
6. Causes 5-7: replay fixture elasticache-cache-parameter-group-workflow steps task#3, task#5 and task#7 — DescribeEngineDefaultParameters(CacheParameterGroupFamily=<family>), then CreateCacheParameterGroup + DescribeCacheParameters, then ModifyCacheParameterGroup + DescribeCacheParameters. Expect a populated CacheNodeTypeSpecificParameters wrapper and the family's full Parameters set (with Marker when truncated) on each; observe the type_mismatch at that node and divergent parameter metadata.
7. Re-replay all four catalog fixtures plus elasticache-cache-parameter-group-workflow and confirm those steps pass.
