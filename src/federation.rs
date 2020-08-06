use std::{collections::BTreeMap, convert::TryFrom};

use ruma::{
    api::client::error::ErrorKind,
    events::{
        room::create,
        room::member::{self, MembershipState},
        EventContent, EventType,
    },
    EventId, Raw, RoomId, UserId,
};

use crate::{Database, Error, PduEvent, Result};

/// A mapping of (event_type, state_key) -> `T`, usually `EventId` or `Pdu`.
pub type StateMap<T> = BTreeMap<(EventType, Option<String>), T>;

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

pub struct EventContext {
    /// The ID of the state group for this event. Note that state events
    /// are persisted with a state group which includes the new event, so this is
    /// effectively the state *after* the event in question.
    // TODO this is a unique ID give it a type ?
    state_group: Option<String>,

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
    current_state_ids: StateMap<EventId>,

    /// The room `StateMap`, excluding the event to be sent. This would be the state
    /// represented by `state_group_before_event`.
    prev_state_ids: StateMap<EventId>,
}

pub struct StateCacheEntry {
    pub state: StateMap<EventId>,

    /// The ID of the state group if one and only one is involved.
    /// [synapse] then says "otherwise, None otherwise?" what does this mean
    state_group: Option<String>,

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

fn pdu_to_state_map(pdu: &PduEvent) -> ((EventType, Option<String>), EventId) {
    (
        (pdu.kind.clone(), pdu.state_key.clone()),
        pdu.event_id.clone(),
    )
}

//
//
//

/// Validate then send an event out to other servers after persisting it locally.
///
/// The whole point of federation.
pub fn check_and_send_pdu_federation(db: &Database, pdu: &PduEvent) -> Result<()> {
    let context = create_event_context(db, pdu)?;

    // TODO send the damn thing
    Ok(())
}

pub fn create_event_context(db: &Database, pdu: &PduEvent) -> Result<EventContext> {
    let room_version = if pdu.kind == EventType::RoomCreate && pdu.state_key == Some("".to_owned())
    {
        serde_json::from_value::<create::CreateEventContent>(pdu.content.clone())
            .map_err(|_| Error::bad_database("Invalid create event in db."))?
            .room_version
    } else {
        db.rooms
            .get_room_version(&pdu.room_id)?
            .ok_or_else(|| Error::BadRequest(ErrorKind::Unknown, "Create event not found."))?
    };

    // TODO validate event based on room_version and PDU

    if pdu.kind == EventType::RoomMember {
        let mut content = serde_json::from_value::<member::MemberEventContent>(pdu.content.clone())
            .map_err(|_| Error::bad_database("Invalid create event in db."))?;

        let membership = &content.membership;
        let target = UserId::try_from(pdu.state_key.as_deref().ok_or(Error::BadRequest(
            ErrorKind::Unauthorized,
            "Member event missing state_key",
        ))?)
        .map_err(|_| Error::bad_database("Invalid event id found"))?;

        if [MembershipState::Invite, MembershipState::Join].contains(&membership) {
            if content.displayname.is_none() {
                content.displayname = db.users.displayname(&target)?;
            }

            if content.avatar_url.is_none() {
                content.avatar_url = db.users.avatar_url(&target)?;
            }
        }

        return create_new_client_event_with_content(pdu, content, None);
    }

    // TODO determine if event is exempt from privacy policy ?
    // TODO then check privacy policy has been accepted

    // TODO handle token_id (access token) and txn_id

    create_new_client_event(db, pdu, None)
}

fn create_new_client_event_with_content<T: EventContent>(
    pdu: &PduEvent,
    content: T,
    prev_event_ids: Option<&[EventId]>,
) -> Result<EventContext> {
    todo!()
}

fn create_new_client_event(
    db: &Database,
    pdu: &PduEvent,
    prev_event_ids: Option<Vec<EventId>>,
) -> Result<EventContext> {
    let prev_events = if let Some(prev) = prev_event_ids {
        db.rooms.prev_events_matching_ids(&pdu.room_id, &prev)?
    } else {
        db.rooms
            .prev_events(&pdu.room_id)
            .collect::<Result<Vec<_>>>()?
    };

    let context = compute_event_context(db, pdu, None)?;

    // TODO [synapse] `context.app_service = requester.app_service` requester
    // would be the PduEvent in our case, I think...

    // Synapse validates the event here, ruma does this for us.
    // TODO retention events are also validated here, max/min_lifetime

    if let Some(relates_to) = pdu.content.get("m.relates_to") {
        let relates =
            serde_json::from_value::<ruma::events::room::message::RelatesTo>(relates_to.clone())
                .map_err(|_| {
                    Error::bad_database("Content with invalid 'm.relates_to' field in db.")
                })?;

        let rel = &relates.in_reply_to.event_id;

        if let Some(_already_exists) = db.rooms.state.annotated_by_user(rel, &pdu.sender)? {
            return Err(Error::Conflict("Can not send same reaction twice."));
        }
    }

    Ok(context)
}

fn compute_event_context(
    db: &Database,
    pdu: &PduEvent,
    old_state: Option<&[PduEvent]>,
) -> Result<EventContext> {
    // TODO check if event is "outlier"
    if pdu.unsigned.get("outlier").is_some() {
        let (curr_state, prev_state) = if let Some(old) = old_state {
            let prev = old
                .iter()
                .map(|pdu| {
                    (
                        (pdu.kind.clone(), pdu.state_key.clone()),
                        pdu.event_id.clone(),
                    )
                })
                .collect::<StateMap<EventId>>();

            let mut curr = prev.clone();
            if pdu.is_state() {
                curr.insert(
                    (pdu.kind.clone(), pdu.state_key.clone()),
                    pdu.event_id.clone(),
                );
                (curr, prev)
            } else {
                (curr, prev)
            }
        } else {
            (StateMap::new(), StateMap::new())
        };

        return Ok(EventContext {
            state_group: None,
            state_group_before_event: None,
            delta: None,
            prev_group: None,
            current_state_ids: curr_state,
            prev_state_ids: prev_state,
            app_service: None,
        });
    };

    //
    // Now that that's out of the way compute the past state
    //

    let mut state_ids_before;
    let mut state_group_before;
    let mut state_group_before_event_prev_group;
    let mut deltas_to_state_group;
    if let Some(old) = old_state {
        state_ids_before = old
            .iter()
            .map(pdu_to_state_map)
            .collect::<StateMap<EventId>>();

        state_group_before = None;
        state_group_before_event_prev_group = None;
        deltas_to_state_group = None;
    } else {
        let entry = resolve_state_group_for_events(&pdu.event_id, &pdu.prev_events)?;

        state_ids_before = entry.state;

        state_group_before = entry.state_group;
        state_group_before_event_prev_group = entry.prev_group;
        deltas_to_state_group = entry.delta_ids;
    };

    // Make sure we have a state_group at this point

    if state_group_before.is_none() {
        // state_group_before = db.rooms.state.get_state_group();
    }

    todo!()
}

pub fn resolve_state_group_for_events(
    event_id: &EventId,
    event_ids: &[EventId],
) -> Result<StateCacheEntry> {
    todo!()
}
