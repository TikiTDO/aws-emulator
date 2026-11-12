Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-configure-egress-logs
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-configure-egress-logs mediapackage-vod-list-packaging-groups-pagination mediapackage-vod-packaging-configuration-multi-format mediapackage-vod-packaging-group-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod|CreatePackagingGroup|2xx->2xx|$.domainName
Issue-Key: mediapackage-vod|ConfigureLogs|2xx->2xx|$.domainName
Issue-Key: mediapackage-vod|DescribePackagingGroup|2xx->2xx|$.domainName
Issue-Key: mediapackage-vod|ListPackagingGroups|2xx->2xx|$.packagingGroups[*].domainName
Issue-Key: mediapackage-vod|UpdatePackagingGroup|2xx->2xx|$.domainName
Issue-Commit: c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3

# MediaPackage VOD packaging-group domainName advertises an unreachable egress host on every projection

> _Note: this issue was identified against the code base at commit `c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePackagingGroup

## Description
Wire replay of simparity-wire-aws-v1 against the booted emulator reports five distinct divergence clusters on the MediaPackage VOD (restJson1, sdkId "MediaPackage Vod") surface that all share ONE fault: every operation that projects a PackagingGroup fills the `domainName` member with an AWS-shaped egress hostname that is NOT an address the emulator answers on. The replay verdict for each is `host_not_own` on the `domainName` JSON path -- the advertised host is not the emulator's own endpoint, so the value is announced but not served. The fault is in the one place that renders a packaging group's egress domain, but it reaches the wire through five separate operations, and each operation must stop advertising a foreign host before it is closed. A fixer must close ALL FIVE causes before submitting.

=== Cause 1 of 5: CreatePackagingGroup projects a domainName the emulator does not serve ===
AWS: Smithy: CreatePackagingGroup POST /packaging_groups, code 200; output CreatePackagingGroupResponse member DomainName (jsonName `domainName`, target __string), alongside Arn/CreatedAt/Id/Authorization/EgressAccessLogs/Tags/ApproximateAssetCount. The model states no format constraint on the member, so: recorded behavior only -- in the recording the created group's `domainName` is the egress host for that group, i.e. a host that is actually reachable and serves the group's egress traffic. The fixture asserts the shape of the value and that the host is one the serving endpoint owns, not the recording's literal hostname.
Emulator: answers HTTP 200 (status matches) but returns `domainName` pointing at a per-group AWS production-style egress host of the form `https://<hex>.egress.mediapackage-vod.<region>.amazonaws.com` -- a name the emulator neither resolves to nor listens on. Replay detail: `$.domainName: host_not_own (expected '<shaped-not-asserted>', got 'https://<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com')`. The hex id, region and port are per-run values and are not requirements; the requirement is that the announced host is the endpoint the emulator is reached on.
Why: a client that reads `domainName` and then talks to it -- a playback path, a CDN origin configuration, a health probe, or any code that joins the domain with an egress path -- is sent to a real AWS hostname instead of the emulator. Against an offline or sandboxed environment that fails to resolve or is refused; against a networked one it leaves the emulator entirely and can hit third-party infrastructure. Either way nothing the emulator created is retrievable through the address it published.
Fix: the CreatePackagingGroup response projection in the mediapackagevod service crate -- specifically the helper that derives a packaging group's egress domain, which must be derived from the emulator's own advertised endpoint (the host/authority the request arrived on, or the configured public endpoint) rather than a synthesized amazonaws.com name.
Traces: mediapackage-vod-configure-egress-logs#setup#1 (also mediapackage-vod-list-packaging-groups-pagination#task#1, mediapackage-vod-list-packaging-groups-pagination#task#2, mediapackage-vod-packaging-configuration-multi-format#setup#1).

=== Cause 2 of 5: ConfigureLogs re-projects the same unreachable domainName ===
AWS: Smithy: ConfigureLogs PUT /packaging_groups/{Id}/configure_logs, code 200; output ConfigureLogsResponse carries DomainName (jsonName `domainName`). Recorded behavior only: configuring egress access logs returns the group's full projection with its reachable egress domain unchanged.
Emulator: answers HTTP 200 and echoes the same foreign egress host. Replay detail: `$.domainName: host_not_own (... got 'https://<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com')`.
Why: the logging-configuration round trip is exactly where a client re-reads the group's projection and caches the domain, so the unreachable host propagates into downstream configuration.
Fix: the ConfigureLogs handler's group projection in the mediapackagevod service crate, which must use the same own-endpoint-derived domain as Cause 1.
Traces: mediapackage-vod-configure-egress-logs#task#2 (also #task#4).

