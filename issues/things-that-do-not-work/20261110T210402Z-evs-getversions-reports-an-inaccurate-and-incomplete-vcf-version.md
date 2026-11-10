Issue-Source: trace-sweep-divergence
Issue-Trace: evs-catalog-get-versions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: evs-catalog-get-versions
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs|GetVersions|2xx->2xx|$.vcfVersions[*]
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# GetVersions reports an inaccurate and incomplete VCF version catalogue

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVersions

## Description
Three net-new root causes in the evs crate, all grounded in the single wire divergence on GetVersions (fixture evs-catalog-get-versions, step task#1). All three are properties of the `vcfVersions` catalogue the emulator synthesises: it reports the wrong availability status, the wrong per-version instance-type list, and omits one modelled version entirely. The response status is 200 both ways; the divergence is entirely in the body.

=== Cause 1 of 3: every VcfVersionInfo is reported AVAILABLE, so RESTRICTED is unreachable ===
AWS: Smithy GetVersions -> GetVersionsResponse$vcfVersions (VcfVersionList of VcfVersionInfo); VcfVersionInfo$status is @required String and its own documentation states the valid values: "AVAILABLE - This VCF version is available to you. RESTRICTED - This VCF version has limited availability." Both values are therefore reportable, and the status is a per-version property of the service catalogue rather than a constant. Recorded behavior: the recorded GetVersions response carries status RESTRICTED for the older VCF 5.2 entry while the newer entries are AVAILABLE, i.e. a version whose availability is limited is reported RESTRICTED and only an unrestricted one is reported AVAILABLE. The requirement is that `status` is driven by the catalogue entry, with both documented values reachable; which particular build is restricted at any moment is catalogue data that moves, so it must be a property of the catalogue table and not asserted as a literal anywhere else.
Emulator: `status` is a hard-coded constant for every entry. services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs `provided_vcf_versions()` sets `status: VERSION_STATUS_AVAILABLE.to_string()` for every version it emits, and the doc comment on `VERSION_STATUS_RESTRICTED` states outright that it is "Reported for no version by this service". Replay: `$.vcfVersions[0]` expected `status: "RESTRICTED"` for vcfVersion VCF-5.2.1, got `status: "AVAILABLE"`.
Why: GetVersions is the discovery call a client uses to decide which VCF version it may offer a user and pass to CreateEnvironment. With every entry pinned to AVAILABLE, a client that filters or warns on RESTRICTED versions sees a catalogue in which no version is ever limited, presents a restricted version as freely usable, and never exercises its restricted-version code path. The RESTRICTED branch of any client built against this emulator is dead code.
Fix: the service version catalogue in crate::state (src/state.rs: the `EsxBuild` / `OFFERED_ESX_BUILDS` table and `provided_vcf_versions`, consumed by src/ops/get_versions.rs) must carry a per-version status drawn from {AVAILABLE, RESTRICTED} instead of substituting a constant in the projection.
Traces: evs-catalog-get-versions#task#1 ($.vcfVersions[0] status mismatch).

=== Cause 2 of 3: instanceTypes is the whole InstanceType enum for every version, not that version's offered types ===
AWS: Smithy VcfVersionInfo$instanceTypes is @required InstanceTypeList of the InstanceType enum, whose declared values are i4i.metal, i7i.metal-24xl, i7i.metal-48xl; the output member is documented as "A list of VCF versions with their availability status, default ESX version, and instance types", i.e. the instance types are stated PER VCF version. Recorded behavior: the recorded response gives different versions different instanceTypes lists — the VCF 5.2 entry is offered on a strict subset (i4i.metal, i7i.metal-24xl) while another entry lists all three — so the hardware a version is offered on is a per-version fact and the list is not simply the enum. The requirement is the per-version narrowing; which exact types a given version carries is catalogue data that changes.
Emulator: every entry gets the full enum. src/state.rs `provided_vcf_versions()` builds `instance_types` by mapping `INSTANCE_TYPE_VALUES` wholesale for every version (the doc comment justifies this as "nothing in the model narrows the types per VCF version"). Replay: `$.vcfVersions[0]` expected `instanceTypes: ["i4i.metal", "i7i.metal-24xl"]`, got `["i4i.metal", "i7i.metal-24xl", "i7i.metal-48xl"]`.
Why: a client that reads instanceTypes to populate a host-type picker, or to pre-validate CreateEnvironmentHost input, is told a version supports hardware it is not offered on. The pre-flight check passes for a combination AWS rejects, so the client's own validation is useless and the failure surfaces only later at host creation.
Fix: the same catalogue in crate::state (src/state.rs `provided_vcf_versions` and the build/instance-type table it reads): a VCF version's `instanceTypes` must come from the catalogue entry for that version, and must stay consistent with `instanceTypeEsxVersions` (that version's `defaultEsxVersion` present among the `esxVersions` of every instance type it names) by construction.
Traces: evs-catalog-get-versions#task#1 ($.vcfVersions[0] instanceTypes mismatch).

