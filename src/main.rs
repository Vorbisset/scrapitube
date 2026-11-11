mod download;
mod theme;
mod timecode;

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::Local;
use eframe::egui::{self, Align, Button, Color32, Key, Layout, ProgressBar, RichText, TextEdit, Vec2};

use download::{Browser, Container, Job, Meta, Mode, Options, Progress, Request, Section, Status};

const APP_NAME: &str = "Scrapitube";
const LOGO: &[u8] = include_bytes!("../assets/icon.png");
const SETTINGS_KEY: &str = "options";
const MAX_PARALLEL: usize = 2;
const DEFAULT_HEIGHTS: [u32; 5] = [2160, 1440, 1080, 720, 480];
/// Délai après la dernière frappe avant d'analyser le lien (un collage est analysé tout de suite).
const DEBOUNCE: Duration = Duration::from_millis(500);

fn main() -> eframe::Result {
    // Lancée depuis le Finder, l'appli a un PATH minimal : on y ajoute Homebrew (yt-dlp, ffmpeg).
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{path}"));
    download::install_bundled_tools();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1000.0, 840.0])
        .with_min_inner_size([780.0, 580.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(LOGO) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

enum Preview {
    Empty,
    Loading,
    Ready(Meta),
    Failed(String),
}

/// Bornes de l'extrait, avec le texte saisi pour chacune.
#[derive(Default)]
struct Clip {
    enabled: bool,
    start: f64,
    end: f64,
    start_text: String,
    end_text: String,
}

impl Clip {
    fn reset(&mut self, duration: f64) {
        *self = Clip { start: 0.0, end: duration, ..Default::default() };
        self.start_text = timecode::format(0.0);
        self.end_text = timecode::format(duration);
    }

    fn error(&self) -> Option<&'static str> {
        if timecode::parse(&self.start_text).is_none() || timecode::parse(&self.end_text).is_none() {
            Some("Temps invalide : écris par exemple 90, 1:30 ou 1:02:03.")
        } else if self.end <= self.start {
            Some("La fin doit être après le début.")
        } else {
            None
        }
    }
}

struct App {
    options: Options,
    url_input: String,
    edited_at: Option<Instant>,
    /// Lien (avec les réglages playlist et session) correspondant à l'aperçu affiché.
    analyzed: Option<(String, bool, Option<Browser>)>,
    browsers: Vec<Browser>,
    preview: Preview,
    analysis: Option<mpsc::Receiver<Result<Meta, String>>>,
    clip: Clip,
    jobs: Vec<Job>,
    yt_dlp: Option<(PathBuf, String)>,
    has_ffmpeg: bool,
    /// Mise à jour de yt-dlp en cours, puis son résultat.
    yt_dlp_update: Option<mpsc::Receiver<String>>,
    yt_dlp_message: Option<String>,
}

/// Action demandée sur un téléchargement de la liste, appliquée après l'affichage.
enum JobAction {
    Cancel(usize),
    Retry(usize),
    Remove(usize),
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let options = cc.storage.and_then(|s| eframe::get_value(s, SETTINGS_KEY)).unwrap_or_default();
        Self {
            options,
            url_input: String::new(),
            edited_at: None,
            analyzed: None,
            browsers: Browser::installed(),
            preview: Preview::Empty,
            analysis: None,
            clip: Clip::default(),
            jobs: Vec::new(),
            yt_dlp: download::find_yt_dlp(),
            has_ffmpeg: download::has_ffmpeg(),
            yt_dlp_update: None,
            yt_dlp_message: None,
        }
    }

    fn start_yt_dlp_update(&mut self, ctx: &egui::Context) {
        let Some((yt_dlp, _)) = self.yt_dlp.clone() else { return };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(download::update_yt_dlp(&yt_dlp));
            ctx.request_repaint();
        });
        self.yt_dlp_update = Some(rx);
        self.yt_dlp_message = None;
    }

    fn poll_yt_dlp_update(&mut self) {
        let Some(message) = self.yt_dlp_update.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.yt_dlp_update = None;
        self.yt_dlp_message = Some(message);
        self.yt_dlp = download::find_yt_dlp();
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match &self.yt_dlp {
                Some((_, version)) => {
                    ui.label(RichText::new(format!("yt-dlp {version}")).small().color(theme::MUTED));
                    if self.yt_dlp_update.is_some() {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(RichText::new("mise à jour…").small().color(theme::MUTED));
                    } else if ui.small_button("Mettre à jour yt-dlp").on_hover_text("À faire si YouTube refuse un lien").clicked() {
                        self.start_yt_dlp_update(ui.ctx());
                    }
                    if let Some(message) = &self.yt_dlp_message {
                        ui.label(RichText::new(message).small().color(theme::MUTED));
                    }
                }
                None => {
                    ui.label(RichText::new("yt-dlp introuvable. Installe-le avec : brew install yt-dlp").color(theme::WARNING));
                }
            }
            if !self.has_ffmpeg {
                ui.label(RichText::new("ffmpeg introuvable : brew install ffmpeg").color(theme::WARNING));
            }
            if self.yt_dlp.is_none() || !self.has_ffmpeg {
                if ui.small_button("Réessayer").clicked() {
                    self.yt_dlp = download::find_yt_dlp();
                    self.has_ffmpeg = download::has_ffmpeg();
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = self.options.cookies_browser.map_or("Aucune", Browser::label);
                egui::ComboBox::from_id_salt("cookies")
                    .selected_text(label)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.options.cookies_browser, None, "Aucune");
                        for &browser in &self.browsers {
                            ui.selectable_value(&mut self.options.cookies_browser, Some(browser), browser.label());
                        }
                    });
                ui.label(RichText::new("Session YouTube :").small().color(theme::MUTED))
                    .on_hover_text("Navigateur dont Scrapitube réutilise la connexion YouTube, si YouTube demande de prouver que tu n'es pas un robot.");
            });
        });
    }

    fn url(&self) -> Option<String> {
        download::youtube_url(&self.url_input).ok()
    }

    /// Lance l'analyse du lien saisi s'il a changé (une fois la frappe terminée).
    fn maybe_analyze(&mut self, ctx: &egui::Context) {
        let url = match download::youtube_url(&self.url_input) {
            Ok(url) => url,
            Err(reason) => {
                // Champ vide : aperçu d'accueil ; lien refusé : on dit pourquoi.
                self.preview = if self.url_input.trim().is_empty() {
                    Preview::Empty
                } else {
                    Preview::Failed(reason.to_string())
                };
                self.analyzed = None;
                self.analysis = None;
                return;
            }
        };
        let key = (url.clone(), self.options.no_playlist, self.options.cookies_browser);
        if self.analyzed.as_ref() == Some(&key) {
            return;
        }
        if let Some(remaining) = self.edited_at.and_then(|t| DEBOUNCE.checked_sub(t.elapsed())) {
            ctx.request_repaint_after(remaining);
            return;
        }
        let Some((yt_dlp, _)) = self.yt_dlp.clone() else { return };

        let (tx, rx) = mpsc::channel();
        let (ctx, options) = (ctx.clone(), self.options.clone());
        std::thread::spawn(move || {
            let _ = tx.send(download::analyze(&yt_dlp, &url, &options));
            ctx.request_repaint();
        });
        // Remplacer le récepteur ignore le résultat d'une analyse précédente encore en cours.
        self.analysis = Some(rx);
        self.analyzed = Some(key);
        self.preview = Preview::Loading;
    }

    fn poll_analysis(&mut self) {
        let Some(result) = self.analysis.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.analysis = None;
        self.preview = match result {
            Ok(meta) => {
                self.clip.reset(meta.duration.unwrap_or(0.0));
                Preview::Ready(meta)
            }
            Err(e) => Preview::Failed(e),
        };
    }

    fn enqueue(&mut self, meta: Meta) {
        let Some(url) = self.url() else { return };
        let section = (self.clip.enabled && meta.playlist_count.is_none())
            .then_some(Section { start: self.clip.start, end: self.clip.end });
        self.jobs.insert(0, Job::new(Request { url, options: self.options.clone(), section, meta }));
        self.url_input.clear();
        self.preview = Preview::Empty;
        self.analyzed = None;
        self.clip = Clip::default();
    }

    fn start_queued(&self, ctx: &egui::Context) {
        let Some((yt_dlp, _)) = &self.yt_dlp else { return };
        let snapshots: Vec<Status> = self.jobs.iter().map(|j| j.snapshot().status).collect();
        let mut running = snapshots.iter().filter(|s| s.is_active()).count();
        if running > 0 {
            // Rafraîchit chaque seconde le temps écoulé et l'heure de fin estimée.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        // Les plus récents sont en haut de la liste : on démarre les plus anciens d'abord.
        for (job, status) in self.jobs.iter().zip(&snapshots).rev() {
            if running >= MAX_PARALLEL {
                break;
            }
            if *status == Status::Queued {
                job.start(yt_dlp.clone(), ctx.clone());
                running += 1;
            }
        }
    }

    // ---------- Interface ----------

    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::from_bytes("bytes://scrapitube-logo.png", LOGO).fit_to_exact_size(egui::vec2(56.0, 56.0)));
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(RichText::new(APP_NAME).font(theme::heavy(30.0)).color(Color32::WHITE));
                ui.label(RichText::new("Télécharge des vidéos YouTube, en entier ou juste un extrait.").color(theme::MUTED));
            });
        });
    }

    fn url_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let paste_width = 120.0;
            let field = TextEdit::singleline(&mut self.url_input)
                .hint_text(RichText::new("Colle ici un lien YouTube (vidéo ou playlist)").color(theme::MUTED))
                .font(egui::FontId::proportional(17.0))
                .margin(egui::vec2(16.0, 13.0));
            let response = ui.add_sized([ui.available_width() - paste_width - 10.0, 50.0], field);
            if response.changed() {
                // Un collage (plusieurs caractères d'un coup) est analysé sans attendre.
                let pasted = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))));
                self.edited_at = (!pasted).then(Instant::now);
            }
            if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.edited_at = None;
            }
            let paste = Button::new(RichText::new("Coller").font(theme::bold(16.0)))
                .min_size(egui::vec2(paste_width, 50.0))
                .rounding(10.0);
            if ui.add(paste).on_hover_text("Coller le lien copié").clicked() {
                if let Ok(out) = Command::new("pbpaste").output() {
                    self.url_input = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    self.edited_at = None;
                }
            }
        });
    }

    /// YouTube exige une session connectée : propose les navigateurs installés.
    fn sign_in_help(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("YouTube demande de vérifier que tu n'es pas un robot").font(theme::bold(19.0)).color(theme::WARNING));
            ui.label(
                RichText::new(
                    "Scrapitube peut réutiliser ta session YouTube. Choisis un navigateur dans lequel tu es \
                     connecté à YouTube (ouvre youtube.com pour vérifier) :",
                )
                .color(theme::MUTED),
            );
            ui.add_space(6.0);
            if self.browsers.is_empty() {
                ui.label(RichText::new("Aucun navigateur compatible trouvé dans Applications.").color(theme::DANGER));
            }
            ui.horizontal_wrapped(|ui| {
                for &browser in &self.browsers {
                    let selected = self.options.cookies_browser == Some(browser);
                    let text = RichText::new(browser.label()).font(theme::bold(15.0)).color(Color32::WHITE);
                    let button = Button::new(text)
                        .fill(if selected { theme::ACCENT } else { theme::SURFACE_HOVER })
                        .min_size(egui::vec2(110.0, 38.0));
                    if ui.add(button).clicked() {
                        self.options.cookies_browser = Some(browser);
                        self.analyzed = None;
                    }
                }
            });
            if let Some(browser) = self.options.cookies_browser {
                ui.label(
                    RichText::new(format!(
                        "YouTube refuse encore avec {} : vérifie que tu y es bien connecté à YouTube, ou essaie un autre navigateur.",
                        browser.label()
                    ))
                    .color(theme::MUTED),
                );
            }
            if self.browsers.iter().any(|b| b.uses_keychain()) {
                ui.label(
                    RichText::new("Avec Chrome, Brave ou Edge, macOS demandera l'accès au trousseau : clique sur « Toujours autoriser ».")
                        .small()
                        .color(theme::MUTED),
                );
            }
        });
    }

    /// macOS a refusé l'accès aux cookies (cas de Safari sans « accès complet au disque »).
    fn cookies_access_help(&mut self, ui: &mut egui::Ui, error: &str) {
        let browser = self.options.cookies_browser.map_or("ce navigateur", Browser::label);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(format!("macOS bloque l'accès à la session {browser}")).font(theme::bold(19.0)).color(theme::WARNING));
            if self.options.cookies_browser == Some(Browser::Safari) {
                ui.label(
                    RichText::new(
                        "Pour lire la session Safari, Scrapitube a besoin de l'« accès complet au disque » : \
                         Réglages Système → Confidentialité et sécurité → Accès complet au disque → active Scrapitube \
                         (ajoute-la avec « + » si besoin), puis relance l'appli.",
                    )
                    .color(theme::MUTED),
                );
                if ui.button("Ouvrir les réglages").clicked() {
                    open(&["x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"]);
                }
            } else {
                ui.label(RichText::new(error).color(theme::MUTED));
            }
            ui.horizontal(|ui| {
                if ui.button("Réessayer").clicked() {
                    self.analyzed = None;
                }
                if ui.button("Choisir un autre navigateur").clicked() {
                    self.options.cookies_browser = None;
                }
            });
        });
    }

    fn preview_card(&mut self, ui: &mut egui::Ui) {
        theme::card().inner_margin(22.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            match &self.preview {
                Preview::Empty => {
                    ui.add_space(14.0);
                    ui.vertical_centered(|ui| {
                        ui.add(
                            egui::Image::from_bytes("bytes://scrapitube-logo.png", LOGO)
                                .fit_to_exact_size(egui::vec2(44.0, 44.0))
                                .tint(Color32::from_white_alpha(110)),
                        );
                        ui.label(RichText::new("Colle un lien pour commencer").font(theme::bold(19.0)));
                        ui.label(
                            RichText::new("La miniature, le titre et les qualités disponibles s'afficheront ici.")
                                .color(theme::MUTED),
                        );
                    });
                    ui.add_space(14.0);
                }
                Preview::Loading => {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Spinner::new().size(32.0).color(theme::ACCENT));
                        ui.label(RichText::new("Analyse du lien…").font(theme::bold(16.0)).color(theme::MUTED));
                    });
                    ui.add_space(30.0);
                }
                Preview::Failed(error) => {
                    let error = error.clone();
                    ui.add_space(16.0);
                    if download::needs_sign_in(&error) {
                        self.sign_in_help(ui);
                    } else if self.options.cookies_browser.is_some() && download::cookies_access_denied(&error) {
                        self.cookies_access_help(ui, &error);
                    } else {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("Impossible de lire ce lien").font(theme::bold(19.0)).color(theme::DANGER));
                            ui.label(RichText::new(error).color(theme::MUTED));
                            if ui.button("Réessayer").clicked() {
                                self.analyzed = None;
                            }
                        });
                    }
                    ui.add_space(16.0);
                }
                Preview::Ready(meta) => {
                    let meta = meta.clone();
                    self.ready_preview(ui, meta);
                }
            }
        });
    }

    fn ready_preview(&mut self, ui: &mut egui::Ui, meta: Meta) {
        // En-tête : miniature + titre.
        ui.horizontal_top(|ui| {
            let thumb_width = (ui.available_width() * 0.4).clamp(240.0, 360.0);
            thumbnail(ui, meta.thumbnail.as_deref(), egui::vec2(thumb_width, thumb_width * 9.0 / 16.0), 12.0);
            ui.vertical(|ui| {
                ui.add(egui::Label::new(RichText::new(&meta.title).font(theme::bold(22.0)).color(Color32::WHITE)).wrap());
                if let Some(uploader) = &meta.uploader {
                    ui.label(RichText::new(uploader).font(theme::bold(15.0)).color(theme::MUTED));
                }
                ui.horizontal_wrapped(|ui| {
                    if let Some(count) = meta.playlist_count {
                        theme::chip(ui, format!("Playlist · {count} vidéos"), theme::PINK);
                    }
                    if let Some(d) = meta.duration {
                        theme::chip(ui, format!("Durée {}", timecode::format(d)), theme::ACCENT_HOVER);
                    }
                    if let Some(best) = meta.heights.first() {
                        theme::chip(ui, format!("Jusqu'à {}", quality_label(Some(*best))), theme::ACCENT_HOVER);
                    }
                    if let Some(views) = meta.view_count {
                        theme::chip(ui, format!("{} vues", human_count(views)), theme::MUTED);
                    }
                });
            });
        });

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        let label = |ui: &mut egui::Ui, text: &str| {
            ui.add_sized([90.0, 34.0], egui::Label::new(RichText::new(text).font(theme::bold(14.0)).color(theme::MUTED)));
        };
        let is_playlist = meta.playlist_count.is_some();
        let has_list_param = self.url_input.contains("list=");

        egui::Grid::new("options").num_columns(2).spacing([18.0, 14.0]).show(ui, |ui| {
            label(ui, "Type");
            segmented(ui, &mut self.options.mode, &[(Mode::Video, "Vidéo"), (Mode::Audio, "Audio MP3")]);
            ui.end_row();

            if self.options.mode == Mode::Video {
                label(ui, "Qualité");
                ui.horizontal(|ui| {
                    let heights = if meta.heights.is_empty() { DEFAULT_HEIGHTS.to_vec() } else { meta.heights.clone() };
                    let o = &mut self.options;
                    egui::ComboBox::from_id_salt("quality")
                        .selected_text(quality_label(o.max_height))
                        .width(190.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut o.max_height, None, quality_label(None));
                            for h in heights.into_iter().filter(|&h| h >= 144) {
                                ui.selectable_value(&mut o.max_height, Some(h), quality_label(Some(h)));
                            }
                        });
                    egui::ComboBox::from_id_salt("container")
                        .selected_text(o.container.as_str().to_uppercase())
                        .width(90.0)
                        .show_ui(ui, |ui| {
                            for c in Container::ALL {
                                ui.selectable_value(&mut o.container, c, c.as_str().to_uppercase());
                            }
                        });
                    ui.checkbox(&mut o.subs, "Sous-titres (fr, en)");
                });
                ui.end_row();
            }

            if has_list_param {
                label(ui, "Playlist");
                let mut whole = !self.options.no_playlist;
                if ui.checkbox(&mut whole, "Télécharger toute la playlist").changed() {
                    self.options.no_playlist = !whole;
                }
                ui.end_row();
            }

            if !is_playlist {
                if let Some(duration) = meta.duration.filter(|&d| d > 0.0) {
                    label(ui, "Extrait");
                    ui.vertical(|ui| {
                        ui.checkbox(&mut self.clip.enabled, "Télécharger seulement un extrait (temps IN / OUT)");
                        if self.clip.enabled {
                            clip_editor(ui, &mut self.clip, duration);
                        }
                    });
                    ui.end_row();
                }
            }

            label(ui, "Dossier");
            ui.horizontal(|ui| {
                ui.label(self.options.output_dir.display().to_string());
                if ui.button("Changer…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_directory(&self.options.output_dir).pick_folder() {
                        self.options.output_dir = dir;
                    }
                }
                if ui.button("Ouvrir").clicked() {
                    let _ = std::fs::create_dir_all(&self.options.output_dir);
                    open(&[&self.options.output_dir]);
                }
            });
            ui.end_row();
        });

        ui.add_space(14.0);
        let clip_error = (self.clip.enabled && !is_playlist).then(|| self.clip.error()).flatten();
        if let Some(error) = clip_error {
            ui.label(RichText::new(error).color(theme::DANGER));
        }
        let text = download_label(&self.options, &meta, self.clip.enabled.then_some(&self.clip));
        let enabled = clip_error.is_none();
        let button = theme::primary_button(text, enabled).min_size(egui::vec2(ui.available_width(), 54.0));
        if ui.add_enabled(enabled, button).clicked() {
            self.enqueue(meta);
        }
    }

    fn job_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Téléchargements").font(theme::heavy(20.0)).color(Color32::WHITE));
            let active = self.jobs.iter().filter(|j| j.snapshot().status.is_active()).count();
            if active > 0 {
                theme::chip(ui, format!("{active} en cours"), theme::ACCENT_HOVER);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let any_finished = self.jobs.iter().any(|j| j.snapshot().status.is_finished());
                if ui.add_enabled(any_finished, Button::new("Effacer les terminés")).clicked() {
                    self.jobs.retain(|j| !j.snapshot().status.is_finished());
                }
            });
        });
        ui.add_space(4.0);

        if self.jobs.is_empty() {
            ui.label(RichText::new("Aucun téléchargement pour l'instant.").color(theme::MUTED));
            return;
        }

        let mut action = None;
        for (i, job) in self.jobs.iter().enumerate() {
            if let Some(a) = job_card(ui, i, job, &job.snapshot()) {
                action = Some(a);
            }
            ui.add_space(2.0);
        }

        match action {
            Some(JobAction::Cancel(i)) => self.jobs[i].cancel(),
            Some(JobAction::Retry(i)) => {
                // Reprend la session YouTube choisie entre-temps (cas « not a bot »).
                let mut request = self.jobs[i].request.clone();
                request.options.cookies_browser = self.options.cookies_browser;
                self.jobs[i] = Job::new(request);
            }
            Some(JobAction::Remove(i)) => {
                self.jobs.remove(i);
            }
            None => {}
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_analysis();
        self.poll_yt_dlp_update();
        self.maybe_analyze(ctx);
        self.start_queued(ctx);

        egui::TopBottomPanel::bottom("footer")
            .frame(egui::Frame::none().fill(theme::SIDEBAR).inner_margin(egui::Margin::symmetric(28.0, 8.0)))
            .show(ctx, |ui| self.footer(ui));

        egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::BG)).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                egui::Frame::none().inner_margin(egui::Margin::symmetric(30.0, 24.0)).show(ui, |ui| {
                    self.header(ui);
                    ui.add_space(14.0);
                    self.url_bar(ui);
                    ui.add_space(8.0);
                    self.preview_card(ui);
                    self.job_list(ui);
                });
            });
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.options);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for job in &self.jobs {
            if job.snapshot().status.is_active() {
                job.cancel();
            }
        }
    }
}

