//! Nova Live: colaboração em tempo real comum aos apps Nova Foods.
//!
//! Todo app Nova Foods edita o documento por comandos (`id` + parâmetros JSON). Uma sessão Nova
//! Live replica esses comandos entre os participantes de um mesmo documento:
//!
//! - O **relay** (`nova-live-relay`, em `wss://live.agcunae.com/`) mantém uma sala por documento,
//!   numera cada alteração (ordem total) e a repassa a todos, inclusive a quem a enviou.
//! - Cada participante aplica as alterações na ordem do relay. As próprias são aplicadas na hora
//!   (otimistas) e ignoradas quando voltam.
//! - O **anfitrião** é a referência: entrega o documento a quem entra e, depois de um tempo sem
//!   edições, publica a "impressão digital" do documento. Quem estiver diferente pede uma cópia
//!   nova e a carrega. Assim, mesmo um comando não determinístico não deixa ninguém divergido.
//! - Mudanças que não são comandos (desfazer, refazer, abrir outro arquivo) seguem como
//!   documento inteiro (`replace`).
//!
//! Convites: `<app>://join?app=<app>&server=<wss>&room=<id>&token=<segredo>` (o esquema é o do
//! app, `novagrid://…`, para o sistema abrir o app certo; `novalive://…` antigo também vale), ou o
//! mesmo dentro de `https://workflowind.agcunae.com/apps/join#invite=<link>` (o segredo fica no
//! fragmento e não chega a nenhum servidor web). Um convite vale para um documento de um app.
//!
//! A biblioteca não usa runtime assíncrono: uma thread de rede por sessão, e o app chama
//! [`Live::poll`] a cada quadro da interface.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod account;
mod adapter;
pub mod cursors;
pub mod document_ticket;
pub mod documents;
pub mod identity;
pub mod links;
pub mod media;
mod net;
pub mod offline;
pub mod proto;

pub use adapter::{Adapter, Change, Notice};
pub use cursors::RemoteCursor;
pub use media::MediaProgress;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value as Json;

pub use proto::{ClientMsg, Peer, ServerMsg};

/// Servidor padrão das sessões online da Nova Foods.
pub const DEFAULT_SERVER: &str = "wss://live.agcunae.com/";
/// Página que abre convites (o app certo ou o launcher para instalá-lo).
pub const JOIN_PAGE: &str = "https://workflowind.agcunae.com/apps/join";
/// Depois de tanto tempo sem alterações, o anfitrião publica a impressão digital.
pub const DIGEST_IDLE: Duration = Duration::from_millis(1500);

/// O que pode dar errado numa sessão.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn err(s: impl Into<String>) -> Error {
    Error(s.into())
}

/// Um convite para um documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// App que abre o documento (`novagrid`, `novaword`…). Outro app recusa o convite.
    pub app: String,
    /// Relay (`wss://live.agcunae.com/`).
    pub server: String,
    /// Sala (aleatória, 128 bits).
    pub room: String,
    /// Segredo de convidado (aleatório, 128 bits).
    pub token: String,
}

pub(crate) fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub(crate) fn dec(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'%') => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            Some(b'+') => {
                out.push(b' ');
                i += 1;
            }
            Some(&b) => {
                out.push(b);
                i += 1;
            }
            None => break,
        }
    }
    String::from_utf8(out).ok()
}

