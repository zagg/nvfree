//! render.rs - mise en page du bare graph.
//!
//! Modes :
//!   * `block` (défaut) : par GPU, titre + barre + graduation + légende, puis
//!     les processus qui tiennent de la VRAM, sur la même échelle que la barre ;
//!   * `compact_line` (`-1`) : une ligne par GPU ;
//!   * `json_gpu` (`--json`) : instantané machine, aucun dessin.
//!
//! Toutes les largeurs sont comptées en COLONNES visibles : les codes ANSI sont
//! exclus du décompte, donc aucune ligne ne dépasse `cols` et le re-rubanement
//! des frames en mode `--loop` (remonter le curseur) reste exact.
//!
//! Les chaînes affichées sont en anglais (l'outil est public) ; les commentaires
//! restent en français, comme dans le reste du projet.

use crate::bar::{fmt_bytes, fmt_kib, fmt_pct, gauge, part, Cell, Pal, Seg, Style};
use crate::cli::{pair, pair_short, Opts};
use crate::gpu::{Gpu, Mem};

/// la barre, la graduation et la légende démarrent à cette colonne
const IND: &str = "  ";

fn cc(s: &str) -> usize {
    s.chars().count()
}

/// troncature a `w` colonnes visibles, avec point de suspension
fn clip(s: &str, w: usize) -> String {
    if cc(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    if w == 1 {
        return "\u{2026}".to_string();
    }
    let mut out: String = s.chars().take(w - 1).collect();
    out.push('\u{2026}');
    out
}

/// ne conserve que `cols` colonnes visibles (les codes ANSI ne comptent pas)
fn clip_ansi(s: &str, cols: usize) -> String {
    if cc(s) <= cols {
        return s.to_string();
    }
    let mut kept = 0usize;
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            let mut esc = String::from(c);
            while let Some(&n) = it.peek() {
                it.next();
                esc.push(n);
                if n == 'm' {
                    break;
                }
            }
            out.push_str(&esc);
            continue;
        }
        if kept >= cols {
            break;
        }
        out.push(c);
        kept += 1;
    }
    out
}

/// le nom brut vient de /proc/<pid>/comm ou de cmdline : on garde le binaire
fn basename(s: &str) -> String {
    let t = s.trim();
    let first = t.split('\0').next().unwrap_or(t);
    let first = first.split_whitespace().next().unwrap_or(first);
    let base = first.rsplit('/').next().unwrap_or(first);
    if base.is_empty() {
        first.to_string()
    } else {
        base.to_string()
    }
}

fn num_of(s: &str) -> Option<f64> {
    let t: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    t.parse().ok()
}

/// champs secondaires (pstate, température, puissance, occupation) reformate :
/// la lib rend des chaines deja formatées ("P2", "77", "106.76 W", "100 %").
pub fn extras_str(g: &Gpu, st: &Style) -> String {
    let sep = if st.ascii { " | " } else { " \u{b7} " };
    let deg = if st.ascii { "C" } else { "\u{b0}C" };
    let mut bits: Vec<String> = Vec::new();
    for (k, v) in &g.extra {
        match k.as_str() {
            "pstate" => bits.push(v.clone()),
            "temperature.gpu" => {
                if let Some(x) = num_of(v) {
                    bits.push(format!("{x:.0}{deg}"));
                }
            }
            "power.draw" => {
                if let Some(x) = num_of(v) {
                    bits.push(format!("{x:.0}W"));
                }
            }
            "utilization.gpu" => {
                if let Some(x) = num_of(v) {
                    bits.push(format!("gpu {x:.0}%"));
                }
            }
            "utilization.memory" => {
                if let Some(x) = num_of(v) {
                    bits.push(format!("mem {x:.0}%"));
                }
            }
            _ => bits.push(v.clone()),
        }
    }
    bits.join(sep)
}

/// "gauche ................ droite" : la partie gauche est tronquee si nécessaire
fn lr(left: &str, left_plain: usize, right: &str, right_plain: usize, cols: usize) -> String {
    let budget = cols.saturating_sub(right_plain + 1);
    let (l, lp) = if left_plain > budget {
        (clip_ansi(left, budget), budget.min(left_plain))
    } else {
        (left.to_string(), left_plain)
    };
    let gap = cols.saturating_sub(lp + right_plain);
    format!("{l}{}{right}", " ".repeat(gap))
}

