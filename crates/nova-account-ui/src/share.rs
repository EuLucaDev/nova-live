//! A janela Compartilhar e o botão da barra, no mesmo desenho em todos os apps: fotos redondas
//! empilhadas, nomes reais, quem coordena, link de convite com Copiar e as saídas no rodapé.
//! As cores vêm do tema do app (`ui.visuals()`); só a ação principal usa o azul Nova Foods.

use egui::{
    Align, Align2, Color32, Context, CornerRadius, CursorIcon, FontId, Frame, Id, Layout, Margin,
    Rect, Response, RichText, Sense, Stroke, Ui, Vec2, vec2,
};
use nova_live::identity::{self, Status};
use nova_live::{Live, Peer, Role};

use crate::native::avatar;

const BLUE: Color32 = Color32::from_rgb(0, 109, 255);
const WIDTH: f32 = 384.0;
const ROW: f32 = 46.0;

/// O que a pessoa pediu na janela; o app executa como já fazia.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareAction {
    /// Começar a compartilhar este arquivo.
    Host,
    /// Entrar com o convite colado.
    Join,
    /// Sair (a sessão continua para os outros).
    Stop,
    /// Encerrar para todos.
    EndAll,
    /// Gerar um convite novo (o anterior deixa de valer).
    Renew,
    /// O link foi copiado.
    Copied,
}

/// Uma transferência de mídia em andamento (vídeo, áudio).
pub struct MediaLine {
    pub name: String,
    pub incoming: bool,
    pub done: u64,
    pub total: u64,
}

/// Tudo o que a janela mostra.
pub struct ShareView<'a> {
    /// Id do app (`novaword`…), para a conta e o nome do app.
    pub app: &'a str,
    /// "este documento", "esta imagem"…
    pub this_doc: &'a str,
    pub live: Option<&'a Live>,
    pub peers: &'a [Peer],
    pub invite_text: &'a mut String,
    pub notice: Option<&'a str>,
    pub error: Option<&'a str>,
    pub media: Vec<MediaLine>,
}

/// Desenha a janela enquanto `open` for verdadeiro.
pub fn share_window(ctx: &Context, open: &mut bool, view: ShareView<'_>) -> Option<ShareAction> {
    if !*open {
        return None;
    }
    let mut action = None;
    let style = ctx.global_style();
    let frame = Frame::window(&style)
        .inner_margin(Margin::same(18))
        .corner_radius(CornerRadius::same(16));
    egui::Window::new("nova-share")
        .id(Id::new("nova-share-window"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_size(vec2(WIDTH, 0.0))
        .anchor(Align2::RIGHT_TOP, vec2(-16.0, 56.0))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            header(ui, &view, open);
            ui.add_space(10.0);
            account_row(ui, view.app);
            divider(ui);
            match view.live {
                None => action = invite_or_join(ui, view.this_doc, view.invite_text),
                Some(live) => {
                    let host = live.role() == Role::Host;
                    if host {
                        action = link_box(ui, live, view.app);
                        divider(ui);
                    } else {
                        ui.label(
                            RichText::new(format!(
                                "Você está editando {} com outras pessoas.",
                                view.this_doc
                            ))
                            .color(weak(ui)),
                        );
                        ui.add_space(4.0);
                    }
                    people(ui, view.peers);
                    for m in &view.media {
                        media_line(ui, m);
                    }
                    divider(ui);
                    if let Some(a) = footer(ui, host) {
                        action = Some(a);
                    }
                }
            }
            if let Some(note) = view.notice {
                ui.add_space(6.0);
                ui.label(RichText::new(note).size(12.0).color(weak(ui)));
            }
            if let Some(e) = view.error {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(e)
                        .size(12.0)
                        .color(Color32::from_rgb(0xd6, 0x28, 0x39)),
                );
            }
        });
    action
}

