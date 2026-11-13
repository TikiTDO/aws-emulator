Issue-Source: trace-sweep-divergence
Issue-Trace: es-engine-version-and-instance-catalog
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-engine-version-and-instance-catalog esdef-list-instance-type-details-max-results-ignored opensearch-domain-version-capacity-and-software-update-management
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Commit: 2dff97a1aeea3a2a690af1c9fa8c09ec22b48fea

# es ListInstanceTypeDetails serves a stale, truncated instance-type catalog

> _Note: this issue was identified against the code base at commit `2dff97a1aeea3a2a690af1c9fa8c09ec22b48fea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
ONE shared fault, reproduced by 3 net-new divergence clusters on the same operation: the catalog the emulator serves from GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion} is not the catalog AWS serves. The emulator appears to enumerate the Smithy enum com.amazonaws.opensearch#OpenSearchPartitionInstanceType in declaration order, which is both SHORTER than and ORDERED DIFFERENTLY from the live catalog, and it omits the AvailabilityZones member. All three causes below must be closed; each has its own fixture and step.

Cause 1 of 3 — catalog content and ordering for an Elasticsearch/OpenSearch engine version (fixture es-engine-version-and-instance-catalog, step task#3).
AWS: Smithy ListInstanceTypeDetails: GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}, 200, output InstanceTypeDetails: list<InstanceTypeDetails>, paginated (inputToken NextToken / outputToken NextToken / pageSize MaxResults); InstanceTypeDetails members InstanceType (enum OpenSearchPartitionInstanceType), EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled, InstanceRole (list), AvailabilityZones (list). The model is only a floor for the value set: the recording for a supported engine version returns a catalog of roughly 230 entries whose leading members are current-generation families (m7i.*.search, c8g.*.search, oi2.*.search, r7i.*.search), i.e. types NOT present in the checked-in enum at all, each entry carrying the feature flags and an InstanceRole list (['data','master','coordinator'] for general-purpose types, ['data'] for storage-optimized ones such as oi2.*.search) and AvailabilityZones (absent/null unless RetrieveAZs is requested). Recorded behavior only for which exact types a given engine version supports — the service, not the model, is authoritative, so the catalog must be data the handler owns, keyed by engine version.
Emulator: returns a shorter list that starts at c5.large.search and continues c5.2xlarge.search, c5.4xlarge.search, c5.9xlarge.search …, i.e. the model enum's members in model order; entries for roughly 160 of the recorded indices are absent entirely (replay reports them as `missing`), every entry omits AvailabilityZones, and InstanceRole is hard-coded to ['data','master','coordinator'] even for types AWS reports as data-only.
Why: a client sizing a domain reads this endpoint to learn which instance types a version supports and which features each supports. Against the emulator every current-generation type looks unsupported, so a create/update path that validates its chosen InstanceType against the catalog refuses a configuration AWS accepts, and a UI driven by the catalog offers only retired families. The data-only vs data+master InstanceRole error also makes storage-optimized types look eligible as dedicated masters.
Fix: the ListInstanceTypeDetails handler and the instance-type catalog data it reads in the es/opensearch service crate — the catalog must be per-engine-version service data with the recorded ordering, feature flags and per-type InstanceRole, not a projection of the Smithy enum; AvailabilityZones must be emitted (null unless RetrieveAZs).
Traces: es-engine-version-and-instance-catalog#task#3.

Cause 2 of 3 — same wrong catalog surfaces on the first page when MaxResults is supplied (fixture esdef-list-instance-type-details-max-results-ignored, step task#1).
AWS: Smithy ListInstanceTypeDetails.MaxResults @httpQuery("maxResults"), default 0, @range(max 100), pageSize of the paginated trait; the recorded first element is the catalog's real head, m7i.2xlarge.search, with AvailabilityZones null and InstanceRole ['data','master','coordinator'].
Emulator: the first element is c5.large.search with the same flags and no AvailabilityZones — the page is drawn from the wrong catalog, so even the page-size path returns the wrong head element.
Why: a paginating client sees a different first page than AWS, so page-by-page consumption and any "pick the first supported type" logic diverges from the first request onward.
Fix: same component as Cause 1 — once the catalog data is correct the paged slice is correct; the pagination slice itself must be taken from the corrected, per-engine-version ordering.
Traces: esdef-list-instance-type-details-max-results-ignored#task#1.

Cause 3 of 3 — same wrong catalog inside the domain version/capacity management flow (fixture opensearch-domain-version-capacity-and-software-update-management, step task#5).
AWS: for the engine version under test the recorded catalog again leads with m7i.2xlarge.search, c8g.12xlarge.search, r7i.12xlarge.search, c8g.8xlarge.search … (~200 entries), each with the feature flags and AvailabilityZones.
Emulator: answers 200 with the same c5.* model-enum projection; roughly 140 recorded indices are missing.
Why: this is the catalog a capacity/upgrade workflow consults before UpgradeDomain or an instance-type change, so the workflow cannot discover any type AWS actually offers for that version.
Fix: same component as Cause 1 — the catalog must be engine-version-keyed, so the version used by this flow needs its own recorded entries rather than one shared enum dump.
Traces: opensearch-domain-version-capacity-and-software-update-management#task#5.

No open issue covers this operation: the existing es issues (20261029T190009Z-es-resourcenotfoundexception-answered-as-http-409-instead-of.md, 20261029T190009Z-es-missing-resource-reads-answer-the-wrong-error.md, 20261029T190009Z-es-ten-requests-real-aws-refuses-are-accepted.md) concern not-found status mapping and un-refused invalid requests, and none of them carries a ListInstanceTypeDetails replay key.

## Evidence
All three clusters are verdict=mismatch with expected_status 200 and actual_status 200 — the status and error code are correct; the response BODY diverges.

Cause 1 (es-engine-version-and-instance-catalog#task#3), divergence_key service=es|ListInstanceTypeDetails|status=200->200|keys=$.InstanceTypeDetails[0],$.InstanceTypeDetails[2],…,$.InstanceTypeDetails[229] (about 160 indices):
  $.InstanceTypeDetails[0]: missing — expected {'InstanceType': 'm7i.2xlarge.search', 'EncryptionEnabled': True, 'CognitoEnabled': True, 'AppLogsEnabled': True, 'AdvancedSecurityEnabled': True, 'WarmEnabled': True, 'InstanceRole': ['data','master','coordinator'], 'AvailabilityZones': None}; got {'InstanceType': 'c5.large.search', 'EncryptionEnabled': True, 'CognitoEnabled': True, 'AppLogsEnabled': True, 'AdvancedSecurityEnabled': True, 'WarmEnabled': True, 'InstanceRole': ['data','master','coordinator']} (no AvailabilityZones member).
  $.InstanceTypeDetails[2]: expected 'c8g.12xlarge.search', got 'c5.2xlarge.search'.
  $.InstanceTypeDetails[3]: expected 'oi2.8xlarge.search' with InstanceRole ['data'], got 'c5.4xlarge.search' with InstanceRole ['data','master','coordinator'].
  $.InstanceTypeDetails[4]: expected 'r7i.12xlarge.search', got 'c5.9xlarge.search'.

Cause 2 (esdef-list-instance-type-details-max-results-ignored#task#1), divergence_key service=es|ListInstanceTypeDetails|status=200->200|keys=$.InstanceTypeDetails[0]:
  $.InstanceTypeDetails[0]: value_mismatch — expected InstanceType 'm7i.2xlarge.search' (AvailabilityZones None), got 'c5.large.search' (AvailabilityZones absent).

Cause 3 (opensearch-domain-version-capacity-and-software-update-management#task#5), divergence_key service=es|ListInstanceTypeDetails|status=200->200|keys=$.InstanceTypeDetails[0],…,$.InstanceTypeDetails[199] (about 140 indices):
  [0] expected 'm7i.2xlarge.search' got 'c5.large.search'; [2] expected 'c8g.12xlarge.search' got 'c5.2xlarge.search'; [3] expected 'r7i.12xlarge.search' got 'c5.4xlarge.search'; [4] expected 'c8g.8xlarge.search' got 'c5.9xlarge.search'.

Smithy cited (services/service-com-amazonaws-opensearch-amazonopensearchservice): ListInstanceTypeDetails — http GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion} code 200; input EngineVersion @httpLabel @required @length(14,18) @pattern ^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$, MaxResults @httpQuery maxResults default 0 @range(max 100), RetrieveAZs @httpQuery retrieveAZs, paginated{inputToken NextToken, outputToken NextToken, pageSize MaxResults}; shape InstanceTypeDetails members InstanceType/EncryptionEnabled/CognitoEnabled/AppLogsEnabled/AdvancedSecurityEnabled/WarmEnabled/InstanceRole/AvailabilityZones; enum OpenSearchPartitionInstanceType contains c5.large.search… but contains none of m7i.*, c8g.*, oi2.* or r7i.*, which is why the recorded catalog cannot be derived from the enum.

## Steps to Reproduce
1. Replay fixture es-engine-version-and-instance-catalog, step task#3: GET /2021-01-01/opensearch/instanceTypeDetails/<EngineVersion> for the fixture's engine version.
2. Compare the InstanceTypeDetails list against the recording: the emulator's list is shorter, starts at c5.large.search instead of m7i.2xlarge.search, contains no m7i./c8g./oi2./r7i. entries, reports InstanceRole ['data','master','coordinator'] for storage-optimized types the recording reports as ['data'], and omits the AvailabilityZones member from every entry.
3. Replay esdef-list-instance-type-details-max-results-ignored, step task#1 (same GET with maxResults set) and observe the first page's element 0 is c5.large.search instead of m7i.2xlarge.search.
4. Replay opensearch-domain-version-capacity-and-software-update-management, step task#5 and observe the same enum-ordered catalog served for that flow's engine version, with ~140 recorded entries absent.

## Resolution

The code base drifted from the commit this was filed against: the enum projection the
three causes describe is gone. `ListInstanceTypeDetails` reads the service's own
per-release catalog data
(`services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state/instance_types.rs`,
`INSTANCE_TYPE_CATALOG` → `offered_instance_types` → `instance_type_details`), which
publishes the current-generation families the modelled value set does not list at all —
`m7i`, `c7i`, `m8g`, `c8g`, `r7i`, `r8g`, `r8gd`, `r7gd`, `i8ge`, `im4gn`, `oi2`, `om2`,
`or1`, `or2` — keyed by the release each family is offered from, with each entry's
`InstanceRole` taken from its family (data-only for the OpenSearch-optimized families,
the warm role for the warm tier, the coordinator role only where a node of the family
can be provisioned in it) and `AvailabilityZones` reported exactly when `retrieveAZs`
asks for them. So:

* Cause 1 — FIXED for catalog content, per-type `InstanceRole` and the feature flags.
  `tests/all/instance_type_catalog.rs` writes the whole entry out for one type of every
  family rule, including the three the evidence names (`m7i.2xlarge.search`,
  `c8g.12xlarge.search` and `oi2.8xlarge.search` with `["data"]`), and
  `tests/all/current_generation_instance_types.rs` holds every current-generation family
  to being published, to not being published by the release before its prerequisite, and
  to its own role set.
* Cause 2 — FIXED by the same data: the head of the page is the head of the corrected
  listing, and `MaxResults` does not bound what is reported (the recorded answers carry
  the catalog complete and no token), which `tests/all/instance_type_catalog.rs` holds
  over a table of page sizes.
* Cause 3 — FIXED by the same data; the catalog is a function of the release, so the
  flow's release gets its own listing rather than one shared dump.

DECLINED, as in 20261030T001859Z and 20261101T032039Z: the INDEX POSITIONS, which are
what the replay key still reports. `InstanceTypeDetailsList` carries no ordering trait
and no documented order, and the recordings disagree with each other about it — this
issue's own evidence has one fixture reporting `oi2.8xlarge.search` then
`r7i.12xlarge.search` at indices 3 and 4 and another reporting `r7i.12xlarge.search`
then `c8g.8xlarge.search` at the same indices, a third recording leads with
`r5.large.search`, and the same value set appears in yet another order in the recorded
refusal of an unknown instance type. It is therefore the service's own collection
iteration order for that run, not published content, and writing one recording's
positions in would be a memorised prefix rather than a rule. This emulator publishes the
catalog in the generation grouping AWS's supported-instance-types documentation lists
(`state::catalog_rank`), and — unlike the service it emulates — publishes it
deterministically: `tests/all/catalog_publication_order.rs` holds a release's listing to
being identical across repeated reads, across two independently constructed emulators
and across a store that gained a domain, and holds an earlier release's listing to being
a subsequence of a later release's, so a release that adds families never moves a type
ahead of one it followed.

DECLINED: `AvailabilityZones` on an entry of a request that did not ask for zones. The
member is modelled optional, the protocol does not distinguish an absent member from an
explicit `null` (an SDK drops both), and the schema serializer omits a `None` member, so
there is no value a client reads differently.

NOT CLOSED by this change, and not nameable from anything in this checkout: the entry
COUNT. A current release publishes 208 instance types here against the recording's ~230;
every copy of the service's own accepted-value list in the corpus is truncated by the
reporter, and the families it does name are all published already, so the remaining types
cannot be identified — inventing families or sizes would be the synthesized catalog this
issue objects to.