/// en-tête : titre + provenance des données, version du pilote, horodatage
pub fn header(o: &Opts, driver: &str, now: &str, cols: usize, pal: &Pal) -> Vec<String> {
    if o.quiet {
        return Vec::new();
    }
    let left = "nvfree \u{b7} VRAM";
    // le sous-titre est la premiere chose qu'on sacrifie : on prend la variante
    // la plus longue qui tient vraiment, comble sinon comble.
    let right = if !driver.is_empty() && cols >= 74 {
        format!("driver {driver}  {now}")
    } else {
        now.to_string()
    };
    let room = cols.saturating_sub(cc(&right) + cc(left) + 2);
    let sub = [
        "read straight from the driver - no NVML, no nvidia-smi",
        "driver-direct, no NVML, no nvidia-smi",
        "no NVML",
        "",
    ]
    .into_iter()
    .find(|t| cc(t) <= room)
    .unwrap_or("");
    let plain_l = cc(left) + if sub.is_empty() { 0 } else { 1 + cc(sub) };
    let rendered = if sub.is_empty() {
        format!("{}{}{}", pal.bold(), left, pal.reset())
    } else {
        format!(
            "{}{}{}{}{}{}",
            pal.bold(),
            left,
            pal.reset(),
            " ",
            pal.dim_label(sub),
            pal.reset()
        )
    };
    let mut out = vec![lr(&rendered, plain_l, &right, cc(&right), cols)];
    let rule = if o.ascii {
        "-".repeat(cols)
    } else {
        "\u{2500}".repeat(cols)
    };
    out.push(format!("{}{}{}", pal.dim(), rule, pal.reset()));
    out
}

/// ligne de synthèse quand il y a plusieurs GPU
pub fn total_line(gpus: &[Gpu], o: &Opts, cols: usize, pal: &Pal, st: &Style) -> String {
    let (mut used, mut res, mut total) = (0u64, 0u64, 0u64);
    for g in gpus {
        if !g.mem.ok {
            continue;
        }
        used += g.mem.used_kib();
        res += g.mem.reserved_kib();
        total += g.mem.total_kib;
    }
    if total == 0 {
        return String::new();
    }
    let free = total.saturating_sub(used + res);
    let m = Mem {
        total_kib: total,
        heap_kib: used + res,
        free_kib: free,
        ok: true,
    };
    let left = format!(
        "{IND}TOTAL \u{b7} {} GPU{}",
        gpus.len(),
        if gpus.len() > 1 { "S" } else { "" }
    );
    let nums = pair(used, total, o.unit);
    let right_plain = format!("{nums}  {}", fmt_pct(m.pct_used()));
    let right = format!(
        "{nums}  {}{}{}",
        pal.used_fg(m.pct_used()),
        fmt_pct(m.pct_used()),
        pal.reset()
    );
    let barw = cols
        .saturating_sub(cc(&left) + cc(&right_plain) + 4)
        .clamp(6, cols);
    let cells = gauge(barw, &m, st);
    clip_ansi(
        &format!("{left}  {}  {right}", pal.paint(&cells, m.pct_used())),
        cols,
    )
}