/// O botão da barra: as fotos de quem está no arquivo, empilhadas, e o "Compartilhar" azul.
pub fn share_button(ui: &mut Ui, people: &[crate::Presence], active: bool) -> Response {
    let me = identity::profile();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let mut faces: Vec<(Option<String>, String)> = Vec::new();
        if let Some(me) = &me {
            faces.push((Some(me.user.id.clone()), me.user.name.clone()));
        }
        for p in people {
            let mine = me
                .as_ref()
                .is_some_and(|m| p.user_id.as_deref() == Some(m.user.id.as_str()));
            if !mine {
                faces.push((p.user_id.clone(), p.name.clone()));
            }
        }
        let label = if active {
            "Compartilhando"
        } else {
            "Compartilhar"
        };
        let hint = if active {
            "Pessoas e convite deste arquivo"
        } else {
            "Convidar pessoas para editar este arquivo"
        };
        let faces_shown = active || faces.len() > 1;
        let size = vec2(if active { 128.0 } else { 116.0 }, 30.0);
        // Na barra montada da direita para a esquerda, o primeiro item fica mais à direita:
        // o botão vem antes, para as fotos ficarem à esquerda dele, como no Figma.
        if ui.layout().prefer_right_to_left() {
            let button = pill(ui, label, size, BLUE, Color32::WHITE, true).on_hover_text(hint);
            if faces_shown {
                stack(ui, &faces, 26.0);
            }
            button
        } else {
            if faces_shown {
                stack(ui, &faces, 26.0);
            }
            pill(ui, label, size, BLUE, Color32::WHITE, true).on_hover_text(hint)
        }
    })
    .inner
}

/// Fotos sobrepostas com um anel da cor do fundo, até três, e "+N" para o resto.
fn stack(ui: &mut Ui, faces: &[(Option<String>, String)], size: f32) {
    let shown = faces.len().min(3);
    let extra = faces.len().saturating_sub(3);
    let step = size * 0.68;
    let width = size + step * (shown.saturating_sub(1) + usize::from(extra > 0)) as f32;
    let (rect, response) = ui.allocate_exact_size(vec2(width, size), Sense::hover());
    let ring = ui.visuals().panel_fill;
    let mut names = Vec::new();
    for (i, (id, name)) in faces.iter().take(shown).enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(step * i as f32, 0.0), Vec2::splat(size));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        avatar(&mut child, id.as_deref(), name, size);
        ui.painter()
            .circle_stroke(r.center(), size * 0.5 + 0.5, Stroke::new(2.0, ring));
        names.push(name.as_str());
    }
    if extra > 0 {
        let c = rect.min + vec2(step * shown as f32 + size * 0.5, size * 0.5);
        ui.painter()
            .circle_filled(c, size * 0.5, ui.visuals().widgets.inactive.bg_fill);
        ui.painter()
            .circle_stroke(c, size * 0.5 + 0.5, Stroke::new(2.0, ring));
        ui.painter().text(
            c,
            Align2::CENTER_CENTER,
            format!("+{extra}"),
            FontId::proportional(size * 0.4),
            ui.visuals().text_color(),
        );
    }
    if !names.is_empty() {
        let mut all: Vec<&str> = faces.iter().map(|(_, n)| n.as_str()).collect();
        all.dedup();
        response.on_hover_text(all.join("\n"));
    }
}

fn header(ui: &mut Ui, view: &ShareView<'_>, open: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Compartilhar").size(16.0).strong());
        ui.label(
            RichText::new(crate::app_name(view.app))
                .size(13.0)
                .color(weak(ui)),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (rect, r) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
            if r.hovered() {
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(8),
                    ui.visuals().widgets.hovered.weak_bg_fill,
                );
            }
            let c = rect.center();
            let s = 4.5;
            let stroke = Stroke::new(1.6, weak(ui));
            ui.painter()
                .line_segment([c - vec2(s, s), c + vec2(s, s)], stroke);
            ui.painter()
                .line_segment([c - vec2(s, -s), c + vec2(s, -s)], stroke);
            if r.on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Fechar")
                .clicked()
            {
                *open = false;
            }
        });
    });
}

/// A conta conectada, compacta: foto, nome, setores e Trocar conta.
fn account_row(ui: &mut Ui, app: &str) {
    match identity::status(app) {
        Status::Verified(profile) => {
            let sectors = profile
                .departments
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            let sub = if sectors.is_empty() {
                profile.organization.name.clone()
            } else {
                sectors
            };
            person(
                ui,
                Some(&profile.user.id),
                &profile.user.name,
                &sub,
                None,
                34.0,
                |ui| {
                    if link(ui, "Trocar conta").clicked() {
                        identity::disconnect();
                        identity::connect(app);
                    }
                },
            );
        }
        _ => {
            ui.label(
                RichText::new("Conecte sua conta Nova Foods para colaborar com o seu nome e foto.")
                    .color(weak(ui)),
            );
            ui.add_space(6.0);
            if pill(
                ui,
                "Conectar minha conta",
                vec2(WIDTH, 36.0),
                BLUE,
                Color32::WHITE,
                true,
            )
            .clicked()
            {
                identity::connect(app);
            }
        }
    }
}

