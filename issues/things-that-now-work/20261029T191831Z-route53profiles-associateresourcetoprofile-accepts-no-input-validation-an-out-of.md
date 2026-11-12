Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.associate-resource-invalid-firewall-priority-rejected route53profiles.associate-resource-unsupported-arn-rejected
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles.associate-resource-invalid-firewall-priority-rejected|run#5|AssertionError|assert error["Code"] == "ValidationException", f"expected ValidationException, got {error['Code']!r}"
Issue-Key: route53profiles.associate-resource-unsupported-arn-rejected|run#5|AssertionError|assert code == "ValidationException", (
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# AssociateResourceToProfile accepts no input validation: an out-of-range DNS Firewall priority and an unsupported resource ARN must be refused with ValidationException ([RSLVR-02017] / [RSLVR-05022])

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateResourceToProfile

## Description
Two net-new validation requirements of AssociateResourceToProfile that no issue currently carries. NOTE on scope: the operation is presently an unimplemented scaffold stub, and the suite auto-files a separate issue for that implementation gap (`AssociateResourceToProfile`, services/service-com-amazonaws-route53profiles-route53profiles/src/ops/associate_resource_to_profile.rs). That issue covers "make the operation work"; THIS issue covers the two refusals the implementation must also produce, which the replay found as `mismatch` (wrong error code returned to the caller) rather than as the blocked-stub error, and which an implementation that only handles the happy path would silently leave as accept-anything. Both causes below are 'overly-permissive' checks: AWS refuses the request, so an implementation that stores the association instead is a fidelity defect in the opposite direction.

Cause 1 of 2 — an out-of-range DNS Firewall `priority` in ResourceProperties is not refused.
AWS: AssociateResourceToProfile takes `ResourceProperties` (Smithy: AssociateResourceToProfile input = ProfileId (ResourceId, required, len 1..64), ResourceArn (Arn, required, len 1..255), Name (Name, required, len 0..64, pattern ^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$), ResourceProperties; modeled errors AccessDeniedException, ConflictException, InternalServiceErrorException, InvalidParameterException, LimitExceededException, ResourceNotFoundException, ThrottlingException, ValidationException — all errorKind client, httpError 400; http POST /profileresourceassociation, success 200). The model puts NO range constraint on the JSON inside ResourceProperties, so the priority rule is recorded behavior only: when the associated resource is a DNS Firewall rule group and `priority` is outside 100..9900, AWS refuses with ValidationException. Message fixed text, no variable parts: `[RSLVR-02017] The priority value you provided is reserved. Provide a number between "100" and "9900".` The bounds 100 and 9900 are part of the fixed text, not request values. An in-range priority (e.g. 150) must be ACCEPTED, stored, and read back by GetProfileResourceAssociation with ResourceProperties' priority preserved.
Emulator: answers Error.Code `NotImplemented` for the rejectable request, so it never applies the rule; nothing in the handler inspects ResourceProperties.
Why: a client relying on AWS gets no rejection for a reserved priority. Once the happy path lands without this check, the emulator accepts a priority AWS refuses and persists an association that cannot exist in AWS, so rule-group ordering tests pass against the emulator and fail in production; and code branching on ValidationException for this case never fires.
Fix: input validation in the AssociateResourceToProfile handler (parse ResourceProperties, range-check `priority` for a DNS Firewall rule-group ResourceArn, refuse with ValidationException before any state write — the listing must not contain the rejected association).
Traces: route53profiles.associate-resource-invalid-firewall-priority-rejected (ResourceProperties={"priority": 1} against a route53resolver firewall rule group ARN).

