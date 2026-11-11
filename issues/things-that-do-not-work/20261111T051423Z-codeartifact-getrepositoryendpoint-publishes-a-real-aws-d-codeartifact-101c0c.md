Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.authorization-token-minimum-duration-expiration-and-use
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.authorization-token-minimum-duration-expiration-and-use|_routed_request#19|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# codeartifact: GetRepositoryEndpoint publishes a real-AWS d.codeartifact authority the emulator does not serve, so a 900 s authorization token can never be proven against the npm /-/ping?write=true route

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryEndpoint

## Description
Cause 1 of 1: GetRepositoryEndpoint answers 200 with a repositoryEndpoint whose authority is an AWS production hostname rather than the address the emulator is reached on, so the minimum-duration authorization-token check leaves the emulator at its first data-plane request and the token's usability can never be observed. Failing check: codeartifact.authorization-token-minimum-duration-expiration-and-use.

NOTE ON RELATED FILED WORK: the open issue 20261109T164005Z-codeartifact-getrepositoryendpoint-publishes-a-real-aws-d-codeartifact.md describes the same announced-host defect, and 20261109T130849Z-codeartifact-getrepositoryendpoint-announces-a-real-aws-d-codeartifact.md describes it from the wire replay. Neither carries this check's replay key — the suite reported this check as NOT owned (every other codeartifact failure in this run was marked OWNED, this one was not) — so it is filed here to keep its replay key attached to a work item. A fixer closing the announced-authority defect should re-replay this check too.

AWS: Smithy (com.amazonaws.codeartifact#CodeArtifactControlPlaneService) GetRepositoryEndpoint — GET /v1/repository/endpoint, success 200; required httpQuery inputs domain (DomainName, length 2–50, pattern ^[a-z][a-z0-9\-]{0,48}[a-z0-9]$), repository (RepositoryName, length 2–100, pattern ^[A-Za-z0-9][A-Za-z0-9._\-]{1,99}$), format (PackageFormat enum); optional domain-owner (AccountId, ^[0-9]{12}$) and endpointType (EndpointType enum); output GetRepositoryEndpointResult$repositoryEndpoint (String), documented as the URL of the returned endpoint. Modeled errors: ValidationException 400, AccessDeniedException 403, ResourceNotFoundException 404, ThrottlingException 429, InternalServerException 500. The returned URL is the address a package manager publishes to and installs from, and on AWS its authority is served by the responder. The requirement is not a literal string: the path shape /<format>/<repository>/ and its variable parts (domain, domain owner, region, format, repository) come from the request and state; the invariant is that the announced authority is one the responder actually serves. GetAuthorizationToken — POST /v1/authorization-token, 200, httpQuery duration (AuthorizationTokenDurationSeconds, @range min 0 max 43200), output authorizationToken + expiration (sensitive). Recorded behavior only, per AWS's tokens-authentication documentation, for the rest of the check: durationSeconds=899 is refused with ValidationException and the message "Invalid durationSeconds value of <durationSeconds>: value must be 0 or between 900 and 43200 (inclusive)." — fixed text except <durationSeconds>, which is the rejected request value; durationSeconds=900 returns an expiration 900 s after issuance; and the issued token authenticates GET <repositoryEndpoint>/-/ping?write=true with 200.

Emulator: GetRepositoryEndpoint returns 200 with repositoryEndpoint on the AWS production authority, of the form https://<domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com/<format>/<repository>/ . The check's first use of it is refused before any request is issued: "simparity.published.PublishedUrlRejected: endpoint_url was given https://<domain>-<account>.d.codeartifact.<region>.amazonaws.com/npm/<repo>/-/ping?write=true (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The status (200) and the path shape are correct; the authority is the only divergence. The host, port, domain/repository names and account id in that text are per-run values and are not part of the requirement.

Why: a client following the documented flow — GetAuthorizationToken, then point npm/pip at repositoryEndpoint — is sent to a name that either does not resolve or resolves to real AWS and answers an unrelated credential failure, surfacing as a network or signature error it cannot act on. For this check specifically, the 900 s minimum-duration token's whole contract (that it authenticates the repository's npm /-/ping?write=true route with 200 while valid) is unverifiable, and so is the npm write-authorization route behind it.

Fix: the GetRepositoryEndpoint handler's response construction in the codeartifact service crate (its endpoint/URL synthesis), which must emit an authority the emulator serves — the host and port the request arrived on, or the per-resource listener it stands up — while preserving the modeled /<format>/<repository>/ path shape and honouring endpointType; plus the repository data-plane routing that must answer the npm /-/ping?write=true route on that authority with 200 for a valid bearer token.

Traces: codeartifact.authorization-token-minimum-duration-expiration-and-use, which reaches the ValidationException and expiration assertions and then stops at its first use of the published endpoint.

## Evidence
[mismatch] codeartifact.authorization-token-minimum-duration-expiration-and-use (under ### codeartifact)
  File "context.py", line 603, in endpoint_route -> return self._published_target(url, "endpoint_url"), {}
  File "context.py", line 617, in _published_target -> raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: endpoint_url was given https://sim2real-behav-ca-5782e560f1d0-000000000000.d.codeartifact.us-west-2.amazonaws.com/npm/sim2real-behav-repo-96f598a3e374/-/ping?write=true (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:40655/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Expected: GetRepositoryEndpoint(domain=<d>, repository=<r>, format=npm) -> 200 with repositoryEndpoint = <scheme>://<the host:port the emulator serves>/npm/<r>/ , and GET <repositoryEndpoint>-/ping?write=true with header Authorization: Bearer <token from GetAuthorizationToken(durationSeconds=900)> -> HTTP 200 while the token is valid.
Actual: 200 with repositoryEndpoint on the AWS production authority <domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com — an address the emulator does not serve — so the ping request is never issued and the token-usability assertion is unreachable.

The earlier parts of the check are consistent with AWS: durationSeconds=899 -> ValidationException with message "Invalid durationSeconds value of 899: value must be 0 or between 900 and 43200 (inclusive)." (fixed text apart from the rejected value), and durationSeconds=900 -> expiration 900 s after issuance.

The host, port, generated domain/repository names, the 000000000000 account id and all timestamps above are per-run values, not requirements.

## Steps to Reproduce
1. Boot the emulator and point a codeartifact client at it.
2. CreateDomain(domain=<d>); CreateRepository(domain=<d>, repository=<r>).
3. GetAuthorizationToken(domain=<d>, durationSeconds=899): confirm ValidationException with message "Invalid durationSeconds value of 899: value must be 0 or between 900 and 43200 (inclusive)."
4. GetAuthorizationToken(domain=<d>, durationSeconds=900): note authorizationToken and expiration (expiration should be ~900 s after issuance).
5. GetRepositoryEndpoint(domain=<d>, repository=<r>, format="npm").
6. Observe the 200 body: repositoryEndpoint's authority is <d>-<account>.d.codeartifact.<region>.amazonaws.com, not the host:port the request was sent to.
7. GET <repositoryEndpoint>-/ping?write=true with header "Authorization: Bearer <authorizationToken>": the request never reaches the emulator, so the expected HTTP 200 cannot be observed. The check stops here.
