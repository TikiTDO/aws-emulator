Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.disable-directory-data-access-recloses-the-door-without-losing-contents
Issue-Key: ds.disable-directory-data-access-recloses-the-door-without-losing-contents|_poll#36|Timeout|return last
Issue-Commit: b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca

# ds: the directory-data-access disable/re-enable cycle reaches no verdict — the check is killed at the 60 s budget inside its fourth transition poll, with no response divergence left

> _Note: this issue was identified against the code base at commit `b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableDirectoryDataAccess

## Description
Relation to an already-open issue: 20261107T061500Z-ds-data-access-cycle-check-cannot-fit-its-sixty-second-budget.md describes this same check and the same timeout, and was filed by a fixer rather than by a replay sweep. The behavioral suite did NOT mark this failing check as OWNED by it, so it is re-filed here on its own replay evidence and scoped to what this replay actually shows; the two should be resolved as one work item, and the earlier file's two candidate resolutions still stand.

Cause 1 of 1: the whole data-access disable/re-enable scenario reaches NO verdict — the emulator answers every call, but the four clock-owed transitions the scenario drives cost the client one full poll interval each, so the check is killed at the harness's 60 s budget before its surviving assertions run.

AWS: recorded behavior only for the timing; the STATUSES are modelled. Smithy com.amazonaws.directoryservice#DataAccessStatus = [Disabled, Disabling, Enabled, Enabling, Failed] and DirectoryStage carries Requested/Creating/Active, so AWS does report transient legs for both toggles and for the create. DescribeDirectoryDataAccess reports DataAccessStatus; EnableDirectoryDataAccess and DisableDirectoryDataAccess start the transition and answer before it settles (both model DirectoryInDesiredStateException, "The directory is already updated to desired update type settings", and DirectoryUnavailableException, HTTP 400). On real AWS the create leg alone takes 20–45 minutes, so the recording is not what bounds the wall clock here — the 60 s budget is the emulator's.

Emulator: every response the scenario asserts on appears to be implemented and correct; nothing in this replay reports a wrong status, code or message. What fails is wall-clock. The scenario drives four transitions, and the client probes each one immediately after the call that starts it, sees the modelled transient value, then sleeps a fixed interval: create → poll Stage at 20 s intervals, enable → poll DataAccessStatus at 10 s, disable → poll at 15 s, re-enable → poll at 10 s. That is 55 s of unavoidable sleeping inside a 60 s ceiling, before the ds-data read-back traffic at the end of the scenario. The emulator's own transition span is 500 ms (state.rs TRANSITION_SPAN) plus a 3 s directory build span, i.e. every transition settles two orders of magnitude inside the client's poll interval but always AFTER the probe that precedes it, so each one still costs a full interval. The two sibling ds checks that drive only the create and the first enable (30 s of the same sleeps) do reach their assertions in this same replay, which locates the overrun in the two extra toggles this scenario adds.

Why: a client gets no verdict at all on the behavior this scenario exists to pin — that disabling data access refuses every ds-data call with AccessDeniedException/Reason DATA_DISABLED and the identical message for a read, a write and an inventory listing; that the directory stays Stage Active while its data plane is shut; that a CreateUser refused while disabled leaves no object behind; and that the user, group, membership and both SIDs survive a disable/re-enable cycle unchanged. None of those contracts is exercised, so a regression in any of them would pass unnoticed: the check cannot distinguish a correct emulator from a broken one while it dies in a sleep. It is also the one failure class that gets worse, not better, as more of the surface is implemented, because each added clock-owed leg costs another poll interval.

Fix: not an operation handler and not a serializer — the scenario's wall-clock cost is set by the project's clock-owed-transition contract as it meets the suite's per-check ceiling, so it belongs to whoever owns one of the two. Either the transition span the ds state applies to the data-access toggles and the directory stage chain (so a settled status is observable on the client's FIRST probe, at the price of removing Enabling/Disabling/Requested and the state refusals that depend on them from the observable surface), or the per-check budget / poll intervals the harness replays this scenario with. A handler change inside DisableDirectoryDataAccess, EnableDirectoryDataAccess or DescribeDirectoryDataAccess cannot move it. File and line citations are under Traces.

Traces: ds.disable-directory-data-access-recloses-the-door-without-losing-contents (verdict mismatch, "Check timed out after 60.0s", killed in the check's _poll helper at its time.sleep(interval_s)).

## Evidence
Failing check (behavioral suite simparity-behavioral-aws-v1, listed under service heading '### ds'):

[mismatch] ds.disable-directory-data-access-recloses-the-door-without-losing-contents
  Error: Check timed out after 60.0s

Expected (AWS / the check): the scenario runs to its assertions — DisableDirectoryDataAccess settles DataAccessStatus on 'Disabled'; ds-data DescribeUser, CreateUser and ListUsers against that directory are then all refused with HTTP 403, Error.Code 'AccessDeniedException', top-level Reason 'DATA_DISABLED' and one IDENTICAL Message (the gate is on the directory, not the operation); ds DescribeDirectories keeps reporting Stage 'Active' throughout; EnableDirectoryDataAccess drives the status back to 'Enabled'; the pre-existing user and group come back with the SAME SIDs and attributes, the membership edge is reported by both ListGroupMembers and ListGroupsForMember, and the user whose CreateUser was refused while the plane was shut is absent from the paginated ListUsers inventory.

Actual (emulator): no assertion is reached. The harness kills the check at 60 s inside a poll sleep. No status, error code or message divergence is reported by the replay.

Why the budget cannot be met, from the check's own intervals (all four polls probe immediately after the call that starts the transition, so each observes the modelled transient and sleeps once):
  create_microsoft_ad   -> poll describe_directories for a terminal Stage, interval 20 s  -> 20 s
  enable_directory_data_access  -> poll DescribeDirectoryDataAccess for Enabled, interval 10 s -> 10 s
  disable_directory_data_access -> poll for Disabled, interval 15 s                          -> 15 s
  enable_directory_data_access  -> poll for Enabled again, interval 10 s                      -> 10 s
                                                                                        total 55 s
plus ~30 further ds / ds-data / ec2 / sts requests, inside a 60 s ceiling.

Static confirmation that no response divergence remains to explain the timeout (services/service-com-amazonaws-directoryservice-directoryservice-20150416/):
  src/ops/disable_directory_data_access.rs:201-235 — refuses a non-Active directory with DirectoryUnavailableException, an already-Disabled/Disabling one with DirectoryInDesiredStateException, then records DATA_ACCESS_DISABLING with data_access_due_at = now + TRANSITION_SPAN;
  src/ops/disable_directory_data_access.rs:244-248 — mirrors the change into the record both planes share (ad_directory_inventory.disable_data_access) with the SAME due instant;
  src/ops/enable_directory_data_access.rs:216-245 — the mirrored toggle, same shape;
  src/state.rs:3681-3691 — advance_clock settles BOTH transients: Enabling -> Enabled and Disabling -> Disabled, clearing data_access_due_at;
  src/state.rs:119 — TRANSITION_SPAN = 500 ms; src/state.rs:146 — DIRECTORY_BUILD_SPAN = 3 s; src/state.rs:156-158 — directory_active_at = now + TRANSITION_SPAN + DIRECTORY_BUILD_SPAN (3.5 s, i.e. far inside the client's 20 s interval but after its first probe);
  services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/service/authorization.rs:99-136 — the ds-data gate is present and directory-scoped: 403 AccessDeniedException with Reason DATA_DISABLED and the single fixed sentence "DS Data feature is not enabled on this directory. This feature must first be enabled by calling the EnableDirectoryDataAccess API." for every operation, read, write and listing alike.

Corroborating replay context: the two sibling ds checks that drive only the create and one enable (30 s of the same fixed sleeps) DO reach their assertions in this same run — ds.ds-data-user-create-describe-search-roundtrip and ds.ds-data-group-inventory-and-search-answer-for-the-builtin-groups both fail on response content, not on the clock — so the overrun is attributable to the two extra toggle legs this scenario adds rather than to slow request service.

Smithy consulted (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json):
  DataAccessStatus = [Disabled, Disabling, Enabled, Enabling, Failed]
  DisableDirectoryDataAccess: input {DirectoryId @required, pattern ^d-[0-9a-f]{10}$}; errors AccessDeniedException(400), ClientException(400), DirectoryDoesNotExistException(400), DirectoryInDesiredStateException(400), DirectoryUnavailableException(400), ServiceException(500), UnsupportedOperationException(400)
  DescribeDirectoryDataAccess: output {DataAccessStatus}
and (services/service-com-amazonaws-directoryservicedata-directoryservicedata/...): AccessDeniedException @httpError 403 with member Reason -> AccessDeniedReason [IAM_AUTH, DIRECTORY_AUTH, DATA_DISABLED]; the 403 is a service-common error on all 17 operations.

## Steps to Reproduce
1. Boot the emulator and create botocore clients for 'ds', 'ds-data', 'ec2' and 'sts' against it.
2. Replay ds.disable-directory-data-access-recloses-the-door-without-losing-contents. It is killed at the 60 s budget inside a poll sleep, with no status/code/message divergence reported.
3. Reproduce the arithmetic by hand, timing each step: create_microsoft_ad in a VPC with two subnets in distinct AZs, then poll describe_directories at 20 s intervals until Stage == 'Active' (the first probe observes 'Requested'); enable_directory_data_access and poll describe_directory_data_access at 10 s intervals until 'Enabled' (first probe 'Enabling'); create a ds-data user, group and membership; disable_directory_data_access and poll at 15 s until 'Disabled' (first probe 'Disabling'); enable_directory_data_access again and poll at 10 s until 'Enabled'. Four probes land on a transient, four intervals are slept, 55 s of the 60 s ceiling is gone before the read-back traffic.
4. Confirm the responses themselves are not the defect: with the polling removed (sleep a second after each toggle instead), walk the same sequence and check each assertion directly — ds-data describe_user / create_user / list_users against the shut directory must each answer HTTP 403, Code 'AccessDeniedException', Reason 'DATA_DISABLED' with one identical Message; ds describe_directories must still report Stage 'Active'; after the re-enable, describe_user and describe_group must return the SAME SIDs recorded before the disable, list_group_members must report exactly the one user and list_groups_for_member the one group, and list_users must contain that user and NOT the user whose create was refused while the plane was shut.
5. Decide the resolution at the level that owns it — either the clock-owed-transition span the ds state applies (so a settled status is observable on the FIRST probe, accepting that Enabling/Disabling/Requested and the DirectoryInDesiredStateException / DirectoryUnavailableException paths that depend on them stop being observable, and loosening the in-tree suites that pin them: core/router/tests/all/directory_data_access_cycle.rs and services/service-com-amazonaws-directoryservice-directoryservice-20150416/tests/all/directory_data_access_disable.rs), or this check's budget and poll intervals.
6. Re-replay ds.disable-directory-data-access-recloses-the-door-without-losing-contents and confirm it reaches a verdict on every assertion listed in step 4 rather than dying in a sleep. Resolve together with the already-open 20261107T061500Z-ds-data-access-cycle-check-cannot-fit-its-sixty-second-budget.md, which carries this same check id.
