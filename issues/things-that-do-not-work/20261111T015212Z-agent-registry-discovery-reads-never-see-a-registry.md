Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: agent-registry.list-discoverable-filters-by-record-and-descriptor-type agent-registry.list-discoverable-pages-through-every-record-once agent-registry.search-custom-metadata-filter-narrows-results
Issue-Shape-Id: com.amazonaws.agentregistry#AgentRegistry
Issue-Key: agent-registry|ListDiscoverableRegistryRecords|ResourceNotFoundException
Issue-Key: agent-registry|SearchDiscoverableRegistryRecords|ResourceNotFoundException
Issue-Commit: ce0a33dd83459f56e544c6d97982c4df2e00c51d

# agent-registry discovery reads never see a registry authored by AgentRegistryControl: every ListDiscoverableRegistryRecords/SearchDiscoverableRegistryRecords answers 404 ResourceNotFoundException "Registry not found: <registryId>"

> _Note: this issue was identified against the code base at commit `ce0a33dd83459f56e544c6d97982c4df2e00c51d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDiscoverableRegistryRecords

## Description
Cause 1 of 1 — the Agent Registry DATA plane keeps a registry/record store of its own that no control-plane write ever reaches, so a registry (and its approved records) created through AgentRegistryControl is invisible to every discovery read. Failing checks: agent-registry.list-discoverable-filters-by-record-and-descriptor-type, agent-registry.list-discoverable-pages-through-every-record-once, agent-registry.search-custom-metadata-filter-narrows-results.

AWS: In AWS, AgentRegistryControl (CreateRegistry / CreateRegistryRecord / SubmitRegistryRecordForApproval / UpdateRegistryRecordStatus) and AgentRegistry (the discovery reads) are two API surfaces over ONE registry resource. Smithy (services/service-com-amazonaws-agentregistry-agentregistry/smithy-model/com-amazonaws-agentregistry-agentregistry.json): ListDiscoverableRegistryRecords POST /registries/{registryId}/discoverable-records-list, input registryId -> RegistryIdentifier (@length 1..2048, pattern ^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$) — documented as "either the full Amazon Resource Name (ARN) or the registry ID" — output registryRecords (required) + nextToken, @paginated, @readonly; SearchDiscoverableRegistryRecords POST /discoverable-records-search, input searchQuery (required), registryIds (@length 1..1, required), maxResults (@range 1..20, default 10), filters (MetadataFilterExpression document), output registryRecords (required). Both model ResourceNotFoundException httpError 404, documented "The requested resource was not found" — i.e. the registry named does not EXIST. After CreateRegistry returns a registryArn and GetRegistry reports status READY, that registry exists, so a discovery read against its id must answer 200 with the page of APPROVED records (RegistryRecordStatus doc: a record is "APPROVED once it is approved and discoverable"), never 404. Per the model the only ground for ResourceNotFoundException here is a registry id that names no registry of the account/region.

Emulator: every discovery read against a registry the control plane just created is refused with HTTP 404 ResourceNotFoundException, message "Registry not found: <identifier>" (fixed text "Registry not found: " plus the identifier echoed from the request — the per-run ids in the replay output, e.g. b25d0b126699 / 3f458ec14242 / 877aa78d3e9e, are just that run's values). This happens even though, in the same check, agent-registry-control CreateRegistry returned a registryArn, GetRegistry polled the registry to READY, CreateRegistryRecord returned recordArns and (with approvalConfiguration autoApprovalRules=["APPROVE_ALL"]) SubmitRegistryRecordForApproval reported APPROVED. The cause is structural, not a lookup bug: services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs documents its store as "populated by ComAmazonawsAgentregistryAgentregistryState::put_record — the seam a test or a fixture loader seeds through", and the handlers resolve the registry out of that crate-local store (state.registry(&input.registry_id) in src/ops/list_discoverable_registry_records.rs), which the agent-registry-control crate never writes to. The data-plane store is therefore permanently empty in a live emulator and the 404 is unconditional for every registry id a client can legitimately hold.

Why: discovery is the entire point of this service, and it is unreachable. A client that creates a registry, publishes records and approves them — exactly the documented AWS workflow — can never list, page or search them: it gets 404 ResourceNotFoundException for a resource its own successful control-plane calls just created and whose GetRegistry says READY. Because 404 is a terminal client error, an SDK does not retry it, and consumers (agent/MCP discovery clients, pagination loops, metadata-filtered searches) read "the registry does not exist" and abort. It also makes every downstream behavior of the three discovery operations unverifiable: recordType/descriptorType filtering and the descriptorTypes projection, maxResults=1 paging that must return each approved record exactly once and end without a nextToken, the filter refusals ValidationException "Filter recordType must specify exactly one value." and "Invalid value for filter recordType: NOTATYPE", and the customMetadata $eq narrowing of SearchDiscoverableRegistryRecords all sit behind this 404 and were never exercised.

Fix: the service's state/seeding seam, not the per-operation handlers. agent-registry's registry+record state must be the SAME state AgentRegistryControl mutates (one shared store in the engine, or the data plane reading the control-plane crate's store) so that CreateRegistry/CreateRegistryRecord/approval transitions are immediately visible to ListDiscoverableRegistryRecords, BatchGetDiscoverableRegistryRecord and SearchDiscoverableRegistryRecords, with the discovery projection (APPROVED only) applied at read time. Once reads resolve a real registry, re-check the filter/pagination/search behavior and the two filter-refusal messages above, which this 404 currently hides. Note: open issue 20261107T045822Z-agent-registry-listdiscoverableregistryrecords-validationexception-enumerates-the-filter-name-enum-value.md concerns only the ORDER of the enum value set inside one ValidationException message and does NOT cover this; the agent-registry routing issues (20261109T125435Z-..., 20261029T192216Z-...) concern pre-dispatch refusals and are not this either — requests now reach the handler and are refused by it on state.

Traces: agent-registry.list-discoverable-filters-by-record-and-descriptor-type, agent-registry.list-discoverable-pages-through-every-record-once, agent-registry.search-custom-metadata-filter-narrows-results. Code citations: services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs lines 3-11 (store "populated by put_record") and 109-120 (the "Registry not found: " refusal), src/ops/list_discoverable_registry_records.rs lines 218-225 (registry resolution).

## Evidence
Behavioral suite: 3/12 passed (25.0%), 0 mismatched, 9 errored, service agent-registry 3/12 (25%).

Three surviving failures, all the same refusal from the data plane after a successful control-plane setup:

1. agent-registry.list-discoverable-filters-by-record-and-descriptor-type
   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListDiscoverableRegistryRecords operation: Registry not found: b25d0b126699
   (setup had already done CreateRegistry(approvalConfiguration={autoApprovalRules:[APPROVE_ALL]}) -> registryArn, GetRegistry -> READY, three CreateRegistryRecord calls (AGENT/MCP/CUSTOM) and SubmitRegistryRecordForApproval -> status APPROVED for each.)

2. agent-registry.list-discoverable-pages-through-every-record-once
   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListDiscoverableRegistryRecords operation: Registry not found: 3f458ec14242
   (setup had created a READY registry and three records polled to status APPROVED through GetRegistryRecord.)

3. agent-registry.search-custom-metadata-filter-narrows-results
   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the SearchDiscoverableRegistryRecords operation: Registry not found: 877aa78d3e9e
   (setup had created a READY registry with customMetadataSchemaConfiguration and two APPROVE_ALL-submitted records.)

Expected (AWS): HTTP 200 with registryRecords containing the approved records — the filtered set for a recordType/descriptorType filter, one record per page for maxResults=1 ending with no nextToken, and the $eq-narrowed set for a customMetadata search.
Actual (emulator): HTTP 404 ResourceNotFoundException, message "Registry not found: <registryId as supplied>". Fixed text: "Registry not found: "; variable part: the identifier echoed from the request. The registry ids above are per-run values, not part of the contract.

Source corroboration that the two planes do not share state: services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs states the store is "populated by ComAmazonawsAgentregistryAgentregistryState::put_record — the seam a test or a fixture loader seeds through", and list_discoverable_registry_records.rs resolves the registry only from that crate-local store, so no AgentRegistryControl write can ever make a registry discoverable.

Not filed here: agent-registry.batch-get-reports-unpublished-records-per-entry, which the report marks ALREADY FILED (blocked by the unimplemented UpdateRegistryRecordStatus scaffold stub).

## Steps to Reproduce
1. Clone the repo and boot the emulator from the checkout; run the behavioral suite (3/12 pass).
2. With the agent-registry-control client: CreateRegistry(name=<unique>, approvalConfiguration={"autoApprovalRules":["APPROVE_ALL"]}) and take registryId from the returned registryArn after ':registry/'. Poll GetRegistry(registryId=...) until status == READY (it does).
3. CreateRegistryRecord(registryId=..., name=<unique>, recordType="CUSTOM", descriptors={"custom":{"data":"{}"}}) and SubmitRegistryRecordForApproval(registryId=..., recordId=...); observe status APPROVED.
4. With the agent-registry (data-plane) client: ListDiscoverableRegistryRecords(registryId=<that id>).
   Expect: HTTP 200, registryRecords containing the approved record (and nextToken absent).
   Observe: HTTP 404 ResourceNotFoundException, message "Registry not found: <that id>".
5. Same with SearchDiscoverableRegistryRecords(searchQuery="...", registryIds=[<that id>], maxResults=20): HTTP 404 "Registry not found: <that id>" instead of the matching approved records.
6. Repeat step 4 with maxResults=1 and with filters=[{"name":"recordType","values":["MCP"]}] — both are refused the same way, so neither paging nor filtering is reachable.
7. After sharing the registry/record state between AgentRegistryControl and AgentRegistry, re-replay the three checks in trace_ids and confirm the discovery reads serve only APPROVED records, page each exactly once, and refuse a multi-valued or unknown recordType filter with ValidationException as the checks assert.