/// le bloc complet d'un GPU
pub fn block(g: &Gpu, o: &Opts, cols: usize, pal: &Pal, st: &Style) -> Vec<String> {
    let mut out = Vec::new();
    let ind_w = cc(IND);
    let barw = cols.saturating_sub(ind_w).max(8);

    if !g.mem.ok {
        let l = format!("{IND}GPU {}  {}", g.idx, g.name);
        out.push(clip(&l, cols));
        out.push(clip_ansi(
            &format!(
                "{IND}{}VRAM unavailable: FB_GET_INFO_V2 did not answer{}",
                pal.dim(),
                pal.reset()
            ),
            cols,
        ));
        return out;
    }

    let used = g.mem.used_kib();
    let res = g.mem.reserved_kib();
    let free = g.mem.avail_kib();
    let total = g.mem.total_kib;
    let pct = g.mem.pct_used();

    // 1. nom + soldes
    let left = format!("{IND}GPU {}  {}", g.idx, g.name);
    let mut nums = pair(used, total, o.unit);
    if cc(&left) + cc(&nums) + cc(&fmt_pct(pct)) + 6 > cols {
        // terminal etroit : la version courante est tronquee, on prend la forme courte
        nums = pair_short(used, total);
    }
    let right_plain = format!("{nums}  {}", fmt_pct(pct));
    let right = format!(
        "{nums}  {}{}{}",
        pal.used_fg(pct),
        fmt_pct(pct),
        pal.reset()
    );
    out.push(lr(&left, cc(&left), &right, cc(&right_plain), cols));

    // 2. le bare graph
    let cells = gauge(barw, &g.mem, st);
    out.push(format!("{IND}{}", pal.paint(&cells, pct)));

    // 3. graduation 0/25/50/75/100 %
    if o.ticks {
        out.push(format!("{IND}{}", crate::bar::ruler(barw, st, pal)));
    }

    // 4. légende + meta
    if !o.quiet {
        let legend_plain = legend_plain(o, used, res, free);
        let legend = legend(o, pal, st, used, res, free, total);
        let meta = extras_str(g, st);
        let mp = cc(&meta);
        let lp = cc(&legend_plain);
        if meta.is_empty() || lp + 3 + mp > cols - ind_w {
            out.push(format!("{IND}{legend}"));
            if !meta.is_empty() {
                out.push(format!("{IND}{}{meta}{}", pal.dim(), pal.reset()));
            }
        } else {
            let gap = cols - ind_w - lp - mp;
            out.push(format!(
                "{IND}{legend}{}{}{meta}{}",
                " ".repeat(gap),
                pal.dim(),
                pal.reset()
            ));
        }
    }

    // 5. qui tient quoi en VRAM (même échelle que la barre principale)
    if o.procs > 0 && !g.procs.is_empty() {
        out.extend(procs_lines(g, o, cols, pal, st, barw));
    }
    out
}

fn legend_plain(o: &Opts, used: u64, res: u64, free: u64) -> String {
    format!(
        "\u{2588} used {}  \u{b7}  \u{2592} reserved {}  \u{b7}  \u{2591} free {}",
        fmt_kib(used, o.unit),
        fmt_kib(res, o.unit),
        fmt_kib(free, o.unit)
    )
}

fn legend(o: &Opts, pal: &Pal, st: &Style, used: u64, res: u64, free: u64, total: u64) -> String {
    let sep = if st.ascii { ", " } else { "  \u{b7}  " };
    let share = |v: u64| 100.0 * v as f64 / (total.max(1) as f64);
    let key = |s: Seg, txt: &str, v: u64| -> String {
        format!(
            "{}{}{} {}{}",
            pal.fg(s, share(v)),
            st.solid(s),
            pal.reset(),
            txt,
            fmt_kib(v, o.unit)
        )
    };
    format!(
        "{}{}{}{}{}",
        key(Seg::Used, "used ", used),
        sep,
        key(Seg::Reserved, "reserved ", res),
        sep,
        key(Seg::Free, "free ", free)
    )
}

fn used_cells(cells: &[Cell]) -> usize {
    cells.iter().take_while(|c| c.seg == Seg::Used).count()
}

/// Une ligne par processus : la barre est a la meme echelle que celle du GPU,
/// et le libelle est pose dans la partie libre (il mord sur la barre si la
/// VRAM est saturee, ce qui est exactement le cas ou cette info compte).
fn procs_lines(g: &Gpu, o: &Opts, cols: usize, pal: &Pal, st: &Style, barw: usize) -> Vec<String> {
    let mut out = Vec::new();
    let total = g.mem.total_kib.max(1);
    let shown = o.procs.min(g.procs.len());
    let pct = g.mem.pct_used();
    let mut hidden: u64 = 0;
    let mut hidden_n = 0usize;
    for p in g.procs.iter().skip(shown) {
        hidden += p.bytes / 1024;
        hidden_n += 1;
    }
    for p in g.procs.iter().take(shown) {
        let pkib = p.bytes / 1024;
        let share = if total > 0 {
            format!("{:.0}%", 100.0 * pkib as f64 / total as f64)
        } else {
            String::new()
        };
        let label = format!(
            " {} {} \u{b7} {} \u{b7} {}{}",
            p.pid,
            basename(&p.name),
            share,
            fmt_bytes(p.bytes, o.unit),
            if p.compute { "" } else { " \u{b7} gfx" }
        );
        let lab = clip(&label, barw.saturating_sub(2));
        let labw = cc(&lab);
        let mut cells = part(barw, pkib, total, st);
        let u = used_cells(&cells);
        let at = if barw - u >= labw {
            u
        } else {
            barw.saturating_sub(labw)
        };
        for (i, ch) in lab.chars().enumerate() {
            if at + i >= barw {
                break;
            }
            cells[at + i].ch = ch;
            cells[at + i].seg = Seg::Label;
        }
        out.push(format!("{IND}{}", pal.paint(&cells, pct)));
    }
    if hidden_n > 0 {
        let txt = format!(
            "  +{hidden_n} more \u{b7} {} \u{b7} {:.1}%",
            fmt_kib(hidden, o.unit),
            if total > 0 {
                100.0 * hidden as f64 / total as f64
            } else {
                0.0
            }
        );
        let mut cells = part(barw, hidden, total, st);
        let u = used_cells(&cells);
        let lab = clip(&txt, barw.saturating_sub(2));
        let labw = cc(&lab);
        let at = if barw - u >= labw {
            u
        } else {
            barw.saturating_sub(labw)
        };
        for (i, ch) in lab.chars().enumerate() {
            if at + i >= barw {
                break;
            }
            cells[at + i].ch = ch;
            cells[at + i].seg = Seg::Label;
        }
        out.push(format!("{IND}{}", pal.paint(&cells, pct)));
    }
    let _ = cols;
    out
}

