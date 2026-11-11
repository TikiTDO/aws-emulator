Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401 amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses amplify.manual-deploy-build-and-serve-roundtrip amplify.redirect-rule-answers-301-with-the-target-location amplify.stopjob-completed-job-rejected
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401|_http#8|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses|_http#7|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify.manual-deploy-build-and-serve-roundtrip|_poll_http_get#23|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify.redirect-rule-answers-301-with-the-target-location|_http#7|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify.stopjob-completed-job-rejected|run#13|AssertionError|assert err == refused, err
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# amplify: 2 net-new causes — App.defaultDomain is published as an unreachable <appId>.amplifyapp.com host, and StopJob's terminal-state refusal is emulator-worded

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApp

## Description
Two net-new root causes in the amplify service, each grounded in failing behavioral checks.

Cause 1 of 2: every app projection publishes `App.defaultDomain` as a real-AWS `<appId>.amplifyapp.com` hostname, which no customer of the emulator can reach, so no branch-hosting (data-plane) behavior can be exercised at all.

  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models GetApp -> GET /apps/{appId}, 200 with `app` @required; `App$defaultDomain` is documented "The default domain for the Amplify app." On real AWS that domain is the host Amplify's hosting edge actually answers on, and the published branch URL `https://<branch displayName>.<app defaultDomain>/` is a live address: a GET of it returns the deployed page, the branch's basic-auth challenge, or the configured redirect. The requirement is that the member name an address the client that received it can reach — the host/port itself is environment-specific and is never the requirement.

  Emulator: services/service-com-amazonaws-amplify-amplify/src/ops/create_app.rs sets `default_domain: format!("{app_id}.amplifyapp.com")`, so GetApp/CreateApp/ListApps report a hostname on AWS's production `amplifyapp.com` zone. The replay harness refuses the published address: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<branch>.<appId>.amplifyapp.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The checks build the URL exactly as a client would, from GetApp.app.defaultDomain plus GetBranch.branch.displayName, so the defect is the announced address, not the check.

  Why: a client takes `defaultDomain` as the app's live hosting address — it is how an Amplify consumer fetches the deployed site, follows a redirect/rewrite rule, or is challenged by branch basic auth. Against the emulator that address resolves off the emulator entirely (to AWS's real zone, or to nothing), so every hosting-dependent flow either fails to connect or silently talks to something that is not the emulator. All four amplify data-plane checks below stop at this point and never reach the behavior they test (basic-auth 401 + challenge at the edge, customHeaders served on a matching path, manual-deploy serve round-trip, 301 redirect with Location).

  Fix: the app record's `defaultDomain` must be minted from the emulator's own published hosting address instead of a synthesized AWS zone — in the app's creation/projection path (services/service-com-amazonaws-amplify-amplify/src/ops/create_app.rs around the `default_domain` assignment, and whatever shared projection in src/state.rs reports it) — and the emulator's hosting edge must be reachable at that same announced host so a GET of `https://<displayName>.<defaultDomain>/` is answered by the emulator that published it.

  Traces: amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401, amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses, amplify.manual-deploy-build-and-serve-roundtrip, amplify.redirect-rule-answers-301-with-the-target-location.

Cause 2 of 2: StopJob on an already-terminal job is refused with the right status and code but an emulator-authored message that interpolates the job id.

  AWS: Smithy models StopJob -> DELETE /apps/{appId}/branches/{branchName}/jobs/{jobId}/stop, 200 with `jobSummary` @required; modeled errors BadRequestException (400), NotFoundException (404), UnauthorizedException (401), LimitExceededException (429), InternalFailureException (500). The model does not state message text, so the message is recorded behavior only: stopping a job already in a terminal status (SUCCEED / FAILED / CANCELLED) is refused with HTTP 400, code `BadRequestException`, message `Unable to cancel job. The given job is already in a terminal state.` That message is ENTIRELY fixed text — no part of it comes from the request or from state (it names neither jobId, branch, nor status).

  Emulator: HTTP 400 and code `BadRequestException` agree, but the message is `Job 1 is not in progress.` — fixed text `Job ` + the request's `jobId` + ` is not in progress.` (the `1` is that run's generated job id and is not itself the defect). Produced by the StopJob handler's terminal-state branch in services/service-com-amazonaws-amplify-amplify/src/ops/stop_job.rs via the message helper in src/state.rs.

  Why: BadRequestException is an untyped catch-all Amplify reuses for every 400 on this surface, so the MESSAGE is a client's only discriminator. A client branching on "is already in a terminal state" (to treat the cancel as a no-op or stop polling) falls through to generic error handling, and the emulator additionally teaches callers that this message carries the job id, which AWS's does not.

  Fix: the StopJob handler's terminal-state refusal path must emit AWS's fixed literal for this error class rather than a per-job template; if the helper is shared with an operation whose AWS message really is job-scoped, this call site needs its own literal.

  Note on existing issues: the open issue 20261030T053217Z-amplify-stopjob-refuses-an-already-terminal-job-with.md describes this same StopJob wording but the replay report did NOT mark this check as owned by it (its recorded replay key has drifted — the assertion is now at check line 57), so it is re-filed here with the current key rather than left uncovered. Cause 1 is not carried by any open amplify issue (the open amplify set covers control-plane projections, job lifecycle, CFN resource types and two scaffold stubs; none touches `defaultDomain`).

  Traces: amplify.stopjob-completed-job-rejected.

