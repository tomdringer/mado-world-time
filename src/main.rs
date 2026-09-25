// mado-world-time — pixel plugin for Mado sidebar
// Shows 4 world city times with day/night indicator, date, and city name.

use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Timelike;
use chrono_tz::Tz;
use serde::Deserialize;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Clone)]
struct CityConfig {
    /// Display name, e.g. "Tokyo"
    name:     String,
    /// IANA timezone, e.g. "Asia/Tokyo"
    timezone: String,
    /// "12h" or "24h". Falls back to global setting.
    #[serde(default)]
    time_format: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WorldTimeConfig {
    /// "12h" or "24h". Default: "24h"
    time_format: String,
    /// Up to 4 cities. Defaults provided if missing.
    cities: Vec<CityConfig>,
}

impl WorldTimeConfig {
    fn load() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let path = std::path::Path::new(&home)
            .join(".config/mado/plugins/world-time.toml");
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let mut cfg: WorldTimeConfig = toml::from_str(&content).unwrap_or_default();

        if cfg.cities.is_empty() {
            cfg.cities = vec![
                CityConfig { name: "New York".into(),  timezone: "America/New_York".into(), time_format: String::new() },
                CityConfig { name: "London".into(),    timezone: "Europe/London".into(),    time_format: String::new() },
                CityConfig { name: "Dubai".into(),     timezone: "Asia/Dubai".into(),       time_format: String::new() },
                CityConfig { name: "Tokyo".into(),     timezone: "Asia/Tokyo".into(),       time_format: String::new() },
            ];
        }
        cfg.cities.truncate(4);
        cfg
    }

    fn time_fmt<'a>(&'a self, city: &'a CityConfig) -> &'a str {
        let fmt = if city.time_format.is_empty() { &self.time_format } else { &city.time_format };
        if fmt == "12h" { "%-I:%M %p" } else { "%H:%M" }
    }
}

// ── Palette ───────────────────────────────────────────────────────────────────

const BG:      [u8; 4] = [15,  23,  42,  255]; // slate-900
const DIVIDER: [u8; 4] = [30,  41,  59,  255]; // slate-800
const TIME:    [u8; 4] = [248, 250, 252, 255]; // slate-50
const CITY:    [u8; 4] = [203, 213, 225, 255]; // slate-300
const DATE:    [u8; 4] = [148, 163, 184, 255]; // slate-400
const SUN:     [u8; 4] = [251, 191,  36, 255]; // amber-400
const MOON:    [u8; 4] = [148, 163, 184, 255]; // slate-400

// nf-fa-sun / nf-fa-moon (Font Awesome via Nerd Fonts)
const ICON_SUN:  &str = "\u{F185}";
const ICON_MOON: &str = "\u{F186}";

/// Day if hour is 6..=19, night otherwise.
fn is_day(hour: u32) -> bool {
    (6..=19).contains(&hour)
}

// ── Font loading ──────────────────────────────────────────────────────────────

static NERD_FONT_BYTES: &[u8] =
    include_bytes!("../assets/HackNerdFontMono-Regular.ttf");

fn load_nerd_font() -> fontdue::Font {
    fontdue::Font::from_bytes(NERD_FONT_BYTES, fontdue::FontSettings::default())
        .expect("mado-world-time: bundled Nerd Font load failed")
}

fn load_system_font() -> Option<fontdue::Font> {
    let candidates = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/Library/Fonts/Arial.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
        "C:\\Windows\\Fonts\\segoeui.ttf",
    ];
    for path in &candidates {
        if let Ok(data) = std::fs::read(path) {
            if let Ok(f) = fontdue::Font::from_bytes(
                data.as_slice(), fontdue::FontSettings::default()) {
                return Some(f);
            }
        }
    }
    None
}

// ── Canvas ────────────────────────────────────────────────────────────────────

struct Canvas { pixels: Vec<u8>, w: usize, h: usize }

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        let mut pixels = vec![0u8; w * h * 4];
        for px in pixels.chunks_exact_mut(4) { px.copy_from_slice(&BG); }
        Canvas { pixels, w, h }
    }

    fn blend(&mut self, x: usize, y: usize, color: [u8; 4], alpha: f32) {
        if x >= self.w || y >= self.h { return; }
        let i = (y * self.w + x) * 4;
        let ia = 1.0 - alpha;
        for c in 0..3 {
            self.pixels[i + c] =
                (self.pixels[i + c] as f32 * ia + color[c] as f32 * alpha).round() as u8;
        }
        self.pixels[i + 3] = 255;
    }

    /// Draw a horizontal divider line at y.
    fn hline(&mut self, y: usize, color: [u8; 4]) {
        if y >= self.h { return; }
        for x in 0..self.w {
            let i = (y * self.w + x) * 4;
            self.pixels[i..i + 4].copy_from_slice(&color);
        }
    }

    fn text(&mut self, font: &fontdue::Font, text: &str,
            size: f32, x: usize, y: usize, color: [u8; 4]) -> usize {
        let mut cx = x;
        for ch in text.chars() {
            if font.lookup_glyph_index(ch) == 0 { continue; }
            let (m, bmp) = font.rasterize(ch, size);
            let gx = cx as isize + m.xmin as isize;
            let gy = y as isize - m.height as isize - m.ymin as isize;
            for (k, &cov) in bmp.iter().enumerate() {
                if cov == 0 { continue; }
                let px = gx + (k % m.width) as isize;
                let py = gy + (k / m.width) as isize;
                if px >= 0 && py >= 0 {
                    self.blend(px as usize, py as usize, color, cov as f32 / 255.0);
                }
            }
            cx += m.advance_width.round() as usize;
        }
        cx
    }

    fn measure(font: &fontdue::Font, text: &str, size: f32) -> usize {
        text.chars().map(|ch| {
            if font.lookup_glyph_index(ch) == 0 { return 0; }
            let (m, _) = font.rasterize(ch, size);
            m.advance_width.round() as usize
        }).sum()
    }

    fn write_frame(&self, out: &mut impl Write) {
        out.write_all(b"MADO").unwrap();
        out.write_all(&(self.w as u32).to_le_bytes()).unwrap();
        out.write_all(&(self.h as u32).to_le_bytes()).unwrap();
        out.write_all(&self.pixels).unwrap();
        out.flush().unwrap();
    }
}

