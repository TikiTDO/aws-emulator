Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-catalog-list-catalog-items
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-catalog-list-catalog-items outposts-catalog-and-instance-types-browsing
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|ListCatalogItems|2xx->2xx|$.CatalogItems[*]
Issue-Commit: cf4b8496ba0c958ea150f5d2d9329060f4b043af

# outposts ListCatalogItems serves a synthesized catalog instead of the AWS-published catalog items

> _Note: this issue was identified against the code base at commit `cf4b8496ba0c958ea150f5d2d9329060f4b043af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCatalogItems

## Description
ListCatalogItems (GET /catalog/items) is a read of the service-published AWS Outposts hardware catalog: the same fixed set of catalog items, with the same per-item hardware facts, for every caller. The emulator instead projects a locally synthesized item table whose membership, ids and hardware attributes it makes up, so the catalog read answers 200 with content that is not AWS's catalog. This bundle has 2 causes, both on ListCatalogItems.

Cause 1 of 2: the unfiltered catalog listing is wholly emulator-authored.
AWS: recorded behavior only for the item contents - the Smithy model defines the shapes but not the published rows (ListCatalogItems -> ListCatalogItemsOutput{CatalogItems: CatalogItemListDefinition, NextToken}, http GET /catalog/items, code 200; CatalogItem members CatalogItemId (SkuCode, @length 1-10, @pattern ^OR-[A-Z0-9]{7}$), ItemClass (enum RACK|SERVER), ItemStatus, Dimensions, RackUnitHeight, RackScalingType, CurrentType, EC2Capacities, PowerKva, WeightLbs, SupportedUplinkGbps, SupportedUplinkGiB, SupportedStorage). The recording shows an unfiltered read answering 200 with a fixed list of published items, each a concrete AWS SKU with its own hardware facts - e.g. a MULTI_RACK, HEIGHT_42U OUTPOST_RACK item whose EC2Capacities names a single c5 family at 24xlarge with quantity 1, with its own PowerKva and WeightLbs, and a second rack item carrying m7i/r7i capacities. The catalog is service data: the SKU ids, the capacity families and quantities, the power/weight numbers, the uplink and storage option lists and ItemStatus are properties of the published item, not of the request.
Emulator: answers 200 but every element of the list is a different item from the one recorded at that position, and the whole list diverges - the replay reports every index of CatalogItems (0 through 36) as missing/mismatched. At index 0 the emulator serves a rack SKU of its own minting whose EC2Capacities is a seven-family block (c5, c5d, i3en, m5, m5d, r5, r5d) all at 24xlarge with an identical quantity, with PowerKva and WeightLbs values that match no published item; at index 1 it serves an item whose ItemStatus is DISCONTINUED and whose SupportedUplinkGiB is a different list from the recorded one. The values look generated from a template (uniform families, uniform quantities) rather than read from a catalog table.
Why: a client that browses the catalog to pick hardware - choose a SKU, size EC2 capacity, check power/weight/uplink/storage feasibility, then pass that CatalogItemId to CreateOrder/CreateQuote - gets ids that do not exist in AWS and capacity/power/weight figures that are not the item's. Any selection logic, test assertion or ordering flow keyed on a real SKU or on an item's attributes fails against the emulator while passing against AWS, and the DISCONTINUED status the emulator invents makes items look unorderable that AWS publishes as AVAILABLE.
Fix: the outposts catalog seed/state that the ListCatalogItems handler projects - it must hold the service-published item rows verbatim (ids, item class, status, dimensions, rack scaling, current type, EC2 capacities, power, weight, uplink lists, storage options) in the published order, instead of generating rows.
Traces: outposts-catalog-list-catalog-items#task#1 (CatalogItems[0]..CatalogItems[36]).

Cause 2 of 2: a filtered catalog listing returns an item the filter excludes.
AWS: recorded behavior only; Smithy: ListCatalogItems input ItemClassFilter (CatalogItemClassList, @httpQuery ItemClassFilter), SupportedStorageFilter (@httpQuery SupportedStorageFilter), EC2FamilyFilter (@httpQuery EC2FamilyFilter), MaxResults (1-1000). The recording shows the filtered read answering 200 with exactly the items that satisfy the filter - three items, and no fourth element.
Emulator: answers 200 with one element too many: CatalogItems[3] is populated where the recording has no such element (an extra SERVER-class, OUTPOST_SERVER_2U item). The filter is therefore applied too loosely, or an extra synthesized row leaks past it.
Why: a client that filters the catalog (by item class, supported storage or EC2 family) and then iterates or indexes the result processes an item that does not meet the filter, so counts, paging offsets and "first matching item" selections differ from AWS. Combined with cause 1 it means neither the membership nor the attributes of a filtered catalog read can be relied on.
Fix: the ListCatalogItems handler's filter evaluation over the catalog table (ItemClassFilter / SupportedStorageFilter / EC2FamilyFilter must each restrict membership, and only published rows may be listed).
Traces: outposts-catalog-and-instance-types-browsing#task#1 (CatalogItems[3] present where AWS returns none).

No open issue covers these: the existing outposts issues are about lifecycle/precondition gates (20261029T191039Z-outposts-lifecycle-and-precondition-gates-diverge-across-capacity.md), omitted server-set fields and the tagging status (20261029T191039Z-outposts-success-responses-omit-server-set-fields-and.md), ListAssetInstances, CreateOutpost generation fields, UpdateOutpost validation and a GetOutpostBillingInformation stub - none of them touch the catalog read path.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true, 89 steps, 18 mismatched).

Cause 1 - fixture outposts-catalog-list-catalog-items, step task#1, ListCatalogItems, expected 200 / actual 200, divergence_key service=outposts|ListCatalogItems|status=200->200|keys=$.CatalogItems[0]..$.CatalogItems[36]:
  $.CatalogItems[0]: missing - expected {'CatalogItemId': 'OR-FWTFFXJ', 'ItemClass': 'RACK', 'ItemStatus': 'AVAILABLE', 'Dimensions': 'OUTPOST_RACK', 'RackUnitHeight': 'HEIGHT_42U', 'RackScalingType': 'MULTI_RACK', 'CurrentType': 'AC', 'EC2Capacities': [{'Family': 'c5', 'MaxSize': '24xlarge', 'Quantity': '1'}], 'PowerKva': 6.39, 'WeightLbs': 1328, 'SupportedUplinkGbps': [10, 40, 100], 'SupportedUplinkGiB': [10, 40, 100], 'SupportedStorage': ['EBS', 'S3']}; got CatalogItemId 'OR-7Q7BLB6' with EC2Capacities [c5, c5d, i3en, m5, m5d, r5, r5d] each 24xlarge/Quantity '15', PowerKva 15.0, WeightLbs 1800.
  $.CatalogItems[1]: missing - expected CatalogItemId 'OR-G4Q39QN' (EC2Capacities m7i 48xlarge x2, r7i 48xlarge x5; PowerKva 13.3; WeightLbs 1432; SupportedUplinkGiB [10,40,100]); got CatalogItemId 'OR-8RC2ZKS', ItemStatus 'DISCONTINUED', SupportedUplinkGiB [1, 10].
  The same verdict is reported for every remaining index through $.CatalogItems[36] - i.e. no element of the served list matches the recorded catalog.
  (The ids, quantities, power and weight values above are one recording's instance of the published catalog, cited to show the shape of the divergence; the requirement is that the served rows are the AWS-published rows, not these literals.)

Cause 2 - fixture outposts-catalog-and-instance-types-browsing, step task#1, ListCatalogItems, expected 200 / actual 200, divergence_key service=outposts|ListCatalogItems|status=200->200|keys=$.CatalogItems[3]:
  $.CatalogItems[3]: value_mismatch - expected None, got {'CatalogItemId': 'OR-GB94EOS', 'ItemClass': 'SERVER', 'ItemStatus': 'AVAILABLE', 'Dimensions': 'OUTPOST_SERVER_2U', 'RackUnitHeight': 'HEIGHT_2U', 'CurrentType': 'AC', 'EC2Capacities': [{'Family': 'c6gd', 'MaxSize': '16xlarge', 'Quantity': '2'}, {'Family': 'c6id', 'MaxSize': '32xlarge', 'Quantity': '1'}], 'PowerKva': 1.0, 'WeightLbs': 70, 'SupportedUplinkGbps': [1, 10], 'SupportedUplinkGiB': [1, 10], 'SupportedStorage': ['EBS']}
  Indices 0-2 matched, so the list is one element longer than AWS's for this filtered read.

Neither cluster carries a stub_path or an owned_by, so neither is already filed.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture outposts-catalog-list-catalog-items step task#1 - GET /catalog/items (ListCatalogItems) with no filters. Compare the returned CatalogItems list element by element against the recorded response: every index diverges (synthesized SKU ids, uniform multi-family EC2Capacities, invented PowerKva/WeightLbs, DISCONTINUED status).
3. Cause 2: replay fixture outposts-catalog-and-instance-types-browsing step task#1 - the filtered ListCatalogItems read. The emulator returns 4 CatalogItems where AWS returns 3; the extra element (index 3) is a SERVER / OUTPOST_SERVER_2U item the filter excludes.
4. Both steps answer HTTP 200, so the divergence is purely in the served catalog content and membership.
