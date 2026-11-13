// SPDX-License-Identifier: CC0-1.0

//! The channel associations an email contact of this service can be the channel
//! of, which live in the User Notifications store.
//!
//! An email contact is not only this service's own resource: User Notifications
//! associates one with a NotificationConfiguration as a CHANNEL — its modelled
//! `ChannelArn` admits exactly the `chatbot`, `consoleapp` and
//! `notifications-contacts` resources, so the arn this service mints is one of
//! the arns `AssociateChannel` persists and `ListChannels` reports. The
//! association is a reference to the contact, so it is only meaningful while the
//! contact exists, which is what `DeleteEmailContact` documents: "Deleting an
//! email contact removes it from all associated notification configurations."
//!
//! The associations themselves are NOT copied here. They stay in the store that
//! owns them, reached — like every other state in this emulator — through the
//! one shared engine, so what this module changes is exactly what the sibling
//! service's own reads report.

use service_com_amazonaws_notifications_notifications as notifications;

/// Drop `contact_arn` as a channel from every notification configuration that
/// carries it, and report how many associations went.
///
/// `0` is the answer when nothing referenced the contact — and also when this
/// emulator is running without the sibling service's store at all, in which case
/// there is no association anywhere to drop. Either way the deletion of the
/// contact itself stands: this removes references to a contact that is going,
/// and never refuses the deletion.
pub fn drop_channel_associations<Eng: Send + Sync + 'static>(
    engine: &Eng,
    contact_arn: &str,
) -> usize {
    let Some(state) = notifications::state::service_state(engine) else {
        return 0;
    };
    let mut store = state.lock();
    store.remove_channel_everywhere(contact_arn)
}
