//! nova-live-relay: o servidor das sessões Nova Live (live.agcunae.com).
//!
//! Uma sala por documento compartilhado. O anfitrião cria a sala com o segredo de convidado e o
//! próprio segredo; convidados entram com o segredo do convite. O relay numera cada alteração
//! (ordem total), repassa a todos e entrega o documento do anfitrião a quem entra. Quando o
//! anfitrião sai (ou cai), o próximo participante vira anfitrião e a sessão continua; só
//! "encerrar" (ou a saída do último) fecha a sala. O anfitrião pode trocar o segredo do convite.
//! Não guarda documentos em disco nem tem catálogo de salas.
//! Sem limite de participantes; limite apenas de tamanho de mensagem.
//!
//! O binário (`main.rs`) só escuta e chama [`run`]. TLS fica no proxy (Traefik/Coolify).

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nova_live::identity::{self, Profile};
use nova_live::proto::{ClientMsg, MAX_MESSAGE, PROTOCOL, PROTOCOL_MIN, Peer, ServerMsg};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

type Tx = mpsc::UnboundedSender<ServerMsg>;

struct Member {
    peer: Peer,
    tx: Tx,
    /// Versão do protocolo do app: só quem entende `Promoted` pode virar anfitrião.
    protocol: u32,
    can_edit: bool,
}

struct Room {
    app: String,
    organization_id: Option<String>,
    token: String,
    host_token: String,
    host: Option<String>,
    members: HashMap<String, Member>,
    order: Vec<String>,
    rev: u64,
    awaiting: HashSet<String>,
}

impl Room {
    fn peers(&self) -> Vec<Peer> {
        self.order
            .iter()
            .filter_map(|id| self.members.get(id).map(|m| m.peer.clone()))
            .collect()
    }
    fn broadcast(&self, msg: &ServerMsg) {
        for m in self.members.values() {
            let _ = m.tx.send(msg.clone());
        }
    }
    fn send_to(&self, id: &str, msg: ServerMsg) {
        if let Some(m) = self.members.get(id) {
            let _ = m.tx.send(msg);
        }
    }
}

#[derive(Default)]
struct State {
    rooms: HashMap<String, Room>,
    next: u64,
    document_key: Option<Vec<u8>>,
}

