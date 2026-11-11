Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-assets-and-connection
Issue-Step: setup#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-assets-and-connection outposts-capacity-task-lifecycle outposts-quote-and-order-lifecycle outposts-renewal-and-billing outposts-site-lifecycle
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|CreateOutpost|2xx->2xx|$.Outpost.AvailabilityZoneId
Issue-Key: outposts|UpdateSite|2xx->2xx|$.Site.Tags.resourceArn
Issue-Commit: 552123697b202fc6b750c729592b678c1a752764

# outposts UpdateSite drops the site's tags and CreateOutpost resolves the wrong AvailabilityZoneId

> _Note: this issue was identified against the code base at commit `552123697b202fc6b750c729592b678c1a752764`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOutpost

## Description
Two independent response-projection defects in the outposts emulator: a created Outpost reports an Availability Zone ID that does not pair with the zone the request named as AWS pairs it, and an updated site reports a tag map that has lost tags the site carries. BOTH MUST BE FIXED AND RE-REPLAYED (2 causes).

Cause 1 of 2: CreateOutpost reports the wrong Outpost$AvailabilityZoneId (fixture outposts-assets-and-connection, step setup#2; also setup#2 of outposts-capacity-task-lifecycle, outposts-quote-and-order-lifecycle and outposts-renewal-and-billing).
  AWS: Smithy: CreateOutpost takes optional AvailabilityZone (AvailabilityZone, len 1..1000, pattern ^([a-zA-Z]+-){1,3}([a-zA-Z]+)?(\d+[a-zA-Z]?)?$) and optional AvailabilityZoneId (AvailabilityZoneId, len 1..255, pattern ^[a-zA-Z]+\d-[a-zA-Z]+\d$), and answers 200 with Outpost, whose Outpost$AvailabilityZone and Outpost$AvailabilityZoneId are the two identities of the one zone the Outpost stands in. The model publishes no formula pairing a zone NAME with a zone ID, so this is recorded behavior only: for the CreateOutpost request this fixture step makes (a zone of us-west-2), the recorded AWS response is 200 with $.Outpost.AvailabilityZoneId = 'usw2-az2'. The requirement is that the zone ID reported for a created Outpost is the one AWS pairs with the zone the request named, and that every later read of the Outpost reports that same pair.
  Emulator: answers 200 but reports $.Outpost.AvailabilityZoneId = 'usw2-az1' for the same request. The id is not taken from AWS's pairing at all: when the request names only the zone NAME, the handler derives the id through a fixed bijection of its own — services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:3140-3149 (`zone_index`) pairs '...a' with '-az1', '...b' with '-az2', and the comment there states that AWS 'publishes no formula for the index, so this service fixes ONE stable bijection'. That self-chosen bijection does not reproduce the pairing the corpus records, so the zone ID is off by one zone for every Outpost created in the corpus.
  Why: AvailabilityZoneId is the region-stable identifier clients join an Outpost to the rest of their infrastructure by (subnets, EC2 capacity, zone-pinned placement). A client that reads the zone ID off CreateOutpost/GetOutpost and resolves subnets or capacity in that zone resolves a DIFFERENT zone than the Outpost is in, and any client that cross-checks the Outpost's AvailabilityZone against its AvailabilityZoneId sees the two disagree with what AWS reports.
  Fix: the Availability-Zone identity resolution shared by CreateOutpost's handler and the Outpost projection — the zone-name/zone-id mapping in the service's state module, not the serializer: the Outpost is stored carrying both identities, so the stored pair has to be the one AWS reports before the reads can agree with it.
  Traces: outposts-assets-and-connection#setup#2 (representative), outposts-capacity-task-lifecycle#setup#2, outposts-quote-and-order-lifecycle#setup#2, outposts-renewal-and-billing#setup#2.

Cause 2 of 2: UpdateSite answers a Site whose tag map has lost the site's tags (fixture outposts-site-lifecycle, step task#4).
  AWS: Smithy: UpdateSite (PATCH /sites/{SiteId}) answers 200 with Site; Site$Tags targets TagMap ('The site tags'), keys TagKey (len 1..128, pattern ^(?!aws:)[a-zA-Z+-=._:/]+$), values TagValue (len 0..256). A site's tags live on the site, so every projection of it — GetSite, ListSites and the Site that UpdateSite answers — reports the tags the site carries at that moment, including the ones an earlier TagResource call applied; UpdateSite touches only Name, Description and Notes and must not drop any other member. Recorded: the step's response is 200 with $.Site.Tags carrying the tag entry applied earlier in the fixture. The requirement is the RULE (previously applied tags are still reported by the update's Site); the fixture's particular entry — key 'resourceArn', value the site's own ARN 'arn:aws:outposts:us-west-2:000000000000:site/os-6ae42cd5d570b7999' — is that fixture's data: the key comes from the earlier tagging request and the value from the site the fixture created, and neither literal is part of the contract.
  Emulator: answers 200, but $.Site.Tags is missing that key entirely ('$.Site.Tags.resourceArn: missing (expected ..., got None)'): the map is reported, so Site$Tags is present and EMPTY of the tag the site should carry.
  Why: a client that tags a site and then updates it reads the update's Site back as the site's current state (that is what the handler's own doc promises: 'exactly what GetSite and ListSites report afterwards'). With the tags silently gone, tag-driven inventory, cost allocation and tag-based access/selection logic see an untagged site, and a client that re-asserts tags from that reply wipes or re-adds them in a loop.
  Fix: the site tag state and the Site projection UpdateSite answers from — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/update_site.rs:191-193 answers `record.site.clone()`, so the entry is absent from the stored record's tag map by the time the update runs. Hypothesis: the tag the fixture applies before this step is not persisted onto this site's stored tag map (crate::state::store::tags_mut, state/store.rs:432-444, writes into `record.site.tags`), or it is written against a record this update does not resolve to; the fixer should re-replay outposts-site-lifecycle and follow the tag from the tagging step into the stored site.
  Traces: outposts-site-lifecycle#task#4.

Neither cause is covered by an open issue: the open CreateOutpost issue 20261028T100620Z-outposts-createoutpost-wrong-behavior.md is about Outpost$Generation/$RackScalingType being permanently absent and says nothing about the Availability Zone identities, and 20261029T191039Z-outposts-lifecycle-and-precondition-gates-diverge-across-capacity.md owns the quote/renewal/private-connectivity gate steps, not these two. No open issue mentions site tags or UpdateSite.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 89 steps, 77 passed):

Cause 1 — divergence_key service=outposts|CreateOutpost|status=200->200|keys=$.Outpost.AvailabilityZoneId; fixture outposts-assets-and-connection, step setup#2, affected_count 4:
  expected status 200, actual status 200
  $.Outpost.AvailabilityZoneId: value_mismatch (expected 'usw2-az2', got 'usw2-az1')
  affected: outposts-assets-and-connection#setup#2, outposts-capacity-task-lifecycle#setup#2, outposts-quote-and-order-lifecycle#setup#2, outposts-renewal-and-billing#setup#2
  code: state.rs:3140-3149 `zone_index` fixes its own '...a'<->'-az1' bijection; create_outpost.rs:247-258 derives the missing identity from it.

Cause 2 — divergence_key service=outposts|UpdateSite|status=200->200|keys=$.Site.Tags.resourceArn; fixture outposts-site-lifecycle, step task#4:
  expected status 200, actual status 200
  $.Site.Tags.resourceArn: missing (expected 'arn:aws:outposts:us-west-2:000000000000:site/os-6ae42cd5d570b7999', got None)
  code: ops/update_site.rs:187-193 patches name/description/notes and answers `record.site.clone()`; state/store.rs:408-444 is the only place tags are read/written for a site.

Neither cluster carries a stub_path or an owned_by, i.e. no unimplemented operation blocks them and no open issue carries their replay keys.

## Steps to Reproduce
Cause 1 (CreateOutpost AvailabilityZoneId):
1. Replay fixture outposts-assets-and-connection against the emulator and stop at step setup#2 (CreateOutpost on the site created in setup, naming a zone of us-west-2).
2. Read $.Outpost.AvailabilityZoneId in the 200 response: the emulator reports 'usw2-az1' where the recorded AWS response reports 'usw2-az2'.
3. GetOutpost/ListOutposts on that Outpost report the same wrong id, since both identities are stored at create time.
4. The same step of outposts-capacity-task-lifecycle, outposts-quote-and-order-lifecycle and outposts-renewal-and-billing reproduces it identically.

Cause 2 (UpdateSite tags):
1. Replay fixture outposts-site-lifecycle in order: create the site, apply the tag the fixture applies to it, then reach step task#4 (UpdateSite on that site).
2. Read $.Site.Tags in the 200 response: the tag entry applied before the update is absent, where the recorded AWS response reports it.
3. Equivalently, by hand: CreateSite, TagResource the site's ARN with one tag, then UpdateSite changing only Name — the answered Site carries an empty tag map instead of the tag just applied.
