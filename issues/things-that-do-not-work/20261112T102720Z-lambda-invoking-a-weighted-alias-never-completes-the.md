Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.alias-weighted-routing-splits-traffic
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.alias-weighted-routing-splits-traffic|run#19|Timeout|resp = lam.invoke(FunctionName=f"{fn}:{alias}", Payload=b"{}")
Issue-Commit: 80a4d1672cdd144a0f484dbb402fdb4ec503e852

# lambda: invoking a weighted alias never completes — the 50/50 AdditionalVersionWeights routing check makes no progress and is cut off by its time budget

> _Note: this issue was identified against the code base at commit `80a4d1672cdd144a0f484dbb402fdb4ec503e852`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a sequence of Invokes against an alias carrying RoutingConfig.AdditionalVersionWeights never completes, so weighted routing is not merely wrong, it is unobservable — the exercise is cut off by its time budget before any routing assertion is reached.
  AWS: CreateAlias/UpdateAlias accept RoutingConfig.AdditionalVersionWeights mapping a second version to a weight, and GetAlias echoes it (Smithy: lambda AliasRoutingConfiguration.AdditionalVersionWeights, map of Version -> Weight, on CreateAlias/UpdateAlias/GetAlias). Each Invoke of <function>:<alias> is answered promptly and independently: StatusCode 200, no FunctionError, and an ExecutedVersion header naming the version that actually ran — which alternates between the primary FunctionVersion and the additionally weighted version across repeated invokes, and the payload the handler returns is the code of the version ExecutedVersion names. Clearing RoutingConfig pins every subsequent Invoke back to the primary version. A run of a few dozen sequential Invokes of a 128 MB python3.12 function is seconds of work, not minutes.
  Emulator: the whole exercise never finishes. The replay reports only 'Error: Check timed out after 60.0s' — no response, no status, no ExecutedVersion — for a sequence that is CreateFunction + wait Active + PublishVersion + UpdateFunctionCode + PublishVersion + CreateAlias with a 0.5 weight + GetAlias + 40 sequential Invokes of <function>:<alias>. The alias invoke path makes no observable progress inside the budget, so whether AdditionalVersionWeights is honoured can never be read off the data plane at all.
  Why: a client doing a canary or blue/green deploy through a weighted alias gets no answer: it cannot shift a fraction of traffic to a new version, cannot read ExecutedVersion to attribute a response to a version, and cannot roll back by clearing the weights, because the invokes it issues do not return in bounded time. Any caller that invokes through an alias stalls rather than being served or refused.
  Fix: the Invoke handler's alias resolution and weighted-routing selection, together with the function-provisioning path that must bring a freshly published version to Active before an alias invoke can be served — one of these does not converge for an alias with AdditionalVersionWeights set. The emulator must answer each alias Invoke promptly, with 200 and an ExecutedVersion naming the version that ran, or refuse it with a modelled error.
  Related but NOT the same work item: open issues 20261101T210635Z-lambda-weighted-alias-traffic-is-never-observable-across.md, 20261108T003043Z-lambda-a-weighted-alias-s-invocations-are-almost.md and 20261109T002603Z-lambda-2-net-new-causes-loggingconfig-logformat-json.md all describe weighted-alias behaviour over a weight LADDER or invocations killed with Runtime.ExitError; none of them carries this check's replay key, and this check never reaches a routing assertion at all — it is cut off with no response.
  Traces: lambda.alias-weighted-routing-splits-traffic.

## Evidence
Check lambda.alias-weighted-routing-splits-traffic (verdict: mismatch)
  Error: Check timed out after 60.0s

No HTTP status, error code or ExecutedVersion was observed — the exercise produced no response to assert on.

Expected (AWS):
  - CreateAlias(FunctionName=fn, Name=alias, FunctionVersion=v1, RoutingConfig={AdditionalVersionWeights: {v2: 0.5}}) echoes that weight, and GetAlias returns FunctionVersion=v1 with the same AdditionalVersionWeights.
  - Each Invoke(FunctionName='<fn>:<alias>') returns StatusCode 200, no FunctionError, and ExecutedVersion in {v1, v2}; the returned payload is the code of the version ExecutedVersion names.
  - Across repeated invokes both v1 and v2 are reached.
  - After UpdateAlias(RoutingConfig={}), GetAlias shows no weights immediately and subsequent invokes report ExecutedVersion=v1 only.

Actual (emulator):
  - The sequence does not complete within the check's budget; nothing is answered, so none of the above can be read.

## Steps to Reproduce
1. Create a python3.12, 128 MB Lambda function whose handler returns a marker identifying its code.
2. Wait for State=Active, PublishVersion -> v1.
3. UpdateFunctionCode with a handler returning a different marker, wait for LastUpdateStatus=Successful, PublishVersion -> v2.
4. CreateAlias(Name='canary', FunctionVersion=v1, RoutingConfig={'AdditionalVersionWeights': {v2: 0.5}}) and GetAlias to confirm the weight round-trips.
5. Invoke '<function>:canary' repeatedly (a few dozen times), reading StatusCode, ExecutedVersion and the payload on each call.
6. Observe the sequence never completes — the invokes do not return in bounded time, so no ExecutedVersion distribution can be collected. On AWS every invoke answers promptly with 200 and an ExecutedVersion naming v1 or v2.
7. UpdateAlias(RoutingConfig={}) and invoke again: on AWS every invoke reports ExecutedVersion=v1.