fn invite_or_join(ui: &mut Ui, this_doc: &str, invite_text: &mut String) -> Option<ShareAction> {
    let mut action = None;
    ui.label(
        RichText::new(format!(
            "Convide pessoas da sua empresa para editar {this_doc} ao vivo."
        ))
        .color(weak(ui)),
    );
    ui.add_space(8.0);
    if pill(
        ui,
        "Convidar pessoas",
        vec2(WIDTH, 38.0),
        BLUE,
        Color32::WHITE,
        true,
    )
    .clicked()
    {
        action = Some(ShareAction::Host);
    }
    divider(ui);
    ui.label(RichText::new("Recebeu um convite?").strong());
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        field(ui, invite_text, WIDTH - 84.0, "Cole o link do convite");
        if pill(
            ui,
            "Entrar",
            vec2(76.0, 32.0),
            secondary(ui),
            ui.visuals().text_color(),
            !invite_text.trim().is_empty(),
        )
        .clicked()
        {
            action = Some(ShareAction::Join);
        }
    });
    action
}

/// Link de convite com Copiar link e, discreto, Gerar novo link.
fn link_box(ui: &mut Ui, live: &Live, app: &str) -> Option<ShareAction> {
    let mut action = None;
    let url = live.invite().to_share_link();
    let ready = !live.peers().is_empty();
    ui.label(RichText::new("Link de convite").strong());
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let mut shown = url.clone();
        field(ui, &mut shown, WIDTH - 112.0, "");
        if pill(
            ui,
            "Copiar link",
            vec2(104.0, 32.0),
            BLUE,
            Color32::WHITE,
            ready,
        )
        .clicked()
        {
            ui.ctx().copy_text(url.clone());
            action = Some(ShareAction::Copied);
        }
    });
    ui.horizontal(|ui| {
        if ready {
            ui.label(
                RichText::new(format!(
                    "Vale só para este arquivo. Quem não tiver o {} instala pelo launcher.",
                    crate::app_name(app)
                ))
                .size(11.5)
                .color(weak(ui)),
            );
        } else {
            ui.spinner();
            ui.label(
                RichText::new("Conectando à colaboração…")
                    .size(12.0)
                    .color(weak(ui)),
            );
        }
    });
    if ready
        && link(ui, "Gerar novo link")
            .on_hover_text("O link anterior deixa de funcionar; quem já entrou continua")
            .clicked()
    {
        action = Some(ShareAction::Renew);
    }
    action
}

fn people(ui: &mut Ui, peers: &[Peer]) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Pessoas neste arquivo").strong());
        ui.label(RichText::new(peers.len().to_string()).color(weak(ui)));
    });
    ui.add_space(2.0);
    let me = identity::profile().map(|p| p.user.id);
    for p in peers {
        let mine = me.is_some() && p.user_id.as_deref() == me.as_deref();
        let sub = if p.host {
            "Coordena a sessão"
        } else {
            "Editando agora"
        };
        let tag = if p.host { "Anfitrião" } else { "Pode editar" };
        let name = if mine {
            format!("{} (você)", p.name)
        } else {
            p.name.clone()
        };
        person(
            ui,
            p.user_id.as_deref(),
            &name,
            sub,
            Some(tag),
            32.0,
            |_| {},
        );
    }
}

/// Uma linha de pessoa: foto, nome e detalhe alinhados ao centro da foto; à direita, a etiqueta
/// e o que mais vier (`trailing`).
fn person(
    ui: &mut Ui,
    user_id: Option<&str>,
    name: &str,
    detail: &str,
    tag: Option<&str>,
    size: f32,
    trailing: impl FnOnce(&mut Ui),
) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), ROW.max(size + 8.0)),
        Sense::hover(),
    );
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 10.0;
    let face = name.trim_end_matches(" (você)");
    avatar(&mut row, user_id, face, size);
    row.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(RichText::new(name).size(13.5).strong());
        if !detail.is_empty() {
            ui.label(RichText::new(detail).size(11.5).color(weak(ui)));
        }
    });
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        trailing(ui);
        if let Some(tag) = tag {
            chip(ui, tag);
        }
    });
}

