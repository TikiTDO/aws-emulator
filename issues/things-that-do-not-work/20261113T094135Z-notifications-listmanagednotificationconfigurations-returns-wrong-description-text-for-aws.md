Issue-Source: trace-sweep-divergence
Issue-Trace: notifications-catalog-list-managed-notification-configurations
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notifications-catalog-list-managed-notification-configurations
Issue-Shape-Id: com.amazonaws.notifications#Notifications
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Billing-and-Invoices"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Financing"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Fraud-Prevention"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Payment"].description
Issue-Commit: 03e920c0d4c12b32c4ff6e56db2bffeedbd15a95

# notifications ListManagedNotificationConfigurations returns wrong description text for AWS-BCM managed configurations

> _Note: this issue was identified against the code base at commit `03e920c0d4c12b32c4ff6e56db2bffeedbd15a95`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListManagedNotificationConfigurations

## Description
One cause (Cause 1 of 1): the emulator's seeded managed-notification catalog carries its own paraphrased `description` strings for the AWS-BCM category entries instead of the descriptions AWS publishes, so ListManagedNotificationConfigurations returns the right ARNs and names with the wrong description for four sub-categories in a single response.

AWS: Smithy `ListManagedNotificationConfigurations` -> GET /managed-notification-configurations, 200, output `ListManagedNotificationConfigurationsResponse.managedNotificationConfigurations` (@required list of `ManagedNotificationConfigurationStructure`). That structure's members `arn`, `name` and `description` are all @required; `description` targets `ManagedNotificationConfigurationDescription` (@length min 0 max 256, pattern ^[^\u0001-\u001F\u007F-\u009F]*$) and is documented only as "The description of the ManagedNotificationConfiguration". The model does not enumerate the catalog itself, so the catalog contents are recorded behavior only: the managed-notification catalog is service-owned, read-only (the operation is @readonly, there is no Create/Update for a managed configuration), and therefore a fixed per-sub-category string that AWS returns verbatim for every caller. The recording shows, for ARN arn:aws:notifications::<account>:managed-notification-configuration/category/AWS-BCM/sub-category/<SubCategory>:
  - sub-category Payment -> "Notifications about payment method failures, past-due alerts, account restrictions, suspension and reinstatement, direct debit enrollment, advance pay, payment profile changes, backup payment attempts, payment verification, and FX rate renewals."
  - sub-category Fraud-Prevention -> "Notifications about potentially unauthorized account activity and/or related remediation requirement(s)."
  - sub-category Financing -> "Notifications about the AWS Financing program including updates about your Financing Line application, activation of your line, withdrawals and status."
  - sub-category Billing-and-Invoices -> "Notifications about your AWS invoices, payments, tax documents, and billing account changes."
In each of these the whole string is FIXED text: a managed-configuration description has no variable parts substituted from the request or from account state (no account id, name, limit or timestamp appears in it). The only request/state-derived value on the entry is the 12-digit account id inside `arn`, which the emulator already renders correctly. The description is keyed solely by the configuration's category/sub-category.

Emulator: status and envelope are correct (expected 200, actual 200; `managedNotificationConfigurations` present with the expected ARNs and names), but four entries' `description` values are different strings:
  - .../sub-category/Payment -> "Notifications about your payments, including upcoming and overdue payments, payment confirmations, failed payment methods, and refunds."
  - .../sub-category/Fraud-Prevention -> "Notifications about suspected fraudulent activity on your account, including unusual usage, credentials that may be compromised, and the action needed to secure the account."
  - .../sub-category/Financing -> "Notifications about AWS financing for your account, including financing application outcomes, installment schedules, and upcoming financed payments."
  - .../sub-category/Billing-and-Invoices -> "Notifications about your bills and invoices, including invoice availability, charges that differ from your usual spend, ..."
Each is a plausible-sounding rewrite rather than the service's own text, which is the signature of a hand-authored seed table rather than one transcribed from the service. The same mismatch appears on all four AWS-BCM sub-categories in one call, so it is a single defective seed table, not four independent slips.

