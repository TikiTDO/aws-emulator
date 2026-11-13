Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-ccp-traj-absent-domain
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-ccp-traj-absent-domain customer-profiles-ccp-traj-connect-integration-lookup customer-profiles-ccp-traj-create-input-mistakes customer-profiles-ccp-traj-duplicate-then-dedupe customer-profiles-ccp-traj-search-request-mistakes customer-profiles-ccp-traj-search-then-create customer-profiles-ccp-traj-wrong-field-then-update customer-profiles-ccp-traj-wrong-region customer-profiles-ccp-traj-wrong-search-key customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling customer-profiles-create-domain-rejects-a-name-outside-the-name-charset customer-profiles-domain-unset-optional-fields-stay-absent customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|GetDomain|4xx->4xx|$.Message
Issue-Key: customer-profiles|DeleteProfile|2xx->2xx|$.Message
Issue-Related: 20261103T055557Z-customer-profiles-400-refusals-carry-no-message-body.md
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# customer-profiles resource-not-found and DeleteProfile success messages are emulator-authored, not AWS wording (5 causes)

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomain

## Description
Five net-new wire causes that share ONE fault: the customer-profiles crate composes its own English for responses whose Message member is a wire requirement. The status and the error code are right in every cause below (404 -> 404, 200 -> 200); only the sentence diverges, and AWS words each of these per-operation rather than with one generic sentence.

