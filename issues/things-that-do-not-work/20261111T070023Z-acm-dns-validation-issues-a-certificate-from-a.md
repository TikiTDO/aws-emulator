Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated|run#34|AssertionError|assert detail_now["Status"] == "PENDING_VALIDATION", (
Issue-Commit: d9b54befa158e14a4810d0b0face0e7b12d06851

# ACM DNS validation issues a certificate from a validation CNAME in an undelegated public hosted zone (overly permissive)

> _Note: this issue was identified against the code base at commit `d9b54befa158e14a4810d0b0face0e7b12d06851`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestCertificate

## Description
One net-new root cause (Cause 1 of 1), grounded in one failing behavioral check.

Cause 1 of 1: ACM drives a DNS-validated certificate from PENDING_VALIDATION to ISSUED purely because the challenge CNAME exists in a Route 53 hosted zone of the same account, without the zone being reachable from the public DNS hierarchy. Failing check: acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated.

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) RequestCertificate{DomainName required -> DomainNameString @length 1..253 @pattern ^(\*\.)?(...)$, ValidationMethod} -> CertificateArn; DescribeCertificate -> CertificateDetail.Status of CertificateStatus [PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED, FAILED]; DomainValidation.ValidationStatus of DomainStatus, documented as one of PENDING_VALIDATION | SUCCESS | FAILED; DomainValidation.ResourceRecord documented as "the CNAME record that you add to your DNS database for domain validation" (https://docs.aws.amazon.com/acm/latest/userguide/dns-validation.html). Recorded AWS behavior for this scenario: for a freshly created PUBLIC hosted zone whose apex is a random label under .com that no registrar delegates to that zone's delegation set, publishing exactly the handed-out CNAME in that zone does NOT prove control, because ACM resolves the challenge from the public root down and the zone's name servers are never consulted. The certificate therefore STAYS PENDING_VALIDATION (per-domain ValidationStatus PENDING_VALIDATION, the ResourceRecord unchanged), is absent from ListCertificates(CertificateStatuses=['ISSUED']), and ListCertificateDomainValidations keeps reporting that name PENDING_VALIDATION; it would only leave that state as VALIDATION_TIMED_OUT after AWS's 72 h window.

Emulator: within seconds of the CNAME being UPSERTed into the undelegated zone (and the change reaching INSYNC), DescribeCertificate reports Status='ISSUED' with FailureReason absent — the replay fails 4 s into a 30 s absence window with "ACM must NOT issue '<apex>' from a CNAME in an undelegated zone: ... the certificate reports Status='ISSUED' (FailureReason None)". The apex in the message ('s2rprobe<hex>.com') is a per-run generated name; the requirement is the status token. Hypothesis: the validation driver matches the challenge against any ResourceRecordSet in any hosted zone of the account whose name suffixes the requested domain, with no public-delegation (root -> TLD -> zone NS) resolution step, so ownership of an undelegated zone is accepted as proof.

Why: a client relying on AWS's behavior sees issuance granted for a name it has not proven public control of. Every negative path collapses: IaC and provisioning code that waits on Status and treats ISSUED as "this name is publicly validated" proceeds to attach the certificate to a listener or custom domain; tests that assert validation is withheld for an unreachable zone pass against the emulator and fail against AWS; and the per-domain ValidationStatus/ListCertificates(ISSUED) index all agree on the wrong outcome, so there is no observable left that would catch it.

Fix: the ACM DNS-validation driver and the CertificateDetail projection in the acm service crate (the RequestCertificate handler's validation state machine plus the DescribeCertificate / ListCertificates / ListCertificateDomainValidations projections). Matching the published CNAME must be gated on the zone being reachable through the public delegation chain for the requested name; a zone that is not publicly delegated must leave Status PENDING_VALIDATION and per-domain ValidationStatus PENDING_VALIDATION with the ResourceRecord unchanged, and the certificate must not appear in ListCertificates(CertificateStatuses=['ISSUED']).

Related open filings that do NOT cover this check id: 20261029T093731Z-acm-dns-validated-certificate-issuance-is-unreachable-the.md, 20261110T152935Z-acm-certificate-state-machine-never-settles-a-non.md and 20261110T181625Z-acm-requested-certificates-never-settle-terminally-describecertificate-stays.md all describe the OPPOSITE direction (certificates that never reach a terminal state / never issue); none of them carries this replay key, and fixing the issuance path without a delegation gate is what produces this over-permissive outcome.

Traces: acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated.

## Evidence
Replay of suite simparity-behavioral-aws-v1: acm 234/252 (93%), 18 failed (17 of them already owned by open issues).

[mismatch] acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated:
  File "check_acm_dns_validation_withholds_issuance_when_the_zone_is_not_publicly_delegated.py", line 341, in run -> assert detail_now["Status"] == "PENDING_VALIDATION"
  AssertionError: ACM must NOT issue 's2rprobe1939de331859.com' from a CNAME in an undelegated zone: after 4s of a 30s window (after the import control reached ISSUED) the certificate reports Status='ISSUED' (FailureReason None)

What the check does: route53.create_hosted_zone(Name=<random>.com, HostedZoneConfig={'PrivateZone': False}) — a PUBLIC zone for a name no registrar delegates to its delegation set; acm.request_certificate(DomainName=<that apex>, ValidationMethod='DNS'); poll DescribeCertificate until DomainValidationOptions[0].ResourceRecord appears (asserting Status starts PENDING_VALIDATION and the record is a CNAME named _<token>... targeting _<token>.<label>.acm-validations.<domain>); UPSERT exactly that Name/Type/Value into the zone and wait for GetChange INSYNC; import a self-signed leaf as a positive control and wait for it to reach ISSUED (proving issuance works at all in this account); then hold a 30 s / >=5-read absence window asserting the DNS-validated certificate stays PENDING_VALIDATION with an unchanged ResourceRecord, is absent from ListCertificates(ISSUED), and still reads PENDING_VALIDATION through ListCertificateDomainValidations.

Expected (AWS): Status PENDING_VALIDATION and DomainValidationOptions[0].ValidationStatus PENDING_VALIDATION for the whole window; the ARN absent from ListCertificates(CertificateStatuses=['ISSUED']). Actual (emulator): Status 'ISSUED' on the first read of the window, with no FailureReason.

Per-run values (the generated apex, the hosted zone id, the challenge token, the certificate ARNs) are not requirements; the requirements are the status tokens 'PENDING_VALIDATION' (certificate and per-domain) and the absence from the ISSUED listing while the zone is not publicly delegated.

Smithy consulted: acm RequestCertificate, DescribeCertificate, CertificateStatus enum, DomainValidation.{ValidationStatus,ResourceRecord}, ResourceRecord.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay suite simparity-behavioral-aws-v1; acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated reproduces.
2. route53.create_hosted_zone(Name='<random-label>.com', CallerReference=<uuid>, HostedZoneConfig={'PrivateZone': False}) — a public zone for a name nothing delegates to it.
3. acm.request_certificate(DomainName='<that apex>', ValidationMethod='DNS'); poll acm.describe_certificate until DomainValidationOptions[0].ResourceRecord appears. Status must be 'PENDING_VALIDATION'.
4. route53.change_resource_record_sets(HostedZoneId=<zone>, Changes=[UPSERT of exactly that ResourceRecord Name/CNAME/Value, TTL 60]); poll route53.get_change until INSYNC.
5. acm.import_certificate of a self-signed leaf as a control; poll until it reads ISSUED.
6. Poll acm.describe_certificate on the DNS-validated ARN for 30 s. Expected (AWS): Status stays 'PENDING_VALIDATION', DomainValidationOptions[0].ValidationStatus stays 'PENDING_VALIDATION', the ResourceRecord is unchanged, the ARN is absent from acm.list_certificates(CertificateStatuses=['ISSUED']) and ListCertificateDomainValidations still reports PENDING_VALIDATION. Observed: Status 'ISSUED' within seconds.
7. Re-replay the check id in trace_ids to confirm the fix.
