Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: evs.create-environment-connector-invalid-secret-rejected evs.create-environment-idempotent-client-token-concurrent evs.create-environment-invalid-vpc-rejected evs.create-environment-terms-not-accepted-rejected evs.delete-environment-with-active-connector-rejected evs.get-versions-returns-known-vcf-versions evs.list-environments-invalid-next-token-rejected evs.put-account-settings-invalid-value-rejected
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs.create-environment-connector-invalid-secret-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: evs.create-environment-idempotent-client-token-concurrent|run#10|AssertionError|assert env_id_a is None, (
Issue-Key: evs.create-environment-invalid-vpc-rejected|run#7|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: evs.create-environment-terms-not-accepted-rejected|run#5|AssertionError|assert message == expected, (
Issue-Key: evs.delete-environment-with-active-connector-rejected|_error_code_and_message#6|AssertionError|raise AssertionError(
Issue-Key: evs.get-versions-returns-known-vcf-versions|run#8|AssertionError|assert all(_ESX_VERSION_RE.match(e) for e in i["esxVersions"]), (
Issue-Key: evs.list-environments-invalid-next-token-rejected|run#6|AssertionError|assert message == (
Issue-Key: evs.put-account-settings-invalid-value-rejected|run#6|AssertionError|assert message == (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# evs: 5 net-new root causes — CreateEnvironment never applies the AWS Premium Support entitlement gate, wrong termsAccepted/nextToken/account-setting refusal wording, and GetVersions ESX versions are not ESXi build ids

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEnvironment

## Description
Five distinct net-new root causes in the evs crate, all grounded in failing behavioral checks. They are independent: four are refusal/representation wording or content defects in separate handlers, one is an entitlement gate CreateEnvironment never applies at all. Cause 1 must be closed before the two checks that only observe the gate can pass, but each cause below needs its own fix.

