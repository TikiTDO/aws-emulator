Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-assets-and-connection
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-assets-and-connection outposts-outpost-lifecycle outposts-quote-and-order-lifecycle outposts-renewal-and-billing outposts-site-lifecycle outposts-capacity-task-lifecycle outposts-tag-resource-site-roundtrip
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|CreateSite|2xx->2xx|$.SiteId
Issue-Key: outposts|CreateOutpost|2xx->2xx|$.Outpost.AvailabilityZoneId
Issue-Key: outposts|UpdateSite|2xx->2xx|$.Site.Tags.resourceArn
Issue-Key: outposts|TagResource|2xx->2xx|status
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# outposts success responses omit server-set fields and use the wrong status on tagging

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSite

## Description
4 net-new root causes in success-path response rendering (no error classes involved; every status below is a 2xx on both sides except cause 4, which is the status itself).

Cause 1 of 4: CreateSite's 200 does not carry the new site's id where AWS carries it.
AWS: 200 whose body exposes the minted site id ($.SiteId in the recorded response) — it is the only place a caller learns the id of the site it just created, and every later call (CreateOutpost.SiteId, UpdateSite, TagResource on the site ARN) is addressed with it (Smithy: Site$SiteId, pattern ^(arn:...:site/)?(os-[a-f0-9]{17})$).
Emulator: 200 whose body has no value at that path, so the replay's binding of the created site id resolves to nothing.
Why: a client (and every fixture that chains off the create) cannot discover the site id from the create response; it must guess or re-list, which AWS never requires.
Fix: CreateSite's response rendering in services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/create_site.rs (which documents that it "mints a SiteId / SiteArn ... and answers the stored record") — the minted SiteId must be serialized at the path the recorded response uses.
Traces: outposts-assets-and-connection#setup#1 (cluster also covers outposts-outpost-lifecycle#task#1, outposts-quote-and-order-lifecycle#setup#1, outposts-renewal-and-billing#setup#1, outposts-site-lifecycle#setup#1).

Cause 2 of 4: CreateOutpost reports an AvailabilityZoneId that does not correspond to the requested Availability Zone.
AWS: 200 with Outpost.AvailabilityZoneId equal to the AZ-id of the Availability Zone the request named (Smithy: CreateOutpost input AvailabilityZone pattern ^([a-zA-Z]+-){1,3}([a-zA-Z]+)?(\d+[a-zA-Z]?)?$ and AvailabilityZoneId pattern ^[a-zA-Z]+\d-[a-zA-Z]+\d$; the response echoes the resolved pair). The id in the recording (usw2-az2) is the mapping of that request's AZ, not a constant.
Emulator: 200 with the region's FIRST AZ id (usw2-az1) regardless of the AZ requested — the request's AvailabilityZone/AvailabilityZoneId is not used to resolve the pair.
Why: a client that places an Outpost in a specific AZ reads back a different AZ id, so AZ-affinity decisions (subnet/AZ placement of instances on the Outpost) are made against the wrong zone.
Fix: CreateOutpost's AZ resolution in src/ops/create_outpost.rs — derive AvailabilityZoneId from the requested AvailabilityZone (and vice-versa) instead of defaulting.
Traces: outposts-assets-and-connection#setup#2 (cluster also covers outposts-capacity-task-lifecycle#setup#2, outposts-quote-and-order-lifecycle#setup#2, outposts-renewal-and-billing#setup#2).

Cause 3 of 4: UpdateSite's response drops the site's tags.
AWS: 200 with Site.Tags carrying the tags currently on the site, including tags applied by an earlier TagResource call (Smithy: Site$Tags, TagMap, "The site tags"); the key/value pair is whatever the caller applied — in the recording a tag keyed `resourceArn` whose value is the site's ARN.
Emulator: 200 whose Site.Tags does not contain the tag that was applied before the update (the key is absent).
Why: a client that updates a site and reads tags back from the same response sees them vanish, and a tag-then-update-then-read round trip loses state that AWS preserves.
Fix: UpdateSite's response rendering in src/ops/update_site.rs — answer the stored record including its Tags map rather than a tagless projection.
Traces: outposts-site-lifecycle#task#4.

Cause 4 of 4: TagResource answers 200 instead of 204.
AWS: 204 No Content with an empty body on a successful tag (recorded behavior; note the checked-in Smithy model declares TagResource http code 200 with an empty TagResourceResponse, so the model is stale here and the recorded 204 is the wire truth).
Emulator: 200.
Why: clients and SDK middleware that branch on 204 vs 200 (or assert "no content") see a different status class boundary than AWS returns; a conformance assertion on the tagging round trip fails against the emulator.
Fix: TagResource's success status in src/ops/tag_resource.rs (SUCCESS_STATUS on the generated Operation impl) — serve 204 with no body. Related open issue 20261024T134249Z-outposts-tagresource-validation covers ARN resolution, not the success status.
Traces: outposts-tag-resource-site-roundtrip#task#2.

## Evidence
Replay of simparity-wire-aws-v1:
- CreateSite (outposts-assets-and-connection#setup#1, 200 vs 200): '$.SiteId' missing — expected the bound site id ('{{asset_site_id}}'), got None. Cluster affects 5 fixtures (outposts-assets-and-connection#setup#1, outposts-outpost-lifecycle#task#1, outposts-quote-and-order-lifecycle#setup#1, outposts-renewal-and-billing#setup#1, outposts-site-lifecycle#setup#1).
- CreateOutpost (outposts-assets-and-connection#setup#2, 200 vs 200): '$.Outpost.AvailabilityZoneId' value_mismatch — expected 'usw2-az2', got 'usw2-az1'. Cluster affects 4 fixtures.
- UpdateSite (outposts-site-lifecycle#task#4, 200 vs 200): '$.Site.Tags.resourceArn' missing — expected 'arn:aws:outposts:us-west-2:000000000000:site/os-43226f8cb5ca1cda4', got None.
- TagResource (outposts-tag-resource-site-roundtrip#task#2): status expected 204, got 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Representative: fixture outposts-assets-and-connection, step setup#1 — POST /sites and inspect the 200 body for the minted site id at the path the recorded response uses ($.SiteId); it is absent.
3. Same fixture setup#2 — POST /outposts naming the second Availability Zone of the region, then read Outpost.AvailabilityZoneId: the emulator answers the region's first AZ id (usw2-az1) instead of the requested zone's id (usw2-az2).
4. outposts-site-lifecycle#task#4 — TagResource the site, then PATCH /sites/{SiteId}; the UpdateSite response's Site.Tags no longer contains the applied tag key.
5. outposts-tag-resource-site-roundtrip#task#2 — POST /tags/{ResourceArn} with a valid tag map and compare the status line: 200 instead of AWS's 204.
