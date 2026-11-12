Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconnect-catalog-list-offerings
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconnect-catalog-list-offerings
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect|ListOfferings|2xx->2xx|$.offerings[*]
Issue-Commit: 06427132ab34f0591e7437b64f91b88a57797a82

# ListOfferings serves a synthetic reservation catalog instead of the AWS offering catalog

> _Note: this issue was identified against the code base at commit `06427132ab34f0591e7437b64f91b88a57797a82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListOfferings

## Description
One net-new wire divergence from the replay of trace set simparity-wire-aws-v1 against the MediaConnect emulator, on service `mediaconnect`.

=== Cause 1 of 1: ListOfferings returns a locally generated bandwidth/price schedule, so every entry of the reservation catalog differs from AWS's ===
AWS: Smithy: ListOfferings, GET /v1/offerings, success code 200, output {NextToken (jsonName nextToken), Offerings (jsonName offerings, list of Offering)}; paginated (inputToken NextToken / outputToken NextToken / items Offerings / pageSize MaxResults), readonly, MaxResults @range 1..1000. Offering requires CurrencyCode, Duration, DurationUnits (enum: MONTHS only), OfferingArn, OfferingDescription, PricePerUnit, PriceUnits (enum: HOURLY only) and ResourceSpecification {ReservedBitrate, ResourceType @required}; the shape docs describe an offering as "a savings plan that reserves a certain amount of outbound bandwidth usage at a discounted rate", with ResourceSpecification "a definition of the amount of outbound bandwidth that you would be reserving if you purchase the offering". The model does not enumerate the catalog itself, so for the catalog content this is recorded behavior only: ListOfferings is a read-only, account-independent price list that MediaConnect publishes per region. The recording shows a 12-entry catalog of Mbps_Outbound_Bandwidth offerings, all duration 12 MONTHS / HOURLY / USD, whose reserved-bitrate tiers and per-tier rates are AWS's published values and are NOT a linear function of bitrate — e.g. 50 Mbps at pricePerUnit '1.161', 150 Mbps at '2.834', 1500 Mbps at '11.444' (the rate per Mbps falls as the tier rises, which is the whole point of a savings plan) — with offeringDescription rendered as "Reserved outbound bandwidth of <reservedBitrate>Mbps at $<pricePerUnit>/hour" from that entry's own values, and offeringArn in the form arn:aws:mediaconnect:<region>:offering:<catalog offering id> (no account id). The catalog is the same for every caller in a region, so the tier set and the rate attached to each tier are the requirement; the individual offering ids in the recording are catalog identifiers, not per-run values, but what a client depends on is that the tiers and rates it is quoted are AWS's.
Emulator: the step answered HTTP 200 (status matched) with a catalog it manufactures itself: all 12 entries diverge ($.offerings[0] through $.offerings[11] in the divergence key). The served entries are an arithmetic bitrate ladder with a linear price schedule and ids derived from the bitrate — offerings[0] is 50 Mbps at pricePerUnit '1.200' with offeringArn arn:aws:mediaconnect:us-west-2:offering:12000050, offerings[1] is 100 Mbps at '2.300' with offeringArn ...:offering:12000100 — where AWS's offerings[1] at that position is a 1500 Mbps tier at '11.444'. So both the SET of reserved-bitrate tiers on offer and the RATE quoted for each tier are wrong, and offeringDescription repeats the wrong numbers ("Reserved outbound bandwidth of 50Mbps at $1.200/hour" vs AWS's "...at $1.161/hour"). The per-entry structure, enums, currency, duration and description template are right; only the catalog data is invented.
Why: ListOfferings is how a client discovers what it can reserve and at what price, and the offeringArn it picks is the input to DescribeOffering and PurchaseOffering. Against this emulator a cost/capacity planner is quoted bandwidth tiers AWS does not sell and rates AWS does not charge, so any computation over the response (cheapest tier that covers N Mbps, projected monthly cost, rate-per-Mbps curve) comes out different from production, and a test that pins an AWS offering id or an expected rate fails against the emulator while the same code works live. Because the emulator's prices are linear in bitrate, the discount-at-scale behavior that makes a reservation worth buying is absent from the data entirely.
Fix: the ListOfferings handler's backing state — the seeded offering catalog for the MediaConnect service — should hold AWS's published per-region offering list (tier bitrates with their real pricePerUnit and catalog offering ids) rather than computing ids and prices from the bitrate, and the handler should keep projecting offeringDescription from each stored entry's own reservedBitrate and pricePerUnit so description, rate and specification stay mutually consistent. DescribeOffering (GET /v1/offerings/{OfferingArn}) reads the same catalog and must resolve the same ids; the related stub issue 20261024T010542Z-mediaconnect-purchaseoffering-stub.md (PurchaseOffering unimplemented) consumes an offeringArn from this catalog, and no open issue covers the catalog's contents.
Traces: mediaconnect-catalog-list-offerings#task#1.

## Evidence
Replay of simparity-wire-aws-v1: 8 fixtures, 50 steps, 46 passed, 4 mismatched (score 92%), emulator_up=true, bootstrap_signal=false. The other three mismatches are marked owned_by 20261029T221313Z-mediaconnect-flow-operations-accept-requests-aws-refuses-with.md and are not re-filed. This cause is a `verdict: mismatch` cluster with empty `stub_path` and empty `owned_by`.

Cause 1 — fixture mediaconnect-catalog-list-offerings, step task#1, operation ListOfferings:
  expected_status 200, actual_status 200 (status matched; the payload diverged)
  divergence_key: service=mediaconnect|ListOfferings|status=200->200|keys=$.offerings[0],$.offerings[10],$.offerings[11],$.offerings[1],$.offerings[2],$.offerings[3],$.offerings[4],$.offerings[5],$.offerings[6],$.offerings[7],$.offerings[8],$.offerings[9]  (all 12 catalog entries diverge)
  detail (first three entries):
    $.offerings[0]: expected {duration 12, durationUnits MONTHS, priceUnits HOURLY, currencyCode USD, resourceSpecification {reservedBitrate 50, resourceType Mbps_Outbound_Bandwidth}, pricePerUnit '1.161', offeringDescription 'Reserved outbound bandwidth of 50Mbps at $1.161/hour', offeringArn 'arn:aws:mediaconnect:us-west-2:offering:80246697'}
                   got      {... reservedBitrate 50, pricePerUnit '1.200', offeringDescription 'Reserved outbound bandwidth of 50Mbps at $1.200/hour', offeringArn 'arn:aws:mediaconnect:us-west-2:offering:12000050'}
    $.offerings[1]: expected {reservedBitrate 1500, pricePerUnit '11.444', offeringDescription 'Reserved outbound bandwidth of 1500Mbps at $11.444/hour', offeringArn '...:offering:80248314'}
                   got      {reservedBitrate 100,  pricePerUnit '2.300',  offeringDescription 'Reserved outbound bandwidth of 100Mbps at $2.300/hour',  offeringArn '...:offering:12000100'}
    $.offerings[2]: expected {reservedBitrate 150,  pricePerUnit '2.834',  offeringDescription 'Reserved outbound bandwidth of 150Mbps at $2.834/hour', offeringArn '...:offering:80248601'}

The diverging facts are the tier set (expected tiers include 1500 and 150 Mbps; the emulator serves a 50/100/... ladder) and the rate per tier (expected 1.161 / 11.444 / 2.834; emulator 1.200 / 2.300 / ... on a linear schedule), with offeringDescription carrying the same wrong numbers. Shape, enum values (MONTHS, HOURLY, Mbps_Outbound_Bandwidth), currencyCode USD, duration 12 and the description template all matched.

Model facts used (services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json): ListOfferings GET /v1/offerings code 200, output member Offerings jsonName offerings, paginated; Offering requires CurrencyCode/Duration/DurationUnits/OfferingArn/OfferingDescription/PricePerUnit/PriceUnits/ResourceSpecification; DurationUnits values [MONTHS]; PriceUnits values [HOURLY]; ResourceSpecification {ReservedBitrate, ResourceType @required}. The catalog's contents are not modelled, hence "recorded behavior only" for the tiers and rates.

## Steps to Reproduce
Boot the emulator and replay the wire corpus (replay_wire_corpus), or drive a botocore `mediaconnect` client at the emulator endpoint:

Cause 1: mc.list_offerings() -> inspect Offerings. Observe HTTP 200 with a 12-entry catalog whose reservedBitrate values form an arithmetic ladder (50, 100, ...) and whose pricePerUnit is linear in the bitrate (50 Mbps -> '1.200', 100 Mbps -> '2.300'), with offeringArn ids derived from the bitrate (arn:aws:mediaconnect:<region>:offering:12000050, ...:12000100). AWS returns its published catalog for the region instead: tiers that include 1500 Mbps and 150 Mbps, with non-linear rates ('1.161' at 50 Mbps, '11.444' at 1500 Mbps, '2.834' at 150 Mbps) and catalog offering ids unrelated to the bitrate. Then take any returned Offerings[i].OfferingArn and call mc.describe_offering(OfferingArn=...) to confirm the same synthetic catalog backs the single-offering read. Fixture mediaconnect-catalog-list-offerings step task#1.
