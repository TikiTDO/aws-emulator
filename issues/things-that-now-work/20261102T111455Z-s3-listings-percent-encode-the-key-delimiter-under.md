Issue-Source: trace-sweep-divergence
Issue-Trace: s3-csbs-cli-ls-request-shapes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-csbs-cli-ls-request-shapes s3-csvcols-reference-hardened-download s3-lsd-invalid-bucket-name-reads-as-missing s3-lsd-text-output-pagination-loop s3-lsd-versions-view-delimiter-paging s3-mvs3-versioned-reference-move s3-seeded-graph-csvs-round-trip-byte-exact
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.CommonPrefixes.Prefix
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Delimiter
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents.Key
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.NextContinuationToken
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.CommonPrefixes[*].Prefix
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.CommonPrefixes[*].Prefix
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Delimiter
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.CommonPrefixes.Prefix
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.NextKeyMarker
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version.Key
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[Key="graph%2Fedges.csv"]
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[Key="graph%2Fvertices.csv"]
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[Key="graph/edges.csv"]
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[Key="graph/vertices.csv"]
Issue-Related: 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md
Issue-Related: 20261030T053639Z-s3-listobjects-and-listobjectsv2-omit-prefix-marker-and.md
Issue-Related: 20261030T053639Z-s3-listobjectversions-omits-markers-owner-and-checksum-members.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 listings percent-encode the key delimiter under encoding-type url

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
Cause 1 of 3: ListObjectsV2 over-encodes '/' in every echoed and derived string. AWS: with encoding-type=url, S3 URL-encodes the key-valued members of the listing (Prefix, Delimiter, Key, CommonPrefixes.Prefix, NextContinuationToken-adjacent markers) but '/' is NOT escaped - the recorded responses carry Delimiter '/', CommonPrefixes.Prefix 'mydocuments/' and Key 'mydocuments/sample.txt'. Smithy: ListObjectsV2 output declares Prefix, Delimiter, EncodingType and CommonPrefixes; the encoding rule itself is recorded behavior only. Emulator: it emits '%2F' everywhere a '/' belongs - Delimiter '%2F', CommonPrefixes.Prefix 'mydocuments%2F', Contents.Key 'mydocuments%2Fsample.txt'. Why: a client that feeds a returned CommonPrefixes.Prefix back as the next prefix, or uses a returned Key in a GetObject, addresses a literally different object; CLI-style recursive walks break at the first slash. Fix: the ListObjectsV2 response serializer's URL-encoding helper (escape per S3's rule, leaving '/' unescaped). Traces: s3-csbs-cli-ls-request-shapes#task#5, #task#7, #task#8, s3-csvcols-reference-hardened-download#task#7, s3-lsd-invalid-bucket-name-reads-as-missing#task#8, s3-lsd-text-output-pagination-loop#task#5, #task#8, #task#9.

Cause 2 of 3: ListObjectVersions over-encodes the same members plus its markers. AWS: Prefix, Delimiter, Version.Key, DeleteMarker.Key, KeyMarker and NextKeyMarker are returned with '/' intact ('documents/', 'documents/sample.txt'). Recorded behavior only. Emulator: all of them come back with '%2F', and the NextKeyMarker it hands out is itself encoded, so the next page is requested with a marker AWS would never have issued. Why: paged version walks and version-qualified reads both address the wrong key. Fix: the same encoding helper, applied in the ListObjectVersions serializer. Traces: s3-lsd-versions-view-delimiter-paging#task#5, #task#6, #task#7, #task#11, s3-mvs3-versioned-reference-move#task#14.

Cause 3 of 3: an encoded key is persisted, not just echoed. AWS: the listing of objects written at 'graph/edges.csv' and 'graph/vertices.csv' reports exactly those keys. Recorded behavior only. Emulator: the listing reports Contents entries keyed 'graph%2Fedges.csv' and 'graph%2Fvertices.csv' and the expected keys are absent, i.e. the percent-encoded form has reached the object index rather than only the serializer. Why: the object is no longer reachable under the key the client wrote, so a write-then-read round trip fails. Fix: decode the key once at the request-parsing boundary of the object write/list path and store the decoded form in state. Traces: s3-seeded-graph-csvs-round-trip-byte-exact#task#4.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-csbs-cli-ls-request-shapes#task#5 (ListObjectsV2, 200/200): $.ListBucketResult.Delimiter value_mismatch (expected '/', got '%2F'); $.ListBucketResult.CommonPrefixes.Prefix value_mismatch (expected 'mydocuments/', got 'mydocuments%2F'). s3-csbs-cli-ls-request-shapes#task#7 adds $.ListBucketResult.Contents.Key (expected 'mydocuments/sample.txt', got 'mydocuments%2Fsample.txt'). s3-lsd-versions-view-delimiter-paging#task#6: $.ListVersionsResult.NextKeyMarker (expected 'documents/', got 'documents%2F'). s3-seeded-graph-csvs-round-trip-byte-exact#task#4: Contents[Key="graph/edges.csv"] missing while Contents[Key="graph%2Fedges.csv"] is extra.

## Steps to Reproduce
1. Replay s3-csbs-cli-ls-request-shapes and read step task#5's ListBucketResult: Delimiter and CommonPrefixes.Prefix come back with %2F. 2. Replay s3-lsd-versions-view-delimiter-paging step task#6 and observe the encoded NextKeyMarker. 3. Replay s3-seeded-graph-csvs-round-trip-byte-exact step task#4 and observe the stored keys themselves are percent-encoded.
