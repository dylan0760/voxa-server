//! UDP voice relay. Packets are decrypted with the sender's session key, checked
//! against talk power / mute state, and re-encrypted individually for every listener
//! in the same channel.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::warn;
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
        let out = route(&server, &buf[..n], src);
        for (addr, pkt) in out {
            let _ = sock.send_to(&pkt, addr).await;
        }
    }
}

fn route(server: &Server, buf: &[u8], src: SocketAddr) -> Vec<(SocketAddr, Vec<u8>)> {
    let Some(id) = voice::peek_client(buf) else { return Vec::new() };
    let mut st = server.state.lock();
    let Some(s) = st.sessions.get_mut(&id) else { return Vec::new() };
    let Some(pkt) = s.voice.open(Direction::ToServer, buf) else { return Vec::new() };
    if !s.replay.accept(pkt.seq) {
        return Vec::new();
    }
    // Only authenticated packets may (re)bind the client's UDP address (NAT rebinding).
    if s.voice_addr != Some(src) {
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
            let mut out = Vec::new();
            for r in st.sessions.values_mut() {
                if r.info.id == id || r.info.channel != channel || r.info.output_muted {
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
