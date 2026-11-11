Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.create-app-default-state amplify.create-app-round-trips-declared-values-and-omits-unset-fields amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401 amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses amplify.manual-deploy-build-and-serve-roundtrip amplify.redirect-rule-answers-301-with-the-target-location
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.create-app-default-state|run#15|AssertionError|assert re.fullmatch(r"[a-z0-9-]+(\.[a-z0-9-]+)+", app["defaultDomain"]), app["defaultDomain"]
Issue-Key: amplify.create-app-round-trips-declared-values-and-omits-unset-fields|run#7|AssertionError|assert plain["defaultDomain"].startswith(app_id + "."), (
Issue-Key: amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401|_http#8|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses|_http#7|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify.manual-deploy-build-and-serve-roundtrip|_poll_http_get#23|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify.redirect-rule-answers-301-with-the-target-location|_http#7|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: ccbb3cdef27cfb35ecd524974183c24766fa6572

# amplify: 2 net-new causes — App.defaultDomain is published as the authority 'localhost:<port>' instead of an appId-led hostname, and the branch hosting host built from it is not one of the emulator's own hosts

> _Note: this issue was identified against the code base at commit `ccbb3cdef27cfb35ecd524974183c24766fa6572`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApp

## Description
Two net-new root causes in the amplify service, both about the hosting address the app projection announces. They are independent defects — the first is the SHAPE of the announced value, the second is that the host composed from it is not served/announced as the emulator's own — and BOTH must be closed before this issue is submitted.

Follow-up context: the fixed issue 20261109T143214Z-amplify-2-net-new-causes-app-defaultdomain-is.md replaced the old `<appId>.amplifyapp.com` value with the emulator's loopback authority (services/service-com-amazonaws-amplify-amplify/src/state/hosting_address.rs, `announced_default_domain` -> `loopback_hosting_authority`). That fix changed the value but did not make the announced address usable: it is no longer a hostname at all (Cause 1), and the branch host built from it is still refused as not one of the emulator's own hosts (Cause 2). The open issue 20261109T143214Z-amplify-s3-every-s3-sourced-deployment-check-stops.md carries the same family of cause under the `amplify__s3` service heading only; the open issue 20261110T142528Z-amplify-3-net-new-causes-getjob-reports-a.md carries it from the WIRE corpus and describes the pre-fix `<appId>.amplifyapp.com` value. Neither carries these behavioral check ids, and neither describes the post-fix value.

Cause 1 of 2: CreateApp/GetApp report `App.defaultDomain` as the emulator's own authority `localhost:<port>` — a single-label host carrying a PORT, with no app-id label — where AWS reports a bare dotted DNS hostname whose first label is the app id.
  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models CreateApp -> POST /apps and GetApp -> GET /apps/{appId}, 200 with `app` @required -> App; `App$defaultDomain` targets `DefaultDomain` (string, @length 1..1000) and is documented "The default domain for the Amplify app." The model fixes no pattern, so the FORM is recorded behavior only: AWS returns a hostname, not an authority — several dot-separated DNS labels, no port and no colon, whose FIRST label is the app's own `appId` (recorded as `<appId>.amplifyapp.com`). The requirement is the form: a bare hostname (`[a-z0-9-]+(\.[a-z0-9-]+)+`, no port) that begins with `<appId>.`. The concrete zone/host and port are environment-specific and are never the requirement.
  Emulator: `defaultDomain` comes back as the authority of the port the emulator was reached on — e.g. `localhost:39279` — produced by src/state/hosting_address.rs `loopback_hosting_authority`, which formats `format!("{LOOPBACK_NAME}:{port}")` and is stored by src/ops/create_app.rs at the `default_domain` assignment. Replay: `AssertionError: localhost:39279` on `re.fullmatch(r"[a-z0-9-]+(\.[a-z0-9-]+)+", app["defaultDomain"])`, and `AssertionError: defaultDomain 'localhost:35329' does not start with the app id`.
  Why: a client treats `defaultDomain` as a DNS name it prefixes one more label to (`Branch$displayName` is documented "the default domain prefix"), and it also keys per-app hosting state on the app-id label. A value with an embedded `:port` is not a legal hostname — it cannot be put in a DNS record, a certificate SAN, a CNAME for a custom domain, or any `Host:` composition that then appends its own port — and a value that does not begin with the app id makes two apps on the same emulator announce the SAME domain, so one app's branch host collides with another's.
  Fix: the announced-domain composition in this crate's hosting-address state (src/state/hosting_address.rs, `announced_default_domain`/`loopback_hosting_authority`, reported through the shared app projection used by GetApp) must mint a bare, port-free hostname whose first label is the app id and which still resolves to this emulator, rather than carrying the endpoint authority verbatim.
  Traces: amplify.create-app-default-state, amplify.create-app-round-trips-declared-values-and-omits-unset-fields.

