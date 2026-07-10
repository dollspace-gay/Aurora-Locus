# install.sh multi-distro preflight smoke

A minimal, containerized check that `install.sh`'s **distro detection** and
**package-manager preflight** work on the RHEL-family and Arch distros the
script claims to support — the branches that are easy to break and hard to
notice from a Debian/Ubuntu dev box.

## What it checks

For each distro, `install.sh` should:

| Distro     | `detect_distro` via      | resolves to | toolchain preflight |
|------------|--------------------------|-------------|---------------------|
| Rocky      | `/etc/redhat-release`    | `rhel`      | `dnf install …`     |
| AlmaLinux  | `/etc/redhat-release`    | `rhel`      | `dnf install …`     |
| Arch       | `/etc/arch-release`      | `arch`      | `pacman -S …`       |

Each `Dockerfile.<distro>` starts from that distro's base image, installs only
minimal prereqs (git + openssl; curl + bash ship in the base), and runs
`install.sh` non-interactively far enough to exercise `ensure_c_toolchain`. The
build's final `RUN` asserts the correct branch fired **and** left a working
toolchain (`cc` + `pkg-config` + openssl headers) — so a green `docker build`
means that distro's preflight passed, and a red build means it didn't.

This is a **preflight** smoke, not a full install: `--skip-rustup` plus the
absence of `cargo` means `bootstrap_rustup`, `proto-blue-codegen`, and the Rust
build are all skipped. The only heavy step is the toolchain install the test
targets.

## Running

```sh
test/install-sh-smoke/run-smoke.sh
```

Builds all three images and prints a `PASS`/`FAIL` summary (exit non-zero if any
failed). Or build one manually — stage the two files into a temp context first:

```sh
stage="$(mktemp -d)"; cp install.sh .env.example "$stage/"
docker build -f test/install-sh-smoke/Dockerfile.rocky -t aurora-install-smoke:rocky "$stage"
rm -rf "$stage"
```

The build context is a temp dir holding only `install.sh` + `.env.example`, **not
the repo root** — the top-level `.dockerignore` excludes `install.sh` (it isn't
an input to the main docker-compose image), so a repo-root build would starve the
`COPY`. The runner stages the **working-tree** `install.sh` rather than `git
clone`-ing, because the current work branch is not pushed to a remote — a clone
would test a stale `install.sh`.

## When a distro fails

Treat a `SMOKE-FAIL` as a **finding to report, not to fix here**: open a
chainlink follow-up with the captured `install.log` and leave `install.sh`
alone pending review. The preflight package sets or `detect_distro` heuristics
are deployment-sensitive and shouldn't be changed unattended.
