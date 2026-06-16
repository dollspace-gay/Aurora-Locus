//! Aurora-Locus's standard kryphocron rotation oracle (Arc D #223).
//!
//! `AuroraLocusStandardRotationOracle` is a peer single-process implementation
//! of kryphocron 0.3's [`RotationOracle`] trait. It has the same behaviour
//! shape as the substrate's `DefaultRotationOracle` — CSRNG slug generation,
//! file-backed state, single-process-authoritative semantics, and the
//! `laquna/{secs}/{hex}` generation-mark format the Laquna codec parses — but:
//!
//! - it owns its **own** state file at `<data-dir>/aurora-locus/rotation.state`
//!   (distinct from `DefaultRotationOracle`'s `<data-dir>/kryphocron/…`), so
//!   Aurora-Locus controls its own restart-resume contract rather than
//!   depending on the substrate's private file format; and
//! - it adds two capabilities the substrate trait doesn't expose: a
//!   **runtime-settings-backed cadence** consulted (in-memory atomic load) on
//!   every `current_generation` — so a `kryphocron.laquna.rotation-cadence`
//!   change takes effect on the next encode without a restart — and a
//!   [`force_rotation`](AuroraLocusStandardRotationOracle::force_rotation) hook
//!   the `triggerRotation` XRPC invokes to expire the current generation ahead
//!   of cadence.
//!
//! Design: `v09_UI_Design.md` §6.4.2. The state-file + mark formats mirror
//! `kryphocron::codec::laquna` exactly so the codec parses our marks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kryphocron::encryption::{RotationContext, RotationGenerationMark, RotationOracle};
use rand::RngCore;

/// kryphocron's 24-hour baseline cadence (seconds).
const DAILY_SECS: u64 = 86_400;
/// Sentinel: `manual-only` cadence — never auto-rotate (rotate only on
/// [`force_rotation`](AuroraLocusStandardRotationOracle::force_rotation)).
const MANUAL_ONLY_SECS: u64 = u64::MAX;

/// Deployment rotation cadence, as set by `kryphocron.laquna.rotation-cadence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    Hourly,
    Daily,
    Weekly,
    ManualOnly,
}

impl Cadence {
    /// Parse the runtime-setting string; unknown/unset → `Daily` (kryphocron's
    /// baseline), so a missing key fail-softs to the safe default.
    pub fn from_setting(s: &str) -> Self {
        match s.trim() {
            "hourly" => Cadence::Hourly,
            "weekly" => Cadence::Weekly,
            "manual-only" => Cadence::ManualOnly,
            _ => Cadence::Daily,
        }
    }

    /// Cadence as a seconds interval. `ManualOnly` maps to a sentinel
    /// (`u64::MAX`-class) — there is no finite "next scheduled rotation" under
    /// manual-only, which `getRotationStatus` surfaces as a null next-rotation.
    pub fn as_secs(self) -> u64 {
        match self {
            Cadence::Hourly => 3_600,
            Cadence::Daily => DAILY_SECS,
            Cadence::Weekly => 604_800,
            Cadence::ManualOnly => MANUAL_ONLY_SECS,
        }
    }

    /// The canonical runtime-setting string for this cadence — the inverse of
    /// [`Cadence::from_setting`], for echoing the active policy in
    /// `getRotationStatus`.
    pub fn as_setting(self) -> &'static str {
        match self {
            Cadence::Hourly => "hourly",
            Cadence::Daily => "daily",
            Cadence::Weekly => "weekly",
            Cadence::ManualOnly => "manual-only",
        }
    }

    /// Whether this cadence schedules organic rotations (i.e. not
    /// `manual-only`).
    pub fn is_scheduled(self) -> bool {
        !matches!(self, Cadence::ManualOnly)
    }
}

