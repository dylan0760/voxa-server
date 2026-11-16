//! The channel calendar (servers 1.21.0+) with the default permissions: the channel's
//! admin arranges events and manages calendars, members see them and answer, people
//! outside the channel see nothing.
//! `cargo run -p voxa-proto --example calendar_test -- 127.0.0.1:9988`
//! Must be built from the client repository (it reports the client version).

use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

struct User {
    r: SecureReader<R>,
    w: SecureWriter<W>,
    id: ClientId,
}

async fn login(addr: &str, nick: &str) -> User {
    let kp = KeyPair::generate();
    let s = TcpStream::connect(addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: APP_VERSION.into(),
        platform: "test".into(),
        nickname: nick.into(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let S2C::Welcome { your_id, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    while let Ok(Ok(_)) = timeout(Duration::from_millis(400), r.recv::<S2C>()).await {}
    User { r, w, id: your_id }
}

async fn wait<T>(u: &mut User, secs: u64, mut f: impl FnMut(S2C) -> Option<T>) -> Option<T> {
    timeout(Duration::from_secs(secs), async {
        loop {
            if let Some(t) = f(u.r.recv::<S2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .ok()
}

async fn error(u: &mut User) -> String {
    wait(u, 5, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await.expect("no error")
}

/// The newest calendar sent (earlier changes may still be waiting to be read).
async fn calendar(u: &mut User, ch: ChannelId) -> (Vec<CalendarInfo>, Vec<CalendarEvent>) {
    let pick = |m: S2C| match m {
        S2C::Calendar { channel, calendars, events } if channel == ch => Some((calendars, events)),
        _ => None,
    };
    let mut last = wait(u, 5, pick).await.expect("no calendar");
    while let Some(newer) = wait(u, 1, pick).await {
        last = newer;
    }
    last
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn event(calendar: CalendarId, title: &str, start: u64, max: Option<u32>) -> CalendarEvent {
    CalendarEvent {
        id: 0,
        calendar,
        title: title.into(),
        description: "Bring snacks".into(),
        start,
        duration_min: 120,
        max_people: max,
        created_by: String::new(),
        created_uid: String::new(),
        rsvps: Vec::new(),
    }
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let mut alice = login(&addr, "Alice").await;
    let mut bob = login(&addr, "Bob").await;
    let mut carol = login(&addr, "Carol").await;

    // Alice makes a channel (she becomes its Channel Admin); Bob joins it.
    alice
        .w
        .send(&C2S::CreateChannel(ChannelEdit {
            parent: None,
            sort: 0,
            name: "Game nights".into(),
            topic: String::new(),
            description: String::new(),
            max_clients: None,
            kind: ChannelKind::Temporary,
            is_default: false,
            codec_quality: 6,
            needed_talk_power: 0,
            needed_join_power: 0,
            voice_encrypted: true,
            password: PasswordChange::Keep,
        }))
        .await
        .unwrap();
    let aid = alice.id;
    let ch = wait(&mut alice, 5, |m| match m {
        S2C::ClientMoved { id, channel, .. } if id == aid => Some(channel),
        _ => None,
    })
    .await
    .expect("Alice was not moved into her channel");
    bob.w.send(&C2S::SwitchChannel { channel: ch, password: None }).await.unwrap();
    let bid = bob.id;
    wait(&mut bob, 5, |m| matches!(m, S2C::ClientMoved { id, .. } if id == bid).then_some(())).await.expect("Bob did not join");

    alice.w.send(&C2S::GetCalendar(ch)).await.unwrap();
    bob.w.send(&C2S::GetCalendar(ch)).await.unwrap();
    let (cals, evs) = calendar(&mut bob, ch).await;
    assert!(cals.is_empty() && evs.is_empty());
    calendar(&mut alice, ch).await;
    println!("1. members see the (empty) calendar");

    // Members may not arrange or manage.
    bob.w.send(&C2S::SaveEvent { channel: ch, event: event(0, "Bob's game", now() + 86_400, None) }).await.unwrap();
    println!("2. a member arranging an event: \"{}\"", error(&mut bob).await);
    bob.w.send(&C2S::CreateCalendar { channel: ch, name: "Mine".into() }).await.unwrap();
    println!("2. a member adding a calendar: \"{}\"", error(&mut bob).await);

    // The channel admin manages and arranges; everyone with it open gets the changes.
    alice.w.send(&C2S::CreateCalendar { channel: ch, name: "Raids".into() }).await.unwrap();
    let (cals, _) = calendar(&mut bob, ch).await;
    assert_eq!(cals.len(), 1);
    let raids = cals[0].id;
    let start = now() + 86_400;
    alice.w.send(&C2S::SaveEvent { channel: ch, event: event(raids, "Raid at 8", start, Some(1)) }).await.unwrap();
    let (_, evs) = calendar(&mut bob, ch).await;
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].created_by, "Alice");
    let eid = evs[0].id;
    println!("3. Alice added a calendar and arranged an event; Bob got both");

    // Answers, with a limit of one person.
    bob.w.send(&C2S::AnswerEvent { channel: ch, id: eid, answer: Some(RsvpAnswer::Going) }).await.unwrap();
    let (_, evs) = calendar(&mut alice, ch).await;
    assert_eq!(evs[0].going(), 1);
    alice.w.send(&C2S::AnswerEvent { channel: ch, id: eid, answer: Some(RsvpAnswer::Going) }).await.unwrap();
    println!("4. Bob is going; Alice going too: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::AnswerEvent { channel: ch, id: eid, answer: Some(RsvpAnswer::Maybe) }).await.unwrap();
    let (_, evs) = calendar(&mut bob, ch).await;
    assert_eq!(evs[0].rsvps.len(), 2);
    bob.w.send(&C2S::AnswerEvent { channel: ch, id: eid, answer: None }).await.unwrap();
    let (_, evs) = calendar(&mut alice, ch).await;
    assert_eq!(evs[0].going(), 0);
    println!("4. Alice may come, Bob took his answer back");

    // Bad events.
    alice.w.send(&C2S::SaveEvent { channel: ch, event: event(raids, "  ", start, None) }).await.unwrap();
    println!("5. an event without a title: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::SaveEvent { channel: ch, event: event(raids, "Yesterday", now() - 3 * 86_400, None) }).await.unwrap();
    println!("5. an event in the past: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::SaveEvent { channel: ch, event: event(raids, "Huge", start, Some(5000)) }).await.unwrap();
    println!("5. 5000 people: \"{}\"", error(&mut alice).await);

    // Outside the channel: nothing.
    carol.w.send(&C2S::GetCalendar(ch)).await.unwrap();
    let leaked = wait(&mut carol, 2, |m| matches!(m, S2C::Calendar { .. }).then_some(())).await;
    assert!(leaked.is_none(), "someone outside the channel got its calendar");
    println!("6. someone outside the channel gets nothing");
    carol.w.send(&C2S::AnswerEvent { channel: ch, id: eid, answer: Some(RsvpAnswer::Going) }).await.unwrap();
    println!("6. ...answering anyway: \"{}\"", error(&mut carol).await);

    // Changing and deleting.
    let mut changed = event(raids, "Raid at 9", start + 3600, None);
    changed.id = eid;
    alice.w.send(&C2S::SaveEvent { channel: ch, event: changed }).await.unwrap();
    let (_, evs) = calendar(&mut bob, ch).await;
    assert_eq!(evs[0].title, "Raid at 9");
    assert_eq!(evs[0].rsvps.len(), 1, "answers were lost when the event changed");
    alice.w.send(&C2S::DeleteCalendar { channel: ch, id: raids }).await.unwrap();
    let (cals, evs) = calendar(&mut bob, ch).await;
    assert!(cals.is_empty() && evs.is_empty());
    println!("7. the event was changed (answers kept), then the calendar deleted with it");

    // Bob leaves: no more updates for him.
    bob.w.send(&C2S::SwitchChannel { channel: NO_CHANNEL, password: None }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    alice.w.send(&C2S::SaveEvent { channel: ch, event: event(0, "After Bob left", start, None) }).await.unwrap();
    let (cals, _) = calendar(&mut alice, ch).await;
    assert_eq!(cals.len(), 1, "a General calendar was not made for the first event");
    let leaked = wait(&mut bob, 2, |m| matches!(m, S2C::Calendar { .. }).then_some(())).await;
    assert!(leaked.is_none(), "someone who left the channel still got its calendar");
    println!("8. a General calendar came with the next event; Bob (gone) got nothing");
    println!("CALENDAR TEST PASSED");
}
