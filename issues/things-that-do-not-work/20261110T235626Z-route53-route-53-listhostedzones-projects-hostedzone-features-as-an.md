Issue-Source: trace-sweep-divergence
Issue-Trace: route53-list-hosted-zones-membership-and-by-vpc
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-list-hosted-zones-membership-and-by-vpc
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|ListHostedZones|2xx->2xx|$.ListHostedZonesResponse.HostedZones.HostedZone[Name="sim2real-probe-route53-lhza-28e1ffd6.com."].Features
Issue-Key: route53|ListHostedZones|2xx->2xx|$.ListHostedZonesResponse.HostedZones.HostedZone[Name="sim2real-probe-route53-lhzb-08df0e19.com."].Features
Issue-Commit: 398fd4ec9a3a28759f8da7a0cb8532c03f56321f

# Route 53 ListHostedZones projects HostedZone.Features as an empty element instead of the zone's feature statuses

> _Note: this issue was identified against the code base at commit `398fd4ec9a3a28759f8da7a0cb8532c03f56321f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostedZones

## Description
Cause 1 of 1: the ListHostedZones hosted-zone projection emits a HostedZone.Features container with no content, where AWS populates it with the zone's feature statuses. Fixture route53-list-hosted-zones-membership-and-by-vpc, step task#6, operation ListHostedZones, status 200 vs 200.

AWS: recorded behavior only for the member value. The checked-in Smithy model (com.amazonaws.route53#HostedZone) declares the optional member `Features` targeting the structure `com.amazonaws.route53#HostedZoneFeatures`, documented as "The features configuration for the hosted zone, including accelerated recovery settings and status information"; the model's own HostedZoneFeatures carries only `AcceleratedRecoveryStatus` (enum) and `FailureReasons` (list). The recording shows the live service additionally reports a feature-status member `PublicDNSFallbackStatus` inside that structure: for every hosted zone returned by ListHostedZones the recorded restXml body carries `HostedZone/Features` with a child element whose value is the string `DISABLED` for a zone on which that feature has not been enabled (both zones in the recorded step, sim2real-probe-route53-lhza-* and sim2real-probe-route53-lhzb-*, report the same). The requirement is therefore: when Route 53 serializes a HostedZone in ListHostedZones it reports the zone's feature statuses as child elements of Features, with the per-feature status value taken from the zone's own state (the not-enabled status being DISABLED); the model's HostedZoneFeatures shape is behind the wire and must be extended with the feature-status member the recording shows rather than treated as the complete set. No error is involved; the step is a 200 on both sides.

Emulator: the ListHostedZones response serializes `Features` as an empty element for every zone. Replay detail, for both zones of the recorded step: `$.ListHostedZonesResponse.HostedZones.HostedZone[Name="sim2real-probe-route53-lhza-28e1ffd6.com."].Features: type_mismatch (expected {'PublicDNSFallbackStatus': 'DISABLED'}, got '')` and the same for `...lhzb-08df0e19.com.`. The emulator opens the Features container unconditionally but writes no member into it, so a structure is expected and an empty string is produced — a type mismatch, not merely a missing child.

Why: an SDK client deserializing the emulator's body gets a HostedZoneFeatures object (or, under a strict XML/structure deserializer, a parse/type error) whose feature-status members are all null. Code that reads a zone's DNS-fallback / feature posture from the listing — to decide whether a feature must be enabled, to render the posture of every zone in an account, or to assert the DISABLED baseline before a toggle — sees no status at all instead of DISABLED, so it either reports the posture as unknown for every hosted zone or dereferences a null status. Golden-response and strict shape comparators fail on the type change as well.

Fix: the hosted-zone projection / restXml serializer for ListHostedZones in the route53 service crate (the shared HostedZone projection, since the same structure is serialized by the other hosted-zone read paths), together with the HostedZoneFeatures shape it serializes: carry the per-zone feature statuses in hosted-zone state and emit them as children of Features, so a zone whose feature is not enabled serializes that feature's status as DISABLED. If no feature status is known for a zone, omit the optional Features member entirely rather than emitting an empty container.

Related but NOT covering this: open issue 20261108T200652Z-route53-route-53-gethostedzone-serializes-an-empty-hostedzone-features.md covers GetHostedZone emitting an empty Features element where AWS omits the member. That is the opposite requirement on a different operation — here AWS DOES send Features and the emulator's empty container is wrong because it is empty, not because it is present — so closing that issue by suppressing an empty Features would leave this listing divergence (and could make it worse). The route53 reference-data, CIDR-ARN and NoSuchTrafficPolicy issues are unrelated.

Traces: route53-list-hosted-zones-membership-and-by-vpc#task#6 (ListHostedZones, 200 vs 200, two HostedZone[].Features entries type_mismatch).

## Evidence
Replay divergence (verdict mismatch, cluster size 1):
  fixture_id: route53-list-hosted-zones-membership-and-by-vpc
  step_key: task#6
  service: route53, operation: ListHostedZones
  expected_status 200, actual_status 200
  detail:
    $.ListHostedZonesResponse.HostedZones.HostedZone[Name="sim2real-probe-route53-lhza-28e1ffd6.com."].Features: type_mismatch (expected {'PublicDNSFallbackStatus': 'DISABLED'}, got '')
    $.ListHostedZonesResponse.HostedZones.HostedZone[Name="sim2real-probe-route53-lhzb-08df0e19.com."].Features: type_mismatch (expected {'PublicDNSFallbackStatus': 'DISABLED'}, got '')

Expected (AWS, recorded): every HostedZone in ListHostedZonesResponse carries a Features element containing a feature-status child reporting DISABLED for the feature that is not enabled on that zone. Zone names above are per-run fixture values; the requirement is the element and its status value, not those names.
Actual (emulator): every HostedZone carries Features as an empty element (no child members), i.e. a string where a structure is expected.

Model evidence: Smithy com.amazonaws.route53#HostedZone member Features -> com.amazonaws.route53#HostedZoneFeatures (optional), doc "The features configuration for the hosted zone, including accelerated recovery settings and status information"; the checked-in HostedZoneFeatures declares only AcceleratedRecoveryStatus and FailureReasons and has no PublicDNSFallbackStatus member, so the recorded wire member is model drift and the recording is the authority for it.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture `route53-list-hosted-zones-membership-and-by-vpc` and stop at step `task#6`.
   Equivalent by hand: create two public hosted zones with the fixture's setup steps, then call route53 ListHostedZones for the account.
3. Inspect the restXml body of ListHostedZonesResponse and the `<Features>` element of each `<HostedZone>`.
4. Observed: `<Features></Features>` — an empty container on every zone.
   Expected: `<Features>` containing the zone's feature statuses, with the not-enabled feature reported as `DISABLED` (recorded as PublicDNSFallbackStatus=DISABLED on both zones). Status is 200 either way.
