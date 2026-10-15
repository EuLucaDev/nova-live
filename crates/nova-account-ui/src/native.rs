use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, Context, CornerRadius, FontId, Frame, Id, Layout, Response, RichText,
    Sense, Stroke, TextureHandle, Ui, Vec2, vec2,
};
use nova_live::identity::{self, Status};

const BLUE: Color32 = Color32::from_rgb(0, 109, 255);
const CYAN: Color32 = Color32::from_rgb(45, 170, 215);

pub fn app_name(app: &str) -> &str {
    match app {
        "novaphoto" => "Nova Photo",
        "novavector" => "Nova Vector",
        "novalight" => "Nova Light",
        "novadeck" => "Nova Deck",
        "novaword" => "Nova Word",
        "novadesign" => "Nova Design",
        "novafig" => "Nova Fig",
        "novapdf" => "Nova PDF",
        "novasound" => "Nova Sound",
        "novafilm" => "Nova Film",
        "novaeffect" => "Nova Effect",
        "novacad" => "Nova CAD",
        "novagrid" => "Nova Grid",
        _ => "Nova Foods Launcher",
    }
}

/// Acabamento consistente, sem trocar cores, fontes, posições ou atalhos do aplicativo.
pub fn soften(ui: &mut Ui) {
    fn rounded(style: &mut egui::Style) {
        style.visuals.window_corner_radius = CornerRadius::same(14);
        style.visuals.menu_corner_radius = CornerRadius::same(10);
        style.visuals.widgets.inactive.corner_radius = CornerRadius::same(8);
        style.visuals.widgets.hovered.corner_radius = CornerRadius::same(8);
        style.visuals.widgets.active.corner_radius = CornerRadius::same(8);
        style.visuals.widgets.open.corner_radius = CornerRadius::same(8);
    }
    ui.ctx().all_styles_mut(rounded);
    rounded(ui.style_mut());
}

fn primary(ui: &mut Ui, text: &str, width: f32) -> Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(BLUE)
        .stroke(Stroke::NONE)
        .corner_radius(12)
        .min_size(vec2(width, 42.0)),
    )
}