/// Champs « Début (IN) / Fin (OUT) » avec leurs curseurs.
fn clip_editor(ui: &mut egui::Ui, clip: &mut Clip, duration: f64) {
    let row = |ui: &mut egui::Ui, name: &str, value: &mut f64, text: &mut String| {
        ui.horizontal(|ui| {
            ui.add_sized([86.0, 32.0], egui::Label::new(RichText::new(name).font(theme::bold(14.0))));
            let valid = timecode::parse(text).is_some();
            let field = TextEdit::singleline(text)
                .desired_width(90.0)
                .font(theme::bold(15.0))
                .margin(egui::vec2(10.0, 7.0))
                .text_color(if valid { theme::TEXT } else { theme::DANGER });
            let response = ui.add(field);
            if response.changed() {
                if let Some(t) = timecode::parse(text) {
                    *value = t.min(duration);
                }
            }
            if response.lost_focus() && valid {
                *text = timecode::format(*value);
            }
            ui.spacing_mut().slider_width = (ui.available_width() - 10.0).max(80.0);
            let mut v = *value;
            if ui.add(egui::Slider::new(&mut v, 0.0..=duration).show_value(false)).changed() {
                *value = v.round();
                *text = timecode::format(*value);
            }
        });
    };
    row(ui, "Début (IN)", &mut clip.start, &mut clip.start_text);
    row(ui, "Fin (OUT)", &mut clip.end, &mut clip.end_text);
    if clip.end > clip.start {
        ui.label(
            RichText::new(format!(
                "Extrait de {} à {} · durée {}",
                timecode::format(clip.start),
                timecode::format(clip.end),
                timecode::format(clip.end - clip.start)
            ))
            .color(theme::MUTED),
        );
    }
}

