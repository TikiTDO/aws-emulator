Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.enable-sso-without-prerequisites-denied
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.enable-sso-without-prerequisites-denied|run#11|AssertionError|assert code == "ClientException", f"expected ClientException, got {code}"
Issue-Commit: 2fc08aa0d9b2f1df9d08f4c49d739311c4e863a6

# EnableSso has no missing-prerequisite refusal: enabling SSO before an access URL/alias exists answers 501 NotImplemented instead of 400 ClientException

> _Note: this issue was identified against the code base at commit `2fc08aa0d9b2f1df9d08f4c49d739311c4e863a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableSso

## Description
One net-new root cause (Cause 1 of 1), grounded in failing check `ds.enable-sso-without-prerequisites-denied`.

Related open issue: `20261029T200700Z-ds-enablesso-stub.md` ("Implement ds EnableSso: unimplemented scaffold stub") covers only the fact that the handler is a scaffold and names a single blocked unit, `ds.disable-sso-authentication-failed-rejected`. It does NOT carry this check's replay key, and it explicitly scopes itself to "implement the operation", saying nothing about the single-sign-on precondition contract that this check asserts. This issue records that contract so the EnableSso handler is implemented with it, and so this check is re-replayed.

Cause 1 of 1: EnableSso does not enforce the access-URL/alias prerequisite, and currently does not answer at all.

AWS: Smithy (com.amazonaws.directoryservice#DirectoryService_20150416) models EnableSso with input `DirectoryId` (required, pattern `^d-[0-9a-f]{10}$`), optional `UserName`/`Password`, an empty output shape `EnableSsoResult`, and exactly five errors: AuthenticationFailedException, ClientException, EntityDoesNotExistException, InsufficientPermissionsException (all client, HTTP 400) and ServiceException (server, HTTP 500). AWS's own Managed Microsoft AD single-sign-on guidance (ms_ad_single_sign_on.html) states the access URL/alias must exist before SSO can be enabled ("If you do not see the Enable button, you may need to first create an Access URL before this option will be displayed"). A directory that has never had CreateAlias called has no access URL: per API_DirectoryDescription.html, "If no alias exists, the alias is the directory identifier", so the no-alias condition is observable as `Alias == DirectoryId`. Of the five modeled errors, the directory exists (so not EntityDoesNotExistException), no alternate credentials were supplied (so not AuthenticationFailedException) and this is not an IAM authorization gap (so not InsufficientPermissionsException); the generic client-side precondition failure AWS uses is therefore ClientException at HTTP 400. Its message is ClientException's fixed per-class text with the variable parts — the directory id from the request and the request id — filled from the request and the service's own response metadata; recorded behavior only for the exact ClientException sentence here, so take the fixed text from the sibling ds refusal messages this emulator already serves (the trailing ` : RequestId: <request-id>` suffix the ds error family carries) and the directory id from the request, never from a recording.

After the one missing prerequisite is supplied the same call must SUCCEED: CreateAlias(DirectoryId, Alias) sets DescribeDirectories' `Alias` to the requested alias, and EnableSso then returns 200 with an empty body, after which DescribeDirectories reports `SsoEnabled == true` for that directory. Before that, a fresh directory must report `SsoEnabled == false` and `Alias == <DirectoryId>`.

Emulator: the EnableSso route answers HTTP 501 with error code `NotImplemented` and message "operation EnableSso is scaffolded but not implemented in this emulator" for every request, including the pre-alias one. The check captured `NotImplemented` where `ClientException` is required (AssertionError: expected ClientException, got NotImplemented). Because the operation never answers, neither the pre-alias refusal, the post-CreateAlias success, nor the resulting `SsoEnabled == true` projection is reachable; no SSO state is persisted at all.

Why: a client that gates on this refusal cannot tell a missing access URL from an unsupported API. Code catching ClientException to prompt the operator to create an access URL instead sees an unmodeled `NotImplemented` (501), which SDK retry/error-classification treats as a server-side/unknown failure rather than a caller-fixable precondition, so the operator is never told what to fix. Symmetrically, a client that enables SSO as part of provisioning gets a hard failure where AWS would turn SSO on, and a client that reads back `SsoEnabled` can never observe it become true. The one outcome that must never happen — SSO being silently turned on without an access URL — must also stay impossible once the handler is written: the refusal must leave `SsoEnabled` false.

Fix: the EnableSso handler and its validation in the directoryservice crate — `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/enable_sso.rs` — plus the directory state that holds the SSO flag and the alias, so DescribeDirectories projects `SsoEnabled` and the alias-defaults-to-directory-id rule. The precondition check (alias present, i.e. an alias distinct from the directory id) belongs in that handler's validation ahead of any state mutation, and the refusal must be classified as ClientException (400), not as the scaffold's NotImplemented (501). When implementing, follow the stub-census rule stated in `20261029T200700Z-ds-enablesso-stub.md` for the `//! SCAFFOLDED, NOT IMPLEMENTED:` header.

