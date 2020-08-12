use std::{
    collections::{BTreeMap, BTreeSet},
    convert::TryFrom,
};

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
    database::{
        rooms::{
            state::{EventContext, EventMap, StateCacheEntry, StateGroupId, StateId, StateMap},
            Rooms,
        },
        users::Users,
    },
    utils, Error, PduEvent, Result,
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
pub fn check_and_send_pdu_federation(db: &Rooms, users: &Users, pdu: &PduEvent) -> Result<()> {
    let context = create_event_context(db, users, pdu)?;

    // TODO send the damn thing
    Ok(())
}

pub fn create_event_context(db: &Rooms, users: &Users, pdu: &PduEvent) -> Result<EventContext> {
    let room_version = if pdu.kind == EventType::RoomCreate && pdu.state_key == Some("".to_owned())
    {
        serde_json::from_value::<create::CreateEventContent>(pdu.content.clone())
            .map_err(|_| Error::bad_database("Invalid create event in db."))?
            .room_version
    } else {
        db.get_room_version(&pdu.room_id)?
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
                content.displayname = users.displayname(&target)?;
            }

            if content.avatar_url.is_none() {
                content.avatar_url = users.avatar_url(&target)?;
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
    db: &Rooms,
    pdu: &PduEvent,
    prev_event_ids: Option<Vec<EventId>>,
) -> Result<EventContext> {
    let prev_events = if let Some(prev) = prev_event_ids {
        db.prev_events_matching_ids(&pdu.room_id, &prev)?
    } else {
        db.prev_events(&pdu.room_id).collect::<Result<Vec<_>>>()?
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

        if let Some(_already_exists) = db.state.annotated_by_user(rel, &pdu.sender)? {
            return Err(Error::Conflict("Can not send same reaction twice."));
        }
    }

    Ok(context)
}

fn compute_event_context(
    db: &Rooms,
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
    db: &Rooms,
    room_id: &RoomId,
    prev_event_ids: &[EventId],
) -> Result<StateCacheEntry> {
    // The state of the room at each previous event
    // in the forum of: state group id -> StateMap<EventId>
    let mut state_group_ids = db.state.get_state_group_ids(room_id, prev_event_ids)?;

    if state_group_ids.is_empty() {
        return Ok(StateCacheEntry::new(StateMap::new(), None, None, None));
    } else if state_group_ids.len() == 1 {
        // Unless rust's b-tree is broken this will never fail
        let k = *state_group_ids.keys().next().unwrap();
        let (name, state_map) = state_group_ids.remove_entry(&k).unwrap();

        // build the delta between this state group and the previous.
        // TODO we know there is no previous state group here Huh?? why does
        // synapse do this and not just use the current state as the delta?

        // let (prev_group, delta_ids) = get_state_group_delta(db, name)?;

        return Ok(StateCacheEntry {
            state_id: StateGroupId::Group(name),
            state: state_map,
            state_group: Some(name),
            prev_group: None,
            delta_ids: None, // FIXME
        });
    }

    // Either a state group ID or a `StateCacheEntry` ID
    let state_id = db
        .state
        .current_state_id()
        .ok_or(utils::to_db("No state group ID found in db."))?;
    // The state
    let state = db.state.current_state()?;
    let state_group = db.state.new_state_group_id().ok();

    let (prev_group, delta_ids) = get_state_group_delta(db, &state, state_id)?;

    // room_version is needed or can come from DB
    resolve_state_groups(db, room_id, state_group_ids, None)
}

pub fn resolve_state_groups(
    db: &Rooms,
    room_id: &RoomId,
    state_groups: BTreeMap<StateId, StateMap<EventId>>,
    event_map: Option<StateMap<EventId>>,
) -> Result<StateCacheEntry> {
    let mut new_state = StateMap::new();
    let mut conflicted = false;
    for st in state_groups.values() {
        for (k, ev_id) in st.iter() {
            if new_state.contains_key(k) {
                conflicted = true;
                break;
            }
            new_state.insert(k.clone(), ev_id.clone());
        }
        if conflicted {
            break;
        }
    }

    if conflicted {
        new_state =
            resolve_events_with_db(db, room_id, state_groups.values().cloned().collect(), None)?;
    }

    let cache = make_state_cache(db, new_state, state_groups)?;

    Ok(cache)
}

pub fn get_state_group_delta(
    db: &Rooms,
    current_state: &StateMap<EventId>,
    state_id: StateId,
) -> Result<(Option<StateId>, Option<StateMap<EventId>>)> {
    let prev_state_group_id = db.state.prev_state_id(state_id);
    let delta_ids = if let Some(prev) = &prev_state_group_id {
        let prev_state = db.state.get_statemap(*prev)?;

        let diff = prev_state.into_iter().collect::<BTreeSet<_>>();
        Some(
            // TODO remove the clones somehow ?
            diff.symmetric_difference(&current_state.clone().into_iter().collect())
                .cloned()
                .collect(),
        )
    } else {
        None
    };
    Ok((prev_state_group_id, delta_ids))
}

pub fn resolve_events_with_db(
    db: &Rooms, // Get room version from DB ?
    room_id: &RoomId,
    state_set: Vec<StateMap<EventId>>,
    event_map: Option<EventMap<PduEvent>>,
) -> Result<StateMap<EventId>> {
    let room_version = todo!();

    // constructing this is free there are no fields this will change to a free function soon
    let resolver = state_res::StateResolution::default();
    match resolver.resolve(room_id, room_version, &state_set, None, &db.state) {
        Ok(state_res::ResolutionResult::Resolved(res)) => Ok(res),
        _ => Err(Error::Conflict(&format!(
            "State resolution failed for {}",
            room_id.as_str()
        ))),
    }
}

pub fn make_state_cache(
    _db: &Rooms, // TODO some sort of mem caching
    new_state: StateMap<EventId>,
    state_groups: BTreeMap<StateId, StateMap<EventId>>,
) -> Result<StateCacheEntry> {
    let new_state_ev_ids = new_state.iter().collect::<BTreeSet<_>>();
    for (sg, state) in state_groups.iter() {
        if new_state_ev_ids.len() != state.len() {
            continue;
        }

        let old_state_ev_ids = state.iter().collect::<BTreeSet<_>>();
        if new_state_ev_ids == old_state_ev_ids {
            return Ok(StateCacheEntry::new(new_state, Some(*sg), None, None));
        }
    }

    let mut prev_group = None;
    let mut delta_ids = None;
    let mut delta_len = 0;

    for (old_group, old_state) in state_groups.iter() {
        let n_delta = new_state
            .iter()
            .filter(|(k, v)| old_state.get(k) != Some(v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<StateMap<_>>();

        // TODO There is also a check if `delta_ids.is_none()` in synapse,
        // because of mutability and borrows we cannot do that but
        // n_delta will never be smaller than 0 so the check should work
        // the same.
        if n_delta.len() > delta_len {
            delta_len = n_delta.len();

            prev_group = Some(*old_group);
            delta_ids = Some(n_delta);
        }
    }

    Ok(StateCacheEntry::new(new_state, None, prev_group, delta_ids))
}
