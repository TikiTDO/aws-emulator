Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Step: task#14
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DeleteIpamPool|2xx->2xx|$.DeleteIpamPoolResponse.ipamPool.locale
Issue-Commit: e72ab6d9ecc28f469f99eceb42a3c20602b3ead9

# ec2 DeleteIpamPool response omits the modeled ipamPool locale member

> _Note: this issue was identified against the code base at commit `e72ab6d9ecc28f469f99eceb42a3c20602b3ead9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIpamPool

## Description
Cause 1 of 1 (fixture ec2-ipam-consolidated-delete-and-deprovision-lifecycle, step task#14).
AWS: Smithy: IpamPool.Locale (xmlName 'locale', ec2QueryName 'Locale', target String) is a modeled member of the ipamPool structure returned by DeleteIpamPool. For a pool that was created without a locale AWS still serializes the element, carrying the literal string 'None' rather than suppressing it - that is what the recording shows for this pool.
Emulator: DeleteIpamPool answers HTTP 200 with an otherwise matching ipamPool, but the locale element is absent from the body entirely.
Why: a client that reads ipamPool.locale to decide whether a pool is regional or locale-less has to distinguish 'member absent' from 'locale None'; against AWS the member is always there, so parsers that index it unconditionally fail, and code that treats absence as 'unknown locale' takes a different branch than in production.
Fix: the DeleteIpamPool response serializer (and the shared IpamPool serializer it uses) must emit the modeled locale member for every pool, using the pool's stored locale and the literal 'None' when the pool has none.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#14.

Related: open issue 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md owns the DescribeRegions geographySet omission only and does not cover this one.

## Evidence
Replay (trace set simparity-wire-aws-v1), fixture ec2-ipam-consolidated-delete-and-deprovision-lifecycle step task#14, DeleteIpamPool, status 200 -> 200: '$.DeleteIpamPoolResponse.ipamPool.locale: missing (expected \'None\', got None)' - i.e. the recording carries <locale>None</locale> and the emulator emits no locale element. Smithy model (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): IpamPool.Locale -> xmlName 'locale', target String, 'The locale of the IPAM pool.'

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-ipam-consolidated-delete-and-deprovision-lifecycle up to step task#14 (create IPAM + scope + pool with no locale, deprovision, then DeleteIpamPool).
3. Inspect the DeleteIpamPoolResponse body: the ipamPool structure has no <locale> child, where AWS returns <locale>None</locale>.
