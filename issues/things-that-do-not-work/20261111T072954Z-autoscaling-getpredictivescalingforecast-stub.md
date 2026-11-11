Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.get-predictive-scaling-forecast-returns-forecast autoscaling.get-predictive-scaling-forecast-server-computed autoscaling.predictive-scaling-forecast-empty-before-data-exists autoscaling.predictive-scaling-forecast-load-capacity-consistency autoscaling.predictive-scaling-forecast-reflects-updated-policy autoscaling.predictive-scaling-forecast-respects-time-window autoscaling.predictive-scaling-forecast-timestamp-value-alignment autoscaling.predictive-scaling-forecast-values-consistent-across-calls autoscaling.predictive-scaling-policy-and-forecast
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: auto-scaling|GetPredictiveScalingForecast|NotImplemented
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# Implement autoscaling GetPredictiveScalingForecast: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPredictiveScalingForecast

## Description
`GetPredictiveScalingForecast` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/get_predictive_scaling_forecast.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `GetPredictiveScalingForecast` call, so each one unblocks only when `GetPredictiveScalingForecast` works: `autoscaling.get-predictive-scaling-forecast-returns-forecast`, `autoscaling.get-predictive-scaling-forecast-server-computed`, `autoscaling.predictive-scaling-forecast-empty-before-data-exists`, `autoscaling.predictive-scaling-forecast-load-capacity-consistency`, `autoscaling.predictive-scaling-forecast-reflects-updated-policy`, `autoscaling.predictive-scaling-forecast-respects-time-window`, `autoscaling.predictive-scaling-forecast-timestamp-value-alignment`, `autoscaling.predictive-scaling-forecast-values-consistent-across-calls`, `autoscaling.predictive-scaling-policy-and-forecast`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/get_predictive_scaling_forecast.rs`. Replaying the corpus, the emulator refused the `GetPredictiveScalingForecast` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_autoscaling_get_predictive_scaling_forecast_returns_forecast.py", line 57, in run
    raise AssertionError(f"forecast unavailable: {exc.response['Error']}") from exc
AssertionError: forecast unavailable: {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation GetPredictiveScalingForecast is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the autoscaling emulator
Call GetPredictiveScalingForecast and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
