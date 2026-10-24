//! cli.rs - analyse des arguments, faite a la main : nvfree n'a aucune
//! dependance, hors la lib C de l'ABI pilote.

use crate::bar::{fmt_kib, Mode, Unit};

#[derive(Clone, Debug)]
pub struct Opts {
    /// filtres GPU : index, sous-chaine de nom, ou GPU-<uuid>
    pub gpus: Vec<String>,
    /// force la largeur du graphe (defaut : largeur du terminal)
    pub width: Option<usize>,
    pub unit: Unit,
    pub ascii: bool,
    pub color: Mode,
    /// nombre de processus affiches par GPU (0 = pas de detail)
    pub procs: usize,
    pub ticks: bool,
    /// Some(secs) => rafraichissement continu (facon `nvsmi -l`)
    pub loop_secs: Option<f64>,
    pub loop_count: Option<usize>,
    pub json: bool,
    pub compact: bool,
    /// pas d'en-tete ni de legende : une barre par GPU, rien d'autre
    pub quiet: bool,
    /// --free-min=<taille> : code de sortie 3 si un GPU a moins de VRAM libre
    pub free_min_kib: Option<u64>,
    pub help: bool,
    pub version: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            gpus: Vec::new(),
            width: None,
            unit: Unit::Auto,
            ascii: false,
            color: Mode::Ext,
            procs: 0,
            ticks: true,
            loop_secs: None,
            loop_count: None,
            json: false,
            compact: false,
            quiet: false,
            free_min_kib: None,
            help: false,
            version: false,
        }
    }
}

pub const HELP: &str = "\
nvfree - how much VRAM is used and how much is left, as a bar graph in your
terminal. The numbers come straight out of the NVIDIA kernel driver: ioctls on
/dev/nvidia*, the RUSD shared page (class 0x00de) and FB_GET_INFO_V2
(0x20801303). No NVML, no libnvidia-ml, no nvidia-smi, no CUDA - and nvfree
never writes anything to the GPU.

Usage: nvfree [OPTION]...

  -l, --loop[=SEC]    keep refreshing (default: 1 s)
      --count=N       number of frames with --loop (0 = forever)
  -i, --gpu=ID        pick a GPU: index, name substring or GPU-<uuid>; repeatable
  -1, --compact       one line per GPU - fits in logs, prompts and scripts
  -p, --procs[=N]     show which processes hold the VRAM (N = 5 by default)
  -w, --width=N       graph width (default: terminal width)
      --no-ticks      drop the 0/25/50/75/100 % scale
  -u, --units=U       auto (default), MiB or GiB
  -a, --ascii         ASCII blocks (# + -) instead of U+2588 U+2592 U+2591
      --no-color      plain text (NO_COLOR, TERM=dumb and pipes do the same)
  -q, --quiet         no header, no legend
      --json          machine-readable snapshot (NDJSON when used with --loop)
      --free-min=S    exit code 3 if any GPU has less than S free
                      (S: 2G, 512M, 1500MiB, 2048K; a bare number means MiB)
  -h, --help          this help
  -V, --version       version, plus where the data comes from

Examples:
  nvfree                      one snapshot, one block per GPU
  nvfree -l 2 -p              live graph with the per-process breakdown
  nvfree -1qa                 one ASCII line per GPU, no colour
  nvfree --free-min=6G        gate before a render job: 0 means enough room
  nvfree --json | jq .vram_mib.free
";

/// "2G", "512M", "1500MiB", "1024K" -> KiB (bare = MiB)
pub fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim().to_ascii_lowercase();
    let split = t
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(t.len());
    let (num, unit) = (&t[..split], t[split..].trim());
    if num.is_empty() {
        return Err(format!("bad size: {s:?}"));
    }
    let v: f64 = num.parse().map_err(|_| format!("bad size: {s:?}"))?;
    let mult: f64 = match unit {
        "" | "mib" | "mi" | "m" => 1024.0,
        "kib" | "ki" | "k" => 1.0,
        "gib" | "gi" | "g" => 1024.0 * 1024.0,
        "tib" | "ti" | "t" => 1024.0 * 1024.0 * 1024.0,
        "b" => 1.0 / 1024.0,
        _ => return Err(format!("unknown size unit: {unit:?} (K M G T)")),
    };
    Ok((v * mult).round() as u64)
}

