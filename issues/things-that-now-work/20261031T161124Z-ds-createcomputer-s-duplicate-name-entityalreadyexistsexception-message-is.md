Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.create-computer-authentication-failed-rejected
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.create-computer-authentication-failed-rejected|run#22|AssertionError|assert dup_message == expected_dup_message, f"unexpected error message: {dup_message!r}"
Issue-Commit: 1df8b1c6697d848ab2962a5410ee5c810b07ea40

# ds: CreateComputer's duplicate-name EntityAlreadyExistsException message is emulator-authored ("Directory <id> already holds the computer account <name>") instead of AWS's "Could not create computer. RequestId: <rid>"

> _Note: this issue was identified against the code base at commit `1df8b1c6697d848ab2962a5410ee5c810b07ea40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComputer

## Description
One net-new root cause in the Directory Service (ds) emulator, found by behavioral replay (suite simparity-behavioral-aws-v1). Every other failing ds / ds__ec2 check in this run was marked ALREADY FILED (unimplemented scaffold stubs: EC2 CreateInternetGateway, DescribeCAEnrollmentPolicy, DeregisterCertificate, EnableSso, CancelSchemaExtension) or OWNED by an open issue and was skipped.

=== Cause 1 of 1: the EntityAlreadyExistsException raised by a second CreateComputer of the same account name carries a message this crate authors, not the fixed sentence ds sends ===

AWS: Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json, smithy_query query=operation) models CreateComputer{DirectoryId required @pattern ^d-[0-9a-f]{10}$, ComputerName required @length 1..15, Password required @sensitive, OrganizationalUnitDistinguishedName, ComputerAttributes} -> Computer, with errors AuthenticationFailedException / ClientException / DirectoryUnavailableException / EntityAlreadyExistsException / EntityDoesNotExistException / InvalidParameterException / UnsupportedOperationException (all errorKind client, @httpError 400) and ServiceException (500). EntityAlreadyExistsException = {Message, RequestId}; the model does not state the text, so this is recorded behavior only: a CreateComputer naming a ComputerName that already exists in the target Active directory is refused with HTTP 400, code EntityAlreadyExistsException, and a message that does NOT name the directory or the account at all — it is the fixed ds sentence "Could not create computer. RequestId: <request-id>" followed by this service's envelope suffix " : RequestId: <request-id>", i.e. the whole message is "Could not create computer. RequestId: <rid> : RequestId: <rid>". Fixed text of the template: "Could not create computer. RequestId: " and the envelope's " : RequestId: "; the only variable part is the response's own request id, which appears twice (once inside the service diagnostic, once in the envelope suffix). AWS discloses neither the DirectoryId nor the conflicting ComputerName on this error class. (Note: this exact wording is already recorded in the resolved issue 20261026T060551Z-ds-3-net-new-behavioral-root-causes-deregistereventtopic.md, which named this leg as not-yet-graded while its own Cause 2 — the DirectoryUnavailableException wording — was the failing leg; that leg now passes and the check fails here instead.)

Emulator: status 400 and code EntityAlreadyExistsException are both correct, but the message is a locally composed sentence naming the directory and the stored account: "Directory d-003ba5fe5a already holds the computer account sim2real3260984 : RequestId: 00000000-0000-4000-8000-000000000002" (the id, name and request id are one run's values; the divergence is the entire sentence template). It is built at services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_computer.rs lines 248-259 with format!("Directory {id} already holds the computer account {name}").

Why: the error message is part of the contract. A client, parity test or operator tool that matches ds's conflict diagnostic, or that scrapes the request id out of it for support correlation, never matches real AWS; and the emulator discloses two values (the directory id and the existing account name) that AWS deliberately withholds on this refusal, so code written against the emulator reads message fields that are absent on real AWS. The preceding legs of the same check — the not-yet-Active DirectoryUnavailableException wording and the Active-stage response echo of ComputerName / ComputerId / the re-cased "Description" attribute — now pass, so this message is the only remaining divergence on CreateComputer.

Fix: the duplicate-account branch of the CreateComputer handler — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_computer.rs lines 248-259 — must build the EntityAlreadyExistsException diagnostic from ds's fixed "Could not create computer. RequestId: <the response's request id>" sentence (passed through the same crate::state::refusal envelope the neighbouring refusals use, which appends " : RequestId: <rid>") instead of interpolating the directory id and the stored computer name. No other branch of the handler changes.

Traces: ds.create-computer-authentication-failed-rejected (AssertionError at check_ds_create_computer_authentication_failed_rejected.py line 82, the dup_message assertion). No open issue carries this replay key: 20261027T012551Z-ds-4-net-new-behavioral-root-causes-createcomputer.md lists this check id, but its key is the earlier ComputerAttributes-echo assertion (run#20), which the report shows now passes — which is why the suite did not mark this check OWNED.

## Evidence
Suite simparity-behavioral-aws-v1 — total 27/156 passed (17.3%); ds 25/141 (18%), 116 failed; ds__ec2 2/13; ds__logs 0/1; ds__sns 0/1. Exactly one failing check in the whole report is neither ALREADY FILED nor OWNED: ds.create-computer-authentication-failed-rejected.

[mismatch] ds.create-computer-authentication-failed-rejected — check_ds_create_computer_authentication_failed_rejected.py:82
  Setup that PASSES first: create_vpc + two subnets in distinct AZs; create_microsoft_ad(Edition='Standard'); CreateComputer while the directory is not yet Active -> 400 DirectoryUnavailableException with "Directory: '<id>' does not exist. : RequestId: <rid>" (matched); wait for Stage 'Active'; CreateComputer(ComputerAttributes=[{'Name':'description','Value':'<marker>'}]) -> Computer echoes ComputerName, a non-empty ComputerId and a 'Description' attribute carrying the marker (matched).
  Failing call: create_computer(DirectoryId=<same active d-...>, ComputerName=<the same 15-char name>, Password='Sim2Real-Computer-Pw2!')
  Code EntityAlreadyExistsException and HTTP 400 — MATCHED.
  Expected message (real AWS): "Could not create computer. RequestId: <response RequestId> : RequestId: <response RequestId>"
  Actual message (emulator):   "Directory d-003ba5fe5a already holds the computer account sim2real3260984 : RequestId: 00000000-0000-4000-8000-000000000002"
  AssertionError: unexpected error message: 'Directory d-003ba5fe5a already holds the computer account sim2real3260984 : RequestId: 00000000-0000-4000-8000-000000000002'
  Delta: the whole service diagnostic ahead of the " : RequestId: " envelope suffix. AWS's fixed text is "Could not create computer. RequestId: " + the request id; the emulator substitutes a sentence interpolating the DirectoryId and the existing ComputerName, neither of which AWS reports on this error class.

Code: services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_computer.rs lines 247-259 — the computers map is keyed by the case-folded name and the hit returns CreateComputerErrorModeled::EntityAlreadyExistsException(crate::state::refusal(format!("Directory {id} already holds the computer account {name}", ...))).

Model corroboration (smithy_query query=operation, com-amazonaws-directoryservice-directoryservice-20150416.json): CreateComputer errors include EntityAlreadyExistsException with errorKind client and httpError 400 (defaulted); EntityAlreadyExistsException = {Message, RequestId}; the model states no message text, hence "recorded behavior only" for the wording above.

## Steps to Reproduce
Boot the emulator on fresh state and point a botocore client at the ds endpoint (plus ec2 for the VPC fixture).

1. ec2.create_vpc(CidrBlock='10.N.0.0/23') and two subnets in two distinct AZs of that VPC.
2. ds.create_microsoft_ad(Name='<unique>.example.com', Password=<strong>, Edition='Standard', VpcSettings={'VpcId': <vpc>, 'SubnetIds': [<subnet-a>, <subnet-b>]}).
3. Poll ds.describe_directories(DirectoryIds=[<id>]) until Stage == 'Active'.
4. ds.create_computer(DirectoryId=<id>, ComputerName='sim2real12345', Password='Sim2Real-Computer-Pw1!', ComputerAttributes=[{'Name': 'description', 'Value': 'marker-1'}]) — succeeds.
5. Repeat the SAME ComputerName: ds.create_computer(DirectoryId=<id>, ComputerName='sim2real12345', Password='Sim2Real-Computer-Pw2!').
6. Read err.response['Error']['Code'] -> 'EntityAlreadyExistsException' (already correct) and err.response['Error']['Message']. Expect it to equal "Could not create computer. RequestId: <err.response['ResponseMetadata']['RequestId']> : RequestId: <the same request id>"; observe "Directory <directory-id> already holds the computer account <computer-name> : RequestId: <rid>".
7. Re-replay ds.create-computer-authentication-failed-rejected to confirm — the whole check must pass, including its earlier DirectoryUnavailableException and Active-stage echo legs, which pass today.
