Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-server-host-key-and-security-policy
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-server-host-key-and-security-policy transfer-catalog-list-security-policies transfer-import-invalid-host-key-then-list-unchanged transfer-list-host-keys-on-server-workflow
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeSecurityPolicy|2xx->2xx|$.SecurityPolicy.SshCiphers[*]
Issue-Key: transfer|DescribeSecurityPolicy|2xx->2xx|$.SecurityPolicy.SshKexs[*]
Issue-Key: transfer|DescribeSecurityPolicy|2xx->2xx|$.SecurityPolicy.SshMacs[*]
Issue-Key: transfer|DescribeSecurityPolicy|2xx->2xx|$.SecurityPolicy.TlsCiphers[*]
Issue-Key: transfer|ListSecurityPolicies|2xx->2xx|$.SecurityPolicyNames[*]
Issue-Key: transfer|ListHostKeys|2xx->2xx|$.HostKeys[Arn="arn
Issue-Key: transfer|ListHostKeys|2xx->2xx|$.HostKeys[*]
Issue-Key: transfer|DescribeHostKey|2xx->2xx|$.HostKey.Type
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# Transfer security-policy catalogue and host-key reads serve emulator reference data instead of the service's

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecurityPolicy

## Description
Six net-new divergences share one fault: the data the emulator serves for security policies and for host keys is its OWN seeded reference data, not what the service publishes or what the caller imported. SIX causes must be closed before submitting.

Cause 1 of 6 — DescribeSecurityPolicy returns the wrong algorithm lists, in the wrong order.
AWS: Smithy: DescribeSecurityPolicy -> DescribedSecurityPolicy{SecurityPolicyName required, SshCiphers, SshKexs, SshMacs, TlsCiphers, SshHostKeyAlgorithms: SecurityPolicyOptions lists}. The model constrains none of the contents, so the algorithm sets and their ORDER are published reference data per policy name — recorded behavior only. The recording shows, for the policy under test, SshCiphers beginning aes128-gcm@openssh.com, aes256-gcm@openssh.com, aes128-ctr, aes256-ctr, aes192-ctr; SshKexs beginning ecdh-sha2-nistp256, ecdh-sha2-nistp384, ecdh-sha2-nistp521, curve25519-sha256, curve25519-sha256@libssh.org, diffie-hellman-group18-sha512, diffie-hellman-group16-sha512, diffie-hellman-group-exchange-sha256; SshMacs containing exactly hmac-sha2-256-etm@openssh.com and hmac-sha2-512-etm@openssh.com; plus a TlsCiphers list that likewise differs.
Emulator: serves different members in a different (alphabetical-looking) order — SshCiphers starting aes128-ctr/aes192-ctr/aes256-ctr, SshKexs starting curve25519-sha256, and SshMacs carrying four EXTRA entries the policy does not contain (hmac-sha2-512, umac-128@openssh.com, umac-128-etm@openssh.com and a non-etm hmac-sha2-256).
Why: these lists are exactly what a client reads to decide whether a policy permits its SSH/TLS negotiation; a policy that advertises non-ETM MACs and umac variants it does not actually enable leads a caller to accept a policy that would fail a real negotiation, and order-sensitive preference logic picks a different algorithm than it would against AWS.
Fix: the service's security-policy reference data (the seeded policy catalogue the DescribeSecurityPolicy handler reads), which must carry each published policy's exact member set in published order.
Traces: transfer-server-host-key-and-security-policy#task#7.

Cause 2 of 6 — ListSecurityPolicies serves a stale and short policy catalogue (server/catalog scenario).
AWS: Smithy: ListSecurityPolicies -> SecurityPolicyNames. Contents are published reference data (recorded behavior only). The recorded listing includes the current generations — TransferSFTPConnectorSecurityPolicy-FIPS-2024-10, TransferSecurityPolicy-2025-03, TransferSecurityPolicy-AS2Restricted-2025-07, TransferSecurityPolicy-FIPS-2025-03 and TransferSecurityPolicy-SshAuditCompliant-2025-02 — and is returned in the service's order.
Emulator: serves an older catalogue: the 2024-10 connector FIPS policy appears as FIPS-2023-07, the 2025-03/2025-07/FIPS-2025-03 entries are absent (older generations occupy their positions) and the SshAuditCompliant-2025-02 entry is missing entirely, so the list is also one element short.
Why: a client that picks a policy name from this listing and passes it to CreateServer/UpdateServer selects a policy that exists in the emulator but not in the service (or cannot select a current one at all), and compliance checks asserting a FIPS/audit-compliant generation is available fail.
Fix: the seeded security-policy name catalogue backing ListSecurityPolicies.
Traces: transfer-catalog-list-security-policies#task#1.

Cause 3 of 6 — the same stale catalogue through the server-scoped listing.
AWS: as cause 2; the recorded response names the same current generations.
Emulator: same stale set and the same missing SshAuditCompliant-2025-02 tail entry, reached through the server/host-key scenario rather than the catalogue scenario — i.e. the defect is in the shared catalogue, not one call site.
Why: as cause 2; it is reproduced on the path a caller actually uses while configuring a server.
Fix: as cause 2 — one catalogue, fixed once, must satisfy both call sites.
Traces: transfer-server-host-key-and-security-policy#task#6.

Cause 4 of 6 — ListHostKeys reports the wrong Type for an imported RSA host key.
AWS: Smithy: ListedHostKey.Type -> HostKeyType, documented as one of ssh-rsa, ssh-ed25519, ecdsa-sha2-nistp256/384/521. The Type is DERIVED from the imported key material, so importing an RSA private key yields Type 'ssh-rsa'.
Emulator: reports Type 'ssh-ed25519' for that host key.
Why: Type is how a client identifies a server's host key algorithm; reporting ed25519 for an RSA key makes any algorithm-dependent decision (client config, key rotation, audit of allowed host-key algorithms) wrong, and it indicates the imported material is not being parsed at all.
Fix: the host-key state/projection — derive Type from the imported key material rather than emitting a constant.
Traces: transfer-import-invalid-host-key-then-list-unchanged#task#3 (cluster also covers #task#5 of the same fixture).

Cause 5 of 6 — ListHostKeys returns a different host key than the one imported.
AWS: the listing projects the host keys of the server, so after an ImportHostKey the entry carries that key's HostKeyId, its ARN under the server, its Fingerprint computed from the imported public key, DateImported and Type 'ssh-rsa' for RSA material.
Emulator: returns an entry for a different, emulator-generated key: a different HostKeyId/ARN, a fingerprint that is not the imported key's, Type 'ssh-ed25519', and a StorageType member the recorded entry does not carry — i.e. the imported key is not the one listed.
Why: a caller cannot confirm its own import landed, cannot look the imported HostKeyId up, and reads a fingerprint that does not match the key it holds — host-key pinning and rotation workflows break.
Fix: the host-key state for the server (ImportHostKey must store the imported key and ListHostKeys must project stored keys), plus the ListedHostKey serializer's member set.
Traces: transfer-list-host-keys-on-server-workflow#task#2.

Cause 6 of 6 — DescribeHostKey reports the wrong Type for the same key.
AWS: Smithy: DescribedHostKey.Type -> HostKeyType with the same documented value set, derived from the key material; for an imported RSA key the value is 'ssh-rsa'.
Emulator: 'ssh-ed25519'.
Why: the single-key read is the authoritative view of a host key; it misreports the algorithm, so a client that branches on Type (or renders it to an operator) is wrong on both the list and the describe path.
Fix: the host-key projection shared by DescribeHostKey and ListHostKeys.
Traces: transfer-list-host-keys-on-server-workflow#task#3.

## Evidence
From replay of simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator; all six steps answered HTTP 200 on both sides, so these are body divergences.

1. DescribeSecurityPolicy / transfer-server-host-key-and-security-policy#task#7 — $.SecurityPolicy.SshCiphers[0..4] expected aes128-gcm@openssh.com, aes256-gcm@openssh.com, aes128-ctr, aes256-ctr, aes192-ctr; got aes128-ctr, aes192-ctr, aes256-ctr, aes128-gcm@openssh.com, aes256-gcm@openssh.com. $.SecurityPolicy.SshKexs[0..7] expected ecdh-sha2-nistp256, ecdh-sha2-nistp384, ecdh-sha2-nistp521, curve25519-sha256, curve25519-sha256@libssh.org, diffie-hellman-group18-sha512, diffie-hellman-group16-sha512, diffie-hellman-group-exchange-sha256; got curve25519-sha256, curve25519-sha256@libssh.org, diffie-hellman-group16-sha512, diffie-hellman-group18-sha512, diffie-hellman-group-exchange-sha256, ecdh-sha2-nistp256, ecdh-sha2-nistp384, ecdh-sha2-nistp521. $.SecurityPolicy.SshMacs expected exactly [hmac-sha2-256-etm@openssh.com, hmac-sha2-512-etm@openssh.com]; got hmac-sha2-256 first plus four extra entries (hmac-sha2-512, hmac-sha2-512-etm@openssh.com, umac-128@openssh.com, umac-128-etm@openssh.com). $.SecurityPolicy.TlsCiphers[0..7] also mismatched.
2. ListSecurityPolicies / transfer-catalog-list-security-policies#task#1 — $.SecurityPolicyNames[2] expected 'TransferSFTPConnectorSecurityPolicy-FIPS-2024-10', got 'TransferSFTPConnectorSecurityPolicy-FIPS-2023-07'; [8] expected 'TransferSecurityPolicy-2025-03', got 'TransferSecurityPolicy-2022-03'; [9] expected 'TransferSecurityPolicy-AS2Restricted-2025-07', got 'TransferSecurityPolicy-2023-05'; [14] expected 'TransferSecurityPolicy-FIPS-2025-03', got 'TransferSecurityPolicy-FIPS-2024-05'; [19] expected 'TransferSecurityPolicy-SshAuditCompliant-2025-02', got None (list short); plus extra entries at [3]-[5].
3. ListSecurityPolicies / transfer-server-host-key-and-security-policy#task#6 — same five expected-vs-actual name divergences at indices 2, 8, 9, 14 and the missing index 19.
4. ListHostKeys / transfer-import-invalid-host-key-then-list-unchanged#task#3 (cluster also covers #task#5) — $.HostKeys[Arn="...host-key/<server>/<hostkey-id>"].Type expected 'ssh-rsa', got 'ssh-ed25519'.
5. ListHostKeys / transfer-list-host-keys-on-server-workflow#task#2 — $.HostKeys[0] expected {Arn: .../host-key/<server>/<imported host key id>, HostKeyId: <imported id>, Fingerprint: <imported key's fingerprint>, DateImported, Type 'ssh-rsa'}; got a different HostKeyId/ARN, a different Fingerprint, Type 'ssh-ed25519' and an extra StorageType 'SERVICE_MANAGED' member.
6. DescribeHostKey / transfer-list-host-keys-on-server-workflow#task#3 — $.HostKey.Type expected 'ssh-rsa', got 'ssh-ed25519'.

Model grounding: DescribedHostKey.Type and ListedHostKey.Type both target HostKeyType and document the value set ssh-rsa / ssh-ed25519 / ecdsa-sha2-nistp256|384|521; DescribedSecurityPolicy's SshCiphers/SshKexs/SshMacs/TlsCiphers are unconstrained SecurityPolicyOptions lists, so their contents are published reference data.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Re-replay the six bound steps:
   - transfer-server-host-key-and-security-policy#task#7 (DescribeSecurityPolicy: compare SshCiphers/SshKexs/SshMacs/TlsCiphers member sets AND order)
   - transfer-catalog-list-security-policies#task#1 (ListSecurityPolicies: compare the whole SecurityPolicyNames list and its length)
   - transfer-server-host-key-and-security-policy#task#6 (ListSecurityPolicies via the server scenario)
   - transfer-import-invalid-host-key-then-list-unchanged#task#3 (ListHostKeys: Type of the imported RSA key)
   - transfer-list-host-keys-on-server-workflow#task#2 (ImportHostKey then ListHostKeys: the listed entry must be the imported key)
   - transfer-list-host-keys-on-server-workflow#task#3 (DescribeHostKey: Type of that key)
