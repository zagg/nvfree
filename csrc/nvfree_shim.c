/* nvfree_shim.c - fine couche d'accès pour que Rust parle à la lib.
 *
 * Tout ce qui connaît l'ABI du pilote (ioctls, RUSD, classes RM) reste dans
 * nvidia-smi-clone-RE : src/nvrm.c + src/fields.c. Ce fichier n'expose qu'un
 * contexte opaque et des accesseurs plats, donc csrc/nvfree_shim.h est la
 * seule interface que le code Rust doit reproduire.
 *
 * nv_ctx_t n'est PAS transcrit côté Rust : il pèse ~5 Mo (32 GPU x 200
 * processus) et sa layout suit les versions de la lib. On le malloc ici, Rust
 * ne manipule qu'un *mut c_void.
 *
 * Lecture seule : uniquement les GET/QUERY déjà utilisés par nvsmi.
 */
#define _GNU_SOURCE
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>

#include "fields.h"

/* ---- erreurs -------------------------------------------------------- */

static char g_err[256] = "";

const char *nvf_errmsg(void) { return g_err; }

/* ---- cycle de vie --------------------------------------------------- */

void *nvf_open(void)
{
    nv_ctx_t *c = calloc(1, sizeof *c);
    if (!c) {
        snprintf(g_err, sizeof g_err, "allocation du contexte RM impossible (OOM)");
        return NULL;
    }
    if (nv_ctx_open(c) != 0) {
        nv_ctx_close(c);
        free(c);
        snprintf(g_err, sizeof g_err,
                 "le pilote NVIDIA ne repond pas sur /dev/nvidiactl "
                 "(module nvidia charge ? droits d'acces ? relancer en root)");
        return NULL;
    }
    if (c->n_gpu <= 0)
        snprintf(g_err, sizeof g_err, "le pilote ne declare aucun GPU");
    return c;
}

void nvf_close(void *p)
{
    if (!p) return;
    nv_ctx_close((nv_ctx_t *)p);
    free(p);
}

int nvf_n_gpu(void *p)
{
    nv_ctx_t *c = p;
    return c ? c->n_gpu : -1;
}

/* Re-lecture complete d'un GPU : poll RUSD (1 ioctl) + FB_GET_INFO_V2 +
 * process list. C'est le meme chemin que `nvsmi -l`. */
int nvf_refresh(void *p, int i)
{
    nv_ctx_t *c = p;
    if (!c || i < 0 || i >= c->n_gpu) return -1;
    gpu_refresh(c, &c->gpu[i]);
    return 0;
}

/* ---- memoire video (le coeur de nvfree) ----------------------------- */

/* Les trois valeurs sont en KiB, telles que le pilote les renvoie :
 *   total = tag TOTAL_RAM_SIZE(8)
 *   heap  = tag HEAP_SIZE(9)
 *   free  = tag HEAP_FREE(22)
 * NVML/en formule usuelle : used = heap - free, reserved = total - heap.
 * retourne 0 si la sonde FB a repondu, -1 sinon. */
int nvf_mem(void *p, int i, uint64_t *total, uint64_t *heap, uint64_t *free_kib)
{
    nv_ctx_t *c = p;
    if (!c || i < 0 || i >= c->n_gpu) return -1;
    gpu_t *g = &c->gpu[i];
    if (!g->have_fb) return -1;
    if (total)     *total     = g->mem_total_kib;
    if (heap)      *heap      = g->mem_heap_kib;
    if (free_kib)  *free_kib  = g->mem_free_kib;
    return 0;
}

/* ---- identification / champs generiques ----------------------------- */

int nvf_index(void *p, int i)
{
    nv_ctx_t *c = p;
    if (!c || i < 0 || i >= c->n_gpu) return -1;
    return (int)c->gpu[i].idx;
}

/* nom commercial ("NVIDIA RTX A4000") */
int nvf_name(void *p, int i, char *out, int outsz)
{
    nv_ctx_t *c = p;
    if (!c || i < 0 || i >= c->n_gpu || outsz <= 0) return -1;
    snprintf(out, outsz, "%s", c->gpu[i].name[0] ? c->gpu[i].name : NA);
    return 0;
}

