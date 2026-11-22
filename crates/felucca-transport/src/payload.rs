//! Per-route Snappy blocks and optional ChaCha20-Poly1305 records.
//! The PSK handshake binds fresh nonces, options, and the exact stream request.
//! HKDF separates traffic directions. Sequence-derived nonces and an
//! authenticated FIN detect modification, replay, reflection, and truncation.
use felucca_core::{Secret, config::payload::PayloadOptions};
use ring::{aead, digest, hkdf, hmac};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

const BLOCK: usize = 16 * 1024;
const TAG: usize = 16;
const MAX_FRAME: usize = BLOCK + 1 + TAG;
const MAGIC: &[u8; 4] = b"FLP5";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Server,
    Agent,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// A raw route has no buffer allocation. Enabled routes retain at most one
/// record per direction; neither framing nor decompression trusts a wire size.
pub struct PayloadStream<T> {
    inner: T,
    state: Option<Box<State>>,
}
impl<T: std::fmt::Debug> std::fmt::Debug for PayloadStream<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayloadStream")
            .field("inner", &self.inner)
            .field("encoded", &self.state.is_some())
            .finish_non_exhaustive()
    }
}
struct State {
    options: PayloadOptions,
    read: Reader,
    write: Writer,
}
#[derive(Default)]
struct Reader {
    cipher: Option<aead::LessSafeKey>,
    sequence: u64,
    header: [u8; 4],
    header_pos: usize,
    frame: Vec<u8>,
    frame_pos: usize,
    plain: bytes::Bytes,
    fin: bool,
    failed: bool,
}
#[derive(Default)]
struct Writer {
    cipher: Option<aead::LessSafeKey>,
    sequence: u64,
    frame: Vec<u8>,
    pos: usize,
    fin: bool,
}

impl<T: AsyncRead + AsyncWrite + Unpin> PayloadStream<T> {
    pub async fn negotiate(
        mut inner: T,
        options: PayloadOptions,
        key: &Secret,
        context: &[u8],
        side: Side,
    ) -> io::Result<Self> {
        if !options.enabled() {
            return Ok(Self { inner, state: None });
        }
        let (send, receive) = tokio::time::timeout(
            Duration::from_secs(10),
            handshake(&mut inner, options, key, context, side),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "payload handshake timed out"))??;
        Ok(Self {
            inner,
            state: Some(Box::new(State {
                options,
                read: Reader {
                    cipher: receive,
                    ..Default::default()
                },
                write: Writer {
                    cipher: send,
                    ..Default::default()
                },
            })),
        })
    }
}