=== Cause 3 of 3: the SELF_DEPLOYED entry is omitted from vcfVersions entirely ===
AWS: Smithy enum com.amazonaws.evs#VcfVersion declares exactly three values — VCF-5.2.1, VCF-5.2.2, SELF_DEPLOYED — and CreateEnvironmentRequest$vcfVersion targets that enum (SELF_DEPLOYED documented as "You install VCF yourself"). `vcfVersions` is @required. Recorded behavior: the recorded response carries a vcfVersions entry for EVERY declared VcfVersion, SELF_DEPLOYED included, as a third element after the two VCF-5.2.x entries (enum declaration order), and that entry is fully populated: a status from {AVAILABLE, RESTRICTED}, a `defaultEsxVersion` in ESXi build-id form (literal prefix `ESXi-`), and a non-empty `instanceTypes` list. The requirement is the completeness of the catalogue — one entry per declared VcfVersion, each fully populated and mutually consistent with instanceTypeEsxVersions — not the particular build id or status of any one entry.
Emulator: the entry is excluded twice over. src/state.rs `provided_vcf_versions()` first applies `.filter(|version| **version != VCF_VERSION_SELF_DEPLOYED)`, then `.filter_map(|version| default_esx_version(version).map(...))`, and no entry of `OFFERED_ESX_BUILDS` names SELF_DEPLOYED in its `default_for`, so even without the explicit filter the version would be dropped for having no default build. Replay: `$.vcfVersions[2]` expected an entry with `vcfVersion: "SELF_DEPLOYED"`, got None — the emulator's list has only two elements.
Why: the discovery catalogue contradicts the operation it describes. CreateEnvironment in this very emulator accepts vcfVersion=SELF_DEPLOYED, but a client that enumerates vcfVersions to build its version picker, to validate user input, or to decide whether self-deployed onboarding is offered concludes that mode does not exist. Self-deployed paths are never surfaced and never exercised. (Open issue 20261110T122920Z-evs-getversions-omits-the-self-deployed-entry-from-vcfversions.md describes the same omission from the behavioral corpus, but it carries no wire replay key for this fixture step and this divergence is not marked owned by it, so this cause is filed here with the binding a fixer can re-replay; the wire recording additionally pins the SHAPE of the missing entry — it is the third element and it carries a populated ESXi-form defaultEsxVersion and instanceTypes, which that issue could not state.)
Fix: the service version catalogue in crate::state (src/state.rs `provided_vcf_versions` plus the `OFFERED_ESX_BUILDS` / `EsxBuild::default_for` table it reads, consumed by src/ops/get_versions.rs): drop both filters and give every modelled VcfVersion a catalogue entry with a status, an ESXi build-id default and instance types, rather than deriving membership from whether some build happens to name the version as its default.
Traces: evs-catalog-get-versions#task#1 ($.vcfVersions[2] missing).

## Evidence
From the simparity-wire-aws-v1 replay: 32 steps, 31 passed, 1 mismatched, 0 errored, 0 blocked (score 96.9%). The single divergence cluster is service=evs, operation=GetVersions, verdict=mismatch, fixture evs-catalog-get-versions, step task#1, expected_status 200 / actual_status 200, affected_count 1. Divergence key: service=evs|GetVersions|status=200->200|keys=$.vcfVersions[0],$.vcfVersions[2].

Reported detail, verbatim:
  $.vcfVersions[0]: missing (expected {'defaultEsxVersion': 'ESXi-8.0U3b-24280767', 'instanceTypes': ['i4i.metal', 'i7i.metal-24xl'], 'status': 'RESTRICTED', 'vcfVersion': 'VCF-5.2.1'}, got {'vcfVersion': 'VCF-5.2.1', 'status': 'AVAILABLE', 'defaultEsxVersion': 'ESXi-8.0U3b-24280767', 'instanceTypes': ['i4i.metal', 'i7i.metal-24xl', 'i7i.metal-48xl']})
  $.vcfVersions[2]: missing (expected {'defaultEsxVersion': 'ESXi-9.0.2.0.25148076', 'instanceTypes': ['i4i.metal', 'i7i.metal-24xl', 'i7i.metal-48xl'], 'status': 'AVAILABLE', 'vcfVersion': 'SELF_DEPLOYED'}, got None)
  $.vcfVersions[0]: extra (same two objects as above)