/// Boutons côte à côte dont un seul est sélectionné.
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, items: &[(T, &str)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (item, text) in items {
            let selected = *value == *item;
            let text = RichText::new(*text).font(theme::bold(15.0)).color(if selected { Color32::WHITE } else { theme::MUTED });
            let button = Button::new(text)
                .fill(if selected { theme::ACCENT } else { theme::SURFACE_HOVER })
                .stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(140.0, 38.0));
            if ui.add(button).clicked() {
                *value = *item;
            }
        }
    });
}

fn thumbnail(ui: &mut egui::Ui, url: Option<&str>, size: Vec2, rounding: f32) {
    match url {
        Some(url) => {
            ui.add(egui::Image::new(url).fit_to_exact_size(size).maintain_aspect_ratio(false).rounding(rounding));
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect_filled(rect, rounding, theme::SURFACE_HOVER);
        }
    }
}

fn job_card(ui: &mut egui::Ui, index: usize, job: &Job, p: &Progress) -> Option<JobAction> {
    let mut action = None;
    let request = &job.request;
    theme::card().inner_margin(14.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            thumbnail(ui, request.meta.thumbnail.as_deref(), egui::vec2(176.0, 99.0), 8.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 150.0);
                ui.add(
                    egui::Label::new(RichText::new(&request.meta.title).font(theme::bold(16.0)).color(Color32::WHITE))
                        .truncate(),
                );
                ui.horizontal_wrapped(|ui| {
                    let (status, color) = status_chip(&p.status);
                    theme::chip(ui, status, color);
                    theme::chip(ui, format_label(request), theme::MUTED);
                    if let Some(s) = request.section {
                        let clip = format!("Extrait {} – {}", timecode::format(s.start), timecode::format(s.end));
                        theme::chip(ui, clip, theme::PINK);
                    }
                    if let Some(count) = request.meta.playlist_count {
                        theme::chip(ui, format!("Playlist · {count} vidéos"), theme::PINK);
                    }
                });

                if p.status.is_active() {
                    ui.horizontal(|ui| {
                        let percent_width = 64.0;
                        let bar = match (p.status == Status::Downloading).then_some(p.fraction).flatten() {
                            Some(f) => ProgressBar::new(f),
                            None => ProgressBar::new(0.0).animate(true),
                        };
                        ui.add(bar.desired_width(ui.available_width() - percent_width).desired_height(8.0).fill(theme::ACCENT));
                        let percent = match (&p.status, p.fraction) {
                            (Status::Downloading, Some(f)) => format!("{:.0} %", f * 100.0),
                            _ => "…".to_string(),
                        };
                        ui.label(RichText::new(percent).font(theme::heavy(18.0)).color(theme::ACCENT_HOVER));
                    });
                }

                let (details, color) = details_line(request, p);
                ui.label(RichText::new(details).color(color));
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| match &p.status {
                s if s.is_active() || *s == Status::Queued => {
                    if ui.button("Annuler").clicked() {
                        action = Some(JobAction::Cancel(index));
                    }
                }
                Status::Done => {
                    ui.vertical(|ui| {
                        if let Some(file) = p.files.last() {
                            if ui.button("Voir le fichier").clicked() {
                                open(&["-R".as_ref(), file.as_os_str()]);
                            }
                        }
                        if ui.button("Retirer").clicked() {
                            action = Some(JobAction::Remove(index));
                        }
                    });
                }
                _ => {
                    ui.vertical(|ui| {
                        if ui.button("Réessayer").clicked() {
                            action = Some(JobAction::Retry(index));
                        }
                        if ui.button("Retirer").clicked() {
                            action = Some(JobAction::Remove(index));
                        }
                    });
                }
            });
        });
    });
    action
}

