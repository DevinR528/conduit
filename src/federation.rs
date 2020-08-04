use std::convert::TryFrom;

use ruma::{
    api::client::error::ErrorKind,
    events::{
        room::create,
        room::member::{self, MembershipState},
        EventType,
    },
    Raw, RoomId, UserId,
};

use crate::{Database, Error, PduEvent, Result};

pub fn check_and_send_pdu_federation(db: &Database, pdu: &PduEvent) -> Result<()> {
    let room_version = if pdu.kind == EventType::RoomCreate && pdu.state_key == Some("".to_owned())
    {
        serde_json::from_value::<create::CreateEventContent>(pdu.content)
            .map_err(|_| Error::bad_database("Invalid create event in db."))?
            .room_version
    } else {
        db.rooms
            .get_room_version(&pdu.room_id)?
            .ok_or_else(|| Error::BadRequest(ErrorKind::Unknown, "Create event not found."))?
    };

    // TODO validate event based on room_version and PDU

    if pdu.kind == EventType::RoomMember {
        let mut content = serde_json::from_value::<member::MemberEventContent>(pdu.content)
            .map_err(|_| Error::bad_database("Invalid create event in db."))?;

        let membership = &content.membership;
        let target = UserId::try_from(pdu.state_key.ok_or(Error::BadRequest(
            ErrorKind::Unauthorized,
            "Member event missing state_key",
        ))?)?;

        if [MembershipState::Invite, MembershipState::Join].contains(&membership) {
            if content.displayname.is_none() {
                content.displayname = db.users.displayname(&target)?;
            }

            if content.avatar_url.is_none() {
                content.avatar_url = db.users.avatar_url(&target)?;
            }
        }
    }

    // TODO determine if event is exempt from privacy policy ?
    // TODO then check privacy policy has been accepted

    // TODO handle token_id (access token) and txn_id

    create_new_client_event(pdu, content, &[])?;

    unimplemented!()
}