/// `-1/--compact` : une ligne par GPU
pub fn compact_line(g: &Gpu, o: &Opts, cols: usize, pal: &Pal, st: &Style) -> String {
    if !g.mem.ok {
        return clip(&format!("{}  {}", g.idx, g.name), cols);
    }
    let left = format!("{:>2}  {}", g.idx, g.name);
    let nums = pair(g.mem.used_kib(), g.mem.total_kib, o.unit);
    let right_plain = format!("{}  {}", nums, fmt_pct(g.mem.pct_used()));
    let right = format!(
        "{nums}  {}{}{}",
        pal.used_fg(g.mem.pct_used()),
        fmt_pct(g.mem.pct_used()),
        pal.reset()
    );
    let barw = cols
        .saturating_sub(cc(&left) + cc(&right_plain) + 4)
        .clamp(6, cols);
    let cells = gauge(barw, &g.mem, st);
    clip_ansi(
        &format!("{left}  {}  {right}", pal.paint(&cells, g.mem.pct_used())),
        cols,
    )
}

// -------------------------------------------------------------------- json

fn jstr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn json_gpu(g: &Gpu, driver: &str, now: &str) -> String {
    let mib = |kib: u64| (kib + 1023) / 1024; // l'arrondi NVML : a la hausse
    let mut s = String::new();
    s.push('{');
    s.push_str(&format!("\"index\":{},", g.idx));
    s.push_str(&format!("\"name\":{},", jstr(&g.name)));
    s.push_str(&format!("\"driver\":{},", jstr(driver)));
    s.push_str(&format!("\"timestamp\":{}", jstr(now)));
    s.push_str(&format!(",\"available\":{}", g.mem.ok));
    s.push_str(&format!(
        ",\"vram_kib\":{{\"total\":{},\"used\":{},\"free\":{},\"reserved\":{},\"heap\":{}}}",
        g.mem.total_kib,
        g.mem.used_kib(),
        g.mem.avail_kib(),
        g.mem.reserved_kib(),
        g.mem.heap_kib
    ));
    s.push_str(&format!(
        ",\"vram_mib\":{{\"total\":{},\"used\":{},\"free\":{},\"reserved\":{}}}",
        mib(g.mem.total_kib),
        mib(g.mem.used_kib()),
        mib(g.mem.avail_kib()),
        mib(g.mem.reserved_kib())
    ));
    s.push_str(&format!(",\"pct_used\":{:.2}", g.mem.pct_used()));
    s.push_str(&format!(
        ",\"extras\":{{{}}}",
        g.extra
            .iter()
            .map(|(k, v)| format!("{}:{}", jstr(k), jstr(v)))
            .collect::<Vec<_>>()
            .join(",")
    ));
    let procs: Vec<String> = g
        .procs
        .iter()
        .map(|p| {
            format!(
                "{{\"pid\":{},\"name\":{},\"compute\":{},\"bytes\":{},\"mib\":{}}}",
                p.pid,
                jstr(&basename(&p.name)),
                p.compute,
                p.bytes,
                p.bytes / (1024 * 1024)
            )
        })
        .collect();
    s.push_str(&format!(
        ",\"processes\":[{procs}]",
        procs = procs.join(",")
    ));
    s.push('}');
    s
}
