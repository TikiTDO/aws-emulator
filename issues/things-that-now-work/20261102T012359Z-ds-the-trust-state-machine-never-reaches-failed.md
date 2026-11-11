Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.trust-create-verify-delete-lifecycle
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.trust-create-verify-delete-lifecycle|_wait_trust_state#7|Timeout|resp = ds.describe_trusts(DirectoryId=directory_id, TrustIds=[trust_id])
Issue-Commit: 52d0b788a035349f931bebcce1dabc39a95b4a82

# ds: the trust state machine never reaches Failed — CreateTrust to an unresolvable remote domain always settles Created, TrustStateReason is unreachable, and VerifyTrust accepts a non-verifiable trust

> _Note: this issue was identified against the code base at commit `52d0b788a035349f931bebcce1dabc39a95b4a82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrust

## Description
Behavioral replay of the ds suite (simparity-behavioral-aws-v1) left exactly ONE failing check after skipping every failure the report marks ALREADY FILED (scaffold stubs: ec2 CreateInternetGateway, which blocks 77 checks, plus ds DescribeCAEnrollmentPolicy, DeregisterCertificate, EnableSso, CancelSchemaExtension) or OWNED by an open issue. It reduces to ONE root cause, described below.

Related open issue, named per the clustering rules rather than relied on: 20261025T030730Z-ds-directory-service-4-behavioral-divergences-no-x-amzn.md describes the same ground as its Cause 4, but the replay report does NOT mark this check OWNED by it — that issue's recorded replay key for this check is `ds.trust-create-verify-delete-lifecycle|_wait_trust_state#7|Timeout|time.sleep(interval_s)`, while the current failure is a bare `Check timed out after 60.0s` with no exception frame, so the key no longer owns it. The defect is still present in the checkout at the commit replayed here, so it is filed here with the current grounding; a fixer landing both can land them together. This is not a reopen: that issue is open, not fixed.

Cause 1 of 1: a trust whose remote domain cannot be reached still settles in TrustState=Created, no TrustStateReason is ever produced, and VerifyTrust is guarded only against Deleting.

