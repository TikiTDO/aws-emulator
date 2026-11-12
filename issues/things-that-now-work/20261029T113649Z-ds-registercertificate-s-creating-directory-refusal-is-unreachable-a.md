Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable
Issue-Key: ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable|run#7|AssertionError|assert d["Stage"] == "Creating", f"precondition: the directory should be Creating, is {d['Stage']!r}"
Issue-Commit: a4702f9d21375d5a7473607a9aa2a567041fbdb6

# RegisterCertificate's Creating-directory refusal is unreachable: a new Microsoft AD is instantly Active, never passing through Creating (1 cause)

> _Note: this issue was identified against the code base at commit `a4702f9d21375d5a7473607a9aa2a567041fbdb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterCertificate

## Description
One net-new root cause in the Directory Service (ds) emulator, found by the behavioral suite simparity-behavioral-ga-prod-v1. The failing check id is ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable — a composite ds+ec2 check (its SERVICES list is ["ds", "ec2"]); the defective behavior is entirely in ds (CreateMicrosoftAD's directory lifecycle and RegisterCertificate's stage gate), so the issue is attributed to ds, matching this repo's existing filings for ds__ec2 checks. The replay report marked this check neither ALREADY FILED nor OWNED by an open issue, so it is filed here. The open issue 20261024T015148Z-ds-directory-service-a-new-microsoft-ad-is-instantly.md describes the same instantly-Active lifecycle for other checks but does not carry this run's replay key for this one, so it does not cover it; it is named here because closing it in the same place would resolve this cause too.

Cause 1 of 1: a freshly created Managed Microsoft AD is reported Stage 'Active' immediately, so RegisterCertificate's DirectoryUnavailableException refusal for a directory that is still provisioning can never be observed.

AWS: Smithy — CreateMicrosoftAD{Name @required @pattern, Password @required @sensitive, VpcSettings @required, Edition, Tags} returns only DirectoryId; DescribeDirectories returns DirectoryDescriptions, each with a Stage member. RegisterCertificate{DirectoryId @required @pattern ^d-[0-9a-f]{10}$, CertificateData @required @length 1..8192, Type} models DirectoryUnavailableException, DirectoryDoesNotExistException, InvalidCertificateException, CertificateAlreadyExistsException, CertificateLimitExceededException, ClientException, InvalidParameterException, UnsupportedOperationException (all @httpError 400) and ServiceException (500). The staged lifecycle itself is recorded behavior only: a new Microsoft AD reports Stage 'Requested' briefly, then 'Creating' for the many minutes the domain controllers take to come up, and only afterwards 'Active'. While it is not Active, a RegisterCertificate carrying a well-formed PEM CA certificate is refused at HTTP 400 with error code DirectoryUnavailableException and the message "Operation could not be performed because directory <DirectoryId> is unavailable. : RequestId: <RequestId>" — fixed text "Operation could not be performed because directory ", " is unavailable. : RequestId: "; the variable parts are the DirectoryId from the request and the RequestId from the response envelope. The refusal leaves no certificate behind, so ListCertificates stays empty before and after.

Emulator: after CreateMicrosoftAD, the very first DescribeDirectories poll past 'Requested' already reports Stage 'Active'. The directory never occupies 'Creating', so the stage-gated RegisterCertificate refusal is unreachable and the check cannot even reach its RegisterCertificate call; the registration would instead be accepted on an Active directory.

Why: the whole class of "not ready yet" refusals disappears. A client that handles DirectoryUnavailableException while a directory provisions — retrying certificate registration, LDAPS enablement or user-password resets until the directory is up — has that branch silently dead against the emulator, and the production race (calls issued during the ~20-minute provisioning window) cannot be reproduced at all. Conversely, a caller gets a 200 for an operation real AWS refuses, and then believes a certificate is registered on a directory that in AWS would not have accepted it.

Fix: the directory lifecycle the CreateMicrosoftAD handler writes and the stage the directory state reports, plus the stage gate RegisterCertificate reads before accepting a certificate — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_microsoft_ad.rs and src/ops/register_certificate.rs, over the directory record in crate::state. The directory must pass through Requested -> Creating -> Active on a clock-owed transition rather than being minted Active, and RegisterCertificate must return the modeled DirectoryUnavailableException with the message above while the directory is not Active, persisting nothing.

Traces: ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable (mismatch, assertion at check_ds__ec2_register_certificate_valid_pem_on_a_creating_directory_rejected_directory_unavailable.py:25).

## Evidence
Suite simparity-behavioral-ga-prod-v1 — total 14/66 passed (21.2%); ds__ec2 2/13 (15%), 11 failed.

[mismatch] ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable — check_..._register_certificate_valid_pem_on_a_creating_directory_rejected_directory_unavailable.py:25
  Sequence: ec2 VPC + two subnets -> ds:CreateMicrosoftAD -> poll DescribeDirectories past Stage 'Requested'.
  Expected: Stage 'Creating'; then ds:ListCertificates == []; then ds:RegisterCertificate(DirectoryId=<id>, CertificateData=<self-signed PEM CA>, Type='ClientLDAPS') refused HTTP 400 code DirectoryUnavailableException with message "Operation could not be performed because directory <DirectoryId> is unavailable. : RequestId: <RequestId>"; then ds:ListCertificates still [].
  Actual: the first poll past 'Requested' reports Stage 'Active', so the precondition fails before RegisterCertificate is ever called.
  AssertionError: precondition: the directory should be Creating, is 'Active'
  (In the expected message, "Operation could not be performed because directory " and " is unavailable. : RequestId: " are the fixed text; the directory id comes from the request and the request id from the response envelope — neither literal value from any single run is part of the requirement.)

Model corroboration (smithy_query query=operation): RegisterCertificate errors include DirectoryUnavailableException (client, httpError 400) and CertificateData is @length 1..8192, so the valid-PEM leg is a legal request that must be refused on stage, not on content; DescribeDirectories output DirectoryDescriptions carries the Stage member the precondition reads.

## Steps to Reproduce
Boot the emulator on fresh state; point botocore clients at the ds and ec2 endpoints.
1. ec2:CreateVpc (e.g. 10.179.0.0/23) with two subnets in two distinct AZs.
2. ds:CreateMicrosoftAD(Name='<unique>.test', Password=<strong password>, VpcSettings={'VpcId': <vpc>, 'SubnetIds': [<two subnets>]}).
3. Poll ds:DescribeDirectories(DirectoryIds=[<id>]) and read DirectoryDescriptions[0].Stage until it is no longer 'Requested'. Expect 'Creating'; observe 'Active'.
4. While it should still be Creating: ds:ListCertificates(DirectoryId=<id>) must be []; then ds:RegisterCertificate(DirectoryId=<id>, CertificateData=<a self-signed CA PEM>, Type='ClientLDAPS'). Expect ClientError code DirectoryUnavailableException at HTTP 400 with message "Operation could not be performed because directory <id> is unavailable. : RequestId: <rid>".
5. ds:ListCertificates(DirectoryId=<id>) must still be [] after the refusal.

Re-replay to confirm the fix: ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable
