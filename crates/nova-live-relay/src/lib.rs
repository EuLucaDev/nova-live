//! nova-live-relay: o servidor das sessões Nova Live (live.agcunae.com).
//!
//! Uma sala por documento compartilhado. O anfitrião cria a sala com o segredo de convidado e o
//! próprio segredo; convidados entram com o segredo do convite. O relay numera cada alteração
//! (ordem total), repassa a todos, entrega o documento do anfitrião a quem entra e encerra a sala
//! quando o anfitrião sai. Não guarda documentos em disco nem tem catálogo de salas.
//! Sem limite de participantes; limite apenas de tamanho de mensagem.
//!
//! O binário (`main.rs`) só escuta e chama [`run`]. TLS fica no proxy (Traefik/Coolify).

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use nova_live::proto::{ClientMsg, MAX_MESSAGE, PROTOCOL, Peer, ServerMsg};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

type Tx = mpsc::UnboundedSender<ServerMsg>;

struct Member {
    peer: Peer,
    tx: Tx,
}

struct Room {
    app: String,
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
}

type Shared = Arc<Mutex<State>>;

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn clean_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    let n = n.trim().to_string();
    if n.is_empty() { "Convidado".into() } else { n }
}

/// Atende conexões para sempre.
pub async fn run(listener: TcpListener) {
    let state: Shared = Arc::default();
    loop {
        let Ok((stream, addr)) = listener.accept().await else {
            continue;
        };
        let state = state.clone();
        tokio::spawn(async move {
            serve(stream, addr, state).await;
        });
    }
}

/// `GET …/healthz` responde 200; o resto vira WebSocket.
async fn serve(stream: TcpStream, _addr: SocketAddr, state: Shared) {
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
    let Ok(ws) = tokio_tungstenite::accept_async_with_config(stream, Some(cfg)).await else {
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
                Ok(m @ ClientMsg::Hello { .. }) => break m,
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
    let ClientMsg::Hello {
        protocol,
        app,
        room,
        token,
        host_token,
        name,
    } = hello
    else {
        return;
    };
    if protocol != PROTOCOL {
        let _ = tx.send(ServerMsg::Refused {
            reason: "versão do app diferente da do servidor: atualize o app pelo launcher".into(),
        });
        return;
    }
    let me = match join(
        &state,
        &app,
        &room,
        &token,
        host_token.as_deref(),
        &clean_name(&name),
        tx.clone(),
    )
    .await
    {
        Ok(id) => id,
        Err(reason) => {
            let _ = tx.send(ServerMsg::Refused { reason });
            let _ = writer.await;
            return;
        }
    };

    while let Some(frame) = source.next().await {
        let text = match frame {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };
        let Ok(msg) = serde_json::from_str::<ClientMsg>(text.as_str()) else {
            continue;
        };
        let mut st = state.lock().await;
        let Some(r) = st.rooms.get_mut(&room) else {
            break;
        };
        let is_host = r.host.as_deref() == Some(me.as_str());
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
            ClientMsg::Bye => break,
            _ => {}
        }
    }
    leave(&state, &room, &me).await;
    drop(tx);
    let _ = writer.await;
}

async fn join(
    state: &Shared,
    app: &str,
    room: &str,
    token: &str,
    host_token: Option<&str>,
    name: &str,
    tx: Tx,
) -> Result<String, String> {
    if room.len() < 16 || room.len() > 64 || token.len() < 16 || token.len() > 128 {
        return Err("convite inválido".into());
    }
    let mut st = state.lock().await;
    st.next += 1;
    let id = format!("p{}", st.next);
    match host_token {
        Some(ht) => {
            if ht.len() < 16 {
                return Err("convite inválido".into());
            }
            let r = st.rooms.entry(room.to_string()).or_insert_with(|| Room {
                app: app.to_string(),
                token: token.to_string(),
                host_token: ht.to_string(),
                host: None,
                members: HashMap::new(),
                order: vec![],
                rev: 0,
                awaiting: HashSet::new(),
            });
            if !same(&r.host_token, ht) || r.app != app || r.host.is_some() {
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
                    },
                    tx,
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
                    },
                    tx,
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

async fn leave(state: &Shared, room: &str, me: &str) {
    let mut st = state.lock().await;
    let Some(r) = st.rooms.get_mut(room) else {
        return;
    };
    r.members.remove(me);
    r.order.retain(|x| x != me);
    r.awaiting.remove(me);
    if r.host.as_deref() == Some(me) {
        r.broadcast(&ServerMsg::Ended {
            reason: "O anfitrião encerrou a sessão.".into(),
        });
        st.rooms.remove(room);
    } else {
        r.broadcast(&ServerMsg::Peers { peers: r.peers() });
    }
}
