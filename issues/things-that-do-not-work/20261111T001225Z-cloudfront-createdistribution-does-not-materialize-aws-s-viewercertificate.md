Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-distribution-tenant-create-and-invalidate
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-distribution-tenant-create-and-invalidate cloudfront-distribution-tenant-lifecycle cloudfront-distribution-tenant-webacl-association
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateDistribution|2xx->2xx|$.Distribution.DistributionConfig.ViewerCertificate.MinimumProtocolVersion
Issue-Related: 20261029T212223Z-cloudfront-distribution-tenant-domain-status-and-distribution-viewercertificate.md
Issue-Related: 20261109T120741Z-cloudfront-distribution-and-connection-group-responses-publish-aws-production.md
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# CloudFront CreateDistribution does not materialize AWS's ViewerCertificate defaults and emits an unrequested TenantConfig

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Two net-new wire divergence clusters (2 causes) share one fault: the DistributionConfig that CreateDistribution echoes back in its 201 response is serialized from the request as received rather than from the distribution's settled server-side state, so the ViewerCertificate members AWS derives are either wrong or absent, and a TenantConfig element the caller never asked for is invented.

Note on related open issues: 20261029T212223Z-cloudfront-distribution-tenant-domain-status-and-distribution-viewercertificate.md carries replay keys for CreateDistribution ViewerCertificate.SSLSupportMethod and TenantConfig, and 20261105T231732Z-cloudfront-readback-omits-defaulted-members-and-reports-stale.md carries the same two members on the GetDistribution / GetDistributionConfig read paths. Neither covers the steps below: the clusters here additionally diverge on ViewerCertificate.MinimumProtocolVersion on the CREATE path, so they are unowned by any open issue and the replay keys below are filed here. The location-header divergence that also appears in these two steps is owned by 20261109T120741Z-cloudfront-create-responses-announce-a-location-uri-on.md / 20261109T120741Z-cloudfront-distribution-and-connection-group-responses-publish-aws-production.md and is NOT re-filed here.

Cause 1 of 2: CreateDistribution returns the wrong ViewerCertificate.SSLSupportMethod and omits ViewerCertificate.MinimumProtocolVersion entirely.
AWS: Smithy: ViewerCertificate.SSLSupportMethod targets enum SSLSupportMethod with values [sni-only, vip, static-ip]; ViewerCertificate.MinimumProtocolVersion targets enum MinimumProtocolVersion with values [SSLv3, TLSv1, TLSv1_2016, TLSv1.1_2016, TLSv1.2_2018, TLSv1.2_2019, TLSv1.2_2021, TLSv1.3_2025, TLSv1.2_2025]. The MinimumProtocolVersion documentation states the rule directly: "If the distribution uses the CloudFront domain name such as d111111abcdef8.cloudfront.net (you set CloudFrontDefaultCertificate to true), CloudFront automatically sets the security policy to TLSv1 regardless of the value that you set here" — so for a distribution on the default CloudFront certificate the member is always present and always carries the server-derived security policy. For SSLSupportMethod on this configuration, recorded behavior only: AWS returns sni-only (the SNI-only method), per the Smithy doc the recommended posture; `vip` is documented as "The distribution accepts HTTPS connections from all viewers including those that don't support SNI. This is not recommended, and results in additional monthly charges from CloudFront." Both members are server-derived, not per-run values.
Emulator: returns $.Distribution.DistributionConfig.ViewerCertificate.SSLSupportMethod = 'vip', and omits $.Distribution.DistributionConfig.ViewerCertificate.MinimumProtocolVersion from the body altogether. HTTP status matches (201).
Why: a client that creates a distribution and reads the echoed config back is told it provisioned the dedicated-IP SSL method — a materially different and separately billable configuration from the SNI-only one AWS actually applied — and gets no security policy at all, so it cannot tell which minimum TLS version viewers will negotiate. An SDK model that treats MinimumProtocolVersion as populated on the create response sees a gap, and a read-modify-write of the returned config into UpdateDistribution round-trips `vip` and loses the policy.
Fix: CreateDistribution's ViewerCertificate defaulting — the handler/validation must settle SSLSupportMethod and MinimumProtocolVersion into distribution state before the response is serialized, and the distribution serializer must emit MinimumProtocolVersion. This is the same derivation the Get* read paths need, so it belongs in the shared DistributionConfig state/serialization rather than per-operation.
Traces: cloudfront-distribution-tenant-create-and-invalidate#setup#1.

