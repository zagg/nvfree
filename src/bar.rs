//! bar.rs - le bare graph. Tout est rendu en caracteres pleine largeur d'une
//! colonne : le niveau de gris des blocs Unicode (█ ▒ ░) porte la lecture, la
//! couleur n'est qu'un renfort, et `--ascii` retombe sur # % .
//!
//! Precision : les frontieres sont calculees en huitiemes de colonne
//! (U+258F..U+2588), donc une barre de 40 colonnes a une resolution de 1/320
//! de la VRAM totale.

use crate::gpu::Mem;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seg {
    Used,
    Reserved,
    Free,
    /// du texte pose dans la barre (etiquette d'un processus) ; ni bloc, ni couleur de palier
    Label,
}

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub seg: Seg,
    pub ch: char,
}

/// Jeu de caracteres : mode Unicode (default) ou `--ascii`.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub ascii: bool,
}

const EIGHTHS: [char; 9] = [
    ' ', '\u{258f}', '\u{258e}', '\u{258d}', '\u{258c}', '\u{258b}', '\u{258a}', '\u{2589}',
    '\u{2588}',
];

impl Style {
    pub fn solid(&self, s: Seg) -> char {
        match (self.ascii, s) {
            (true, Seg::Used) => '#',
            (true, Seg::Reserved) => '+',
            (true, Seg::Free) => '-',
            (false, Seg::Used) => '\u{2588}',
            (false, Seg::Reserved) => '\u{2592}',
            (false, Seg::Free) => '\u{2591}',
            (_, Seg::Label) => ' ',
        }
    }

    pub fn partial(&self, s: Seg, eighths: usize) -> char {
        if self.ascii || s != Seg::Used {
            return self.solid(s);
        }
        EIGHTHS[eighths.min(8)]
    }

    pub fn empty(&self) -> char {
        if self.ascii {
            '.'
        } else {
            '\u{00b7}'
        }
    }
}

/// Repartition `width` colonnes entre utilise / reserve / libre.
pub fn gauge(width: usize, m: &Mem, style: &Style) -> Vec<Cell> {
    let mut out = Vec::with_capacity(width);
    if width == 0 {
        return out;
    }
    if !m.ok || m.total_kib == 0 {
        out.resize(
            width,
            Cell {
                seg: Seg::Free,
                ch: style.empty(),
            },
        );
        return out;
    }

    let units = (width * 8) as u128;
    let total = m.total_kib.max(1) as u128;
    let used = m.used_kib().min(m.total_kib) as u128;
    let res = m.reserved_kib() as u128;

    let b_used = (((used * units) as f64 / total as f64).round() as u128).min(units);
    let b_res = (b_used + (((res * units) as f64 / total as f64).round() as u128))
        .min(units)
        .max(b_used);

    let runs = [
        (Seg::Used, 0u128, b_used),
        (Seg::Reserved, b_used, b_res),
        (Seg::Free, b_res, units),
    ];

    for c in 0..width {
        let lo = (c * 8) as u128;
        let hi = lo + 8;
        let mut best = (Seg::Free, 0u128);
        for (seg, a, b) in runs {
            let ov = hi.min(b).saturating_sub(lo.max(a));
            if ov > best.1 {
                best = (seg, ov);
            }
        }
        let ch = if best.1 >= 8 {
            style.solid(best.0)
        } else {
            style.partial(best.0, best.1 as usize)
        };
        out.push(Cell { seg: best.0, ch });
    }
    out
}

/// Une part d'un tout : `part` colore en Utilise, le reste en Libre (pas de
/// segment reserve) - c'est l'echelle des barres de processus.
pub fn part(width: usize, part_kib: u64, total_kib: u64, style: &Style) -> Vec<Cell> {
    let p = part_kib.min(total_kib);
    let m = Mem {
        total_kib,
        heap_kib: total_kib,
        free_kib: total_kib - p,
        ok: total_kib > 0,
    };
    gauge(width, &m, style)
}

// ---------------------------------------------------------------- couleurs

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Off,
    Basic,
    Ext,
}

#[derive(Clone, Copy, Debug)]
pub struct Pal {
    pub mode: Mode,
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// vert -> jaune -> rouge selon la pression sur la VRAM
fn ramp(pct: f64) -> (u8, u8, u8) {
    let p = pct.clamp(0.0, 100.0) / 100.0;
    type Stop = (f64, f64, f64);
    let (g, y, r): (Stop, Stop, Stop) = (
        (74.0, 202.0, 116.0),
        (226.0, 199.0, 58.0),
        (236.0, 71.0, 71.0),
    );
    let (a, b, t) = if p < 0.6 {
        (g, y, p / 0.6)
    } else {
        (y, r, (p - 0.6) / 0.4)
    };
    (
        lerp(a.0, b.0, t) as u8,
        lerp(a.1, b.1, t) as u8,
        lerp(a.2, b.2, t) as u8,
    )
}

impl Pal {
    fn esc(&self, code: &str) -> String {
        if self.mode == Mode::Off {
            String::new()
        } else {
            format!("\x1b[{code}m")
        }
    }

