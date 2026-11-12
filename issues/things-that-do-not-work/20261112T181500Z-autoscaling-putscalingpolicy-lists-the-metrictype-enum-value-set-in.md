Issue-Source: fixer-residual-divergence
Issue-Trace: asg-err-target-tracking-metric-type
Issue-Step: task#8
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asg-err-target-tracking-metric-type
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|PutScalingPolicy|4xx->4xx|$.ErrorResponse.Error.Message

# autoscaling PutScalingPolicy lists the MetricType enum value set in an order AWS does not state (1 cause)

> _Note: this issue was filed by the fixer of
> `20261111T102954Z-autoscaling-describepolicies-drops-defaulted-scaling-policy-members-and.md`,
> which cleared every other divergence of this fixture's cluster. It is the ONE divergence
> that fixture still has, and it belongs to no open issue._

## Operation
PutScalingPolicy

## Description
Cause 1 of 1 — the `ValidationException` envelope for a `PredefinedMetricType` outside
`com.amazonaws.autoscaling#MetricType` matches AWS in status (400), error code
(`ValidationError`), member path and echoed value, and differs only in the tail of its
requirement clause: the bracketed enum value set.

AWS: recorded behavior only. The recording's message begins
`1 validation error detected: Value 'ASGBogusMetric' at
'targetTrackingConfiguration.predefinedMetricSpecification.predefinedMetricType' failed to
satisfy constraint: Member must satis…` and the replay WITHHOLDS the rest, which is where
the two differ.

Emulator: the same envelope, with the clause
`Member must satisfy enum value set: [ASGAverageCPUUtilization, ASGAverageNetworkIn,
ASGAverageNetworkOut, ALBRequestCountPerTarget]` — the framework's own rendering, which
lists the values in the order the model declares them
(`smithy_query(query="shape",shape="MetricType")`).

Why: a client that parses the admitted set out of the message, or an assertion that pins
the sentence, sees a set AWS does not state.

Fix: the ORDER (or, if the clause differs in some other way, the clause) a refusal of a
value outside an enumerated type states, reworded for this service through
`Service::override_refusal` and `service_common::validation_message` — which already
carries the three orders AWS is recorded stating a value set in
(`in_sorted_enum_order`, `in_hashed_enum_order`, `in_stated_enum_order`). A fixer needs a
recording whose MESSAGE IS VISIBLE past the point of divergence: the three orders above
were each tried against this fixture and none cleared it, and the replay's display
truncates identically for all three, so the evidence available through the replay cannot
distinguish them. Nothing should be guessed into the emulator for it.

## Evidence
asg-err-target-tracking-metric-type#task#8, PutScalingPolicy, 400 vs 400:
  $.ErrorResponse.Error.Message: value_mismatch past
  `… failed to satisfy constraint: Member must satis` (the recorded remainder is withheld
  by the replay); the status, the `<Code>`, the member path and the echoed value all match.

## Steps to Reproduce
1. `CreateAutoScalingGroup` for any group.
2. `PutScalingPolicy` with `PolicyType=TargetTrackingScaling`,
   `TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType` set to
   a value the `MetricType` enum does not carry (`ASGBogusMetric`), and any in-range
   `TargetTrackingConfiguration.TargetValue`.
3. Read the `<Message>` of the 400 `ValidationError`: the bracketed set after
   `Member must satisfy enum value set: ` is in the model's declaration order.
