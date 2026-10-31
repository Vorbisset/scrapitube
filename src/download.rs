//! Lancement de yt-dlp : analyse d'un lien, téléchargement et suivi de la progression.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::timecode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Container {
    Mp4,
    Mkv,
    Webm,
}

impl Container {
    pub const ALL: [Container; 3] = [Container::Mp4, Container::Mkv, Container::Webm];

    pub fn as_str(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::Webm => "webm",
        }
    }
}

/// Réglages d'un téléchargement (mémorisés d'une session à l'autre).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Options {
    pub mode: Mode,
    /// Hauteur maximale de la vidéo ; `None` = meilleure qualité disponible.
    pub max_height: Option<u32>,
    pub container: Container,
    pub subs: bool,
    pub no_playlist: bool,
    pub output_dir: PathBuf,
}

impl Default for Options {
    fn default() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        Self {
            mode: Mode::Video,
            max_height: None,
            container: Container::Mp4,
            subs: false,
            no_playlist: true,
            output_dir: home.join("Movies").join("YouTube"),
        }
    }
}

/// Extrait de la vidéo à télécharger, en secondes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    pub start: f64,
    pub end: f64,
}

/// Informations sur un lien, obtenues avec `yt-dlp -J` avant de télécharger.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub view_count: Option<u64>,
    /// Hauteurs vidéo disponibles, de la meilleure à la moins bonne.
    pub heights: Vec<u32>,
    /// Nombre de vidéos si le lien est une playlist.
    pub playlist_count: Option<usize>,
}

impl Meta {
    fn from_json(v: &Value) -> Self {
        let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
        let entries = v.get("entries").and_then(Value::as_array);
        let heights: BTreeSet<u32> = v
            .get("formats")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|f| f.get("vcodec").and_then(Value::as_str).is_some_and(|c| c != "none"))
            .filter_map(|f| f.get("height").and_then(Value::as_u64))
            .map(|h| h as u32)
            .collect();

        Self {
            title: text(v, "title").unwrap_or_else(|| "Sans titre".into()),
            uploader: text(v, "uploader").or_else(|| text(v, "channel")),
            duration: v.get("duration").and_then(Value::as_f64),
            thumbnail: best_thumbnail(v).or_else(|| entries.and_then(|e| e.first()).and_then(best_thumbnail)),
            view_count: v.get("view_count").and_then(Value::as_u64),
            heights: heights.into_iter().rev().collect(),
            playlist_count: (v.get("_type").and_then(Value::as_str) == Some("playlist"))
                .then(|| entries.map_or(0, Vec::len)),
        }
    }
}

fn best_thumbnail(v: &Value) -> Option<String> {
    v.get("thumbnail").and_then(Value::as_str).map(str::to_string).or_else(|| {
        // Dans la liste "thumbnails", yt-dlp range les miniatures de la moins bonne à la meilleure.
        v.get("thumbnails")?.as_array()?.iter().rev().find_map(|t| t.get("url")?.as_str().map(str::to_string))
    })
}

/// Analyse un lien sans rien télécharger (titre, miniature, durée, qualités…).
pub fn analyze(yt_dlp: &Path, url: &str, no_playlist: bool) -> Result<Meta, String> {
    let mut cmd = Command::new(yt_dlp);
    cmd.args(["-J", "--no-warnings", "--flat-playlist"]);
    if no_playlist {
        cmd.arg("--no-playlist");
    }
    let out = cmd.arg(url).output().map_err(|e| format!("impossible de lancer yt-dlp : {e}"))?;
    if !out.status.success() {
        return Err(error_message(String::from_utf8_lossy(&out.stderr).lines()));
    }
    let json: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("réponse de yt-dlp illisible : {e}"))?;
    Ok(Meta::from_json(&json))
}

/// Tout ce qu'il faut pour lancer (ou relancer) un téléchargement.
#[derive(Clone, Debug)]
pub struct Request {
    pub url: String,
    pub options: Options,
    pub section: Option<Section>,
    pub meta: Meta,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Queued,
    Starting,
    Downloading,
    Converting,
    Done,
    Failed(String),
    Cancelled,
}