Cause 2 of 2 — a well-formed ARN of an UNSUPPORTED resource type is not refused.
AWS: Route 53 Profiles supports associating only certain resource types (private hosted zone, DNS Firewall rule group, Resolver rule, interface VPC endpoint). `ResourceArn` carries no pattern constraint in the model (Smithy: Arn, len 1..255), so the request reaches the service and the rejection is genuine server-side behavior — recorded behavior only for the text. A syntactically valid ARN of an unsupported type (e.g. an S3 bucket, `arn:aws:s3:::<bucket>`) is refused with ValidationException and message fixed text `[RSLVR-05022] The ARN in your request isn't valid:"` + the ResourceArn from the request + `". Provide a valid ARN.` — one variable part, the ARN echoed verbatim from the request (note AWS's spacing: no space after the colon before the opening quote).
Emulator: answers Error.Code `NotImplemented`, so the type check does not exist.
Why: a client gets an association for a resource type AWS cannot associate, or gets `NotImplemented` where it expects ValidationException. ARN-type validation is the only thing stopping an arbitrary ARN from being persisted, and ListProfileResourceAssociations must not report the rejected ARN.
Why it is not Cause 1: a different input member (ResourceArn vs ResourceProperties), a different rule (supported resource type vs numeric range) and a different RSLVR code; one can be implemented without the other.
Fix: input validation in the same handler — classify ResourceArn's service/resource-type against the supported set and refuse with ValidationException before any state write.
Traces: route53profiles.associate-resource-unsupported-arn-rejected.

## Evidence
Cause 1 (route53profiles.associate-resource-invalid-firewall-priority-rejected):
  call: associate_resource_to_profile(ProfileId=<profile>, ResourceArn=<firewall rule group arn>, Name=<name>, ResourceProperties='{"priority": 1}')
  expected: Error.Code='ValidationException', HTTP 400, Message '[RSLVR-02017] The priority value you provided is reserved. Provide a number between "100" and "9900".'
  actual:   Error.Code='NotImplemented'  (AssertionError: expected ValidationException, got 'NotImplemented')
  additionally asserted by the check: the rejected association must be absent from ListProfileResourceAssociations, and the same call with priority 150 must succeed and be readable via GetProfileResourceAssociation with priority==150.

Cause 2 (route53profiles.associate-resource-unsupported-arn-rejected):
  call: associate_resource_to_profile(ProfileId=<profile>, ResourceArn='arn:aws:s3:::<bucket>', Name=<name>)
  expected: Error.Code='ValidationException', HTTP 400, Message '[RSLVR-05022] The ARN in your request isn\'t valid:"<resourceArn>". Provide a valid ARN.'
  actual:   Error.Code='NotImplemented'  (AssertionError: expected a validation-style rejection for an unsupported resource ARN, got 'NotImplemented')
  additionally asserted: the rejected ARN must not appear in ListProfileResourceAssociations.
(The concrete bucket/profile/rule-group names in the replay are per-run generated values; the requirement is the template with the request's own ResourceArn.)

## Steps to Reproduce
Cause 1:
1. CreateProfile (route53profiles).
2. route53resolver CreateFirewallRuleGroup -> take its Arn.
3. AssociateResourceToProfile(ProfileId, ResourceArn=<rule group arn>, Name=<valid name>, ResourceProperties='{"priority": 1}').
4. Expect ValidationException / HTTP 400 / '[RSLVR-02017] The priority value you provided is reserved. Provide a number between "100" and "9900".'
5. ListProfileResourceAssociations(ProfileId) must not contain that ResourceArn.
6. Repeat step 3 with ResourceProperties='{"priority": 150}': must succeed; GetProfileResourceAssociation(<Id>) must report the same ResourceArn and priority 150.

Cause 2:
1. CreateProfile.
2. AssociateResourceToProfile(ProfileId, ResourceArn='arn:aws:s3:::some-bucket', Name=<valid name>).
3. Expect ValidationException / HTTP 400 / '[RSLVR-05022] The ARN in your request isn\'t valid:"arn:aws:s3:::some-bucket". Provide a valid ARN.'
4. ListProfileResourceAssociations(ProfileId) must not contain that ARN.