=== Cause 3 of 5: DescribePackagingGroup returns an unreachable domainName ===
AWS: Smithy: DescribePackagingGroup GET /packaging_groups/{Id}, code 200; output DescribePackagingGroupResponse member DomainName (jsonName `domainName`). Recorded behavior only: a describe reports the group's actual, served egress domain, and it is stable across describes of the same group.
Emulator: answers HTTP 200 with the same synthesized amazonaws.com egress host. Replay detail: `$.domainName: host_not_own`.
Why: Describe is the canonical read a client uses to discover where to fetch a group's output; a host the emulator does not own makes that discovery useless and makes the describe disagree with reality rather than report it.
Fix: the DescribePackagingGroup response projection / the stored packaging-group state in the mediapackagevod service crate -- if the domain is persisted at create time it must be persisted as an own-endpoint domain, otherwise derived per response.
Traces: mediapackage-vod-configure-egress-logs#task#3 (also #task#5, mediapackage-vod-packaging-group-lifecycle#task#3, mediapackage-vod-packaging-group-lifecycle#task#6).

=== Cause 4 of 5: ListPackagingGroups returns unreachable domainName on every listed member ===
AWS: Smithy: ListPackagingGroups GET /packaging_groups, code 200, paginated (inputToken/outputToken NextToken, items PackagingGroups, pageSize MaxResults with @range 1..1000); output member PackagingGroups (jsonName `packagingGroups`, __listOfPackagingGroup) whose elements carry DomainName. Recorded behavior only: each listed group reports the same served egress domain its Describe reports.
Emulator: answers HTTP 200 and each list element's domain is the foreign egress host. Replay detail: `$.packagingGroups[0].domainName: host_not_own (... 'https://<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com')` -- the index shown is just the first element; the fault is per element, not positional.
Why: a client that enumerates groups and fans out to their domains (inventory, bulk configuration, dashboards) is pointed off-emulator for every group at once, so the blast radius is the whole account's groups rather than one.
Fix: the ListPackagingGroups element projection in the mediapackagevod service crate, sharing the single own-endpoint domain helper from Cause 1 so paginated pages stay consistent.
Traces: mediapackage-vod-list-packaging-groups-pagination#task#3 (also #task#4, mediapackage-vod-packaging-group-lifecycle#task#4).

=== Cause 5 of 5: UpdatePackagingGroup returns an unreachable domainName ===
AWS: Smithy: UpdatePackagingGroup PUT /packaging_groups/{Id}, code 200; output UpdatePackagingGroupResponse member DomainName (jsonName `domainName`). Recorded behavior only: updating a group's Authorization returns the group's projection with its served egress domain preserved.
Emulator: answers HTTP 200 with the same synthesized amazonaws.com host. Replay detail: `$.domainName: host_not_own`.
Why: an update is the other write path a client re-reads the domain from, so a client that refreshes its cached origin after an authorization change re-adopts the unreachable host.
Fix: the UpdatePackagingGroup handler's group projection in the mediapackagevod service crate, using the shared own-endpoint domain helper.
Traces: mediapackage-vod-packaging-group-lifecycle#task#5.

Related open issue, NOT a duplicate: "MediaPackage VOD: egress manifest URL serves nothing, CreatePackagingGroup is not authorization-gated, and DeleteAsset answers the wrong success status" (20261022T132356Z) covers, as its Cause 1, that an ASSET's EgressEndpoints[].Url returns 404 -- an asset-level data-plane gap found behaviorally. This issue is about the PACKAGING GROUP's `domainName` member on five control-plane projections announcing a host the emulator does not own at all, which that issue does not cover. Also open and distinct: 20261025T172535Z (DeletePackagingGroup 204 Content-Length), 20261029T174428Z (filtered ListAssets body + empty-204 Content-Length), 20261108T141446Z (CreateAsset success status + CreatePackagingConfiguration validation message) -- the latter two already own the TagResource/UntagResource/DeletePackagingGroup Content-Length and the CreateAsset/CreatePackagingConfiguration clusters in this same replay, which is why those are not re-filed here.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the booted emulator (emulator_up=true, /__admin/health 200): 79 steps, 59 passed, 20 mismatched, score 74.7%, 10 divergence clusters -- all 10 on service `mediapackage-vod`. Five clusters are already owned by open issues (CreatePackagingConfiguration message, CreateAsset 202->200, DeletePackagingGroup/TagResource/UntagResource Content-Length) and are not re-filed. The five bundled here all carry verdict `mismatch` with the expected status MATCHING the actual status -- the divergence is purely the `domainName` value:

