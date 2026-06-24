# Contributing to emthin

For architecture and internals, read [`CLAUDE.md`](CLAUDE.md) and the per-crate `crates/*/CLAUDE.md` files.

## Setup

Install the pinned Rust toolchain (currently 1.92.0 — `rustup show` inside
`crates/emthin/` picks it up from `rust-toolchain.toml`) and the system libs.
On Arch:

```
sudo pacman -S wayland libxkbcommon libinput mesa seatd \
               fontconfig freetype2 ttf-dejavu
```

Test client: `wl-clipboard`, `xclip`. On Arch:

```
sudo pacman -S wl-clipboard xclip
```

`xwayland-satellite` is **not** required to run the test suite —
emthin's satellite supervisor probes the binary at startup and falls
back to "Wayland-only" if missing. Install it (AUR on Arch) only when you
want to exercise emthin end-to-end against real X applications.

## Local checks

```
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## Commits & PRs

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/).

```
feat:     new user-facing feature
fix:      bug fix
perf:     performance improvement
refactor: no behavior change
docs:     documentation only
test:     tests only
ci:       CI config
build:    build system / deps
```

Open PRs against `main`. CI must be green.
