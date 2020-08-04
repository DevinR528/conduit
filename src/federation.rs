use ruma::{
    api::client::{
        error::ErrorKind,
        r0::membership::{
            ban_user, forget_room, get_member_events, invite_user, join_room_by_id,
            join_room_by_id_or_alias, joined_members, joined_rooms, kick_user, leave_room,
            unban_user,
        },
    },
    events::{room::member, EventType},
    Raw, RoomId,
};

use crate::{Database, PduEvent, Result};

pub fn check_and_send_pdu_federation(db: &Database, pdu: &PduEvent) -> Result<()> {
    if pdu.kind == EventType::RoomCreate && pdu.state_key == Some("".to_owned()) {
    } else {
    }

    if pdu.kind == EventType::RoomMember {}
    unimplemented!()
}