type Shared = Arc<Mutex<State>>;

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn valid_secret(s: &str) -> bool {
    (16..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn clean_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(160).collect();
    let n = n.trim().to_string();
    if n.is_empty() { "Convidado".into() } else { n }
}

/// Atende conexões para sempre.
pub async fn run(listener: TcpListener) {
    run_inner(listener, true).await;
}

/// Apenas builds de desenvolvimento podem hospedar testes locais sem conta.
#[cfg(debug_assertions)]
pub async fn run_local(listener: TcpListener) {
    if !listener.local_addr().is_ok_and(|a| a.ip().is_loopback()) {
        return;
    }
    run_inner(listener, false).await;
}

async fn run_inner(listener: TcpListener, account_required: bool) {
    let state: Shared = Arc::new(Mutex::new(State {
        document_key: nova_live::document_ticket::key_from_hex(
            &std::env::var("NOVA_DOCUMENT_PUBLIC_KEY_HEX")
                .unwrap_or_else(|_| nova_live::offline::NOVA_FOODS_SIGNING_PUBLIC_KEY_HEX.into()),
        ),
        ..State::default()
    }));
    loop {
        let Ok((stream, addr)) = listener.accept().await else {
            continue;
        };
        let state = state.clone();
        tokio::spawn(async move {
            serve(stream, addr, state, account_required).await;
        });
    }
}

/// `GET …/healthz` responde 200; o resto vira WebSocket.
async fn serve(stream: TcpStream, _addr: SocketAddr, state: Shared, account_required: bool) {
    let mut buf = [0u8; 512];
    let n = stream.peek(&mut buf).await.unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    let line = head.lines().next().unwrap_or_default().to_string();
    if line.starts_with("GET ")
        && line
            .split(' ')
            .nth(1)
            .is_some_and(|p| p.ends_with("/healthz"))
    {
        let mut stream = stream;
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 3\r\nconnection: close\r\n\r\nok\n").await;
        return;
    }
    let mut cfg = WebSocketConfig::default();
    cfg.max_message_size = Some(MAX_MESSAGE);
    cfg.max_frame_size = Some(MAX_MESSAGE);
    let mut account_token: Option<String> = None;
    // Tungstenite fixes this callback error type; boxing it changes the required trait.
    #[allow(clippy::result_large_err)]
    let capture_account =
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
         response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            account_token = request
                .headers()
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .filter(|v| v.len() == 43)
                .map(str::to_string);
            Ok(response)
        };
    let Ok(ws) =
        tokio_tungstenite::accept_hdr_async_with_config(stream, capture_account, Some(cfg)).await
    else {
        return;
    };
    let (mut sink, mut source) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let Ok(text) = serde_json::to_string(&msg) else {
                continue;
            };
            let end = matches!(msg, ServerMsg::Ended { .. } | ServerMsg::Refused { .. });
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
            if end {
                let _ = sink.close().await;
                break;
            }
        }
    });

    // Primeira mensagem: Hello.
    let hello = loop {
        match source.next().await {
            Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClientMsg>(t.as_str()) {
                Ok(m @ (ClientMsg::Hello { .. } | ClientMsg::DocumentHello { .. })) => break m,
                _ => {
                    let _ = tx.send(ServerMsg::Refused {
                        reason: "mensagem inicial inválida".into(),
                    });
                    return;
                }
            },
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => return,
        }
    };
    let document_key = state.lock().await.document_key.clone();
    let mut document_claims = None;
    let (protocol, app, room, token, host_token, name) = match hello {
        ClientMsg::Hello {
            protocol,
            app,
            room,
            token,
            host_token,
            name,
        } if !room.starts_with("doc-") => (protocol, app, room, token, host_token, name),
        ClientMsg::DocumentHello {
            protocol,
            payload,
            signature,
        } => {
            let checked = document_key.as_deref().and_then(|key| {
                nova_live::document_ticket::verify(
                    &payload,
                    &signature,
                    key,
                    identity::ORIGIN,
                    nova_live::document_ticket::now(),
                )
            });
            let Some(c) = checked else {
                let _ = tx.send(ServerMsg::Refused {
                    reason: "Autorização do documento inválida ou expirada.".into(),
                });
                return;
            };
            let values = (
                protocol,
                c.app_id.clone(),
                format!("doc-{}", c.document_id),
                String::new(),
                None,
                c.name.clone(),
            );
            document_claims = Some(c);
            values
        }
        _ => {
            let _ = tx.send(ServerMsg::Refused {
                reason: "Sala reservada para arquivos permanentes.".into(),
            });
            return;
        }
    };
    if !(PROTOCOL_MIN..=PROTOCOL).contains(&protocol) {
        let _ = tx.send(ServerMsg::Refused {
            reason: "versão do app diferente da do servidor: atualize o app pelo launcher".into(),
        });
        return;
    }
    let mut account = if account_required {
        let Some(token) = account_token.clone() else {
            let _ = tx.send(ServerMsg::Refused {
                reason: "Conecte sua conta Nova Foods pelo launcher para colaborar.".into(),
            });
            return;
        };
        match tokio::task::spawn_blocking(move || identity::validate_token(&token)).await {
            Ok(Ok(profile)) if profile.allows(&app) => Some(profile),
            _ => {
                let _ = tx.send(ServerMsg::Refused {
                    reason: "Sua conta não tem acesso a este aplicativo ou a conexão expirou."
                        .into(),
                });
                return;
            }
        }
    } else {
        None
    };
    if let Some(c) = &document_claims
        && account_required
        && !account
            .as_ref()
            .is_some_and(|p| p.user.id == c.user_id && p.organization.id == c.organization_id)
    {
        let _ = tx.send(ServerMsg::Refused {
            reason: "A conta não corresponde à autorização do documento.".into(),
        });
        return;
    }
    let joined = if let Some(c) = &document_claims {
        join_document(&state, c, tx.clone(), protocol).await
    } else {
        join(
            &state,
            &app,
            &room,
            &token,
            host_token.as_deref(),
            &clean_name(&name),
            account.as_ref(),
            tx.clone(),
            protocol,
        )
        .await
    };
    let me = match joined {
        Ok(id) => id,
        Err(reason) => {
            let _ = tx.send(ServerMsg::Refused { reason });
            let _ = writer.await;
            return;
        }
    };

    let mut lease = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(60),
        Duration::from_secs(60),
    );
    let mut expiry_tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        let frame = if account_required || document_claims.is_some() {
            tokio::select! {
                frame = source.next() => frame,
                _ = expiry_tick.tick(), if document_claims.is_some() => {
                    if document_claims.as_ref().is_some_and(|c| c.expires_at <= nova_live::document_ticket::now()) {
                        let _ = tx.send(ServerMsg::Refused { reason: "Autorização do documento expirada. Reconecte.".into() });
                        break;
                    }
                    continue;
                }
                _ = lease.tick(), if account_required => {
                    let token = account_token.clone().unwrap_or_default();
                    let checked = tokio::task::spawn_blocking(move || identity::validate_token(&token)).await;
                    let valid = match (&account, checked) {
                        (Some(previous), Ok(Ok(profile))) if profile.allows(&app) && profile.user.id == previous.user.id && profile.organization.id == previous.organization.id => Some(profile),
                        _ => None,
                    };
                    let Some(profile) = valid else {
                        let _ = tx.send(ServerMsg::Refused { reason: "Seu acesso mudou ou sua conta desconectou. Conecte novamente pelo launcher.".into() });
                        break;
                    };
                    let mut st = state.lock().await;
                    if let Some(room) = st.rooms.get_mut(&room) {
                        if let Some(member) = room.members.get_mut(&me) { member.peer.name = clean_name(&profile.user.name); }
                        room.broadcast(&ServerMsg::Peers { peers: room.peers() });
                    }
                    account = Some(profile);
                    continue;
                }
            }
        } else {
            source.next().await
        };
        let Some(frame) = frame else { break };
        let text = match frame {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };
        let Ok(msg) = serde_json::from_str::<ClientMsg>(text.as_str()) else {
            continue;
        };
        if let ClientMsg::RenewDocumentTicket { payload, signature } = &msg {
            let renewed = document_key.as_deref().and_then(|key| {
                nova_live::document_ticket::verify(
                    payload,
                    signature,
                    key,
                    identity::ORIGIN,
                    nova_live::document_ticket::now(),
                )
            });
            match (&document_claims, renewed) {
                (Some(old), Some(new))
                    if old.document_id == new.document_id
                        && old.user_id == new.user_id
                        && old.organization_id == new.organization_id
                        && old.role == new.role =>
                {
                    document_claims = Some(new)
                }
                _ => {
                    let _ = tx.send(ServerMsg::Refused {
                        reason: "Seu acesso ao documento mudou. Reconecte.".into(),
                    });
                    break;
                }
            }
            continue;
        }
        if document_claims
            .as_ref()
            .is_some_and(|c| c.expires_at <= nova_live::document_ticket::now())
        {
            break;
        }
        let mut st = state.lock().await;
        let Some(r) = st.rooms.get_mut(&room) else {
            break;
        };
        let is_host = r.host.as_deref() == Some(me.as_str());
        if !r.members.get(&me).is_some_and(|m| m.can_edit)
            && !matches!(
                &msg,
                ClientMsg::View { .. }
                    | ClientMsg::WantSnapshot
                    | ClientMsg::WantMedia { .. }
                    | ClientMsg::Cursor { .. }
                    | ClientMsg::CursorGone
                    | ClientMsg::Bye
            )
        {
            let _ = tx.send(ServerMsg::Refused {
                reason: "Este documento está disponível somente para leitura.".into(),
            });
            break;
        }
        match msg {
            ClientMsg::Op {
                local,
                cmd,
                params,
                ctx,
            } => {
                r.rev += 1;
                let m = ServerMsg::Op {
                    rev: r.rev,
                    author: me.clone(),
                    local,
                    cmd,
                    params,
                    ctx,
                };
                r.broadcast(&m);
            }
            ClientMsg::Replace { local, doc } => {
                r.rev += 1;
                let m = ServerMsg::Replace {
                    rev: r.rev,
                    author: me.clone(),
                    local,
                    doc,
                };
                r.broadcast(&m);
            }
            ClientMsg::Snapshot { rev, doc } if is_host => {
                let rev = rev.min(r.rev);
                for id in std::mem::take(&mut r.awaiting) {
                    r.send_to(
                        &id,
                        ServerMsg::Snapshot {
                            rev,
                            doc: doc.clone(),
                        },
                    );
                }
            }
            ClientMsg::Digest { rev, digest } if is_host => {
                for (id, m) in &r.members {
                    if *id != me {
                        let _ = m.tx.send(ServerMsg::Digest { rev, digest });
                    }
                }
            }
            ClientMsg::WantSnapshot if !is_host => {
                r.awaiting.insert(me.clone());
                if let Some(h) = r.host.clone() {
                    r.send_to(&h, ServerMsg::WantSnapshot);
                }
            }
            ClientMsg::End if is_host => {
                let who = r
                    .members
                    .get(&me)
                    .map(|m| m.peer.name.clone())
                    .unwrap_or_default();
                r.broadcast(&ServerMsg::Ended {
                    reason: format!("{who} encerrou a sessão."),
                });
                st.rooms.remove(&room);
                break;
            }
            ClientMsg::NewToken { token } if is_host => {
                if valid_secret(&token) {
                    r.token = token.clone();
                    r.broadcast(&ServerMsg::Token { token });
                }
            }
            ClientMsg::View {
                center,
                zoom,
                rotation,
            } => {
                if center.iter().all(|n| n.is_finite() && n.abs() <= 1.0e9)
                    && zoom.is_finite()
                    && (0.0001..=10000.0).contains(&zoom)
                    && rotation.is_finite()
                    && rotation.abs() <= 360.0
                {
                    for (id, m) in &r.members {
                        if *id != me {
                            let _ = m.tx.send(ServerMsg::View {
                                from: me.clone(),
                                center,
                                zoom,
                                rotation,
                            });
                        }
                    }
                }
            }
            ClientMsg::Cursor { space, x, y } => {
                if space.len() <= 256 && x.is_finite() && y.is_finite() {
                    let m = ServerMsg::Cursor {
                        from: me.clone(),
                        space,
                        x,
                        y,
                    };
                    for (id, member) in &r.members {
                        if *id != me {
                            let _ = member.tx.send(m.clone());
                        }
                    }
                }
            }
            ClientMsg::CursorGone => {
                let m = ServerMsg::CursorGone { from: me.clone() };
                for (id, member) in &r.members {
                    if *id != me {
                        let _ = member.tx.send(m.clone());
                    }
                }
            }
            ClientMsg::WantMedia { key } => {
                if key.len() <= 4096 {
                    for (id, m) in &r.members {
                        if *id != me && m.protocol >= 2 {
                            let _ = m.tx.send(ServerMsg::WantMedia {
                                from: me.clone(),
                                key: key.clone(),
                            });
                        }
                    }
                }
            }
            ClientMsg::MediaChunk {
                to,
                key,
                offset,
                total,
                data,
                digest,
            } => {
                if to != me {
                    r.send_to(
                        &to,
                        ServerMsg::MediaChunk {
                            from: me.clone(),
                            key,
                            offset,
                            total,
                            data,
                            digest,
                        },
                    );
                }
            }
            ClientMsg::Bye => break,
            _ => {}
        }
    }
    leave(&state, &room, &me).await;
    drop(tx);
    let _ = writer.await;
}

