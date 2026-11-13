Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.auto-termination-policy-invalid-idle-timeout-rejected emr.op-auto-termination-policy-ops-reject-unknown-cluster
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.auto-termination-policy-invalid-idle-timeout-rejected|_error#5|AssertionError|raise AssertionError("expected PutAutoTerminationPolicy to be rejected, but it succeeded")
Issue-Key: emr.op-auto-termination-policy-ops-reject-unknown-cluster|_assert_rejections#11|AssertionError|assert not mismatched, f"expected the message {expected_message!r}, got: {mismatched}"
Issue-Commit: 09ea2d6b5347c41b77d17c7337847f8c3ab13f71

# emr: auto-termination policy writes skip the IdleTimeout range refusal and word the unknown-cluster refusal as a job flow

> _Note: this issue was identified against the code base at commit `09ea2d6b5347c41b77d17c7337847f8c3ab13f71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAutoTerminationPolicy

## Description
Two net-new root causes in the EMR (elasticmapreduce) control plane, both on the auto-termination-policy write path. Each cause below is a separate work item; TWO causes must be closed before this issue is submitted.

Neither cause is covered by an existing issue. The open issue `20261105T111420Z-emr-three-cluster-scoped-refusal-paths-are-unreachable.md` names PutAutoTerminationPolicy, but it is grounded in the operation answering `NotImplemented` (its Issue-Key is `emr.auto-termination-policy-invalid-idle-timeout-rejected|run#6|AssertionError|assert code == "ValidationException"`). The operation is now implemented and answers 200, so that key no longer reproduces: the check now fails EARLIER, because the out-of-range call SUCCEEDS instead of being refused. The open stub issue `20261029T175551Z-emr-putautoterminationpolicy-stub.md` asks only that the operation be stood up and states no bound check, no refusal code and no message. The replay report marks neither of the two checks below ALREADY FILED or OWNED.

