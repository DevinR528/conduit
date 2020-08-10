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

use crate::{
    database::rooms::state::{EventContext, StateCacheEntry, StateGroupId, StateId, StateMap},
    utils, Database, Error, PduEvent, Result,
};

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

    let state_ids_before;
    let state_group_before;
    let state_group_before_event_prev_group;
    let deltas_to_state_group;
    if let Some(old) = old_state {
        state_ids_before = old
            .iter()
            .map(pdu_to_state_map)
            .collect::<StateMap<EventId>>();

        state_group_before = None;
        state_group_before_event_prev_group = None;
        deltas_to_state_group = None;
    } else {
        let entry = resolve_state_group_for_events(db, &pdu.room_id, &pdu.prev_events)?;

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
    db: &Database,
    room_id: &RoomId,
    prev_event_ids: &[EventId],
) -> Result<StateCacheEntry> {
    // The state of the room at each previous event
    // in the forum of: state group id -> StateMap<EventId>
    let state_group_ids = db
        .rooms
        .state
        .get_state_group_ids(room_id, prev_event_ids)?;

    if state_group_ids.is_empty() {
        return Ok(StateCacheEntry::new(StateMap::new(), None, None, None));
    } else if state_group_ids.len() == 1 {
        // Unless rust's b-tree is broken this will never fail
        let k = state_group_ids.keys().next().unwrap();
        let (name, state_map) = state_group_ids.remove_entry(k).unwrap();

        // build the delta between this state group and the previous
        let (prev_group, delta_ids) = get_state_group_delta(db, name)?;

        return Ok(StateCacheEntry {
            state_id: StateGroupId::Group(name),
            state: state_map,
            state_group: Some(name),
            prev_group,
            delta_ids,
        });
    }
    // Either a state group ID or a `StateCacheEntry` ID
    let state_id = db
        .rooms
        .state
        .current_state_id()
        .ok_or(utils::to_db("No state group ID found in db."))?;
    // The state
    let state = db.rooms.state.current_state()?;
    let state_group = db.rooms.state.new_state_group_id().ok();

    let (prev_group, delta_ids) = get_state_group_delta(db, state_id)?;

    Ok(StateCacheEntry {
        state,
        state_group,
        state_id: StateGroupId::Group(state_id),
        prev_group,
        delta_ids,
    })
}

pub fn resolve_state_groups(db: &Database) -> Result<StateCacheEntry> {
    todo!()
}

pub fn get_state_group_delta(
    db: &Database,
    state_id: StateId,
) -> Result<(Option<StateId>, Option<StateMap<EventId>>)> {
    Ok((db.rooms.state.prev_state_id(state_id), todo!()))
}
