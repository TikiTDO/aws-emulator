Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-domain-validation-and-notfound-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-domain-validation-and-notfound-errors cloudwatchomni-space-and-access-control-errors
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|CreateDomain|4xx->4xx|$.message
Issue-Key: cloudwatchomni|CreateAccessGrant|4xx->4xx|$.message
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudwatchomni ValidationException message echoes value and unordered enum

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
The emulator's constraint-violation renderer builds the AWS "N validation error detected" message incorrectly: it splices the offending input value into the clause where AWS omits it. This is ONE shared fault in the shared validation-message formatter, with TWO causes; the second cause additionally renders the enum value set in the wrong order. Status (400) and error code (ValidationException) are correct in both; only `message` diverges.

AWS (common): Smithy com.amazonaws.cloudwatchomni#ValidationException, errorKind client, @httpError 400, required member `message`, doc "A parameter is specified incorrectly." (a service-common error on every operation). The message text itself is recorded behavior only; the recordings show the AWS template as "<n> validation error detected: Value at '<memberName>' failed to satisfy constraint: <constraint clause>" — the fixed parts are "<n> validation error detected: Value at '", "' failed to satisfy constraint: ", and the constraint clause; the variables are the error count, the member name, and the constraint's own limit/value-set, all taken from the model and the request. Crucially the offending VALUE is NOT part of the AWS template for these members.

Cause 1 of 2 — CreateDomain, `name` length constraint (fixture cloudwatchomni-domain-validation-and-notfound-errors, step task#1).
  AWS: 400 ValidationException, message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length greater than or equal to 3". Smithy: CreateDomainInput.name is required, @length min 3 max 63, @pattern ^[a-z0-9]+(-[a-z0-9]+)*$ — so 3 in the clause is the model's min, and 'name' is the member name; the submitted value does not appear.
  Emulator: "1 validation error detected: Value 'ab' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 3" — it inserts "Value '<submitted value>' at" where AWS has "Value at". ('ab' is the recorded run's input, not part of the requirement; the defect is that ANY value is interpolated at all.)
  Why: clients that surface or assert on `message` see text AWS never emits, and the emulator reflects caller-supplied input back in the error, which AWS deliberately withholds for this member.
  Fix: the shared constraint-violation message formatter used by CreateDomain's input validation — drop the value clause for these members.

Cause 2 of 2 — CreateAccessGrant, `permission` enum constraint (fixture cloudwatchomni-space-and-access-control-errors, step task#7).
  AWS: 400 ValidationException, message "1 validation error detected: Value at 'permission' failed to satisfy constraint: Member must satisfy enum value set: [CUSTOM, READ_WRITE_DELETE, READ, SPACE_ADMIN]". Smithy: CreateAccessGrantInput.permission is required and targets enum AccessGrantPermission. Two requirements here: (a) as in cause 1, the offending value is omitted; (b) the value set is rendered in the order CUSTOM, READ_WRITE_DELETE, READ, SPACE_ADMIN — which is NOT the model's declaration order (SPACE_ADMIN, READ, READ_WRITE_DELETE, CUSTOM) but its reverse. The rendering order is recorded behavior only; the member names/values are from the model.
  Emulator: "1 validation error detected: Value 'BOGUS_PERM' at 'permission' failed to satisfy constraint: Member must satisfy enum value set: [SPACE_ADMIN, READ, READ_WRITE_DELETE, CUSTOM]" — it both interpolates the rejected value and lists the enum in model declaration order.
  Why: an exact-match comparison on the validation message fails on two counts; a client that parses the bracketed set to offer valid choices gets them in an order AWS never returns, so golden-output comparisons diverge on every enum validation failure in the service.
  Fix: the shared constraint-violation message formatter (enum-set rendering order and the value clause), reached from CreateAccessGrant's input validation.

Not covered by any existing open issue: the two open cloudwatchomni issues are unimplemented-stub reports for CreateIntegration and ListDomains.

Traces: cloudwatchomni-domain-validation-and-notfound-errors#task#1 (CreateDomain; cluster also covers #task#2); cloudwatchomni-space-and-access-control-errors#task#7 (CreateAccessGrant).

## Evidence
Cause 1 CreateDomain — status 400 both sides; $.message expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length greater than or equal to 3", got "1 validation error detected: Value 'ab' at 'name' failed to satisfy constraint: Member must have length greater than or equal to 3". The only difference is the extra "'ab' " value clause. Cluster of 2 steps: task#1, task#2. Smithy: CreateDomainInput.name @length min 3 max 63.
Cause 2 CreateAccessGrant — status 400 both sides; $.message expected "1 validation error detected: Value at 'permission' failed to satisfy constraint: Member must satisfy enum value set: [CUSTOM, READ_WRITE_DELETE, READ, SPACE_ADMIN]", got "1 validation error detected: Value 'BOGUS_PERM' at 'permission' failed to satisfy constraint: Member must satisfy enum value set: [SPACE_ADMIN, READ, READ_WRITE_DELETE, CUSTOM]". Two differences: the interpolated value clause, and the enum set order (AWS reverses the model declaration order SPACE_ADMIN, READ, READ_WRITE_DELETE, CUSTOM). Smithy: CreateAccessGrantInput.permission -> enum AccessGrantPermission values [SPACE_ADMIN, READ, READ_WRITE_DELETE, CUSTOM]; ValidationException @httpError 400.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Inspect fixture cloudwatchomni-domain-validation-and-notfound-errors step task#1 (CreateDomain with a `name` shorter than the modeled @length min of 3) and fixture cloudwatchomni-space-and-access-control-errors step task#7 (CreateAccessGrant with a `permission` outside the AccessGrantPermission enum).
4. Both return 400 ValidationException with the correct code but a message that adds a "Value '<input>' at" clause AWS omits; the enum case additionally lists the value set in model declaration order rather than AWS's order.