fn media_line(ui: &mut Ui, m: &MediaLine) {
    let pct = if m.total > 0 {
        (m.done as f64 / m.total as f64).clamp(0.0, 1.0) as f32
    } else {
        1.0
    };
    ui.label(
        RichText::new(format!(
            "{} {}",
            if m.incoming { "Recebendo" } else { "Enviando" },
            m.name
        ))
        .size(12.0)
        .color(weak(ui)),
    );
    ui.add(
        egui::ProgressBar::new(pct)
            .desired_height(4.0)
            .desired_width(WIDTH)
            .fill(BLUE),
    );
}

fn footer(ui: &mut Ui, host: bool) -> Option<ShareAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if pill(
            ui,
            "Sair",
            vec2(72.0, 30.0),
            secondary(ui),
            ui.visuals().text_color(),
            true,
        )
        .on_hover_text(if host {
            "Quem está aqui segue editando; a pessoa que entrou primeiro passa a coordenar"
        } else {
            "Sair desta sessão"
        })
        .clicked()
        {
            action = Some(ShareAction::Stop);
        }
        if host
            && link(ui, "Encerrar para todos")
                .on_hover_text("Fecha a sessão de todo mundo")
                .clicked()
        {
            action = Some(ShareAction::EndAll);
        }
    });
    ui.add_space(2.0);
    ui.label(
        RichText::new("Só pessoas da sua empresa com acesso ao app podem entrar.")
            .size(11.0)
            .color(weak(ui)),
    );
    action
}

/// Botão em pílula com o texto centralizado, desenhado aqui para ficar igual em todos os apps.
fn pill(
    ui: &mut Ui,
    text: &str,
    size: Vec2,
    fill: Color32,
    text_color: Color32,
    enabled: bool,
) -> Response {
    let size = vec2(size.x.min(ui.available_width()), size.y);
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let mut bg = fill;
    if !enabled {
        bg = fill.gamma_multiply(0.45);
    } else if response.is_pointer_button_down_on() {
        bg = fill.lerp_to_gamma(Color32::BLACK, 0.15);
    } else if response.hovered() {
        bg = fill.lerp_to_gamma(Color32::WHITE, 0.1);
    }
    let radius = CornerRadius::same((size.y * 0.5).min(127.0) as u8);
    ui.painter().rect_filled(rect, radius, bg);
    if fill != BLUE {
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, line(ui)),
            egui::StrokeKind::Inside,
        );
    }
    let color = if enabled {
        text_color
    } else {
        text_color.gamma_multiply(0.6)
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(13.0),
        color,
    );
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

fn chip(ui: &mut Ui, text: &str) {
    let font = FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, weak(ui));
    let size = galley.size() + vec2(16.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same((size.y * 0.5) as u8),
        secondary(ui),
    );
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, weak(ui));
}

fn link(ui: &mut Ui, text: &str) -> Response {
    // No escuro, o azul da marca some sobre cinza; um azul mais claro mantém o contraste.
    let color = if ui.visuals().dark_mode {
        Color32::from_rgb(0x6c, 0xb0, 0xff)
    } else {
        BLUE
    };
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).sense(Sense::click()))
        .on_hover_cursor(CursorIcon::PointingHand)
}

fn field(ui: &mut Ui, text: &mut String, width: f32, hint: &str) {
    Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(Stroke::new(1.0, line(ui)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .frame(Frame::NONE)
                    .hint_text(hint)
                    .desired_width(width - 20.0),
            );
        });
}

fn divider(ui: &mut Ui) {
    ui.add_space(8.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, line(ui));
    ui.add_space(8.0);
}

fn weak(ui: &Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

fn line(ui: &Ui) -> Color32 {
    ui.visuals().widgets.noninteractive.bg_stroke.color
}

fn secondary(ui: &Ui) -> Color32 {
    let v = ui.visuals();
    if v.dark_mode {
        v.faint_bg_color.lerp_to_gamma(Color32::WHITE, 0.06)
    } else {
        v.faint_bg_color.lerp_to_gamma(Color32::BLACK, 0.04)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn share_window_draws_without_a_session() {
        let ctx = egui::Context::default();
        let mut open = true;
        let mut text = String::new();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let view = super::ShareView {
                app: "novaword",
                this_doc: "este documento",
                live: None,
                peers: &[],
                invite_text: &mut text,
                notice: None,
                error: None,
                media: Vec::new(),
            };
            let _ = super::share_window(ui.ctx(), &mut open, view);
        });
        assert!(open);
    }
}