=== Cause 1 of 2: PutAutoTerminationPolicy does not enforce the documented IdleTimeout bounds, and persists an out-of-range policy ===
AWS: Smithy (com.amazonaws.emr#ElasticMapReduce) PutAutoTerminationPolicy takes ClusterId (@required, @length 0..256) and AutoTerminationPolicy; AutoTerminationPolicy.IdleTimeout is a Long whose model documentation states "Specifies the amount of idle time in seconds after which the cluster automatically terminates. You can specify a minimum of 60 seconds and a maximum of 604800 seconds (seven days)". The operation models no errors, so the refusal IDENTITY is recorded behavior only: an IdleTimeout outside the 60..604800-second window is refused with HTTP 400, error code `ValidationException`, and the message `Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds.` Every word of that sentence is FIXED text; the two numbers are the member's documented bounds, not values echoed from the request, and the offending IdleTimeout does NOT appear in the message. The refused call has no effect: a following GetAutoTerminationPolicy returns a response carrying no AutoTerminationPolicy.IdleTimeout. An in-range IdleTimeout on the same cluster is accepted (200) and GetAutoTerminationPolicy echoes it back unchanged.
Emulator: PutAutoTerminationPolicy accepts any IdleTimeout. The handler resolves the cluster and then stores `input.auto_termination_policy` verbatim with no bounds check at all, so a request with IdleTimeout=10 returns 200 and the sub-minimum policy becomes the cluster's attached policy. The check reports `AssertionError: expected PutAutoTerminationPolicy to be rejected, but it succeeded`.
Why: a client that relies on EMR's 60-second floor to validate its own cost-safety configuration never sees `ValidationException`, so its error branch never runs; it believes an unusable idle timeout was accepted, and the emulator then holds a cluster state AWS would never hold (an attached policy below the minimum), which also makes any auto-termination behaviour driven off that value divergent.
Fix: PutAutoTerminationPolicy's input validation in its handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/put_auto_termination_policy.rs, the `handle` body around line 172, which assigns `cluster.auto_termination_policy = input.auto_termination_policy` with no range check), refusing through the same 400 `ValidationException` channel the handler already uses for an unknown cluster, and leaving the cluster's stored policy untouched on refusal.
Traces: emr.auto-termination-policy-invalid-idle-timeout-rejected

=== Cause 2 of 2: Put/RemoveAutoTerminationPolicy refuse an unknown ClusterId with the job-flow sentence instead of the cluster-id sentence ===
AWS: Smithy PutAutoTerminationPolicy and RemoveAutoTerminationPolicy both take ClusterId (@required, @length 0..256) and model no errors, so the refusal identity is recorded behavior only. All three auto-termination-policy operations — GetAutoTerminationPolicy, PutAutoTerminationPolicy, RemoveAutoTerminationPolicy — refuse a ClusterId the service holds no record for uniformly with HTTP 400, error code `ValidationException`, and the message `Cluster id '<ClusterId>' is not valid.` The fixed text is `Cluster id '` … `' is not valid.`; the quoted value is the ClusterId copied verbatim from the request (it is a request value, not a constant — the recording's `j-ZZZZZZZZZZZZZ` is only one instance of it). The same three calls round-trip a policy against a real cluster: Put(IdleTimeout=600) -> 200, Get -> IdleTimeout 600, Remove -> 200, Get -> no IdleTimeout.
Emulator: the error CODE is right (`ValidationException`) but the message is wrong on the two mutators. Both answer the constant job-flow sentence `Specified job flow ID not valid.`, which names no id, while GetAutoTerminationPolicy on the same unknown id answers the correct `Cluster id '<ClusterId>' is not valid.` The check reports `AssertionError: expected the message "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.", got: {'PutAutoTerminationPolicy': 'Specified job flow ID not valid.', 'RemoveAutoTerminationPolicy': 'Specified job flow ID not valid.'}`.
Why: a client that parses or logs the refusal to report which cluster id was rejected gets a sentence that names no id at all, and the family is self-inconsistent — the same unknown id produces two different messages depending on which of the three policy operations was called, so no single error matcher works across the family.
Fix: the unknown-cluster refusal in the PutAutoTerminationPolicy and RemoveAutoTerminationPolicy handlers (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/put_auto_termination_policy.rs around line 163 and services/service-com-amazonaws-emr-elasticmapreduce/src/ops/remove_auto_termination_policy.rs around line 152), both of which call `crate::state::unknown_job_flow_message()`; they must use the shared cluster-id message GetAutoTerminationPolicy already produces, so one place renders the sentence for the whole policy family.
Traces: emr.op-auto-termination-policy-ops-reject-unknown-cluster

## Evidence
Behavioral replay against the booted emulator, service heading `### emr`. Both failing checks are verdict `mismatch` and are marked neither ALREADY FILED nor OWNED in the report.

Cause 1 — check emr.auto-termination-policy-invalid-idle-timeout-rejected:
  expected: on a live (WAITING/RUNNING) cluster, PutAutoTerminationPolicy(ClusterId=<id>, AutoTerminationPolicy={"IdleTimeout": 10}) -> HTTP 400, Code=ValidationException, Message="Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds." (all fixed text); then GetAutoTerminationPolicy(ClusterId=<id>) -> no AutoTerminationPolicy.IdleTimeout; then PutAutoTerminationPolicy(IdleTimeout=3600) -> 200 and GetAutoTerminationPolicy echoes 3600.
  actual: the call returned 200. AssertionError: expected PutAutoTerminationPolicy to be rejected, but it succeeded
  (source: check_emr_auto_termination_policy_invalid_idle_timeout_rejected.py line 32 -> _error at line 65)

Cause 2 — check emr.op-auto-termination-policy-ops-reject-unknown-cluster:
  expected: with ClusterId = "j-" + "Z"*13 (an id the service holds no record for), each of GetAutoTerminationPolicy, PutAutoTerminationPolicy(AutoTerminationPolicy={"IdleTimeout": 600}) and RemoveAutoTerminationPolicy -> HTTP 400, Code=ValidationException, Message="Cluster id 'j-ZZZZZZZZZZZZZ' is not valid." — fixed text "Cluster id '" … "' is not valid." with the quoted id taken from the request.
  actual: all three answered ValidationException (code assertion passed), but the messages diverged. AssertionError: expected the message "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.", got: {'PutAutoTerminationPolicy': 'Specified job flow ID not valid.', 'RemoveAutoTerminationPolicy': 'Specified job flow ID not valid.'}
  (source: check_emr_op_auto_termination_policy_ops_reject_unknown_cluster.py line 66 -> _assert_rejections at line 35; GetAutoTerminationPolicy is absent from the mismatch map, so only the two mutators diverge)

Corroborating source read (commit on the cloned finder branch): put_auto_termination_policy.rs `handle` assigns `cluster.auto_termination_policy = input.auto_termination_policy` with no IdleTimeout bounds check, and both put_auto_termination_policy.rs and remove_auto_termination_policy.rs refuse an unresolved cluster with `crate::scaffolded::Scaffolded::validation(crate::state::unknown_job_flow_message())`.

## Steps to Reproduce
1. Boot the emulator and point an EMR (elasticmapreduce) client at it.
2. RunJobFlow a keep-alive cluster and wait until DescribeCluster reports RUNNING or WAITING; call it <id>.
3. Cause 1: call PutAutoTerminationPolicy(ClusterId=<id>, AutoTerminationPolicy={"IdleTimeout": 10}). Observe HTTP 200 instead of a 400 ValidationException carrying "Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds.". Then call GetAutoTerminationPolicy(ClusterId=<id>) and observe the sub-minimum policy was persisted (AWS returns no IdleTimeout, because the call was refused). Repeat with IdleTimeout above 604800 for the upper bound. Finally call PutAutoTerminationPolicy(IdleTimeout=3600) and GetAutoTerminationPolicy to confirm the in-range value still round-trips after the fix.
4. Cause 2: with j = "j-" + "Z"*13, call GetAutoTerminationPolicy(ClusterId=j), PutAutoTerminationPolicy(ClusterId=j, AutoTerminationPolicy={"IdleTimeout": 600}) and RemoveAutoTerminationPolicy(ClusterId=j). All three answer Code=ValidationException, but Get answers Message "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid." while Put and Remove answer "Specified job flow ID not valid.". All three must answer the cluster-id sentence with the request's ClusterId quoted.
5. Confirm the positive control still holds on a real cluster: Put(IdleTimeout=600) -> 200, Get -> 600, Remove -> 200, Get -> no IdleTimeout.
6. Re-replay the two checks named in trace_ids to confirm the fix.