/// Verifica a conta ao abrir o aplicativo. Uma queda de rede não apaga o documento já aberto.
/// Instalação, abertura pelo launcher e colaboração revalidam permissões separadamente.
pub fn gate(ui: &mut Ui, app: &str) -> bool {
    soften(ui);
    let ctx = ui.ctx().clone();
    let status = identity::status(app);
    if matches!(
        status,
        Status::Loading | Status::Connecting | Status::Waiting { .. }
    ) {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    if let Status::Waiting { url, code } = &status {
        let opened = Id::new(("nova-account-browser", code));
        if !ctx
            .data(|data| data.get_temp::<bool>(opened))
            .unwrap_or(false)
        {
            ctx.open_url(egui::OpenUrl::new_tab(url));
            ctx.data_mut(|data| data.insert_temp(opened, true));
        }
    }
    let unlocked = Id::new(("nova-account-unlocked", app));
    if matches!(&status, Status::Verified(profile) if profile.allows(app)) {
        ctx.data_mut(|data| data.insert_temp(unlocked, true));
        return true;
    }
    if ctx
        .data(|data| data.get_temp::<bool>(unlocked))
        .unwrap_or(false)
    {
        return true;
    }
    let dark = ui.visuals().dark_mode;
    let bg = if dark {
        Color32::from_rgb(12, 17, 25)
    } else {
        Color32::from_rgb(242, 246, 252)
    };
    let panel = if dark {
        Color32::from_rgb(24, 32, 44)
    } else {
        Color32::WHITE
    };
    let muted = if dark {
        Color32::from_rgb(158, 175, 195)
    } else {
        Color32::from_rgb(94, 111, 133)
    };
    let line = if dark {
        Color32::from_rgb(46, 60, 78)
    } else {
        Color32::from_rgb(220, 228, 238)
    };
    egui::CentralPanel::default().frame(Frame::new().fill(bg)).show(ui, |ui| {
        let width = (ui.available_width() - 56.0).clamp(200.0, 440.0);
        ui.add_space(((ui.available_height() - 430.0) * 0.4).max(20.0));
        ui.vertical_centered(|ui| {
            Frame::new().fill(panel).stroke(Stroke::new(1.0, line)).corner_radius(22).inner_margin(28).show(ui, |ui| {
                ui.set_min_width(width - 56.0); ui.set_max_width(width - 56.0);
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.label(RichText::new("Nova Foods").size(15.0).strong().color(BLUE));
                    ui.add_space(14.0);
                    ui.heading(RichText::new(app_name(app)).size(28.0).strong());
                    ui.add_space(8.0);
                    ui.label(RichText::new("Seu espaço de criar, com a sua conta.").size(14.0).color(muted));
                    ui.add_space(26.0);
                    match &status {
                        Status::Loading | Status::Connecting => {
                            ui.horizontal(|ui| { ui.spinner(); ui.label(if matches!(status, Status::Connecting) { "Preparando a conexão…" } else { "Carregando sua conta…" }); });
                            ctx.request_repaint_after(Duration::from_millis(100));
                        }
                        Status::Waiting { url, code } => {
                            ui.label(RichText::new("Confirme no seu navegador").size(17.0).strong());
                            ui.add_space(8.0);
                            ui.label(RichText::new("No site da Nova Foods, confira seu nome e confirme a conexão deste computador.").color(muted));
                            ui.add_space(14.0);
                            ui.label(RichText::new(format!("Código  {code}")).monospace().size(16.0).color(CYAN));
                            let opened = Id::new(("nova-account-browser", code));
                            if !ctx.data(|data| data.get_temp::<bool>(opened)).unwrap_or(false) {
                                ctx.open_url(egui::OpenUrl::new_tab(url));
                                ctx.data_mut(|data| data.insert_temp(opened, true));
                            }
                            ui.add_space(20.0);
                            if primary(ui, "Abrir o site novamente", width - 56.0).clicked() { ctx.open_url(egui::OpenUrl::new_tab(url)); }
                            ui.add_space(8.0);
                            if ui.link("Cancelar conexão").clicked() { identity::disconnect(); }
                            ctx.request_repaint_after(Duration::from_millis(150));
                        }
                        Status::Verified(profile) => {
                            person_row(ui, &profile.user.id, &profile.user.name, &profile.organization.name);
                            ui.add_space(18.0);
                            ui.label(RichText::new("Este aplicativo não está liberado para os seus setores.").size(15.0).strong());
                            ui.add_space(8.0);
                            ui.label(RichText::new("Fale com a administração para revisar seu acesso.").color(muted));
                            ui.add_space(18.0);
                            if primary(ui, "Atualizar meus acessos", width - 56.0).clicked() { identity::refresh(); }
                            if ui.link("Usar outra conta").clicked() { identity::disconnect(); identity::connect(app); }
                        }
                        Status::SignedOut | Status::Problem(_) => {
                            if let Status::Problem(message) = &status {
                                ui.label(RichText::new(message).color(if dark { Color32::from_rgb(255, 166, 142) } else { Color32::from_rgb(158, 64, 42) }));
                                ui.add_space(14.0);
                            }
                            ui.label(RichText::new("Entre com a mesma conta do site. Seu nome, foto e setores serão carregados automaticamente.").size(14.0).color(muted));
                            ui.add_space(24.0);
                            if primary(ui, "Entrar com minha conta", width - 56.0).clicked() { identity::connect(app); }
                            if matches!(status, Status::Problem(_)) && ui.link("Tentar validar novamente").clicked() { identity::refresh(); }
                        }
                    }
                    ui.add_space(26.0);
                    ui.separator(); ui.add_space(10.0);
                    ui.label(RichText::new("Uma conexão para os aplicativos Nova Foods deste computador.").size(11.5).color(muted));
                });
            });
        });
    });
    false
}

struct Avatar {
    image: Option<TextureHandle>,
    loading: bool,
    updated: Instant,
}
static AVATARS: OnceLock<Mutex<HashMap<String, Avatar>>> = OnceLock::new();
fn avatars() -> &'static Mutex<HashMap<String, Avatar>> {
    AVATARS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn photo(ctx: &Context, user: &str) -> Option<TextureHandle> {
    let profile = identity::profile()?;
    if user == profile.user.id && profile.user.avatar_url.is_none() {
        return None;
    }
    let key = format!("{}:{user}", profile.organization.id);
    let mut cache = avatars().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(entry) = cache.get(&key) {
        if entry.loading || entry.updated.elapsed() < Duration::from_secs(120) {
            return entry.image.clone();
        }
    }
    if cache.len() > 128 {
        cache.clear();
    }
    let old = cache.get(&key).and_then(|entry| entry.image.clone());
    cache.insert(
        key.clone(),
        Avatar {
            image: old.clone(),
            loading: true,
            updated: Instant::now(),
        },
    );
    drop(cache);
    let ctx = ctx.clone();
    let user = user.to_string();
    std::thread::spawn(move || {
        let bytes = if let Some(name) = user.strip_prefix("name:") {
            identity::avatar_by_name(name)
        } else {
            identity::avatar(&user)
        };
        let image = bytes.ok().and_then(|bytes| {
            let mut reader = image::ImageReader::new(Cursor::new(bytes))
                .with_guessed_format()
                .ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(512);
            limits.max_image_height = Some(512);
            limits.max_alloc = Some(4 << 20);
            reader.limits(limits);
            let image = reader.decode().ok()?.to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            Some(ctx.load_texture(
                format!("nova-person-{user}"),
                egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                egui::TextureOptions::LINEAR,
            ))
        });
        avatars()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                key,
                Avatar {
                    image,
                    loading: false,
                    updated: Instant::now(),
                },
            );
        ctx.request_repaint();
    });
    old
}
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