Cause 2 of 2: the branch hosting host composed from the announced domain (`<branch displayName>.<defaultDomain>`, i.e. `<displayName>.localhost:<port>`) is not one of the hosts the emulator announces as its own, so every hosting request a client makes from the API's own output never reaches the emulator and the whole branch data plane is unobservable.
  AWS: `App$defaultDomain` plus `Branch$displayName` (@required, "The display name for the branch. This is used as the default domain prefix.") are the two values a client composes the live site address from: `https://<displayName>.<defaultDomain>/`. On AWS that address is answered by Amplify's hosting edge — it returns the bytes the deployment published (200), 404 for a path the deployment does not contain, `301` with `Location` set to the target of a custom rule, the configured custom headers on a matching path, and a `401` with a `WWW-Authenticate: Basic ...` challenge while branch basic auth is enabled. The requirement is that the announced address be reachable by the client that was handed it; the host label and port are per-run values.
  Emulator: the address published for the branch is refused by the client before any request is sent: `simparity.published.PublishedUrlRejected: endpoint_url was given https://main.localhost:36859/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:36859/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. (Identical refusals for `https://main.localhost:41879/`, `https://main.localhost:35383/` and `https://main3e8e38e8af12.localhost:33005/` — the labels and ports are per-run.) The crate's edge only compares a received `Host` against `hosted_branch_host` (src/state/hosting_address.rs:127) and never announces `<displayName>.<defaultDomain>` as a host it serves on, so the branch host is not registered on a per-resource listener.
  Why: fetching the branch URL is the only way a client confirms what a deployment published and the only way it observes branch basic auth, custom headers and redirect rules. Against the emulator the fetch leaves the emulator entirely, so a successful SUCCEED job is unverifiable, basic auth appears unenforced, configured custom headers appear unserved, and a 301 rule appears to never apply.
  Fix: the hosting edge's addressing in this crate (src/state/hosting_address.rs together with the hosting edge in src/service.rs that claims branch hosts) must announce every branch host it serves as one of the emulator's own hosts — register `<displayName>.<defaultDomain>` on a per-resource listener so that `https://<displayName>.<defaultDomain>/<path>` is routed to the emulator that published it — and the announced `defaultDomain` must be the host of that listener.
  Traces: amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401, amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses, amplify.manual-deploy-build-and-serve-roundtrip, amplify.redirect-rule-answers-301-with-the-target-location.

## Evidence
Cause 1 — defaultDomain is an authority, not an appId-led hostname (both checks read it back with GetApp, HTTP 200):
  [mismatch] amplify.create-app-default-state — `assert re.fullmatch(r"[a-z0-9-]+(\.[a-z0-9-]+)+", app["defaultDomain"])` -> `AssertionError: localhost:39279`. Expected: a bare dotted hostname with no port.
  [mismatch] amplify.create-app-round-trips-declared-values-and-omits-unset-fields — `assert plain["defaultDomain"].startswith(app_id + ".")` -> `AssertionError: defaultDomain 'localhost:35329' does not start with the app id`. Expected: the first label is the appId GetApp reports.
  (The ports 39279/35329 are per-run values and are not part of the requirement.)
  Source: services/service-com-amazonaws-amplify-amplify/src/state/hosting_address.rs:78-94 — `loopback_hosting_authority` returns `format!("{LOOPBACK_NAME}:{port}")` with `LOOPBACK_NAME = "localhost"`; stored by src/ops/create_app.rs:295-299 (`default_domain: crate::state::announced_default_domain(...)`).

