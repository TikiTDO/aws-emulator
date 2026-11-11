Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.disable-directory-data-access-recloses-the-door-without-losing-contents
Issue-Key: ds.disable-directory-data-access-recloses-the-door-without-losing-contents|_poll#32|Timeout|last = fn()
Issue-Commit: 0cb9466a75cf8bd8800aa1a80d3e628d3200d311

# ds: the directory-data-access disable/re-enable scenario reaches no verdict — every clock-owed transition settles only AFTER the client's first probe, so the check is killed at the 60 s budget inside a poll sleep

> _Note: this issue was identified against the code base at commit `0cb9466a75cf8bd8800aa1a80d3e628d3200d311`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableDirectoryDataAccess

## Description
Related open issues that do NOT cover this failure per the replay report: 20261110T232837Z-ds-the-directory-data-access-disable-re-enable.md carries this same check id (and itself names 20261107T061500Z-ds-data-access-cycle-check-cannot-fit-its-sixty-second-budget.md), and 20261111T085740Z-ds-the-ldaps-enable-disable-and-directory-data.md owns the sibling LDAPS / data-access toggle checks for the same root cause. The behavioral suite did NOT mark this failing check as OWNED by any of them in this replay, so it is re-filed here on its own replay evidence; all of these should be resolved as one work item.

Cause 1 of 1: the data-access disable/re-enable scenario reaches NO verdict. The emulator answers every call, but each of the four clock-owed transitions the scenario drives becomes observable only after the probe that immediately follows the call that starts it, so the client pays one full poll interval per transition and the harness kills the check at its 60 s budget before any of the scenario's surviving assertions run.

AWS: the statuses are modelled — Smithy com.amazonaws.directoryservice#DataAccessStatus = [Disabled, Disabling, Enabled, Enabling, Failed]; DescribeDirectoryDataAccess output {DataAccessStatus}; EnableDirectoryDataAccess and DisableDirectoryDataAccess both take only {DirectoryId @required, pattern ^d-[0-9a-f]{10}$}, return an empty result, and model DirectoryInDesiredStateException (400) and DirectoryUnavailableException (400), i.e. AWS does start a transition and answer before it settles. The ds-data gate the scenario asserts on is modelled too: com.amazonaws.directoryservicedata#AccessDeniedException @httpError 403 with member Reason -> AccessDeniedReason [IAM_AUTH, DIRECTORY_AUTH, DATA_DISABLED], a service-common error on every ds-data operation. For the WALL CLOCK there is recorded behavior only: on real AWS the CreateMicrosoftAD leg alone takes tens of minutes, so the recording is not what bounds the clock here — the 60 s per-check ceiling is the emulator's environment.

Emulator: no response divergence is reported at all; the replay's only output for this check is "Check timed out after 60.0s". The scenario drives four transitions and probes each one immediately after the call that starts it, observes the modelled transient value, then sleeps a fixed interval: CreateMicrosoftAD -> poll DescribeDirectories for a terminal Stage at 20 s; EnableDirectoryDataAccess -> poll DescribeDirectoryDataAccess for Enabled at 10 s; DisableDirectoryDataAccess -> poll for Disabled at 15 s; EnableDirectoryDataAccess -> poll for Enabled at 10 s. That is 55 s of unavoidable sleeping inside the 60 s ceiling, before the ds-data read-back traffic at the end. The emulator's own transition spans are two orders of magnitude shorter than the client's intervals, but they always come due AFTER the probe preceding them, so each still costs a whole interval. Sibling ds checks that drive only the create and one enable (30 s of the same sleeps) do reach their assertions in this same replay, which locates the overrun in the two extra toggle legs this scenario adds.

Why: a client gets no verdict on the contract this scenario exists to pin — that while data access is disabled every ds-data call is refused with HTTP 403, Error.Code 'AccessDeniedException' and top-level Reason 'DATA_DISABLED', carrying ONE identical message for a read (DescribeUser), a write (CreateUser) and an inventory listing (ListUsers), because the gate is on the directory and not on the operation; that DescribeDirectories keeps reporting Stage 'Active' while the data plane is shut; that a CreateUser refused while disabled leaves no object behind; and that the user, the group, the membership edge (both projections) and both SIDs survive a disable/re-enable cycle unchanged. None of that is exercised, so a regression in any of it passes unnoticed. In the refusal message, the fixed text is the sentence AWS words for the DATA_DISABLED class ("DS Data feature is not enabled on this directory. This feature must first be enabled by calling the EnableDirectoryDataAccess API."); the requirement is that it is IDENTICAL across read, write and listing for a given directory — no part of it is taken from the request, and no directory id, user name or timestamp from any single run belongs in it. This failure class also worsens as more of the surface is implemented, since every additional clock-owed leg costs another poll interval.