Cause-by-cause expected vs actual, per element:
  Cause 1 — vcfVersions entry for VCF-5.2.1: expected status 'RESTRICTED', actual status 'AVAILABLE'. Note defaultEsxVersion matches exactly ('ESXi-8.0U3b-24280767'), so the mismatch is confined to the status member; the emulator cannot produce 'RESTRICTED' for any version at all.
  Cause 2 — same entry's instanceTypes: expected the 2-element subset ['i4i.metal', 'i7i.metal-24xl'], actual the full 3-element InstanceType enum ['i4i.metal', 'i7i.metal-24xl', 'i7i.metal-48xl'].
  Cause 3 — third vcfVersions element: expected an entry for 'SELF_DEPLOYED' (status AVAILABLE, defaultEsxVersion of ESXi- build-id form, instanceTypes non-empty), actual None — the emulator's vcfVersions has length 2.

Source of all three, in services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:
  - provided_vcf_versions() line ~2890: `.filter(|version| **version != VCF_VERSION_SELF_DEPLOYED)` then `.filter_map(|version| default_esx_version(version).map(...))` (cause 3), `status: VERSION_STATUS_AVAILABLE.to_string()` (cause 1), `instance_types: INSTANCE_TYPE_VALUES.iter().map(...)` (cause 2).
  - VERSION_STATUS_RESTRICTED line ~2709, whose doc comment states it is "Reported for no version by this service".
  - OFFERED_ESX_BUILDS line ~2780: no EsxBuild lists SELF_DEPLOYED in default_for; the build the recording names for SELF_DEPLOYED ('ESXi-9.0.2.0.25148076') is present in the table with `default_for: &[]`.
  - src/ops/get_versions.rs line 165 reads the catalogue unchanged, so the handler itself is not at fault.

Smithy consulted: GetVersions (closure) — GetVersionsResponse$vcfVersions and $instanceTypeEsxVersions both @required; VcfVersionInfo requires vcfVersion (enum VcfVersion), status (String), defaultEsxVersion (String), instanceTypes (InstanceTypeList); enum VcfVersion values ['VCF-5.2.1','VCF-5.2.2','SELF_DEPLOYED']; enum InstanceType values ['i4i.metal','i7i.metal-24xl','i7i.metal-48xl']. The AVAILABLE/RESTRICTED value list is prose on VcfVersionInfo$status.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture evs-catalog-get-versions, step task#1 (a plain evs GetVersions call; GetVersionsRequest has no members). The step returns HTTP 200 with a body that diverges from the recording.
2. Cause 1: read vcfVersions[] and collect the distinct `status` values. Emulator: {'AVAILABLE'} only. AWS: the limited-availability entry (VCF-5.2.1 in the recording) reports 'RESTRICTED' while others report 'AVAILABLE'. Confirm in src/state.rs that provided_vcf_versions() assigns VERSION_STATUS_AVAILABLE unconditionally, so no input can make RESTRICTED appear.
3. Cause 2: compare vcfVersions[].instanceTypes across entries. Emulator: every entry carries the identical full InstanceType enum (3 types). AWS: the lists differ per version — the VCF-5.2.1 entry carries only ['i4i.metal','i7i.metal-24xl'] and omits i7i.metal-48xl.
4. Cause 3: collect the set of vcfVersions[].vcfVersion values. Emulator: {'VCF-5.2.1','VCF-5.2.2'} (length 2). AWS: those two plus 'SELF_DEPLOYED' as the third element, carrying a status in {AVAILABLE, RESTRICTED}, a defaultEsxVersion beginning with 'ESXi-', and a non-empty instanceTypes list.
5. For any entry AWS returns, cross-check consistency with the other half of the response: every instanceTypes member must appear as an instanceTypeEsxVersions[].instanceType, with that entry's defaultEsxVersion among its esxVersions. This must hold for the restored SELF_DEPLOYED entry too.
6. Cross-check CreateEnvironment, which in this emulator accepts vcfVersion='SELF_DEPLOYED': the discovery catalogue must not omit a version the create operation accepts.