#[allow(clippy::too_many_arguments)]
async fn join(
    state: &Shared,
    app: &str,
    room: &str,
    token: &str,
    host_token: Option<&str>,
    name: &str,
    account: Option<&Profile>,
    tx: Tx,
    protocol: u32,
) -> Result<String, String> {
    if room.len() < 16 || room.len() > 64 || token.len() < 16 || token.len() > 128 {
        return Err("convite inválido".into());
    }
    if account.is_some_and(|profile| !profile.allows(app)) {
        return Err("Aplicativo não liberado para esta conta.".into());
    }
    let mut st = state.lock().await;
    let organization_id = account.map(|p| p.organization.id.clone());
    let user_id = account.map(|p| p.user.id.clone());
    let name = account
        .map(|p| clean_name(&p.user.name))
        .unwrap_or_else(|| clean_name(name));
    st.next += 1;
    let id = format!("p{}", st.next);
    match host_token {
        Some(ht) => {
            if ht.len() < 16 {
                return Err("convite inválido".into());
            }
            let r = st.rooms.entry(room.to_string()).or_insert_with(|| Room {
                app: app.to_string(),
                organization_id: organization_id.clone(),
                token: token.to_string(),
                host_token: ht.to_string(),
                host: None,
                members: HashMap::new(),
                order: vec![],
                rev: 0,
                awaiting: HashSet::new(),
            });
            if !same(&r.host_token, ht)
                || r.app != app
                || r.host.is_some()
                || r.organization_id != organization_id
            {
                return Err("esta sessão já tem anfitrião".into());
            }
            r.host = Some(id.clone());
            r.members.insert(
                id.clone(),
                Member {
                    peer: Peer {
                        id: id.clone(),
                        name: name.to_string(),
                        host: true,
                        user_id: user_id.clone(),
                    },
                    tx,
                    protocol,
                    can_edit: true,
                },
            );
            r.order.push(id.clone());
            let _ = r.members[&id].tx.send(ServerMsg::Welcome {
                you: id.clone(),
                rev: r.rev,
                peers: r.peers(),
            });
        }
        None => {
            let Some(r) = st.rooms.get_mut(room) else {
                return Err("esta sessão terminou ou o convite foi revogado".into());
            };
            if r.app != app {
                return Err(format!("este convite é do app {}", r.app));
            }
            if r.organization_id != organization_id {
                return Err("Este convite pertence a outra organização.".into());
            }
            if !same(&r.token, token) {
                return Err("convite inválido".into());
            }
            r.members.insert(
                id.clone(),
                Member {
                    peer: Peer {
                        id: id.clone(),
                        name: name.to_string(),
                        host: false,
                        user_id,
                    },
                    tx,
                    protocol,
                    can_edit: true,
                },
            );
            r.order.push(id.clone());
            r.awaiting.insert(id.clone());
            r.send_to(
                &id,
                ServerMsg::Welcome {
                    you: id.clone(),
                    rev: r.rev,
                    peers: r.peers(),
                },
            );
            if let Some(h) = r.host.clone() {
                r.send_to(&h, ServerMsg::WantSnapshot);
            }
        }
    }
    if let Some(r) = st.rooms.get(room) {
        r.broadcast(&ServerMsg::Peers { peers: r.peers() });
    }
    Ok(id)
}