fn valid_id(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Invite {
    /// `<app>://join?…`: o link que abre o app (o sistema escolhe o app pelo esquema).
    pub fn to_link(&self) -> String {
        format!(
            "{}://join?app={}&server={}&room={}&token={}",
            self.app,
            enc(&self.app),
            enc(&self.server),
            enc(&self.room),
            enc(&self.token)
        )
    }

    /// O link para enviar às pessoas: abre a página de convite, que abre o app ou o launcher.
    pub fn to_share_link(&self) -> String {
        format!("{JOIN_PAGE}#invite={}", enc(&self.to_link()))
    }

    /// Aceita o link do app, o link de compartilhamento ou o link colado com espaços em volta.
    pub fn parse(input: &str) -> Result<Invite> {
        let s = input.trim();
        if s.len() > 4096 {
            return Err(err("convite longo demais"));
        }
        let link = if let Some(encoded) = s.strip_prefix(&format!("{JOIN_PAGE}#invite=")) {
            dec(encoded).ok_or_else(|| err("convite inválido"))?
        } else {
            s.to_string()
        };
        let (scheme, query) = link
            .split_once("://join?")
            .ok_or_else(|| err("isto não é um convite Nova Live"))?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "novalive" && !valid_id(&scheme, 2, 40) {
            return Err(err("isto não é um convite Nova Live"));
        }
        let (mut app, mut server, mut room, mut token) = (None, None, None, None);
        for pair in query.split('&') {
            let (k, v) = pair
                .split_once('=')
                .ok_or_else(|| err("convite inválido"))?;
            let v = dec(v).ok_or_else(|| err("convite inválido"))?;
            if k == "name" {
                // Nome da conta Nova Foods de quem abriu o convite pelo site (guardado pelo app).
                continue;
            }
            let slot = match k {
                "app" => &mut app,
                "server" => &mut server,
                "room" => &mut room,
                "token" => &mut token,
                _ => return Err(err("convite inválido")),
            };
            if slot.replace(v).is_some() {
                return Err(err("convite inválido"));
            }
        }
        let (Some(app), Some(server), Some(room), Some(token)) = (app, server, room, token) else {
            return Err(err("convite incompleto"));
        };
        if scheme != "novalive" && scheme != app {
            return Err(err("convite inválido"));
        }
        if !valid_id(&app, 2, 40) || !valid_id(&room, 16, 64) || !valid_id(&token, 16, 128) {
            return Err(err("convite inválido"));
        }
        if server.len() > 300 || net::validate_server(&server).is_err() {
            return Err(err("servidor do convite inválido"));
        }
        Ok(Invite {
            app,
            server,
            room,
            token,
        })
    }
}

/// Texto aleatório com `bytes` bytes de entropia, em base64 url-safe sem preenchimento.
pub fn random_id(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| err(format!("sem aleatoriedade do sistema: {e}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

/// Valor inicial da impressão digital (FNV-1a 64).
pub(crate) const DIGEST_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// Continua uma impressão digital FNV-1a 64 com mais bytes (arquivos em pedaços).
pub(crate) fn digest_update(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Impressão digital estável (FNV-1a 64) de bytes canônicos do documento.
pub fn digest(bytes: &[u8]) -> u64 {
    digest_update(DIGEST_SEED, bytes)
}

/// O que o app precisa fazer, vindo da sessão.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Carregar este documento inteiro (entrada na sessão, ressincronização ou desfazer de alguém).
    Load(Vec<u8>),
    /// Aplicar o comando de outra pessoa, com o contexto (seleção) que ela tinha.
    Apply {
        author: String,
        cmd: String,
        params: Json,
        ctx: Json,
    },
    /// Alguém pediu o documento: responder com [`Live::send_snapshot`] (só o anfitrião recebe).
    SnapshotWanted,
    /// Quem está na sessão mudou.
    Peers(Vec<Peer>),
    /// A sessão terminou (motivo para mostrar).
    Ended(String),
    /// Quem coordenava saiu e agora você é o anfitrião (entrega o documento a quem entrar).
    Promoted,
    /// Alguém (`from`) pediu a mídia `key`.
    MediaWanted { from: String, key: String },
    /// Chegou um pedaço de mídia.
    MediaChunk {
        from: String,
        key: String,
        offset: u64,
        total: u64,
        data: String,
        digest: Option<u64>,
    },
}

/// Papel nesta sessão.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Guest,
}

/// Uma sessão em andamento. Soltar (drop) encerra a conexão.
pub struct Live {
    role: Role,
    invite: Invite,
    me: String,
    out: Sender<ClientMsg>,
    inbox: Receiver<ServerMsg>,
    peers: Vec<Peer>,
    /// Alterações próprias ainda não devolvidas pelo relay (ids locais).
    pending: Vec<u64>,
    next_local: u64,
    /// Última revisão recebida do relay.
    rev: u64,
    last_change: Option<Instant>,
    digest_sent_for: u64,
    /// Impressão digital do anfitrião ainda não conferida: (rev, valor).
    check: Option<(u64, u64)>,
    ended: bool,
    /// Anfitrião: alguém espera o documento (respondido quando não houver alterações a caminho).
    snapshot_wanted: bool,
    /// Convidado: já carregou o documento; antes disso as alterações ficam guardadas.
    loaded: bool,
    early: Vec<ServerMsg>,
    /// Ciclo por quadro ([`Live::pump`]): documento inteiro pendente e participantes conhecidos.
    replace_due: bool,
    last_replace: Option<Instant>,
    known: Vec<Peer>,
    /// Mídias pedidas, recebendo e enviando.
    pub(crate) media: media::MediaState,
    /// Bytes de mídia na fila da thread de rede.
    pub(crate) backlog: Arc<AtomicUsize>,
    /// Mouses (o meu e os dos colegas).
    pub(crate) cursors: cursors::CursorState,
    views: std::collections::HashMap<String, ([f32; 2], f32, f32)>,
    view_sent: Option<Instant>,
}

impl Live {
    /// Abre uma sessão para o documento atual e devolve o convite para enviar.
    pub fn host(app: &str, server: &str, name: &str) -> Result<Live> {
        let invite = Invite {
            app: app.to_string(),
            server: server.to_string(),
            room: random_id(16)?,
            token: random_id(18)?,
        };
        let host_token = random_id(18)?;
        let hello = ClientMsg::Hello {
            protocol: proto::PROTOCOL,
            app: invite.app.clone(),
            room: invite.room.clone(),
            token: invite.token.clone(),
            host_token: Some(host_token),
            name: name.to_string(),
        };
        Self::start(Role::Host, invite, hello)
    }

    /// Entra num documento pelo convite (de qualquer forma aceita por [`Invite::parse`]).
    pub fn join(invite: &Invite, app: &str, name: &str) -> Result<Live> {
        if invite.app != app {
            return Err(err(format!(
                "este convite é do app {}, não deste",
                invite.app
            )));
        }
        let hello = ClientMsg::Hello {
            protocol: proto::PROTOCOL,
            app: invite.app.clone(),
            room: invite.room.clone(),
            token: invite.token.clone(),
            host_token: None,
            name: name.to_string(),
        };
        Self::start(Role::Guest, invite.clone(), hello)
    }

    /// Opens the live room after the caller has downloaded the persisted revision.
    pub fn document(server: &str, payload: &str, signature: &str) -> Result<Live> {
        let claims =
            document_ticket::decode(payload).ok_or_else(|| err("Autorização inválida."))?;
        let invite = Invite {
            app: claims.app_id,
            server: server.into(),
            room: format!("doc-{}", claims.document_id),
            token: String::new(),
        };
        let mut live = Self::start(
            Role::Guest,
            invite,
            ClientMsg::DocumentHello {
                protocol: proto::PROTOCOL,
                payload: payload.into(),
                signature: signature.into(),
            },
        )?;
        live.loaded = true;
        Ok(live)
    }
    pub fn renew_document_ticket(&mut self, payload: &str, signature: &str) {
        self.send(ClientMsg::RenewDocumentTicket {
            payload: payload.into(),
            signature: signature.into(),
        });
    }
    fn start(role: Role, invite: Invite, hello: ClientMsg) -> Result<Live> {
        let backlog = Arc::new(AtomicUsize::new(0));
        let (out, inbox) = net::spawn(&invite.server, hello, backlog.clone())?;
        Ok(Live {
            role,
            invite,
            me: String::new(),
            out,
            inbox,
            peers: vec![],
            pending: vec![],
            next_local: 1,
            rev: 0,
            last_change: None,
            digest_sent_for: 0,
            check: None,
            ended: false,
            snapshot_wanted: false,
            loaded: role == Role::Host,
            early: vec![],
            replace_due: false,
            last_replace: None,
            known: vec![],
            media: media::MediaState::default(),
            backlog,
            cursors: cursors::CursorState::default(),
            views: Default::default(),
            view_sent: None,
        })
    }

    pub fn view(&self, peer: &str) -> Option<([f32; 2], f32, f32)> {
        self.views.get(peer).copied()
    }
    pub fn send_view(&mut self, center: [f32; 2], zoom: f32, rotation: f32) {
        if self
            .view_sent
            .is_some_and(|t| t.elapsed() < Duration::from_millis(100))
        {
            return;
        }
        self.view_sent = Some(Instant::now());
        self.send(ClientMsg::View {
            center,
            zoom,
            rotation,
        });
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn invite(&self) -> &Invite {
        &self.invite
    }
    pub fn local_peer_id(&self) -> &str {
        &self.me
    }
    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Anfitrião: encerra a sessão para todos (sair sem encerrar é só soltar a sessão: outro
    /// participante assume).
    pub fn end(&mut self) {
        if self.role == Role::Host {
            self.send(ClientMsg::End);
        }
        self.ended = true;
    }

    /// Anfitrião: gera um convite novo. O anterior deixa de valer para quem ainda não entrou;
    /// quem já está na sessão continua.
    pub fn renew_invite(&mut self) -> Result<Invite> {
        if self.role != Role::Host {
            return Err(err("só o anfitrião troca o convite"));
        }
        let token = random_id(18)?;
        self.send(ClientMsg::NewToken {
            token: token.clone(),
        });
        self.invite.token = token;
        Ok(self.invite.clone())
    }

    pub(crate) fn send(&mut self, msg: ClientMsg) {
        if self.out.send(msg).is_err() {
            self.ended = true;
        }
    }

    /// Uma alteração local feita por comando (já aplicada aqui).
    pub fn send_op(&mut self, cmd: &str, params: &Json, ctx: &Json) {
        let local = self.next_local;
        self.next_local += 1;
        self.pending.push(local);
        self.last_change = Some(Instant::now());
        self.send(ClientMsg::Op {
            local,
            cmd: cmd.to_string(),
            params: params.clone(),
            ctx: ctx.clone(),
        });
    }

    /// Uma alteração local que não é comando (desfazer, refazer…): o documento inteiro.
    pub fn send_replace(&mut self, doc: &[u8]) {
        let local = self.next_local;
        self.next_local += 1;
        self.pending.push(local);
        self.last_change = Some(Instant::now());
        self.send(ClientMsg::Replace {
            local,
            doc: base64::engine::general_purpose::STANDARD.encode(doc),
        });
    }

    /// Anfitrião: o documento para quem pediu (entrada ou ressincronização).
    pub fn send_snapshot(&mut self, doc: &[u8]) {
        self.send(ClientMsg::Snapshot {
            rev: self.rev,
            doc: base64::engine::general_purpose::STANDARD.encode(doc),
        });
    }

    /// Anfitrião: chame a cada quadro; quando a sessão fica parada, publica a impressão digital
    /// calculada por `digest_of` (só é chamada quando precisa).
    pub fn tick_digest(&mut self, digest_of: impl FnOnce() -> u64) {
        if self.role != Role::Host || self.rev == self.digest_sent_for || !self.pending.is_empty() {
            return;
        }
        if self.last_change.is_some_and(|t| t.elapsed() < DIGEST_IDLE) {
            return;
        }
        let d = digest_of();
        self.digest_sent_for = self.rev;
        self.send(ClientMsg::Digest {
            rev: self.rev,
            digest: d,
        });
    }

    /// Convidado: confere a impressão digital do anfitrião quando estiver em dia; diferente, pede
    /// o documento de novo. Chame a cada quadro.
    pub fn tick_check(&mut self, digest_of: impl FnOnce() -> u64) {
        let Some((rev, d)) = self.check else { return };
        if self.role != Role::Guest || rev != self.rev || !self.pending.is_empty() {
            return;
        }
        self.check = None;
        if digest_of() != d {
            self.send(ClientMsg::WantSnapshot);
        }
    }

    /// Mensagens do relay transformadas no que o app precisa fazer, na ordem.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = vec![];
        loop {
            match self.inbox.try_recv() {
                Ok(msg) => self.handle(msg, &mut events),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.ended {
                        self.ended = true;
                        events.push(Event::Ended("A conexão com a sessão caiu.".into()));
                    }
                    break;
                }
            }
        }
        if self.snapshot_wanted && self.pending.is_empty() {
            self.snapshot_wanted = false;
            events.push(Event::SnapshotWanted);
        }
        events
    }

    fn handle(&mut self, msg: ServerMsg, events: &mut Vec<Event>) {
        match msg {
            ServerMsg::Welcome { you, rev, peers } => {
                self.me = you;
                self.rev = rev;
                self.peers = peers.clone();
                events.push(Event::Peers(peers));
            }
            ServerMsg::Peers { peers } => {
                self.views
                    .retain(|id, _| peers.iter().any(|peer| peer.id == *id));
                self.peers = peers.clone();
                events.push(Event::Peers(peers));
            }
            ServerMsg::WantSnapshot => {
                if self.role == Role::Host {
                    self.snapshot_wanted = true;
                }
            }
            ServerMsg::Snapshot { rev, doc } => {
                if self.role == Role::Guest
                    && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(doc)
                {
                    self.rev = rev;
                    self.pending.clear();
                    self.loaded = true;
                    events.push(Event::Load(bytes));
                    // Alterações que chegaram antes do documento: só as posteriores a ele.
                    for early in std::mem::take(&mut self.early) {
                        let newer = match &early {
                            ServerMsg::Op { rev: r, .. } | ServerMsg::Replace { rev: r, .. } => {
                                *r > rev
                            }
                            _ => true,
                        };
                        if newer {
                            self.handle(early, events);
                        }
                    }
                }
            }
            msg @ (ServerMsg::Op { .. } | ServerMsg::Replace { .. }) if !self.loaded => {
                self.early.push(msg);
            }
            ServerMsg::Op {
                rev,
                author,
                local,
                cmd,
                params,
                ctx,
            } => {
                self.rev = rev;
                if author == self.me {
                    self.pending.retain(|l| *l != local);
                } else {
                    self.last_change = Some(Instant::now());
                    events.push(Event::Apply {
                        author,
                        cmd,
                        params,
                        ctx,
                    });
                }
            }
            ServerMsg::Replace {
                rev,
                author,
                local,
                doc,
            } => {
                self.rev = rev;
                if author == self.me {
                    self.pending.retain(|l| *l != local);
                } else if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(doc) {
                    self.last_change = Some(Instant::now());
                    events.push(Event::Load(bytes));
                }
            }
            ServerMsg::Digest { rev, digest } => {
                if self.role == Role::Guest {
                    self.check = Some((rev, digest));
                }
            }
            ServerMsg::Promoted => {
                if self.role == Role::Guest {
                    self.role = Role::Host;
                    self.loaded = true;
                    self.check = None;
                    self.digest_sent_for = 0;
                    events.push(Event::Promoted);
                }
            }
            ServerMsg::View {
                from,
                center,
                zoom,
                rotation,
            } => {
                if self.peers.iter().any(|p| p.id == from)
                    && center.iter().all(|n| n.is_finite() && n.abs() <= 1.0e9)
                    && zoom.is_finite()
                    && (0.0001..=10000.0).contains(&zoom)
                    && rotation.is_finite()
                    && rotation.abs() <= 360.0
                {
                    self.views.insert(from, (center, zoom, rotation));
                }
            }
            ServerMsg::Cursor { from, space, x, y } => self.cursor_seen(from, Some((space, x, y))),
            ServerMsg::CursorGone { from } => self.cursor_seen(from, None),
            ServerMsg::WantMedia { from, key } => events.push(Event::MediaWanted { from, key }),
            ServerMsg::MediaChunk {
                from,
                key,
                offset,
                total,
                data,
                digest,
            } => events.push(Event::MediaChunk {
                from,
                key,
                offset,
                total,
                data,
                digest,
            }),
            ServerMsg::Token { token } => {
                if valid_id(&token, 16, 128) {
                    self.invite.token = token;
                }
            }
            ServerMsg::Ended { reason } | ServerMsg::Refused { reason } => {
                self.ended = true;
                events.push(Event::Ended(reason));
            }
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.out.send(ClientMsg::Bye);
    }
}

#[cfg(test)]
mod tests;