// ── Resize event ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind:   String,
    width:  Option<u32>,
    height: Option<u32>,
}

// ── Rendering ─────────────────────────────────────────────────────────────────

struct Fonts {
    nerd: fontdue::Font,
    body: fontdue::Font,
}

struct CitySnapshot {
    name:  String,
    time:  String,
    date:  String,
    day:   bool,
}

fn snapshot(cfg: &WorldTimeConfig) -> Vec<CitySnapshot> {
    cfg.cities.iter().map(|city| {
        let tz: Tz = city.timezone.parse().unwrap_or(chrono_tz::UTC);
        let now = chrono::Utc::now().with_timezone(&tz);
        CitySnapshot {
            name:  city.name.clone(),
            time:  now.format(cfg.time_fmt(city)).to_string(),
            date:  now.format("%-d %b").to_string(),
            day:   is_day(now.hour()),
        }
    }).collect()
}

fn render(canvas: &mut Canvas, fonts: &Fonts, cities: &[CitySnapshot]) {
    let w = canvas.w as f32;
    let h = canvas.h as f32;
    let n = cities.len().max(1);

    let row_h = h / n as f32;
    let pad   = (w * 0.05).max(6.0) as usize;

    // Font sizes scale with row height and panel width.
    let time_size = (row_h * 0.30).clamp(16.0, 44.0);
    let city_size = time_size;
    let date_size = (row_h * 0.16).clamp(10.0, 18.0);
    let icon_size = time_size;

    for (i, city) in cities.iter().enumerate() {
        let row_top    = (i as f32 * row_h) as usize;
        let row_bottom = ((i + 1) as f32 * row_h) as usize;
        let mid_y      = (row_top + row_bottom) / 2;

        // ── Layout: icon + city + time share one baseline; date sits below ──
        //
        //   [☀]  New York       14:30
        //                       12 Sep
        //
        // The main-line baseline and date are vertically centred as a block.
        let gap       = (time_size * 0.18) as usize;
        let block_h   = time_size as usize + gap + date_size as usize;
        let main_y    = mid_y.saturating_sub(block_h / 2) + time_size as usize;
        let date_y    = main_y + gap + date_size as usize;

        // ── Sun / Moon icon — left edge, on the main baseline ────────────
        let icon_glyph = if city.day { ICON_SUN } else { ICON_MOON };
        let icon_color = if city.day { SUN } else { MOON };
        canvas.text(&fonts.nerd, icon_glyph, icon_size, pad, main_y, icon_color);

        let icon_w = Canvas::measure(&fonts.nerd, icon_glyph, icon_size);
        let text_x = pad + icon_w + pad;

        // ── City name — immediately right of icon, same baseline ──────────
        canvas.text(&fonts.body, &city.name, city_size, text_x, main_y, CITY);

        // ── Time — right-aligned, same baseline as icon and city ─────────
        let avail_w = canvas.w.saturating_sub(text_x + pad);
        let time_tw = Canvas::measure(&fonts.body, &city.time, time_size);
        let time_x  = canvas.w.saturating_sub(pad + time_tw);
        canvas.text(&fonts.body, &city.time, time_size, time_x, main_y, TIME);

        // ── Date — right-aligned, below the time ─────────────────────────
        let date_tw = Canvas::measure(&fonts.body, &city.date, date_size);
        let date_x  = canvas.w.saturating_sub(pad + date_tw);
        canvas.text(&fonts.body, &city.date, date_size, date_x, date_y, DATE);

        // Divider between rows (skip last)
        if i + 1 < n {
            canvas.hline(row_bottom.saturating_sub(1), DIVIDER);
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let cfg = WorldTimeConfig::load();

    let nerd = load_nerd_font();
    let body = match load_system_font() {
        Some(f) => f,
        None => {
            eprintln!("mado-world-time: no system font found");
            std::process::exit(1);
        }
    };
    let fonts = Arc::new(Fonts { nerd, body });

    let dims: Arc<Mutex<(u32, u32)>> = Arc::new(Mutex::new((300, 400)));

    // Stdin event listener
    {
        let dims = Arc::clone(&dims);
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in BufReader::new(stdin.lock()).lines().flatten() {
                if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                    if ev.kind == "resize" {
                        if let (Some(w), Some(h)) = (ev.width, ev.height) {
                            if w > 0 && h > 0 { *dims.lock().unwrap() = (w, h); }
                        }
                    }
                }
            }
        });
    }

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    loop {
        let (w, h) = *dims.lock().unwrap();
        let (w, h) = (w as usize, h as usize);

        let cities = snapshot(&cfg);
        let mut canvas = Canvas::new(w, h);
        render(&mut canvas, &fonts, &cities);
        canvas.write_frame(&mut out);

        std::thread::sleep(Duration::from_secs(1));
    }
}