Cause 1 of 5: GetDomain's domain-not-found sentence.
AWS: Smithy: GetDomain (GET /domains/{DomainName}) models ResourceNotFoundException with httpError 404. Recorded response: 404, body Message 'Domain <DomainName> is not found'. Fixed text: 'Domain ' + ' is not found'; the single variable part is the DomainName from the request path.
Emulator: 404 with 'The domain <DomainName> does not exist' - a different sentence shape with an added article and a different verb.
Why: 'domain missing' is the most-taken branch in any Customer Profiles client (existence probes, teardown sweeps, idempotent create). Clients and harnesses that match AWS's sentence do not match the emulator's, so they misclassify a genuine 404.
Fix: the shared domain-lookup not-found refusal behind GetDomain (the crate's domain-not-found helper and the get_domain handler) - emit the fixed sentence with the requested DomainName interpolated.
Traces: customer-profiles-ccp-traj-absent-domain#teardown#9 - the largest cluster in the sweep, 13 steps across 10 fixtures.

Cause 2 of 5: SearchProfiles' domain-not-found sentence drops the account id.
AWS: Smithy: SearchProfiles (POST /domains/{DomainName}/profiles/search) models ResourceNotFoundException/404. Recorded response: 404, body Message 'DomainName <DomainName> is not found in account <accountId>'. Fixed text: 'DomainName ' + ' is not found in account '; variable parts: the DomainName from the request path and the 12-digit account id of the calling credentials (from the emulator's own request scope - never a literal from one recording).
Emulator: 404 with 'The domain <DomainName> does not exist' - the emulator's own sentence, and the account id is dropped.
Why: the account id is what distinguishes 'you are in the wrong account' from 'this domain does not exist'; a client logging or parsing AWS's sentence loses that discrimination entirely.
Fix: SearchProfiles' domain resolution - build the message from the requested DomainName plus the resolved caller account id.
Traces: customer-profiles-ccp-traj-absent-domain#task#2.

Cause 3 of 5: CreateProfile's domain-not-found sentence has the same defect.
AWS: Smithy: CreateProfile (POST /domains/{DomainName}/profiles) models ResourceNotFoundException/404. Recorded response: 404, body Message 'DomainName <DomainName> is not found in account <accountId>' - same template as Cause 2.
Emulator: 404 with 'The domain <DomainName> does not exist'.
Why: a client creating profiles into a mistyped or cross-account domain cannot tell which of the two happened.
Fix: CreateProfile's domain resolution, sharing the account-aware message builder from Cause 2.
Traces: customer-profiles-ccp-traj-absent-domain#task#3.

Cause 4 of 5: DeleteProfile's profile-not-found sentence is reworded and names the domain AWS does not name.
AWS: Smithy: DeleteProfile (POST /domains/{DomainName}/profiles/delete) models ResourceNotFoundException/404; ProfileId is a required `uuid` (@pattern ^[a-f0-9]{32}$). Recorded response: 404, body Message 'Profile with profileId <ProfileId> is not found in the domain'. Fixed text: 'Profile with profileId ' + ' is not found in the domain'; the only variable part is the ProfileId from the request - the domain name does NOT appear in AWS's sentence.
Emulator: 404 with 'The profile <ProfileId> does not exist in domain <DomainName>' - reworded and interpolating an extra request value.
Why: deleting an already-deleted profile is a normal idempotency path; callers that match AWS's sentence to treat it as success instead surface an error, and the surplus domain name invites parsers to depend on a field AWS never sends.
Fix: DeleteProfile's profile-lookup refusal - emit the fixed sentence with only the ProfileId interpolated.
Traces: customer-profiles-ccp-traj-duplicate-then-dedupe#task#7.

Cause 5 of 5: DeleteProfile's SUCCESS body returns a composed sentence instead of AWS's fixed acknowledgement.
AWS: Smithy: DeleteProfileResponse$Message targets `message` and is documented as 'A message that indicates the delete request is done.' Recorded response: 200 with Message exactly 'OK' - the whole value is fixed text and interpolates nothing from the request or state.
Emulator: 200 with 'Profile <ProfileId> deleted' - a per-request sentence where AWS returns a constant.
Why: the delete acknowledgement is the one value in the response; a client asserting the documented acknowledgement, or comparing success bodies, diverges on every successful delete, and the emulator's variable value makes those bodies non-deterministic.
Fix: the DeleteProfile handler's success projection - serialize the fixed acknowledgement value.
Traces: customer-profiles-ccp-traj-duplicate-then-dedupe#task#5.

Related but not covering: open behavioral issue 20261102T122410Z-customer-profiles-10-net-new-behavioral-causes-refusal.md records the CreateProfile wording (its Cause 2) from the behavioral suite; it carries no wire replay keys, and the GetDomain, DeleteProfile-404 and DeleteProfile-200 sentences are not among its causes.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false). All five clusters are verdict=mismatch with expected_status == actual_status; the only diverging key is $.Message.

Cause 1 - fixture customer-profiles-ccp-traj-absent-domain step teardown#9, GetDomain, 404 -> 404:
  $.Message: value_mismatch (expected 'Domain <DomainName> is not found', got 'The domain <DomainName> does not exist'). affected_count=13, spanning customer-profiles-ccp-traj-connect-integration-lookup#teardown#7, customer-profiles-ccp-traj-create-input-mistakes#teardown#10, customer-profiles-ccp-traj-duplicate-then-dedupe#teardown#10, customer-profiles-ccp-traj-search-request-mistakes#teardown#11, customer-profiles-ccp-traj-search-then-create#teardown#11, customer-profiles-ccp-traj-wrong-field-then-update#teardown#10, customer-profiles-ccp-traj-wrong-region#teardown#7, customer-profiles-ccp-traj-wrong-search-key#teardown#9, customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling#teardown#7, customer-profiles-create-domain-rejects-a-name-outside-the-name-charset#teardown#6, customer-profiles-domain-unset-optional-fields-stay-absent#teardown#4, customer-profiles-tag-resource-after-create-unions-and-untag-resource-scopes-the-removal#teardown#9.

Cause 2 - fixture customer-profiles-ccp-traj-absent-domain step task#2, SearchProfiles, 404 -> 404:
  $.Message: value_mismatch (expected 'DomainName <DomainName> is not found in account <accountId>', got 'The domain <DomainName> does not exist').

Cause 3 - same fixture step task#3, CreateProfile, 404 -> 404: identical expected/actual pair as Cause 2.

Cause 4 - fixture customer-profiles-ccp-traj-duplicate-then-dedupe step task#7, DeleteProfile, 404 -> 404:
  $.Message: value_mismatch (expected 'Profile with profileId <ProfileId> is not found in the domain', got 'The profile <ProfileId> does not exist in domain <DomainName>').

Cause 5 - same fixture step task#5, DeleteProfile, 200 -> 200:
  $.Message: value_mismatch (expected 'OK', got 'Profile <ProfileId> deleted').

The domain names, profile ids and account id in the recorded strings are that run's fill-ins of the templates named in the description; the fixed text and which parts are variable are the requirement.

## Steps to Reproduce
Cause 1: GetDomain(DomainName=<a name never created>) -> expect 404 ResourceNotFoundException with Message 'Domain <name> is not found'; observe 'The domain <name> does not exist'. Same for a domain deleted earlier in the run (the teardown path all 13 affected steps take).
Cause 2: SearchProfiles(DomainName=<absent domain>, KeyName='_account', Values=['1']) -> expect 404 with 'DomainName <name> is not found in account <caller account id>'; observe the emulator sentence with no account id.
Cause 3: CreateProfile(DomainName=<absent domain>, AccountNumber=A) -> expect the same account-bearing sentence; observe the same emulator sentence.
Cause 4: CreateDomain(D); CreateProfile(D, AccountNumber=A) -> P; DeleteProfile(D, ProfileId=P); DeleteProfile(D, ProfileId=P) again -> expect 404 with 'Profile with profileId <P> is not found in the domain' (no domain name); observe 'The profile <P> does not exist in domain D'.
Cause 5: the FIRST DeleteProfile(D, ProfileId=P) above -> expect 200 with Message exactly 'OK'; observe 'Profile <P> deleted'.

Re-replay fixtures customer-profiles-ccp-traj-absent-domain and customer-profiles-ccp-traj-duplicate-then-dedupe to confirm; the GetDomain sentence is exercised by the teardown step of ten further fixtures.
