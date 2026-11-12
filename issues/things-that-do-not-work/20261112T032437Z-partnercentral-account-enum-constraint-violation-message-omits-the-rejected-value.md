Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-account-partner-onboarding-attempt
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-account-partner-onboarding-attempt
Issue-Shape-Id: com.amazonaws.partnercentralaccount#PartnerCentralAccount
Issue-Key: partnercentral-account|CreatePartner|4xx->4xx|$.Message
Issue-Commit: 79457d2673f4c9d4b1868321841a2b6376250fb9

# Enum constraint violation message omits the rejected value: "Value at 'primarySolutionType'" instead of "Value 'NOT_A_SOLUTION_TYPE' at ..."

> _Note: this issue was identified against the code base at commit `79457d2673f4c9d4b1868321841a2b6376250fb9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePartner

## Description
Cause 1 of 1: the shared input-validation message formatter drops the offending value when the violated constraint is an enum value set, so an enum rejection is reported as "Value at '<member>' failed ..." with the quoted value missing. The same response's length violation for another member renders correctly, so only the enum branch of the formatter is affected.

AWS: AWS's generic request-validation refusal for this service is a 400 ValidationException whose Message is the standard constraint-violation summary: the fixed text "<N> validation errors detected: " followed by one clause per violated member, clauses joined by "; ". Each clause is the fixed template "Value '<value>' at '<memberPath>' failed to satisfy constraint: <constraint clause>", and for an enum member the constraint clause is the fixed text "Member must satisfy enum value set: [" + the modeled values + "]". The variable parts are taken from the request and the model: <N> is the number of violations, <value> is the value the client actually sent for that member, <memberPath> is the wire (lowerCamel) member name, and the bracketed list is the member's modeled enum values. Smithy: CreatePartner.PrimarySolutionType -> enum PrimarySolutionType, values [SOFTWARE_PRODUCTS, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, MANAGED_SERVICES, HARDWARE_PRODUCTS, COMMUNICATION_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, TRAINING_SERVICES]; CreatePartner.ClientToken @length min 1 max 64, pattern ^[A-Za-z0-9-_]+$; error ValidationException (HTTP 400, Message required). The fixed text of the template and the 400/ValidationException classification are what the recorded response shows.

Emulator: status 400 and ValidationException are correct, and the clientToken length clause is correct, but the enum clause is emitted as "Value at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [...]" — the "'<value>' " segment of the template is simply absent (the recorded run sent primarySolutionType=NOT_A_SOLUTION_TYPE and AWS echoes it; the emulator echoes nothing). Everything else in the Message, including the "2 validation errors detected: " prefix, the ordering and the bracketed value set, matches.

Why: clients and SDK test suites parse this Message to tell the operator WHICH value was rejected; an enum rejection that names no value cannot be acted on or asserted against, and any client that regex-extracts the quoted value from a constraint clause (a common pattern for surfacing field-level errors in UIs and for retry/repair logic) gets no match and either reports an empty field or treats the refusal as unparseable. It also makes the two clauses of one and the same response inconsistent in shape.

Fix: the shared request-validation error formatter — the enum-value-set branch of the constraint-violation clause builder used by the centralized deserialize -> validate step (the pre-handler validation walk, not CreatePartner's handler, which is still a scaffold stub and never runs for this request). The clause must interpolate the request's rejected value with the same "Value '<value>' at '<member>'" prefix the length branch already produces.

Traces: partnercentral-account-partner-onboarding-attempt#task#3 (CreatePartner, expected 400 / got 400, $.Message value_mismatch on the primarySolutionType clause).

## Evidence
Fixture partnercentral-account-partner-onboarding-attempt, step task#3, operation CreatePartner. Expected status 400, actual status 400 — the divergence is $.Message.

Expected (AWS recording):
  "2 validation errors detected: Value 'sim2real-probe-partnercentral-account-create-partner-bad-076ba00b' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64; Value 'NOT_A_SOLUTION_TYPE' at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [TRAINING_SERVICES, COMMUNICATION_SERVICES, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES]"

Actual (emulator):
  "2 validation errors detected: Value 'sim2real-probe-partnercentral-account-create-partner-bad-076ba00b' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 64; Value at 'primarySolutionType' failed to satisfy constraint: Member must satisfy enum value set: [TRAINING_SERVICES, COMMUNICATION_SERVICES, CONSULTING_SERVICES, PROFESSIONAL_SERVICES, VALUE_ADDED_RESALE_AWS_SERVICES, HARDWARE_PRODUCTS, SOFTWARE_PRODUCTS, MANAGED_SERVICES]"

Fixed vs variable in the required message: fixed text = "<N> validation errors detected: ", "Value '", "' at '", "' failed to satisfy constraint: ", "Member must satisfy enum value set: [", "]". Variable = N (violation count), the rejected value (from the request), the member name (wire name of the member), and the enum value list (from the model). The long client-token literal and the specific bad enum token above are this one run's request values, not part of the requirement.

No open issue covers this: the open partnercentral-account issues are the GetConnectionPreferences and ListPartners stub issues, and the open partnercentral-channel / partnercentral-benefits issues are other services or other operations.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture partnercentral-account-partner-onboarding-attempt.
2. At step task#3 the fixture calls partnercentral-account CreatePartner with two simultaneously invalid members: a clientToken longer than the modeled 64-character maximum, and primarySolutionType set to a token that is not in the PrimarySolutionType enum (recorded run used NOT_A_SOLUTION_TYPE).
3. Observe the 400 ValidationException body. The clientToken clause quotes the rejected value; the primarySolutionType clause reads "Value at 'primarySolutionType' failed to satisfy constraint: ..." with no quoted value.
4. Expected: the enum clause quotes the value the request sent for that member, exactly as the length clause does.
