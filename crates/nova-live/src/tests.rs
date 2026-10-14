#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

fn sample() -> Invite {
    Invite {
        app: "novagrid".into(),
        server: "wss://live.agcunae.com/".into(),
        room: "AbCdEfGhIjKlMnOpQr".into(),
        token: "TtTtTtTtTtTtTtTtTtTt-_".into(),
    }
}

#[test]
fn invite_links_round_trip() {
    let i = sample();
    assert_eq!(Invite::parse(&i.to_link()).unwrap(), i);
    let share = i.to_share_link();
    assert!(
        share.starts_with("https://workflowind.agcunae.com/apps/join#invite=novagrid%3A%2F%2Fjoin")
    );
    assert_eq!(Invite::parse(&share).unwrap(), i);
    assert_eq!(Invite::parse(&format!("  {share}\n")).unwrap(), i);
}

#[test]
fn invites_reject_garbage_and_other_schemes() {
    let i = sample();
    for bad in [
        String::new(),
        "javascript:alert(1)".into(),
        "novafig://join?host=1.2.3.4:1&file=a&token=bbbbbbbbbbbbbbbbbb".into(),
        i.to_link().replace("token=", "token=x&token="),
        i.to_link().replace("wss%3A", "https%3A"),
        i.to_link().replace("app=novagrid", "app=nova grid"),
        i.to_link() + "&extra=1",
        i.to_link().replace("room=AbCdEfGhIjKlMnOpQr", "room=short"),
        "x".repeat(5000),
    ] {
        assert!(Invite::parse(&bad).is_err(), "aceitou {bad:?}");
    }
}

#[test]
fn ids_are_random_and_long_enough() {
    let a = random_id(16).unwrap();
    let b = random_id(16).unwrap();
    assert_ne!(a, b);
    assert!(a.len() >= 21);
}

#[test]
fn digest_is_stable() {
    assert_eq!(digest(b"nova"), digest(b"nova"));
    assert_ne!(digest(b"nova"), digest(b"novb"));
}

#[test]
fn invitations_reject_foreign_wrappers_and_credential_bearing_endpoints() {
    let i = sample();
    assert!(
        Invite::parse(
            &i.to_share_link()
                .replace(JOIN_PAGE, "https://example.org/invite")
        )
        .is_err()
    );
    for server in [
        "wss://user:password@live.agcunae.com/",
        "wss://live.agcunae.com/?token=x",
        "wss://live.agcunae.com/#x",
        "wss://",
        "wss://live.agcunae.com:0/",
    ] {
        let bad = Invite {
            server: server.into(),
            ..i.clone()
        };
        assert!(Invite::parse(&bad.to_link()).is_err());
    }
}

#[test]
fn each_app_has_its_own_link_and_old_links_still_work() {
    let i = sample();
    assert!(i.to_link().starts_with("novagrid://join?app=novagrid&"));
    let old = i.to_link().replacen("novagrid://", "novalive://", 1);
    assert_eq!(Invite::parse(&old).unwrap(), i);
    // O esquema precisa ser o do app do convite.
    assert!(Invite::parse(&i.to_link().replacen("novagrid://", "novaword://", 1)).is_err());
    assert!(Invite::parse(&i.to_link().replacen("novagrid://", "java script://", 1)).is_err());
    assert_eq!(
        Invite::parse(&i.to_link().replacen("novagrid://", "NovaGrid://", 1)).unwrap(),
        i
    );
}
