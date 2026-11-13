Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-catalog-list-security-requirement-packs
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-catalog-list-security-requirement-packs
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="ASA Base Pack"].createdAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="ASA Base Pack"].description
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="ASA Base Pack"].updatedAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="AWS Well-Architected Pack"].createdAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="AWS Well-Architected Pack"].description
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="AWS Well-Architected Pack"].updatedAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="NIST CSF Pack"].createdAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="NIST CSF Pack"].description
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="NIST CSF Pack"].updatedAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="PCI DSS Pack"].createdAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="PCI DSS Pack"].description
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="PCI DSS Pack"].updatedAt
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# ListSecurityRequirementPacks AWS-managed packs lose description and fixed timestamps

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecurityRequirementPacks

## Description
Cause 1 of 1 - the built-in (AWS-managed) security requirement pack catalog is served without descriptions and with boot-time timestamps.
AWS: Smithy: ListSecurityRequirementPacks returns the required list securityRequirementPackSummaries of SecurityRequirementPackSummary, whose members include an optional `description` and REQUIRED createdAt/updatedAt (Timestamp, date-time). For the AWS-managed packs this is static catalog content: per the recording each managed pack carries its own fixed description text (for example the base pack is described as the default security requirements shipped with AWS Security Agent, and the framework packs as requirements derived from their respective standard - NIST Cybersecurity Framework, PCI DSS v4.0.1, AWS Well-Architected Framework Security Pillar) and createdAt == updatedAt == the catalog's publication instant, which is a constant of the managed catalog and identical for every managed pack, not a per-request or per-boot value.
Emulator: for every managed pack it returns `description` absent (null) and sets createdAt/updatedAt to the time the emulator seeded its catalog (they move with each boot), so a required, fixed member pair is per-run and an optional one is dropped. Status is 200 in both, so the divergence is entirely in the body.
Why: a client rendering the pack picker has no description to show, and anything that keys off a managed pack's createdAt/updatedAt - caching, change detection, "has the managed catalog been updated since X" comparisons - sees the catalog appear newly published on every emulator start, so it re-syncs constantly and can never detect a real catalog revision.
Fix: the seeded managed-pack catalog in the service's state (and the ListSecurityRequirementPacks summary serializer) - seed each managed pack with its fixed description and the catalog's constant publication timestamp for both createdAt and updatedAt, instead of leaving description unset and stamping the current clock.
Traces: securityagent-catalog-list-security-requirement-packs#task#1.

## Evidence
securityagent-catalog-list-security-requirement-packs#task#1, ListSecurityRequirementPacks, expected 200 / actual 200. Twelve field divergences across the four managed packs ('ASA Base Pack', 'NIST CSF Pack', 'PCI DSS Pack', 'AWS Well-Architected Pack'), three per pack:
- $.securityRequirementPackSummaries[name="<pack>"].description: missing - expected that pack's fixed description text ('Default security requirements shipped with AWS Security Agent', 'Security requirements derived from the NIST Cybersecurity Framework', 'Security requirements derived from the PCI DSS v4.0.1 standard', 'Security requirements derived from the AWS Well-Architected Framework Security Pillar'); got None.
- $.securityRequirementPackSummaries[name="<pack>"].createdAt: value_mismatch - expected the managed catalog's constant publication timestamp (recorded as '2025-12-01T00:00:00Z', the same for all four packs); got a timestamp equal to the emulator's catalog-seed instant, which differs run to run.
- $.securityRequirementPackSummaries[name="<pack>"].updatedAt: same divergence as createdAt.
The requirement is that these are fixed catalog values shared by all managed packs and stable across restarts, not the single recorded literal. No other open securityagent issue covers this (no existing issues match 'securityagent').

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Replay fixture securityagent-catalog-list-security-requirement-packs, step task#1: POST /ListSecurityRequirementPacks with no filter.
3. Inspect securityRequirementPackSummaries for the AWS-managed packs: description is absent on every one, and createdAt/updatedAt equal the emulator's start/seed time rather than the catalog's fixed publication timestamp.
4. Restart the emulator and repeat step 2 - createdAt/updatedAt change, confirming they are per-boot values rather than constants of the managed catalog.
