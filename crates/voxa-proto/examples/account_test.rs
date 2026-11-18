//! Accounts and hidden channels (servers 1.25.0+): making an account, an authenticator
//! app, the server's rules (staff limited to Guest until they sign in), remembered
//! devices, recovery codes, and channels hidden from a group.
//! `cargo run -p voxa-proto --example account_test -- 127.0.0.1:9988 <server data dir>`
//! Must be built from the client repository (it reports the client version).

use std::time::Duration;

use hmac::{Hmac, Mac};
use tokio::net::TcpStream;
use tokio::time::timeout;
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

const NORMAL: GroupId = 2;

struct User {
    r: SecureReader<R>,
    w: SecureWriter<W>,
    id: ClientId,
    /// What arrived while logging in.
    welcome: Vec<S2C>,
    channels: Vec<ChannelId>,
}

async fn login(addr: &str, kp: &KeyPair, nick: &str) -> User {
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
    let S2C::Welcome { your_id, channels, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    let mut welcome = Vec::new();
    while let Ok(Ok(m)) = timeout(Duration::from_millis(400), r.recv::<S2C>()).await {
        welcome.push(m);
    }
    User { r, w, id: your_id, welcome, channels: channels.iter().map(|c| c.id).collect() }
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

async fn state(u: &mut User) -> AccountState {
    wait(u, 5, |m| if let S2C::AccountState(a) = m { Some(a) } else { None }).await.expect("no account state")
}

fn welcome_state(u: &User) -> AccountState {
    u.welcome.iter().rev().find_map(|m| if let S2C::AccountState(a) = m { Some(a.clone()) } else { None }).expect("no account state after welcome")
}

fn unbase32(s: &str) -> Vec<u8> {
    const A: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut buf, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.chars() {
        buf = (buf << 5) | A.find(c).unwrap() as u32;
        bits += 5;
        if bits >= 8 {
            out.push((buf >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    out
}

/// The 6-digit code an authenticator app would show now (RFC 6238).
fn totp(secret: &[u8], offset: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let step = (now / 30) as i64 + offset;
    let mut m = Hmac::<sha1::Sha1>::new_from_slice(secret).unwrap();
    m.update(&(step as u64).to_be_bytes());
    let h = m.finalize().into_bytes();
    let o = (h[19] & 0xf) as usize;
    let bin = ((h[o] as u32 & 0x7f) << 24) | ((h[o + 1] as u32) << 16) | ((h[o + 2] as u32) << 8) | h[o + 3] as u32;
    format!("{:06}", bin % 1_000_000)
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let alice_key = KeyPair::generate();
    let mut alice = login(&addr, &alice_key, "Alice").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    alice.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    wait(&mut alice, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    let st = welcome_state(&alice);
    assert!(st.username.is_none() && !st.limited);
    let mut bob = login(&addr, &KeyPair::generate(), "Bob").await;
    println!("1. accounts are off by default: nobody is limited");

    // Accounts off: none can be made.
    bob.w.send(&C2S::CreateAccount { username: "bob".into(), password: "0123456789".into(), email: None }).await.unwrap();
    println!("2. making an account while they are off: \"{}\"", error(&mut bob).await);

    // An admin cannot require accounts for staff without one (they would lose their rights).
    let staff = AccountPolicy { mode: AccountMode::RequiredForStaff, mfa_for_staff: false, email_reset: false };
    alice.w.send(&C2S::SetAccountPolicy(staff)).await.unwrap();
    println!("3. requiring accounts without one: \"{}\"", error(&mut alice).await);

    alice.w.send(&C2S::SetAccountPolicy(AccountPolicy { mode: AccountMode::Optional, ..staff })).await.unwrap();
    let p = wait(&mut bob, 5, |m| if let S2C::AccountPolicy(p) = m { Some(p) } else { None }).await.expect("no policy");
    assert_eq!(p.mode, AccountMode::Optional);
    assert!(!p.email_reset, "no SMTP in the test server");

    // Weak passwords and bad names are refused.
    alice.w.send(&C2S::CreateAccount { username: "alice".into(), password: "short".into(), email: None }).await.unwrap();
    println!("4. a short password: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::CreateAccount { username: "a l".into(), password: "0123456789".into(), email: None }).await.unwrap();
    println!("4. a bad username: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::CreateAccount { username: "alice".into(), password: "correct horse 1".into(), email: Some("alice@example.com".into()) }).await.unwrap();
    let st = state(&mut alice).await;
    assert!(st.signed_in && st.username.as_deref() == Some("alice") && st.email.as_deref() == Some("a***@example.com"));
    println!("4. made an account, signed in; email shown as {}", st.email.unwrap());

    // Usernames are unique (any case).
    bob.w.send(&C2S::CreateAccount { username: "ALICE".into(), password: "0123456789x".into(), email: None }).await.unwrap();
    println!("5. a taken username: \"{}\"", error(&mut bob).await);

    // Requiring MFA for staff needs it first.
    alice.w.send(&C2S::SetAccountPolicy(AccountPolicy { mfa_for_staff: true, ..staff })).await.unwrap();
    println!("6. requiring MFA without it: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::StartMfa).await.unwrap();
    let (secret, uri) = wait(&mut alice, 5, |m| if let S2C::MfaSetup { secret, uri } = m { Some((secret, uri)) } else { None }).await.expect("no MFA setup");
    assert!(uri.starts_with("otpauth://totp/") && uri.contains(&secret));
    let secret = unbase32(&secret);
    alice.w.send(&C2S::ConfirmMfa("000000".into())).await.unwrap();
    println!("6. a wrong code: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::ConfirmMfa(totp(&secret, 0))).await.unwrap();
    let codes = wait(&mut alice, 5, |m| if let S2C::RecoveryCodes(c) = m { Some(c) } else { None }).await.expect("no recovery codes");
    assert_eq!(codes.len(), 10);
    let st = state(&mut alice).await;
    assert!(st.mfa && st.recovery_left == 10);
    println!("6. authenticator app on, 10 recovery codes ({}...)", codes[0]);
    alice.w.send(&C2S::SetAccountPolicy(AccountPolicy { mfa_for_staff: true, ..staff })).await.unwrap();
    let p = wait(&mut bob, 5, |m| if let S2C::AccountPolicy(p) = m { Some(p) } else { None }).await.expect("no policy");
    assert_eq!(p.mode, AccountMode::RequiredForStaff);
    let st = state(&mut bob).await;
    assert!(!st.limited, "members (the default group) are not staff");
    println!("7. accounts required for staff, with MFA; Bob (a member) is not limited");

    // A channel hidden from members: Bob loses it and Alice in it.
    let mut e = ChannelEdit::new_default(None);
    e.name = "Staff room".into();
    alice.w.send(&C2S::CreateChannel(e)).await.unwrap();
    let aid = alice.id;
    let secret_ch = wait(&mut alice, 5, |m| match m {
        S2C::ClientMoved { id, channel, .. } if id == aid => Some(channel),
        _ => None,
    })
    .await
    .expect("no channel");
    alice.w.send(&C2S::SetChannelHidden { channel: secret_ch, groups: vec![NORMAL] }).await.unwrap();
    let hidden = wait(&mut alice, 5, |m| if let S2C::HiddenChannels(h) = m { Some(h) } else { None }).await.expect("no hidden list");
    assert_eq!(hidden, vec![(secret_ch, vec![NORMAL])]);
    let (mut lost_ch, mut lost_alice) = (false, false);
    wait(&mut bob, 5, |m| {
        match m {
            S2C::ChannelsVisible { hidden, .. } if hidden.contains(&secret_ch) => lost_ch = true,
            S2C::ClientLeft { id, .. } if id == aid => lost_alice = true,
            _ => {}
        }
        (lost_ch && lost_alice).then_some(())
    })
    .await
    .expect("Bob still sees the channel or Alice");
    bob.w.send(&C2S::SwitchChannel { channel: secret_ch, password: None }).await.unwrap();
    println!("8. hidden from members: Bob lost the channel and Alice; joining it: \"{}\"", error(&mut bob).await);
    bob.w.send(&C2S::FetchHistory { target: TextTarget::Channel(secret_ch), before: None, limit: 10 }).await.unwrap();
    println!("8. reading its chat: \"{}\"", error(&mut bob).await);
    let bob2 = login(&addr, &KeyPair::generate(), "Bob2").await;
    assert!(!bob2.channels.contains(&secret_ch), "a newcomer was told about the hidden channel");
    assert!(!bob2.welcome.iter().any(|m| matches!(m, S2C::ClientJoined(c) if c.id == aid)));
    println!("8. a newcomer does not get it either");

    // Alice again, from scratch: limited to Guest (staff, not signed in), so the channel is
    // hidden from her too... except it is her own: she is put in the default channel.
    drop(alice);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut alice = login(&addr, &alice_key, "Alice").await;
    let st = welcome_state(&alice);
    assert!(st.limited && !st.signed_in && st.username.as_deref() == Some("alice"));
    alice.w.send(&C2S::SetChannelHidden { channel: secret_ch, groups: vec![] }).await.unwrap();
    println!("9. staff not signed in are limited (\"{}\"); editing: \"{}\"", st.reason, error(&mut alice).await);
    // The right password alone must not make recovery codes (they would replace the app).
    alice.w.send(&C2S::NewRecoveryCodes { password: "correct horse 1".into() }).await.unwrap();
    println!("9. new recovery codes with only the password: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::SignIn { username: "alice".into(), password: "wrong password".into(), code: None, remember: false }).await.unwrap();
    println!("10. a wrong password: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::SignIn { username: "alice".into(), password: "correct horse 1".into(), code: None, remember: false }).await.unwrap();
    println!("10. without the code: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::SignIn { username: "alice".into(), password: "correct horse 1".into(), code: Some(codes[0].to_lowercase()), remember: true }).await.unwrap();
    let token = wait(&mut alice, 5, |m| if let S2C::AccountToken(t) = m { Some(t) } else { None }).await.expect("no token");
    let st = state(&mut alice).await;
    assert!(st.signed_in && !st.limited && st.recovery_left == 9);
    println!("10. signed in with a recovery code (9 left), this device remembered");
    alice.w.send(&C2S::SignIn { username: "alice".into(), password: "correct horse 1".into(), code: Some(codes[0].clone()), remember: false }).await.unwrap();
    println!("10. the same recovery code again: \"{}\"", error(&mut alice).await);

    // A remembered device signs in without the password.
    drop(alice);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut alice = login(&addr, &alice_key, "Alice").await;
    alice.w.send(&C2S::ResumeAccount("not a token".into())).await.unwrap();
    println!("11. a bad token: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::ResumeAccount(token.clone())).await.unwrap();
    let st = wait(&mut alice, 5, |m| match m {
        S2C::AccountState(a) if a.signed_in => Some(a),
        _ => None,
    })
    .await
    .expect("not signed in by token");
    assert!(!st.limited);
    println!("11. the remembered device signed in by itself");

    // Showing the channel again: Bob gets it back.
    alice.w.send(&C2S::SetChannelHidden { channel: secret_ch, groups: vec![] }).await.unwrap();
    wait(&mut bob, 5, |m| matches!(&m, S2C::ChannelsVisible { shown, .. } if shown.iter().any(|c| c.id == secret_ch)).then_some(())).await.expect("Bob did not get the channel back");
    println!("12. no longer hidden: Bob sees the channel again");

    // A forgotten password, reset with a recovery code; other devices must sign in again.
    alice.w.send(&C2S::ResetPassword { username: "alice".into(), code: "AAAAA-AAAAA".into(), password: "a new password".into() }).await.unwrap();
    println!("13. a wrong recovery code: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::ResetPassword { username: "alice".into(), code: codes[1].clone(), password: "a new password".into() }).await.unwrap();
    wait(&mut alice, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await.expect("no reset notice");
    alice.w.send(&C2S::ResumeAccount(token)).await.unwrap();
    println!("13. password reset; the old device token: \"{}\"", error(&mut alice).await);
    // (The code of the next 30 seconds: the current one was used to turn the app on.)
    alice.w.send(&C2S::SignIn { username: "alice".into(), password: "a new password".into(), code: Some(totp(&secret, 1)), remember: false }).await.unwrap();
    let st = wait(&mut alice, 5, |m| match m {
        S2C::AccountState(a) if a.signed_in => Some(a),
        _ => None,
    })
    .await
    .expect("could not sign in with the new password");
    assert!(st.recovery_left == 8);

    // Turning accounts off again: nobody is limited.
    alice.w.send(&C2S::SetAccountPolicy(AccountPolicy::default())).await.unwrap();
    wait(&mut alice, 5, |m| matches!(m, S2C::AccountPolicy(p) if p.mode == AccountMode::Off).then_some(())).await.expect("no policy");
    println!("ACCOUNT TEST PASSED");
}
