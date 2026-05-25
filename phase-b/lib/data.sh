# phase-b/lib/data.sh — fresh-data-dir helpers for per-scenario isolation.
#
# Source this file. Provides:
#
#   pb_fresh_data_dir <role>     # rm -rf + mkdir for the role's data dir
#   pb_fresh_all                 # both roles + the shared root
#   pb_purge_legacy              # remove the v0.5 hardcoded per-arc data dirs
#
# Why fresh-not-soft-reset: content-addressed IDs (record CIDs, blob
# CIDs, commit CIDs) are deterministic across runs given identical
# inputs. A soft-reset that re-uses the data dir cross-contaminates
# CID sets across scenarios — Phase B then exercises the wrong
# preconditions (see memory: feedback_phase_b_state_isolation).
#
# Setup-only-never-judgment: these helpers create directories and
# refuse to run if a role var is unset. They do NOT decide whether a
# scenario passed.

set -u

# -----------------------------------------------------------------------------
# pb_fresh_data_dir <role>
# Wipe (rm -rf) and recreate the role's data dir. Required between
# scenarios that assert side-effects against fresh state.
# -----------------------------------------------------------------------------

pb_fresh_data_dir() {
    local role="$1"
    local upper
    upper=$(echo "$role" | tr '[:lower:]' '[:upper:]')
    local data_var="${upper}_DATA"
    local data="${!data_var:-}"

    if [ -z "$data" ]; then
        echo "[pb-data] cannot fresh-data-dir role=$role: ${data_var} is unset" >&2
        return 1
    fi
    # Guard rail: refuse if the path looks dangerous (root, home, /etc).
    case "$data" in
    /|/home|/home/*|/etc|/etc/*|/usr|/usr/*|/var|/var/*)
        echo "[pb-data] refusing to wipe '$data' (looks like a system path)" >&2
        return 1
        ;;
    esac

    rm -rf "$data"
    mkdir -p "$data"
    echo "[pb-data] fresh role=$role  path=$data"
}

# -----------------------------------------------------------------------------
# pb_fresh_all — wipe both A and B data dirs + the shared root.
# Common at scenario start (Setup-to-confirmed-up block).
# -----------------------------------------------------------------------------

pb_fresh_all() {
    pb_fresh_data_dir a
    pb_fresh_data_dir b

    # Shared root (/tmp/aurora-phase-b) — wipe only the role children;
    # leave the root in place so re-runs don't recreate it from scratch.
    :
}

# -----------------------------------------------------------------------------
# pb_purge_legacy — remove the v0.5 hardcoded per-arc data dirs the
# operator-driven markdown left behind (e.g. /tmp/aurora-arc17-phase-b/*).
# Optional; helps avoid confusion between old markdown-driven runs and
# the new harness layout.
# -----------------------------------------------------------------------------

pb_purge_legacy() {
    local stale
    for stale in /tmp/aurora-arc12-phase-b /tmp/aurora-arc13-phase-b /tmp/aurora-arc14-phase-b /tmp/aurora-arc15-phase-b /tmp/aurora-arc16c-phase-b /tmp/aurora-arc16d-phase-b /tmp/aurora-arc16e-phase-b /tmp/aurora-arc16f-phase-b /tmp/aurora-arc17-phase-b; do
        if [ -d "$stale" ]; then
            rm -rf "$stale"
            echo "[pb-data] purged legacy $stale"
        fi
    done
}

:
