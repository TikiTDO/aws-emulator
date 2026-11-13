Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.delete-profile-key-mismatched-value-rejected
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.delete-profile-key-mismatched-value-rejected|run#12|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: f33a87284c95bf3cf6c54b2e04b0977f381a0f6d

# customer-profiles DeleteProfileKey answers 501 NotImplemented: the key-removal operation is an unimplemented scaffold stub (1 cause)

> _Note: this issue was identified against the code base at commit `f33a87284c95bf3cf6c54b2e04b0977f381a0f6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteProfileKey

## Description
Cause 1 of 1: `DeleteProfileKey` is never served — every call, valid or invalid, is refused with the scaffold's 501 `NotImplemented`, so neither the success path nor the key-not-found refusal exists.

AWS: Smithy (services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/smithy-model/com-amazonaws-customerprofiles-customerprofiles-20200815.json) models `DeleteProfileKey` as POST /domains/{DomainName}/profiles/keys/delete, success HTTP 200 with output member `Message` (shape `message`), required input `DomainName` (name, len 1..64, pattern ^[a-zA-Z0-9_-]+$, httpLabel), `ProfileId` (uuid, pattern ^[a-f0-9]{32}$), `KeyName` (name, len 1..64, same pattern) and `Values` (requestValueList); modeled errors AccessDeniedException (403), BadRequestException (400), InternalServerException (500), ResourceNotFoundException (404) — "The requested resource does not exist, or access was denied" — and ThrottlingException (429). 501/`NotImplemented` is not in that set. Recorded behavior: deleting a value that was never added for the given KeyName on that profile is refused with HTTP 404, error code `ResourceNotFoundException` and the message `Failed to delete profile key due to key not found`. That message is entirely FIXED text for this error class — it interpolates nothing from the request (no domain, profile id, key name or value appears in it), so it is reproduced verbatim. The rejected delete must also be a no-op: the real value stays searchable through SearchProfiles. Deleting with the value that IS present on the key succeeds (200) and the value stops matching in SearchProfiles.

Emulator: the handler body in services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/delete_profile_key.rs is still the generated stub (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteProfileKey"))`, SCAFFOLDED marker on line 4), so every request — including a well-formed delete of an existing key value — comes back as HTTP 501 with error code `NotImplemented`. The replay asserts the key-not-found refusal and observes code 'NotImplemented' instead of 'ResourceNotFoundException'; the positive-control delete that should return 200 is never reached.

Why: a client cannot remove a searchable key from a profile at all, and cannot distinguish a bad key value from a server that has not implemented the call. Code that treats 404/ResourceNotFoundException as "that key value is already gone" (an idempotent cleanup) sees a 501 instead, which botocore surfaces as a non-retryable server-side failure, so profile-key cleanup and contact-id re-keying flows abort; nothing ever becomes unsearchable because no key is ever deleted.

Fix: implement `DeleteProfileKey::handle` in that operation handler — remove the listed Values from the named key of the profile in the service state, return 200 with the output `Message`, and refuse an unknown domain/profile or a value not present on the key with the modeled `ResourceNotFoundException` carrying AWS's fixed sentence, leaving state untouched on refusal. Then delete the SCAFFOLDED marker line.

Traces: customer-profiles.delete-profile-key-mismatched-value-rejected.

## Evidence
Check customer-profiles.delete-profile-key-mismatched-value-rejected [mismatch]:
  AssertionError: expected ResourceNotFoundException deleting a value never added for this key, got 'NotImplemented'
  (check_customer_profiles_delete_profile_key_mismatched_value_rejected.py line 51)

Expected (real AWS, per the check's recorded assertions and the Smithy model):
  DeleteProfileKey(DomainName=<domain>, ProfileId=<32-hex id>, KeyName="_ctrContactId", Values=[<value never added>])
    -> HTTP 404, Error.Code = "ResourceNotFoundException",
       Error.Message = "Failed to delete profile key due to key not found" (fixed text, no interpolated values)
    -> the previously added value is still returned by SearchProfiles for that key
  DeleteProfileKey(..., Values=[<value that was added>])
    -> HTTP 200 with output member Message; the value no longer matches SearchProfiles

Actual (emulator):
  HTTP 501, Error.Code = "NotImplemented" for every DeleteProfileKey call.
  Handler: services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/delete_profile_key.rs
    line 4: "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`"
    line 165: Err(crate::scaffolded::Scaffolded::not_implemented("DeleteProfileKey"))

## Steps to Reproduce
1. Boot the emulator and point a botocore customer-profiles client at it.
2. cp.create_domain(DomainName=<unique domain>, DefaultExpirationDays=30, Tags={...}).
3. profile_id = cp.create_profile(DomainName=<domain>)["ProfileId"].
4. cp.add_profile_key(DomainName=<domain>, ProfileId=profile_id, KeyName="_ctrContactId", Values=[<real value>]) and confirm cp.search_profiles(DomainName=<domain>, KeyName="_ctrContactId", Values=[<real value>]) returns the profile.
5. cp.delete_profile_key(DomainName=<domain>, ProfileId=profile_id, KeyName="_ctrContactId", Values=[<value never added>]).
   Observed: ClientError with Code 'NotImplemented' (HTTP 501).
   Expected: HTTP 404, Code 'ResourceNotFoundException', Message 'Failed to delete profile key due to key not found', with the real value still searchable.
6. cp.delete_profile_key(..., Values=[<real value>]).
   Observed: HTTP 501 'NotImplemented'. Expected: HTTP 200 with a Message, after which SearchProfiles on that value returns no items.

Replay: run the behavioral suite check customer-profiles.delete-profile-key-mismatched-value-rejected.
