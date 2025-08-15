use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use borsh::{BorshDeserialize, BorshSerialize};
use libp2p::{
    kad::{ProviderRecord, Record},
    Multiaddr, PeerId,
};

use crate::{entry::Borsh, key, types::PredefinedEntry};

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct RecordMeta {
    pub expires: Option<u128>,
    pub key: key::RecordMeta,
    pub peer_id: Option<Vec<u8>>,
    pub value: Vec<u8>,
}

impl RecordMeta {
    #[must_use]
    pub fn new(record: Record) -> Self {
        Self {
            expires: record.expires.map(instant_to_timestamp),
            key: key::RecordMeta::new(&record.key).into(),
            peer_id: record.publisher.map(|a| a.to_bytes()),
            value: record.value,
        }
    }

    pub fn record(self) -> eyre::Result<Record> {
        Ok(Record {
            key: self.key.record(),
            value: self.value,
            expires: self.expires.and_then(timestamp_to_instant),
            publisher: if let Some(e) = self.peer_id {
                Some(PeerId::from_bytes(&e)?)
            } else {
                None
            },
        })
    }
}

impl PredefinedEntry for key::RecordMeta {
    type Codec = Borsh;
    type DataType<'a> = RecordMeta;
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct ProviderRecordMeta {
    pub expires: Option<u128>,
    pub key: key::RecordMeta,
    pub provider: Vec<u8>,
    pub addresses: Vec<Vec<u8>>,
}

impl ProviderRecordMeta {
    #[must_use]
    pub fn new(record: ProviderRecord) -> Self {
        Self {
            expires: record.expires.map(instant_to_timestamp),
            key: key::RecordMeta::new(&record.key).into(),
            addresses: record.addresses.into_iter().map(|a| a.to_vec()).collect(),
            provider: record.provider.to_bytes(),
        }
    }

    pub fn provider_record(self) -> eyre::Result<ProviderRecord> {
        let mut addresses = Vec::with_capacity(self.addresses.len());

        for peer_address_data in self.addresses {
            addresses.push(Multiaddr::try_from(peer_address_data)?);
        }

        Ok(ProviderRecord {
            key: self.key.record(),
            addresses,
            provider: PeerId::from_bytes(&self.provider)?,
            expires: self.expires.and_then(timestamp_to_instant),
        })
    }
}

impl PredefinedEntry for key::ProviderRecordMeta {
    type Codec = Borsh;
    type DataType<'a> = ProviderRecordMeta;
}

fn instant_to_timestamp(deadline: Instant) -> u128 {
    let duration_left = deadline.saturating_duration_since(Instant::now());
    let Some(abs) = SystemTime::now().checked_add(duration_left) else {
        return 0; // Time elapsed
    };

    match abs.duration_since(UNIX_EPOCH) {
        Ok(e) => e.as_micros(),
        Err(e) => 0,
    }
}

fn timestamp_to_instant(timestamp: u128) -> Option<Instant> {
    if timestamp == 0 {
        return Some(Instant::now() - Duration::from_secs(1));
    }

    let secs = timestamp / 1_000_000;

    if secs > u128::from(u64::MAX) {
        return None; // TODO: How do we handle such case.
    }
    let nanos = ((timestamp % 1_000_000) * 1_000) as u32;

    let target_sys = UNIX_EPOCH.checked_add(Duration::new(secs as u64, nanos))?;
    let now_s = SystemTime::now();
    let now_i = Instant::now();
    match target_sys.duration_since(now_s) {
        Ok(delta) => now_i.checked_add(delta),
        Err(e) => now_i.checked_sub(e.duration()),
    }
}
