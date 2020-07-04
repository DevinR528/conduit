use crate::{utils, Error, Result};
use ruma::{
    api::client::error::ErrorKind,
    events::{AnyEvent as EduEvent, EventJson, EventType},
    identifiers::{RoomId, UserId},
};
use sled::IVec;
use std::{collections::HashMap, convert::TryFrom};

pub struct AccountData {
    pub(super) roomuserdataid_accountdata: sled::Tree, // RoomUserDataId = Room + User + Count + Type
}

impl AccountData {
    /// Places one event in the account data of the user and removes the previous entry.
    pub fn update<T: ruma::events::Event>(
        &self,
        room_id: Option<&RoomId>,
        user_id: &UserId,
        event: &T,
        globals: &super::globals::Globals,
    ) -> Result<()> {
        let user_id_string = user_id.to_string();
        let kind_string = event.event_type().to_string();

        let mut prefix = room_id
            .map(|r| r.to_string())
            .unwrap_or_default()
            .as_bytes()
            .to_vec();
        prefix.push(0xff);
        prefix.extend_from_slice(&user_id_string.as_bytes());
        prefix.push(0xff);

        // Remove old entry
        if let Some((old, _)) = self
            .find_events_of_type(room_id, user_id, &event.event_type())
            .next()
        {
            self.roomuserdataid_accountdata.remove(old)?;
        }

        let mut key = prefix;
        key.extend_from_slice(&globals.next_count()?.to_be_bytes());
        key.push(0xff);
        key.extend_from_slice(kind_string.as_bytes());

        self.roomuserdataid_accountdata.insert(
            key,
            &*serde_json::to_string(&event).expect("Map::to_string always works"),
        )?;

        Ok(())
    }

    pub fn get<T: ruma::events::TryFromRaw>(
        &self,
        room_id: Option<&RoomId>,
        user_id: &UserId,
        kind: EventType,
    ) -> Result<Option<T>> {
        self.find_events_of_type(room_id, user_id, &kind)
            .map(|(_, v)| AccountData::deserialize_to_type(&v))
            .next()
            .transpose()
    }

    /// Returns all changes to the account data that happened after `since`.
    pub fn changes_since(
        &self,
        room_id: Option<&RoomId>,
        user_id: &UserId,
        since: u64,
    ) -> Result<HashMap<EventType, EventJson<EduEvent>>> {
        let mut userdata = HashMap::new();

        let mut prefix = room_id
            .map(|r| r.to_string())
            .unwrap_or_default()
            .as_bytes()
            .to_vec();
        prefix.push(0xff);
        prefix.extend_from_slice(&user_id.to_string().as_bytes());
        prefix.push(0xff);

        // Skip the data that's exactly at since, because we sent that last time
        let mut first_possible = prefix.clone();
        first_possible.extend_from_slice(&(since + 1).to_be_bytes());

        for r in self
            .roomuserdataid_accountdata
            .range(&*first_possible..)
            .filter_map(|r| r.ok())
            .take_while(move |(k, _)| k.starts_with(&prefix))
            .map(|(k, v)| {
                Ok::<_, Error>((
                    EventType::try_from(
                        utils::string_from_bytes(k.rsplit(|&b| b == 0xff).next().ok_or_else(
                            || Error::bad_database("RoomUserData ID in db is invalid."),
                        )?)
                        .map_err(|_| Error::bad_database("RoomUserData ID in db is invalid."))?,
                    )
                    .map_err(|_| Error::bad_database("RoomUserData ID in db is invalid."))?,
                    serde_json::from_slice::<EventJson<EduEvent>>(&v).map_err(|_| {
                        Error::bad_database("Database contains invalid account data.")
                    })?,
                ))
            })
        {
            let (kind, data) = r?;
            userdata.insert(kind, data);
        }

        Ok(userdata)
    }

    fn find_events_of_type(
        &self,
        room_id: Option<&RoomId>,
        user_id: &UserId,
        kind: &EventType,
    ) -> impl Iterator<Item = (IVec, IVec)> {
        let mut prefix = room_id
            .map(|r| r.to_string())
            .unwrap_or_default()
            .as_bytes()
            .to_vec();
        prefix.push(0xff);
        prefix.extend_from_slice(&user_id.to_string().as_bytes());
        prefix.push(0xff);
        let kind = kind.clone();

        self.roomuserdataid_accountdata
            .scan_prefix(prefix)
            .rev()
            .filter_map(|v| v.ok())
            .filter(move |(k, _)| AccountData::key_matches_with_event_type(&kind, k))
    }

    fn deserialize_to_type<T: ruma::events::TryFromRaw>(v: &IVec) -> Result<T> {
        serde_json::from_slice::<EventJson<T>>(&v)
            .expect("from_slice always works")
            .deserialize()
            .map_err(|_| Error::BadDatabase("could not deserialize"))
    }

    fn key_matches_with_event_type(kind: &EventType, k: &IVec) -> bool {
        k.rsplit(|&b| b == 0xff)
            .next()
            .map(|current_event_type| current_event_type == kind.to_string().as_bytes())
            .unwrap_or(false)
    }
}