Cause 2 of 2: the same ViewerCertificate divergence, plus CreateDistribution invents a TenantConfig element for a distribution created without tenant parameter definitions.
AWS: Smithy: DistributionConfig.TenantConfig targets structure TenantConfig whose only member is ParameterDefinitions (a list), documented as "The parameters that you specify for a distribution tenant" and noted as supported only for multi-tenant distributions. Recorded behavior: when the request carries no tenant parameter definitions, the response has NO TenantConfig element at all — AWS does not synthesize an empty container.
Emulator: emits $.Distribution.DistributionConfig.TenantConfig as {'ParameterDefinitions': ''} (an empty ParameterDefinitions container) where the recording has the member absent, and again returns SSLSupportMethod 'vip' with MinimumProtocolVersion missing.
Why: a REST-XML client that round-trips the returned config into UpdateDistribution now submits a TenantConfig it never configured, which is the member AWS rejects on a standard (non-multi-tenant) distribution — so the echoed config is not safely re-submittable. A client that tests for the presence of TenantConfig to decide whether a distribution is multi-tenant also gets the wrong answer.
Fix: the DistributionConfig serializer — suppress TenantConfig when the distribution has no tenant parameter definitions (do not emit empty optional structure containers) — together with the ViewerCertificate defaulting from Cause 1.
Traces: cloudfront-distribution-tenant-lifecycle#setup#1 (cluster also covers cloudfront-distribution-tenant-webacl-association#setup#1).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false, 1303 steps, score 78.0%, 84 divergence clusters, all cloudfront). These are the only cloudfront CreateDistribution clusters with an empty owned_by and an empty stub_path.

Cause 1 — cloudfront-distribution-tenant-create-and-invalidate#setup#1, CreateDistribution, expected status 201 / actual 201:
  $.Distribution.DistributionConfig.ViewerCertificate.SSLSupportMethod: value_mismatch (expected 'sni-only', got 'vip')
  $.Distribution.DistributionConfig.ViewerCertificate.MinimumProtocolVersion: missing (expected 'TLSv1', got None)
  (divergence_key service=cloudfront|CreateDistribution|status=201->201|keys=$.Distribution.DistributionConfig.ViewerCertificate.MinimumProtocolVersion,$.Distribution.DistributionConfig.ViewerCertificate.SSLSupportMethod,$headers.location; affected_count 1)

Cause 2 — cloudfront-distribution-tenant-lifecycle#setup#1, CreateDistribution, expected status 201 / actual 201:
  $.Distribution.DistributionConfig.ViewerCertificate.SSLSupportMethod: value_mismatch (expected 'sni-only', got 'vip')
  $.Distribution.DistributionConfig.ViewerCertificate.MinimumProtocolVersion: missing (expected 'TLSv1', got None)
  $.Distribution.DistributionConfig.TenantConfig: type_mismatch (expected '' / member absent, got {'ParameterDefinitions': ''})
  (divergence_key service=cloudfront|CreateDistribution|status=201->201|keys=$.Distribution.DistributionConfig.TenantConfig,...ViewerCertificate.MinimumProtocolVersion,...ViewerCertificate.SSLSupportMethod,$headers.location; affected_count 2, other member cloudfront-distribution-tenant-webacl-association#setup#1)

The $headers.location mismatch reported in both clusters is already owned by the open CloudFront location-URI issues and is deliberately excluded from this issue's scope. The values asserted here (an enum member and the presence of an enum member) are server-derived properties of the distribution, not per-run identifiers, hosts or timestamps.

## Steps to Reproduce
1. git_clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture cloudfront-distribution-tenant-create-and-invalidate and inspect step setup#1 (CreateDistribution). Diff the 201 body against the recording: ViewerCertificate.SSLSupportMethod reads 'vip' instead of 'sni-only' and ViewerCertificate.MinimumProtocolVersion is absent instead of 'TLSv1'.
4. Cause 2: replay fixture cloudfront-distribution-tenant-lifecycle and inspect step setup#1 (CreateDistribution). The same two ViewerCertificate divergences appear, plus a TenantConfig element containing an empty ParameterDefinitions where the recording has no TenantConfig member.
5. Equivalently by hand: POST /2020-05-31/distribution with a DistributionConfig that uses the default CloudFront certificate (CloudFrontDefaultCertificate true), no explicit SSLSupportMethod/MinimumProtocolVersion and no TenantConfig; the 201 body echoes 'vip', omits MinimumProtocolVersion, and (in the tenant-capable shape) carries an empty TenantConfig.
6. Both causes must match the recordings before closing.
