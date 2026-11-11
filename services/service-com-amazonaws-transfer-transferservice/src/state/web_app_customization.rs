// SPDX-License-Identifier: CC0-1.0

//! The guard that admits ONE writer of a web app's customization.
//!
//! `UpdateWebAppCustomization` and `DeleteWebAppCustomization` both write the single
//! customization object a web app holds, and both model `ConflictException`
//! (`smithy_query(query="operation",operation="UpdateWebAppCustomization,DeleteWebAppCustomization")`
//! lists it, HTTP 409, for each). The service applies one such write at a time: a
//! request that arrives while another is still being applied to the SAME web app is
//! refused with that conflict rather than racing it, so two writers that overlap have
//! exactly one winner and the surviving customization is one submitter's values rather
//! than a mixture of both.
//!
//! This is MUTUAL EXCLUSION between overlapping calls, not a span the web app sits in:
//! the claim is taken when a write begins and released as soon as it is over, so a
//! caller that writes AFTER reading the previous write's answer never meets it, and only
//! a caller whose call is in flight at the same time as another's does. (The roll-out
//! window a configuration change of the web app itself sits in is a different thing,
//! with its own due instant — `crate::state::WEB_APP_UPDATE_SPAN`.)
//!
//! The claim is OWNED, not flagged: [`claim`] hands back a [`WriteClaim`] whose `Drop`
//! releases it, so it is released on every exit a request can take — the write
//! completing, a refusal after it was taken, the future being DROPPED at the await
//! (a caller that disconnects, a request that times out, a cancelled task), or an
//! unwind. A claim can therefore never outlive the request that took it and wedge a web
//! app into answering 409 forever. Nothing in this module holds the state lock across an
//! await, and the release takes that lock, so a caller must not be holding it when the
//! claim is dropped.

use crate::ops::types::ConflictException;
use crate::state::ComAmazonawsTransferTransferserviceState as State;

/// Which write of a web app's customization a refusal is about: the two operations that
/// write the one object, and the two sentences the service refuses them with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    /// `UpdateWebAppCustomization`, which assigns the customization's members.
    Update,
    /// `DeleteWebAppCustomization`, which removes the whole object.
    Delete,
}

/// `ConflictException`'s `Message` for an UPDATE of a web app's customization that could
/// not be carried out because another write of the same object was still being applied.
///
/// The service's own fixed text for the condition: it states that the write could not be
/// done, names the object by the service's own name for it, and carries nothing from the
/// request or the store — the web app a caller addressed is the one it named. Worded for
/// the write it refuses, which is what [`DELETE_CONFLICT`] is for the other one.
pub const UPDATE_CONFLICT: &str = "Unable to update WebApp customization";

/// `ConflictException`'s `Message` for a DELETE of a web app's customization that lost
/// the claim — [`UPDATE_CONFLICT`]'s sentence for the other write of the one object.
pub const DELETE_CONFLICT: &str = "Unable to delete WebApp customization";

/// The conflict payload for a customization write that lost the claim, worded for the
/// write that lost it.
pub fn write_conflict(write: Write) -> ConflictException {
    ConflictException {
        message: match write {
            Write::Update => UPDATE_CONFLICT.to_string(),
            Write::Delete => DELETE_CONFLICT.to_string(),
        },
    }
}

/// Why a write did not get the claim on a web app's customization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Another write of this web app's customization holds it: the modelled
    /// `ConflictException`.
    Conflict,
    /// The store holds no such web app: the modelled `ResourceNotFoundException`.
    NoSuchWebApp,
}

/// The claim one request holds on one web app's customization while it writes it.
///
/// Held by VALUE for as long as the write may still happen, and released when it is
/// dropped — see the module header for why that is what the claim is, rather than a flag
/// a success path clears.
///
/// No `Debug`: the state handle it carries is the whole store, which has none to
/// borrow.
pub struct WriteClaim {
    /// The store the claim lives in, as a handle: the claim has to be releasable without
    /// the request that took it still being there to do it.
    state: ::engine::ServiceState<State>,
    web_app_id: String,
}

impl Drop for WriteClaim {
    fn drop(&mut self) {
        // Releasing a claim on a web app the store no longer holds is nothing to do: the
        // web app the claim was on is gone, and so is the claim.
        if let Some(web_app) = self.state.lock().web_apps.get_mut(&self.web_app_id) {
            web_app.customization_write_in_flight = false;
        }
    }
}

/// Take the claim on `web_app_id`'s customization, when the web app exists and no other
/// write holds it.
///
/// The caller must NOT hold the state lock: this takes it, and releases it before
/// answering.
pub fn claim(
    state: &::engine::ServiceState<State>,
    web_app_id: &str,
) -> Result<WriteClaim, Refusal> {
    {
        let mut guard = state.lock();
        let Some(web_app) = guard.web_apps.get_mut(web_app_id) else {
            return Err(Refusal::NoSuchWebApp);
        };
        if web_app.customization_write_in_flight {
            return Err(Refusal::Conflict);
        }
        web_app.customization_write_in_flight = true;
    }
    Ok(WriteClaim {
        state: state.clone(),
        web_app_id: web_app_id.to_string(),
    })
}

/// Let the runtime run whatever else is ready while this request holds the claim.
///
/// The claim is only worth taking if a request that is being handled AT THE SAME TIME
/// can be admitted far enough to see it: this is the point in a write where the peer
/// racing it reaches [`claim`], which is what makes one of two overlapping writers the
/// loser instead of both silently winning.
pub async fn admit_a_racing_writer() {
    ::tokio::task::yield_now().await;
}