/// Document rooms have no invitation secret. Only a signed, person-bound grant admits a peer.
async fn join_document(
    state: &Shared,
    c: &nova_live::document_ticket::Claims,
    tx: Tx,
    protocol: u32,
) -> Result<String, String> {
    let mut st = state.lock().await;
    st.next += 1;
    let id = format!("p{}", st.next);
    let room = format!("doc-{}", c.document_id);
    let r = st.rooms.entry(room).or_insert_with(|| Room {
        app: c.app_id.clone(),
        organization_id: Some(c.organization_id.clone()),
        token: String::new(),
        host_token: String::new(),
        host: None,
        members: HashMap::new(),
        order: vec![],
        rev: 0,
        awaiting: HashSet::new(),
    });
    if r.organization_id.as_deref() != Some(&c.organization_id) || r.app != c.app_id {
        return Err("Documento de outra organização.".into());
    }
    let can_edit = c.role != "viewer";
    let promoted = can_edit && r.host.is_none();
    if promoted {
        r.host = Some(id.clone());
    }
    r.members.insert(
        id.clone(),
        Member {
            peer: Peer {
                id: id.clone(),
                name: clean_name(&c.name),
                host: promoted,
                user_id: Some(c.user_id.clone()),
            },
            tx,
            protocol,
            can_edit,
        },
    );
    r.order.push(id.clone());
    r.send_to(
        &id,
        ServerMsg::Welcome {
            you: id.clone(),
            rev: r.rev,
            peers: r.peers(),
        },
    );
    if promoted {
        r.send_to(&id, ServerMsg::Promoted);
        if !r.awaiting.is_empty() {
            r.send_to(&id, ServerMsg::WantSnapshot);
        }
    } else {
        r.awaiting.insert(id.clone());
        if let Some(h) = &r.host {
            r.send_to(h, ServerMsg::WantSnapshot);
        }
    }
    r.broadcast(&ServerMsg::Peers { peers: r.peers() });
    Ok(id)
}

