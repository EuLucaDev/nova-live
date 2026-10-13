//! Mensagens entre os apps e o relay (JSON em frames de texto WebSocket).

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// Versão do protocolo; o relay recusa outra.
pub const PROTOCOL: u32 = 1;

/// Maior mensagem aceita (um documento inteiro em base64 cabe nisso).
pub const MAX_MESSAGE: usize = 256 << 20;

/// Quem está na sessão.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub host: bool,
}

/// Do app para o relay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Primeira mensagem. Com `host_token`, cria a sala (anfitrião); sem, entra nela.
    Hello {
        protocol: u32,
        app: String,
        room: String,
        token: String,
        host_token: Option<String>,
        name: String,
    },
    /// Alteração por comando, já aplicada por quem enviou.
    Op {
        local: u64,
        cmd: String,
        params: Json,
        ctx: Json,
    },
    /// Documento inteiro (desfazer, refazer, mudanças que não são comandos).
    Replace { local: u64, doc: String },
    /// Anfitrião: documento para quem espera (entrada ou ressincronização).
    Snapshot { rev: u64, doc: String },
    /// Anfitrião: impressão digital na revisão `rev`.
    Digest { rev: u64, digest: u64 },
    /// Convidado: pede o documento ao anfitrião.
    WantSnapshot,
    /// Saída limpa.
    Bye,
}

/// Do relay para o app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        you: String,
        rev: u64,
        peers: Vec<Peer>,
    },
    Peers {
        peers: Vec<Peer>,
    },
    Op {
        rev: u64,
        author: String,
        local: u64,
        cmd: String,
        params: Json,
        ctx: Json,
    },
    Replace {
        rev: u64,
        author: String,
        local: u64,
        doc: String,
    },
    Snapshot {
        rev: u64,
        doc: String,
    },
    Digest {
        rev: u64,
        digest: u64,
    },
    /// Para o anfitrião: alguém precisa do documento.
    WantSnapshot,
    /// A sessão acabou (o anfitrião saiu ou encerrou).
    Ended {
        reason: String,
    },
    /// Entrada recusada (convite inválido, revogado, app errado…).
    Refused {
        reason: String,
    },
}