## Evidence
Cause 1 — four failing amplify checks, identical refusal:
  [mismatch] amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401
    `simparity.published.PublishedUrlRejected: endpoint_url was given https://main.d2ovcsh8hthens.amplifyapp.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:35325/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`
  [mismatch] amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses — same, `https://main.d1wvydp717cru8.amplifyapp.com/`
  [mismatch] amplify.manual-deploy-build-and-serve-roundtrip — same, `https://main6413cfd3e108.d1tw910ddofvom.amplifyapp.com/`
  [mismatch] amplify.redirect-rule-answers-301-with-the-target-location — same, `https://main.d32zhywme1evku.amplifyapp.com/`
(The host labels and ports above are per-run values; the requirement is only that the announced host be one the emulator serves.)
The URL is assembled by the checks' own helper exactly as a client would: `domain = get_app(appId)['app']['defaultDomain']; display = get_branch(appId, branch)['branch']['displayName']; url = f"https://{display}.{domain}/"`.
Source: services/service-com-amazonaws-amplify-amplify/src/ops/create_app.rs — `default_domain: format!("{app_id}.amplifyapp.com")`.

Cause 2 — [mismatch] amplify.stopjob-completed-job-rejected
  Suite detail: `AssertionError: ('BadRequestException', 'Job 1 is not in progress.')` at check_amplify_stopjob_completed_job_rejected.py:57 (`assert err == refused`).
  Expected: HTTP 400, Error.Code `BadRequestException`, Error.Message `Unable to cancel job. The given job is already in a terminal state.` (all fixed text).
  Actual: HTTP 400, Error.Code `BadRequestException`, Error.Message `Job <jobId> is not in progress.` (fixed text plus the request's jobId).
  The positive controls in the same check pass: the in-flight StopJob returned a jobSummary in (CANCELLING, CANCELLED) and the job settled terminal, so the divergence is isolated to the refusal text.

## Steps to Reproduce
Cause 1 (defaultDomain unreachable):
1. amplify.create_app(name=<unique>) -> appId; amplify.create_branch(appId=appId, branchName='main').
2. d = amplify.create_deployment(appId, 'main'); HTTP PUT a zip containing index.html to d['zipUploadUrl']; amplify.start_deployment(appId, 'main', jobId=d['jobId']); poll get_job until SUCCEED.
3. domain = amplify.get_app(appId=appId)['app']['defaultDomain'] -> observed `<appId>.amplifyapp.com`, a host on AWS's production zone that the emulator does not serve.
4. display = amplify.get_branch(appId, 'main')['branch']['displayName']; GET https://{display}.{domain}/ .
   Observed: the address is not one of the emulator's own hosts (connection goes off-emulator / is refused by the harness).
   Expected: defaultDomain names a host the emulator itself serves, and the GET returns the deployed index.html (200), the branch's 401 + `WWW-Authenticate: Basic ...` challenge when basic auth is enabled, the configured custom header on a matching path, or 301 + Location for a redirect rule.

Cause 2 (StopJob terminal refusal message):
1. create_app -> appId; create_branch(appId, <branch>).
2. d = create_deployment(appId, branch); PUT a site zip to d['zipUploadUrl']; start_deployment(appId, branch, jobId=d['jobId']).
3. stop_job(appId, branch, jobId) -> 200 jobSummary.status in (CANCELLING, CANCELLED); poll get_job to CANCELLED.
4. stop_job(appId, branch, jobId) AGAIN, now terminal.
   Observed: 400 BadRequestException 'Job <jobId> is not in progress.'
   Expected: 400 BadRequestException 'Unable to cancel job. The given job is already in a terminal state.'
5. get_job -> status unchanged (the refused call changes nothing).

Replay: run_behavioral_suite and re-run the checks listed in trace_ids.