fn status_chip(status: &Status) -> (&'static str, Color32) {
    match status {
        Status::Queued => ("En attente", theme::MUTED),
        Status::Starting => ("Démarrage", theme::ACCENT_HOVER),
        Status::Downloading => ("Téléchargement", theme::ACCENT_HOVER),
        Status::Converting => ("Finalisation", theme::WARNING),
        Status::Done => ("Terminé", theme::SUCCESS),
        Status::Failed(_) => ("Erreur", theme::DANGER),
        Status::Cancelled => ("Annulé", theme::MUTED),
    }
}

fn format_label(request: &Request) -> String {
    match request.options.mode {
        Mode::Audio => "Audio MP3".to_string(),
        Mode::Video => format!(
            "{} · {}",
            quality_label(request.options.max_height),
            request.options.container.as_str().to_uppercase()
        ),
    }
}

/// Détails sous la barre : début, quantité, débit, temps restant, heure de fin estimée.
fn details_line(request: &Request, p: &Progress) -> (String, Color32) {
    let started = p.started.map(|s| format!("Démarré à {}", s.clock.format("%H:%M")));
    let mut parts: Vec<String> = started.into_iter().collect();

    match &p.status {
        Status::Queued => return ("En attente d'un téléchargement libre…".into(), theme::MUTED),
        Status::Starting => parts.push("connexion à YouTube…".into()),
        Status::Downloading => {
            if request.meta.playlist_count.is_some() {
                if let Some(current) = &p.current {
                    parts.push(format!("vidéo en cours : {current}"));
                }
            }
            match (p.clip_len, p.clip_done) {
                (Some(len), Some(done)) => {
                    parts.push(format!("{} / {} d'extrait", timecode::format(done), timecode::format(len)));
                    if let Some(speed) = p.clip_speed {
                        parts.push(format!("vitesse ×{}", format!("{speed:.1}").replace('.', ",")));
                    }
                }
                _ => {
                    match p.total {
                        Some(total) => parts.push(format!("{} / {}", human_bytes(p.downloaded as f64), human_bytes(total as f64))),
                        None if p.downloaded > 0 => parts.push(human_bytes(p.downloaded as f64)),
                        None => {}
                    }
                    if let Some(speed) = p.speed {
                        parts.push(format!("{}/s", human_bytes(speed)));
                    }
                }
            }
            if let Some(eta) = p.eta {
                let end = Local::now() + chrono::Duration::seconds(eta.round() as i64);
                parts.push(format!("reste {}", timecode::format(eta)));
                parts.push(format!("fin estimée vers {}", end.format("%H:%M")));
            }
        }
        Status::Converting => parts.push("assemblage du fichier final…".into()),
        Status::Done => {
            if let (Some(start), Some(end)) = (p.started, p.finished) {
                let took = end.instant.duration_since(start.instant).as_secs_f64();
                parts.push(format!("terminé à {} (en {})", end.clock.format("%H:%M"), timecode::format(took)));
            }
            if p.files.len() > 1 {
                parts.push(format!("{} fichiers", p.files.len()));
            }
        }
        Status::Failed(e) if download::needs_sign_in(e) => {
            let text = "YouTube demande une connexion : choisis un navigateur dans « Session YouTube » (en bas), puis Réessayer.";
            return (text.into(), theme::WARNING);
        }
        Status::Failed(e) => return (e.clone(), theme::DANGER),
        Status::Cancelled => parts.push("annulé".into()),
    }

    if let Some(start) = p.started.filter(|_| p.status.is_active()) {
        parts.push(format!("écoulé {}", timecode::format(start.instant.elapsed().as_secs_f64())));
    }
    (parts.join("  ·  "), theme::MUTED)
}

