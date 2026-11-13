Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codestar-connections.create-connection-without-provider-type-or-host codestar-connections.delete-host-with-active-connection-rejected codestar-connections.delete-repository-link-removes-it codestar-connections.host-status-message-present-when-pending
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|CreateConnection|ValidationException
Issue-Key: codestar-connections.delete-host-with-active-connection-rejected|run#8|AssertionError|assert err["Message"] == f"There exist connections for host {host_arn}, host cannot be deleted", (
Issue-Key: codestar-connections.delete-repository-link-removes-it|run#11|AssertionError|assert message == expected_message, (
Issue-Key: codestar-connections.host-status-message-present-when-pending|run#9|AssertionError|assert status_message is None, (
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# codestar-connections: four behavioral divergences — CreateConnection/DeleteHost/GetRepositoryLink refusal messages and a fabricated Host StatusMessage

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
Replay of suite simparity-behavioral-aws-v1 against the booted emulator (29/45 passed; the codestar-connections service answers, so these are fidelity defects, not bootstrap gaps). This bundle carries FOUR distinct root causes, all in the codestar-connections crate. ALL FOUR must be fixed before submitting.

Related open issue, which does NOT cover these behavioral checks (it carries only wire replay keys codestar-connections|DeleteHost|4xx->4xx|$.Message, |GetRepositoryLink|..., |CreateRepositoryLink|...): '20261029T175651Z-codestar-connections-error-messages-diverge-from-aws-wording.md'. Causes 2 and 3 below are the same emulator defects seen from the behavioral suite, so a single message-template fix can close both work items; Causes 1 and 4 are net-new and are named in no open issue.

Cause 1 of 4: CreateConnection with neither ProviderType nor HostArn is refused with emulator-authored prose instead of AWS's message.
AWS: CreateConnection requires exactly one of ProviderType or HostArn (Smithy: CreateConnectionInput.ConnectionName required @length 1..32; ProviderType is an optional enum; HostArn optional @length 0..256 pattern ^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$ — neither member is modelled required, so the one-of rule is enforced by the service, not by the model; modelled errors are LimitExceededException 429 / ResourceNotFoundException 404 / ResourceUnavailableException 400, so the refusal code itself is recorded behavior only). Recorded behavior: HTTP 400, Error Code 'ValidationException', Message exactly 'Must specify either HostArn or ProviderType'. That message is entirely fixed text — it has NO variable part: no member name, no request value, no connection name is interpolated.
Emulator: returns the right code ('ValidationException' — that assertion passed) but Message 'CreateConnection requires either ProviderType, naming the external provider directly, or HostArn, naming the host that holds it; the request supplied neither'.
Why: a client, CLI user or test that branches on or surfaces the one-of validation message sees text real CodeConnections never emits; only the code matches, so message-based disambiguation between the missing-both and the supplied-both rejections breaks.
Fix: the CreateConnection handler's input validation in the codestar-connections service crate — the place that builds the one-of ValidationException. Emit the fixed sentence above verbatim; do not describe the members.
Traces: codestar-connections.create-connection-without-provider-type-or-host.

Cause 2 of 4: DeleteHost on a host that still has connections returns an emulator-authored message that also leaks a connection count.
AWS: DeleteHost refuses a host with associated connections (Smithy: DeleteHost errors = ResourceNotFoundException httpError 404, ResourceUnavailableException httpError 400; DeleteHostInput.HostArn required). Recorded behavior: ResourceUnavailableException with Message 'There exist connections for host <HostArn>, host cannot be deleted'. Fixed text: 'There exist connections for host ' and ', host cannot be deleted'. The only variable part is <HostArn>, copied verbatim from the request. The message carries NO connection count.
Emulator: code ResourceUnavailableException is correct, but Message is 'The host <HostArn> cannot be deleted because <N> connection(s) are still associated with it. Delete those connections first.' (observed: 'The host arn:aws:codestar-connections:us-west-2:000000000000:host/4a26fc8a-... cannot be deleted because 1 connection(s) are still associated with it. Delete those connections first.' — the ARN and the count are per-run values; the invented prose and the presence of a count at all are the defect).
Why: clients that assert on or log the dependency-violation text get text AWS never emits, and the injected count makes the message vary with emulator state where AWS's does not.
Fix: the DeleteHost handler's dependency check in the codestar-connections crate — the ResourceUnavailableException message construction. Status and code already match; only the template changes.
Traces: codestar-connections.delete-host-with-active-connection-rejected.

