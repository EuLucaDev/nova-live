//! Uma thread por sessão: conecta (ws:// ou wss://), envia o Hello e depois alterna entre ler o
//! relay (com timeout curto) e escrever o que o app mandou. Sem runtime assíncrono.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

use crate::proto::{ClientMsg, MAX_MESSAGE, ServerMsg};
use crate::{Error, Result, err};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_SLICE: Duration = Duration::from_millis(40);
const PING_EVERY: Duration = Duration::from_secs(20);

struct Target {
    secure: bool,
    host: String,
    port: u16,
    url: String,
}

fn parse(server: &str) -> Result<Target> {
    let (secure, rest) = if let Some(r) = server.strip_prefix("wss://") {
        (true, r)
    } else if let Some(r) = server.strip_prefix("ws://") {
        (false, r)
    } else {
        return Err(err("o servidor precisa começar com wss:// ou ws://"));
    };
    let uri: tungstenite::http::Uri = server
        .parse()
        .map_err(|_| err("endereço inválido no servidor"))?;
    if rest.contains(['@', '?', '#']) || rest.chars().any(char::is_whitespace) {
        return Err(err("endereço inválido no servidor"));
    }
    let host = uri
        .host()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| err("servidor sem endereço"))?
        .trim_matches(['[', ']'])
        .to_string();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    if port == 0 {
        return Err(err("porta inválida no servidor"));
    }
    Ok(Target {
        secure,
        host,
        port,
        url: server.to_string(),
    })
}

pub(crate) fn validate_server(server: &str) -> Result<()> {
    parse(server).map(|_| ())
}

/// Fluxo TLS ou simples, com leitura e escrita pelo mesmo dono.
enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

fn connect(t: &Target) -> Result<WebSocket<Stream>> {
    let addr = (t.host.as_str(), t.port)
        .to_socket_addrs()
        .map_err(|e| err(format!("não encontrei o servidor {}: {e}", t.host)))?
        .next()
        .ok_or_else(|| err(format!("não encontrei o servidor {}", t.host)))?;
    let tcp = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| err(format!("sem conexão com {}: {e}", t.host)))?;
    let _ = tcp.set_nodelay(true);
    tcp.set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|e| err(e.to_string()))?;
    let stream = if t.secure {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| err(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(t.host.clone())
            .map_err(|_| err("nome de servidor inválido"))?;
        let conn = rustls::ClientConnection::new(Arc::new(config), name)
            .map_err(|e| err(e.to_string()))?;
        Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)))
    } else {
        Stream::Plain(tcp)
    };
    let mut cfg = WebSocketConfig::default();
    cfg.max_message_size = Some(MAX_MESSAGE);
    cfg.max_frame_size = Some(MAX_MESSAGE);
    let (ws, _) = tungstenite::client::client_with_config(t.url.as_str(), stream, Some(cfg))
        .map_err(|e| err(format!("o servidor recusou a conexão: {e}")))?;
    let tcp = match ws.get_ref() {
        Stream::Plain(s) => s,
        Stream::Tls(s) => s.get_ref(),
    };
    tcp.set_read_timeout(Some(READ_SLICE))
        .map_err(|e| err(e.to_string()))?;
    Ok(ws)
}

fn idle(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

/// Conecta na thread de rede e devolve (envio, recebimento). Falha de conexão é devolvida aqui.
pub(crate) fn spawn(
    server: &str,
    hello: ClientMsg,
) -> Result<(Sender<ClientMsg>, Receiver<ServerMsg>)> {
    let target = parse(server)?;
    let (out_tx, out_rx) = mpsc::channel::<ClientMsg>();
    let (in_tx, in_rx) = mpsc::channel::<ServerMsg>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
    std::thread::Builder::new()
        .name("nova-live".into())
        .spawn(move || {
            let mut ws = match connect(&target) {
                Ok(ws) => ws,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let first = serde_json::to_string(&hello).map_err(|e| err(e.to_string()));
            let sent =
                first.and_then(|t| ws.send(Message::text(t)).map_err(|e| err(e.to_string())));
            if let Err(e) = sent {
                let _ = ready_tx.send(Err(e));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            run(ws, out_rx, in_tx);
        })
        .map_err(|e| err(e.to_string()))?;
    match ready_rx.recv_timeout(CONNECT_TIMEOUT * 2) {
        Ok(Ok(())) => Ok((out_tx, in_rx)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(Error("o servidor não respondeu".into())),
    }
}

fn run(mut ws: WebSocket<Stream>, out: Receiver<ClientMsg>, inbox: Sender<ServerMsg>) {
    let mut last_ping = Instant::now();
    loop {
        // Escreve tudo o que o app mandou.
        loop {
            match out.try_recv() {
                Ok(ClientMsg::Bye) | Err(TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return;
                }
                Ok(msg) => {
                    let Ok(text) = serde_json::to_string(&msg) else {
                        continue;
                    };
                    if ws.write(Message::text(text)).is_err() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if last_ping.elapsed() > PING_EVERY {
            last_ping = Instant::now();
            let _ = ws.write(Message::Ping(Vec::new().into()));
        }
        match ws.flush() {
            Ok(()) => {}
            Err(e) if idle(&e) => {}
            Err(_) => return,
        }
        // Lê o que chegou (até o timeout curto).
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Ok(msg) = serde_json::from_str::<ServerMsg>(t.as_str())
                    && inbox.send(msg).is_err()
                {
                    return;
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(e) if idle(&e) => {}
            Err(_) => return,
        }
    }
}
