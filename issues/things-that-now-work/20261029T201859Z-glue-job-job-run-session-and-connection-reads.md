Issue-Source: trace-sweep-divergence
Issue-Trace: glue-job-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-job-lifecycle glue-job-lifecycle-with-script glue-job-trigger-lifecycle glue-job-duplicate-create-error glue-job-run-arguments-propagated glue-job-update-reflected glue-session-command-args-round-trip glue-session-description-and-tags-round-trip glue-jdbc-connection-management
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetJob|2xx->2xx|$.Job.JobRunQueuingEnabled
Issue-Key: glue|GetJob|2xx->2xx|$.Job.NumberOfWorkers
Issue-Key: glue|GetJob|2xx->2xx|$.Job.WorkerType
Issue-Key: glue|GetJobRun|2xx->2xx|$.JobRun.ExecutionArguments
Issue-Key: glue|GetJobRun|2xx->2xx|$.JobRun.GlueVersion
Issue-Key: glue|GetJobRun|2xx->2xx|$.JobRun.JobRunQueuingEnabled
Issue-Key: glue|GetJobRun|2xx->2xx|$.JobRun.LogGroupName
Issue-Key: glue|GetJobRun|2xx->2xx|$.JobRun.PredecessorRuns
Issue-Key: glue|GetJob|2xx->2xx|$.Job.GlueVersion
Issue-Key: glue|GetSession|2xx->2xx|$.Session.MaxCapacity
Issue-Key: glue|GetSession|2xx->2xx|$.Session.NumberOfWorkers
Issue-Key: glue|GetSession|2xx->2xx|$.Session.SessionType
Issue-Key: glue|GetSession|2xx->2xx|$.Session.Timeout
Issue-Key: glue|GetSession|2xx->2xx|$.Session.WorkerType
Issue-Key: glue|GetSession|2xx->2xx|$.Session.DPUSeconds
Issue-Key: glue|GetSession|2xx->2xx|$.Session.DefaultArguments
Issue-Key: glue|GetSession|2xx->2xx|$.Session.ExecutionTime
Issue-Key: glue|GetConnection|2xx->2xx|$.Connection.LastUpdatedBy
Issue-Key: glue|GetConnections|2xx->2xx|$.ConnectionList[Name="sim2real-probe-glue-sim2real-probe-conn-08fe166e"].LastUpdatedBy
Issue-Key: glue|GetConnections|2xx->2xx|$.ConnectionList[Name="sim2real-probe-glue-sim2real-probe-conn2-08fe166e"].LastUpdatedBy
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue job, job-run, session and connection reads drop modeled members

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
This issue bundles 8 net-new causes across the job, job-run, session and connection reads. Shared fault: the emulator only round-trips the members it happens to store, so modeled output members that AWS always populates (either because the caller supplied them or because AWS defaults them) are absent, an UpdateJob field is not carried into the read, and the connection's caller identity is rendered in the wrong form.

Cause 1 of 8: GetJob omits Job.JobRunQueuingEnabled.
AWS: Smithy: Job.JobRunQueuingEnabled (NullableBoolean) — 'A value of true means job run queuing is enabled... If false or not populated, the job run...'. When CreateJob supplied JobRunQueuingEnabled, AWS echoes it (false when the caller sent false).
Emulator: the member is absent from Job.
Why: a client reading the flag to decide whether to queue a run sees it unset and cannot distinguish 'disabled' from 'not supported'.
Fix: GetJob serializer + job state (persist and emit the flag as submitted).
Traces: glue-job-lifecycle#task#3 (also glue-job-lifecycle-with-script#task#4, glue-job-trigger-lifecycle#task#3).

Cause 2 of 8: GetJob omits WorkerType, NumberOfWorkers and JobRunQueuingEnabled.
AWS: Smithy: Job.WorkerType (enum), Job.NumberOfWorkers (NullableInteger), Job.JobRunQueuingEnabled — all echoed as supplied to CreateJob.
Emulator: all three absent.
Why: the job definition a client reads back does not describe the capacity it asked for, so sizing logic and drift detection break.
Fix: GetJob serializer + CreateJob state persistence of the capacity members.
Traces: glue-job-duplicate-create-error#task#4.