type Keys = (Option<aead::LessSafeKey>, Option<aead::LessSafeKey>);
async fn handshake<T: AsyncRead + AsyncWrite + Unpin>(
    io: &mut T,
    options: PayloadOptions,
    secret: &Secret,
    context: &[u8],
    side: Side,
) -> io::Result<Keys> {
    let flags = u8::from(options.compress) | (u8::from(options.encrypt) << 1);
    let own: [u8; 32] = rand::random();
    let mut leader = [0u8; 37];
    leader[..4].copy_from_slice(MAGIC);
    leader[4] = flags;
    let mut follower = [0u8; 32];
    if side == Side::Server {
        leader[5..].copy_from_slice(&own);
        io.write_all(&leader).await?;
        io.flush().await?;
        io.read_exact(&mut follower).await?;
    } else {
        io.read_exact(&mut leader).await?;
        if &leader[..4] != MAGIC || leader[4] != flags {
            return Err(invalid("payload options mismatch"));
        }
        follower = own;
        io.write_all(&follower).await?;
    }
    let mut hash = digest::Context::new(&digest::SHA256);
    hash.update(b"felucca/payload/v5");
    hash.update(&leader);
    hash.update(&follower);
    hash.update(context);
    let transcript = hash.finish();
    let mac_key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let proof = |label: &[u8]| {
        let mut mac = hmac::Context::with_key(&mac_key);
        mac.update(label);
        mac.update(transcript.as_ref());
        mac.sign()
    };
    // Distinct domains prevent reflecting one side's proof back at it.
    let server_proof = proof(b"server-proof");
    let agent_proof = proof(b"agent-proof");
    let mut received = [0u8; 32];
    if side == Side::Server {
        io.read_exact(&mut received).await?;
        let mut message = b"agent-proof".to_vec();
        message.extend_from_slice(transcript.as_ref());
        hmac::verify(&mac_key, &message, &received)
            .map_err(|_| invalid("payload authentication failed"))?;
        io.write_all(server_proof.as_ref()).await?;
        io.flush().await?;
    } else {
        io.write_all(agent_proof.as_ref()).await?;
        io.flush().await?;
        io.read_exact(&mut received).await?;
        let mut message = b"server-proof".to_vec();
        message.extend_from_slice(transcript.as_ref());
        hmac::verify(&mac_key, &message, &received)
            .map_err(|_| invalid("payload authentication failed"))?;
    }
    if !options.encrypt {
        return Ok((None, None));
    }
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, transcript.as_ref());
    let extracted = salt.extract(secret.as_bytes());
    let key = |label: &[u8]| -> io::Result<aead::LessSafeKey> {
        let mut key = [0u8; 32];
        extracted
            .expand(&[label], hkdf::HKDF_SHA256)
            .map_err(|_| invalid("payload key derivation failed"))?
            .fill(&mut key)
            .map_err(|_| invalid("payload key derivation failed"))?;
        Ok(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::CHACHA20_POLY1305, &key)
                .map_err(|_| invalid("payload key derivation failed"))?,
        ))
    };
    let forward = Some(key(b"server-to-agent")?);
    let reverse = Some(key(b"agent-to-server")?);
    Ok(if side == Side::Server {
        (forward, reverse)
    } else {
        (reverse, forward)
    })
}

