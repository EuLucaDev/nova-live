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
    std::thread::spawn(move || rt.block_on(nova_live_relay::run_local(listener)));
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
fn wrong_app_or_token_is_refused_and_host_can_end_the_session() {
    let server = relay();
    let mut host = Live::host("novagrid", &server, "Ana").unwrap();
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
    host.end();
    drop(host);
    let start = Instant::now();
    let mut ended = false;
    while start.elapsed() < Duration::from_secs(5) && !ended {
        ended = guest.poll().iter().any(|e| matches!(e, Event::Ended(_)));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ended, "encerrar, pelo anfitrião, encerra para todos");
}

fn wait(clients: &mut [&mut Client], what: &str, cond: impl Fn(&[&mut Client]) -> bool) {
    let start = Instant::now();
    while !cond(clients) {
        assert!(start.elapsed() < Duration::from_secs(15), "{what}");
        for c in clients.iter_mut() {
            c.pump();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn when_the_host_leaves_the_next_person_takes_over_and_the_session_goes_on() {
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
    let mut caio = Client::new(
        Live::join(&invite, "novagrid", "Caio").unwrap(),
        Doc::default(),
    );
    wait(&mut [&mut host, &mut bia, &mut caio], "entrada", |c| {
        c.iter().all(|x| x.app.doc.0 == vec!["a".to_string()]) && c[0].live.peers().len() == 3
    });
    host.edit("push", json!({"v": "da Ana"}));
    wait(
        &mut [&mut host, &mut bia, &mut caio],
        "edição da Ana",
        |c| c.iter().all(|x| x.app.doc.0.len() == 2),
    );

    // A anfitriã fecha o app (sem encerrar): Bia, a mais antiga, assume.
    drop(host);
    wait(&mut [&mut bia, &mut caio], "Bia vira anfitriã", |c| {
        c[0].live.role() == nova_live::Role::Host && c[1].live.peers().len() == 2
    });
    assert!(!bia.live.is_ended() && !caio.live.is_ended());
    assert!(caio.live.peers().iter().any(|p| p.name == "Bia" && p.host));

    // A sessão segue: edições continuam e quem entra agora recebe o documento da Bia.
    caio.edit("push", json!({"v": "do Caio"}));
    let mut duda = Client::new(
        Live::join(&invite, "novagrid", "Duda").unwrap(),
        Doc::default(),
    );
    wait(
        &mut [&mut bia, &mut caio, &mut duda],
        "Duda recebe o documento",
        |c| {
            c.iter()
                .all(|x| x.app.doc.0 == vec!["a".to_string(), "da Ana".into(), "do Caio".into()])
        },
    );
}

#[test]
fn a_new_invite_stops_the_old_link_but_keeps_who_is_already_in() {
    let server = relay();
    let mut host = Client::new(
        Live::host("novagrid", &server, "Ana").unwrap(),
        Doc(vec!["a".into()]),
    );
    let old = host.live.invite().clone();
    let mut bia = Client::new(Live::join(&old, "novagrid", "Bia").unwrap(), Doc::default());
    wait(&mut [&mut host, &mut bia], "entrada", |c| {
        c[1].app.doc.0.len() == 1
    });

    let new = host.live.renew_invite().unwrap();
    assert_ne!(new.token, old.token);
    assert_eq!(new.room, old.room);
    wait(
        &mut [&mut host, &mut bia],
        "Bia recebe o segredo novo",
        |c| c[1].live.invite().token == new.token,
    );

    // O link antigo é recusado; o novo entra; Bia continua editando.
    let mut late = Live::join(&old, "novagrid", "Caio").unwrap();
    let start = Instant::now();
    let mut refused = false;
    while start.elapsed() < Duration::from_secs(5) && !refused {
        refused = late.poll().iter().any(|e| matches!(e, Event::Ended(_)));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(refused, "o convite antigo não vale mais");
    let mut duda = Client::new(
        Live::join(&new, "novagrid", "Duda").unwrap(),
        Doc::default(),
    );
    bia.edit("push", json!({"v": "da Bia"}));
    wait(
        &mut [&mut host, &mut bia, &mut duda],
        "convite novo funciona",
        |c| {
            c.iter()
                .all(|x| x.app.doc.0 == vec!["a".to_string(), "da Bia".into()])
        },
    );
}

/// Um app com mídias: o documento usa `keys`; `files` diz onde cada uma está nesta máquina.
struct MediaApp {
    doc: Doc,
    keys: Vec<String>,
    files: std::collections::HashMap<String, std::path::PathBuf>,
}

impl Adapter for MediaApp {
    fn take_changes(&mut self) -> Vec<Change> {
        vec![]
    }
    fn snapshot(&mut self) -> nova_live::Result<Vec<u8>> {
        Ok(self.doc.bytes())
    }
    fn load(&mut self, doc: &[u8]) -> nova_live::Result<()> {
        self.doc.load(doc);
        Ok(())
    }
    fn apply(&mut self, _: &str, _: &Value, _: &Value) -> nova_live::Result<()> {
        Ok(())
    }
    fn digest(&mut self) -> nova_live::Result<u64> {
        Ok(nova_live::digest(&self.doc.bytes()))
    }
    fn media(&mut self) -> Vec<String> {
        self.keys.clone()
    }
    fn media_available(&mut self, key: &str) -> bool {
        self.files.contains_key(key)
    }
    fn media_open(&mut self, key: &str) -> Option<(Box<dyn std::io::Read + Send>, u64)> {
        nova_live::media::open_file(self.files.get(key)?)
    }
    fn media_ready(&mut self, key: &str, path: &std::path::Path) -> nova_live::Result<()> {
        self.files.insert(key.to_string(), path.to_path_buf());
        Ok(())
    }
}

#[test]
fn media_the_document_uses_reaches_the_guest_and_nothing_else_leaves() {
    let dir = std::env::temp_dir().join(format!("nova-live-media-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ana")).unwrap();
    // SAFETY dos testes: a pasta de mídias recebidas é a deste teste.
    unsafe { std::env::set_var("NOVA_LIVE_MEDIA_DIR", dir.join("recebidas")) };
    let video = dir.join("ana/entrevista.mov");
    let content: Vec<u8> = (0..3_500_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&video, &content).unwrap();
    let secret = dir.join("ana/segredo.txt");
    std::fs::write(&secret, b"nao pode sair").unwrap();
    let key = video.to_string_lossy().into_owned();

    let server = relay();
    let mut ana_live = Live::host("novafilm", &server, "Ana").unwrap();
    let mut ana = MediaApp {
        doc: Doc(vec![key.clone()]),
        keys: vec![key.clone()],
        files: [(key.clone(), video.clone())].into(),
    };
    let invite = ana_live.invite().clone();
    let mut bia_live = Live::join(&invite, "novafilm", "Bia").unwrap();
    // Bia "acha" que o documento também usa o segredo da Ana: o pedido não pode ser atendido.
    let secret_key = secret.to_string_lossy().into_owned();
    let mut bia = MediaApp {
        doc: Doc::default(),
        keys: vec![key.clone(), secret_key.clone()],
        files: Default::default(),
    };
    let start = Instant::now();
    while !bia.files.contains_key(&key) {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "a mídia não chegou"
        );
        ana_live.pump(&mut ana);
        bia_live.pump(&mut bia);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        std::fs::read(&bia.files[&key]).unwrap(),
        content,
        "conteúdo idêntico"
    );
    assert!(
        bia.files[&key].starts_with(dir.join("recebidas")),
        "fica na pasta de mídias"
    );
    // Mais um tempo: o segredo (fora do documento da Ana) nunca chega.
    let wait = Instant::now();
    while wait.elapsed() < Duration::from_secs(2) {
        ana_live.pump(&mut ana);
        bia_live.pump(&mut bia);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !bia.files.contains_key(&secret_key),
        "só sai o que o documento usa"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn each_person_sees_the_others_mouse_with_name_and_colour() {
    let server = relay();
    let mut ana = Client::new(
        Live::host("novafig", &server, "Ana").unwrap(),
        Doc(vec!["a".into()]),
    );
    let invite = ana.live.invite().clone();
    let mut bia = Client::new(
        Live::join(&invite, "novafig", "Bia").unwrap(),
        Doc::default(),
    );
    wait(&mut [&mut ana, &mut bia], "entrada", |c| {
        c[1].app.doc.0.len() == 1
    });
    ana.live.set_cursor(Some(("pagina-1".into(), 120.5, 40.0)));
    wait(&mut [&mut ana, &mut bia], "mouse da Ana chega", |c| {
        c[1].live
            .cursors()
            .iter()
            .any(|m| m.name == "Ana" && m.space == "pagina-1" && m.x == 120.5)
    });
    let m = &bia.live.cursors()[0];
    assert_eq!(m.color, nova_live::cursors::color_for("Ana"));
    assert!(ana.live.cursors().is_empty(), "ninguém vê o próprio mouse");
    ana.live.set_cursor(None);
    wait(&mut [&mut ana, &mut bia], "mouse sai", |c| {
        c[1].live.cursors().is_empty()
    });
}