Cause 2 — the composed branch host is not one of the emulator's own hosts. All four checks build the URL exactly as a client would (`domain = get_app(appId)['app']['defaultDomain']; display = get_branch(appId, branch)['branch']['displayName']; url = f"https://{display}.{domain}/"`) and are refused before the request:
  [mismatch] amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401 — `PublishedUrlRejected: endpoint_url was given https://main.localhost:36859/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:36859/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`
  [mismatch] amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses — same refusal, `https://main.localhost:41879/`
  [mismatch] amplify.redirect-rule-answers-301-with-the-target-location — same refusal, `https://main.localhost:35383/`
  [mismatch] amplify.manual-deploy-build-and-serve-roundtrip — same refusal, `https://main3e8e38e8af12.localhost:33005/`
  Expected once the address is the emulator's own: GET of the branch URL after a SUCCEED deployment returns the published bytes (200); an unpublished `.css` path returns 404; with a `{source:/legacy.css, target:/styles/main.css, status:301}` rule the same path returns 301 with `Location: /styles/main.css` and no body; with `enableBasicAuth` on, an anonymous or wrong-password GET returns 401 carrying `WWW-Authenticate: Basic ...` and the correct credentials return the page; a configured custom header is present on a matching response.
  Source: src/state/hosting_address.rs:127-141 (`hosted_branch_host`/`hosted_site_host` compare an INBOUND Host only; nothing announces that host as the emulator's own).

## Steps to Reproduce
Cause 1 (defaultDomain form):
1. r = amplify.create_app(name=<unique>); app_id = r['app']['appId'].
2. app = amplify.get_app(appId=app_id)['app']; read app['defaultDomain'].
   Observed: 'localhost:<port>' — one label plus a port, and no app-id label.
   Expected: a bare dotted DNS hostname with no port (matching `[a-z0-9-]+(\.[a-z0-9-]+)+`) whose first label is app_id, e.g. `<appId>.<hosting zone this emulator serves>`.

Cause 2 (branch hosting host not the emulator's own):
1. app_id = amplify.create_app(name=<unique>)['app']['appId']; amplify.create_branch(appId=app_id, branchName='main').
2. dep = amplify.create_deployment(appId=app_id, branchName='main'); PUT a zip containing index.html to dep['zipUploadUrl']; amplify.start_deployment(appId=app_id, branchName='main', jobId=dep['jobId']); poll amplify.get_job until summary.status == 'SUCCEED'.
3. domain = amplify.get_app(appId=app_id)['app']['defaultDomain']; display = amplify.get_branch(appId=app_id, branchName='main')['branch']['displayName']; GET https://{display}.{domain}/ .
   Observed: the published host `<display>.localhost:<port>` is not one of the emulator's own hosts, so the GET never reaches the emulator.
   Expected: the emulator answers that address — 200 with the deployed index.html bytes; 404 for a path the deployment does not contain; after update_app(customRules=[{'source':'/legacy.css','target':'/styles/main.css','status':'301'}]) a GET of /legacy.css answers 301 with `Location: /styles/main.css`; after update_branch(enableBasicAuth=True, basicAuthCredentials=<base64 user:pass>) an anonymous GET answers 401 with a `WWW-Authenticate: Basic` challenge and the right credentials answer 200; after update_app(customHeaders=<YAML>) the configured header is present on a matching response.

Replay: run_behavioral_suite and re-run the checks listed in trace_ids.
