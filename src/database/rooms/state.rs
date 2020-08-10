use crate::{utils, Error, PduEvent, Result};
use js_int::UInt;
use ruma::{
    events::{
        presence::{PresenceEvent, PresenceEventContent},
        AnyEvent as EduEvent, EventType, SyncEphemeralRoomEvent,
    },
    presence::PresenceState,
    EventId, Raw, RoomId, UserId,
};
use std::{
    collections::{BTreeMap, HashMap},
    convert::{TryFrom, TryInto},
    mem,
};

/// A mapping of (event_type, state_key) -> `T`, usually `EventId` or `Pdu`.
pub type StateMap<T> = BTreeMap<(EventType, Option<String>), T>;

pub type StateGroupId = u64;

pub enum StateId {
    Cached(String),
    Group(String),
}

/// TODO
pub enum AppService {
    A,
    B,
    C,
}

// TODO these are the database, the group ids are keys and the StateMap is the values. A method
// to reconstruct them from a state group id and other keys will be needed. Break this up into sled::Tree's

pub struct EventContext {
    /// The ID of the state group for this event. Note that state events
    /// are persisted with a state group which includes the new event, so this is
    /// effectively the state *after* the event in question.
    // TODO this is a unique ID give it a type ?
    pub(crate) state_group: Option<String>,

    /// The state group id of the previous event. If this is not a state event it will
    /// be the same as `state_group`.
    pub state_group_before_event: Option<String>,

    /// The previous state group. Not necessarily related to `prev_group`, or `prev_state_ids`.
    pub prev_group: Option<String>,

    /// The difference between `prev_group` and `state_group`, if present.
    pub delta: Option<StateMap<EventId>>,

    /// If this event is sent by a (local) app-service.
    pub app_service: Option<AppService>,

    /// The room `StateMap` including the event to be sent.
    pub(crate) current_state_ids: StateMap<EventId>,

    /// The room `StateMap`, excluding the event to be sent. This would be the state
    /// represented by `state_group_before_event`.
    pub(crate) prev_state_ids: StateMap<EventId>,
}

pub struct StateCacheEntry {
    pub state: StateMap<EventId>,

    /// The ID of the state group if one and only one is involved.
    /// [synapse] then says "otherwise, None otherwise?" what does this mean
    pub(crate) state_group: Option<String>,

    /// The unique ID of this resolved state group. [synapse] This may be a state_group id
    /// or a cached state entry but the two should not be confused.
    pub state_id: StateId,

    /// The ID of the previous resolved state group.
    pub prev_group: Option<String>,

    pub delta_ids: Option<StateMap<EventId>>,
}

impl StateCacheEntry {
    pub fn new(
        state: StateMap<EventId>,
        state_group: Option<String>,
        prev_group: Option<String>,
        delta_ids: Option<StateMap<EventId>>,
    ) -> Self {
        Self {
            state,
            state_id: if let Some(id) = state_group.as_ref() {
                StateId::Group(id.clone())
            } else {
                StateId::Cached(gen_state_id())
            },
            state_group,
            prev_group,
            delta_ids,
        }
    }
}

fn gen_state_id() -> String {
    crate::utils::random_string(10)
}

pub struct RoomState {
    /// related EventId + Sender -> Pdu (the event that relates to "related EventId").
    // example: ^^         ^^                       ^^
    /// A message event   the sender of this Pdu    Pdu of a emoji reaction
    pub(in super::super) eventiduser_pdu: sled::Tree,

    /// A numeric ID assigned to every event -> PDU
    pub(in super::super) eventnumid_pdu: sled::Tree,

    //
    /// A numeric ID assigned to every event -> EventId
    pub(in super::super) eventnumid_eventid: sled::Tree,
    /// Reverse mapping of (RoomId EventId) -> its numeric event id
    pub(in super::super) roomideventid_eventnumid: sled::Tree,
    /// eventid -> EventType
    pub(in super::super) eventnumid_eventtype: sled::Tree,
    /// eventid -> state_key
    pub(in super::super) eventnumid_statekey: sled::Tree,

    //
    /// Numeric event ID -> Numeric id of a state snapshot
    pub(in super::super) eventnumid_snapshotid: sled::Tree,

    /// Numeric snapshot ID -> Numeric state group ID
    pub(in super::super) snapshotnumid_stategroupid: sled::Tree,