fn parse_secs(v: Option<&str>) -> Result<f64, String> {
    match v {
        None | Some("") => Ok(1.0),
        Some(s) => s
            .parse::<f64>()
            .ok()
            .filter(|x| *x >= 0.05)
            .ok_or_else(|| format!("bad interval: {s:?} (in seconds)")),
    }
}

fn parse_unit(v: &str) -> Result<Unit, String> {
    let t = v.trim().to_ascii_lowercase();
    Ok(match t.as_str() {
        "auto" | "" => Unit::Auto,
        "mib" | "mi" | "m" => Unit::Mib,
        "gib" | "gi" | "g" => Unit::Gib,
        _ => return Err(format!("expected auto, MiB or GiB (got {v:?})")),
    })
}

pub fn parse(argv: &[String]) -> Result<Opts, String> {
    // la couleur depend du terminal : on part de la detection, les options
    // (--no-color, --color=...) la surchargent ensuite.
    let mut o = Opts {
        color: default_color(),
        ..Opts::default()
    };
    let mut it = argv.iter().peekable();

    while let Some(a) = it.next() {
        if a == "--" {
            for x in it.by_ref() {
                o.gpus.push(x.clone());
            }
            break;
        }
        if a.starts_with("--") {
            let (name, inline) = match a.find('=') {
                Some(p) => (a[2..p].to_string(), Some(a[p + 1..].to_string())),
                None => (a.strip_prefix("--").unwrap_or(a).to_string(), None),
            };
            // valeur explicite au prochain argument si necessaire et absente
            let mut val = inline;
            macro_rules! need_val {
                () => {
                    match val.clone() {
                        Some(v) => v,
                        None => it
                            .next()
                            .cloned()
                            .ok_or_else(|| format!("--{name} needs a value"))?,
                    }
                };
            }
            match name.as_str() {
                "help" | "h" => o.help = true,
                "version" | "V" => o.version = true,
                "loop" | "loop-with-delay" | "listen" => {
                    val = Some(need_val!());
                    o.loop_secs = Some(parse_secs(val.as_deref())?);
                }
                "count" => {
                    let v = need_val!();
                    o.loop_count = Some(
                        v.parse::<usize>()
                            .map_err(|_| "--count: expected an integer")?,
                    );
                }
                "width" => {
                    let v = need_val!();
                    let w: usize = v.parse().map_err(|_| "--width: expected an integer")?;
                    if w < 12 {
                        return Err("--width: at least 12 columns".into());
                    }
                    o.width = Some(w);
                }
                "units" | "unit" => {
                    let v = need_val!();
                    o.unit = parse_unit(&v)?;
                }
                "gpu" | "id" | "index" => {
                    let v = need_val!();
                    o.gpus.push(v);
                }
                "procs" | "processes" => {
                    // valeur optionnelle : --procs, --procs=8
                    match val.as_deref() {
                        None | Some("") => o.procs = 5,
                        Some(s) => {
                            o.procs = s
                                .parse::<usize>()
                                .map_err(|_| "--procs: expected an integer")?
                                .max(1)
                        }
                    }
                }
                "ascii" => o.ascii = true,
                "no-color" => o.color = Mode::Off,
                "color" => {
                    let v = need_val!();
                    o.color = match v.as_str() {
                        "never" | "no" | "off" => Mode::Off,
                        "always" | "yes" => Mode::Ext,
                        "basic" | "8" => Mode::Basic,
                        _ => return Err(format!("--color: always, basic or never (got {v:?})")),
                    };
                }
                "compact" | "oneline" | "line" => o.compact = true,
                "quiet" | "no-header" => o.quiet = true,
                "no-ticks" => o.ticks = false,
                "json" => o.json = true,
                "free-min" | "check" => {
                    let v = need_val!();
                    o.free_min_kib = Some(parse_size(&v)?);
                }
                _ => return Err(format!("unknown option: --{name} (nvfree --help)")),
            }
            continue;
        }

        if a.starts_with('-') && a.len() > 1 {
            let chars: Vec<char> = a[1..].chars().collect();
            let mut k = 0usize;
            while k < chars.len() {
                let c = chars[k];
                k += 1;
                // valeur colleee (-l2, -w40, -i0, -uGiB) ou au prochain argument
                let attached: Option<String> = if k < chars.len() {
                    Some(chars[k..].iter().collect())
                } else {
                    None
                };
                match c {
                    'h' => o.help = true,
                    'V' => o.version = true,
                    'a' => o.ascii = true,
                    'q' => o.quiet = true,
                    '1' => o.compact = true,
                    'p' => {
                        match attached
                            .filter(|s| !s.is_empty() && s.chars().all(|d| d.is_ascii_digit()))
                        {
                            Some(s) => {
                                o.procs = s.parse::<usize>().unwrap_or(5).max(1);
                                k = chars.len();
                            }
                            None => o.procs = 5,
                        }
                    }
                    'l' | 'w' | 'u' | 'i' => {
                        let v = match attached {
                            Some(s) => s,
                            None => it
                                .next()
                                .cloned()
                                .ok_or_else(|| format!("-{c} needs a value"))?,
                        };
                        match c {
                            'l' => o.loop_secs = Some(parse_secs(Some(&v))?),
                            'w' => {
                                let v2 = v.replace(' ', "");
                                let w: usize = v2.parse().map_err(|_| "-w: expected an integer")?;
                                if w < 12 {
                                    return Err("-w: at least 12 columns".into());
                                }
                                o.width = Some(w);
                            }
                            'u' => o.unit = parse_unit(&v)?,
                            _ => o.gpus.push(v),
                        }
                        k = chars.len();
                    }
                    _ => return Err(format!("unknown option: -{c} (nvfree --help)")),
                }
            }
            continue;
        }

        return Err(format!("unexpected argument: {a}"));
    }
    Ok(o)
}