Fix: not an operation handler and not a serializer — nothing in DisableDirectoryDataAccess, EnableDirectoryDataAccess or DescribeDirectoryDataAccess answers wrongly. It belongs to whichever owns the project's clock-owed-transition contract as it meets the suite's per-check ceiling: either the transition span the ds state machine applies to the data-access toggles and to the directory-stage chain (so a settled status is observable on the client's FIRST probe, at the cost of Enabling/Disabling/Requested and the DirectoryInDesiredStateException / DirectoryUnavailableException paths that depend on them no longer being observable, and of loosening the in-tree suites that pin them), or the per-check budget / poll intervals the harness replays this scenario with.

Traces: ds.disable-directory-data-access-recloses-the-door-without-losing-contents (verdict mismatch, "Check timed out after 60.0s", killed in the check's _poll helper at its time.sleep(interval_s)). File and line citations for the state machine and the ds-data gate are in 20261110T232837Z-ds-the-directory-data-access-disable-re-enable.md.

## Evidence
Failing check (behavioral suite simparity-behavioral-aws-v1, listed under service heading '### ds'):

  [mismatch] ds.disable-directory-data-access-recloses-the-door-without-losing-contents
    Error: Check timed out after 60.0s

Expected (AWS / the check): the scenario runs to its assertions — DisableDirectoryDataAccess settles DataAccessStatus on 'Disabled'; ds-data DescribeUser, CreateUser and ListUsers against that directory are each refused with HTTP 403, Error.Code 'AccessDeniedException', top-level Reason 'DATA_DISABLED' and one IDENTICAL Message; ds DescribeDirectories keeps reporting Stage 'Active' throughout; EnableDirectoryDataAccess drives the status back to 'Enabled'; the pre-existing user and group return with the SAME SIDs (S-1-... minted once by AD, not re-derivable) and the same EmailAddress/Enabled attributes; the membership edge is reported by both ListGroupMembers and ListGroupsForMember; and the user whose CreateUser was refused while the plane was shut is ABSENT from the paginated ListUsers inventory.

Actual (emulator): no assertion is reached. The harness kills the check at 60 s inside a poll sleep. The replay reports no status, error-code, message or shape divergence for this check — only the timeout.

Why the ceiling cannot be met, from the check's own intervals (every poll probes immediately after the call that starts the transition, observes the modelled transient, and sleeps once):
  create_microsoft_ad            -> poll describe_directories for a terminal Stage, interval 20 s -> 20 s
  enable_directory_data_access   -> poll describe_directory_data_access for 'Enabled',  interval 10 s -> 10 s
  disable_directory_data_access  -> poll for 'Disabled',                                interval 15 s -> 15 s
  enable_directory_data_access   -> poll for 'Enabled' again,                           interval 10 s -> 10 s
                                                                                        total          55 s
plus roughly thirty further sts / ec2 / ds / ds-data requests, inside a 60 s ceiling.

Smithy consulted (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json):
  DataAccessStatus = [Disabled, Disabling, Enabled, Enabling, Failed]
  DisableDirectoryDataAccess: input {DirectoryId @required, pattern ^d-[0-9a-f]{10}$}, output {}; errors AccessDeniedException(400), ClientException(400), DirectoryDoesNotExistException(400), DirectoryInDesiredStateException(400), DirectoryUnavailableException(400), ServiceException(500), UnsupportedOperationException(400)
  EnableDirectoryDataAccess: identical signature and error set
  DescribeDirectoryDataAccess: output {DataAccessStatus}
and (services/service-com-amazonaws-directoryservicedata-directoryservicedata/): AccessDeniedException @httpError 403 with member Reason -> AccessDeniedReason [IAM_AUTH, DIRECTORY_AUTH, DATA_DISABLED], a service-common error on every ds-data operation.

Corroborating replay context: sibling ds checks that drive only the create plus one enable (30 s of the same fixed sleeps) reach their assertions in this same run and fail on response content rather than on the clock, so the overrun is attributable to the two extra toggle legs this scenario adds, not to slow request service.

## Steps to Reproduce
1. Boot the emulator and create botocore clients for 'sts', 'ec2', 'ds' and 'ds-data' against it.
2. Replay ds.disable-directory-data-access-recloses-the-door-without-losing-contents. It is killed at the 60 s budget inside a poll sleep, with no status/code/message divergence reported.
3. Reproduce the arithmetic by hand, timing each leg: create_microsoft_ad in a VPC with two subnets in distinct AZs, then poll describe_directories at 20 s intervals until Stage == 'Active' (the first probe observes a transient stage); enable_directory_data_access and poll describe_directory_data_access at 10 s until 'Enabled' (first probe 'Enabling'); create a ds-data user, group and membership; disable_directory_data_access and poll at 15 s until 'Disabled' (first probe 'Disabling'); enable_directory_data_access again and poll at 10 s until 'Enabled'. Four probes land on a transient and four intervals are slept: 55 s of the 60 s ceiling is gone before the read-back traffic.
4. Confirm the responses themselves are not the defect: with the polling replaced by a one-second sleep after each toggle, walk the same sequence and check each assertion directly — ds-data describe_user / create_user / list_users against the shut directory must each answer HTTP 403, Code 'AccessDeniedException', Reason 'DATA_DISABLED' with one identical Message; ds describe_directories must still report Stage 'Active'; after the re-enable, describe_user and describe_group must return the SAME SIDs recorded before the disable, list_group_members must report exactly the one user and list_groups_for_member the one group, and list_users must contain that user and NOT the user whose create was refused while the plane was shut.
5. Resolve at the level that owns it — either the clock-owed transition span the ds state applies to the data-access toggles and the directory-stage chain (so a settled status is observable on the client's FIRST probe, accepting that Enabling/Disabling/Requested and the DirectoryInDesiredStateException / DirectoryUnavailableException paths depending on them stop being observable, and loosening the in-tree suites that pin them), or this check's budget and poll intervals.
6. Re-replay ds.disable-directory-data-access-recloses-the-door-without-losing-contents and confirm it reaches a verdict on every assertion in step 4 rather than dying in a sleep. Resolve together with 20261110T232837Z-ds-the-directory-data-access-disable-re-enable.md, 20261107T061500Z-ds-data-access-cycle-check-cannot-fit-its-sixty-second-budget.md and 20261111T085740Z-ds-the-ldaps-enable-disable-and-directory-data.md, which carry the same or sibling check ids.
