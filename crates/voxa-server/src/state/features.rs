use super::*;

pub struct TransferSlot(Arc<AtomicUsize>, Arc<AtomicUsize>);
impl Drop for TransferSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
        self.1.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(super) struct PrivateShare {
    peer: ClientId,
    stream: u32,
    accepted: bool,
    started: Instant,
    last: Option<Instant>,
    sequence: u32,
    // Only the latest picture for a web viewer. Native viewers use bounded bulk queues.
    jpeg: Option<Arc<Vec<u8>>>,
}

impl State {
    pub(super) fn end_media(&mut self, from: ClientId, to: ClientId, stream: u32) {
        let Some(s) = self.sessions.get_mut(&from) else { return };
        if s.private_share.as_ref().is_some_and(|m| (m.peer, m.stream) == (to, stream)) {
            s.private_share = None;
            self.send(to, S2C::MediaEnded { peer: from, stream });
            self.send(from, S2C::MediaEnded { peer: to, stream });
        }
    }

    pub(super) fn end_peer_media(&mut self, id: ClientId) {
        let links: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(&from, s)| {
                let m = s.private_share.as_ref()?;
                (from == id || m.peer == id).then_some((from, m.peer, m.stream))
            })
            .collect();
        for (from, to, stream) in links {
            self.end_media(from, to, stream);
        }
    }
}

impl Server {
    pub(super) fn prune_media(&self, st: &mut State) {
        let ended: Vec<_> = st
            .sessions
            .iter()
            .filter_map(|(&from, s)| {
                let m = s.private_share.as_ref()?;
                (self.media_allowed(st, from, m.peer).is_err() || (!m.accepted && m.started.elapsed().as_secs() >= 60))
                    .then_some((from, m.peer, m.stream))
            })
            .collect();
        for (from, to, id) in ended {
            st.end_media(from, to, id);
        }
    }
    pub fn transfer_slot(&self, me: ClientId, upload: bool) -> Res<TransferSlot> {
        let st = self.state.lock();
        let s = st.session(me)?;
        let limits = st.transfer_limits;
        let (slots, cap) = if upload { (&self.upload_slots, limits.uploads) } else { (&self.download_slots, limits.downloads) };
        let local = &s.transfers_running[upload as usize];
        let personal = if upload { s.transfer_prefs.uploads } else { s.transfer_prefs.downloads };
        if (cap > 0 && slots.load(Ordering::Relaxed) >= cap as usize) || (personal > 0 && local.load(Ordering::Relaxed) >= personal as usize) {
            return Err("All transfer slots in this direction are busy. Retry shortly.".into());
        }
        slots.fetch_add(1, Ordering::Relaxed);
        local.fetch_add(1, Ordering::Relaxed);
        Ok(TransferSlot(slots.clone(), local.clone()))
    }

    pub(super) fn set_transfer_limits(&self, st: &mut State, me: ClientId, limits: TransferLimits) -> Res {
        self.need(st.server_perms(st.session(me)?), perm::SERVER_EDIT)?;
        if !limits.valid() {
            return Err("Invalid transfer limits".into());
        }
        db(self.store.save_preference("transfer_limits", &Some(limits)))?;
        st.transfer_limits = limits;
        self.upload_rate.set(limits.upload_kib);
        self.download_rate.set(limits.download_kib);
        for s in st.sessions.values().filter(|s| s.client_at_least("1.40.0")) {
            s.tx.send(Out::Msg(S2C::TransferLimits(limits)));
        }
        Ok(())
    }