fn download_label(options: &Options, meta: &Meta, clip: Option<&Clip>) -> String {
    if let Some(count) = meta.playlist_count {
        return format!("Télécharger la playlist ({count} vidéos)");
    }
    let what = match options.mode {
        Mode::Audio => "l'audio",
        Mode::Video => "la vidéo",
    };
    match clip {
        Some(c) if c.end > c.start => {
            format!("Télécharger l'extrait ({} – {})", timecode::format(c.start), timecode::format(c.end))
        }
        _ => format!("Télécharger {what}"),
    }
}

fn quality_label(q: Option<u32>) -> String {
    match q {
        None => "Meilleure qualité".to_string(),
        Some(4320) => "8K (4320p)".to_string(),
        Some(2160) => "4K (2160p)".to_string(),
        Some(1440) => "2K (1440p)".to_string(),
        Some(1080) => "Full HD (1080p)".to_string(),
        Some(720) => "HD (720p)".to_string(),
        Some(h) => format!("{h}p"),
    }
}

fn human_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1} k", n as f64 / 1e3).replace(".0 ", " ").replace('.', ","),
        1_000_000..=999_999_999 => format!("{:.1} M", n as f64 / 1e6).replace(".0 ", " ").replace('.', ","),
        _ => format!("{:.1} Md", n as f64 / 1e9).replace(".0 ", " ").replace('.', ","),
    }
}

fn human_bytes(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["o", "Ko", "Mo", "Go"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

/// Lance la commande macOS `open` (dossier, ou `-R fichier` pour le montrer dans le Finder).
fn open<S: AsRef<std::ffi::OsStr>>(args: &[S]) {
    let _ = Command::new("open").args(args).spawn();
}