/// Couleur par defaut : NO_COLOR / TERM=dumb / sortie non tty => aucune couleur.
pub fn default_color() -> Mode {
    use std::env;
    use std::io::IsTerminal;
    if env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal() {
        return Mode::Off;
    }
    match env::var("TERM").ok() {
        Some(t) if t == "dumb" || t.is_empty() => Mode::Off,
        _ => {
            if env::var("COLORTERM")
                .map(|c| c == "truecolor" || c == "24bit")
                .unwrap_or(false)
            {
                Mode::Ext
            } else {
                Mode::Basic
            }
        }
    }
}

/// forme courte quand la largeur manque : "15.1/16.0G" (GiB) ou "471/1024M" (MiB)
pub fn pair_short(used: u64, total: u64) -> String {
    if total >= 1024 * 1024 {
        format!(
            "{:.1}/{:.1}G",
            used as f64 / (1024.0 * 1024.0),
            total as f64 / (1024.0 * 1024.0)
        )
    } else {
        format!("{:.0}/{:.0}M", used as f64 / 1024.0, total as f64 / 1024.0)
    }
}

/// "15.10 / 15.99 GiB" : une seule unite pour les deux soldes.
pub fn pair(used: u64, total: u64, u: Unit) -> String {
    let eff = match u {
        Unit::Auto => {
            if total >= 1024 * 1024 {
                Unit::Gib
            } else {
                Unit::Mib
            }
        }
        x => x,
    };
    format!("{} / {}", fmt_kib(used, eff), fmt_kib(total, eff))
}