/* version du pilote (chaine deja lue via NV0000_CTRL_CMD_SYSTEM_GET_BUILD_VERSION) */
int nvf_driver(void *p, char *out, int outsz)
{
    nv_ctx_t *c = p;
    if (!c || outsz <= 0) return -1;
    snprintf(out, outsz, "%s", c->kmd_version[0] ? c->kmd_version : c->driver_version);
    return 0;
}

/* Champ nomme, syntaxe nvidia-smi ("temperature.gpu", "power.draw", ...).
 * rend l'unite deja formatee par la lib ; renvoie 0 si le champ existe. */
int nvf_field(void *p, int i, const char *name, char *out, int outsz, int nounits)
{
    nv_ctx_t *c = p;
    if (!c || i < 0 || i >= c->n_gpu || outsz <= 0 || !name) return -1;
    out[0] = 0;
    return field_get(c, &c->gpu[i], name, out, (size_t)outsz, nounits) ? 0 : -1;
}

/* ---- processus qui tiennent de la VRAM ------------------------------ */

int nvf_nproc(void *p, int i)
{
    nv_ctx_t *c = p;
    if (!c || i < 0 || i >= c->n_gpu) return -1;
    return c->gpu[i].n_proc;
}

/* Entree k du tableau de processus : pid, octets alloues, type (1 = compute),
 * nom de commande. mem_bytes est en octets bruts (la lib cumule les canaux). */
int nvf_proc(void *p, int i, int k, uint32_t *pid, uint64_t *bytes,
             int *type_c, char *cmd, int cmdsz)
{
    nv_ctx_t *c = p;
    if (!c || i < 0 || i >= c->n_gpu || cmdsz <= 0) return -1;
    gpu_t *g = &c->gpu[i];
    if (k < 0 || k >= g->n_proc) return -1;
    const proc_t *pr = &g->proc[k];
    if (pid)     *pid     = pr->pid;
    if (bytes)   *bytes   = pr->mem_bytes;
    if (type_c)  *type_c  = pr->type_c;
    if (pr->cmd[0]) snprintf(cmd, cmdsz, "%s", pr->cmd);
    else            snprintf(cmd, cmdsz, "%s", pr->cmdline[0] ? pr->cmdline : "?");
    return 0;
}

/* ---- terminal et signaux (pour eviter la crate libc) ---------------- */

/* Largeur du terminal : ioctl TIOCGWINSZ sur stdout, puis COLUMNS/LINES,
 * puis 80x24 quand la sortie n'est pas un tty (pipes, logs). */
void nvf_term(int *cols, int *rows)
{
    struct winsize ws;
    if (cols) *cols = 0;
    if (rows) *rows = 0;
    if (ioctl(1, TIOCGWINSZ, &ws) == 0 && ws.ws_col) {
        if (cols) *cols = (int)ws.ws_col;
        if (rows) *rows = (int)ws.ws_row;
        return;
    }
    const char *e = getenv("COLUMNS");
    if (e && atoi(e) > 0 && cols) *cols = atoi(e);
    e = getenv("LINES");
    if (e && atoi(e) > 0 && rows) *rows = atoi(e);
    if (cols && !*cols) *cols = 80;
    if (rows && !*rows) *rows = 24;
}

static volatile sig_atomic_t g_stop = 0;

static void on_term(int sig) { (void)sig; g_stop = 1; }
static void on_int(int sig)  { (void)sig; g_stop = 2; }

void nvf_trap_signals(void)
{
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_term;
    sigaction(SIGTERM, &sa, NULL);
    sa.sa_handler = on_int;
    sigaction(SIGINT, &sa, NULL);
    signal(SIGPIPE, SIG_IGN);   /* `nvfree | head` ne doit pas nous tuer */
}

int nvf_stop_requested(void) { return (int)g_stop; }

/* stdout est-il un terminal ? (decide du re-rubanement ANSI en mode --loop) */
int nvf_is_tty(void) { return isatty(1); }

/* Horodatage local, format "2026-10-24 10:30:12" : plus simple que de lier
 * une crate de temps côté Rust. */
int nvf_now(char *out, int outsz)
{
    time_t t;
    struct tm tv;
    if (!out || outsz <= 0) return -1;
    out[0] = 0;
    t = time(NULL);
    if (t == (time_t)-1 || !localtime_r(&t, &tv)) return -1;
    return (int)strftime(out, (size_t)outsz, "%Y-%m-%d %H:%M:%S", &tv);
}