fn nonce(sequence: u64) -> io::Result<aead::Nonce> {
    if sequence == u64::MAX {
        return Err(invalid("payload sequence exhausted"));
    }
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&sequence.to_be_bytes());
    Ok(aead::Nonce::assume_unique_for_key(bytes))
}
impl Writer {
    fn encode(&mut self, input: &[u8], options: PayloadOptions, fin: bool) -> io::Result<()> {
        let mut body = vec![if fin { 2 } else { 0 }];
        if !fin && options.compress {
            let compressed = snap::raw::Encoder::new()
                .compress_vec(input)
                .map_err(io::Error::other)?;
            if compressed.len() < input.len() {
                body[0] = 1;
                body.extend_from_slice(&compressed);
            }
        }
        if body[0] == 0 {
            body.extend_from_slice(input);
        }
        let size = body.len() + if self.cipher.is_some() { TAG } else { 0 };
        let header = (size as u32).to_be_bytes();
        if let Some(key) = &self.cipher {
            key.seal_in_place_append_tag(nonce(self.sequence)?, aead::Aad::from(header), &mut body)
                .map_err(|_| invalid("payload encryption failed"))?;
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("payload sequence exhausted"))?;
        self.frame.clear();
        self.frame.extend_from_slice(&header);
        self.frame.extend_from_slice(&body);
        self.pos = 0;
        self.fin = fin;
        Ok(())
    }
    fn drain<T: AsyncWrite + Unpin>(
        &mut self,
        inner: &mut T,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        while self.pos < self.frame.len() {
            let count = ready!(Pin::new(&mut *inner).poll_write(cx, &self.frame[self.pos..]))?;
            if count == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "payload write made no progress",
                )));
            }
            self.pos += count;
        }
        Poll::Ready(Ok(()))
    }
}
impl Reader {
    fn decode(&mut self, options: PayloadOptions) -> io::Result<()> {
        let body = match &self.cipher {
            Some(key) => key
                .open_in_place(
                    nonce(self.sequence)?,
                    aead::Aad::from(self.header),
                    &mut self.frame,
                )
                .map_err(|_| invalid("payload integrity check failed"))?,
            None => &mut self.frame[..],
        };
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("payload sequence exhausted"))?;
        let Some((&kind, data)) = body.split_first() else {
            return Err(invalid("empty payload record"));
        };
        match kind {
            0 if !data.is_empty() && data.len() <= BLOCK => {
                self.plain = bytes::Bytes::copy_from_slice(data)
            }
            1 if options.compress => {
                let size = snap::raw::decompress_len(data).map_err(io::Error::other)?;
                if size == 0 || size > BLOCK {
                    return Err(invalid("Snappy block exceeds payload limit"));
                }
                self.plain = snap::raw::Decoder::new()
                    .decompress_vec(data)
                    .map_err(io::Error::other)?
                    .into();
            }
            2 if data.is_empty() => self.fin = true,
            _ => return Err(invalid("invalid payload record type or length")),
        }
        self.header_pos = 0;
        self.frame_pos = 0;
        self.frame.clear();
        Ok(())
    }
    fn read<T: AsyncRead + Unpin>(
        &mut self,
        inner: &mut T,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
        options: PayloadOptions,
    ) -> Poll<io::Result<()>> {
        loop {
            if self.failed {
                return Poll::Ready(Err(invalid("payload stream failed")));
            }
            if !self.plain.is_empty() {
                let size = buf.remaining().min(self.plain.len());
                buf.put_slice(&self.plain.split_to(size));
                return Poll::Ready(Ok(()));
            }
            if self.fin || buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            while self.header_pos < 4 {
                let mut bytes = ReadBuf::new(&mut self.header[self.header_pos..]);
                ready!(Pin::new(&mut *inner).poll_read(cx, &mut bytes))?;
                let count = bytes.filled().len();
                if count == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "payload ended without FIN",
                    )));
                }
                self.header_pos += count;
            }
            if self.frame.is_empty() {
                let size = u32::from_be_bytes(self.header) as usize;
                let minimum = if self.cipher.is_some() { 1 + TAG } else { 1 };
                if !(minimum..=MAX_FRAME).contains(&size) {
                    return Poll::Ready(Err(invalid("payload frame exceeds limit")));
                }
                self.frame.resize(size, 0);
            }
            while self.frame_pos < self.frame.len() {
                let mut bytes = ReadBuf::new(&mut self.frame[self.frame_pos..]);
                ready!(Pin::new(&mut *inner).poll_read(cx, &mut bytes))?;
                let count = bytes.filled().len();
                if count == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated payload record",
                    )));
                }
                self.frame_pos += count;
            }
            self.decode(options)?;
        }
    }
}
impl<T: AsyncRead + Unpin> AsyncRead for PayloadStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let Some(state) = &mut this.state else {
            return Pin::new(&mut this.inner).poll_read(cx, buf);
        };
        let result = state.read.read(&mut this.inner, cx, buf, state.options);
        if matches!(&result, Poll::Ready(Err(_))) {
            state.read.failed = true;
        }
        result
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for PayloadStream<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let Some(state) = &mut this.state else {
            return Pin::new(&mut this.inner).poll_write(cx, buf);
        };
        if state.write.fin || state.read.failed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "payload stream closed",
            )));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        ready!(state.write.drain(&mut this.inner, cx))?;
        let count = buf.len().min(BLOCK);
        state.write.encode(&buf[..count], state.options, false)?;
        // Consumed bytes may remain queued, like BufWriter. The next write,
        // flush, or shutdown drains exactly this one bounded record.
        if let Poll::Ready(Err(error)) = state.write.drain(&mut this.inner, cx) {
            return Poll::Ready(Err(error));
        }
        Poll::Ready(Ok(count))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(state) = &mut this.state {
            ready!(state.write.drain(&mut this.inner, cx))?;
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(state) = &mut this.state {
            ready!(state.write.drain(&mut this.inner, cx))?;
            if !state.write.fin {
                state.write.encode(&[], state.options, true)?;
                ready!(state.write.drain(&mut this.inner, cx))?;
            }
        }
        ready!(Pin::new(&mut this.inner).poll_flush(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "payload_tests.rs"]
mod tests;
