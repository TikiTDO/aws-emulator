Issue-Source: trace-sweep-divergence
Issue-Trace: route53domains-check-domain-availability
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53domains-check-domain-availability route53domains-domain-availability-and-transferability-checks
Issue-Shape-Id: com.amazonaws.route53domains#Route53Domains_v20140515
Issue-Key: route53domains|CheckDomainAvailability|2xx->2xx|$.Price
Issue-Key: route53domains|CheckDomainAvailability|2xx->2xx|$.Availability
Issue-Commit: 63738de0101ebf6c323b1ab59b247c4705f0deaf

# route53domains CheckDomainAvailability response omits Price and reports registered names AVAILABLE

> _Note: this issue was identified against the code base at commit `63738de0101ebf6c323b1ab59b247c4705f0deaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckDomainAvailability

## Description
TWO net-new root causes in the 200-response body CheckDomainAvailability serves. Both are in the same handler (services/service-com-amazonaws-route53domains-route53domains-v20140515/src/ops/check_domain_availability.rs). Both must be closed before this issue is resolved.

Cause 1 of 2: the Price member is never populated for an AVAILABLE name.
AWS: CheckDomainAvailability's output carries Availability and Price — Smithy: CheckDomainAvailabilityResponse.Price targets PriceWithCurrency, whose members are Price (double, required, @range(min: 0)), Currency (required, @length 3..3) and PriceType (enum PriceType, STANDARD | REGISTRY_PREMIUM). The member documentation states "The price to register the domain name, in the currency that Currency specifies... This element is returned only when Availability is AVAILABLE" — i.e. Price is REQUIRED IN THE RESPONSE whenever Availability is AVAILABLE, and absent otherwise. The recording shows the shape AWS fills in for a standard name under a supported TLD: Currency "USD", PriceType "STANDARD", and the TLD's standard registration price (16.0 for .com in the recording). The currency and price type are fixed for a standard registration; the numeric price is a per-TLD constant (not a per-run value), so it must come from a stable per-TLD price table rather than being invented per request.
Emulator: returns 200 with Availability "AVAILABLE" and NO Price at all — the handler hardcodes `price: None` with the comment that premium pricing is not modeled, which conflates the premium rate with the standard rate the member always carries.
Why: a client that prices a registration before calling RegisterDomain — the documented use of this operation — reads a null Price and either crashes on the missing required sub-members or shows no price; it has no other way to obtain the standard rate for an available name when ListPrices is unavailable.
Fix: the CheckDomainAvailability handler plus a per-TLD standard price table in crate::state — populate Price with { Price: <standard rate for the TLD>, Currency: "USD", PriceType: "STANDARD" } exactly when the computed Availability is AVAILABLE, and omit it otherwise.
Traces: route53domains-check-domain-availability#task#1 (cluster also covers route53domains-domain-availability-and-transferability-checks#task#1).

Cause 2 of 2: availability is computed only from this account's own domain store, so a name already registered elsewhere is reported AVAILABLE.
AWS: Availability is the REGISTRY's answer for the name, not an account-scoped answer — Smithy: DomainAvailability enum [AVAILABLE, AVAILABLE_RESERVED, AVAILABLE_PREORDER, UNAVAILABLE, UNAVAILABLE_PREMIUM, UNAVAILABLE_RESTRICTED, RESERVED, DONT_KNOW, INVALID_NAME_FOR_TLD, PENDING]; the Availability doc states "You can register only domains designated as AVAILABLE" and UNAVAILABLE means the domain name is not available. A well-formed name under a supported TLD that is already registered by anyone — including a third party, not just the calling account — is UNAVAILABLE (recorded behavior: the fixture's already-registered name returns UNAVAILABLE).
Emulator: `let availability = if registered { "UNAVAILABLE" } else { "AVAILABLE" }`, where `registered` is `state.lock().domains.contains_key(&name)` — only names THIS account holds in the emulator's store are UNAVAILABLE, so every other syntactically valid name, including names the recording shows as taken, answers AVAILABLE.
Why: a client that gates RegisterDomain on Availability == AVAILABLE is told a taken name is registrable and proceeds, which is the opposite of the check's purpose; and the UNAVAILABLE branch of any caller's logic is never exercised against the emulator.
Fix: the CheckDomainAvailability handler's availability computation, backed by state in crate::state — the store needs a notion of names registered outside this account (a seeded set of already-taken names) that the handler consults in addition to this account's domains, so the answer is registry-scoped rather than account-scoped. The same source must back CheckDomainTransferability once it is implemented.
Traces: route53domains-domain-availability-and-transferability-checks#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, 60 steps, 4 passed). Both causes are 200->200 body mismatches: the status is correct, the body is not.

Cause 1 — route53domains-check-domain-availability#task#1, CheckDomainAvailability, expected 200 / actual 200:
  $.Price: missing — expected {'Currency': 'USD', 'Price': 16.0, 'PriceType': 'STANDARD'}, got None.
  ($.Availability matched, AVAILABLE. 16.0/USD/STANDARD is the .com standard registration rate in the recording; the requirement is that Price is present with its required Currency and Price members whenever Availability is AVAILABLE.)
  Cluster also covers route53domains-domain-availability-and-transferability-checks#task#1 (affected_count 2).
  Source: ops/check_domain_availability.rs:208-213 — `price: None` hardcoded.

Cause 2 — route53domains-domain-availability-and-transferability-checks#task#2, CheckDomainAvailability, expected 200 / actual 200:
  $.Availability: value_mismatch — expected 'UNAVAILABLE', got 'AVAILABLE'.
  Source: ops/check_domain_availability.rs:196-207 — availability derived solely from `state.domains.contains_key(name)`.

Smithy (services/service-com-amazonaws-route53domains-route53domains-v20140515/smithy-model/com-amazonaws-route53domains-route53domains-v20140515.json): CheckDomainAvailabilityResponse { Availability: DomainAvailability (enum), Price: PriceWithCurrency }; PriceWithCurrency { Price: double required @range(min 0), Currency: required @length(3,3), PriceType: enum } with doc "This element is returned only when Availability is AVAILABLE"; DomainAvailability values include AVAILABLE and UNAVAILABLE.

## Steps to Reproduce
Region us-east-1 against the emulator endpoint.

Cause 1:
1. aws route53domains check-domain-availability --domain-name <a-free-name>.com
2. Observe 200 with {"Availability": "AVAILABLE"} and no Price member.
3. AWS answers 200 with Availability AVAILABLE and Price { Price: <.com standard rate>, Currency: "USD", PriceType: "STANDARD" }.
4. Register the name (aws route53domains register-domain ...) and re-check: Availability becomes UNAVAILABLE and Price must then be absent.

Cause 2:
1. aws route53domains check-domain-availability --domain-name google.com (a name registered outside this account)
2. Observe 200 with Availability "AVAILABLE".
3. AWS answers 200 with Availability "UNAVAILABLE".
4. A free name under the same supported TLD must still answer AVAILABLE, so the fix must not blanket-report UNAVAILABLE.
