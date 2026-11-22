//! Bounded Reed-Solomon packet groups. Data travels immediately; parity follows
//! each complete group. No group assembly delay is added to the data path.
use felucca_core::config::KcpConfig;
use reed_solomon_erasure::galois_8::ReedSolomon;
use std::{
    collections::{HashMap, VecDeque},
    io,
    time::Duration,
};
use tokio::time::Instant;

pub(super) const HEADER: usize = 18;
const MAGIC: &[u8; 4] = b"FKP5";
const GROUPS: usize = 32;
const HISTORY: usize = 128;
const EXPIRY: Duration = Duration::from_secs(2);

pub(super) fn raw_mtu(config: &KcpConfig) -> usize {
    config.mtu as usize
        - if config.parity_shards == 0 {
            0
        } else {
            HEADER + 2
        }
}
pub(super) struct Packet<'a> {
    pub conv: u32,
    pub data: Option<&'a [u8]>,
    group: u32,
    index: usize,
    shard: &'a [u8],
}
pub(super) fn inspect<'a>(packet: &'a [u8], config: &KcpConfig) -> Option<Packet<'a>> {
    if config.parity_shards == 0 {
        if !(24..=config.mtu as usize).contains(&packet.len()) {
            return None;
        }
        return Some(Packet {
            conv: u32::from_le_bytes(packet[..4].try_into().ok()?),
            data: Some(packet),
            group: 0,
            index: 0,
            shard: packet,
        });
    }
    if packet.len() <= HEADER
        || packet.len() > config.mtu as usize
        || &packet[..4] != MAGIC
        || packet[12] != config.data_shards
        || packet[13] != config.parity_shards
        || packet[15] != 0
        || packet[14] as usize >= config.data_shards as usize + config.parity_shards as usize
        || u16::from_le_bytes(packet[16..18].try_into().ok()?) as usize != raw_mtu(config) + 2
    {
        return None;
    }
    let conv = u32::from_le_bytes(packet[4..8].try_into().ok()?);
    if conv == 0 {
        return None;
    }
    let data = if packet[14] < config.data_shards {
        if !(24..=raw_mtu(config)).contains(&(packet.len() - HEADER))
            || u32::from_le_bytes(packet[HEADER..HEADER + 4].try_into().ok()?) != conv
        {
            return None;
        }
        Some(&packet[HEADER..])
    } else {
        if packet.len() != config.mtu as usize {
            return None;
        }
        None
    };
    Some(Packet {
        conv,
        data,
        group: u32::from_le_bytes(packet[8..12].try_into().ok()?),
        index: packet[14] as usize,
        shard: &packet[HEADER..],
    })
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid KCP FEC packet")
}
fn codec(config: &KcpConfig) -> io::Result<Option<ReedSolomon>> {
    if config.parity_shards == 0 {
        return Ok(None);
    }
    ReedSolomon::new(config.data_shards as usize, config.parity_shards as usize)
        .map(Some)
        .map_err(|error| io::Error::other(error.to_string()))
}
pub(super) struct Encoder {
    codec: Option<ReedSolomon>,
    config: KcpConfig,
    conv: u32,
    group: u32,
    shards: Vec<Vec<u8>>,
}
impl Encoder {
    pub fn new(config: &KcpConfig, conv: u32) -> io::Result<Self> {
        Ok(Self {
            codec: codec(config)?,
            config: config.clone(),
            conv,
            group: rand::random(),
            shards: Vec::new(),
        })
    }
    fn frame(&self, index: usize, payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(HEADER + payload.len());
        frame.extend_from_slice(MAGIC);
        frame.extend_from_slice(&self.conv.to_le_bytes());
        frame.extend_from_slice(&self.group.to_le_bytes());
        frame.extend_from_slice(&[
            self.config.data_shards,
            self.config.parity_shards,
            index as u8,
            0,
        ]);
        frame.extend_from_slice(&((raw_mtu(&self.config) + 2) as u16).to_le_bytes());
        frame.extend_from_slice(payload);
        frame
    }
    pub fn encode(&mut self, packet: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        let Some(codec) = &self.codec else {
            return Ok(vec![packet.to_vec()]);
        };
        if !(24..=raw_mtu(&self.config)).contains(&packet.len())
            || u32::from_le_bytes(packet[..4].try_into().unwrap()) != self.conv
        {
            return Err(invalid());
        }
        let mut frames = vec![self.frame(self.shards.len(), packet)];
        let mut shard = vec![0; raw_mtu(&self.config) + 2];
        shard[..2].copy_from_slice(&(packet.len() as u16).to_le_bytes());
        shard[2..2 + packet.len()].copy_from_slice(packet);
        self.shards.push(shard);
        if self.shards.len() == self.config.data_shards as usize {
            for _ in 0..self.config.parity_shards {
                self.shards.push(vec![0; raw_mtu(&self.config) + 2]);
            }
            codec
                .encode(&mut self.shards)
                .map_err(|error| io::Error::other(error.to_string()))?;
            for index in self.config.data_shards as usize..self.shards.len() {
                frames.push(self.frame(index, &self.shards[index]));
            }
            self.shards.clear();
            self.group = self.group.wrapping_add(1);
        }
        Ok(frames)
    }
}
struct Group {
    shards: Vec<Option<Vec<u8>>>,
    seen: u64,
    created: Instant,
}
pub(super) struct Decoder {
    codec: Option<ReedSolomon>,
    config: KcpConfig,
    conv: u32,
    groups: HashMap<u32, Group>,
    completed: VecDeque<(u32, Instant)>,
}
impl Decoder {
    pub fn new(config: &KcpConfig, conv: u32) -> io::Result<Self> {
        Ok(Self {
            codec: codec(config)?,
            config: config.clone(),
            conv,
            groups: HashMap::new(),
            completed: VecDeque::new(),
        })
    }
    pub fn input(&mut self, packet: &[u8], now: Instant) -> io::Result<Vec<Vec<u8>>> {
        let view = inspect(packet, &self.config).ok_or_else(invalid)?;
        if view.conv != self.conv {
            return Err(invalid());
        }
        let Some(codec) = &self.codec else {
            return Ok(vec![packet.to_vec()]);
        };
        self.groups
            .retain(|_, group| now.saturating_duration_since(group.created) < EXPIRY);
        while self
            .completed
            .front()
            .is_some_and(|(_, time)| now.saturating_duration_since(*time) >= EXPIRY)
        {
            self.completed.pop_front();
        }
        if self.completed.iter().any(|(group, _)| *group == view.group) {
            return Ok(Vec::new());
        }
        if !self.groups.contains_key(&view.group) && self.groups.len() == GROUPS {
            let oldest = *self
                .groups
                .iter()
                .min_by_key(|(_, group)| group.created)
                .unwrap()
                .0;
            self.groups.remove(&oldest);
        }
        let group = self.groups.entry(view.group).or_insert_with(|| Group {
            shards: vec![
                None;
                self.config.data_shards as usize + self.config.parity_shards as usize
            ],
            seen: 0,
            created: now,
        });
        if group.seen & (1u64 << view.index) != 0 {
            return Ok(Vec::new());
        }
        let mut output = Vec::new();
        let shard = if let Some(data) = view.data {
            output.push(data.to_vec());
            let mut shard = vec![0; raw_mtu(&self.config) + 2];
            shard[..2].copy_from_slice(&(data.len() as u16).to_le_bytes());
            shard[2..2 + data.len()].copy_from_slice(data);
            shard
        } else {
            view.shard.to_vec()
        };
        group.shards[view.index] = Some(shard);
        group.seen |= 1u64 << view.index;
        if group.seen.count_ones() as usize >= self.config.data_shards as usize {
            let mut group = self.groups.remove(&view.group).unwrap();
            let recovered = codec.reconstruct_data(&mut group.shards);
            // Retire even invalid groups; repeated bad parity cannot force
            // repeated reconstruction/allocation for the same group.
            self.completed.push_back((view.group, now));
            if self.completed.len() > HISTORY {
                self.completed.pop_front();
            }
            if recovered.is_err() {
                return Ok(output);
            }
            for index in 0..self.config.data_shards as usize {
                if group.seen & (1u64 << index) != 0 {
                    continue;
                }
                let shard = group.shards[index].as_ref().ok_or_else(invalid)?;
                let size = u16::from_le_bytes(shard[..2].try_into().unwrap()) as usize;
                if !(24..=raw_mtu(&self.config)).contains(&size) {
                    continue;
                }
                let data = &shard[2..2 + size];
                if u32::from_le_bytes(data[..4].try_into().unwrap()) == self.conv {
                    output.push(data.to_vec());
                }
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
#[path = "kcp_fec_tests.rs"]
mod tests;