Traces: ds.enable-sso-without-prerequisites-denied.

## Evidence
Failing check: [mismatch] ds.enable-sso-without-prerequisites-denied

Replay output:
  File "check_ds_enable_sso_without_prerequisites_denied.py", line 63, in run
    assert code == "ClientException", f"expected ClientException, got {code}"
  AssertionError: expected ClientException, got NotImplemented

Expected (AWS): EnableSso(DirectoryId=<d-...>) on an Active Microsoft AD that has never had CreateAlias called -> HTTP 400, Error.Code = "ClientException", message = ClientException's fixed text for a missing-access-URL precondition with the directory id taken from the request and the ` : RequestId: <request-id>` suffix taken from the response metadata. DescribeDirectories afterwards still reports SsoEnabled = false. Then CreateAlias(DirectoryId, Alias=<alias>) -> DescribeDirectories reports Alias = <alias>; the identical EnableSso call -> HTTP 200 with an empty EnableSsoResult body; DescribeDirectories then reports SsoEnabled = true.

Actual (emulator): EnableSso -> HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation EnableSso is scaffolded but not implemented in this emulator", for both the pre-alias refusal and the post-CreateAlias success path. No SSO state is persisted, so SsoEnabled never becomes true.

Fixed text vs variable parts of the required message: the error-class sentence (the missing-access-URL/SSO-precondition wording) is fixed; the directory id is a value from the request; the request id is a value from the response metadata. Neither the directory id nor the request id from any single recording is part of the requirement.

Smithy citation: EnableSso input DirectoryId @required @pattern ^d-[0-9a-f]{10}$; output EnableSsoResult (empty); errors AuthenticationFailedException, ClientException, EntityDoesNotExistException, InsufficientPermissionsException (client, httpError 400), ServiceException (server, httpError 500). NotImplemented is not among them. CreateAlias input Alias @required @length 1..62 @pattern ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$, output {DirectoryId, Alias}.

## Steps to Reproduce
1. Boot the ds (Directory Service) emulator.
2. Create a VPC with two subnets in two distinct availability zones (ec2 CreateVpc + CreateSubnet).
3. ds CreateMicrosoftAD(Name=<name>.example.com, Password=<compliant password>, Edition="Standard", VpcSettings={VpcId, SubnetIds=[two subnets]}) and wait for DescribeDirectories to report Stage = "Active".
4. ds DescribeDirectories(DirectoryIds=[<directory-id>]) and confirm the baseline: SsoEnabled = false and Alias = <directory-id> (no access URL yet).
5. ds EnableSso(DirectoryId=<directory-id>) with no alias created. Observe HTTP 501 / Error.Code "NotImplemented"; AWS answers HTTP 400 / "ClientException".
6. ds DescribeDirectories again and confirm SsoEnabled is still false (the refusal must have no effect).
7. Positive control: ds CreateAlias(DirectoryId=<directory-id>, Alias=<alias>), confirm DescribeDirectories reports Alias = <alias>, then repeat the identical ds EnableSso call. It must return 200 with an empty body and DescribeDirectories must then report SsoEnabled = true; the emulator returns 501 NotImplemented here too.

Replay: ds.enable-sso-without-prerequisites-denied