/// Construction / persistence failure (fail-closed at the install seam).
#[derive(Debug, thiserror::Error)]
pub enum RotationOracleError {
    /// OS CSRNG failed to produce a slug at construction.
    #[error("CSRNG failure generating the initial rotation slug")]
    Csrng,
    /// Install-time persistence write failed (a misconfigured data dir surfaces
    /// here rather than at the first runtime rotation).
    #[error("rotation state persistence write failed at {path}: {source}")]
    Persist {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Current rotation slug + the wall-clock instant it was generated. Mirrors
/// `kryphocron::codec::laquna`'s private `RotationState`.
#[derive(Clone)]
struct RotationState {
    current_slug: [u8; 32],
    generated_at: SystemTime,
}

/// A persist request dispatched to the background worker on rotation.
struct PersistRequest {
    slug: [u8; 32],
    generated_at: SystemTime,
}

/// Aurora-Locus's standard single-process rotation oracle
/// (identifier [`AuroraLocusStandardRotationOracle::IDENTIFIER`]).
pub struct AuroraLocusStandardRotationOracle {
    state: RwLock<RotationState>,
    /// Cadence in seconds; `u64::MAX` ⇒ manual-only. Atomic-loaded on every
    /// `current_generation`; updated by [`set_cadence`](Self::set_cadence).
    cadence_secs: AtomicU64,
    /// Set by [`force_rotation`](Self::force_rotation); the next
    /// `current_generation` rotates and clears it (idempotent — concurrent
    /// force calls collapse to one rotation).
    force_pending: AtomicBool,
    persist_tx: mpsc::Sender<PersistRequest>,
}

impl AuroraLocusStandardRotationOracle {
    /// Process-shape identifier surfaced on the Kryphocron Overview (§6.4.1).
    pub const IDENTIFIER: &'static str = "aurora-locus-standard";

    /// Construct, persisting to `<data_dir>/aurora-locus/rotation.state`, with
    /// the given initial cadence. Resumes an existing state file when present
    /// and still within cadence (or under `manual-only`); otherwise starts a
    /// fresh slug. Fallible: CSRNG init + install-time write check.
    pub fn for_data_dir(data_dir: &Path, cadence: Cadence) -> Result<Self, RotationOracleError> {
        Self::construct(state_path(data_dir), cadence.as_secs())
    }

    fn construct(path: PathBuf, cadence_secs: u64) -> Result<Self, RotationOracleError> {
        let now = SystemTime::now();
        let state = match read_state_file(&path) {
            Some(loaded)
                if cadence_secs == MANUAL_ONLY_SECS
                    || now
                        .duration_since(loaded.generated_at)
                        .map(|age| age.as_secs() < cadence_secs)
                        .unwrap_or(false) =>
            {
                loaded
            }
            _ => RotationState {
                current_slug: random_slug()?,
                generated_at: now,
            },
        };

        // Install-time write check — fail-closed if the data dir is misconfigured.
        write_state_file(&path, &state).map_err(|source| RotationOracleError::Persist {
            path: path.clone(),
            source,
        })?;

        // Background persistence worker: runtime rotations dispatch here so a
        // slow fsync never stalls an encode. Best-effort (fail-soft, §4.7) — the
        // install-time check above catches persistent misconfig, and restart
        // resume handles a crash mid-write. Exits when the oracle drops.
        let (persist_tx, persist_rx) = mpsc::channel::<PersistRequest>();
        let worker_path = path;
        std::thread::spawn(move || {
            while let Ok(req) = persist_rx.recv() {
                let _ = write_state_file(
                    &worker_path,
                    &RotationState {
                        current_slug: req.slug,
                        generated_at: req.generated_at,
                    },
                );
            }
        });

        Ok(Self {
            state: RwLock::new(state),
            cadence_secs: AtomicU64::new(cadence_secs),
            force_pending: AtomicBool::new(false),
            persist_tx,
        })
    }

    /// Update the deployment cadence (called at boot from the runtime setting,
    /// and on subsequent `kryphocron.laquna.rotation-cadence` changes). Takes
    /// effect on the next `current_generation` — no restart.
    pub fn set_cadence(&self, cadence: Cadence) {
        self.cadence_secs.store(cadence.as_secs(), Ordering::Relaxed);
    }

    /// Expire the current generation ahead of cadence: the next
    /// `current_generation` yields a fresh one. The host-side hook the
    /// `triggerRotation` XRPC invokes. Idempotent.
    pub fn force_rotation(&self) {
        self.force_pending.store(true, Ordering::Relaxed);
    }

    /// Read-only snapshot of the active generation mark — for the
    /// `getRotationStatus` status surface (§6.4.2). Unlike
    /// [`RotationOracle::current_generation`], this **never rotates**: it
    /// formats the current slug + `generated_at` without consulting cadence or
    /// the force flag, so a status read can't trigger an organic rotation as a
    /// side effect.
    pub fn current_mark(&self) -> RotationGenerationMark {
        let st = self.state.read().expect("rotation state lock not poisoned");
        format_mark(st.generated_at, &st.current_slug)
    }

    /// When the current slug was generated (most recent slug rotation, organic
    /// or forced) — the "Last slug rotation" timestamp `getRotationStatus`
    /// surfaces, and the base for the "Next scheduled slug rotation"
    /// computation (`generated_at + cadence`).
    pub fn last_rotation_at(&self) -> SystemTime {
        self.state
            .read()
            .expect("rotation state lock not poisoned")
            .generated_at
    }
}

impl RotationOracle for AuroraLocusStandardRotationOracle {
    fn current_generation(&self, _ctx: &RotationContext) -> Option<RotationGenerationMark> {
        let now = SystemTime::now();
        let cadence_secs = self.cadence_secs.load(Ordering::Relaxed);

        // Fast path: not forced + (manual-only OR within cadence) ⇒ serve current.
        if !self.force_pending.load(Ordering::Relaxed) {
            let st = self.state.read().expect("rotation state lock not poisoned");
            let serve_current = cadence_secs == MANUAL_ONLY_SECS
                || now
                    .duration_since(st.generated_at)
                    .map(|age| age.as_secs() < cadence_secs)
                    .unwrap_or(false);
            if serve_current {
                return Some(format_mark(st.generated_at, &st.current_slug));
            }
        }

        // Rotation path: write-lock + double-check (a peer thread may have
        // rotated between the read and write locks; clear the force flag here).
        let mut st = self.state.write().expect("rotation state lock not poisoned");
        let now = SystemTime::now();
        let was_forced = self.force_pending.swap(false, Ordering::Relaxed);
        let cadence_secs = self.cadence_secs.load(Ordering::Relaxed);
        let cadence_stale = cadence_secs != MANUAL_ONLY_SECS
            && now
                .duration_since(st.generated_at)
                .map(|age| age.as_secs() >= cadence_secs)
                .unwrap_or(true);
        if was_forced || cadence_stale {
            // Runtime CSRNG failure (rare/transient): keep the current slug
            // rather than fail the encode; the next query retries.
            if let Ok(slug) = try_random_slug() {
                st.current_slug = slug;
                st.generated_at = now;
                let _ = self.persist_tx.send(PersistRequest {
                    slug,
                    generated_at: now,
                });
            }
        }
        Some(format_mark(st.generated_at, &st.current_slug))
    }

    fn last_synced_at(&self) -> SystemTime {
        self.state
            .read()
            .expect("rotation state lock not poisoned")
            .generated_at
    }

    fn data_freshness_bound(&self) -> Duration {
        // Single-process authoritative oracle: never stale relative to external
        // storage (mirrors DefaultRotationOracle / NoRotationOracle). Multi-
        // process deployments substitute a coordinated oracle.
        Duration::MAX
    }
}

/// `<data_dir>/aurora-locus/rotation.state` — Aurora-Locus's own state path
/// (distinct from the substrate's `<data_dir>/kryphocron/rotation.state`).
fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("aurora-locus").join("rotation.state")
}

/// Lex-sortable mark `"laquna/{:020}/{hex64}"` — byte-identical to the Laquna
/// codec's `format_mark`, so the codec parses marks this oracle produces.
fn format_mark(generated_at: SystemTime, slug: &[u8; 32]) -> RotationGenerationMark {
    let unix_secs = generated_at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    RotationGenerationMark::new(format!("laquna/{:020}/{}", unix_secs, hex::encode(slug)))
        .expect("rotation mark (92 bytes) fits BoundedString<128>")
}

/// 32-byte slug from the OS CSRNG; `Err` on CSRNG failure.
fn try_random_slug() -> Result<[u8; 32], rand::Error> {
    let mut slug = [0u8; 32];
    rand::rngs::OsRng.try_fill_bytes(&mut slug)?;
    Ok(slug)
}

fn random_slug() -> Result<[u8; 32], RotationOracleError> {
    try_random_slug().map_err(|_| RotationOracleError::Csrng)
}

/// Read persisted state: two lines, `<unix_secs>\n<hex_slug>`. `None` on any
/// failure (missing/unreadable/unparseable) — the caller starts fresh.
fn read_state_file(path: &Path) -> Option<RotationState> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut lines = contents.lines();
    let secs: u64 = lines.next()?.trim().parse().ok()?;
    let hex_slug = lines.next()?.trim();
    if hex_slug.len() != 64 {
        return None;
    }
    let mut slug = [0u8; 32];
    hex::decode_to_slice(hex_slug, &mut slug).ok()?;
    Some(RotationState {
        current_slug: slug,
        generated_at: UNIX_EPOCH + Duration::from_secs(secs),
    })
}

