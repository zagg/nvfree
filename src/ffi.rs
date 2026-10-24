//! ffi.rs - les seuls symboles C dont nvfree a besoin.
//!
//! Toute la connaissance de l'ABI du pilote (ioctls NV_ESC_*, page partagee
//! RUSD, controles RM) vit dans la lib C `nvidia-smi-clone-RE` ; see
//! https://github.com/zagg/nvidia-smi-clone-RE. `nv_ctx_t` reste un type
//! opaque ici : on ne manipule qu'un pointeur, et la couche Cplat du shim
//! (csrc/nvfree_shim.c) est le seul contrat a reproduire.
//!
//! Pas de crate `libc` : `std` lie deja la glibc, il suffit de declarer les
//! prototypes (meme recette que dans nvtop-rs).

use std::os::raw::{c_char, c_int, c_uint, c_ulonglong, c_void};

extern "C" {
    pub fn nvf_open() -> *mut c_void;
    pub fn nvf_errmsg() -> *const c_char;
    pub fn nvf_close(ctx: *mut c_void);

    pub fn nvf_n_gpu(ctx: *mut c_void) -> c_int;
    pub fn nvf_refresh(ctx: *mut c_void, i: c_int) -> c_int;
    pub fn nvf_index(ctx: *mut c_void, i: c_int) -> c_int;

    /// total / heap / free en KiB, tels que renvoyes par FB_GET_INFO_V2
    pub fn nvf_mem(
        ctx: *mut c_void,
        i: c_int,
        total: *mut c_ulonglong,
        heap: *mut c_ulonglong,
        free_kib: *mut c_ulonglong,
    ) -> c_int;

    pub fn nvf_name(ctx: *mut c_void, i: c_int, buf: *mut c_char, bufsz: c_int) -> c_int;
    pub fn nvf_driver(ctx: *mut c_void, buf: *mut c_char, bufsz: c_int) -> c_int;
    pub fn nvf_field(
        ctx: *mut c_void,
        i: c_int,
        name: *const c_char,
        buf: *mut c_char,
        bufsz: c_int,
        nounits: c_int,
    ) -> c_int;

    pub fn nvf_nproc(ctx: *mut c_void, i: c_int) -> c_int;
    pub fn nvf_proc(
        ctx: *mut c_void,
        i: c_int,
        k: c_int,
        pid: *mut c_uint,
        bytes: *mut c_ulonglong,
        type_c: *mut c_int,
        cmd: *mut c_char,
        cmdsz: c_int,
    ) -> c_int;

    pub fn nvf_term(cols: *mut c_int, rows: *mut c_int);
    pub fn nvf_trap_signals();
    pub fn nvf_stop_requested() -> c_int;
    pub fn nvf_is_tty() -> c_int;
    /// horodatage local "YYYY-MM-DD HH:MM:SS" (fourni par la couche C : pas de crate de temps)
    pub fn nvf_now(buf: *mut c_char, bufsz: c_int) -> c_int;
}

/// Copie une chaine C dans un String (vide si pointeur nul).
pub fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}