pub fn avatar(ui: &mut Ui, user_id: Option<&str>, name: &str, size: f32) -> Response {
    let key = user_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("name:{name}"));
    let image = photo(ui.ctx(), &key);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let c = nova_live::cursors::color_for(name);
    let fill = Color32::from_rgb(c[0], c[1], c[2]);
    ui.painter().circle_filled(rect.center(), size * 0.5, fill);
    if let Some(image) = image {
        egui::Image::new((image.id(), rect.size()))
            .corner_radius((size * 0.5) as u8)
            .paint_at(ui, rect);
    } else {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            initials(name),
            FontId::proportional(size * 0.37),
            Color32::WHITE,
        );
    }
    response.on_hover_text(name)
}

pub fn person_row(ui: &mut Ui, user_id: &str, name: &str, detail: &str) {
    ui.horizontal(|ui| {
        avatar(ui, Some(user_id), name, 38.0);
        ui.add_space(5.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(name).strong().size(14.0));
            ui.label(RichText::new(detail).weak().size(11.5));
        });
    });
}

#[derive(Clone)]
pub struct Presence {
    pub user_id: Option<String>,
    pub name: String,
    pub host: bool,
}

/// Avatares e ação principal compacta; nomes reais estão disponíveis no hover e na lista.
pub fn share_button(ui: &mut Ui, people: &[Presence], active: bool) -> Response {
    let me = identity::profile();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if let Some(me) = &me {
            avatar(ui, Some(&me.user.id), &me.user.name, 27.0);
            for person in people
                .iter()
                .filter(|person| person.user_id.as_deref() != Some(me.user.id.as_str()))
                .take(2)
            {
                avatar(ui, person.user_id.as_deref(), &person.name, 27.0);
            }
        }
        let label = "Compartilhar";
        ui.add(
            egui::Button::new(
                RichText::new(label)
                    .size(12.5)
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(BLUE)
            .stroke(Stroke::NONE)
            .corner_radius(16)
            .min_size(vec2(112.0, 32.0)),
        )
        .on_hover_text(if active {
            "Pessoas e convite deste arquivo"
        } else {
            "Convidar pessoas para editar este arquivo"
        })
    })
    .inner
}

pub fn account_panel(ui: &mut Ui, app: &str) {
    match identity::status(app) {
        Status::Verified(profile) => {
            person_row(
                ui,
                &profile.user.id,
                &profile.user.name,
                &profile
                    .departments
                    .iter()
                    .map(|d| d.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            if ui.small_button("Trocar conta").clicked() {
                identity::disconnect();
                identity::connect(app);
            }
        }
        _ => {
            if primary(ui, "Conectar minha conta", ui.available_width()).clicked() {
                identity::connect(app);
            }
        }
    }
}

/// Presença do Fig sem alterar seu protocolo binário nem os arquivos compartilhados existentes.
pub fn paint_person(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    name: &str,
    fill: Color32,
) {
    let key = identity::profile()
        .filter(|p| p.user.name == name)
        .map(|p| p.user.id)
        .unwrap_or_else(|| format!("name:{name}"));
    painter.circle_filled(center, radius, fill);
    if let Some(texture) = photo(painter.ctx(), &key) {
        let rect = egui::Rect::from_center_size(center, Vec2::splat(radius * 2.0));
        let mut mesh = egui::Mesh::with_texture(texture.id());
        mesh.vertices.push(egui::epaint::Vertex {
            pos: center,
            uv: egui::pos2(0.5, 0.5),
            color: Color32::WHITE,
        });
        for step in 0..=48 {
            let angle = step as f32 * std::f32::consts::TAU / 48.0;
            let direction = vec2(angle.cos(), angle.sin());
            mesh.vertices.push(egui::epaint::Vertex {
                pos: center + direction * radius,
                uv: egui::pos2(0.5 + direction.x * 0.5, 0.5 + direction.y * 0.5),
                color: Color32::WHITE,
            });
            if step > 0 {
                mesh.indices.extend_from_slice(&[0, step, step + 1]);
            }
        }
        let _ = rect;
        painter.add(egui::Shape::mesh(mesh));
    } else {
        painter.text(
            center,
            Align2::CENTER_CENTER,
            initials(name),
            FontId::proportional(radius * 0.8),
            Color32::WHITE,
        );
    }
}

pub fn profile() -> Option<identity::Profile> {
    identity::profile()
}