async fn leave(state: &Shared, room: &str, me: &str) {
    let mut st = state.lock().await;
    let Some(r) = st.rooms.get_mut(room) else {
        return;
    };
    let name = r
        .members
        .get(me)
        .map(|m| m.peer.name.clone())
        .unwrap_or_default();
    r.members.remove(me);
    r.order.retain(|x| x != me);
    r.awaiting.remove(me);
    if r.host.as_deref() != Some(me) {
        r.broadcast(&ServerMsg::Peers { peers: r.peers() });
        return;
    }
    // O anfitrião saiu: o participante mais antigo que entende o protocolo novo assume.
    let next = r
        .order
        .iter()
        .find(|id| {
            r.members
                .get(*id)
                .is_some_and(|m| m.protocol >= 2 && m.can_edit)
        })
        .cloned();
    match next {
        Some(id) => {
            r.host = Some(id.clone());
            if let Some(m) = r.members.get_mut(&id) {
                m.peer.host = true;
            }
            r.awaiting.remove(&id);
            r.send_to(&id, ServerMsg::Promoted);
            if !r.awaiting.is_empty() {
                r.send_to(&id, ServerMsg::WantSnapshot);
            }
            r.broadcast(&ServerMsg::Peers { peers: r.peers() });
        }
        None if room.starts_with("doc-") && !r.members.is_empty() => {
            r.host = None;
            r.awaiting.extend(r.order.iter().cloned());
            r.broadcast(&ServerMsg::Peers { peers: r.peers() });
        }
        None => {
            r.broadcast(&ServerMsg::Ended {
                reason: format!("{name} saiu e a sessão terminou."),
            });
            st.rooms.remove(room);
        }
    }
}

