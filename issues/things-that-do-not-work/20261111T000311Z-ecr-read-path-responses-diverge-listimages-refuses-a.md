Issue-Source: trace-sweep-divergence
Issue-Trace: ecr-batch-diag-image-uri-passed-as-repository-name
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecr-batch-diag-image-uri-passed-as-repository-name api-account-settings-and-authorization-token
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|ListImages|4xx->4xx|$.__type
Issue-Key: ecr|GetAuthorizationToken|2xx->2xx|$.authorizationData[*]
Issue-Related: 20261107T203415Z-ecr-validation-and-not-found-error-messages-diverge.md
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# ECR read-path responses diverge: ListImages refuses a non-matching repositoryName instead of answering RepositoryNotFoundException, and GetAuthorizationToken's authorizationData element does not match the recorded AuthorizationData shape

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListImages

## Description
Two net-new ECR read-path divergences, both on responses the emulator serves for a repository read. Each cause is independent and must be closed before this issue is submitted.

Cause 1 of 2: ListImages with a repositoryName that does not match the modeled RepositoryName pattern (here a full image URI, '<host>:<port>/<repo>', as a client that passed an image URI where a repository name belongs produces) is refused as a parameter-validation failure instead of being looked up and reported as a missing repository.
  AWS: Smithy: ListImages.repositoryName -> RepositoryName @length(min 2, max 256) @pattern ^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$; modeled errors InvalidParameterException (400) and RepositoryNotFoundException (400, doc: 'The specified repository could not be found. Check the spelling of the specified repository and ensure that you are performing operations on the correct registry.'). The model's pattern is NOT what the service enforces on this read: the recorded response for a repositoryName containing characters the pattern forbids is HTTP 400 with __type 'RepositoryNotFoundException' and message "The repository with name '<repositoryName>' does not exist in the registry with id '<registryId>'". Fixed text of that message: "The repository with name '" ... "' does not exist in the registry with id '" ... "'"; its variable parts are the repositoryName exactly as the request sent it and the registry id (the request's registryId, else the caller's account id). So, for a name inside the modeled length bounds, a pattern mismatch is a lookup MISS, not a parameter refusal.
  Emulator: HTTP 400 with __type 'InvalidParameterException' and message "Invalid parameter at 'repositoryName' failed to satisfy constraint: 'must satisfy regular expression '[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*)*''" — the centrally enforced @pattern refusal fires at the request boundary, so the handler's repository lookup (which already produces the correct RepositoryNotFoundException wording for names that do match the pattern) never runs.
  Why: a client that branches on the error class — the common 'does this repository exist yet?' probe, and every tool that accidentally passes a registry-qualified image URI — sees InvalidParameterException where AWS says RepositoryNotFoundException, so the not-found branch (create-if-missing, retry against another registry, clean diagnostic) is never taken and the condition surfaces as an unrecoverable argument error.
  Fix: ECR's ListImages input validation — the boundary @pattern enforcement for repositoryName must not refuse a pattern-mismatching-but-length-legal name on this read; the name must reach the handler's repository lookup so the existing RepositoryNotFoundException path answers with AWS's not-found message.
  Traces: ecr-batch-diag-image-uri-passed-as-repository-name#task#2. Note the already-open issue 'ECR validation and not-found error messages diverge from AWS wording' (20261107T203415Z-ecr-validation-and-not-found-error-messages-diverge.md) covers only the WORDING of messages the emulator already answers with the right error class; it does not cover this step, where the error CLASS itself is wrong.

