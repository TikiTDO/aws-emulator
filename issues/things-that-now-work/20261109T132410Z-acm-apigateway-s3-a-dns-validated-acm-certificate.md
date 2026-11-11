Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates
Issue-Key: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates|_poll_until#21|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: cda81fc610349487f102bcc6f8ad570fe265d90b

# acm__apigateway__s3: a DNS-validated ACM certificate ends FAILED instead of ISSUED after its validation CNAME is published, blocking the mutual-TLS custom-domain scenario

> _Note: this issue was identified against the code base at commit `cda81fc610349487f102bcc6f8ad570fe265d90b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestCertificate

## Description
One net-new root cause (Cause 1 of 1).

Related open filings that do NOT cover this replay: 20261030T015855Z-acm-dns-validated-certificate-flips-status-and-per.md and 20261029T093731Z-acm-dns-validated-certificate-issuance-is-unreachable-the.md describe DNS-validated issuance defects, and 20261101T214920Z-appsync-custom-domain-checks-are-all-blocked-upstream.md records the same FAILED observable for AppSync; none of them is filed under this service heading or carries this check id, so this failure survives unowned.

Cause 1 of 1: ACM moves a DNS-validated certificate to Status FAILED instead of ISSUED after the requester publishes the exact CNAME that DescribeCertificate handed out, in a public hosted zone the account owns.
AWS: Smithy (acm) RequestCertificate(DomainName required, ValidationMethod=DNS) -> CertificateArn; DescribeCertificate -> CertificateDetail.Status of CertificateStatus {PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED, FAILED}, with per-domain DomainValidation.ValidationStatus (PENDING_VALIDATION | SUCCESS | FAILED) and DomainValidation.ResourceRecord, documented as "the CNAME record that you add to your DNS database for domain validation". AWS behavior: while the CNAME is absent the certificate stays PENDING_VALIDATION (and only after 72 h becomes VALIDATION_TIMED_OUT); once the published CNAME resolves to the handed-out value, ValidationStatus becomes SUCCESS and Status becomes ISSUED. FAILED is reserved for a definite refusal and is then explained by CertificateDetail.FailureReason (FailureReason enum: NO_AVAILABLE_CONTACTS, ADDITIONAL_VERIFICATION_REQUIRED, DOMAIN_NOT_ALLOWED, INVALID_PUBLIC_DOMAIN, DOMAIN_VALIDATION_DENIED, CAA_ERROR, ..., OTHER) — a correctly published CNAME for a domain in the account's own public hosted zone is not one of those cases.
Emulator: after RequestCertificate(ValidationMethod='DNS') and a route53.change_resource_record_sets CREATE of exactly the DomainValidation.ResourceRecord Name/Type/Value in the tagged public hosted zone, DescribeCertificate's Status reaches FAILED and stays there (the check polls for ISSUED for 300 s and reports "Last result: 'FAILED'"). The domain here is a sub-name of the account's own baseline zone, so validation must succeed.
Why: every scenario that needs an AWS-issued, DNS-validated certificate is unreachable, and the terminal FAILED is worse than a stall: a client (or IaC tooling) that correctly waits on Status sees a definite, non-retryable failure for a certificate AWS would have issued, with no FailureReason that explains it, so it tears down or aborts instead of proceeding. In this check it prevents the API Gateway REGIONAL custom domain plus S3 truststore mutual-TLS flow from ever starting.
Fix: the ACM RequestCertificate/DescribeCertificate validation state machine in the acm service crate — the DNS-validation resolver that checks the published CNAME against Route 53 and the transition it drives must take a matching record to ValidationStatus SUCCESS and Status ISSUED, and must never settle on FAILED without a modeled FailureReason.
Traces: acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates.

## Evidence
Replay of suite simparity-behavioral-aws-v1: acm__apigateway__s3 0/1 (0%), 1 failed.

[mismatch] acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates:
  File "check_...mutual_tls_enforces_trusted_client_certificates.py", line 140, in _issue_public_certificate -> _poll_until(...)
  AssertionError: the certificate for 'm1670e8586e7d.sim2real-baseline-4771d6853596.example.net' reaching ISSUED via DNS validation not met within 300s. Last result: 'FAILED'

_issue_public_certificate: acm.request_certificate(DomainName=<name in the account's tagged public hosted zone>, ValidationMethod='DNS'), polls describe_certificate for DomainValidationOptions[].ResourceRecord, CREATEs exactly that Name/Type/Value in the zone with route53.change_resource_record_sets, then polls describe_certificate for Status == 'ISSUED'. Expected: PENDING_VALIDATION -> ISSUED after the record is published. Actual: Status FAILED for the whole 300 s window. The domain name shown is a per-run value; the requirement is that a published, matching validation CNAME yields ISSUED.

Smithy consulted: acm RequestCertificate (DomainName/ValidationMethod), DescribeCertificate, CertificateStatus enum, DomainValidation.{ValidationStatus,ResourceRecord}, FailureReason enum.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; acm__apigateway__s3.regional-custom-domain-mutual-tls-enforces-trusted-client-certificates reproduces.
2. route53.list_hosted_zones -> pick the public zone carrying the DNS-baseline tag; take its apex domain and zone id.
3. acm.request_certificate(DomainName='<label>.<apex>', ValidationMethod='DNS') and poll acm.describe_certificate until DomainValidationOptions[0].ResourceRecord appears.
4. route53.change_resource_record_sets(HostedZoneId=<zone>, Changes=[CREATE of that ResourceRecord Name/Type/Value, TTL 60]).
5. Poll acm.describe_certificate: AWS reports ValidationStatus SUCCESS and Status ISSUED; the emulator reports Status FAILED and never leaves it (no modeled FailureReason justifies it).
6. Re-replay the check id in trace_ids to confirm the fix.
