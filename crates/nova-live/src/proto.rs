//! Mensagens entre os apps e o relay (JSON em frames de texto WebSocket).

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// Versão do protocolo. 2: anfitrião passa adiante ao sair, encerrar para todos, trocar o convite,
/// troca de mídias (vídeos, áudios) entre os participantes.
pub const PROTOCOL: u32 = 2;
/// Versão mais antiga que o relay ainda aceita (apps não atualizados).
pub const PROTOCOL_MIN: u32 = 1;

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
    /// Saída limpa. Se for o anfitrião, a sessão continua com o próximo participante.
    Bye,
    /// Anfitrião: encerra a sessão para todos.
    End,
    /// Anfitrião: troca o segredo do convite; o link anterior deixa de valer para quem não entrou.
    NewToken { token: String },
    /// Pede a todos a mídia `key` (usada pelo documento e que falta aqui).
    WantMedia { key: String },
    /// Um pedaço da mídia `key` para quem pediu (`to`). `digest` vem no último pedaço.
    MediaChunk {
        to: String,
        key: String,
        offset: u64,
        total: u64,
        data: String,
        digest: Option<u64>,
    },
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
    /// Você passou a ser o anfitrião (o anterior saiu).
    Promoted,
    /// O segredo do convite mudou (o anfitrião gerou um convite novo).
    Token {
        token: String,
    },
    /// Alguém (`from`) precisa da mídia `key`.
    WantMedia {
        from: String,
        key: String,
    },
    /// Um pedaço da mídia `key`, enviado por `from`.
    MediaChunk {
        from: String,
        key: String,
        offset: u64,
        total: u64,
        data: String,
        digest: Option<u64>,
    },
}
