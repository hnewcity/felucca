use super::*;

fn raw(tag: u8) -> Vec<u8> {
    let mut packet = vec![0; 25 + tag as usize * 3];
    packet[..4].copy_from_slice(&42u32.to_le_bytes());
    packet[4] = 81;
    packet[12..16].copy_from_slice(&(tag as u32).to_le_bytes());
    let size = packet.len() - 24;
    packet[20..24].copy_from_slice(&(size as u32).to_le_bytes());
    packet[24..].fill(tag);
    packet
}
fn group(config: &KcpConfig) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut encoder = Encoder::new(config, 42).unwrap();
    let originals: Vec<_> = (0..config.data_shards).map(raw).collect();
    let mut frames = Vec::new();
    for packet in &originals {
        frames.extend(encoder.encode(packet).unwrap());
    }
    (originals, frames)
}

#[test]
fn ten_plus_three_recovers_every_three_packet_loss_combination_without_duplicates() {
    let config = KcpConfig::default();
    let (mut originals, frames) = group(&config);
    originals.sort();
    assert_eq!(frames.len(), 13);
    for a in 0..11 {
        for b in a + 1..12 {
            for c in b + 1..13 {
                let mut decoder = Decoder::new(&config, 42).unwrap();
                let mut output = Vec::new();
                let now = Instant::now();
                for (index, frame) in frames.iter().enumerate().rev() {
                    assert!(frame.len() <= config.mtu as usize);
                    if [a, b, c].contains(&index) {
                        continue;
                    }
                    output.extend(decoder.input(frame, now).unwrap());
                    assert!(decoder.input(frame, now).unwrap().is_empty());
                }
                output.sort();
                assert_eq!(output, originals, "lost indices {a}, {b}, {c}");
            }
        }
    }
}

#[test]
fn invalid_fec_metadata_is_rejected_before_shard_allocation() {
    let config = KcpConfig::default();
    let (_, frames) = group(&config);
    for (index, value) in [(0, 0), (12, 255), (13, 255), (14, 255), (15, 1), (16, 0)] {
        let mut frame = frames[0].clone();
        frame[index] = value;
        let mut decoder = Decoder::new(&config, 42).unwrap();
        assert!(
            decoder.input(&frame, Instant::now()).is_err(),
            "byte {index}"
        );
        assert!(decoder.groups.is_empty());
    }
    let mut short = frames[12].clone();
    short.pop();
    assert!(inspect(&short, &config).is_none());
    let mut wrong = Decoder::new(&config, 43).unwrap();
    assert!(wrong.input(&frames[0], Instant::now()).is_err());
    let mut encoder = Encoder::new(&config, 42).unwrap();
    let mut oversized = vec![0; raw_mtu(&config) + 1];
    oversized[..4].copy_from_slice(&42u32.to_le_bytes());
    assert!(encoder.encode(&oversized).is_err());
}

#[test]
fn incomplete_groups_expire_and_both_active_and_completed_caches_are_bounded() {
    let config = KcpConfig::default();
    let (_, frames) = group(&config);
    let mut decoder = Decoder::new(&config, 42).unwrap();
    let now = Instant::now();
    for id in 0..100u32 {
        let mut packet = frames[0].clone();
        packet[8..12].copy_from_slice(&id.to_le_bytes());
        decoder
            .input(&packet, now + Duration::from_millis(id as u64))
            .unwrap();
        assert!(decoder.groups.len() <= GROUPS);
    }
    assert_eq!(decoder.groups.len(), GROUPS);
    decoder
        .input(&frames[1], now + EXPIRY + Duration::from_secs(1))
        .unwrap();
    assert_eq!(decoder.groups.len(), 1);

    let config = KcpConfig {
        data_shards: 2,
        parity_shards: 1,
        ..Default::default()
    };
    let mut encoder = Encoder::new(&config, 42).unwrap();
    let mut decoder = Decoder::new(&config, 42).unwrap();
    for _ in 0..HISTORY + 10 {
        for tag in [0, 1] {
            for packet in encoder.encode(&raw(tag)).unwrap() {
                decoder.input(&packet, now).unwrap();
            }
        }
        assert!(decoder.completed.len() <= HISTORY);
    }
    assert_eq!(decoder.completed.len(), HISTORY);
}

#[test]
fn disabled_fec_leaves_raw_kcp_packets_unchanged() {
    let config = KcpConfig {
        data_shards: 0,
        parity_shards: 0,
        ..Default::default()
    };
    let mut encoder = Encoder::new(&config, 42).unwrap();
    let mut decoder = Decoder::new(&config, 42).unwrap();
    let original = raw(3);
    let frames = encoder.encode(&original).unwrap();
    assert_eq!(frames, vec![original.clone()]);
    assert_eq!(
        decoder.input(&frames[0], Instant::now()).unwrap(),
        vec![original]
    );
}
