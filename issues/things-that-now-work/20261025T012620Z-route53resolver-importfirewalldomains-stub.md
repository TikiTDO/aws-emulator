Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.e2e-firewall-domain-block-full-lifecycle route53resolver.e2e-firewall-domain-import-block-consequence route53resolver.import-firewall-domains-conflict-while-in-progress route53resolver.import-firewall-domains-conflict-while-in-progress-rejected route53resolver.import-firewall-domains-invalid-s3-uri-rejected
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|ImportFirewallDomains|NotImplemented
Issue-Commit: 5f3c74c83654b73bfd2d2e3d0b1746f9df515a9b

# Implement route53resolver ImportFirewallDomains: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5f3c74c83654b73bfd2d2e3d0b1746f9df515a9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportFirewallDomains

## Description
`ImportFirewallDomains` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/import_firewall_domains.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `ImportFirewallDomains` call, so each one unblocks only when `ImportFirewallDomains` works: `route53resolver.e2e-firewall-domain-block-full-lifecycle`, `route53resolver.e2e-firewall-domain-import-block-consequence`, `route53resolver.import-firewall-domains-conflict-while-in-progress`, `route53resolver.import-firewall-domains-conflict-while-in-progress-rejected`, `route53resolver.import-firewall-domains-invalid-s3-uri-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/import_firewall_domains.rs`. Replaying the corpus, the emulator refused the `ImportFirewallDomains` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportFirewallDomains operation: operation ImportFirewallDomains is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53resolver emulator
Call ImportFirewallDomains and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
