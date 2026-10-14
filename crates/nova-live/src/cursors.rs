//! Mouses dos colegas no documento, com nome e cor (como no Nova Fig).
//!
//! Cada app diz onde o mouse está em coordenadas do **documento** (não da tela), junto com o
//! "espaço" em que a pessoa está olhando (página, aba, sequência…). Quem recebe converte para a
//! própria tela, com o próprio zoom e rolagem, e só mostra quem está no mesmo espaço.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::Live;
use crate::proto::ClientMsg;

/// Intervalo mínimo entre dois envios do mouse.
const SEND_EVERY: Duration = Duration::from_millis(40);
/// Sem notícia por tanto tempo, o mouse do colega some (ele pode ter trocado de janela).
const STALE: Duration = Duration::from_secs(30);

/// Cores dos colegas (fundo da etiqueta e da seta), legíveis com texto branco.
const PALETTE: [[u8; 3]; 10] = [
    [0xE5, 0x3E, 0xB0],
    [0xF2, 0x9D, 0x0C],
    [0x00, 0x6D, 0xFF],
    [0x0B, 0x8A, 0x4A],
    [0x7A, 0x3B, 0xF0],
    [0xD6, 0x28, 0x39],
    [0x00, 0x9E, 0xA8],
    [0xB5, 0x5A, 0x00],
    [0x3E, 0x4C, 0xC9],
    [0x9C, 0x27, 0x8B],
];

/// A cor de uma pessoa (sempre a mesma para o mesmo nome).
pub fn color_for(name: &str) -> [u8; 3] {
    let h = crate::digest(name.to_lowercase().as_bytes());
    PALETTE[(h % PALETTE.len() as u64) as usize]
}

/// O mouse de um colega.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteCursor {
    pub peer: String,
    pub name: String,
    pub color: [u8; 3],
    pub space: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Default)]
pub(crate) struct CursorState {
    mine: Option<(String, f64, f64)>,
    sent: Option<(String, f64, f64)>,
    last_send: Option<Instant>,
    theirs: HashMap<String, (String, f64, f64, Instant)>,
}

impl Live {
    /// Onde está o mouse desta pessoa (`None`: fora do documento). Chame a cada quadro; o envio
    /// é limitado e só acontece quando muda.
    pub fn set_cursor(&mut self, cursor: Option<(String, f64, f64)>) {
        self.cursors.mine = cursor.filter(|(_, x, y)| x.is_finite() && y.is_finite());
    }

    pub(crate) fn cursor_flush(&mut self) {
        if self.cursors.mine == self.cursors.sent
            || self
                .cursors
                .last_send
                .is_some_and(|t| t.elapsed() < SEND_EVERY)
        {
            return;
        }
        let msg = match &self.cursors.mine {
            Some((space, x, y)) => ClientMsg::Cursor {
                space: space.clone(),
                x: *x,
                y: *y,
            },
            None => ClientMsg::CursorGone,
        };
        self.send(msg);
        self.cursors.sent = self.cursors.mine.clone();
        self.cursors.last_send = Some(Instant::now());
    }

    pub(crate) fn cursor_seen(&mut self, from: String, pos: Option<(String, f64, f64)>) {
        match pos {
            Some((space, x, y)) => {
                self.cursors
                    .theirs
                    .insert(from, (space, x, y, Instant::now()));
            }
            None => {
                self.cursors.theirs.remove(&from);
            }
        }
    }

    /// Os mouses dos colegas que estão na sessão (com nome e cor).
    pub fn cursors(&self) -> Vec<RemoteCursor> {
        let mut out: Vec<RemoteCursor> = self
            .cursors
            .theirs
            .iter()
            .filter(|(_, (_, _, _, t))| t.elapsed() < STALE)
            .filter_map(|(id, (space, x, y, _))| {
                let peer = self.peers().iter().find(|p| p.id == *id)?;
                Some(RemoteCursor {
                    peer: id.clone(),
                    name: peer.name.clone(),
                    color: color_for(&peer.name),
                    space: space.clone(),
                    x: *x,
                    y: *y,
                })
            })
            .collect();
        out.sort_by(|a, b| a.peer.cmp(&b.peer));
        out
    }
}
