//! build.rs - compile la lib (nvidia-smi-clone-RE) en archive statique et la lie
//! au binaire Rust. Aucune crate externe : on appelle directement gcc et ar.
//!
//! Emplacement de la lib, cherché dans cet ordre :
//!   1. $NVSMI_RE            (variable d'environnement)
//!   2. ./vendor/nvidia-smi-clone-RE   (déposé par `make vendor`)
//!   3. ../nvidia-smi-clone-RE, ../gh-nvsmi, /root/gh-nvsmi, /root/nvrea
//!   4. clone depuis GitHub si $NVSMI_RE_GIT est défini et le dossier absent

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const C_SRCS: &[&str] = &["src/nvrm.c", "src/fields.c"];
const SHIM: &str = "csrc/nvfree_shim.c";

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));

    // on recompile si un .c/.h de la lib change, pas seulement si build.rs change
    println!("cargo:rerun-if-changed={}", SHIM);
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=NVSMI_RE");

    let lib = match find_lib(&manifest) {
        Some(p) => p,
        None => {
            fail(
                "lib introuvable (nvidia-smi-clone-RE). Définir NVSMI_RE=/chemin/vers/la/lib ou lancer `make vendor`.",
            );
        }
    };
    for s in C_SRCS {
        println!("cargo:rerun-if-changed={}", lib.join(s).display());
    }
    for h in ["include/nvrm.h", "include/rusd.h", "include/fields.h"] {
        println!("cargo:rerun-if-changed={}", lib.join(h).display());
    }

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let cc = env::var("CC").unwrap_or_else(|_| "cc".into());
    let opt = match env::var("DEBUG").as_deref() {
        Ok("true") => "-O0",
        _ => "-O2",
    };

    let mut objs = Vec::new();
    for src in C_SRCS
        .iter()
        .map(|s| lib.join(s))
        .chain(std::iter::once(manifest.join(SHIM)))
    {
        let name = src.file_stem().unwrap().to_string_lossy().into_owned();
        let obj = out.join(format!("{name}.o"));
        let status = Command::new(&cc)
            .arg(opt)
            .arg("-std=gnu11")
            .arg("-Wall")
            .arg("-I")
            .arg(lib.join("include"))
            .arg("-c")
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(_) => fail(&format!("echec de compilation de {}", src.display())),
            Err(e) => fail(&format!("{} introuvable ({e}) - installer gcc", cc)),
        }
        objs.push(obj);
    }

    let ar = env::var("AR").unwrap_or_else(|_| "ar".into());
    let archive = out.join("libnvf.a");
    let _ = std::fs::remove_file(&archive);
    let mut cmd = Command::new(&ar);
    cmd.arg("rcs").arg(&archive);
    for o in &objs {
        cmd.arg(o);
    }
    match cmd.status() {
        Ok(s) if s.success() => {}
        _ => fail(&format!("echec de '{ar} rcs {}'", archive.display())),
    }

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=nvf");
    println!("cargo:rustc-link-lib=dylib=dl");

    // pratique : `nvfree --version` cite la lib (chemin canonique, lisible)
    let shown = lib.canonicalize().unwrap_or_else(|_| lib.clone());
    println!("cargo:rustc-env=NVF_LIB={}", shown.display());
}

fn find_lib(manifest: &Path) -> Option<PathBuf> {
    let mut cand: Vec<PathBuf> = Vec::new();
    if let Ok(v) = env::var("NVSMI_RE") {
        cand.push(v.into());
    }
    cand.push(manifest.join("vendor/nvidia-smi-clone-RE"));
    cand.push(manifest.join("../nvidia-smi-clone-RE"));
    cand.push(manifest.join("../gh-nvsmi"));
    cand.push(manifest.join("../nvrea"));
    if let Ok(home) = env::var("HOME") {
        cand.push(PathBuf::from(&home).join("gh-nvsmi"));
        cand.push(PathBuf::from(home).join("nvidia-smi-clone-RE"));
    }
    for c in &cand {
        if c.join("include/fields.h").is_file() && c.join("src/nvrm.c").is_file() {
            return Some(c.clone());
        }
    }
    None
}

fn fail(msg: &str) -> ! {
    // message lisible dans la sortie cargo + vrai code d'erreur
    println!("cargo:warning=nvfree: {msg}");
    eprintln!("nvfree(build): {msg}");
    std::process::exit(101);
}
