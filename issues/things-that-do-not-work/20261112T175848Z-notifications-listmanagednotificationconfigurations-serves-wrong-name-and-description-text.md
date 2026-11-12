Issue-Source: trace-sweep-divergence
Issue-Trace: notifications-catalog-list-managed-notification-configurations
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notifications-catalog-list-managed-notification-configurations
Issue-Shape-Id: com.amazonaws.notifications#Notifications
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Billing-and-Invoices"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Financing"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Fraud-Prevention"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Payment"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-BCM/sub-category/Subscriptions"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-Health/sub-category/Billing"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-Health/sub-category/Billing"].name
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-Health/sub-category/Issue"].description
Issue-Key: notifications|ListManagedNotificationConfigurations|2xx->2xx|$.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-Health/sub-category/Issue"].name
Issue-Commit: a123a2be024ce4110ce5f12504ed1f76c9963c73

# notifications ListManagedNotificationConfigurations serves wrong name and description text for the managed-notification-configuration catalog

> _Note: this issue was identified against the code base at commit `a123a2be024ce4110ce5f12504ed1f76c9963c73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListManagedNotificationConfigurations

## Description
Cause 1 of 1: the notifications service's seeded managed-notification-configuration catalog carries the wrong `name` and `description` strings for several AWS-managed categories, so ListManagedNotificationConfigurations returns a correctly shaped page whose per-entry human-readable text does not match the AWS catalog.

AWS: Smithy: ListManagedNotificationConfigurations is GET /managed-notification-configurations, 200, output `managedNotificationConfigurations` (required) of ManagedNotificationConfigurationStructure, whose `arn` (pattern ^arn:[a-z-]{3,10}:notifications::[0-9]{12}:managed-notification-configuration/category/[A-Za-z0-9-]{3,64}/sub-category/[A-Za-z0-9-]{3,64}$), `name` (@length 1..64) and `description` (@length 0..256) are all @required. The model does not enumerate the catalog's members or their text, so the required values are recorded behavior only: AWS serves a fixed, service-owned catalog in which each entry's `name` and `description` are determined by the entry's category/sub-category ARN, and they are the same for every account. Note that the recorded `name` values contain spaces (e.g. 'Account-Specific Issues'), i.e. AWS does not apply the modeled `name` pattern ^[A-Za-z0-9\-]+$ on output; the recording is the requirement, not the output pattern.

Emulator: the emulator answers 200 with the right ARNs and the right number of entries, but mis-populates the text of seven entries. For category/AWS-Health/sub-category/Issue it serves name 'Issue' where AWS serves 'Account-Specific Issues', and a description about 'All account health issue events that are sent through email. By default, these are sent to the root email address.' where AWS serves 'All account-specific Health events that do not send emails by default. These Health events provide frequent updates about the state of your AWS infrastructure, such as service events or maintenance notices.'. For category/AWS-Health/sub-category/Billing it serves name 'Health Billing' where AWS serves 'Billing Notification', and a description that omits the operations alternate contact ('...sent to the root email address and the billing alternate contact.' instead of AWS's '...sent to the root email address, operations alternate contact, and the billing alternate contact.'). The `description` is likewise wrong for five AWS-BCM sub-categories — Subscriptions (emulator: 'Notifications about the subscriptions billed to your account, including upcoming renewals, expirations, and cancellations.' vs AWS: 'Updates about your AWS subscriptions, plans, credits, and billing status, including sign-ups, plan changes, and Marketplace activity.'), Billing-and-Invoices, Financing, Fraud-Prevention and Payment. In each case the ARN and the structure are correct and only the catalog text diverges; all nine diverging JSON paths are reported by the replay as value_mismatch inside a 200 response.

Why: the managed catalog is how a client discovers which AWS-managed notification categories exist and what each one does. A console, a CLI listing or any UI that renders `name`/`description` shows a different category label and a different (sometimes contradictory) explanation of the delivery behavior than AWS does — the AWS-Health/Issue entry is described as email-by-default when AWS says it does not send email by default, and the AWS-Health/Billing entry drops one of the contacts AWS says is notified. Code that keys off the display name (matching 'Account-Specific Issues' or 'Billing Notification') finds no such configuration even though the ARN is present, and golden-output or snapshot tests over the catalog fail against the emulator.

Fix: the seeded managed-notification-configuration catalog in the notifications service's state — the static table ListManagedNotificationConfigurations (and GetManagedNotificationConfiguration, which must agree with it) serves from. Correct the `name` and `description` for each affected category/sub-category to the AWS catalog text; no handler, routing or serializer change is needed because status, ARNs and shape already match.

Traces: notifications-catalog-list-managed-notification-configurations#task#1 (9 value_mismatch paths across 7 catalog entries: AWS-Health/Issue name+description, AWS-Health/Billing name+description, AWS-BCM/{Subscriptions,Billing-and-Invoices,Financing,Fraud-Prevention,Payment} description).

## Evidence
Fixture notifications-catalog-list-managed-notification-configurations, step task#1 (GET /managed-notification-configurations). Expected status 200, actual status 200 — the divergence is in the body.

Replay detail (value_mismatch, expected = recorded AWS, got = emulator):
- $.managedNotificationConfigurations[arn="arn:aws:notifications::000000000000:managed-notification-configuration/category/AWS-Health/sub-category/Issue"].name: expected 'Account-Specific Issues', got 'Issue'
- ...Health/Issue].description: expected 'All account-specific Health events that do not send emails by default. These Health events provide frequent updates about the state of your AWS infrastructure, such as service events or maintenance notices.', got 'All account health issue events that are sent through email. By default, these are sent to the root email address.'
- ...Health/Billing].name: expected 'Billing Notification', got 'Health Billing'
- ...Health/Billing].description: expected 'Health events with Billing implications that are sent through email. By default, these are sent to the root email address, operations alternate contact, and the billing alternate contact.', got 'Health events with Billing implications that are sent through email. By default, these are sent to the root email address and the billing alternate contact.'
- ...AWS-BCM/Subscriptions].description: expected 'Updates about your AWS subscriptions, plans, credits, and billing status, including sign-ups, plan changes, and Marketplace activity.', got 'Notifications about the subscriptions billed to your account, including upcoming renewals, expirations, and cancellations.'
- ...AWS-BCM/Billing-and-Invoices].description, ...AWS-BCM/Financing].description, ...AWS-BCM/Fraud-Prevention].description, ...AWS-BCM/Payment].description: value_mismatch

The 12-digit account segment of each ARN is an environment value and is not part of the requirement; the category/sub-category path and the name/description text are. Model reference: smithy_query operation=ListManagedNotificationConfigurations and shape=ManagedNotificationConfigurationStructure (name/description both @required).

Related but NOT covering this: open issue 20261108T010234Z-notifications-tag-plane-204-responses-omit-the-content.md (notifications tag-plane 204 Content-Length, which owns the other two divergences in this replay) and 20261104T055216Z-notifications-cloudwatch-alarm-state-changes-never-produce-a.md (NotificationEvent generation / EventRule conflict wording). Neither touches the managed-configuration catalog text.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture notifications-catalog-list-managed-notification-configurations, step task#1.
2. Equivalently: GET /managed-notification-configurations against the notifications endpoint (aws notifications list-managed-notification-configurations). The response is 200 with the full managed catalog.
3. Join the returned entries to the recorded AWS catalog on `arn` and compare `name` and `description`.
4. Observe: the entry for category/AWS-Health/sub-category/Issue has name 'Issue' instead of 'Account-Specific Issues'; category/AWS-Health/sub-category/Billing has name 'Health Billing' instead of 'Billing Notification'; and the `description` differs for AWS-Health/Issue, AWS-Health/Billing and the AWS-BCM sub-categories Subscriptions, Billing-and-Invoices, Financing, Fraud-Prevention and Payment — 9 mismatched fields over 7 entries, with every ARN matching.
