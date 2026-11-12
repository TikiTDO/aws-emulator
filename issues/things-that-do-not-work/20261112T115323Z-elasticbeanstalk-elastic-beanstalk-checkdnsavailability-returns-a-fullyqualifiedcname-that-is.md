Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-dns-availability-and-environment-discovery
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-dns-availability-and-environment-discovery
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|CheckDNSAvailability|2xx->2xx|$.CheckDNSAvailabilityResponse.CheckDNSAvailabilityResult.FullyQualifiedCNAME
Issue-Commit: 4839448a699fa31c23507e6b5ab2cdd919186720

# Elastic Beanstalk CheckDNSAvailability returns a FullyQualifiedCNAME that is not a DNS name

> _Note: this issue was identified against the code base at commit `4839448a699fa31c23507e6b5ab2cdd919186720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckDNSAvailability

## Description
One net-new cause (Cause 1 of 1) from the wire replay of simparity-wire-aws-v1; the other 18 divergence clusters of this replay all carry a non-empty owned_by and were skipped.

Cause 1 of 1: CheckDNSAvailability publishes FullyQualifiedCNAME as a host:port plus a URL path instead of a fully qualified domain name (fixture elasticbeanstalk-dns-availability-and-environment-discovery, step task#2).

AWS: Smithy: CheckDNSAvailability(input CheckDNSAvailabilityMessage{CNAMEPrefix required, target DNSCnamePrefix @length 4..63}) -> CheckDNSAvailabilityResultMessage{Available: CnameAvailability boolean, FullyQualifiedCNAME: DNSCname @length 1..255}; no modeled operation errors. The model documents FullyQualifiedCNAME as 'The fully qualified CNAME to reserve when CreateEnvironment is called with the provided prefix'. The recorded 200 response shows the required form: a dot-separated DNS name whose FIRST label is exactly the requested CNAMEPrefix, whose SECOND label is the request's region, followed by the service's own domain (on AWS '<prefix>.<region>.elasticbeanstalk.com'). The fixed part of the requirement is that form — prefix label, then region label, then a domain containing at least one dot, and no ':' or '/' characters, which a DNS name cannot carry. The variable parts are the CNAMEPrefix (taken from the request), the region (taken from the request) and the trailing domain (the serving target's own); the specific prefix and the 'elasticbeanstalk.com' domain of one recording are per-run / environment fill-ins, not requirements.

Emulator: answers HTTP 200 with Available set, but FullyQualifiedCNAME is '<host>:<port>/_aws/elasticbeanstalk/<region>/<prefix>' — the prefix is the LAST path segment, the region is a path segment, and the value carries a colon and slashes. Replay flags $.CheckDNSAvailabilityResponse.CheckDNSAvailabilityResult.FullyQualifiedCNAME as value_mismatch: expected 'sim2real-probe-elasticbeanstalk-cname-cb581921.us-west-2.elasticbeanstalk.com', got '127.0.0.1:33959/_aws/elasticbeanstalk/us-west-2/sim2real-probe-elasticbeanstalk-cname-cb581921' (the host, port and generated prefix are per-run values; the diverging thing is the FORM of the value, not those values).

Why: FullyQualifiedCNAME exists so a client can predict the hostname its future environment will answer on, and then use it — write it into a DNS record or an alias target, set it as an HTTP Host header, or parse the first dot-label back out to recover the prefix. A string containing a port and a path is not a name any resolver, DNS record or Host header accepts, so every client that composes an address from this member builds an invalid one, and clients that split it on '.' to recover the prefix read the first component of a host literal instead of the prefix they asked about.

Fix: the CheckDNSAvailability handler's response projection and the shared address helper it calls — services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/check_dns_availability.rs builds the member from crate::state::environment_cname(&plane_authority, &region, &input.cname_prefix), which composes the environment plane's socket authority and the '_aws/elasticbeanstalk/<region>/<name>' path. That helper (shared with the environment CNAME that CreateEnvironment stores, so the two cannot disagree) must mint a dot-separated name whose first label is the request's CNAMEPrefix and whose second label is the region, on a domain the emulator itself answers on, with the port and any path kept out of the value; the handler's Available logic is not in question.

Traces: elasticbeanstalk-dns-availability-and-environment-discovery#task#2.

Related open issues that do NOT carry this failure (the replay reported owned_by [] for this cluster, so none of them owns its replay key): 20261112T072923Z-elasticbeanstalk-elastic-beanstalk-publishes-cnames-that-are-not-dns.md is a BEHAVIORAL issue (corpus simparity-behavioral-aws-v1) whose Cause 1 describes the same root cause but is grounded only in behavioral check ids, so a fixer re-replaying it never exercises this wire fixture step; 20261109T144630Z-elasticbeanstalk-elastic-beanstalk-publishes-environment-cname-endpointurl-and-log.md and 20261109T203350Z-elasticbeanstalk-environment-reads-drop-modeled-members-and-mint.md concern the CNAME/EndpointURL published on EnvironmentDescription reads, not the form of CheckDNSAvailability's FullyQualifiedCNAME; 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-control-plane-guards-are-not-enforced.md concerns CheckDNSAvailability's availability guard, not this member's shape.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, 19 fixtures replayed, 157 steps (133 passed, 24 mismatched, 0 errored, 0 blocked), score_pct 84.7, divergence_count 19. All 19 clusters are service=elasticbeanstalk; 18 carry a non-empty owned_by and are skipped. This cluster carries owned_by [] and stub_path "".

Cluster: fixture elasticbeanstalk-dns-availability-and-environment-discovery, step task#2, service elasticbeanstalk, operation CheckDNSAvailability, verdict mismatch, expected_status 200, actual_status 200, affected_count 1.
Detail: $.CheckDNSAvailabilityResponse.CheckDNSAvailabilityResult.FullyQualifiedCNAME: value_mismatch (expected 'sim2real-probe-elasticbeanstalk-cname-cb581921.us-west-2.elasticbeanstalk.com', got '127.0.0.1:33959/_aws/elasticbeanstalk/us-west-2/sim2real-probe-elasticbeanstalk-cname-cb581921').

Expected vs actual, stated as the requirement: expected a DNSCname of the form <CNAMEPrefix>.<region>.<service domain> — first label exactly the requested prefix, second label the request's region, then a domain with at least one dot, and no ':' or '/'; got a host:port followed by the path '/_aws/elasticbeanstalk/<region>/<prefix>'. HTTP status is 200 on both sides and no error code or error message is involved. The host 127.0.0.1, the port 33959 and the generated prefix sim2real-probe-elasticbeanstalk-cname-cb581921 are per-run values and are not themselves the requirement.

Model evidence: smithy_query(operation=CheckDNSAvailability) -> input {CNAMEPrefix required, DNSCnamePrefix @length 4..63}, output {Available CnameAvailability, FullyQualifiedCNAME DNSCname @length 1..255}, errors []. smithy_query(doc, shape=CheckDNSAvailabilityResultMessage, member=FullyQualifiedCNAME) -> 'The fully qualified CNAME to reserve when CreateEnvironment is called with the provided prefix.'

Code evidence: services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/check_dns_availability.rs lines 139-168 — the handler obtains crate::state::environment_plane_authority(...) and sets fully_qualified_cname = crate::state::environment_cname(&plane_authority, &region, &input.cname_prefix), i.e. the member is built from the serving socket authority plus a path rather than as a domain name.

## Steps to Reproduce
1. git clone the project repo and build/boot the emulator from the checkout.
2. Replay the wire fixture corpus simparity-wire-aws-v1 (replay_wire_corpus); the cluster reproduces with no other setup.
3. Replay fixture elasticbeanstalk-dns-availability-and-environment-discovery, step task#2: CheckDNSAvailability with a fresh legal CNAMEPrefix (4..63 characters).
4. Read CheckDNSAvailabilityResult.FullyQualifiedCNAME from the 200 body: it is '<host>:<port>/_aws/elasticbeanstalk/<region>/<prefix>' instead of '<prefix>.<region>.<service domain>'. Check the form, not the recorded host/port: the first dot-separated label is not the requested prefix, the second is not the region, and the value contains ':' and '/'.
5. Equivalently with an SDK: eb.check_dns_availability(CNAMEPrefix='<fresh legal prefix>') and inspect FullyQualifiedCNAME; attempting to resolve it, or taking FullyQualifiedCNAME.split('.')[0] to recover the prefix, fails.