=== Cause 1 of 5: CreateEnvironment accepts every request; the account-level AWS Premium Support entitlement gate is never applied, and its clause never leads the joined validation message ===
AWS: recorded behavior only (the Smithy model declares CreateEnvironment's single error as ValidationException, HTTP 400, and says nothing about support plans). On the recorded account class — an account with no AWS Business/Enterprise(-On-Ramp) Support plan — Amazon EVS refuses EVERY CreateEnvironment call, however well formed, with HTTP 400 ValidationException and the message `Request validation failure: You must be subscribed to AWS Premium Support`. The whole message is fixed text for this error class; it carries no value from the request. When the request ALSO fails other request validation, real AWS reports every failed validation in ONE message joined by ` | `, with the entitlement clause FIRST: e.g. `Request validation failure: You must be subscribed to AWS Premium Support | The VPC id <vpcId> is not valid or does not exist. Please verify the VPC id exists and is valid.` (the `<vpcId>` there is the value taken from the request; the rest of that clause is fixed text). The refusal is total: nothing is created, so the environment never appears in ListEnvironments.
Emulator: CreateEnvironment answers HTTP 200 with a full `environment` object (environmentState CREATING, a minted environmentId/environmentArn) for a well-formed request — see the replayed responses in the checks below — so the entitlement gate is absent entirely. Where the request does fail another validation the emulator refuses correctly in isolation but omits the leading gate clause: for a nonexistent vpcId it returns `Request validation failure: The VPC id vpc-00000000 is not valid or does not exist. Please verify the VPC id exists and is valid.` instead of the gate clause joined with ` | ` ahead of it.
Why: every client that asserts EVS is unavailable on a non-Premium-Support account sees creation succeed, so negative-path code (entitlement handling, support-plan onboarding prompts, retry/abort decisions) is never exercised and is validated against the opposite of production. Clients parsing the joined ` | ` message also mis-attribute the failure: they see only the VPC clause and conclude the account is entitled.
Fix: CreateEnvironment's validation/handler (services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment.rs) plus the shared validation-failure aggregation in crate::state (aggregate_validation_error / request_validation_error) so the entitlement clause is collected as the FIRST failure and joined with ` | ` with the other collected clauses, and the call is refused before any environment is materialised.
Traces: evs.create-environment-connector-invalid-secret-rejected, evs.delete-environment-with-active-connector-rejected, evs.create-environment-idempotent-client-token-concurrent (both legs of the concurrent same-clientToken pair must be refused identically, and neither must create an environment), evs.create-environment-invalid-vpc-rejected (joined-clause case), evs.create-environment-terms-not-accepted-rejected.

=== Cause 2 of 5: the termsAccepted refusal clause is worded in the emulator's own terms instead of AWS's ===
AWS: recorded behavior only; Smithy marks CreateEnvironment.termsAccepted @required Boolean, and the only modeled error is ValidationException (400). With `termsAccepted=false` real AWS adds the clause `Terms and Conditions must be accepted before creating an environment` to the joined validation message. That clause is entirely fixed text — it names no member and interpolates nothing from the request.
Emulator: the clause reads `termsAccepted must be accepted (true) before an environment can be created` (observed as the whole message `Request validation failure: termsAccepted must be accepted (true) before an environment can be created`). Code is ValidationException, which is correct; the wording is not.
Why: a message that differs from AWS's wording breaks clients and tests that match on the refusal text to distinguish an unaccepted-terms failure from other request-validation failures, and it leaks an implementation-flavoured member name instead of AWS's user-facing phrasing.
Fix: the termsAccepted check in CreateEnvironment's validation (src/ops/create_environment.rs) — emit AWS's fixed clause text verbatim.
Traces: evs.create-environment-terms-not-accepted-rejected.

=== Cause 3 of 5: GetVersions reports bare vSphere version numbers where AWS reports ESXi build ids ===
AWS: Smithy GetVersions output members `vcfVersions` (VcfVersionList) and `instanceTypeEsxVersions` (InstanceTypeEsxVersionsList) are both @required; VcfVersionInfo.defaultEsxVersion is documented as "The default ESX version for this VCF version. It is based on Broadcom's Bill Of Materials (BOM)", and EsxVersion is a string of max length 128. Recorded behavior: every ESX version Amazon EVS reports — both each `instanceTypeEsxVersions[].esxVersions[]` entry and each `vcfVersions[].defaultEsxVersion` — is an ESXi build identifier beginning with the literal prefix `ESXi-`. The particular build ids are service catalogue data and vary; the `ESXi-` build-id FORM is the requirement, as is the internal consistency the model implies (a version's defaultEsxVersion appears among the esxVersions of each instance type it names).
Emulator: the catalogue reports plain vSphere version strings — for instance type `i4i.metal`, `esxVersions: ['8.0.3']` — with no `ESXi-` build-id prefix, so defaultEsxVersion is in the same non-AWS form.
Why: callers that select or pin a host ESX version (and anything that round-trips the value into CreateEnvironmentHost) are built against values real EVS never returns, so version pinning and parsing of build ids fail against production.
Fix: the service version catalogue in crate::state (crate::state::provided_vcf_versions / crate::state::instance_type_esx_versions, read by src/ops/get_versions.rs) — the ESX versions must be ESXi build ids, and the transpose must keep each VCF version's default build id present in every instance type it lists.
Traces: evs.get-versions-returns-known-vcf-versions.

=== Cause 4 of 5: the unissued/expired nextToken refusal message is the crate's own phrasing, not AWS's ===
AWS: Smithy ListEnvironments takes `nextToken` (PaginationToken) and models ValidationException (400) as its only error. Recorded behavior: a nextToken this service never issued is refused with HTTP 400 ValidationException and the message `The provided nextToken is malformed or has expired. Please retry with a valid token or start a new list operation.` — fixed text in full, with no member name and no echo of the supplied token.
Emulator: the refusal is ValidationException (correct code) but the message is `Request validation failure: nextToken is not a pagination token this service issued` — AWS's text is replaced by the crate's internal wording, and the crate's generic `Request validation failure: ` prefix is prepended where AWS prepends nothing for this error.
Why: paginating clients match this text to decide between "restart the listing" and "a genuine input error", and generic error-reporting layers surface it to users; a different message routes them down the wrong branch. The same decoder serves ListEnvironmentConnectors / ListEnvironmentHosts / ListVmEntitlements / ListEnvironmentVlans, so every paginated read of the service answers the wrong text.
Fix: the pagination-token refusal text in crate::state (PageTokenError -> invalid_next_token, src/state.rs, around the `is not a pagination token this service issued` / `Request validation failure:` prefix construction) consumed by src/ops/list_environments.rs. NOTE: open issue 20261029T085937Z-evs-listenvironments-pagination.md covers WHICH tokens are refused (a forged hex cursor is honoured instead of refused); it does not cover the message text of the refusal, which is this cause.
Traces: evs.list-environments-invalid-next-token-rejected.

=== Cause 5 of 5: PutAccountSettings' out-of-range refusal names a wire path and a derived limit sentence instead of AWS's setting-named message ===
AWS: Smithy AccountSetting.name documents `vcfPortedCoreCount` (numeric string) with "The maximum value is 1,000,000 cores"; PutAccountSettings models ValidationException (400). Recorded behavior: a value above that maximum is refused with HTTP 400 ValidationException and `Request validation failure: Setting '<name>' must not exceed <maximum>.` — `Request validation failure: Setting '` ... `' must not exceed ` ... `.` is fixed text, `<name>` is the setting name taken from the request (`vcfPortedCoreCount`) and `<maximum>` is that setting's documented maximum (1000000). The rejected put must leave the stored setting unchanged, and an in-range put must persist.
Emulator: ValidationException (correct code) with `Request validation failure: settings.1.member.value must be at most 1000000, the maximum value of the vcfPortedCoreCount setting` — it names the wire member path `settings.1.member.value` of the offending list entry and re-words the limit sentence, where AWS quotes the setting name and uses the `must not exceed` phrasing.
Why: clients and operators keying on this refusal cannot map it to a setting (they get an index-bearing member path that shifts with request ordering), and anything asserting AWS's wording fails; a leaked internal member-path convention also differs between the emulator and production for identical input.
Fix: the account-setting domain validation in crate::state (crate::state::account_setting_failures and the aggregate_validation_error assembly it feeds, src/state.rs) invoked from src/ops/put_account_settings.rs — emit AWS's fixed text with the setting name from the request and the catalogued maximum.
Traces: evs.put-account-settings-invalid-value-rejected.

## Evidence
All evidence is from the simparity-behavioral-aws-v1 replay (evs 12/20 passed, 8 mismatches, 0 errors, 0 skipped; no failure was marked ALREADY FILED or OWNED).

Cause 1 — CreateEnvironment entitlement gate absent:
- evs.create-environment-connector-invalid-secret-rejected: expected ValidationException `Request validation failure: You must be subscribed to AWS Premium Support`; actual HTTP 200 with {'environment': {'environmentId': 'env-2af63a6d86', 'environmentState': 'CREATING', 'environmentArn': 'arn:aws:evs:us-west-2:000000000000:environment/env-2af63a6d86', ... 'termsAccepted': True, 'environmentStatus': 'UNKNOWN'}} (ids/timestamps are per-run; the 200 is the divergence).
- evs.delete-environment-with-active-connector-rejected: same expectation, actual HTTP 200 with environmentId 'env-e09d2d22c6'.
- evs.create-environment-idempotent-client-token-concurrent: "concurrent CreateEnvironment (leg a) unexpectedly succeeded ('env-aaf91e9a26')" — both legs sharing one clientToken must be refused identically with the gate message.
- evs.create-environment-invalid-vpc-rejected: expected `Request validation failure: You must be subscribed to AWS Premium Support | The VPC id vpc-00000000 is not valid or does not exist. Please verify the VPC id exists and is valid.`; got `Request validation failure: The VPC id vpc-00000000 is not valid or does not exist. Please verify the VPC id exists and is valid.` (VPC clause correct, leading gate clause and ` | ` join missing).

Cause 2 — termsAccepted wording (termsAccepted=false leg of evs.create-environment-terms-not-accepted-rejected):
  expected: 'Request validation failure: You must be subscribed to AWS Premium Support | Terms and Conditions must be accepted before creating an environment'
  got:      'Request validation failure: termsAccepted must be accepted (true) before an environment can be created'
  (code ValidationException in both; the clause text diverges independently of cause 1.)

Cause 3 — GetVersions ESX versions (evs.get-versions-returns-known-vcf-versions):
  "i4i.metal esxVersions do not look like ESXi build ids: ['8.0.3']" — the check requires every esxVersions entry and every vcfVersions[].defaultEsxVersion to match ^ESXi-.

Cause 4 — ListEnvironments nextToken message (evs.list-environments-invalid-next-token-rejected), nextToken='sim2real-not-a-real-token':
  expected: 'The provided nextToken is malformed or has expired. Please retry with a valid token or start a new list operation.'
  got:      'Request validation failure: nextToken is not a pagination token this service issued'
  (positive control — list_environments() with no nextToken — succeeded, so the refusal path itself is reached.)

Cause 5 — PutAccountSettings out-of-range message (evs.put-account-settings-invalid-value-rejected), settings=[{name:'vcfPortedCoreCount', value:'50000000'}]:
  expected: "Request validation failure: Setting 'vcfPortedCoreCount' must not exceed 1000000."
  got:      'Request validation failure: settings.1.member.value must be at most 1000000, the maximum value of the vcfPortedCoreCount setting'

## Steps to Reproduce
Cause 1:
1. Create a VPC and a subnet, and attach a DHCP options set carrying domain-name, two domain-name-servers and ntp-servers.
2. Call evs CreateEnvironment with that vpcId/serviceAccessSubnetId, vcfVersion=SELF_DEPLOYED, termsAccepted=true, the ten initialVlans and an environmentName. Emulator: HTTP 200 with a CREATING environment. AWS: HTTP 400 ValidationException `Request validation failure: You must be subscribed to AWS Premium Support`, and ListEnvironments afterwards shows no such environment.
3. Repeat with vpcId='vpc-00000000'/serviceAccessSubnetId='subnet-00000000'. Emulator returns only the VPC clause; AWS returns the gate clause, then ` | `, then the VPC clause.
4. Submit two concurrent CreateEnvironment calls sharing one clientToken: both must be refused with the identical code and message, and no environment may be created.

Cause 2:
5. Call CreateEnvironment with parameter validation disabled client-side and termsAccepted=false (otherwise valid request). Compare the ValidationException message against AWS's `Terms and Conditions must be accepted before creating an environment` clause.

Cause 3:
6. Call evs GetVersions. Inspect every instanceTypeEsxVersions[].esxVersions[] entry and every vcfVersions[].defaultEsxVersion: the emulator returns bare versions such as '8.0.3'; AWS returns ESXi build ids beginning with 'ESXi-'.

Cause 4:
7. Call ListEnvironments with no nextToken (succeeds), then with nextToken='sim2real-not-a-real-token'. Compare the ValidationException message against AWS's 'The provided nextToken is malformed or has expired. Please retry with a valid token or start a new list operation.'

Cause 5:
8. Read the current vcfPortedCoreCount with GetAccountSettings, then call PutAccountSettings with settings=[{name:'vcfPortedCoreCount', value:'50000000'}]. Compare the ValidationException message against AWS's "Request validation failure: Setting 'vcfPortedCoreCount' must not exceed 1000000."; confirm the stored value is unchanged, then confirm an in-range put (value '42') persists.
