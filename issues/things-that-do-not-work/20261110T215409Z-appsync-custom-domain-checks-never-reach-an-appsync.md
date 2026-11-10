Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.domain-name-lifecycle appsync.domain-name-lifecycle-state-transitions appsync.domain-name-crud-lifecycle appsync.domain-name-get-not-found-after-delete appsync.api-association-get-not-found-when-unassociated appsync.associate-api-domain-mismatched-certificate-rejected appsync.deploy-domain-name-association-cfn-behavioral
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.domain-name-lifecycle|_wait_for#4|Timeout|time.sleep(5)
Issue-Key: appsync.domain-name-lifecycle-state-transitions|_wait_for#4|Timeout|time.sleep(5)
Issue-Key: appsync.domain-name-crud-lifecycle|_wait_for#4|Timeout|time.sleep(5)
Issue-Key: appsync.domain-name-get-not-found-after-delete|_wait_for#4|Timeout|time.sleep(5)
Issue-Key: appsync.api-association-get-not-found-when-unassociated|_wait_for#4|Timeout|time.sleep(5)
Issue-Key: appsync.associate-api-domain-mismatched-certificate-rejected|_wait_for#5|Timeout|time.sleep(5)
Issue-Key: appsync.deploy-domain-name-association-cfn-behavioral|_wait_for#8|Timeout|time.sleep(5)
Issue-Commit: 790c5c5e1c0f240249b840d64996fdcf530cd42d

# AppSync custom-domain checks never reach an AppSync call: the prerequisite ACM DNS-validated certificate for a name in the account's own hosted zone never becomes usable, so every requires:dns-baseline domain check stalls until the run budget expires

> _Note: this issue was identified against the code base at commit `790c5c5e1c0f240249b840d64996fdcf530cd42d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
One net-new root cause, grounded in 7 failing checks of this suite.

=== Cause 1 of 1: the custom-domain prerequisite (an ACM certificate DNS-validated through the account's tagged public hosted zone) never converges, so CreateDomainName/GetApiAssociation are never exercised and the checks stall ===
AWS: recorded behavior of the prerequisite these checks build before touching AppSync at all: route53 ListHostedZones + ListTagsForResource find the account's public baseline zone (tag sim2real-infra-dns-baseline=true) and return its apex; acm.RequestCertificate(ValidationMethod='DNS') in us-east-1 publishes a DomainValidationOptions[0].ResourceRecord; after that CNAME is UPSERTed into the zone, DescribeCertificate transitions the certificate PENDING_VALIDATION -> ISSUED; AppSync's CreateDomainName then accepts that ACM-issued certificate (it accepts only certificates ACM itself issued, from us-east-1) and the domain becomes active with appsyncDomainName and domainNameArn populated, GetApiAssociation 404s with "Domain name is not associated." until AssociateApi links an Api, and GetDomainName 404s with "Domain name not found" after DeleteDomainName.
Emulator: none of these checks ever reaches an AppSync domain call. Each one blocks in the prerequisite — the baseline zone lookup and/or the DNS-validated certificate never reaching ISSUED (the checks poll DescribeCertificate for up to 600s) — and the suite reports them only as "Error: Check timed out after 60.0s". Because the stall is upstream of AppSync, no status, error code or message can be quoted for the AppSync operations themselves; what is observable is that the whole requires:dns-baseline family cannot make progress.
Why: a client following AWS's documented custom-domain flow (ACM DNS-validated cert -> CreateDomainName -> AssociateApi -> serve GraphQL on the custom domain) cannot complete even the first step against the emulator, so the entire custom-domain surface of AppSync is untestable and the behaviour of CreateDomainName/AssociateApi/GetApiAssociation/UpdateDomainName/DeleteDomainName stays unverified.
Fix: the prerequisite chain these checks depend on — the route53 hosted-zone tag discovery and the ACM DNS-validation state machine that must take a requested certificate to ISSUED once its validation CNAME exists — and then the AppSync CreateDomainName certificate-provenance path that consumes it. NOTE: open issues 20261030T083911Z-appsync-the-account-s-dns-baseline-public-hosted.md and 20261101T214920Z-appsync-custom-domain-checks-are-all-blocked-upstream.md describe this same upstream blockage; the suite did not mark these 7 checks as owned by either, so they are filed here and may be closed by the same fix.
Traces: the 7 check ids in trace_ids, all tagged requires:dns-baseline, all ending "Check timed out after 60.0s".

## Evidence
Suite verdict for all 7 checks: "Error: Check timed out after 60.0s" with no AppSync response captured.
What the checks do before timing out (from the replayed sources): _dns_zone(ctx) walks route53 ListHostedZones and ListTagsForResource for the public zone tagged sim2real-infra-dns-baseline=true; _dns_validated_cert() calls acm(us-east-1).RequestCertificate(DomainName=<label>.<apex>, ValidationMethod='DNS'), waits up to 120s for DomainValidationOptions[0].ResourceRecord, UPSERTs that record via route53 ChangeResourceRecordSets, then waits up to 600s for DescribeCertificate Status to leave PENDING_VALIDATION and asserts it is ISSUED. Only after that does the check call appsync.create_domain_name(...).
Expected, once the prerequisite converges: CreateDomainName returns domainNameConfig with domainName/certificateArn echoed and appsyncDomainName + domainNameArn populated; GetDomainName after DeleteDomainName returns NotFoundException with the message "Domain name not found"; GetApiAssociation on an unassociated domain returns NotFoundException with the message "Domain name is not associated."; AssociateApi then drives associationStatus PROCESSING -> SUCCESS.
Actual: the run budget expires inside the prerequisite polling, so no AppSync domain operation is reached at all.

## Steps to Reproduce
1. route53: list_hosted_zones + list_tags_for_resource and find the public zone tagged sim2real-infra-dns-baseline=true; note its apex.
2. acm (region us-east-1): request_certificate(DomainName='<unique-label>.<apex>', ValidationMethod='DNS'); poll describe_certificate until Certificate.DomainValidationOptions[0].ResourceRecord appears.
3. route53: change_resource_record_sets UPSERT that Name/Type/Value into the zone.
4. acm: poll describe_certificate until Certificate.Status leaves PENDING_VALIDATION — expect ISSUED. Observe that it does not converge, so the caller never proceeds.
5. The AppSync steps that follow (create_domain_name(domainName=..., certificateArn=...), get_domain_name, update_domain_name(description=...), associate_api(domainName, apiId), get_api_association(domainName), delete_domain_name) are therefore never exercised; replay any check in trace_ids to see the 60s stall.
