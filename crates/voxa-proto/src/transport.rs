//! Noise (XX) secured, length framed message stream over any async byte stream.
//!
//! Wire format after the handshake: a stream of Noise transport messages, each
//! `[u16 BE length][ciphertext]`. The decrypted plaintext forms a continuous byte stream
//! of application messages framed as `[u32 BE length][bincode payload]`, which lets a
//! single application message span many Noise messages (screen share frames, files).

use std::sync::Arc;

use serde::{de::DeserializeOwned, Serialize};
use snow::StatelessTransportState;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::identity::{NOISE_PARAMS, NOISE_PROLOGUE};

const NOISE_MAX: usize = 65535;
const TAG: usize = 16;
const CHUNK_PLAIN: usize = NOISE_MAX - TAG;
pub const MAX_MESSAGE: usize = 32 * 1024 * 1024;

#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    #[error("connection error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encryption error: {0}")]
    Noise(#[from] snow::Error),
    #[error("protocol error: {0}")]
    Codec(String),
    #[error("message too large")]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, TransportError>;

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> Result<()> {
    let len = r.read_u16().await? as usize;
    buf.resize(len, 0);
    r.read_exact(buf).await?;
    Ok(())
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> Result<()> {
    let mut out = Vec::with_capacity(data.len() + 2);
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    w.write_all(&out).await?;
    Ok(())
}

pub struct SecureReader<R> {
    inner: R,
    state: Arc<StatelessTransportState>,
    nonce: u64,
    plain: Vec<u8>,
    frame: Vec<u8>,
    scratch: Vec<u8>,
}

pub struct SecureWriter<W> {
    inner: W,
    state: Arc<StatelessTransportState>,
    nonce: u64,
    scratch: Vec<u8>,
}

pub struct Secured<R, W> {
    pub reader: SecureReader<R>,
    pub writer: SecureWriter<W>,
    /// The peer's long term static public key.
    pub remote_static: Vec<u8>,
}

fn split<R, W>(r: R, w: W, state: StatelessTransportState) -> Result<Secured<R, W>> {
    let remote_static = state
        .get_remote_static()
        .map(|k| k.to_vec())
        .ok_or_else(|| TransportError::Codec("peer sent no static key".into()))?;
    let state = Arc::new(state);
    Ok(Secured {
        reader: SecureReader {
            inner: r,
            state: state.clone(),
            nonce: 0,
            plain: Vec::new(),
            frame: Vec::new(),
            scratch: vec![0u8; NOISE_MAX],
        },
        writer: SecureWriter { inner: w, state, nonce: 0, scratch: vec![0u8; NOISE_MAX] },
        remote_static,
    })
}

/// Client side of the XX handshake.
pub async fn connect<R, W>(mut r: R, mut w: W, local_private: &[u8]) -> Result<Secured<R, W>>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut hs = snow::Builder::new(NOISE_PARAMS.parse().expect("params"))
        .local_private_key(local_private)?
        .prologue(NOISE_PROLOGUE)?
        .build_initiator()?;
    let mut buf = vec![0u8; NOISE_MAX];
    let mut frame = Vec::new();
    let n = hs.write_message(&[], &mut buf)?;
    write_frame(&mut w, &buf[..n]).await?;
    read_frame(&mut r, &mut frame).await?;
    hs.read_message(&frame, &mut buf)?;
    let n = hs.write_message(&[], &mut buf)?;
    write_frame(&mut w, &buf[..n]).await?;
    w.flush().await?;
    split(r, w, hs.into_stateless_transport_mode()?)
}

/// Server side of the XX handshake.
pub async fn accept<R, W>(mut r: R, mut w: W, local_private: &[u8]) -> Result<Secured<R, W>>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut hs = snow::Builder::new(NOISE_PARAMS.parse().expect("params"))
        .local_private_key(local_private)?
        .prologue(NOISE_PROLOGUE)?
        .build_responder()?;
    let mut buf = vec![0u8; NOISE_MAX];
    let mut frame = Vec::new();
    read_frame(&mut r, &mut frame).await?;
    hs.read_message(&frame, &mut buf)?;
    let n = hs.write_message(&[], &mut buf)?;
    write_frame(&mut w, &buf[..n]).await?;
    w.flush().await?;
    read_frame(&mut r, &mut frame).await?;
    hs.read_message(&frame, &mut buf)?;
    split(r, w, hs.into_stateless_transport_mode()?)
}

impl<R: AsyncRead + Unpin> SecureReader<R> {
    /// Reads the next application message.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T> {
        loop {
            if self.plain.len() >= 4 {
                let len = u32::from_be_bytes(self.plain[..4].try_into().unwrap()) as usize;
                if len > MAX_MESSAGE {
                    return Err(TransportError::TooLarge);
                }
                if self.plain.len() >= 4 + len {
                    let msg = bincode::deserialize(&self.plain[4..4 + len])
                        .map_err(|e| TransportError::Codec(e.to_string()));
                    self.plain.drain(..4 + len);
                    return msg;
                }
            }
            read_frame(&mut self.inner, &mut self.frame).await?;
            let n = self.state.read_message(self.nonce, &self.frame, &mut self.scratch)?;
            self.nonce += 1;
            self.plain.extend_from_slice(&self.scratch[..n]);
        }
    }
}

impl<W: AsyncWrite + Unpin> SecureWriter<W> {
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        let body = bincode::serialize(msg).map_err(|e| TransportError::Codec(e.to_string()))?;
        if body.len() > MAX_MESSAGE {
            return Err(TransportError::TooLarge);
        }
        let mut plain = Vec::with_capacity(body.len() + 4);
        plain.extend_from_slice(&(body.len() as u32).to_be_bytes());
        plain.extend_from_slice(&body);
        let mut out = Vec::with_capacity(plain.len() + plain.len() / CHUNK_PLAIN * 18 + 18);
        for chunk in plain.chunks(CHUNK_PLAIN) {
            let n = self.state.write_message(self.nonce, chunk, &mut self.scratch)?;
            self.nonce += 1;
            out.extend_from_slice(&(n as u16).to_be_bytes());
            out.extend_from_slice(&self.scratch[..n]);
        }
        self.inner.write_all(&out).await?;
        self.inner.flush().await?;
        Ok(())
    }
}