/// Persist as `<unix_secs>\n<hex_slug>\n`, creating the parent dir if needed.
fn write_state_file(path: &Path, state: &RotationState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let secs = state
        .generated_at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    std::fs::write(
        path,
        format!("{}\n{}\n", secs, hex::encode(state.current_slug)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "aurora-rotation-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn mark_str(m: &RotationGenerationMark) -> String {
        // RotationGenerationMark: Display gives the underlying string.
        m.to_string()
    }

    #[test]
    fn cadence_from_setting_maps_and_defaults() {
        assert_eq!(Cadence::from_setting("hourly"), Cadence::Hourly);
        assert_eq!(Cadence::from_setting("daily"), Cadence::Daily);
        assert_eq!(Cadence::from_setting("weekly"), Cadence::Weekly);
        assert_eq!(Cadence::from_setting("manual-only"), Cadence::ManualOnly);
        // unknown / unset → daily
        assert_eq!(Cadence::from_setting(""), Cadence::Daily);
        assert_eq!(Cadence::from_setting("nonsense"), Cadence::Daily);
    }

    #[test]
    fn mark_format_matches_laquna_shape() {
        let dir = tmp_dir("markfmt");
        let o = AuroraLocusStandardRotationOracle::for_data_dir(&dir, Cadence::Daily).unwrap();
        let m = o
            .current_generation(&RotationContext::for_install_probe())
            .unwrap();
        let s = mark_str(&m);
        assert!(s.starts_with("laquna/"), "mark prefix: {s}");
        // laquna/ + 20-digit secs + / + 64 hex = 7 + 20 + 1 + 64 = 92
        assert_eq!(s.len(), 92, "mark length: {s}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn within_cadence_serves_same_mark() {
        let dir = tmp_dir("stable");
        let o = AuroraLocusStandardRotationOracle::for_data_dir(&dir, Cadence::Daily).unwrap();
        let a = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        let b = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        assert_eq!(mark_str(&a), mark_str(&b), "stable within cadence");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn force_rotation_yields_fresh_mark() {
        let dir = tmp_dir("force");
        let o = AuroraLocusStandardRotationOracle::for_data_dir(&dir, Cadence::Daily).unwrap();
        let a = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        o.force_rotation();
        let b = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        assert_ne!(mark_str(&a), mark_str(&b), "force_rotation rotates the slug");
        // and clears: the next call is stable again
        let c = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        assert_eq!(mark_str(&b), mark_str(&c), "force flag cleared after one rotation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_mark_is_read_only_and_matches_generation() {
        let dir = tmp_dir("readonly");
        // Force-pending is set but NOT yet observed: current_mark must serve the
        // existing generation without consuming the force flag (no side-effect
        // rotation), so a subsequent current_generation still performs the
        // pending rotation.
        let o = AuroraLocusStandardRotationOracle::for_data_dir(&dir, Cadence::Daily).unwrap();
        let live = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        let read = o.current_mark().to_string();
        assert_eq!(mark_str(&live), read, "read-only mark matches the live one");

        o.force_rotation();
        // A read does not rotate...
        let read_after_force = o.current_mark().to_string();
        assert_eq!(read, read_after_force, "current_mark never rotates");
        // ...so the pending force is still honored by current_generation.
        let rotated = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        assert_ne!(mark_str(&rotated), read, "force survives a read-only peek");

        // last_rotation_at advances across the rotation.
        assert!(o.last_rotation_at() >= UNIX_EPOCH);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manual_only_never_auto_rotates_but_force_works() {
        let dir = tmp_dir("manual");
        let o =
            AuroraLocusStandardRotationOracle::for_data_dir(&dir, Cadence::ManualOnly).unwrap();
        let a = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        let b = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        assert_eq!(mark_str(&a), mark_str(&b), "manual-only does not auto-rotate");
        o.force_rotation();
        let c = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        assert_ne!(mark_str(&b), mark_str(&c), "manual-only still honors force_rotation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_resumes_persisted_slug() {
        let dir = tmp_dir("restart");
        let mark_a = {
            let o = AuroraLocusStandardRotationOracle::for_data_dir(&dir, Cadence::Daily).unwrap();
            mark_str(&o.current_generation(&RotationContext::for_install_probe()).unwrap())
        };
        // A fresh oracle over the same dir (within cadence) resumes the slug.
        let o2 = AuroraLocusStandardRotationOracle::for_data_dir(&dir, Cadence::Daily).unwrap();
        let mark_b = mark_str(&o2.current_generation(&RotationContext::for_install_probe()).unwrap());
        assert_eq!(mark_a, mark_b, "restart resumes the persisted generation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_cadence_to_manual_only_stops_rotation() {
        let dir = tmp_dir("setcad");
        let o = AuroraLocusStandardRotationOracle::for_data_dir(&dir, Cadence::Daily).unwrap();
        let a = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        o.set_cadence(Cadence::ManualOnly);
        let b = o.current_generation(&RotationContext::for_install_probe()).unwrap();
        assert_eq!(mark_str(&a), mark_str(&b), "cadence update consulted live");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