impl Status {
    pub fn is_active(&self) -> bool {
        matches!(self, Status::Starting | Status::Downloading | Status::Converting)
    }

    pub fn is_finished(&self) -> bool {
        matches!(self, Status::Done | Status::Failed(_) | Status::Cancelled)
    }
}

/// Un instant, à la fois pour mesurer les durées et pour l'afficher (« 14:32 »).
#[derive(Clone, Copy, Debug)]
pub struct Stamp {
    pub instant: Instant,
    pub clock: DateTime<Local>,
}

impl Stamp {
    fn now() -> Self {
        Self { instant: Instant::now(), clock: Local::now() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    /// Vidéo en cours (utile pour suivre une playlist).
    pub current: Option<String>,
    pub status: Status,
    pub started: Option<Stamp>,
    pub finished: Option<Stamp>,
    /// Avancement global (vidéo + audio), entre 0 et 1.
    pub fraction: Option<f32>,
    /// Temps restant estimé, en secondes.
    pub eta: Option<f64>,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Débit en octets par seconde.
    pub speed: Option<f64>,
    /// Extrait : durée de l'extrait, position atteinte et vitesse de traitement (×).
    pub clip_len: Option<f64>,
    pub clip_done: Option<f64>,
    pub clip_speed: Option<f64>,
    pub files: Vec<PathBuf>,
    // Comptabilité des flux : YouTube envoie la vidéo puis l'audio, chacun avec sa propre progression.
    expected_size: Option<u64>,
    finished_streams: u64,
    stream_downloaded: u64,
    stream_total: Option<u64>,
}

impl Default for Status {
    fn default() -> Self {
        Status::Queued
    }
}

pub struct Job {
    pub request: Request,
    state: Arc<Mutex<Progress>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Job {
    pub fn new(request: Request) -> Self {
        let progress = Progress { clip_len: request.section.map(|s| s.end - s.start), ..Default::default() };
        Self { request, state: Arc::new(Mutex::new(progress)), child: Arc::new(Mutex::new(None)) }
    }

    pub fn snapshot(&self) -> Progress {
        self.state.lock().unwrap().clone()
    }

    pub fn start(&self, yt_dlp: PathBuf, ctx: eframe::egui::Context) {
        {
            let mut p = self.state.lock().unwrap();
            p.status = Status::Starting;
            p.started = Some(Stamp::now());
        }
        let request = self.request.clone();
        let (state, child) = (self.state.clone(), self.child.clone());

        thread::spawn(move || {
            let result = run(&yt_dlp, &request, &state, &child, &ctx);
            let mut p = state.lock().unwrap();
            p.finished = Some(Stamp::now());
            if p.status != Status::Cancelled {
                p.status = match result {
                    Ok(()) => Status::Done,
                    Err(e) => Status::Failed(e),
                };
            }
            ctx.request_repaint();
        });
    }

    pub fn cancel(&self) {
        self.state.lock().unwrap().status = Status::Cancelled;
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

/// Dossier où l'appli garde ses propres copies de yt-dlp et ffmpeg. Contrairement au paquet
/// .app, il est modifiable : yt-dlp peut donc s'y mettre à jour (`yt-dlp -U`).
pub fn tools_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/Scrapitube/bin")
}

/// Outils embarqués dans l'appli distribuée (Scrapitube.app/Contents/Resources/bin).
fn bundled_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?.join("Resources/bin");
    dir.is_dir().then_some(dir)
}

/// Copie les outils embarqués vers `tools_dir()` (à la première ouverture, ou quand l'appli
/// fournit un ffmpeg différent). yt-dlp n'est copié qu'une fois : ensuite il se met à jour seul.
pub fn install_bundled_tools() {
    let Some(src) = bundled_dir() else { return };
    let dst = tools_dir();
    if std::fs::create_dir_all(&dst).is_err() {
        return;
    }
    for name in ["yt-dlp", "ffmpeg"] {
        let (from, to) = (src.join(name), dst.join(name));
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        let outdated = name == "ffmpeg" && size(&from) != size(&to);
        if from.exists() && (!to.exists() || outdated) && std::fs::copy(&from, &to).is_ok() {
            // L'utilisateur a déjà approuvé l'appli : sa copie ne doit pas être bloquée une seconde fois.
            let _ = Command::new("xattr").args(["-d", "com.apple.quarantine"]).arg(&to).output();
        }
    }
}

/// ffmpeg fourni avec l'appli, s'il y en a un ; sinon yt-dlp cherche celui du système.
fn ffmpeg_location() -> Option<&'static Path> {
    static FFMPEG: OnceLock<Option<PathBuf>> = OnceLock::new();
    FFMPEG.get_or_init(|| Some(tools_dir().join("ffmpeg")).filter(|p| p.exists())).as_deref()
}

pub fn has_ffmpeg() -> bool {
    ffmpeg_location().is_some() || Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

/// Met à jour la copie de yt-dlp gérée par l'appli et renvoie un message pour l'utilisateur.
pub fn update_yt_dlp(yt_dlp: &Path) -> String {
    if !yt_dlp.starts_with(tools_dir()) {
        return "Ce yt-dlp vient de Homebrew : mets-le à jour avec « brew upgrade yt-dlp ».".into();
    }
    match Command::new(yt_dlp).arg("-U").output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.contains("up to date") { "yt-dlp est déjà à jour.".into() } else { "yt-dlp a été mis à jour.".into() }
        }
        Ok(out) => error_message(String::from_utf8_lossy(&out.stderr).lines()),
        Err(e) => format!("impossible de lancer yt-dlp : {e}"),
    }
}

/// Cherche yt-dlp : la copie de l'appli d'abord, puis Homebrew (lancée depuis le Finder,
/// l'appli a un PATH minimal), et renvoie son chemin et sa version.
pub fn find_yt_dlp() -> Option<(PathBuf, String)> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let candidates = [
        tools_dir().join("yt-dlp"),
        PathBuf::from("/opt/homebrew/bin/yt-dlp"),
        PathBuf::from("/usr/local/bin/yt-dlp"),
        home.join(".local/bin/yt-dlp"),
        PathBuf::from("yt-dlp"),
    ];
    candidates.into_iter().find_map(|bin| {
        let out = Command::new(&bin).arg("--version").output().ok()?;
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        out.status.success().then_some((bin, version))
    })
}