AWS: Smithy (com.amazonaws.directoryservice#DirectoryService_20150416): `TrustState` is an enum over Creating, Created, Verifying, VerifyFailed, Verified, Updating, UpdateFailed, Updated, Deleting, Deleted, Failed; `Trust.TrustStateReason` is documented "The reason for the TrustState"; `VerifyTrust` models InvalidParameterException (client, HTTP 400) and `DescribeTrusts` models EntityDoesNotExistException (client, HTTP 400). Recorded behavior (the check measures it on real AWS): CreateTrust(DirectoryId=<active Managed Microsoft AD>, RemoteDomainName=<unique>.invalid, TrustDirection='One-Way: Outgoing', ConditionalForwarderIpAddrs=['203.0.113.5']) goes Creating -> Failed — never Verified or VerifyFailed — within the check's bounded wait, and the Failed trust carries a non-empty TrustStateReason diagnostic; DescribeTrusts reports the RemoteDomainName the create was given. VerifyTrust on that Failed trust is then refused HTTP 400 InvalidParameterException whose message NAMES the state it refused from: the fixed text is the refusal sentence form AWS uses for a non-verifiable trust ("Cannot verify trust in state: <TrustState>"), its variable part is the trust's current state taken from stored state (here the literal token `Failed`), followed by this service's " : RequestId: <id>" suffix whose id is a per-run value, not a requirement. The refused verify leaves TrustState unchanged at Failed. DeleteTrust then removes the trust, after which DescribeTrusts(DirectoryId=..., TrustIds=[<id>]) is refused EntityDoesNotExistException.

Emulator: the remote domain is never resolved or contacted, so Failed is unreachable for ANY input. services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_trust.rs stores every new trust with `trust_state: TRUST_STATE_CREATING` and `trust_state_due_at: Some(now + TRANSITION_SPAN)` and inspects `RemoteDomainName` only for duplicate detection and for minting the conditional forwarder; src/state.rs `advance_clock` then maps the trust's Creating straight to Created (and Verifying straight to Verified) unconditionally, and no code path ever sets a TrustStateReason. Consequently the check's bounded poll of DescribeTrusts for TrustState=Failed never converges and the harness kills the check at its 60 s budget (replay output: `Error: Check timed out after 60.0s`, no exception frame). The same file set shows the dependent half: src/ops/verify_trust.rs refuses only `trust_state == TRUST_STATE_DELETING`, so a trust in any other non-verifiable state — Failed included — is ACCEPTED and driven to Verifying instead of refused 400 InvalidParameterException naming that state.

Why: against this emulator the only outcome a trust can have is success, so every trust failure path a client depends on is unreachable — a caller cannot diagnose a trust that did not establish (no Failed, no TrustStateReason), cannot exercise its refusal branch for a verify from a bad state (the verify succeeds and reports progress toward Verified), and a client polling DescribeTrusts for a terminal failure hangs until its own timeout instead of being told the trust failed. A caller that reads the emulator's Created/Verified result also gets a false positive: it believes a trust to an unreachable domain is established.

Fix: the trust state machine of this service — the CreateTrust handler plus the trust branch of `advance_clock` must settle a trust whose remote domain cannot be resolved/reached into Failed with a non-empty TrustStateReason (keeping Creating as the transient first state), and the VerifyTrust handler must refuse a verify started from a non-verifiable state (Failed included) with InvalidParameterException naming that state, leaving the stored TrustState untouched. File and line citations are under Traces/Evidence.

Traces: ds.trust-create-verify-delete-lifecycle.

## Evidence
Suite: simparity-behavioral-aws-v1 — total 29/156 passed (18.6%); 44 mismatched, 83 errored, 0 skipped, 0 unloadable. Per service: ds 27/141 (19%, 114 failed), ds__ec2 2/13 (15%, 11 failed), ds__logs 0/1, ds__sns 0/1. Every other failure in the report is marked ALREADY FILED (unimplemented scaffold stub) or OWNED by an open issue and was skipped; this is the only surviving failure.

Failing check: [mismatch] ds.trust-create-verify-delete-lifecycle — report detail is exactly `Error: Check timed out after 60.0s` with no exception frame, i.e. the check never got past a bounded poll.

What the check does (from the replayed source): obtains/creates a shared Managed Microsoft AD and waits it to Stage=Active, then
  resp = ds.create_trust(DirectoryId=<dir>, RemoteDomainName='<unique>.invalid', TrustPassword=..., TrustDirection='One-Way: Outgoing', ConditionalForwarderIpAddrs=['203.0.113.5'])
and polls `_wait_trust_state(..., terminal_ok={'Failed'}, terminal_bad={'Verified','VerifyFailed'}, timeout_s=200, interval_s=10)` over ds.describe_trusts(DirectoryId=<dir>, TrustIds=[<trust id>]). It then asserts: settled['TrustState'] == 'Failed'; settled.get('TrustStateReason') non-empty; settled['RemoteDomainName'] == the created domain; VerifyTrust(TrustId=<id>) raises with Error.Code == 'InvalidParameterException' and 'Failed' present in Error.Message; DescribeTrusts afterwards still reports TrustState 'Failed'; and after DeleteTrust, DescribeTrusts raises EntityDoesNotExistException.

Expected vs actual:
  Expected: TrustState Creating -> Failed with a non-empty TrustStateReason; VerifyTrust refused HTTP 400 InvalidParameterException naming the state it refused from (fixed text "Cannot verify trust in state: <state>", variable part the stored state — `Failed` here — plus this service's " : RequestId: <id>" suffix); the refused verify changes nothing.
  Actual: the poll never observes Failed (the emulator drives the trust Creating -> Created on a 500 ms clock span) so the check is killed at the 60 s harness budget; no TrustStateReason is ever populated; and VerifyTrust on such a trust returns 200 with the TrustId and moves the trust to Verifying.

Checkout evidence (commit replayed by this run):
  - services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_trust.rs lines ~276-305: inserts TrustRecord with `trust_state: crate::state::TRUST_STATE_CREATING.to_string()` and `trust_state_due_at: Some(now + crate::state::TRANSITION_SPAN)`; `RemoteDomainName` is used only for duplicate detection (`store.trust_of`) and the conditional-forwarder key — reachability of the remote domain is never evaluated, and the record has no trust_state_reason field at all.
  - services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/state.rs: module docs (lines ~17-27) describe advance_clock as materialising "a trust leaving `Creating` or `Verifying`" only; TRANSITION_SPAN = 500 ms (line 82). No path produces Failed/VerifyFailed or a TrustStateReason.
  - services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/verify_trust.rs lines 163-182: the only refusal branch is `if trust.trust_state == crate::state::TRUST_STATE_DELETING { InvalidParameterException("TrustId {id} is in state {state} and cannot be verified") }`; every other state falls through to `trust.trust_state = TRUST_STATE_VERIFYING`.

Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): `TrustState` enum values = Creating, Created, Verifying, VerifyFailed, Verified, Updating, UpdateFailed, Updated, Deleting, Deleted, Failed; `Trust.TrustStateReason` — "The reason for the TrustState"; CreateTrust errors ClientException/EntityAlreadyExistsException/EntityDoesNotExistException/InvalidParameterException/UnsupportedOperationException (400) and ServiceException (500); VerifyTrust errors ClientException/EntityDoesNotExistException/InvalidParameterException/UnsupportedOperationException (400) and ServiceException (500).

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite() to build, boot and replay against the emulator.
2. Create (or reuse) a Managed Microsoft AD and wait until DescribeDirectories reports Stage=Active.
3. Call CreateTrust(DirectoryId=<that directory>, RemoteDomainName='<unique>.invalid', TrustPassword='<any valid>', TrustDirection='One-Way: Outgoing', ConditionalForwarderIpAddrs=['203.0.113.5']).
4. Poll DescribeTrusts(DirectoryId=<directory>, TrustIds=[<TrustId>]). Expect TrustState Creating -> Failed with a non-empty TrustStateReason; observe it settle at Created with no TrustStateReason member (the poll for Failed never converges — the behavioral check is killed at its 60 s budget).
5. Call VerifyTrust(TrustId=<TrustId>) on that trust. Expect HTTP 400 InvalidParameterException whose message names the state it refused from ("Cannot verify trust in state: Failed : RequestId: <id>", the id being a per-run value) and expect DescribeTrusts to still report Failed; observe HTTP 200 with the TrustId and the trust moved to Verifying (then Verified by the clock).
6. Call DeleteTrust(TrustId=<TrustId>) and then DescribeTrusts(DirectoryId=<directory>, TrustIds=[<TrustId>]): expect EntityDoesNotExistException.
7. After fixing, re-replay ds.trust-create-verify-delete-lifecycle.
