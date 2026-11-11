Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: location.distance-based-position-filtering-suppresses-near-duplicate
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location.distance-based-position-filtering-suppresses-near-duplicate|run#18|AssertionError|assert positions == [far_point_1, far_point_2], (
Issue-Commit: 99b989e3b3177988cf3e5b7648fdb6911919cb86

# Location tracker PositionFiltering is stored but never applied to writes: a DistanceBased tracker records a near-duplicate update AWS drops

> _Note: this issue was identified against the code base at commit `99b989e3b3177988cf3e5b7648fdb6911919cb86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateDevicePosition

## Description
One net-new root cause in the Amazon Location tracking data plane.

Cause 1 of 1: BatchUpdateDevicePosition ignores the tracker's PositionFiltering mode, so every in-domain update is stored and later served by GetDevicePositionHistory / GetDevicePosition, including the updates the real service filters out.

AWS: the Smithy model (services/service-com-amazonaws-location-locationservice/smithy-model/com-amazonaws-location-locationservice.json) documents the filter on BatchUpdateDevicePosition itself: "Position updates are handled based on the PositionFiltering property of the tracker. ... When PositionFiltering is set to DistanceBased filtering, location data is stored and evaluated against linked geofence collections only if the device has moved more than 30 m (98.4 ft)." CreateTrackerRequest$PositionFiltering repeats it per mode — TimeBased: at most one stored position per unique DeviceId per 30 s interval; DistanceBased: "If the device has moved less than 30 m (98.4 ft), location updates are ignored. Location updates within this area are neither evaluated against linked geofence collections, nor stored."; AccuracyBased: an update is ignored when the device has moved less than the sum of the two consecutive horizontal accuracies, with a default of {"Horizontal": 0} when Accuracy is absent. PositionFiltering is an enum of exactly TimeBased | DistanceBased | AccuracyBased, defaulting to TimeBased when the member is omitted. Filtering is silent: the dropped entry is NOT reported in BatchUpdateDevicePositionResponse$Errors — the call still answers HTTP 200 with an empty Errors list, it simply does not persist the update, and the device's last known position and its history stay at the previously stored sample.

Emulator: PositionFiltering is accepted by CreateTracker and echoed by DescribeTracker, but the write path never consults it. services/service-com-amazonaws-location-locationservice/src/ops/batch_update_device_position.rs validates each entry's coordinate (crate::state::wgs84_position) and then unconditionally calls tracker.record_position(sample) for every accepted entry; the module header even states the gap ("NOT enforced here ... the tracker's PositionFiltering mode ... is not applied to writes, so every in-domain update is stored where the real service would drop some of them"). Replay: a tracker created with PositionFiltering=DistanceBased received three updates for one device — a first point, a second ~1.5 m away, and a third ~10 km away. GetDevicePositionHistory returned all three positions ([[-122.3321, 47.6062], [-122.332101, 47.606201], [-122.4321, 47.7062]]) where AWS returns only the two that are more than 30 m apart ([[-122.3321, 47.6062], [-122.4321, 47.7062]]). The within-30 m sample was both stored and served.

Why: a client that selects a filtering mode to control cost, de-noise GPS jitter and bound history size gets unfiltered data. Position history is longer than AWS would ever return, so pagination counts, trajectory rendering and any de-duplication a caller relies on the service to perform all diverge; GetDevicePosition reports a sub-threshold jitter sample as the device's last known position; and because a filtered update is also not evaluated against linked geofence collections, geofence ENTER/EXIT evaluation fires on updates AWS suppresses. Nothing in the response signals the difference (Errors is empty either way), so the client cannot detect or compensate for it.

Fix: the BatchUpdateDevicePosition handler's write path — before recording an accepted entry, read the named tracker's stored PositionFiltering and compare the entry against that device's last stored sample: drop (without an Errors entry) a DistanceBased entry whose great-circle distance from the last stored position is under 30 m, a TimeBased entry whose SampleTime is under 30 s after the last stored sample for that DeviceId, and an AccuracyBased entry whose movement is under the sum of the previous and current horizontal accuracies (absent Accuracy defaulting to horizontal 0). The default when PositionFiltering was never supplied at CreateTracker/UpdateTracker is TimeBased, so the state's tracker record must carry the effective mode. The same predicate must gate geofence evaluation, not just persistence.

Traces: location.distance-based-position-filtering-suppresses-near-duplicate.

## Evidence
Check location.distance-based-position-filtering-suppresses-near-duplicate (mismatch):

  AssertionError: expected only the two far positions (near-duplicate filtered), got [[-122.3321, 47.6062], [-122.332101, 47.606201], [-122.4321, 47.7062]]

Expected (AWS): CreateTracker(PositionFiltering="DistanceBased"); three BatchUpdateDevicePosition calls for one DeviceId at P1, P1+~1.5 m, and P1+~10 km; each call answers 200 with Errors=[]. GetDevicePositionHistory then returns exactly two DevicePositions, P1 and the ~10 km point — the sub-30 m sample is silently not stored.

Actual (emulator): all three calls answer 200 with Errors=[], and GetDevicePositionHistory returns three DevicePositions including the ~1.5 m near-duplicate. GetDevicePosition likewise reports the near-duplicate as the last known position until the far update arrives.

Code: services/service-com-amazonaws-location-locationservice/src/ops/batch_update_device_position.rs lines 256-261 record every entry that passed coordinate validation (`for sample in accepted { tracker.record_position(sample); }`) with no reference to the tracker's PositionFiltering; lines 11-15 of the same file document the omission.

Model citations: Smithy BatchUpdateDevicePosition operation documentation ("Position updates are handled based on the PositionFiltering property of the tracker ... DistanceBased ... only if the device has moved more than 30 m (98.4 ft)"); CreateTrackerRequest$PositionFiltering documentation (per-mode rules, default TimeBased); enum PositionFiltering values TimeBased | DistanceBased | AccuracyBased.

## Steps to Reproduce
1. CreateTracker(TrackerName=<unique>, PositionFiltering="DistanceBased").
2. BatchUpdateDevicePosition(TrackerName=<tracker>, Updates=[{DeviceId: D, SampleTime: now, Position: [lon, lat]}]) — call it P1. Poll GetDevicePosition until it reports P1.
3. Wait ~5 s, then BatchUpdateDevicePosition with the same DeviceId and a Position under 30 m from P1 (e.g. +0.000001 on each ordinate).
4. Wait ~5 s, then BatchUpdateDevicePosition with the same DeviceId and a Position ~10 km from P1. Poll GetDevicePosition until it reports that point.
5. GetDevicePositionHistory(TrackerName=<tracker>, DeviceId=D) and list the Position of each DevicePosition.
   AWS: two entries — P1 and the ~10 km point.
   Emulator: three entries — the sub-30 m sample is present.
6. Repeat with PositionFiltering="TimeBased" sending two updates under 30 s apart, and with "AccuracyBased" sending a second update whose movement is under the summed horizontal accuracies: the emulator stores both in each case.