#[cfg(test)]
mod account_tests {
    use super::*;
    fn profile(org: &str, apps: &[&str]) -> Profile {
        serde_json::from_value(serde_json::json!({
            "user": {"id":"person-1", "name":"Ana Oliveira", "avatarUrl":null},
            "organization":{"id":org,"name":"Nova Foods"}, "departments":[],
            "apps":apps, "policyEnabled":true,"canManage":false
        }))
        .unwrap()
    }
    fn claims(role: &str) -> nova_live::document_ticket::Claims {
        let now = nova_live::document_ticket::now();
        nova_live::document_ticket::Claims {
            purpose: "native-document-session".into(),
            origin: identity::ORIGIN.into(),
            document_id: "00000000-0000-4000-8000-000000000001".into(),
            organization_id: "00000000-0000-4000-8000-000000000002".into(),
            app_id: "novaphoto".into(),
            user_id: "00000000-0000-4000-8000-000000000003".into(),
            name: "Test".into(),
            role: role.into(),
            issued_at: now,
            expires_at: now + 300,
        }
    }
    #[tokio::test]
    async fn persistent_coordinator_is_never_a_reader_and_survives_editor_departure() {
        let state: Shared = Arc::default();
        let (tx, _rx) = mpsc::unbounded_channel();
        let viewer = join_document(&state, &claims("viewer"), tx, PROTOCOL)
            .await
            .unwrap();
        let room = format!("doc-{}", claims("viewer").document_id);
        assert!(state.lock().await.rooms[&room].host.is_none());
        let (tx, _rx) = mpsc::unbounded_channel();
        let editor = join_document(&state, &claims("editor"), tx, PROTOCOL)
            .await
            .unwrap();
        assert_eq!(
            state.lock().await.rooms[&room].host.as_deref(),
            Some(editor.as_str())
        );
        leave(&state, &room, &editor).await;
        let locked = state.lock().await;
        assert!(locked.rooms[&room].host.is_none());
        assert!(!locked.rooms[&room].members[&viewer].can_edit);
        drop(locked);
        let mut outsider = claims("editor");
        outsider.organization_id = "00000000-0000-4000-8000-000000000004".into();
        let (tx, _rx) = mpsc::unbounded_channel();
        assert!(
            join_document(&state, &outsider, tx, PROTOCOL)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn signed_reader_cannot_mutate_over_the_wire() {
        use base64::Engine as _;
        use ring::signature::KeyPair;
        let key = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[9; 32]).unwrap();
        let state = Arc::new(Mutex::new(State {
            document_key: Some(key.public_key().as_ref().to_vec()),
            ..State::default()
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, addr) = listener.accept().await.unwrap();
            serve(stream, addr, state, false).await;
        });
        let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
            .await
            .unwrap();
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims("viewer")).unwrap());
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(key.sign(payload.as_bytes()).as_ref());
        client
            .send(Message::text(
                serde_json::to_string(&ClientMsg::DocumentHello {
                    protocol: PROTOCOL,
                    payload,
                    signature,
                })
                .unwrap(),
            ))
            .await
            .unwrap();
        client
            .send(Message::text(
                serde_json::to_string(&ClientMsg::Replace {
                    local: 1,
                    doc: "malicious".into(),
                })
                .unwrap(),
            ))
            .await
            .unwrap();
        let refused = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(frame)) = client.next().await {
                if let Ok(text) = frame.to_text() {
                    let msg: ServerMsg = serde_json::from_str(text).unwrap();
                    assert!(!matches!(
                        msg,
                        ServerMsg::Replace { .. } | ServerMsg::Op { .. }
                    ));
                    if matches!(msg, ServerMsg::Refused { .. }) {
                        return true;
                    }
                }
            }
            false
        })
        .await
        .unwrap();
        assert!(refused);
        server.abort();
    }
    #[tokio::test]
    async fn strict_relay_refuses_unsigned_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(run(listener));
        let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
            .await
            .unwrap();
        let hello = ClientMsg::Hello {
            protocol: PROTOCOL,
            app: "novaphoto".into(),
            room: "room-123456789012".into(),
            token: "token-123456789012".into(),
            host_token: Some("host-123456789012".into()),
            name: "LPC".into(),
        };
        client
            .send(Message::text(serde_json::to_string(&hello).unwrap()))
            .await
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let message: ServerMsg = serde_json::from_str(response.to_text().unwrap()).unwrap();
        assert!(matches!(message, ServerMsg::Refused { .. }));
        server.abort();
    }
    #[tokio::test]
    async fn canonical_identity_and_organization_are_enforced() {
        let state: Shared = Arc::default();
        let (tx, _rx) = mpsc::unbounded_channel();
        let person = profile("org-1", &["novaphoto"]);
        let id = join(
            &state,
            "novaphoto",
            "room-123456789012",
            "token-123456789012",
            Some("host-123456789012"),
            "LPC",
            Some(&person),
            tx,
            PROTOCOL,
        )
        .await
        .unwrap();
        {
            let locked = state.lock().await;
            let peer = &locked.rooms["room-123456789012"].members[&id].peer;
            assert_eq!(peer.name, "Ana Oliveira");
            assert_eq!(peer.user_id.as_deref(), Some("person-1"));
        }
        let (tx, _rx) = mpsc::unbounded_channel();
        let outsider = profile("org-2", &["novaphoto"]);
        assert!(
            join(
                &state,
                "novaphoto",
                "room-123456789012",
                "token-123456789012",
                None,
                "Ana",
                Some(&outsider),
                tx,
                PROTOCOL
            )
            .await
            .is_err()
        );
        let (tx, _rx) = mpsc::unbounded_channel();
        let denied = profile("org-1", &["novaword"]);
        assert!(
            join(
                &state,
                "novaphoto",
                "room-123456789012",
                "token-123456789012",
                None,
                "Ana",
                Some(&denied),
                tx,
                PROTOCOL
            )
            .await
            .is_err()
        );
    }
}
