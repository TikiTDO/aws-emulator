Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-service-and-listener-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-service-and-listener-lifecycle vpclattice-service-network-service-association
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|CreateService|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|GetService|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|CreateServiceNetworkServiceAssociation|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|GetServiceNetworkServiceAssociation|2xx->2xx|$.dnsEntry.domainName
Issue-Key: vpc-lattice|ListServiceNetworkServiceAssociations|2xx->2xx|$.items[*].dnsEntry.domainName
Issue-Commit: 0aa1df021fcab924dd462ae3dfcde463e64681ac

# vpc-lattice dnsEntry.domainName announces a real AWS production host the emulator does not serve

> _Note: this issue was identified against the code base at commit `0aa1df021fcab924dd462ae3dfcde463e64681ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Every vpc-lattice response that carries a `DnsEntry` fills `domainName` with a name in AWS's REAL production VPC Lattice DNS suffix (`<name>-<id suffix>.<account segment>.vpc-lattice-svcs.<region>.on.aws`) instead of a host the emulator itself answers on. The replay harness flags each one `host_not_own`: the emulator announced an address other than the one it is reached on. There are FIVE causes below — one per operation that serializes a `DnsEntry` — and all five must be closed before this issue is done.

Shared root cause: the generated-domain helper in this service's state module mints a production AWS hostname and persists it on the record (`LatticeService.dns_domain_name`, `ServiceNetworkServiceAssociation.dns_domain_name`), so every read-back path re-serves it.

Cause 1 of 5: CreateService (fixture `vpclattice-service-and-listener-lifecycle`, step `task#1`).
  AWS: Smithy `com.amazonaws.vpclattice#DnsEntry` has members `domainName` (String, "The domain name of the service.") and `hostedZoneId`; `CreateService` output member `dnsEntry` is documented "The public DNS name of the service." The model places NO pattern or length constraint on `domainName`, so the exact text is recorded behavior only: on real AWS the value is a resolvable host served by the VPC Lattice data plane in the caller's own account. The fixture asserts the member is present and shaped, not a literal value, so the requirement is that the announced host be one the responder actually serves.
  Emulator: answers 201 with `$.dnsEntry.domainName` set to a name under the real AWS suffix `.vpc-lattice-svcs.<region>.on.aws` — a host the emulator neither serves nor can be reached on. Status, code and all other members match.
  Why: a client that takes `dnsEntry.domainName` and connects to it leaves the emulator entirely — the name either fails to resolve or resolves to genuine AWS infrastructure, so every in-VPC service-to-service call modelled on a recording silently escapes the emulated environment instead of reaching the emulated service.
  Fix: the `DnsEntry` value synthesized in the vpc-lattice state module's domain-name helper (and the two record fields that persist it) must derive from the endpoint the emulator is actually reached on, not from AWS's production suffix; the serializers that read it then need no change.

Cause 2 of 5: GetService (fixture `vpclattice-service-and-listener-lifecycle`, step `task#2`).
  AWS: same `DnsEntry` shape; `GetService` returns the service's own DNS entry, so a read-back reports the same resolvable host the create announced.
  Emulator: answers 200 and re-serves the stored production-suffix name at `$.dnsEntry.domainName` (`host_not_own`).
  Why: the defect is durable, not just a create-time artifact — a client that re-reads the service to recover its endpoint gets the unreachable host again.
  Fix: the stored value, as in Cause 1; GetService's serializer reads the record.

Cause 3 of 5: CreateServiceNetworkServiceAssociation (fixture `vpclattice-service-network-service-association`, step `task#4`).
  AWS: `CreateServiceNetworkServiceAssociation` output carries `dnsEntry` (same `DnsEntry` shape) — the name under which the associated service is reachable through that service network. Model constrains nothing about the value; recorded behavior only.
  Emulator: answers 200 with `$.dnsEntry.domainName` under the real `.vpc-lattice-svcs.<region>.on.aws` suffix (`host_not_own`).
  Why: the association's DNS entry is exactly the name a caller in the VPC is told to use, so the association path hands out an address outside the emulator.
  Fix: the association record's persisted DNS domain name, from the same helper as Cause 1.

Cause 4 of 5: GetServiceNetworkServiceAssociation (fixture `vpclattice-service-network-service-association`, step `task#5`).
  AWS: read-back of the association reports its stored DNS entry.
  Emulator: answers 200 and re-serves the production-suffix name at `$.dnsEntry.domainName` (`host_not_own`).
  Why: as Cause 2 — the unreachable host survives into every read of the association.
  Fix: the stored association DNS value.

