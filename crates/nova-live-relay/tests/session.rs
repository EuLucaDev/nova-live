//! Um anfitrião e dois convidados editando o mesmo documento por um relay real (127.0.0.1).
//! O "documento" é uma lista de textos; comandos: push (acrescenta) e set (troca um item).

use std::time::{Duration, Instant};

use nova_live::{Adapter, Change, Event, Invite, Live};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, PartialEq)]
struct Doc(Vec<String>);

impl Doc {
    fn exec(&mut self, cmd: &str, p: &Value) {
        match cmd {
            "push" => self.0.push(p["v"].as_str().unwrap_or_default().to_string()),
            "set" => {
                let i = p["i"].as_u64().unwrap_or(0) as usize;
                if let Some(x) = self.0.get_mut(i) {
                    *x = p["v"].as_str().unwrap_or_default().to_string();
                }
            }
            _ => {}
        }
    }
    fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.0).unwrap()
    }
    fn load(&mut self, b: &[u8]) {
        self.0 = serde_json::from_slice(b).unwrap();
    }
}

/// O "app": documento + alterações locais a enviar.
struct App {
    doc: Doc,
    outbox: Vec<Change>,
}

impl Adapter for App {
    fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.outbox)
    }
    fn snapshot(&mut self) -> nova_live::Result<Vec<u8>> {
        Ok(self.doc.bytes())
    }
    fn load(&mut self, doc: &[u8]) -> nova_live::Result<()> {
        self.doc.load(doc);
        Ok(())
    }
    fn apply(&mut self, cmd: &str, params: &Value, _ctx: &Value) -> nova_live::Result<()> {
        self.doc.exec(cmd, params);
        Ok(())
    }
    fn digest(&mut self) -> nova_live::Result<u64> {
        Ok(nova_live::digest(&self.doc.bytes()))
    }
}

struct Client {
    live: Live,
    app: App,
}

impl Client {
    fn new(live: Live, doc: Doc) -> Client {
        Client {
            live,
            app: App {
                doc,
                outbox: vec![],
            },
        }
    }
    fn edit(&mut self, cmd: &str, p: Value) {
        self.app.doc.exec(cmd, &p);
        self.app.outbox.push(Change::Op {
            cmd: cmd.into(),
            params: p,
            ctx: json!({}),
        });
    }
    /// Desfazer local: o documento inteiro vai para a sessão.
    fn undo_last(&mut self) {
        self.app.doc.0.pop();
        self.app.outbox.push(Change::Replace);
    }
    fn pump(&mut self) {
        self.live.pump(&mut self.app);
    }
}

fn relay() -> String {
    // NOVA_LIVE_SERVER=wss://live.agcunae.com/ roda os mesmos testes contra o servidor real.
    if let Ok(s) = std::env::var("NOVA_LIVE_SERVER") {
        return s;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || rt.block_on(nova_live_relay::run(listener)));
    format!("ws://{addr}/")
}

fn pump_until(clients: &mut [&mut Client], cond: impl Fn(&[&mut Client]) -> bool) {
    let start = Instant::now();
    while !cond(clients) {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "não convergiu: {:?}",
            clients
                .iter()
                .map(|c| c.app.doc.clone())
                .collect::<Vec<_>>()
        );
        for c in clients.iter_mut() {
            c.pump();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn host_and_guests_converge_and_late_joiner_gets_the_document() {
    let server = relay();
    let mut host = Client::new(
        Live::host("novagrid", &server, "Ana").unwrap(),
        Doc(vec!["a".into()]),
    );
    let invite = host.live.invite().clone();
    let mut bia = Client::new(
        Live::join(&invite, "novagrid", "Bia").unwrap(),
        Doc::default(),
    );
    pump_until(&mut [&mut host, &mut bia], |c| {
        c[1].app.doc.0 == vec!["a".to_string()]
    });

    // Edições simultâneas dos dois lados.
    host.edit("push", json!({"v": "host-1"}));
    bia.edit("push", json!({"v": "bia-1"}));
    bia.edit("set", json!({"i": 0, "v": "A"}));
    host.edit("push", json!({"v": "host-2"}));

    // Terceira pessoa entra no meio.
    let mut caio = Client::new(
        Live::join(
            &Invite::parse(&invite.to_share_link()).unwrap(),
            "novagrid",
            "Caio",
        )
        .unwrap(),
        Doc::default(),
    );
    caio.edit("push", json!({"v": "caio-1"}));

    pump_until(&mut [&mut host, &mut bia, &mut caio], |c| {
        c[0].app.doc.0.len() == 5
            && c.iter().all(|x| x.app.doc == c[0].app.doc)
            && c[0].live.peers().len() == 3
    });
    assert!(host.app.doc.0.contains(&"caio-1".to_string()) && host.app.doc.0[0] == "A");

    // Desfazer de um convidado chega a todos como documento inteiro.
    bia.undo_last();
    pump_until(&mut [&mut host, &mut bia, &mut caio], |c| {
        c[0].app.doc.0.len() == 4 && c.iter().all(|x| x.app.doc == c[0].app.doc)
    });
}

#[test]
fn wrong_app_or_token_is_refused_and_host_leaving_ends_the_session() {
    let server = relay();
    let host = Live::host("novagrid", &server, "Ana").unwrap();
    let invite = host.invite().clone();
    assert!(
        Live::join(&invite, "novaword", "X").is_err(),
        "convite de outro app"
    );

    let mut bad = invite.clone();
    bad.token = "zzzzzzzzzzzzzzzzzzzzzz".into();
    let mut intruder = Live::join(&bad, "novagrid", "X").unwrap();
    let start = Instant::now();
    let mut refused = false;
    while start.elapsed() < Duration::from_secs(5) && !refused {
        refused = intruder
            .poll()
            .iter()
            .any(|e| matches!(e, Event::Ended(r) if r.contains("inválido")));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(refused, "segredo errado precisa ser recusado");

    let mut guest = Live::join(&invite, "novagrid", "Bia").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    drop(host);
    let start = Instant::now();
    let mut ended = false;
    while start.elapsed() < Duration::from_secs(5) && !ended {
        ended = guest.poll().iter().any(|e| matches!(e, Event::Ended(_)));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ended, "a saída do anfitrião encerra a sessão");
}
