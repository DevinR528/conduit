use crate::{utils, Error, PduEvent, Result};
use js_int::UInt;
use ruma::{
    events::{
        presence::{PresenceEvent, PresenceEventContent},
        AnyEvent as EduEvent, SyncEphemeralRoomEvent,
    },
    presence::PresenceState,
    EventId, Raw, RoomId, UserId,
};
use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};

pub struct RoomState {
    // related EventId + Sender -> Pdu (the event that relates to "related EventId")
    // example ^^         ^^                       ^^
    // a message event   the sender of this Pdu    Pdu of a emoji reaction
    pub(in super::super) eventiduser_pdu: sled::Tree,

    // StateId -> EventContext
    pub(in super::super) stategroupid_context: sled::Tree,

    // StateId -> StateCacheEntry
    pub(in super::super) stategroupid_cachedstate: sled::Tree,
}

impl RoomState {
    /// The `event_id` from the "m.relates_to" field and the `sender` of the event
    /// creating the relationship.
    ///
    /// The `event_id` is a message event and the `sender` adds an emoji to the message (reaction event).
    pub fn annotated_by_user(
        &self,
        event_id: &EventId,
        sender: &UserId,
    ) -> Result<Option<PduEvent>> {
        let mut prefix = event_id.as_str().as_bytes().to_vec();
        prefix.push(0xff);
        prefix.extend_from_slice(&sender.as_str().as_bytes());

        self.eventiduser_pdu
            .get(prefix)?
            .map_or(Err(utils::to_db("PDU in db is invalid.")), |b| {
                utils::deserialize(&b)
            })
    }
}
