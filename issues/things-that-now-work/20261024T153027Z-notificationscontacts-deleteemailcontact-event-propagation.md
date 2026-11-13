Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Deleting an email contact must remove it from all associated notification configurations

_Filed: 2026-10-24T15:30:27Z_
_Operation: DeleteEmailContact | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy model's own documentation for DeleteEmailContact states: "Deleting an email contact removes it from all associated notification configurations." In real AWS, after an email-contact ARN has been associated as a channel of a NotificationConfiguration (notifications:AssociateChannel), deleting that email contact drops the association, so notifications:ListChannels for that configuration no longer reports the deleted contact's ARN.

The emulator's DeleteEmailContact handler only removes the record from its own store (`store.remove_contact(&input.arn)`) and never touches the sibling `notifications` service's channel-association state. Expected: after DeleteEmailContact succeeds, ListChannels on every NotificationConfiguration that referenced that contact ARN omits it (and the configuration's channel count drops accordingly). Actual: the stale email-contact ARN remains associated and is still reported by ListChannels, so a dangling channel pointing at a contact that returns ResourceNotFoundException on GetEmailContact survives indefinitely.

## Evidence

Spec: `smithy_query(query="doc", shape="DeleteEmailContact")` returns the note "Deleting an email contact removes it from all associated notification configurations." Cross-service reachability is spec-grounded too: in the sibling model, `com.amazonaws.notifications#ChannelArn` has pattern `^arn:[a-z-]{3,10}:(chatbot|consoleapp|notifications-contacts):...$`, i.e. an email-contact ARN IS a valid ChannelArn.

Handler: services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/ops/delete_email_contact.rs lines ~154-163 — the whole body is `store.remove_contact(&input.arn)` against this service's own `contacts` map; no access to any other service's state.

Sibling emulator holds the state that should be updated: services/service-com-amazonaws-notifications-notifications/src/state.rs defines `NotificationConfigurationRecord::channel_arns` (line ~93) with `associate_channel`/`disassociate_channel`/`channels_in_association_order`, and services/service-com-amazonaws-notifications-notifications/src/ops/associate_channel.rs (lines 166-205) persists ANY pattern-valid channel ARN — including an email-contact ARN — with ListChannels reading it back. Both services are implemented (not stubs).

Test coverage: none. tests/all/email_contacts.rs::deleting_a_contact_removes_it_from_every_later_read asserts only that the contact disappears from this service's Get/List; no test in either crate asserts the channel association is dropped.

## Steps to Reproduce

1. POST /2022-09-19/emailcontacts on the notifications-contacts endpoint to create an email contact; keep the returned `arn` (arn:aws:notifications-contacts::<acct>:emailcontact/<id>).
2. Against the notifications endpoint, create a NotificationConfiguration (CreateNotificationConfiguration) and call AssociateChannel with `arn` = the email-contact ARN and `notificationConfigurationArn` = the configuration ARN.
3. Call ListChannels for that configuration and confirm the email-contact ARN is reported.
4. Call DELETE /emailcontacts/{arn} on the notifications-contacts endpoint (succeeds, 200).
5. Call ListChannels for the same configuration again: the deleted email-contact ARN is still listed (expected: it is gone, per the DeleteEmailContact doc note), and GetEmailContact on that ARN now returns ResourceNotFoundException — a dangling association.
