use super::*;
use tokio::io::DuplexStream;

async fn pair(
    options: PayloadOptions,
    capacity: usize,
) -> (PayloadStream<DuplexStream>, PayloadStream<DuplexStream>) {
    let (left, right) = tokio::io::duplex(capacity);
    let key = Secret::new("a-test-only-psk");
    tokio::try_join!(
        PayloadStream::negotiate(left, options, &key, b"route1/source/target", Side::Server),
        PayloadStream::negotiate(right, options, &key, b"route1/source/target", Side::Agent),
    )
    .unwrap()
}

#[tokio::test]
async fn payload_streams_preserve_bulk_backpressure_and_independent_half_close() {
    for compress in [false, true] {
        for encrypt in [false, true] {
            let (server, agent) = pair(PayloadOptions { compress, encrypt }, 127).await;
            let echo = tokio::spawn(async move {
                let (mut read, mut write) = tokio::io::split(agent);
                tokio::io::copy(&mut read, &mut write).await.unwrap();
                write.write_all(b"response after EOF").await.unwrap();
                write.shutdown().await.unwrap();
            });
            let mut payload = vec![b'A'; 2 * 1024 * 1024];
            let random: Vec<u8> = (0..65536).map(|_| rand::random()).collect();
            payload[65536..131072].copy_from_slice(&random);
            let (mut read, mut write) = tokio::io::split(server);
            let mut received = Vec::new();
            tokio::time::timeout(Duration::from_secs(30), async {
                let (sent, read) = tokio::join!(
                    async {
                        assert_eq!(write.write(&[]).await.unwrap(), 0);
                        write.write_all(&payload).await?;
                        write.shutdown().await
                    },
                    read.read_to_end(&mut received),
                );
                sent.unwrap();
                read.unwrap();
                echo.await.unwrap();
            })
            .await
            .unwrap_or_else(|_| panic!("bulk timed out compress={compress} encrypt={encrypt}"));
            payload.extend_from_slice(b"response after EOF");
            assert_eq!(received, payload);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn payload_handshake_binds_both_keys_options_and_routing_context() {
    for mismatch in ["key", "context", "compression", "direction"] {
        let (left, right) = tokio::io::duplex(1024);
        let options = PayloadOptions {
            compress: true,
            encrypt: true,
        };
        let other = PayloadOptions {
            compress: mismatch != "compression",
            encrypt: true,
        };
        let key = Secret::new("key");
        let wrong = Secret::new(if mismatch == "key" { "wrong" } else { "key" });
        let (server, agent) = tokio::join!(
            PayloadStream::negotiate(left, options, &key, b"route1", Side::Server),
            PayloadStream::negotiate(
                right,
                other,
                &wrong,
                if mismatch == "context" {
                    b"route2"
                } else {
                    b"route1"
                },
                if mismatch == "direction" {
                    Side::Server
                } else {
                    Side::Agent
                }
            ),
        );
        assert!(server.is_err() && agent.is_err(), "{mismatch}");
    }
}

fn cipher() -> aead::LessSafeKey {
    aead::LessSafeKey::new(aead::UnboundKey::new(&aead::CHACHA20_POLY1305, &[7; 32]).unwrap())
}
fn writer(encrypted: bool) -> Writer {
    Writer {
        cipher: encrypted.then(cipher),
        ..Default::default()
    }
}
fn reader(encrypted: bool) -> Reader {
    Reader {
        cipher: encrypted.then(cipher),
        ..Default::default()
    }
}
fn decode(reader: &mut Reader, frame: &[u8], options: PayloadOptions) -> io::Result<()> {
    reader.header.copy_from_slice(&frame[..4]);
    reader.frame = frame[4..].to_vec();
    reader.decode(options)
}

#[test]
fn records_compress_only_when_smaller_and_authenticate_the_size_and_sequence() {
    let options = PayloadOptions {
        compress: true,
        encrypt: true,
    };
    let input = vec![b'X'; BLOCK];
    let mut send = writer(true);
    send.encode(&input, options, false).unwrap();
    let frame = send.frame.clone();
    assert!(frame.len() < input.len() / 10);
    assert!(!frame.windows(16).any(|window| window == [b'X'; 16]));
    let mut good = reader(true);
    decode(&mut good, &frame, options).unwrap();
    assert_eq!(good.plain.as_ref(), input);
    assert!(
        decode(&mut good, &frame, options).is_err(),
        "replayed sequence"
    );
    for offset in [0, 4, frame.len() - 1] {
        let mut bad = frame.clone();
        bad[offset] ^= 1;
        assert!(decode(&mut reader(true), &bad, options).is_err());
    }
    send.sequence = u64::MAX;
    assert!(send.encode(b"last", options, false).is_err());
    let mut plain = writer(false);
    plain.encode(&[42], options, false).unwrap();
    assert_eq!(plain.frame[4], 0, "small input stays uncompressed");
}

#[tokio::test]
async fn record_reader_rejects_oversized_frames_bombs_truncation_and_missing_fin() {
    let options = PayloadOptions {
        compress: true,
        encrypt: true,
    };
    let mut sender = writer(true);
    sender
        .encode(b"authenticated bytes", options, false)
        .unwrap();
    let data = sender.frame.clone();
    sender.encode(&[], options, true).unwrap();
    let fin = sender.frame.clone();
    for attack in [
        u32::MAX.to_be_bytes().to_vec(),
        data[..data.len() - 1].to_vec(),
        data.clone(), // valid data followed by transport EOF without FIN
    ] {
        let mut stream = PayloadStream {
            inner: std::io::Cursor::new(attack),
            state: Some(Box::new(State {
                options,
                read: reader(true),
                write: writer(true),
            })),
        };
        assert!(stream.read_to_end(&mut Vec::new()).await.is_err());
        assert!(stream.read_u8().await.is_err(), "errors remain terminal");
    }
    let mut valid = data;
    valid.extend_from_slice(&fin);
    let mut stream = PayloadStream {
        inner: std::io::Cursor::new(valid),
        state: Some(Box::new(State {
            options,
            read: reader(true),
            write: writer(true),
        })),
    };
    let mut result = Vec::new();
    stream.read_to_end(&mut result).await.unwrap();
    assert_eq!(result, b"authenticated bytes");
    // A tiny Snappy preamble claiming >16 KiB must be checked before allocation.
    let bomb = snap::raw::Encoder::new()
        .compress_vec(&vec![0; BLOCK + 1])
        .unwrap();
    let mut frame = (bomb.len() as u32 + 1).to_be_bytes().to_vec();
    frame.push(1);
    frame.extend_from_slice(&bomb);
    assert!(decode(&mut reader(false), &frame, options).is_err());
}

#[tokio::test]
async fn direction_keys_and_fresh_nonces_prevent_reflection_and_cross_stream_replay() {
    let options = PayloadOptions {
        compress: false,
        encrypt: true,
    };
    let (mut first, mut receiver) = pair(options, 1024).await;
    let (mut second, _) = pair(options, 1024).await;
    let a = first.state.as_mut().unwrap();
    a.write.encode(b"secret", options, false).unwrap();
    let record = a.write.frame.clone();
    assert!(
        decode(&mut a.read, &record, options).is_err(),
        "opposite direction key"
    );
    assert!(
        decode(&mut second.state.as_mut().unwrap().read, &record, options).is_err(),
        "different stream key"
    );
    decode(&mut receiver.state.as_mut().unwrap().read, &record, options).unwrap();
}
