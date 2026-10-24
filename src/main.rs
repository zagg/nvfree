//! nvfree - la VRAM consommee et disponible, en bare graph dans un terminal.
//!
//! Aucune librairie proprietaire n'est utilisee : les nombres viennent du
//! pilote NVIDIA lui-meme, via les ioctls `/dev/nvidia*`, la page partagee
//! RUSD (classe 0x00de) et le controle `FB_GET_INFO_V2` (0x20801303). Cette
//! partie est la lib `nvidia-smi-clone-RE` (https://github.com/zagg/
//! nvidia-smi-clone-RE), compilee en archive statique par build.rs ; le
//! reverse-engineering complet est documente dans docs/REVERSE_ENGINEERING.md
//! de cette lib.
//!
//! Strictement lecture seule : uniquement des controles GET/QUERY. Aucun SET,
//! aucun reset, aucune ecriture de limite de puissance.

mod bar;
mod cli;
mod ffi;
mod gpu;
mod render;

use std::io::Write;

/// ecrit sur stdout sans paniquer si le lecteur a ferme le tube : `nvfree --json | head`
/// doit se terminer proprement (le shim C ignore SIGPIPE, donc std recoit EPIPE).
fn emit(s: &str) -> bool {
    let out = std::io::stdout();
    let mut h = out.lock();
    h.write_all(s.as_bytes()).is_ok() && h.flush().is_ok()
}
use std::os::raw::{c_char, c_int};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use bar::{fmt_kib, Pal, Style};
use cli::Opts;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let o = match cli::parse(&argv) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("nvfree: {e}");
            eprintln!("try `nvfree --help`");
            return ExitCode::from(2);
        }
    };

    if o.help {
        print!("{}", cli::HELP);
        return ExitCode::SUCCESS;
    }
    if o.version {
        println!("{}", version_line());
        return ExitCode::SUCCESS;
    }

    // le shim installe SIGINT/SIGTERM : on peut quitter la boucle proprement
    unsafe { ffi::nvf_trap_signals() };

    let ctx = match gpu::Ctx::open() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("nvfree: {e}");
            return ExitCode::from(1);
        }
    };
    let n = ctx.n_gpu();
    if n == 0 {
        eprintln!("nvfree: the driver reports no NVIDIA GPU");
        return ExitCode::from(1);
    }
    let sel = select(&ctx, &o, n);
    if sel.is_empty() {
        eprintln!("nvfree: no GPU matches [{}]", o.gpus.join(", "));
        eprintln!(
            "nvfree: GPUs seen by the driver: {}",
            (0..n)
                .map(|i| format!("{i} {}", ctx.gpu_name(i)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        return ExitCode::from(4);
    }

    let driver = ctx.driver();
    let pal = Pal { mode: o.color };
    let st = Style { ascii: o.ascii };
    let looping = o.loop_secs.is_some();
    let secs = o.loop_secs.unwrap_or(0.0);
    let tty = unsafe { ffi::nvf_is_tty() } != 0;
    let redraw = looping && tty && !o.json;

    if redraw {
        print!("\x1b[?25l"); // curseur cache pendant le suivi
    }

    let mut frame = 0usize;
    let mut prev_lines = 0usize;
    let mut shortage: Option<(usize, u64)> = None;

    loop {
        frame += 1;
        let stamp = match (looping, frame) {
            (true, _) => format!("{} #{}", now(), frame),
            _ => now(),
        };
        let cols = term_cols(&o);
        let gpus: Vec<gpu::Gpu> = sel
            .iter()
            .map(|&i| ctx.snapshot(i, o.procs > 0 || o.json))
            .collect();

        let lines = frame_lines(&o, &gpus, &driver, &stamp, cols, &pal, &st);
        if redraw {
            let mut buf = String::new();
            buf.push_str(&format!("\x1b[{prev_lines}A"));
            for l in &lines {
                buf.push_str("\x1b[0K");
                buf.push_str(l);
                buf.push('\n');
            }
            buf.push_str("\x1b[0J");
            if !emit(&buf) {
                break;
            }
        } else {
            let mut broke = false;
            for l in &lines {
                if !emit(&format!("{l}\n")) {
                    broke = true;
                    break;
                }
            }
            if broke {
                break;
            }
        }
        prev_lines = lines.len();

        // garde-fou de script : --free-min
        if let Some(min) = o.free_min_kib {
            for g in &gpus {
                if g.mem.ok && g.mem.avail_kib() < min {
                    shortage = Some((g.idx, g.mem.avail_kib()));
                    break;
                }
            }
        }

        if !looping {
            break;
        }
        if let Some(c) = o.loop_count {
            if frame >= c {
                break;
            }
        }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs_f64(secs) {
            if unsafe { ffi::nvf_stop_requested() } != 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        if unsafe { ffi::nvf_stop_requested() } != 0 {
            break;
        }
    }

    if redraw {
        print!("\x1b[?25h");
    }

    if let Some((idx, free)) = shortage {
        eprintln!(
            "nvfree: GPU {idx} has only {} free VRAM (needs {})",
            fmt_kib(free, o.unit),
            fmt_kib(o.free_min_kib.unwrap(), o.unit)
        );
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

/// titre + provenance + version du pilote + horodatage, puis un bloc par GPU
fn frame_lines(
    o: &Opts,
    gpus: &[gpu::Gpu],
    driver: &str,
    stamp: &str,
    cols: usize,
    pal: &Pal,
    st: &Style,
) -> Vec<String> {
    if o.json {
        return gpus
            .iter()
            .map(|g| render::json_gpu(g, driver, stamp))
            .collect();
    }
    let mut lines = render::header(o, driver, stamp, cols, pal);
    if o.compact {
        for g in gpus {
            lines.push(render::compact_line(g, o, cols, pal, st));
        }
        if gpus.len() > 1 {
            lines.push(render::total_line(gpus, o, cols, pal, st));
        }
        return lines;
    }
    for (k, g) in gpus.iter().enumerate() {
        if k > 0 && !o.quiet {
            lines.push(String::new());
        }
        lines.extend(render::block(g, o, cols, pal, st));
    }
    if gpus.len() > 1 {
        if !o.quiet {
            lines.push(String::new());
        }
        lines.push(render::total_line(gpus, o, cols, pal, st));
    }
    lines
}

/// -i accepte un index, une sous-chaine du nom, ou un UUID "GPU-..."
fn select(ctx: &gpu::Ctx, o: &Opts, n: usize) -> Vec<usize> {
    if o.gpus.is_empty() {
        return (0..n).collect();
    }
    let mut out = Vec::new();
    for i in 0..n {
        let name = ctx.gpu_name(i).to_lowercase();
        let uuid = ctx.field(i, "gpu_uuid").to_lowercase();
        for f in &o.gpus {
            let fl = f.trim().to_lowercase();
            let hit = match fl.parse::<usize>() {
                Ok(k) => k == i,
                Err(_) => {
                    if fl.starts_with("gpu-") {
                        uuid.contains(&fl)
                    } else {
                        name.contains(&fl)
                    }
                }
            };
            if hit {
                out.push(i);
                break;
            }
        }
    }
    out
}

fn term_cols(o: &Opts) -> usize {
    if let Some(w) = o.width {
        return w;
    }
    let (mut c, mut r) = (0 as c_int, 0 as c_int);
    unsafe { ffi::nvf_term(&mut c, &mut r) };
    if c < 24 {
        24
    } else {
        c as usize
    }
}

fn now() -> String {
    let mut buf = [0u8; 32];
    unsafe {
        ffi::nvf_now(buf.as_mut_ptr() as *mut c_char, buf.len() as c_int);
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

const VERSION: &str = match option_env!("CARGO_PKG_VERSION") {
    Some(v) => v,
    None => "0.1.0",
};

fn version_line() -> String {
    let lib = option_env!("NVF_LIB").unwrap_or("nvidia-smi-clone-RE");
    format!(
        "nvfree {}\n  data source: the NVIDIA kernel driver - ioctls on /dev/nvidia*, RUSD page (0x00de), FB_GET_INFO_V2 (0x20801303)\n  driver-ABI library: {}\n  this binary links no NVML, no libnvidia-ml and no nvidia-smi",
        VERSION,
        lib
    )
}
