Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: location.route-calculator-rejects-unsupported-data-source
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|CreateRouteCalculator|ValidationException
Issue-Commit: 92a45844816178fadeed3fd2cf996315613ecee5

# CreateRouteCalculator's unsupported-DataSource ValidationException message renders the allowed values as a bracketed list instead of AWS's comma-separated sentence

> _Note: this issue was identified against the code base at commit `92a45844816178fadeed3fd2cf996315613ecee5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRouteCalculator

## Description
One net-new root cause in the location crate.

Cause 1 of 1: the ValidationException message for an out-of-set CreateRouteCalculator DataSource is formatted with the emulator's own debug-style list rendering rather than the service's sentence.

AWS: Smithy (services/service-com-amazonaws-location-locationservice/smithy-model/com-amazonaws-location-locationservice.json): CreateRouteCalculator POST /routes/v0/calculators; input member DataSource targets a plain String (NOT a modelled enum) and is required; the member's documentation states "This field is case-sensitive. Enter the valid values as shown" and lists the valid values Esri, Grab, Here. The operation models ValidationException (httpError 400). A DataSource outside that set is refused 400 ValidationException with the message "'DataSource' must be one of Esri, Grab, Here." — fixed text is "'", the member name as modelled, "' must be one of ", the allowed values joined by ", ", and a terminating period; the only variable part of the template is the member name and the allowed-value set drawn from the model/documentation, never a value echoed from the request. Nothing is created: a subsequent DescribeRouteCalculator for that name answers 404 ResourceNotFoundException.

Emulator: the 400 status and the ValidationException code are correct, but the message is "'DataSource' must be one of [Esri, Grab, Here]" — the allowed values are wrapped in square brackets (the crate's collection/Debug rendering) and the terminating period is missing. The replayed check asserted the message and failed with: AssertionError: unexpected message "'DataSource' must be one of [Esri, Grab, Here]".

Why: a client that surfaces or matches the refusal text (operator-facing error display, SDK-level wording assertions, log scraping, or a wrapper that re-parses the allowed-value list out of the message) sees text the service never emits; the bracket characters make the allowed-value list unparseable by anything expecting AWS's comma-separated sentence. Because this is a crate-wide "must be one of" formatter rather than a one-off string, every other location operation that constrains a member to a documented value set (DataSource on CreatePlaceIndex, Style on CreateMap, and similar) will emit the same bracketed form once those operations are reachable.

Fix: the location crate's shared validation/error-message formatting path that builds the "must be one of" sentence, as used by the CreateRouteCalculator handler's DataSource check — join the allowed values with ", " and terminate with a period, with no surrounding brackets. This is a distinct cause from the open issue 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit, which covers the generic resource/constraint templates for conflict, not-found and regex-pattern refusals and the missing ValidationException Reason/FieldList members; that issue says nothing about the value-set ("must be one of") wording asserted here, so it does not cover this check.

Traces: location.route-calculator-rejects-unsupported-data-source.

## Evidence
Check location.route-calculator-rejects-unsupported-data-source [mismatch]:
  CreateRouteCalculator(CalculatorName=<unique bad name>, DataSource="Bing")
  Status: 400 (matches AWS)
  Error code: ValidationException (matches AWS)
  Error message expected: "'DataSource' must be one of Esri, Grab, Here."
  Error message actual:   "'DataSource' must be one of [Esri, Grab, Here]"
  Failure: check_location_route_calculator_rejects_unsupported_data_source.py line 41 — AssertionError: unexpected message "'DataSource' must be one of [Esri, Grab, Here]"
Fixed text of the template: "'" + <member name> + "' must be one of " + <allowed values joined by ", "> + "."; variable parts are the member name (DataSource) and the allowed-value set from the model documentation (Esri, Grab, Here). The rejected request value ("Bing") is not echoed by AWS and is not echoed by the emulator either.
The rest of the check's expectations held on the emulator: the rejected calculator was never created (DescribeRouteCalculator answered ResourceNotFoundException) and CreateRouteCalculator with DataSource="Esri" succeeded and described back DataSource "Esri".

## Steps to Reproduce
1. Boot the emulator and point a botocore `location` client at it.
2. Call CreateRouteCalculator(CalculatorName="sim2real-behav-location-routecalc-bad-<suffix>", DataSource="Bing").
3. Inspect the ClientError: response["Error"]["Code"] is ValidationException and the HTTP status is 400 (both correct), but response["Error"]["Message"] is "'DataSource' must be one of [Esri, Grab, Here]" instead of "'DataSource' must be one of Esri, Grab, Here.".
4. Re-replay behavioral check location.route-calculator-rejects-unsupported-data-source to confirm the fix.
