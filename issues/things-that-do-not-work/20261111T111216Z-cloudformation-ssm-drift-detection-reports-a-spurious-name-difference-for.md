Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.stack-detect-drift-logical-subset-checks-only-named-resources cloudformation__ssm.change-set-revert-drift-restores-an-out-of-band-parameter-change
Issue-Key: cloudformation__ssm.stack-detect-drift-logical-subset-checks-only-named-resources|run#10|AssertionError|assert drifts == {"A": ("MODIFIED", [("/Value", "orig-a", "changed", "NOT_EQUAL")])}, (
Issue-Key: cloudformation__ssm.change-set-revert-drift-restores-an-out-of-band-parameter-change|run#20|AssertionError|assert targets == [("Value", "drifted", "ACTUAL_STATE", "template", "TEMPLATE")], targets
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Drift detection reports a spurious Name difference for every AWS::SSM::Parameter

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackResourceDrift

## Description
Cause 1 of 1: the actual state the drift detector reads for an SSM parameter's Name differs from the template value only by the leading '/', so Name is always reported NOT_EQUAL.

AWS: SSM parameter names are compared after SSM's own normalization — a parameter declared in a template as '/<name>' is stored and returned by GetParameter/DescribeParameters with that exact name, so CloudFormation drift detection finds the Name property IN_SYNC and reports only the properties that genuinely differ. For a parameter whose Value was changed out of band, DetectStackResourceDrift returns StackResourceDriftStatus MODIFIED with exactly one PropertyDifference: PropertyPath '/Value', ExpectedValue the template value, ActualValue the out-of-band value, DifferenceType NOT_EQUAL.

Emulator: two property differences are reported — the genuine /Value one plus a bogus /Name one in which the two sides are the same name with and without the leading slash ('/sim2real-cfn-subdrift-a-...' vs 'sim2real-cfn-subdrift-a-...'; in the change-set view, ACTUAL_STATE 'sim2real-xsvc-csdrift-...' against TEMPLATE '/sim2real-xsvc-csdrift-...'). The same spurious entry appears whether drift is read through DetectStackDrift with a LogicalResourceIds subset or through a change-set's revert-drift preview.

Why: drift detection exists to tell an operator exactly what changed. A property that is always reported as drifted makes every SSM-parameter stack permanently 'MODIFIED', so real drift is buried; a revert-drift change set additionally proposes to rewrite the parameter's Name, i.e. to replace the parameter, for a difference that does not exist.

Fix: the CloudFormation drift-detection comparison for AWS::SSM::Parameter (the actual-state reader must normalize the parameter name the same way SSM does before comparing it to the template).

Traces: the two failing checks in trace_ids.

## Evidence
Expected (subset drift): {'A': ('MODIFIED', [('/Value', 'orig-a', 'changed', 'NOT_EQUAL')])}.
Actual: {'A': ('MODIFIED', [('/Name', '/sim2real-cfn-subdrift-a-4f71c8a29a33', 'sim2real-cfn-subdrift-a-4f71c8a29a33', 'NOT_EQUAL'), ('/Value', 'orig-a', 'changed', 'NOT_EQUAL')])}.
Expected (revert-drift change set): [('Value', 'drifted', 'ACTUAL_STATE', 'template', 'TEMPLATE')].
Actual: [('Name', 'sim2real-xsvc-csdrift-2c67e0e33b0f', 'ACTUAL_STATE', '/sim2real-xsvc-csdrift-2c67e0e33b0f', 'TEMPLATE'), ('Value', 'drifted', 'ACTUAL_STATE', 'template', 'TEMPLATE')].

## Steps to Reproduce
1. CreateStack a template with AWS::SSM::Parameter resources whose Name starts with '/'.
2. PutParameter out of band to change only one parameter's Value.
3. Call DetectStackDrift (optionally with LogicalResourceIds for a subset) and DescribeStackResourceDrifts, and create a change set to revert the drift: both report an extra /Name NOT_EQUAL difference whose two sides differ only by the leading slash.
