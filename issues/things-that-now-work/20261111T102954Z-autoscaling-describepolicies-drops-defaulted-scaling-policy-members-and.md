Issue-Source: trace-sweep-divergence
Issue-Trace: asg-err-scaling-policy-adjustments
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asg-err-scaling-policy-adjustments autoscaling-op-contract-describe-policies asg-err-target-tracking-metric-type autoscaling-step-scaling-and-predictive-scaling-policy-defaults autoscaling-target-tracking-target-value-decimal-precision-preserved
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.StepAdjustments
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.TargetTrackingConfiguration.DisableScaleIn
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.TargetTrackingConfiguration.TargetValue
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.MetricAggregationType
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.StepAdjustments.member.MetricIntervalLowerBound
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.PredictiveScalingConfiguration.MaxCapacityBreachBehavior
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.PredictiveScalingConfiguration.MetricSpecifications.member.TargetValue
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.PredictiveScalingConfiguration.Mode
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# autoscaling DescribePolicies drops defaulted scaling-policy members and serializes doubles as integers (5 causes)

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePolicies

## Description
The DescribePolicies read projection is incomplete in the same way across every policy type: members AWS always serializes (empty collections and server-applied defaults) are omitted entirely, and modelled double members are serialized as integers when their value happens to be integral. 5 net-new causes, all on autoscaling DescribePolicies. The autoscaling Smithy model is not present in this checkout (the repo is a multi-service root and no services/service-com-amazonaws-autoscaling-autoscaling/smithy-model/*.json exists), so each cause below is grounded in recorded behavior only; the fixed text is named per cause.

Cause 1 of 5 — the empty StepAdjustments member is omitted for non-step policies.
AWS: recorded behavior only. The recorded DescribePolicies response carries ScalingPolicies[].StepAdjustments for every policy, serialized as an empty member list (the query-protocol empty-list form) when the policy is not a StepScaling policy.
Emulator: the member is absent from the projection (expected '', got None).
Why: a client that reads policy.step_adjustments gets an absent member rather than an empty list, so SDKs that distinguish 'absent' from 'empty' (and any code that iterates the list without a None guard) behave differently against the emulator than against AWS.
Fix: the DescribePolicies serializer for the ScalingPolicy shape — always emit StepAdjustments, empty when the policy has none.
Traces: asg-err-scaling-policy-adjustments#task#14 (also autoscaling-op-contract-describe-policies#task#6).

Cause 2 of 5 — TargetTrackingConfiguration loses the DisableScaleIn default and renders TargetValue as an integer.
AWS: recorded behavior only. For a target-tracking policy created without DisableScaleIn, the recording serializes ScalingPolicies[].TargetTrackingConfiguration.DisableScaleIn as 'false' (the server-applied default) and TargetValue as a double, '50.0'.
Emulator: DisableScaleIn is omitted (expected 'false', got None) and TargetValue is serialized '50' — the trailing '.0' is dropped. StepAdjustments is omitted as in cause 1.
Why: the default is lost, so a client cannot tell whether scale-in is disabled without re-reading its own request, and the integer rendering of a double member makes round-tripping a policy value lossy for strict parsers.
Fix: the DescribePolicies serializer / policy state defaults — materialize DisableScaleIn=false at PutScalingPolicy time and serialize double members with their fractional part.
Traces: asg-err-target-tracking-metric-type#task#10.

Cause 3 of 5 — a StepScaling policy loses the MetricAggregationType default and renders MetricIntervalLowerBound as an integer.
AWS: recorded behavior only. A step policy created without MetricAggregationType reads back with MetricAggregationType 'Average', and StepAdjustments[].MetricIntervalLowerBound is a double, '0.0'.
Emulator: MetricAggregationType is omitted (expected 'Average', got None) and MetricIntervalLowerBound is '0'.
Why: a client that echoes a described policy back into PutScalingPolicy (the documented update idiom) silently changes the aggregation type, because the member it read was absent.
Fix: the PutScalingPolicy state defaults plus the DescribePolicies serializer's double formatting.
Traces: autoscaling-step-scaling-and-predictive-scaling-policy-defaults#task#6.

Cause 4 of 5 — PredictiveScalingConfiguration loses its Mode and MaxCapacityBreachBehavior defaults and renders TargetValue as an integer.
AWS: recorded behavior only. A predictive-scaling policy created without them reads back with PredictiveScalingConfiguration.Mode 'ForecastOnly' and MaxCapacityBreachBehavior 'HonorMaxCapacity', and MetricSpecifications[].TargetValue as '30.0'.
Emulator: both defaults are omitted (got None) and TargetValue is '30'; StepAdjustments is omitted as in cause 1.
Why: predictive scaling's two most consequential switches (whether it only forecasts, and whether it may breach max capacity) are invisible to a client reading the policy back.
Fix: the PutScalingPolicy state defaults for PredictiveScalingConfiguration plus the DescribePolicies serializer.
Traces: autoscaling-step-scaling-and-predictive-scaling-policy-defaults#task#8.

Cause 5 of 5 — the same omissions on a target-tracking policy whose TargetValue carries real decimal precision.
AWS: recorded behavior only. The described policy carries DisableScaleIn 'false' and an (empty) StepAdjustments member.
Emulator: both are omitted (got None).
Why: same as causes 1 and 2 — a client cannot read back the effective policy.
Fix: the DescribePolicies serializer / policy state defaults.
Traces: autoscaling-target-tracking-target-value-decimal-precision-preserved#task#6.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — asg-err-scaling-policy-adjustments#task#14, DescribePolicies, 200 vs 200:
  $.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies.member.StepAdjustments: missing (expected '', got None)

Cause 2 — asg-err-target-tracking-metric-type#task#10, DescribePolicies, 200 vs 200:
  ...ScalingPolicies.member.TargetTrackingConfiguration.DisableScaleIn: missing (expected 'false', got None)
  ...ScalingPolicies.member.TargetTrackingConfiguration.TargetValue: value_mismatch (expected '50.0', got '50')
  ...ScalingPolicies.member.StepAdjustments: missing (expected '', got None)

Cause 3 — autoscaling-step-scaling-and-predictive-scaling-policy-defaults#task#6, DescribePolicies, 200 vs 200:
  ...ScalingPolicies.member.MetricAggregationType: missing (expected 'Average', got None)
  ...ScalingPolicies.member.StepAdjustments.member.MetricIntervalLowerBound: value_mismatch (expected '0.0', got '0')

Cause 4 — autoscaling-step-scaling-and-predictive-scaling-policy-defaults#task#8, DescribePolicies, 200 vs 200:
  ...ScalingPolicies.member.StepAdjustments: missing (expected '', got None)
  ...ScalingPolicies.member.PredictiveScalingConfiguration.Mode: missing (expected 'ForecastOnly', got None)
  ...ScalingPolicies.member.PredictiveScalingConfiguration.MetricSpecifications.member.TargetValue: value_mismatch (expected '30.0', got '30')
  ...ScalingPolicies.member.PredictiveScalingConfiguration.MaxCapacityBreachBehavior: missing (expected 'HonorMaxCapacity', got None)

Cause 5 — autoscaling-target-tracking-target-value-decimal-precision-preserved#task#6, DescribePolicies, 200 vs 200:
  ...ScalingPolicies.member.TargetTrackingConfiguration.DisableScaleIn: missing (expected 'false', got None)
  ...ScalingPolicies.member.StepAdjustments: missing (expected '', got None)

No open issue owns these steps (the replay reported owned_by empty for all five clusters).

## Steps to Reproduce
1. Replay the wire corpus: the five fixtures above each create a scaling policy and then call DescribePolicies.
2. Cause 1/5: PutScalingPolicy with PolicyType TargetTrackingScaling, then DescribePolicies — observe no StepAdjustments member.
3. Cause 2: create a target-tracking policy with TargetValue 50 and no DisableScaleIn, then DescribePolicies — observe no DisableScaleIn and TargetValue '50'.
4. Cause 3: create a StepScaling policy with one step adjustment and no MetricAggregationType, then DescribePolicies — observe no MetricAggregationType and MetricIntervalLowerBound '0'.
5. Cause 4: create a PredictiveScaling policy with no Mode/MaxCapacityBreachBehavior, then DescribePolicies — observe both members absent and TargetValue '30'.
