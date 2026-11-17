//! The server calendar (servers 1.23.0+): seen from anywhere with the server groups'
//! permissions, events with a channel to meet in; and removing people from the server.
//! `cargo run -p voxa-proto --example server_calendar_test -- 127.0.0.1:9988 <server data dir>`
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
    uid: String,
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
    let S2C::Welcome { your_id, your_uid, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    while let Ok(Ok(_)) = timeout(Duration::from_millis(400), r.recv::<S2C>()).await {}
    User { r, w, id: your_id, uid: your_uid }
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

type Cal = (Vec<CalendarInfo>, Vec<CalendarEvent>, Vec<(EventId, ChannelId)>);

/// The newest server calendar sent.
async fn calendar(u: &mut User) -> Cal {
    let pick = |m: S2C| match m {
        S2C::ServerCalendar { calendars, events, meet } => Some((calendars, events, meet)),
        _ => None,
    };
    let mut last = wait(u, 5, pick).await.expect("no server calendar");
    while let Some(newer) = wait(u, 1, pick).await {
        last = newer;
    }
    last
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn event(title: &str, start: u64) -> CalendarEvent {
    CalendarEvent {
        id: 0,
        calendar: 0,
        title: title.into(),
        description: "Racing, then whatever".into(),
        start,
        duration_min: 180,
        max_people: None,
        created_by: String::new(),
        created_uid: String::new(),
        rsvps: Vec::new(),
    }
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let mut alice = login(&addr, "Alice").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    alice.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    wait(&mut alice, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    let mut bob = login(&addr, "Bob").await;

    // A channel to meet in.
    let mut e = ChannelEdit::new_default(None);
    e.name = "Game nights".into();
    alice.w.send(&C2S::CreateChannel(e)).await.unwrap();
    let aid = alice.id;
    let ch = wait(&mut alice, 5, |m| match m {
        S2C::ClientMoved { id, channel, .. } if id == aid => Some(channel),
        _ => None,
    })
    .await
    .expect("no channel");

    // Bob, in no particular channel, sees the (empty) server calendar.
    bob.w.send(&C2S::GetCalendar(SERVER_CALENDAR)).await.unwrap();
    alice.w.send(&C2S::GetCalendar(SERVER_CALENDAR)).await.unwrap();
    let (cals, evs, meet) = calendar(&mut bob).await;
    assert!(cals.is_empty() && evs.is_empty() && meet.is_empty());
    calendar(&mut alice).await;
    println!("1. members see the server calendar from anywhere");

    // Members may not arrange (Normal has no "Create and edit calendar events").
    bob.w.send(&C2S::SaveServerEvent { event: event("Bob's night", now() + 86_400), meet: None }).await.unwrap();
    println!("2. a member arranging: \"{}\"", error(&mut bob).await);
    alice.w.send(&C2S::SaveServerEvent { event: event("Bad place", now() + 86_400), meet: Some(987_654) }).await.unwrap();
    println!("2. meeting in a channel that does not exist: \"{}\"", error(&mut alice).await);

    // An admin arranges a game night in the channel; Bob gets it with the place.
    alice.w.send(&C2S::SaveServerEvent { event: event("Game night", now() + 86_400), meet: Some(ch) }).await.unwrap();
    let (_, evs, meet) = calendar(&mut bob).await;
    assert_eq!(evs.len(), 1);
    let eid = evs[0].id;
    assert_eq!(meet, vec![(eid, ch)]);
    println!("3. Alice arranged a game night in Game nights; Bob got it with the place");

    // Bob answers (server groups count, not his channel).
    bob.w.send(&C2S::AnswerEvent { channel: SERVER_CALENDAR, id: eid, answer: Some(RsvpAnswer::Going) }).await.unwrap();
    let (_, evs, _) = calendar(&mut alice).await;
    assert_eq!(evs[0].going(), 1);
    println!("4. Bob is going");

    // Changing the event: no place any more (answers kept).
    let mut changed = event("Game night (moved)", now() + 2 * 86_400);
    changed.id = eid;
    alice.w.send(&C2S::SaveServerEvent { event: changed, meet: None }).await.unwrap();
    let (_, evs, meet) = calendar(&mut bob).await;
    assert_eq!(evs[0].title, "Game night (moved)");
    assert_eq!(evs[0].going(), 1, "answers were lost");
    assert!(meet.is_empty(), "the place was not cleared");
    // The place comes back, and goes when the channel is deleted.
    let mut again = evs[0].clone();
    again.rsvps.clear();
    alice.w.send(&C2S::SaveServerEvent { event: again, meet: Some(ch) }).await.unwrap();
    let (_, _, meet) = calendar(&mut bob).await;
    assert_eq!(meet.len(), 1);
    alice.w.send(&C2S::SwitchChannel { channel: NO_CHANNEL, password: None }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    alice.w.send(&C2S::DeleteChannel { id: ch, force: true }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    bob.w.send(&C2S::GetCalendar(SERVER_CALENDAR)).await.unwrap();
    let (_, evs, meet) = calendar(&mut bob).await;
    assert_eq!(evs.len(), 1);
    assert!(meet.is_empty(), "a deleted channel is still a meeting place");
    println!("5. the event changed (answers kept); its place went with the channel");

    // Removing people: only offline ones, only by admins, never yourself.
    let carol = login(&addr, "Carol").await;
    let carol_uid = carol.uid.clone();
    drop(carol);
    tokio::time::sleep(Duration::from_millis(800)).await;
    bob.w.send(&C2S::ForgetClient(carol_uid.clone())).await.unwrap();
    println!("6. a member removing someone: \"{}\"", error(&mut bob).await);
    alice.w.send(&C2S::ForgetClient(bob.uid.clone())).await.unwrap();
    println!("6. removing someone online: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::ForgetClient(alice.uid.clone())).await.unwrap();
    println!("6. removing yourself: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::ForgetClient(carol_uid.clone())).await.unwrap();
    let people = wait(&mut alice, 5, |m| if let S2C::People(p) = m { Some(p) } else { None }).await.expect("no people list");
    assert!(!people.iter().any(|p| p.uid == carol_uid), "Carol is still listed");
    assert!(people.iter().any(|p| p.uid == bob.uid));
    alice.w.send(&C2S::ForgetClient(carol_uid)).await.unwrap();
    println!("6. Carol was removed; again: \"{}\"", error(&mut alice).await);
    let _ = bob.id;
    println!("SERVER CALENDAR TEST PASSED");
}