Cause 1 -- mediapackage-vod-configure-egress-logs#setup#1, CreatePackagingGroup, status 200 -> 200
  expected: $.domainName = a group egress host owned/served by the responding endpoint (<shaped-not-asserted>)
  actual:   $.domainName = 'https://<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com' -- verdict host_not_own
  cluster also covers: mediapackage-vod-list-packaging-groups-pagination#task#1, #task#2, mediapackage-vod-packaging-configuration-multi-format#setup#1 (affected_count 4)
  divergence_key: service=mediapackage-vod|CreatePackagingGroup|status=200->200|keys=$.domainName

Cause 2 -- mediapackage-vod-configure-egress-logs#task#2, ConfigureLogs, status 200 -> 200
  $.domainName: host_not_own -- host 'https://<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com' is not the responding endpoint's own
  cluster also covers: mediapackage-vod-configure-egress-logs#task#4 (affected_count 2)
  divergence_key: service=mediapackage-vod|ConfigureLogs|status=200->200|keys=$.domainName

Cause 3 -- mediapackage-vod-configure-egress-logs#task#3, DescribePackagingGroup, status 200 -> 200
  $.domainName: host_not_own
  cluster also covers: mediapackage-vod-configure-egress-logs#task#5, mediapackage-vod-packaging-group-lifecycle#task#3, #task#6 (affected_count 4)
  divergence_key: service=mediapackage-vod|DescribePackagingGroup|status=200->200|keys=$.domainName

Cause 4 -- mediapackage-vod-list-packaging-groups-pagination#task#3, ListPackagingGroups, status 200 -> 200
  $.packagingGroups[0].domainName: host_not_own ('https://<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com')
  cluster also covers: mediapackage-vod-list-packaging-groups-pagination#task#4, mediapackage-vod-packaging-group-lifecycle#task#4 (affected_count 3)
  divergence_key: service=mediapackage-vod|ListPackagingGroups|status=200->200|keys=$.packagingGroups[0].domainName

Cause 5 -- mediapackage-vod-packaging-group-lifecycle#task#5, UpdatePackagingGroup, status 200 -> 200
  $.domainName: host_not_own
  affected_count 1
  divergence_key: service=mediapackage-vod|UpdatePackagingGroup|status=200->200|keys=$.domainName

Note on the recorded values: in Causes 2-5 the expected and actual strings read identically because the recording's own egress hostname is reproduced verbatim by the emulator; the replay still fails them because the assertion is that the announced host belongs to the endpoint actually serving the request. Hostnames, the <hex> group discriminator, the region and the listener port are per-environment values and are NOT requirements -- the requirement is that `domainName` names the address the emulator is reached on.

Model citations from services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json (restJson1, sigv4): CreatePackagingGroup POST /packaging_groups code 200; ConfigureLogs PUT /packaging_groups/{Id}/configure_logs code 200; DescribePackagingGroup GET /packaging_groups/{Id} code 200; UpdatePackagingGroup PUT /packaging_groups/{Id} code 200; ListPackagingGroups GET /packaging_groups code 200 (paginated, items PackagingGroups); DomainName -> jsonName `domainName`, target __string, no modeled format constraint.

## Steps to Reproduce
1. git clone the project repo and build/boot the emulator from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus) and read the mediapackage-vod divergence clusters.
3. Cause 1: replay fixture mediapackage-vod-configure-egress-logs step setup#1 -- POST /packaging_groups with an id (and tags) and read `domainName` from the 200 body. Observe a host of the form <hex>.egress.mediapackage-vod.<region>.amazonaws.com; confirm it is not the host:port the request was sent to, and that connecting to it does not reach the emulator.
4. Cause 2: replay mediapackage-vod-configure-egress-logs step task#2 -- PUT /packaging_groups/{Id}/configure_logs with egressAccessLogs and read `domainName` from the 200 body: same foreign host.
5. Cause 3: replay mediapackage-vod-configure-egress-logs step task#3 -- GET /packaging_groups/{Id} and read `domainName`: same foreign host (reproduced by mediapackage-vod-packaging-group-lifecycle#task#3 and #task#6).
6. Cause 4: replay mediapackage-vod-list-packaging-groups-pagination step task#3 -- GET /packaging_groups (with maxResults to page) and read `packagingGroups[*].domainName`: every element carries a foreign host.
7. Cause 5: replay mediapackage-vod-packaging-group-lifecycle step task#5 -- PUT /packaging_groups/{Id} with an authorization update and read `domainName`: same foreign host.
8. After fixing: re-replay the four fixtures named above (mediapackage-vod-configure-egress-logs, mediapackage-vod-list-packaging-groups-pagination, mediapackage-vod-packaging-group-lifecycle, mediapackage-vod-packaging-configuration-multi-format) and confirm all 12 affected steps pass, i.e. no `host_not_own` verdict remains on any `domainName` path and the statuses stay 200.