Cause 3 of 8: GetJobRun omits run members and reports the wrong GlueVersion.
AWS: a job run echoes the arguments it was started with and AWS's run bookkeeping: JobRun.ExecutionArguments (the --key arguments passed to StartJobRun), JobRun.LogGroupName (the job's log group), JobRun.PredecessorRuns (empty list when there are none), JobRun.JobRunQueuingEnabled, and JobRun.GlueVersion equal to the job definition's GlueVersion.
Emulator: ExecutionArguments, LogGroupName, PredecessorRuns and JobRunQueuingEnabled absent, and GlueVersion is the emulator's default rather than the job's configured version.
Why: a client that reads back the arguments or log group of a run (the standard way to confirm what ran) gets nothing, and version-dependent behavior is mis-reported.
Fix: GetJobRun serializer + job-run state (carry the start arguments and the job's GlueVersion onto the run).
Traces: glue-job-run-arguments-propagated#task#5.

Cause 4 of 8: GetJob after UpdateJob reports the pre-update GlueVersion.
AWS: Smithy: Job.GlueVersion (GlueVersionString, pattern '^(\\w+\\.)+\\w+$'); its doc states jobs created without a Glue version default to 5.1 — but after UpdateJob sets a version, the read returns the version that was set.
Emulator: returns the default version, i.e. the update's GlueVersion is dropped; JobRunQueuingEnabled is also absent.
Why: a client upgrading a job's Glue version sees the upgrade silently discarded.
Fix: UpdateJob handler/state (apply GlueVersion from JobUpdate) and the GetJob serializer.
Traces: glue-job-update-reflected#task#4.

Cause 5 of 8: GetSession omits the session's capacity and type members.
AWS: Smithy: Session.MaxCapacity (NullableDouble), Session.NumberOfWorkers, Session.WorkerType (enum), Session.SessionType (enum), Session.Timeout — AWS returns the values the session was created with (and SessionType for the session's kind).
Emulator: all five absent.
Why: a client inspecting a session's shape before submitting statements cannot.
Fix: GetSession serializer + session state.
Traces: glue-session-command-args-round-trip#task#4.

Cause 6 of 8: GetSession omits execution accounting and defaults.
AWS: Smithy: Session.DPUSeconds and Session.ExecutionTime (NullableDouble; 0.0 for a session that has not executed), Session.DefaultArguments (OrchestrationArgumentsMap; the empty map when none were given), Session.SessionType, Session.Timeout.
Emulator: all five absent.
Why: cost/usage reporting over sessions reads nothing, and a client cannot distinguish 'no arguments' from 'unsupported'.
Fix: GetSession serializer + session state.
Traces: glue-session-description-and-tags-round-trip#task#4.

Cause 7 of 8: GetConnection renders LastUpdatedBy as a full IAM ARN.
AWS: Connection.LastUpdatedBy is the caller identity in its short relative form 'user/<userName>' — the ARN's resource part only, not the full 'arn:aws:iam::<accountId>:user/<userName>'.
Emulator: returns the full IAM ARN.
Why: a client that displays or matches the updater identity against AWS's short form does not match.
Fix: CreateConnection/UpdateConnection state capture and the GetConnection serializer (store the relative identity).
Traces: glue-jdbc-connection-management#task#5 (also task#8).

Cause 8 of 8: GetConnections renders LastUpdatedBy the same way for every entry.
AWS: as cause 7, per ConnectionList entry.
Emulator: full IAM ARN on each entry.
Why: as cause 7, across listings.
Fix: same identity rendering in the GetConnections serializer.
Traces: glue-jdbc-connection-management#task#6.

## Evidence
Replay of simparity-wire-aws-v1, all steps 200/200:
1. GetJob: $.Job.JobRunQueuingEnabled missing (expected False, got None).
2. GetJob: $.Job.JobRunQueuingEnabled missing (expected False), $.Job.NumberOfWorkers missing (expected 2), $.Job.WorkerType missing (expected 'G.1X').
3. GetJobRun: $.JobRun.ExecutionArguments missing (expected {'--my-custom-arg': 'hello-world'}), $.JobRun.LogGroupName missing (expected '/aws-glue/python-jobs'), $.JobRun.PredecessorRuns missing (expected []), $.JobRun.JobRunQueuingEnabled missing (expected False), $.JobRun.GlueVersion expected '6.0' got '5.1'.
4. GetJob: $.Job.GlueVersion expected '6.0' got '5.1'; $.Job.JobRunQueuingEnabled missing.
5. GetSession: $.Session.MaxCapacity (expected 2.0), $.Session.NumberOfWorkers (expected 2), $.Session.SessionType (expected 'LIVY'), $.Session.Timeout (expected 5), $.Session.WorkerType (expected 'G.1X') all missing.
6. GetSession: $.Session.DPUSeconds (expected 0.0), $.Session.DefaultArguments (expected {}), $.Session.ExecutionTime (expected 0.0), $.Session.SessionType (expected 'LIVY'), $.Session.Timeout (expected 1) all missing.
7. GetConnection: $.Connection.LastUpdatedBy expected 'user/emulator' got 'arn:aws:iam::000000000000:user/emulator'.
8. GetConnections: the same divergence on each ConnectionList entry.
(Worker types, versions, timeouts and the user name above are that run's request values.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-job-lifecycle#task#3, glue-job-duplicate-create-error#task#4, glue-job-run-arguments-propagated#task#5, glue-job-update-reflected#task#4, glue-session-command-args-round-trip#task#4, glue-session-description-and-tags-round-trip#task#4, glue-jdbc-connection-management#task#5, glue-jdbc-connection-management#task#6.
3. Each fixture creates (or updates) the job/session/connection, then reads it back and asserts the response members listed per cause.