    pub fn reset(&self) -> String {
        if self.mode == Mode::Off {
            String::new()
        } else {
            "\x1b[0m".to_string()
        }
    }

    pub fn dim(&self) -> String {
        self.esc("2")
    }

    /// un bloc de texte en gris atténuu (libellés de processus, graduations)
    pub fn dim_label(&self, s: &str) -> String {
        if self.mode == Mode::Off {
            s.to_string()
        } else {
            format!("{}{}{}", self.dim(), s, self.reset())
        }
    }

    pub fn bold(&self) -> String {
        self.esc("1")
    }

    pub fn used_fg(&self, pct: f64) -> String {
        match self.mode {
            Mode::Off => String::new(),
            Mode::Basic => {
                if pct < 55.0 {
                    self.esc("32")
                } else if pct < 80.0 {
                    self.esc("33")
                } else {
                    self.esc("1;31")
                }
            }
            Mode::Ext => {
                let (r, g, b) = ramp(pct);
                self.esc(&format!("38;2;{r};{g};{b}"))
            }
        }
    }

    pub fn fg(&self, s: Seg, pct: f64) -> String {
        match s {
            Seg::Label => self.esc("2"),
            Seg::Used => self.used_fg(pct),
            Seg::Reserved => match self.mode {
                Mode::Off => String::new(),
                Mode::Basic => self.esc("90"),
                Mode::Ext => self.esc("38;2;118;118;140"),
            },
            Seg::Free => match self.mode {
                Mode::Off => String::new(),
                Mode::Basic => self.esc("90"),
                Mode::Ext => self.esc("38;2;74;74;74"),
            },
        }
    }

    /// Colore une rangee de cellules en fusionnant les voisinages de meme style.
    pub fn paint(&self, cells: &[Cell], pct: f64) -> String {
        if self.mode == Mode::Off {
            return cells.iter().map(|c| c.ch).collect();
        }
        let mut s = String::new();
        let mut cur = String::new();
        let mut style: Option<String> = None;
        for c in cells {
            let fg = self.fg(c.seg, pct);
            if style.as_deref() != Some(fg.as_str()) {
                if let Some(st) = style.take() {
                    s.push_str(&st);
                    s.push_str(&cur);
                    s.push_str(&self.reset());
                }
                cur.clear();
                style = Some(fg);
            }
            cur.push(c.ch);
        }
        if let Some(st) = style {
            s.push_str(&st);
            s.push_str(&cur);
            s.push_str(&self.reset());
        }
        s
    }
}

// ------------------------------------------------------------------ regles

/// graduations 0/25/50/75/100 % sous la barre.
pub fn ruler(width: usize, style: &Style, pal: &Pal) -> String {
    if width < 12 {
        return String::new();
    }
    let fill = if style.ascii { '-' } else { '\u{2500}' };
    let mut line = vec![fill; width];
    let labels = ["0%", "25%", "50%", "75%", "100%"];
    let mut prev_end = 0usize;
    for (i, lab) in labels.iter().enumerate() {
        let frac = i as f64 / (labels.len() - 1) as f64;
        let ll = lab.chars().count();
        let mut at = ((frac * width as f64).round() as usize).min(width.saturating_sub(1));

        if at + ll > width {
            at = width.saturating_sub(ll);
        }
        if i > 0 && at < prev_end + 1 {
            at = prev_end + 1;
        }
        if at + ll > width {
            continue;
        }
        for (k, ch) in lab.chars().enumerate() {
            line[at + k] = ch;
        }
        prev_end = at + ll;
    }
    format!(
        "{}{}{}",
        pal.dim(),
        line.iter().collect::<String>(),
        pal.reset()
    )
}

// ------------------------------------------------------------------ unites

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Auto,
    Mib,
    Gib,
}

/// KiB -> texte. `Auto` bascule en GiB des qu'on depasse 1 GiB (like nvidia-smi
/// le ferait pour les grandes cartes, mais avec deux decimales pour que les
/// soldes se lisent d'un coup d'oeil).
pub fn fmt_kib(kib: u64, u: Unit) -> String {
    // les valeurs du pilote sont en KiB : MiB = /1024, GiB = /1024^2
    let gib = kib as f64 / (1024.0 * 1024.0);
    let mib = kib as f64 / 1024.0;
    match u {
        Unit::Mib => format!("{mib:.0} MiB"),
        Unit::Gib => format!("{gib:.2} GiB"),
        Unit::Auto => {
            if kib >= 1024 * 1024 {
                format!("{gib:.2} GiB")
            } else if kib >= 10 * 1024 {
                format!("{mib:.0} MiB")
            } else {
                format!("{mib:.1} MiB")
            }
        }
    }
}

/// Octets -> texte (pour les processus).
pub fn fmt_bytes(bytes: u64, u: Unit) -> String {
    fmt_kib(bytes / 1024, u)
}

pub fn fmt_pct(p: f64) -> String {
    format!("{:.1}%", p)
}
