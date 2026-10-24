//! gpu.rs - vue sur etroite du modele de donnees de la lib : uniquement la VRAM,
//! les quelques champs secondaires affiches en dessous du graphe, et la liste
//! des processus qui tiennent de la memoire video.
//!
//! Les nombres viennent du pilote lui-meme : FB_GET_INFO_V2 (0x20801303),
//! tags TOTAL_RAM_SIZE / HEAP_SIZE / HEAP_FREE, en KiB. Formule reprise de
//! NVML (documentee dans docs/REVERSE_ENGINEERING.md de la lib) :
//!     used     = heap  - free
//!     reserved = total - heap
//!     free     = heap  - used  (= le HEAP_FREE brut rendu par le pilote)

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uint, c_ulonglong, c_void};

use crate::ffi;

/// Memoire video d'un GPU, en KiB (unites brutes du pilote).
#[derive(Clone, Copy, Debug, Default)]
pub struct Mem {
    pub total_kib: u64,
    pub heap_kib: u64,
    pub free_kib: u64,
    pub ok: bool,
}

impl Mem {
    pub fn used_kib(&self) -> u64 {
        self.heap_kib.saturating_sub(self.free_kib)
    }
    pub fn reserved_kib(&self) -> u64 {
        self.total_kib.saturating_sub(self.heap_kib)
    }
    /// ce que nvidia-smi appelle "memory.free"
    pub fn avail_kib(&self) -> u64 {
        self.free_kib
    }
    pub fn pct_used(&self) -> f64 {
        if self.total_kib == 0 {
            0.0
        } else {
            100.0 * self.used_kib() as f64 / self.total_kib as f64
        }
    }
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub bytes: u64,
    pub compute: bool,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Gpu {
    pub idx: usize,
    pub name: String,
    pub mem: Mem,
    /// champs secondaires deja formates par la lib ("57", "108.09", "P2", ...)
    pub extra: Vec<(String, String)>,
    pub procs: Vec<Proc>,
}

/// Champs secondaires demandes a la lib (syntaxe nvidia-smi).
const EXTRAS: &[&str] = &[
    "pstate",
    "temperature.gpu",
    "power.draw",
    "utilization.gpu",
    "utilization.memory",
];

/// Le contexte RM de la lib, detenu par Rust. `nv_ctx_t` reste opaque.
pub struct Ctx {
    p: *mut c_void,
}

unsafe impl Send for Ctx {}

impl Ctx {
    pub fn open() -> Result<Ctx, String> {
        let p = unsafe { ffi::nvf_open() };
        if p.is_null() {
            return Err(unsafe { ffi::cstr(ffi::nvf_errmsg()) });
        }
        Ok(Ctx { p })
    }

    pub fn driver(&self) -> String {
        let mut buf = [0u8; 64];
        unsafe {
            ffi::nvf_driver(self.p, buf.as_mut_ptr() as *mut c_char, buf.len() as c_int);
        }
        String::from_utf8_lossy(&buf[..strlen(&buf)]).into_owned()
    }

    pub fn n_gpu(&self) -> usize {
        let n = unsafe { ffi::nvf_n_gpu(self.p) };
        if n < 0 {
            0
        } else {
            n as usize
        }
    }

    /// Un champ nomme deja formate par la lib (syntaxe nvidia-smi).
    pub fn field(&self, i: usize, name: &str) -> String {
        let ckey = match CString::new(name) {
            Ok(v) => v,
            Err(_) => return String::new(),
        };
        let mut buf = [0u8; 256];
        let r = unsafe {
            ffi::nvf_field(
                self.p,
                i as c_int,
                ckey.as_ptr(),
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as c_int,
                1,
            )
        };
        if r != 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..strlen(&buf)]).into_owned()
    }

    pub fn gpu_name(&self, i: usize) -> String {
        let mut buf = [0u8; 96];
        unsafe {
            ffi::nvf_name(
                self.p,
                i as c_int,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as c_int,
            );
        }
        String::from_utf8_lossy(&buf[..strlen(&buf)]).into_owned()
    }

    /// Un instantane : re-interroge le pilote puis recopie ce qui nous interesse.
    pub fn snapshot(&self, i: usize, with_procs: bool) -> Gpu {
        let ii = i as c_int;
        unsafe { ffi::nvf_refresh(self.p, ii) };

        let idx = unsafe { ffi::nvf_index(self.p, ii) }.max(0) as usize;

        let mut name = [0u8; 96];
        unsafe {
            ffi::nvf_name(
                self.p,
                ii,
                name.as_mut_ptr() as *mut c_char,
                name.len() as c_int,
            );
        }

        let (mut total, mut heap, mut free) = (0u64, 0u64, 0u64);
        let ok = unsafe {
            ffi::nvf_mem(
                self.p,
                ii,
                &mut total as *mut u64 as *mut c_ulonglong,
                &mut heap as *mut u64 as *mut c_ulonglong,
                &mut free as *mut u64 as *mut c_ulonglong,
            ) == 0
        };

        let mut extra = Vec::new();
        for f in EXTRAS {
            let ckey = CString::new(*f).unwrap();
            let mut buf = [0u8; 96];
            let r = unsafe {
                ffi::nvf_field(
                    self.p,
                    ii,
                    ckey.as_ptr(),
                    buf.as_mut_ptr() as *mut c_char,
                    buf.len() as c_int,
                    0,
                )
            };
            if r != 0 {
                continue;
            }
            let v = String::from_utf8_lossy(&buf[..strlen(&buf)]).into_owned();
            if !v.is_empty() && v != "N/A" && v != "[Not Supported]" {
                extra.push(((*f).to_string(), v));
            }
        }

        let mut procs = Vec::new();
        if with_procs {
            let n = unsafe { ffi::nvf_nproc(self.p, ii) };
            for k in 0..n.max(0) {
                let (mut pid, mut bytes, mut tc) = (0u32, 0u64, 0);
                let mut cmd = [0u8; 512];
                let r = unsafe {
                    ffi::nvf_proc(
                        self.p,
                        ii,
                        k,
                        &mut pid as *mut u32 as *mut c_uint,
                        &mut bytes as *mut u64 as *mut c_ulonglong,
                        &mut tc as *mut i32 as *mut c_int,
                        cmd.as_mut_ptr() as *mut c_char,
                        cmd.len() as c_int,
                    )
                };
                if r != 0 || bytes == 0 {
                    continue;
                }
                procs.push(Proc {
                    pid,
                    bytes,
                    compute: tc != 0,
                    name: String::from_utf8_lossy(&cmd[..strlen(&cmd)]).into_owned(),
                });
            }
            procs.sort_by_key(|p| std::cmp::Reverse(p.bytes));
        }

        Gpu {
            idx,
            name: String::from_utf8_lossy(&name[..strlen(&name)]).into_owned(),
            mem: Mem {
                total_kib: total,
                heap_kib: heap,
                free_kib: free,
                ok,
            },
            extra,
            procs,
        }
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        unsafe { ffi::nvf_close(self.p) };
    }
}

fn strlen(b: &[u8]) -> usize {
    b.iter().position(|&c| c == 0).unwrap_or(b.len())
}
