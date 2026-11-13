Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.deploy-opensearch-3-fgac-secret-master-basic-auth
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.deploy-opensearch-3-fgac-secret-master-basic-auth|run#38|TimeoutError|raise TimeoutError("GET / to answer 200 for the master: the run budget ended first; not graded")
Issue-Commit: 06a3184ccc527ca9d3f46198a2801cea06e3f766

# opensearch: an FGAC domain's endpoint never answers HTTP Basic auth from the internal-user master, so the whole internal user database / role-mapping surface is unreachable

> _Note: this issue was identified against the code base at commit `06a3184ccc527ca9d3f46198a2801cea06e3f766`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Cause 1 of 1: a domain created with fine-grained access control and an internal user database never serves an HTTP Basic-authenticated request on its published endpoint.

AWS: Smithy (com.amazonaws.opensearch) models CreateDomain.AdvancedSecurityOptions as AdvancedSecurityOptionsInput with Enabled, InternalUserDatabaseEnabled and MasterUserOptions{MasterUserName @length 1..64 @sensitive, MasterUserPassword @length 8..128 @sensitive} — "Credentials for the master user for a domain", MasterUserName/MasterUserPassword "Only specify if InternalUserDatabaseEnabled is true". Beyond that the data-plane contract is recorded behavior only: once such a domain reports Processing=false, its endpoint (DomainStatus.Endpoint) answers `GET /` presented with HTTP Basic credentials equal to the configured master user with HTTP 200 and a body whose `version` block carries `number` starting with the deployed engine's major.minor (here the 2.11 line) and `distribution` = "opensearch"; the same request with a wrong password, and with no Authorization header, is refused 401 (the anonymous refusal carrying a `WWW-Authenticate: Basic realm=...` challenge and neither 401 body leaking `cluster_name`); `GET /_plugins/_security/api/account` as that master returns 200 with user_name = the configured master user, is_internal_user = true and `all_access` among its roles. The master's 200 is what gates every later FGAC check (creating a second internal user, its 403 `security_exception` with "no permissions for [indices:data/read/search]" on an unmapped index, and its 200 after a role mapping).

Emulator: that first 200 never arrives. The check polls `GET /` with the master credentials until the run budget is exhausted and then raises `TimeoutError: GET / to answer 200 for the master: the run budget ended first; not graded` (check_opensearch_deploy_opensearch_3_fgac_secret_master_basic_auth.py line 201). Everything before it succeeded: the CloudFormation stack reached CREATE_COMPLETE, the Secrets Manager dynamic reference resolved, and DescribeDomain reported AdvancedSecurityOptions.Enabled/InternalUserDatabaseEnabled true with an Endpoint matching the stack output — so the control plane accepted and echoed FGAC while the endpoint it published answers no Basic-authenticated request at all. Note: the per-run values in that recording (generated domain/secret/user names, the generated 32-character password, the endpoint host) are not part of the requirement; only the statuses, codes and message texts above are.

Why: a client that enables fine-grained access control with an internal user database and then waits for its domain to answer hangs instead of being served a 200 or refused a 401/403. No internal user can be created, no role mapping made, and no authorization refusal observed, so the FGAC identity model is untestable and unusable against the emulator.

Fix: the domain's data-plane endpoint handler must serve requests authenticated with HTTP Basic against the internal user database once the domain is active — admitting the configured master user (200 on `/` with the version block, 200 on the security account API with all_access), refusing a wrong or absent credential with 401 (Basic challenge on the anonymous case, no cluster info in either body), and refusing an authenticated-but-unmapped user with a 403 security_exception naming the refused action. Related open issue 20261102T091623Z-opensearch-a-created-domain-s-endpoint-never-answers.md reports the same endpoint silence for a SigV4/IAM master role and carries that other check's replay key; this one is the internal-user/Basic-auth path and the security plugin APIs behind it.

Traces: opensearch.deploy-opensearch-3-fgac-secret-master-basic-auth.

## Evidence
Expected: GET / on the FGAC domain's endpoint with the configured internal master user's Basic credentials -> HTTP 200 with version.number starting "2.11." and version.distribution == "opensearch"; wrong password -> 401; no Authorization -> 401 with `WWW-Authenticate: Basic realm=...`; GET /_plugins/_security/api/account as master -> 200 {user_name: <configured master user>, is_internal_user: true, roles: [... "all_access" ...]}.

Actual: no response with status 200 is ever produced for the master; the poll runs until the runner's budget ends and the check dies with `TimeoutError: GET / to answer 200 for the master: the run budget ended first; not graded` (raised at check_opensearch_deploy_opensearch_3_fgac_secret_master_basic_auth.py:201). The preceding control-plane assertions all passed: stack CREATE_COMPLETE, secret resolved with the template's username and a 32-char password absent from the stored template, DescribeDomain reporting AdvancedSecurityOptions{Enabled: true, InternalUserDatabaseEnabled: true}, EncryptionAtRest/NodeToNode enabled, EnforceHTTPS true and Endpoint equal to the stack's DomainEndpoint output.

## Steps to Reproduce
1. CreateDomain (directly, or via an AWS::OpenSearchService::Domain stack as the check does) with EngineVersion OpenSearch_2.11, AdvancedSecurityOptions{Enabled: true, InternalUserDatabaseEnabled: true, MasterUserOptions{MasterUserName, MasterUserPassword}}, NodeToNodeEncryptionOptions and EncryptionAtRestOptions enabled and DomainEndpointOptions.EnforceHTTPS true.
2. Wait for DescribeDomain to report Processing=false and read DomainStatus.Endpoint.
3. Send GET / to that endpoint with HTTP Basic auth using the configured master user name and password: observe that no 200 is ever returned (the poll never succeeds).
4. Or replay opensearch.deploy-opensearch-3-fgac-secret-master-basic-auth.
