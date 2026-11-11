Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.post-agent-profile-data-aggregates-into-retrievable-profile
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.post-agent-profile-data-aggregates-into-retrievable-profile|_poll_until_aggregated#20|Timeout|time.sleep(interval_s)
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# codeguruprofiler: GetProfile never makes a posted agent profile retrievable, so the aggregated-profile poll never completes

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProfile

## Description
One net-new root cause in the codeguruprofiler emulator, grounded in a failing behavioral check of the simparity-behavioral-aws-v1 replay.

Already-filed work that does NOT cover this cause: the open issue 20261109T163828Z-codeguruprofiler-2-net-new-causes-the-aggregated-profile.md concerns WHAT the aggregated document contains once GetProfile serves one (epoch-millisecond `start`/`end` instead of ISO-8601, missing aggregatedMetadata) and PostAgentProfile's not-found wording. At the commit replayed here GetProfile no longer serves a document at all for the posted window, so the check never reaches the assertion that issue's replay key records (`run#22|AssertionError|assert profile["start"] == expected_start`) and instead exhausts the check budget with no response. The open issue 20261029T105339Z-codeguruprofiler-listprofiletimes-wrong-behavior--r2.md concerns which windows ListProfileTimes announces, not whether GetProfile answers.

Cause 1 of 1: GetProfile does not make the aggregated profile for a window a PostAgentProfile populated retrievable, and does not complete within the check's budget
AWS: Smithy GetProfile — GET /profilingGroups/{profilingGroupName}/profile, success 200 with the required `profile` (@httpPayload blob, target AggregatedProfile) and the required Content-Type header; `period` and `accept` are the httpQuery/httpHeader inputs the check uses; modeled errors are ValidationException (400), ResourceNotFoundException (404), ThrottlingException (429), InternalServerException (500). The AggregatedProfile blob has no modeled schema, so the retrievability behavior is recorded behavior only: after PostAgentProfile(contentType=application/json) submits a document for a CLOSED, UTC-aligned 5-minute window, CodeGuru Profiler finishes aggregating that submission and GetProfile(period='PT5M', accept='application/json') then answers 200 with the aggregated document for that window; until aggregation completes it answers 404 ResourceNotFoundException, which is why the recorded check polls GetProfile and tolerates only that code. The check's own poll budget is 900 s at a 20 s interval, so against AWS the 200 arrives well inside the harness's per-check limit.
Emulator: GetProfile never answers 200 for the posted window. The check — CreateProfilingGroup, ConfigureAgent, PostAgentProfile for the last closed PT5M window, DescribeProfilingGroup (latestAgentProfileReportedAt present), then the GetProfile poll — reaches the harness limit and is reported as `Check timed out after 60.0s`, so no assertion on the document was reached at all. Hypothesis (the replay records only the timeout): either GetProfile keeps answering 404 ResourceNotFoundException for a window PostAgentProfile populated, so the poll spins forever, or a request on that path does not return a response. Either way the emulator produced no 200 for a window its own DescribeProfilingGroup already reports an agent profile for.
Why: the profile payload is the entire point of the data plane. A client that posts agent data and then polls GetProfile for the aggregated window — every agent-side round-trip, flame-graph renderer, console format and regression gate — loops until its own budget expires and never obtains the bytes it is waiting for, and cannot distinguish 'still aggregating' from 'will never aggregate'. A call that does not return at all additionally hangs the caller rather than failing with a modeled error.
Fix: GetProfile's handler and the aggregation state behind PostAgentProfile — a submission accepted for an aligned window must make that window's aggregated profile retrievable by GetProfile for the matching `period`, and GetProfile must always terminate with either the 200 payload or one of its modeled errors rather than stalling. Any file/line citation belongs under Traces.
Traces: codeguruprofiler.post-agent-profile-data-aggregates-into-retrievable-profile.

## Evidence
Cause 1 — GetProfile after a PostAgentProfile for the last closed UTC-aligned PT5M window:
  expected: the poll's GetProfile(profilingGroupName=N, period='PT5M', accept='application/json') answers 200 with the aggregated document for that window (its `start`/`end` the window bounds, aggregatedMetadata.numReportedAgents == 1), well inside the per-check budget
  actual:   the check is reported `[mismatch] codeguruprofiler.post-agent-profile-data-aggregates-into-retrievable-profile — Error: Check timed out after 60.0s`; the only loop in the check is _poll_until_aggregated, which calls GetProfile every 20 s and swallows ONLY ResourceNotFoundException, so within the budget GetProfile either answered 404 ResourceNotFoundException on every attempt or did not return
  note: PostAgentProfile itself succeeded and DescribeProfilingGroup's profilingStatus.latestAgentProfileReportedAt was populated (those assertions precede the poll and did not fail), so the emulator had accepted and recorded the submission
  note: at the commit of the earlier open issue the same check failed instead at `profile['start']`, i.e. GetProfile did answer 200 there; the retrievability failure recorded here is a different behavior and a different replay key

## Steps to Reproduce
1. Boot the emulator and point a botocore codeguruprofiler client at it (the replay ran with account 000000000000).
2. CreateProfilingGroup(profilingGroupName=N, agentOrchestrationConfig={'profilingEnabled': True}).
3. ConfigureAgent(profilingGroupName=N, fleetInstanceId=F) — shouldProfile is True.
4. Compute the last CLOSED 5-minute window [S,E) aligned to UTC and PostAgentProfile(profilingGroupName=N, agentProfile=json.dumps({'start': S_ms, 'end': E_ms, 'callgraph': {'frameId':'ALL','runnableCount':1000,'children':{}}}).encode(), contentType='application/json').
5. DescribeProfilingGroup(profilingGroupName=N) — profilingStatus.latestAgentProfileReportedAt is set.
6. Poll GetProfile(profilingGroupName=N, period='PT5M', accept='application/json'), treating only ResourceNotFoundException as 'not yet aggregated': no attempt ever returns 200 (or a call does not return), so the poll never ends and the check is killed at its 60 s limit.
7. Or replay the check listed in trace_ids.