Cause 2 of 2: the GetAuthorizationToken response's single authorizationData element does not match the recorded AuthorizationData shape; replay reports $.authorizationData[0] as both missing (recorded element absent) and extra (served element unrecognized) while the key set is the same three members.
  AWS: Smithy: GetAuthorizationToken -> output authorizationData (AuthorizationDataList of AuthorizationData); AuthorizationData.authorizationToken (Base64, @pattern ^\S+$, doc 'When the string is decoded, it is presented in the format user:password'); AuthorizationData.expiresAt (ExpirationTimestamp, doc 'The Unix time in seconds and milliseconds when the authorization token expires. Authorization tokens are valid for 12 hours.'); AuthorizationData.proxyEndpoint (doc 'The registry URL to use for this authorization token in a docker login command'). The recorded response carries exactly one element whose three members are shape-asserted, not value-asserted, so the requirement is the SHAPE: a whitespace-free base64 token decoding to 'AWS:<password>', an expiry expressed in Unix seconds AND MILLISECONDS (at most three fractional digits — every other epoch timestamp in this corpus, e.g. the recorded imagePushedAt values, carries three) set 12 hours after the call, and a registry URL for docker login.
  Emulator: the element is served as {authorizationToken: '<base64>', expiresAt: 1794398339.4114988, proxyEndpoint: 'https://<emulator-host>:<port>'} and fails the recorded element's shape assertion. Hypothesis: the diverging member is expiresAt, emitted from a SystemTime with full sub-millisecond precision (seven fractional digits above) where AWS emits seconds-and-milliseconds only.
  Why: a client that parses expiresAt with a seconds-and-milliseconds epoch reader (what the documented precision licenses, and what several SDK/CLI timestamp parsers implement) either rejects or truncates the value, so the credential's refresh deadline is unusable and docker-login refresh logic cannot be exercised against the emulator.
  Fix: the GetAuthorizationToken handler's output construction / timestamp serialization (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/get_authorization_token.rs, the AuthorizationData built at the end of handle) — truncate the issued/expiry instant to millisecond precision before it is serialized, so the element matches the modeled seconds-and-milliseconds shape.
  NOT the requirement, and not what this cause asks to be changed: the host and port inside proxyEndpoint, the token's own bytes, and the absolute expiry instant are environment- and run-specific values.
  Traces: api-account-settings-and-authorization-token#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (74 fixtures, 534 steps, 512 passed, 22 mismatched) reported these two clusters with no owning issue and no stub path:

Cause 1 — fixture ecr-batch-diag-image-uri-passed-as-repository-name, step task#2, operation ListImages, verdict mismatch, expected_status 400, actual_status 400:
  $.__type: value_mismatch (expected 'RepositoryNotFoundException', got 'InvalidParameterException')
  $.message: value_mismatch (expected "The repository with name '<host>:<port>/sim2real-probe-ecr-openmp-ea35cf35' does not exist in the registry with id '000000000000'", got "Invalid parameter at 'repositoryName' failed to satisfy constraint: 'must satisfy regular expression '[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*)*''")

Cause 2 — fixture api-account-settings-and-authorization-token, step task#6, operation GetAuthorizationToken, verdict mismatch, expected_status 200, actual_status 200:
  $.authorizationData[0]: missing (expected {'authorizationToken': '<shaped-not-asserted>', 'expiresAt': '<shaped-not-asserted>', 'proxyEndpoint': '<shaped-not-asserted>'}, got {'authorizationToken': '<base64 of AWS:...>', 'expiresAt': 1794398339.4114988, 'proxyEndpoint': 'https://<emulator-host>:<port>'})
  $.authorizationData[0]: extra (same pair)
The served key set equals the recorded one, so the failing assertion is on a member's shape, not on the element's membership; the only member whose format differs from every other recorded epoch timestamp in the corpus is expiresAt (seven fractional digits vs the documented and recorded seconds-and-milliseconds).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: replay fixture ecr-batch-diag-image-uri-passed-as-repository-name, step task#2 — ListImages with repositoryName set to a registry-qualified image URI ('<host>:<port>/<repo-name>', length within 2..256 but not matching the RepositoryName pattern). Expect HTTP 400 __type RepositoryNotFoundException with message "The repository with name '<repositoryName>' does not exist in the registry with id '<registryId>'"; observe HTTP 400 __type InvalidParameterException with the repositoryName regex-constraint message.
4. Cause 2: replay fixture api-account-settings-and-authorization-token, step task#6 — GetAuthorizationToken against the emulator. Inspect authorizationData[0]: expiresAt is serialized with sub-millisecond precision (e.g. 1794398339.4114988) instead of Unix seconds-and-milliseconds, and the element fails the recorded AuthorizationData shape assertion.