Why: the managed-notification catalog is the only place a client can learn what a managed configuration covers, and the description is a @required field. A console, a catalog browser or a subscription wizard renders this string straight to the user, so it shows text AWS never emits; wording differs materially (e.g. the real Payment description enumerates direct-debit enrollment, advance pay, payment-profile changes, payment verification and FX-rate renewals, which the emulator's text drops entirely, and the real Fraud-Prevention text is about "potentially unauthorized account activity" rather than "suspected fraudulent activity"). Any client that snapshots, hashes, localizes against, or asserts on the catalog's descriptions diverges, and anyone using the emulator to decide which managed sub-category to subscribe to is reading a different description of scope than production will give them.

Fix: the notifications managed-notification catalog seed data behind the ListManagedNotificationConfigurations handler — the static per-category/sub-category table the handler reads to build each `ManagedNotificationConfigurationStructure`. The handler's shape, pagination and ARN construction are fine; only the stored description strings are wrong, so the correction belongs in that state/seed table (and must be shared with GetManagedNotificationConfiguration so both read the same text).

Traces: notifications-catalog-list-managed-notification-configurations#task#1 (all four sub-category descriptions diverge in this one response).

## Evidence
Fixture notifications-catalog-list-managed-notification-configurations, step task#1, operation ListManagedNotificationConfigurations, verdict mismatch. Expected status 200, actual status 200 — the divergence is entirely in the response body.

$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Payment"].description
  expected: 'Notifications about payment method failures, past-due alerts, account restrictions, suspension and reinstatement, direct debit enrollment, advance pay, payment profile changes, backup payment attempts, payment verification, and FX rate renewals.'
  actual:   'Notifications about your payments, including upcoming and overdue payments, payment confirmations, failed payment methods, and refunds.'

$.managedNotificationConfigurations[arn=".../category/AWS-BCM/sub-category/Fraud-Prevention"].description
  expected: 'Notifications about potentially unauthorized account activity and/or related remediation requirement(s).'
  actual:   'Notifications about suspected fraudulent activity on your account, including unusual usage, credentials that may be compromised, and the action needed to secure the account.'

$.managedNotificationConfigurations[arn=".../category/AWS-BCM/sub-category/Financing"].description
  expected: 'Notifications about the AWS Financing program including updates about your Financing Line application, activation of your line, withdrawals and status.'
  actual:   'Notifications about AWS financing for your account, including financing application outcomes, installment schedules, and upcoming financed payments.'

$.managedNotificationConfigurations[arn=".../category/AWS-BCM/sub-category/Billing-and-Invoices"].description
  expected: 'Notifications about your AWS invoices, payments, tax documents, and billing account changes.'
  actual:   'Notifications about your bills and invoices, including invoice availability, charges that differ from your usual spend, ...'

All four are reported by the replay as `value_mismatch` under divergence_key service=notifications|ListManagedNotificationConfigurations|status=200->200. The 000000000000 account id in each ARN is this environment's account and is NOT part of the requirement; the description strings are. Smithy citations: ListManagedNotificationConfigurations http GET /managed-notification-configurations code 200, output member managedNotificationConfigurations @required; ManagedNotificationConfigurationStructure.description @required, target ManagedNotificationConfigurationDescription @length(0,256), pattern ^[^\u0001-\u001F\u007F-\u009F]*$; operation is @readonly (no client-supplied description path exists). Catalog text itself: recorded behavior only.

Related but NOT covering this: open issue 20261108T010234Z-notifications-tag-plane-204-responses-omit-the-content.md owns the notifications tag-plane 204 Content-Length divergences on fixture notifications-tag-resource-list-tags-untag-roundtrip (task#5 TagResource, task#7 UntagResource) and says nothing about the managed-notification catalog; open issue 20261104T055216Z-notifications-cloudwatch-alarm-state-changes-never-produce-a.md covers NotificationEvent generation and the EventRule ConflictException wording, not catalog descriptions.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture `notifications-catalog-list-managed-notification-configurations`, step `task#1`: GET /managed-notification-configurations against the notifications endpoint (no query parameters needed), e.g. `aws notifications list-managed-notification-configurations --endpoint-url <emulator>`.
3. In the 200 response, locate the four `managedNotificationConfigurations` entries whose `arn` ends in `/category/AWS-BCM/sub-category/Payment`, `/sub-category/Fraud-Prevention`, `/sub-category/Financing` and `/sub-category/Billing-and-Invoices`.
4. Compare each entry's `description` against the recorded AWS text listed in the evidence. Observed: all four differ (paraphrased seed text), while `arn` and `name` match.
5. Confirmed fixed when replaying `notifications-catalog-list-managed-notification-configurations#task#1` passes, i.e. each AWS-BCM sub-category's `description` is returned as AWS's exact fixed string for that sub-category.
