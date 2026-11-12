Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item|run#24|KeyError|resource = json.loads(rule["Definition"])["Resource"]
Issue-Commit: 732d86be3641565fdf04b49c6a1d1231e9cf6723

# CloudWatch: a managed Contributor Insights rule enabled by PutManagedInsightRules has a Definition that omits the top-level "Resource" member, so DescribeInsightRules never shows which resource the rule is bound to

> _Note: this issue was identified against the code base at commit `732d86be3641565fdf04b49c6a1d1231e9cf6723`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutManagedInsightRules

## Description
Cause 1 of 1: PutManagedInsightRules enables the managed rule and DescribeInsightRules lists it (ManagedRule=true, State=ENABLED), but the rule's Definition JSON has no top-level "Resource" key naming the ARN the rule was enabled for, so a client cannot tell which Amazon Web Services resource a managed rule monitors.

AWS: Smithy (com.amazonaws.cloudwatch#GraniteServiceVersion20100801): InsightRule.Definition is required, @length 1..8192, @pattern ^[\x00-\x7F]+$ and is documented as 'the definition of the rule, as a JSON object'; InsightRule.Schema is documented as '{"Name": "CloudWatchLogRule", "Version": 1}' for rules you create and '{"Name": "ServiceLogRule", "Version": 1}' for managed rules, and ManagedRuleDescription.ResourceARN is 'the ARN for the related Amazon Web Services resource' when a managed rule is enabled. The specific content of a managed rule's Definition is not stated by the model — recorded behavior only: the recording shows that the Definition of a rule created by PutManagedInsightRules parses as a JSON object carrying a top-level member "Resource" whose value is exactly the ResourceARN the managed rule was enabled for (here the vpc-endpoint-service ARN passed in ManagedRules[].ResourceARN). The ARN is a request-derived value; the presence of the "Resource" member is the requirement.

Emulator: DescribeInsightRules returns the managed rule with a Definition that parses as JSON but contains no "Resource" member — the check's read of it raises KeyError: 'Resource'. Everything the check asserts before that point passes in this replay: the two bad TemplateNames come back as per-item Failures, exactly the valid template is ENABLED, and DescribeInsightRules reports the rule with ManagedRule=true and State=ENABLED. Only the Definition body diverges.

Why: a client that enables a managed Contributor Insights rule and then reads the rule back through DescribeInsightRules has no way to recover the resource binding from the rule definition — the association between the minted rule name and the monitored resource is lost. Tooling that reconciles managed rules against resources (or renders the rule definition) sees a definition that is not the one AWS stores, and code that indexes Definition["Resource"] fails outright instead of returning the ARN.

Fix: the PutManagedInsightRules handler / managed-rule state in the cloudwatch crate (the definition synthesised for a managed rule must carry the resource binding) and the DescribeInsightRules serializer that renders InsightRule.Definition.

Note: the open issue 20261102T004630Z-cloudwatch-publishes-no-managed-contributor-insights-rule-templates.md covers ListManagedInsightRules returning an empty ManagedRules list; in this replay templates ARE offered and the rule IS enabled, and that issue does not carry this check's replay key, so it does not cover this divergence.

Traces: cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item.

## Evidence
Behavioral suite replay against the emulator built from this checkout (521/924 passed).

Check cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item [error]:
  expected: after PutManagedInsightRules enables the offered template for a vpc-endpoint-service ARN, DescribeInsightRules returns that rule with ManagedRule=true, State=ENABLED and a Definition whose parsed JSON has Definition["Resource"] == the ResourceARN the rule was enabled for.
  actual:   the rule is listed and ENABLED, but its Definition JSON has no "Resource" member.
  runner output: KeyError: 'Resource' at `resource = json.loads(rule["Definition"])["Resource"]` (check_cloudwatch_put_managed_insight_rules_invalid_template_name_rejected_per_item.py, line 75 in run).

The ARN itself is a per-run value (arn:<partition>:ec2:<region>:<account>:vpc-endpoint-service/<service-id>); the requirement is that the managed rule's Definition carries the "Resource" binding at all, and that its value equals the ResourceARN supplied to PutManagedInsightRules.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from this checkout.
2. Replay check cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item.
3. Manual repro: create a VPC, a subnet, an internal network load balancer and a VPC endpoint service configuration on it; build the ARN arn:aws:ec2:<region>:<account>:vpc-endpoint-service/<ServiceId>. Call ListManagedInsightRules(ResourceARN=<arn>) and take ManagedRules[0].TemplateName; call PutManagedInsightRules(ManagedRules=[{TemplateName:<template>, ResourceARN:<arn>}]); take the minted RuleState.RuleName from ListManagedInsightRules and read it through DescribeInsightRules. Expect json.loads(Definition)["Resource"] == <arn>; observe a Definition with no "Resource" member.
