Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.op-contract-deleteInsightRules-dup
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.op-contract-deleteInsightRules-dup|run#15|AssertionError|assert failures2 == {malformed_name: "INVALID_RULE_NAME"}, (
Issue-Commit: 89057c52301fd36ae7e24000691cb660eaaa73a8

# CloudWatch DeleteInsightRules never reports a malformed rule name: a name containing a space answers 200 with an empty Failures[] instead of a per-name PartialFailure with FailureCode INVALID_RULE_NAME

> _Note: this issue was identified against the code base at commit `89057c52301fd36ae7e24000691cb660eaaa73a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInsightRules

## Description
One net-new root cause.

Cause 1 of 1: DeleteInsightRules does not classify a syntactically malformed Contributor Insights rule name. AWS answers HTTP 200 and reports that name per-item in Failures[] with FailureCode 'INVALID_RULE_NAME'; the emulator answers HTTP 200 with Failures=[] (an empty array), silently treating the malformed name as a no-op.
Failing check ids: cloudwatch.op-contract-deleteInsightRules-dup

AWS: Smithy (com.amazonaws.cloudwatch#GraniteServiceVersion20100801) models DeleteInsightRules with input RuleNames (@required, clientOptional, target InsightRuleNames) and output Failures -> BatchFailures, a list of PartialFailure{FailureResource, ExceptionType, FailureCode, FailureDescription}. PartialFailure is documented as the contract for this array: "This array is empty if the API operation was successful for all the rules specified in the request. If the operation could not process one of the rules, the following data is returned for each of those rules." Its members are documented FailureResource = "The specified rule that could not be deleted.", ExceptionType = "The type of error.", FailureCode = "The code of the error.", FailureDescription = "A description of the error." DeleteInsightRulesOutput$Failures is documented "An array listing the rules that could not be deleted." The operation's only modeled errors are InvalidParameterValueException (client, httpError 400, awsQueryError InvalidParameterValue) and MissingRequiredParameterException (400, awsQueryError MissingParameter) — so AWS does NOT refuse this request; the per-name report is the whole mechanism.
The model's member constraint is deliberately WIDER than the service's own rule-name syntax: InsightRuleName is a string with @length {min 1, max 128} and @pattern ^[\x20-\x7E]+$, and \x20 is the space character, so a name containing spaces satisfies the model and reaches the service. Beyond that the model says nothing about which printable-ASCII strings are legal rule names — recorded behavior only: a rule name containing a space is not a legal Contributor Insights rule name, and AWS reports it as one PartialFailure entry on an HTTP 200, keyed by FailureResource equal to the request's own member verbatim, with FailureCode exactly 'INVALID_RULE_NAME'. The sibling Enable/DisableInsightRules recordings (see open issue 20261101T102741Z-cloudwatch-enable-disableinsightrules-refuse-the-whole-batch-instead.md) show the vocabulary for this same failure class on the same PartialFailure shape: ExceptionType 'InvalidParameterException' paired with FailureCode 'INVALID_RULE_NAME', versus ExceptionType 'ResourceNotFoundException' / FailureCode 'RULE_NAME' for a well-formed name that simply names no rule. The fixed text here is the code vocabulary itself ('INVALID_RULE_NAME', 'InvalidParameterException'); the only variable part is FailureResource, which is the offending RuleNames member copied from the request.

Emulator: DeleteInsightRules(RuleNames=['bad rule name']) answers HTTP 200 with Failures=[] — the malformed name is accepted and discarded with no report at all. The surrounding legs of the same check pass, which localises the defect precisely: PutInsightRule creates the rule, DeleteInsightRules(RuleNames=[<existing rule>, <well-formed name that names no rule>]) correctly answers 200 with Failures=[] (a never-created name IS a silent no-op), and DescribeInsightRules correctly no longer lists the deleted rule. Only the malformed-name classification is missing. The handler builds its response as Ok(DeleteInsightRulesOutput{failures: Some(failures)}) from a single store walk that resolves names against the rule store, so a name that resolves to nothing — whether because it is unknown or because it is not a legal name at all — produces no entry, collapsing the two cases the PartialFailure vocabulary exists to separate.

Why: a client cannot tell "that rule name was never created, nothing to do" from "that string is not a legal rule name, your request is a bug". Both come back as an unqualified success. A reconciler or console that deletes a user-supplied or template-interpolated set of Contributor Insights rules records the malformed entry as successfully deleted and never surfaces the input error, so a typo (a space, a shell-quoting accident) is permanently swallowed instead of being reported on the one response AWS reports it on. Any client branching on Failures[].FailureCode == 'INVALID_RULE_NAME' has dead code against the emulator.

Fix: the DeleteInsightRules handler and the shared rule-name resolution walk in the cloudwatch crate's state. The per-name loop must distinguish three outcomes — name is legal and names a rule (delete it, no entry), name is legal and names no rule (no entry, a silent no-op), name is not a legal rule name (emit one PartialFailure{FailureResource: the member verbatim, ExceptionType 'InvalidParameterException', FailureCode 'INVALID_RULE_NAME'}) — with the whole call still answering 200. The legality check belongs in that per-name loop and must NOT be lifted into request-level model-constraint validation: refusing the request with a 400 would be the defect open issue 20261101T102741Z-cloudwatch-enable-disableinsightrules-refuse-the-whole-batch-instead.md already records for Enable/DisableInsightRules. That issue is NOT a substitute for this one: it carries only the Enable/DisableInsightRules check ids and names only enable_insight_rules.rs and disable_insight_rules.rs, and DeleteInsightRules fails the opposite way (it under-reports on a 200 rather than over-refusing with a 400).

Traces: cloudwatch.op-contract-deleteInsightRules-dup. Handler: services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/delete_insight_rules.rs (handle() delegates the whole walk to store.delete_insight_rules(&input.rule_names) at line 178 and wraps its result unchanged at lines 180-182); the walk itself lives in services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state.rs (delete_insight_rules), shared with the enable/disable operations as the handler's comment at lines 171-176 notes.

## Evidence
Replayed behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout: 786/1226 passed (64.1%); 364 mismatched, 76 errored. Service cloudwatch: 556/696 (80%), 140 failed. Of the whole report's failing checks, only two are marked NEITHER "ALREADY FILED" nor "OWNED by"; this is the cloudwatch one.

cloudwatch.op-contract-deleteInsightRules-dup — verdict 'mismatch'.

Setup (passes): cloudwatch:PutInsightRule RuleName=<r>, RuleDefinition={"Schema":{"Name":"CloudWatchLogRule","Version":1},"AggregateOn":"Count","Contribution":{"Filters":[],"Keys":["$.ip"]},"LogFormat":"JSON","LogGroupNames":[<generated>]}.

Passing legs (so the defect is localised to the malformed-name leg):
  * DeleteInsightRules(RuleNames=[<r>, <missing_name>]) where <missing_name> is well formed and names no rule -> HTTP 200, Failures=[] (correct: a never-created name is a silent no-op, not a reported failure).
  * DescribeInsightRules -> <r> is no longer listed (correct).

Failing leg:
  expected: DeleteInsightRules(RuleNames=['bad rule name']) -> HTTP 200 with Failures[] carrying exactly one entry, {FailureResource: 'bad rule name', FailureCode: 'INVALID_RULE_NAME'} (ExceptionType 'InvalidParameterException' per the recorded vocabulary of this failure class on the same PartialFailure shape).
  actual:   HTTP 200 with Failures=[].
  assertion: File "check_cloudwatch_op_contract_deleteInsightRules_dup.py", line 52, in run
             assert failures2 == {malformed_name: "INVALID_RULE_NAME"}, (
             AssertionError: expected the malformed name refused in Failures[] with INVALID_RULE_NAME, got Failures=[]

Message anatomy: there is no HTTP status or error-code divergence here — both sides answer 200 — so the requirement is the response BODY. 'INVALID_RULE_NAME' and 'InvalidParameterException' are fixed vocabulary strings, not per-run values. The only variable part of the expected entry is FailureResource, which must be the RuleNames member copied verbatim from the request ('bad rule name' in this run — the space inside it is what makes the name illegal; the literal string is the check's filler for the template, not part of the requirement).

Smithy citations used (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json):
  * DeleteInsightRules: input {RuleNames: target InsightRuleNames, required, clientOptional}; output {Failures: target BatchFailures}; errors InvalidParameterValueException (client, httpError 400, awsQueryError InvalidParameterValue) and MissingRequiredParameterException (400, awsQueryError MissingParameter). No ValidationError/ValidationException is modeled, and no error covers a bad rule name inside a batch.
  * DeleteInsightRulesOutput: {Failures: BatchFailures} — "An array listing the rules that could not be deleted. You cannot delete built-in rules."
  * PartialFailure: structure {FailureResource, ExceptionType, FailureCode, FailureDescription} — "This array is empty if the API operation was successful for all the rules specified in the request. If the operation could not process one of the rules, the following data is returned for each of those rules."
  * InsightRuleName: string, @length {min 1, max 128}, @pattern ^[\x20-\x7E]+$ — note \x20 is SPACE, so 'bad rule name' satisfies the model and must reach the handler rather than being refused by request-level validation.

Emulator source read for this finding: services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/delete_insight_rules.rs lines 141-183 — the handler validates only the empty-list case (lines 160-169) and then returns whatever the shared store walk produces (lines 177-182); nothing in that path can emit an INVALID_RULE_NAME entry.

## Steps to Reproduce
Boot the emulator from this checkout, then:

1. cloudwatch:PutInsightRule RuleName=<r>, RuleDefinition={"Schema":{"Name":"CloudWatchLogRule","Version":1},"AggregateOn":"Count","Contribution":{"Filters":[],"Keys":["$.ip"]},"LogFormat":"JSON","LogGroupNames":["<any>"]}.
2. Control that must keep working: cloudwatch:DeleteInsightRules RuleNames=[<r>, <m>] where <m> is well formed (matches ^[\x20-\x7E]+$, length 1..128) and names no rule -> HTTP 200 with Failures=[]. Then cloudwatch:DescribeInsightRules must no longer list <r>.
3. The divergence: cloudwatch:DeleteInsightRules RuleNames=['bad rule name'] (a name containing spaces — legal per the model's @pattern, illegal as a Contributor Insights rule name) -> expect HTTP 200 with Failures[] = exactly one entry {FailureResource: 'bad rule name', ExceptionType: 'InvalidParameterException', FailureCode: 'INVALID_RULE_NAME'}.
   OBSERVED: HTTP 200 with Failures=[] — the malformed name is silently discarded.
4. Control that must NOT regress while fixing this: the call must stay a 200. Do not lift the rule-name legality check into request-level model-constraint validation — a 400 ValidationError here would reproduce the defect already filed for Enable/DisableInsightRules in 20261101T102741Z-cloudwatch-enable-disableinsightrules-refuse-the-whole-batch-instead.md.
5. Control for the three-way split: cloudwatch:DeleteInsightRules RuleNames=[<r2 that exists>, <m that is well formed but unknown>, 'bad rule name'] -> HTTP 200, <r2> actually deleted (DescribeInsightRules no longer lists it), and Failures[] carrying exactly ONE entry, for 'bad rule name', with FailureCode 'INVALID_RULE_NAME'.

Re-replay cloudwatch.op-contract-deleteInsightRules-dup to confirm the fix.
