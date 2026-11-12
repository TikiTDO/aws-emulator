Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# A Snowball job ordered with RemoteManagement enabled never produces a managed device in the Snow Device Management inventory

_Filed: 2026-10-24T20:07:11Z_
_Operation: ListDevices | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Snow Device Management models no device-creation operation because a device becomes remotely manageable by being ORDERED: a Snow Family job created through the Snowball API with `RemoteManagement` set to `INSTALLED_AUTOSTART` or `INSTALLED_ONLY`. Once such a job exists, the device it ships appears in Snow Device Management with `DeviceSummary$associatedWithJob` carrying that job's `JobId`, and `ListDevices?jobId=<JobId>` narrows the inventory to the devices of that job.

The emulator wires none of that. The Snowball emulator persists `remote_management` on the job it creates (`service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/create_job.rs`) and nothing ever reads it; the Snow Device Management store's only inventory seam, `activate_devices`, is called from no `src/` code at all, and it fabricates its own ordering job ids (`ordering_job_id` → `JID{index:08x}-0000-4000-8000-{index:012x}`) that correspond to no Snowball job in the engine.

Expected (observable over HTTP): create a Snowball job with `RemoteManagement: INSTALLED_AUTOSTART` (and a Snow Family device type that supports it), then call Snow Device Management `ListDevices` on the same engine/region — the ordered device is listed, `associatedWithJob` equals the `JobId` Snowball returned, `ListDevices?jobId=<that JobId>` returns exactly that job's device(s), and `DescribeDevice` on its `managedDeviceId` answers 200.

Actual: `ListDevices` answers HTTP 403 `AccessDeniedException` ("no devices in this Region") no matter how many remote-management Snowball jobs were created, and if an inventory is ever seeded by other means its `associatedWithJob` values are synthetic, so filtering by a real Snowball `JobId` always yields an empty page. The whole device/task surface of this service is therefore unreachable from a client that drives the emulator the way AWS is driven (order a job, then manage the device).

This is the missing cross-service mechanism, distinct from the already-filed ListDevices/wrong-behavior issue about the 403-on-empty-inventory and the absence of any seeding: even with startup seeding, a device ordered through Snowball would still not appear, and `jobId` filtering would still match nothing.

## Evidence

Spec/model: `ListDevicesInput$jobId` — "The ID of the job used to order the device" — and `DeviceSummary$associatedWithJob` — "The ID of the job used to order the device" (smithy_query shape=DeviceSummary; OPERATIONS.md ListDevices). Well-established AWS behavior: Snow Device Management manages Snow Family devices whose ordering job was created with remote management installed (`RemoteManagement` on Snowball `CreateJob`: INSTALLED_AUTOSTART | INSTALLED_ONLY | NOT_INSTALLED); SDM models no device-creation API precisely because the job order is the creation path.

Handler/state: `services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/state.rs` `activate_devices` (~line 451) is documented as "the ONLY way a device enters the inventory" and "deliberately not reachable from a request"; `grep -r activate_devices services/` matches only this crate's `tests/all/*.rs`, never `src/`. `ordering_job_id` (~line 664) mints `JID{index:08x}-0000-4000-8000-{index:012x}` from the device counter, with no lookup into the Snowball job store, while the Snowball emulator mints its own job ids via `mint_job_id` → `mint_id("JID", …)` (`service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/state.rs` ~line 1194). Snowball `create_job.rs` line ~566 stores `remote_management: input.remote_management` and no code anywhere reads it. Both crates reach state through the same `engine::InMemoryEngine` (`state::service_state` downcasts it), so the propagation is implementable in-process.

Tests: no test crosses the two services. `tests/all/device_task_lifecycle.rs::list_devices_narrows_the_inventory_to_one_job_id` filters on the SYNTHETIC `ordering_job_id` values after injecting the inventory through `service_state(...).lock().activate_devices(...)`, i.e. the only passing path is one no HTTP client can take, and no test asserts that a Snowball job produces a managed device.

## Steps to Reproduce

1. Start the emulator with both the Snowball (`snowball` / AWSIESnowballJobManagementService) and Snow Device Management services mounted on one shared in-memory engine.
2. Create an address and then a Snowball job: `POST /` with `X-Amz-Target: AWSIESnowballJobManagementService.CreateJob`, body including `"JobType":"LOCAL_USE"`, a Snow Family `SnowballType` that supports remote management (e.g. `SNC1_HDD`), `"ShippingOption":"NEXT_DAY"` and `"RemoteManagement":"INSTALLED_AUTOSTART"`. Note the returned `JobId` (`JID…`).
3. Call Snow Device Management `ListDevices`: `GET /managed-devices`, signed for the same region. Observe HTTP 403 `AccessDeniedException` instead of HTTP 200 listing the device the job just ordered.
4. Call `GET /managed-devices?jobId=<the JobId from step 2>`. Observe the same 403 (and, on an inventory seeded by any other means, an empty `devices` list) instead of the device(s) shipped by that job.
5. Take the `managedDeviceId` the job's device should carry and call `POST /managed-device/{managedDeviceId}/describe` — there is no id to use, confirming that no Snowball job ever yields a manageable device, so `CreateTask`, `DescribeTask`, `ListExecutions`, the tagging operations on a device ARN, etc. can never be exercised end-to-end.
