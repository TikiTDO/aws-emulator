# Committed AWS CDK snapshot fixtures

These two JSON files are unmodified copies from **aws/aws-cdk** commit
`79914032a02397df81261ad57b2bdfc03fa8b4a5`. They are Apache-2.0 material,
not covered by the CC0 header used for the emulator's Rust tests. The upstream
`LICENSE` and `NOTICE` at this commit are copied without modification here.

Original prefix:
`packages/@aws-cdk-testing/framework-integ/test/`

| Local file | Original path after prefix | SHA256 |
| --- | --- | --- |
| `integ.sns-fifo/SNSFifoInteg.template.json` | `aws-sns/test/integ.sns-fifo.js.snapshot/SNSFifoInteg.template.json` | `84d514b00d9e3dc065215aeb875494767355d96ed11c81410036cd85a35e6c46` |
| `integ.expose-metric/aws-cdk-expose-metric-integ.template.json` | `aws-logs/test/integ.expose-metric.js.snapshot/aws-cdk-expose-metric-integ.template.json` | `eca17ca94625bc7a08a880410570afed5eecf3d9a01b6c401155caa073187adb` |

Source repository: https://github.com/aws/aws-cdk/tree/79914032a02397df81261ad57b2bdfc03fa8b4a5

Both templates retain their original bootstrap-version parameter and rule. The
in-process suite supplies the referenced SSM parameter as an explicit fixture
precondition, not by claiming a successful `cdk bootstrap`. It then submits the
exact bytes through CREATE change-set execution and reads the owning services.
The Logs snapshot has only a template-file asset, not a provider Lambda or image.

This tests snapshot replay and owner effects, not `integ-runner`, real CLI
endpoint/authentication behavior, asset publishing, or a full deployment runtime.
No network retrieval occurs in tests. Updating these fixtures requires an explicit
upstream commit, matching hashes, and review of their resources and preconditions.