// Préfixes des lignes que l'on demande à yt-dlp d'imprimer, pour les reconnaître.
const TITLE_TAG: &str = "__TITLE__";
const PROGRESS_TAG: &str = "__PROG__";
const FILE_TAG: &str = "__FILE__";
const SIZE_TAG: &str = "__SIZE__";

fn build_command(yt_dlp: &Path, request: &Request) -> Command {
    let options = &request.options;
    let mut cmd = Command::new(yt_dlp);

    // Un extrait porte ses bornes dans le nom, pour ne pas écraser la vidéo complète.
    let suffix = request
        .section
        .map(|s| format!(" (extrait {}-{})", timecode::for_filename(s.start), timecode::for_filename(s.end)))
        .unwrap_or_default();
    let template = options.output_dir.join(format!("%(title)s [%(id)s]{suffix}.%(ext)s"));

    // --print rend yt-dlp silencieux : --progress réactive les lignes de progression.
    cmd.arg("--newline")
        .arg("--progress")
        .arg("--no-warnings")
        .arg("--no-simulate")
        .args(["-o", &template.to_string_lossy()])
        .args(["--print", &format!("before_dl:{TITLE_TAG}%(title)s")])
        // Taille totale prévue (vidéo + audio), pour un pourcentage global.
        .args(["--print", &format!("before_dl:{SIZE_TAG}%(filesize,filesize_approx)s")])
        .args(["--print", &format!("after_move:{FILE_TAG}%(filepath)s")])
        .args([
            "--progress-template",
            &format!(
                "download:{PROGRESS_TAG}%(progress.downloaded_bytes)s|%(progress.total_bytes)s|\
                 %(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s"
            ),
        ]);

    match options.mode {
        Mode::Audio => {
            cmd.args(["-f", "bestaudio/best", "-x", "--audio-format", "mp3", "--audio-quality", "0"]);
        }
        Mode::Video => {
            let height = options.max_height.map(|h| format!("[height<={h}]")).unwrap_or_default();
            cmd.args(["-f", &format!("bv*{height}+ba/b{height}")])
                .args(["--merge-output-format", options.container.as_str()]);
        }
    }

    if let Some(section) = request.section {
        // Coupe aux images clés demandées (ré-encodage des bords) pour que l'extrait soit précis.
        cmd.args(["--download-sections", &format!("*{}-{}", section.start, section.end)])
            .arg("--force-keyframes-at-cuts")
            // ffmpeg découpe l'extrait sans que yt-dlp ne donne de progression : on lui demande
            // d'écrire la sienne (lignes « out_time=… », « speed=… ») sur la sortie standard.
            .args(["--downloader-args", "ffmpeg_i:-progress pipe:1 -nostats"]);
    }
    if let Some(ffmpeg) = ffmpeg_location() {
        cmd.arg("--ffmpeg-location").arg(ffmpeg);
    }
    if options.subs {
        cmd.args(["--write-subs", "--sub-langs", "fr,en", "--embed-subs"]);
    }
    if options.no_playlist {
        cmd.arg("--no-playlist");
    }

    cmd.arg(&request.url);
    cmd
}