Cause 3 of 4: the repository-link not-found message capitalises the fixed label 'uuid' as 'UUID'.
AWS: GetRepositoryLink (and UpdateRepositoryLink) answer ResourceNotFoundException when the RepositoryLinkId names no link (Smithy: GetRepositoryLink / UpdateRepositoryLink both list ResourceNotFoundException httpError 404; RepositoryLinkId required, pattern ^[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{12}$). Recorded message is the wrapped CodeConnections form 'Repository not found for accountId: <accountId> and uuid: <RepositoryLinkId> (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <requestId>; Proxy: null)'. Fixed text includes the LOWERCASE token 'and uuid: ' and the framework tail; variable parts are <accountId> (caller account), <RepositoryLinkId> (from the request) and <requestId> (per-run).
Emulator: same wrapper, correct code and 404, but emits 'and UUID: ' — 'Repository not found for accountId: 000000000000 and UUID: 8dfee30f-... (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)'. The differing Request ID is a per-run value and is NOT claimed as a defect; the casing of the fixed label is.
Why: a client or test that matches the documented not-found message fails on the casing even though the resource really is absent; the same formatter is reached from GetRepositoryLink and UpdateRepositoryLink, so both refusals are wrong.
Fix: the shared repository-link not-found message formatter used by the GetRepositoryLink / UpdateRepositoryLink handlers in the codestar-connections crate — emit the token as lowercase 'uuid:'.
Traces: codestar-connections.delete-repository-link-removes-it.

Cause 4 of 4: ListHosts reports a fabricated StatusMessage for a PENDING host that has no VpcConfiguration.
AWS: StatusMessage on the Host structure is 'The status description for the host.' (Smithy: Host.StatusMessage, optional; GetHost's output shape has no StatusMessage member at all, so only ListHosts exposes it). Recorded behavior: a host created with CreateHost(Name, ProviderType=GitHubEnterpriseServer, ProviderEndpoint) and NO VpcConfiguration is listed with Status 'PENDING' and NO StatusMessage member at all — the member is absent/omitted. StatusMessage is populated only for the VPC-config-related transitional states (VPC_CONFIG_INITIALIZING, VPC_CONFIG_FAILED_INITIALIZATION, VPC_CONFIG_DELETING); a host merely awaiting the manual console setup step carries none.
Emulator: Status is 'PENDING' (correct) but it also serialises StatusMessage = 'Host is pending. Complete the host setup in the console to make it available.' — a constant the emulator invents for every pending host.
Why: a client that treats a present StatusMessage as a signal that the host is in a VPC-config transition (or that surfaces it to a user as the reason for PENDING) is misled on every ordinary non-VPC host, and a client that round-trips the Host structure carries a field AWS never set.
Fix: the host state model / ListHosts serializer in the codestar-connections crate — leave StatusMessage unset unless the host is in one of the VPC-config transitional states; do not derive it from Status == PENDING.
Traces: codestar-connections.host-status-message-present-when-pending.

## Evidence
Suite: simparity-behavioral-aws-v1. Total 29/45 passed (64.4%); codestar-connections 29/45, 16 failed. 12 of the 16 failures are already filed (blocked by the unimplemented CreateSyncConfiguration / GetRepositorySyncStatus / ListRepositorySyncDefinitions / UpdateRepositoryLink scaffold stubs) and are excluded here. The four below are the remaining, net-new failures.

Cause 1 — [mismatch] codestar-connections.create-connection-without-provider-type-or-host
  CreateConnection(ConnectionName=<name>) with no ProviderType and no HostArn.
  Error Code: expected 'ValidationException', actual 'ValidationException' (matches).
  Message: expected 'Must specify either HostArn or ProviderType'
           actual   'CreateConnection requires either ProviderType, naming the external provider directly, or HostArn, naming the host that holds it; the request supplied neither'

Cause 2 — [mismatch] codestar-connections.delete-host-with-active-connection-rejected
  CreateHost -> CreateConnection(HostArn=<host>) -> DeleteHost(HostArn=<host>).
  Error Code: ResourceUnavailableException (matches). HTTP 400 (matches).
  Message: expected 'There exist connections for host <hostArn>, host cannot be deleted'
           actual   'The host arn:aws:codestar-connections:us-west-2:000000000000:host/4a26fc8a-8b98-4d48-86d9-000000000000 cannot be deleted because 1 connection(s) are still associated with it. Delete those connections first.'
  Diverging fixed text: AWS's 'There exist connections for host '/', host cannot be deleted' vs invented prose; AWS's template contains no count. The host ARN is a per-run value.

Cause 3 — [mismatch] codestar-connections.delete-repository-link-removes-it
  CreateConnection -> CreateRepositoryLink -> DeleteRepositoryLink -> GetRepositoryLink(<dead id>).
  Error Code: ResourceNotFoundException (matches). Status 404 (matches).
  Message (Request ID masked by the check): 
    expected 'Repository not found for accountId: 000000000000 and uuid: 8dfee30f-55d2-4e61-8b61-000000000001 (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)'
    actual   'Repository not found for accountId: 000000000000 and UUID: 8dfee30f-55d2-4e61-8b61-000000000001 (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)'
  Diverging fixed text: 'and uuid: ' vs 'and UUID: '. accountId and the link id are request/state values; the Request ID is per-run and not claimed.

Cause 4 — [mismatch] codestar-connections.host-status-message-present-when-pending
  CreateHost(ProviderType=GitHubEnterpriseServer, ProviderEndpoint=https://ghe.example.com, no VpcConfiguration) -> ListHosts, pick the matching HostArn.
  Status: 'PENDING' (matches).
  StatusMessage: expected absent (None), actual 'Host is pending. Complete the host setup in the console to make it available.'

Model references used (services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/smithy-model/com-amazonaws-codestarconnections-codestar-connections-20191201.json): CreateConnection (ConnectionName required @length 1..32; ProviderType/HostArn both optional; errors LimitExceededException 429, ResourceNotFoundException 404, ResourceUnavailableException 400), DeleteHost (errors ResourceNotFoundException 404, ResourceUnavailableException 400), GetRepositoryLink and UpdateRepositoryLink (RepositoryLinkId required uuid pattern; errors incl. ResourceNotFoundException 404), ListHosts (output Hosts: HostList), Host.StatusMessage ('The status description for the host.').

## Steps to Reproduce
1. Boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1 against it (the four checks below are self-contained and can be run individually).
2. Cause 1 — replay codestar-connections.create-connection-without-provider-type-or-host: call CreateConnection with only ConnectionName. Assert the refusal is ValidationException with Message exactly 'Must specify either HostArn or ProviderType' (no member names, no request values), then confirm the positive control (ProviderType='GitHub' alone) still succeeds and GetConnection reports ConnectionStatus 'PENDING'.
3. Cause 2 — replay codestar-connections.delete-host-with-active-connection-rejected: CreateHost (GitHubEnterpriseServer), CreateConnection with that HostArn, then DeleteHost on the host. Assert ResourceUnavailableException with Message 'There exist connections for host <the HostArn from the request>, host cannot be deleted' and no connection count. Then delete the connection and confirm DeleteHost succeeds and GetHost 404s with 'No host exists with the ARN: <hostArn>'.
4. Cause 3 — replay codestar-connections.delete-repository-link-removes-it: create a connection and a repository link, DeleteRepositoryLink, then GetRepositoryLink on the dead id. Assert ResourceNotFoundException whose Message's fixed label reads lowercase 'and uuid: ' followed by the request's RepositoryLinkId and the '(Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: ...; Proxy: null)' tail. The same assertion applies to the subsequent UpdateRepositoryLink on the dead id.
5. Cause 4 — replay codestar-connections.host-status-message-present-when-pending: CreateHost with no VpcConfiguration, then ListHosts (paging NextToken) and locate the new HostArn. Assert Status == 'PENDING' and that the Host structure has NO StatusMessage member.
6. All four checks must pass; re-replay the four check ids listed in trace_ids to confirm.