Cause 5 of 5: ListServiceNetworkServiceAssociations (fixture `vpclattice-service-network-service-association`, step `task#7`).
  AWS: each `items[]` summary carries the association's `dnsEntry`.
  Emulator: answers 200 with `$.items[0].dnsEntry.domainName` under the real AWS suffix (`host_not_own`).
  Why: the listing is how a client enumerates reachable endpoints, so a sweep over the list yields only addresses the emulator does not answer on.
  Fix: the stored association DNS value surfaced by the list serializer.

No existing open vpc-lattice issue covers the DNS entry: the nearest ones are `20261030T175927Z-vpc-lattice-mints-rule-and-target-group-ids` and `20261103T105308Z-vpc-lattice-rule-ids-omit-aws-s-leading`, which are about id/ARN character form, and `20261101T145430Z-vpc-lattice-listener-rule-update-path-wrong-batchupdaterule`, which is about rule update messages. None of them touches `dnsEntry.domainName` or the host the emulator announces.

## Evidence
Replay verdict `mismatch`, assertion class `host_not_own`, on 5 steps of 2 fixtures (statuses themselves all matched):

1. CreateService — vpclattice-service-and-listener-lifecycle#task#1, expected 201 / actual 201:
   `$.dnsEntry.domainName: host_not_own (expected '<shaped-not-asserted>', got 'sim2real-probe-lat-svc-b52d9f07-06e8f46a117e08e24.f7b1150.vpc-lattice-svcs.us-west-2.on.aws')`
2. GetService — vpclattice-service-and-listener-lifecycle#task#2, expected 200 / actual 200: same `$.dnsEntry.domainName` host_not_own.
3. CreateServiceNetworkServiceAssociation — vpclattice-service-network-service-association#task#4, 200/200:
   `$.dnsEntry.domainName: host_not_own (expected '<shaped-not-asserted>', got 'sim2real-probe-lat-assvc-f73b1d40-096d05b8785aef32c.f7b1150.vpc-lattice-svcs.us-west-2.on.aws')`
4. GetServiceNetworkServiceAssociation — vpclattice-service-network-service-association#task#5, 200/200: same host_not_own.
5. ListServiceNetworkServiceAssociations — vpclattice-service-network-service-association#task#7, 200/200:
   `$.items[0].dnsEntry.domainName: host_not_own`.

The generated-name, account-segment and id fragments in the quoted values are per-run and are NOT the requirement; the requirement is the DNS SUFFIX/authority: the emulator emits AWS's production `.vpc-lattice-svcs.<region>.on.aws` authority rather than a host it serves.

Source of the value in the checkout:
- services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/state.rs:1545-1553 — `service_domain_name()` formats `"{name}-{suffix}.{account_segment}.vpc-lattice-svcs.{region}.on.aws"`.
- services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/state.rs:412-413 and 456-458 — `LatticeService.dns_domain_name` / `ServiceNetworkServiceAssociation.dns_domain_name` persist it, so reads and listings re-serve it.
- services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/create_service.rs:318 — CreateService stores `crate::state::service_domain_name(&input.name, &id, &region)`; create_service.rs:163-176 (`output_of`) serializes it into `DnsEntry`.

Smithy consulted: shape `com.amazonaws.vpclattice#DnsEntry` (members `domainName`, `hostedZoneId`, both plain `String`, no constraints); `CreateService` output member `dnsEntry` doc "The public DNS name of the service."

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `vpclattice-service-and-listener-lifecycle`.
2. At step `task#1`, CreateService answers 201; read `$.dnsEntry.domainName` in the response. It ends in `.vpc-lattice-svcs.<region>.on.aws` — AWS's production authority, not the host:port the emulator is being reached on. Step `task#2` (GetService) returns the same value.
3. Replay fixture `vpclattice-service-network-service-association`. Step `task#4` (CreateServiceNetworkServiceAssociation) and step `task#5` (GetServiceNetworkServiceAssociation) return `$.dnsEntry.domainName`, and step `task#7` (ListServiceNetworkServiceAssociations) returns `$.items[0].dnsEntry.domainName`, all with the same production authority.
4. Try to connect to any returned domain name: it is not served by the emulator. Expected: a DNS entry naming a host the emulator answers on, so the announced endpoint is reachable within the emulated environment.