fn run(
    yt_dlp: &Path,
    request: &Request,
    state: &Mutex<Progress>,
    child_slot: &Mutex<Option<Child>>,
    ctx: &eframe::egui::Context,
) -> Result<(), String> {
    let output_dir = &request.options.output_dir;
    std::fs::create_dir_all(output_dir).map_err(|e| format!("impossible de créer {} : {e}", output_dir.display()))?;

    let mut child = build_command(yt_dlp, request)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("impossible de lancer yt-dlp : {e}"))?;
    let stdout = child.stdout.take().expect("stdout capturé");
    let stderr = child.stderr.take().expect("stderr capturé");
    *child_slot.lock().unwrap() = Some(child);

    // stderr est lu dans un thread à part pour ne pas bloquer yt-dlp.
    let stderr_thread =
        thread::spawn(move || BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<_>>());

    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        handle_line(&line, &mut state.lock().unwrap());
        ctx.request_repaint();
    }

    let child = child_slot.lock().unwrap().take();
    let status = child.map(|mut c| c.wait()).transpose().map_err(|e| e.to_string())?;
    let stderr = stderr_thread.join().unwrap_or_default();

    match status {
        Some(s) if s.success() => Ok(()),
        _ => Err(error_message(stderr.iter().map(String::as_str))),
    }
}

/// Garde les lignes "ERROR:" de yt-dlp, sans le préfixe.
fn error_message<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let errors: Vec<&str> = lines.filter_map(|l| l.strip_prefix("ERROR: ")).collect();
    if errors.is_empty() {
        "yt-dlp s'est arrêté sans donner de raison".into()
    } else {
        errors.join("\n")
    }
}

