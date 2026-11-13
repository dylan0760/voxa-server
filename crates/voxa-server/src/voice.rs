//! UDP voice relay. Packets are decrypted with the sender's session key, checked
//! against talk power / mute state, and re-encrypted individually for every listener
//! in the same channel.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::warn;
use voxa_proto::traffic::{Cat, TRAFFIC};
use voxa_proto::voice::{self, Direction, Kind};

use crate::state::Server;

pub async fn run(server: Arc<Server>, sock: UdpSocket) {
    let mut buf = vec![0u8; 2048];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                // Windows reports ICMP port unreachable as a recv error; ignore it.
                if e.kind() != std::io::ErrorKind::ConnectionReset {
                    warn!("udp receive error: {e}");
                }
                continue;
            }
        };
        TRAFFIC.received(Cat::Voice, n);
        let out = route(&server, &buf[..n], src);
        for (addr, pkt) in out {
            if sock.send_to(&pkt, addr).await.is_ok() {
                TRAFFIC.sent(Cat::Voice, pkt.len());
            }
        }
    }
}

fn route(server: &Server, buf: &[u8], src: SocketAddr) -> Vec<(SocketAddr, Vec<u8>)> {
    let Some(id) = voice::peek_client(buf) else { return Vec::new() };
    // Decrypt without holding the lock: junk packets cost the attacker, not everyone.
    let Some(cipher) = server.state.lock().sessions.get(&id).map(|s| s.voice.clone()) else { return Vec::new() };
    let Some(pkt) = cipher.open(Direction::ToServer, buf) else { return Vec::new() };
    let mut st = server.state.lock();
    let Some(s) = st.sessions.get_mut(&id) else { return Vec::new() };
    // The id may have been given to a new session in the meantime.
    if !Arc::ptr_eq(&s.voice, &cipher) || !s.replay.accept(pkt.seq) {
        return Vec::new();
    }
    // Only authenticated packets may (re)bind the client's UDP address (NAT rebinding),
    // and only to the address the client is connected from: otherwise one spoofed
    // packet could send the channel's voice to someone else.
    if s.voice_addr != Some(src) {
        if src.ip().to_canonical() != s.ip.to_canonical() {
            return Vec::new();
        }
        s.voice_addr = Some(src);
    }
    match pkt.kind {
        Kind::Ping => {
            s.s2c_seq += 1;
            let reply = s.voice.seal(Direction::ToClient, Kind::Pong, id, s.s2c_seq, &pkt.payload);
            vec![(src, reply)]
        }
        Kind::Pong => Vec::new(),
        Kind::Voice => {
            let channel = s.info.channel;
            let s = &st.sessions[&id];
            if !st.can_talk(s) {
                return Vec::new();
            }
            // Lossless packets are not Opus: clients before 1.16.0 would play them as noise.
            let lossless = voice::decode_voice(&pkt.payload).map(|(_, flags, _)| flags & voice::FLAG_LOSSLESS != 0).unwrap_or(false);
            let mut out = Vec::new();
            for r in st.sessions.values_mut() {
                if r.info.id == id || r.info.channel != channel || r.info.output_muted {
                    continue;
                }
                if lossless && !r.client_at_least("1.16.0") {
                    continue;
                }
                let Some(addr) = r.voice_addr else { continue };
                r.s2c_seq += 1;
                out.push((addr, r.voice.seal(Direction::ToClient, Kind::Voice, id, r.s2c_seq, &pkt.payload)));
            }
            out
        }
    }
}
