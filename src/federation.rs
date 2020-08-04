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
    pub delta: StateMap<EventId>,

    /// If this event is sent by a (local) app-service.
    pub app_service: Option<AppService>,

    /// The room `StateMap` including the event to be sent.
    current_state_ids: StateMap<EventId>,

    /// The room `StateMap`, excluding the event to be sent. This would be the state
    /// represented by `state_group_before_event`.
    prev_state_ids: StateMap<EventId>,
}

pub fn check_and_send_pdu_federation(db: &Database, pdu: &PduEvent) -> Result<()> {
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
) -> Result<()> {
    todo!()
}

fn create_new_client_event(
    db: &Database,
    pdu: &PduEvent,
    prev_event_ids: Option<Vec<EventId>>,
) -> Result<()> {
    let prev_events = if let Some(prev) = prev_event_ids {
        db.rooms.prev_events_matching_ids(&pdu.room_id, &prev)?
    } else {
        db.rooms
            .prev_events(&pdu.room_id)
            .collect::<Result<Vec<_>>>()?
    };

    let context = compute_event_context(pdu, None)?;

    Ok(())
}

fn compute_event_context(pdu: &PduEvent, old_state: Option<&[PduEvent]>) -> Result<EventContext> {
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
            // TODO is insert ok
            curr.insert(
                (pdu.kind.clone(), pdu.state_key.clone()),
                pdu.event_id.clone(),
            );
            (curr, prev)
        } else {
            (curr, prev)
        }
    } else {
        todo!()
    };

    todo!()
}