    /// Numeric state group ID -> range of eventnumid's
    ///
    /// The range allows iteration through a slice of any Tree with a eventnumid key.
    /// They are the valid state events at the time of an incoming event being
    /// resolved and added.
    pub(in super::super) stategroupid_eventnumidrange: sled::Tree,
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
        let mut prefix = event_id.as_bytes().to_vec();
        prefix.push(0xff);
        prefix.extend_from_slice(sender.as_bytes());

        self.eventiduser_pdu
            .get(prefix)?
            .map_or(Err(utils::to_db("PDU in db is invalid.")), |b| {
                utils::deserialize(&b)
            })
    }

    /// Returns a mapping of `StateGroupId` to StateMap<EventId>.
    /// The state at `event_ids` represents the state at that point in time.
    pub fn get_state_group_ids(
        &self,
        room_id: &RoomId,
        event_ids: &[EventId],
    ) -> Result<BTreeMap<StateGroupId, StateMap<EventId>>> {
        let mut prefix = room_id.as_str().as_bytes().to_vec();
        prefix.push(0xff);

        let mut state_groups = BTreeMap::new();
        for id in event_ids {
            let mut prefix = prefix.to_vec();
            prefix.extend(id.as_bytes());

            let state_id = self.roomideventid_eventnumid.get(prefix)?;

            if let Some(state_group_id) = state_id {
                if let Some(range) = self.stategroupid_eventnumidrange.get(state_group_id)? {
                    state_groups.insert(
                        utils::u64_from_bytes(&state_group_id)
                            .map_err(|_| utils::to_db("Invalid bytes to u64 in db."))?,
                        self.statemap_from_numid_range(range)?,
                    );
                } else {
                    // TODO Error
                }
            } else {
                // TODO is this an Error ?
            }
        }

        Ok(state_groups)
    }

    ///
    pub fn statemap_from_numid_range(&self, range: sled::IVec) -> Result<StateMap<EventId>> {
        let from = &range[..mem::size_of::<u64>()];
        let to = &range[mem::size_of::<u64>()..];

        self.eventnumid_eventtype
            .range(from..to)
            .zip(self.eventnumid_statekey.range(from..to))
            .filter_map(|(ty, key)| Some((&ty.ok()?.1, &key.ok()?.1)))
            .zip(self.eventnumid_eventid.range(from..to))
            .filter_map(|(key, id)| Some((key, &id.ok()?.1)))
            .map(|((ty, key), id)| {
                let ev_type: EventType = utils::string_from_bytes(ty)
                    .map_err(|_| utils::to_db("Invalid bytes to u64 in db."))?
                    .into();
                Ok((
                    (
                        ev_type,
                        // TODO this needs to be Option<state_key> save in the DB
                        utils::string_from_bytes(key).ok(),
                    ),
                    EventId::try_from(
                        utils::string_from_bytes(id)
                            .map_err(|_| utils::to_db("Invalid bytes to u64 in db."))?,
                    )
                    .map_err(|_| utils::to_db("Invalid bytes to u64 in db."))?,
                ))
            })
            .collect::<Result<StateMap<_>>>()
    }

    ///
    pub fn current_state_id(&self) -> Result<StateMap<EventId>> {
        let from = &range[..mem::size_of::<u64>()];
        let to = &range[mem::size_of::<u64>()..];

        self.eventnumid_eventtype
            .range(from..to)
            .zip(self.eventnumid_statekey.range(from..to))
            .filter_map(|(ty, key)| Some((&ty.ok()?.1, &key.ok()?.1)))
            .zip(self.eventnumid_eventid.range(from..to))
            .filter_map(|(key, id)| Some((key, &id.ok()?.1)))
            .map(|((ty, key), id)| {
                let ev_type: EventType = utils::string_from_bytes(ty)
                    .map_err(|_| utils::to_db("Invalid bytes to u64 in db."))?
                    .into();
                Ok((
                    (
                        ev_type,
                        // TODO this needs to be Option<state_key> save in the DB
                        utils::string_from_bytes(key).ok(),
                    ),
                    EventId::try_from(
                        utils::string_from_bytes(id)
                            .map_err(|_| utils::to_db("Invalid bytes to u64 in db."))?,
                    )
                    .map_err(|_| utils::to_db("Invalid bytes to u64 in db."))?,
                ))
            })
            .collect::<Result<StateMap<_>>>()
    }
}