fn handle_line(line: &str, p: &mut Progress) {
    if let Some(title) = line.strip_prefix(TITLE_TAG) {
        // Nouvelle vidéo (une seule, ou la suivante d'une playlist) : on repart de zéro.
        *p = Progress {
            current: Some(title.to_string()),
            status: Status::Downloading,
            started: p.started,
            clip_len: p.clip_len,
            files: std::mem::take(&mut p.files),
            ..Default::default()
        };
    } else if let Some(size) = line.strip_prefix(SIZE_TAG) {
        p.expected_size = size.parse::<f64>().ok().map(|s| s as u64);
    } else if let Some(progress) = line.strip_prefix(PROGRESS_TAG) {
        if p.clip_len.is_some() {
            // Extrait : la progression vient de ffmpeg ; yt-dlp n'envoie qu'une ligne finale.
            p.status = Status::Converting;
            return;
        }
        // "téléchargé|total|total_estimé|vitesse|eta", avec "NA" pour les valeurs inconnues.
        let fields: Vec<Option<f64>> = progress.split('|').map(|s| s.parse().ok()).collect();
        let field = |i: usize| fields.get(i).copied().flatten();
        let downloaded = field(0).unwrap_or(0.0) as u64;
        if downloaded < p.stream_downloaded {
            // Le compteur repart à zéro : le flux précédent (la vidéo) est terminé, l'audio commence.
            p.finished_streams += p.stream_total.unwrap_or(p.stream_downloaded);
        }
        p.stream_downloaded = downloaded;
        p.stream_total = field(1).or(field(2)).map(|t| t as u64);

        p.downloaded = p.finished_streams + downloaded;
        // La taille prévue est approximative : si on la dépasse, on se rabat sur le flux en cours.
        p.total = p
            .expected_size
            .filter(|&s| s >= p.downloaded)
            .or(p.stream_total.map(|t| p.finished_streams + t));
        p.speed = field(3);
        p.fraction = p.total.filter(|&t| t > 0).map(|t| (p.downloaded as f32 / t as f32).min(1.0));
        p.eta = match (p.total, p.speed) {
            (Some(total), Some(speed)) if speed > 0.0 => Some(total.saturating_sub(p.downloaded) as f64 / speed),
            _ => field(4),
        };
        // yt-dlp ne signale pas la fusion / conversion (mode silencieux imposé par --print) :
        // une fois presque tout reçu, on passe en « finalisation » jusqu'au fichier final.
        let stream_complete = p.stream_total.is_some_and(|t| downloaded >= t);
        let nearly_all = p.fraction.is_some_and(|f| f >= 0.97);
        p.status = if stream_complete && nearly_all { Status::Converting } else { Status::Downloading };
    } else if let Some(path) = line.strip_prefix(FILE_TAG) {
        p.files.push(PathBuf::from(path));
    } else if let Some(len) = p.clip_len {
        handle_ffmpeg_line(line, p, len);
    }
}

/// Lignes de progression de ffmpeg (« clé=valeur ») lors du téléchargement d'un extrait.
fn handle_ffmpeg_line(line: &str, p: &mut Progress, len: f64) {
    let Some((key, value)) = line.split_once('=') else { return };
    match key {
        "out_time" => {
            let Some(done) = timecode::parse(value) else { return };
            let done = done.min(len);
            p.status = Status::Downloading;
            p.clip_done = Some(done);
            p.fraction = (len > 0.0).then(|| (done / len) as f32);
            p.eta = p.clip_speed.filter(|&s| s > 0.0).map(|s| (len - done) / s);
        }
        "speed" => p.clip_speed = value.trim().trim_end_matches('x').parse().ok(),
        "total_size" => p.downloaded = value.parse().unwrap_or(p.downloaded),
        "progress" if value == "end" => p.status = Status::Converting,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(p: &mut Progress, lines: &[&str]) {
        for line in lines {
            handle_line(line, p);
        }
    }

    #[test]
    fn overall_progress_spans_video_and_audio() {
        let mut p = Progress::default();
        feed(&mut p, &["__TITLE__Vidéo", "__SIZE__1000", "__PROG__400|800|NA|100|NA"]);
        assert_eq!(p.fraction, Some(0.4));
        assert_eq!(p.eta, Some(6.0));
        // Vidéo terminée, l'audio repart de zéro : l'avancement global continue.
        feed(&mut p, &["__PROG__800|800|NA|100|NA", "__PROG__100|200|NA|100|NA"]);
        assert_eq!(p.downloaded, 900);
        assert_eq!(p.fraction, Some(0.9));
        assert_eq!(p.status, Status::Downloading);
        feed(&mut p, &["__PROG__200|200|NA|100|NA"]);
        assert_eq!(p.status, Status::Converting);
    }

    #[test]
    fn clip_progress_comes_from_ffmpeg() {
        let mut p = Progress { clip_len: Some(90.0), ..Default::default() };
        feed(&mut p, &["__TITLE__Extrait", "out_time=N/A", "speed=2.5x", "out_time=00:00:45.000000"]);
        assert_eq!(p.fraction, Some(0.5));
        assert_eq!(p.eta, Some(18.0));
        feed(&mut p, &["progress=end", "__PROG__1000|1000|NA|NA|NA"]);
        assert_eq!(p.status, Status::Converting);
    }
}