    pub(super) fn set_icon(&self, st: &mut State, me: ClientId, image: Option<Vec<u8>>) -> Res {
        self.need(st.server_perms(st.session(me)?), perm::SERVER_EDIT)?;
        st.spend_chat(me)?;
        if let Some(b) = &image {
            if b.len() > MAX_AVATAR_BYTES || !image_dims(b).is_some_and(|(w, h)| w == h && (1..=MAX_AVATAR_DIM).contains(&w)) {
                return Err("Choose a square PNG or JPEG up to 256 pixels and 96 KiB".into());
            }
            let mut reader = image::ImageReader::new(std::io::Cursor::new(b)).with_guessed_format().map_err(|_| "Invalid icon")?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_AVATAR_DIM);
            limits.max_image_height = Some(MAX_AVATAR_DIM);
            limits.max_alloc = Some(4 * 1024 * 1024);
            reader.limits(limits);
            reader.decode().map_err(|_| "Invalid icon image")?;
        }
        db(self.store.save_preference("server_icon", &image))?;
        st.server_icon = image.clone();
        for s in st.sessions.values().filter(|s| s.client_at_least("1.40.0")) {
            s.tx.send(Out::Msg(S2C::ServerIcon(image.clone())));
        }
        Ok(())
    }

    pub(super) fn set_game(&self, st: &mut State, me: ClientId, game: Option<String>) -> Res {
        let game = game.map(|s| clean_line(&s, 60)).filter(|s| !s.is_empty());
        if st.session(me)?.game == game {
            return Ok(());
        }
        if game.is_some() {
            st.spend_chat(me)?;
        }
        st.sessions.get_mut(&me).unwrap().game = game.clone();
        let subject = st.session(me)?;
        for s in st.sessions.values().filter(|s| s.client_at_least("1.40.0") && st.sees(s, subject)) {
            s.tx.send(Out::Msg(S2C::Playing { client: me, game: game.clone() }));
        }
        Ok(())
    }

    pub(super) fn check_dm_uid(&self, s: &Session, uid: &str) -> Res {
        if uid.len() > 64
            || uid.is_empty()
            || s.blocks.iter().any(|b| b == uid)
            || db(self.store.preference::<Vec<String>>(&format!("dm-blocks:{uid}")))?.contains(&s.info.uid)
        {
            return Err("Direct messages are unavailable for this person".into());
        }
        Ok(())
    }

    pub(super) fn dm_pair(&self, st: &State, from: ClientId, to: ClientId) -> Res {
        let (s, peer) = (st.session(from)?, st.session(to)?);
        self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
        if !st.sees(s, peer) || s.blocks.contains(&peer.info.uid) || peer.blocks.contains(&s.info.uid) {
            return Err("Direct messages are unavailable for this person".into());
        }
        Ok(())
    }

    pub(super) fn block_dm(&self, st: &mut State, me: ClientId, uid: String, block: bool) -> Res {
        st.spend_chat(me)?;
        let s = st.session(me)?;
        if st.limited(s) || uid == s.info.uid || uid.len() > 64 || uid.is_empty() {
            return Err("Cannot change that block".into());
        }
        let owner = s.info.uid.clone();
        let mut list = s.blocks.clone();
        list.retain(|x| *x != uid);
        if block {
            if list.len() >= 512 {
                return Err("The block list is full".into());
            }
            list.push(uid.clone());
        }
        db(self.store.save_preference(&format!("dm-blocks:{owner}"), &list))?;
        for s in st.sessions.values_mut().filter(|s| s.info.uid == owner) {
            s.blocks = list.clone();
            if s.client_at_least("1.40.0") {
                s.tx.send(Out::Msg(S2C::DmBlocks(list.clone())));
            }
        }
        let ended: Vec<_> = st
            .sessions
            .values()
            .filter_map(|s| {
                let m = s.private_share.as_ref()?;
                let peer = st.sessions.get(&m.peer)?;
                (block && ((s.info.uid == owner && peer.info.uid == uid) || (s.info.uid == uid && peer.info.uid == owner)))
                    .then_some((s.info.id, m.peer, m.stream))
            })
            .collect();
        for (a, b, stream) in ended {
            st.end_media(a, b, stream);
        }
        Ok(())
    }

    fn media_allowed(&self, st: &State, from: ClientId, to: ClientId) -> Res {
        self.dm_pair(st, from, to)?;
        let s = st.session(from)?;
        if from == to || !st.sees(st.session(to)?, s) {
            return Err("Private media unavailable".into());
        }
        self.need(st.server_perms(s), perm::SCREEN_SHARE)?;
        if !st.session(to)?.client_at_least("1.40.0") {
            return Err("The recipient needs Voxa 1.40 or newer".into());
        }
        Ok(())
    }

    pub(super) fn offer_media(&self, st: &mut State, from: ClientId, to: ClientId, stream: u32, camera: bool) -> Res {
        st.spend_chat(from)?;
        self.media_allowed(st, from, to)?;
        if let Some(old) = &st.session(from)?.private_share {
            st.end_media(from, old.peer, old.stream);
        }
        st.sessions.get_mut(&from).unwrap().private_share =
            Some(PrivateShare { peer: to, stream, accepted: false, started: Instant::now(), last: None, sequence: 0, jpeg: None });
        st.send(to, S2C::MediaOffer { from, stream, camera });
        Ok(())
    }

    pub(super) fn answer_media(&self, st: &mut State, me: ClientId, from: ClientId, stream: u32, accept: bool) -> Res {
        self.media_allowed(st, from, me)?;
        let share = st
            .sessions
            .get_mut(&from)
            .and_then(|s| s.private_share.as_mut())
            .filter(|m| m.peer == me && m.stream == stream && !m.accepted)
            .ok_or("Invitation expired")?;
        if !accept || share.started.elapsed().as_secs() >= 60 {
            st.end_media(from, me, stream);
        } else {
            share.accepted = true;
            st.send(from, S2C::MediaAnswer { peer: me, stream, accept: true });
            st.send(me, S2C::MediaAnswer { peer: from, stream, accept: true });
        }
        Ok(())
    }

    pub fn direct_frame(&self, from: ClientId, to: ClientId, stream: u32, jpeg: Vec<u8>) -> Res {
        let mut st = self.state.lock();
        self.media_allowed(&st, from, to)?;
        let web = st.session(to)?.web;
        let share = st
            .sessions
            .get_mut(&from)
            .and_then(|s| s.private_share.as_mut())
            .filter(|m| m.accepted && m.peer == to && m.stream == stream)
            .ok_or("Private stream ended")?;
        if share.last.is_some_and(|t| t.elapsed().as_millis() < 70) {
            return Ok(());
        }
        if !voxa_proto::direct::frame_ok(&jpeg) {
            return Err("Invalid private video frame".into());
        }
        share.last = Some(Instant::now());
        share.sequence = share.sequence.wrapping_add(1);
        if web {
            share.jpeg = Some(Arc::new(jpeg));
        } else if let Some(tx) = &st.session(to)?.bulk_tx {
            if tx.capacity() >= 44 {
                let _ = tx.try_send(BulkS2C::DirectFrame { from, stream, jpeg });
            }
        }
        Ok(())
    }

    pub fn web_frame(&self, me: ClientId, from: ClientId, stream: u32, after: u32) -> Option<(u32, Arc<Vec<u8>>)> {
        let st = self.state.lock();
        self.media_allowed(&st, from, me).ok()?;
        let m = st.sessions.get(&from)?.private_share.as_ref()?;
        if !m.accepted || m.peer != me || m.stream != stream || m.sequence == after {
            return None;
        }
        Some((m.sequence, m.jpeg.clone()?))
    }

    pub fn video_message(&self, me: ClientId, uid: &str, data: &[u8]) -> Res {
        if voxa_proto::direct::clip(data).is_none() {
            return Err("Invalid or oversized video message".into());
        }
        let mut st = self.state.lock();
        st.spend_chat(me)?;
        let s = st.session(me)?;
        self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
        self.check_dm_uid(s, uid)?;
        if uid == s.info.uid || db(self.store.get_client(uid))?.is_none() {
            return Err("Unknown recipient".into());
        }
        let recipients: Vec<_> = st.sessions.values().filter(|p| p.info.uid == uid).collect();
        let visible = recipients.iter().filter(|p| st.sees(s, p)).min_by_key(|p| p.web).map(|p| p.info.id);
        if recipients.is_empty() && db(self.store.offline_count(uid))? >= MAX_OFFLINE_WAITING {
            return Err("Recipient inbox full".into());
        }
        let image = db(self.store.new_image_id())?;
        let ts = now_unix();
        let meta = ImageMeta { ch: None, msg: 0, uids: vec![s.info.uid.clone(), uid.to_owned()], ts, name: "video.vxc".into() };
        let text = format!("🎥 Video message\n[voxa-clip:{image}]");
        let m = ChatMessage {
            id: 0,
            target: TextTarget::Private(me),
            from: Some(me),
            from_uid: s.info.uid.clone(),
            from_name: s.info.nickname.clone(),
            ts,
            text: text.clone(),
        };
        if recipients.is_empty() {
            let note = OfflineMessage { from_uid: m.from_uid.clone(), from_name: m.from_name.clone(), ts, text: text.clone() };
            db(self.store.put_clip(image, &meta, data, Some((uid, &note))))?;
        } else {
            db(self.store.put_clip(image, &meta, data, None))?;
            for recipient in recipients {
                st.send(recipient.info.id, S2C::Text(m.clone()));
            }
        }
        if let Some(peer) = visible {
            st.send(me, S2C::Text(ChatMessage { target: TextTarget::Private(peer), ..m }));
        } else {
            // Mirror a queued text DM receipt even if an invisible user received it immediately.
            st.send(me, S2C::OfflineVideoSent { uid: uid.into(), text, ts });
            st.send(me, S2C::OfflineQueued { to_uid: uid.into(), ts });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Exercise the normal server handlers without a camera, microphone, renderer or browser.
    #[test]
    fn personal_limits_consent_and_dm_blocks_follow_the_authenticated_identity() {
        let dir = std::env::temp_dir().join(format!("voxa-controls-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        {
            let db = Store::open(&dir.join("data.redb"), &[3u8; 32]).unwrap();
            let mut cfg = Config::default();
            cfg.transfers.uploads = 2;
            let srv = Server::new(cfg, dir.clone(), db, KeyPair::generate()).unwrap();
            let login = |uid: &str| {
                let (tx, rx) = mpsc::channel(128);
                let close = Arc::new(Notify::new());
                let hello = C2S::Hello {
                    protocol: PROTOCOL_VERSION,
                    version: MIN_CLIENT_VERSION.into(),
                    platform: "Web".into(),
                    nickname: uid.into(),
                    server_password: None,
                    default_channel: None,
                    default_channel_password: None,
                };
                let id = srv.login(uid.into(), "127.0.0.1".parse().unwrap(), hello, Outbox { tx, close: close.clone() }, close, true).unwrap();
                (id, rx)
            };
            let (alice, _a) = login("Alice");
            let (bob, mut b) = login("Bob");
            let (chris, mut c) = login("Chris");
            // An expired invitation cannot be revived by a late answer.
            srv.handle(chris, C2S::OfferMedia { to: bob, stream: 99, camera: true });
            {
                let mut st = srv.state.lock();
                let offer = st.sessions.get_mut(&chris).unwrap().private_share.as_mut().unwrap();
                offer.started -= std::time::Duration::from_secs(61);
            }
            srv.handle(bob, C2S::AnswerMedia { from: chris, stream: 99, accept: true });
            assert!(srv.state.lock().session(chris).unwrap().private_share.is_none());
            let initial_channel = srv.state.lock().session(alice).unwrap().info.channel;
            srv.handle(alice, C2S::ClientTransferLimits(TransferLimits { uploads: 1, downloads: 0, ..Default::default() }));
            let first = srv.transfer_slot(alice, true).unwrap();
            assert!(srv.transfer_slot(alice, true).is_err());
            let second = srv.transfer_slot(bob, true).unwrap();
            assert!(srv.transfer_slot(chris, true).is_err());
            assert!(srv.transfer_slot(alice, false).is_ok());
            drop(second);
            drop(first);
            assert!(srv.transfer_slot(alice, true).is_ok());
            srv.handle(alice, C2S::SetGame(Some("Example game".into())));
            srv.handle(alice, C2S::OfferMedia { to: bob, stream: 41, camera: false });
            let mut offer = false;
            while let Ok(Out::Msg(m)) = b.try_recv() {
                if matches!(m,S2C::MediaOffer{from,stream:41,..} if from==alice) {
                    offer = true;
                }
            }
            assert!(offer);
            while let Ok(Out::Msg(m)) = c.try_recv() {
                assert!(!matches!(m, S2C::MediaOffer { .. }));
            }
            let mut jpeg = Vec::new();
            let img = image::RgbImage::from_pixel(2, 2, image::Rgb([128, 10, 20]));
            image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&img).unwrap();
            assert!(srv.direct_frame(alice, bob, 41, jpeg.clone()).is_err());
            srv.handle(bob, C2S::AnswerMedia { from: alice, stream: 41, accept: true });
            srv.direct_frame(alice, bob, 41, jpeg.clone()).unwrap();
            assert_eq!(&*srv.web_frame(bob, alice, 41, 0).unwrap().1, &jpeg);
            assert!(srv.web_frame(chris, alice, 41, 0).is_none());
            assert_eq!(srv.state.lock().session(alice).unwrap().info.channel, initial_channel);
            let mut clip = b"VXC1".to_vec();
            for n in [1u32, 0, jpeg.len() as u32] {
                clip.extend_from_slice(&n.to_le_bytes());
            }
            clip.extend_from_slice(&jpeg);
            assert!(voxa_proto::direct::clip(&clip[..clip.len()-1]).is_none());
            for (field, value) in [(4, 0u32), (4, 121), (8, 1), (8, 12002), (12, u32::MAX)] {
                let mut invalid = clip.clone();
                invalid[field..field+4].copy_from_slice(&value.to_le_bytes());
                assert!(voxa_proto::direct::clip(&invalid).is_none());
            }
            srv.video_message(alice, "Bob", &clip).unwrap();
            let mut image = None;
            while let Ok(Out::Msg(m)) = b.try_recv() {
                if let S2C::Text(m) = m {
                    image = voxa_proto::direct::clip_id(&m.text);
                }
            }
            let image = image.unwrap();
            assert_eq!(srv.chat_image(bob, image).unwrap(), clip);
            assert_eq!(srv.chat_image(alice, image).unwrap(), clip);
            assert!(srv.chat_image(chris, image).is_err());
            srv.handle(bob, C2S::BlockDm { uid: "Alice".into(), block: true });
            assert!(srv.direct_frame(alice, bob, 41, jpeg).is_err());
            assert!(srv.video_message(alice, "Bob", &clip).is_err());
            assert!(srv.send_text(&mut srv.state.lock(), alice, TextTarget::Private(bob), "hello".into()).is_err());
            srv.remove_session(bob, LeaveReason::Disconnected(String::new()));
            assert!(srv.send_offline(&mut srv.state.lock(), alice, "Bob".into(), "hello".into()).is_err());
            let (bob, _b) = login("Bob");
            srv.handle(bob, C2S::BlockDm { uid: "Alice".into(), block: false });
            assert!(srv.send_text(&mut srv.state.lock(), alice, TextTarget::Private(bob), "hello".into()).is_ok());
            srv.remove_session(bob, LeaveReason::Disconnected(String::new()));
            srv.video_message(alice, "Bob", &clip).unwrap();
            let (bob, mut inbox) = login("Bob");
            let mut delivered = None;
            while let Ok(Out::Msg(msg)) = inbox.try_recv() {
                if let S2C::OfflineMessages(notes) = msg {
                    delivered = notes.iter().find_map(|note| voxa_proto::direct::clip_id(&note.text));
                }
            }
            assert_eq!(srv.chat_image(bob, delivered.unwrap()).unwrap(), clip);
            let snapshot = dir.join("backup.redb");
            srv.store.snapshot_to(&snapshot).unwrap();
            let restored = Store::open(&snapshot, &[3u8; 32]).unwrap();
            assert_eq!(restored.image_data(image).unwrap().unwrap(), clip);
            restored.expire_clips(now_unix()+1).unwrap();
            assert!(restored.image_data(image).unwrap().is_none());
            assert_eq!(restored.preference::<u64>("video_bytes").unwrap(), 0);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
