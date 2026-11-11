Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: location.get-map-tile-enforces-per-style-zoom-bound
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|GetMapTile|NotImplemented
Issue-Commit: 99b989e3b3177988cf3e5b7648fdb6911919cb86

# Implement location GetMapTile: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `99b989e3b3177988cf3e5b7648fdb6911919cb86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMapTile

## Description
`GetMapTile` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-location-locationservice/src/ops/get_map_tile.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetMapTile` call, so each one unblocks only when `GetMapTile` works: `location.get-map-tile-enforces-per-style-zoom-bound`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-location-locationservice/src/ops/get_map_tile.rs`. Replaying the corpus, the emulator refused the `GetMapTile` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_location_get_map_tile_enforces_per_style_zoom_bound.py", line 45, in run
    assert err["Code"] == "ValidationException", f"expected ValidationException, got {err['Code']!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got 'NotImplemented'

## Steps to Reproduce
Boot the location emulator
Call GetMapTile and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
