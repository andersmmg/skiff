# Skiff

A native download manager for desktop, built with Rust and GPUI. The UI is a
native GPU-rendered app (no WebView); the download engine is
[aria2-next](https://github.com/AnInsomniacy/aria2-next), driven over its local
JSON-RPC interface — the same engine architecture Rayburst uses, in a fully
native shell.

## Architecture

The workspace is split the way [sonora](https://github.com/sonorahq/sonora)
splits its GPUI app: a thin binary crate plus library crates with narrow jobs.

```
crates/
├── skiff/    # binary: logging, engine startup, window creation
├── ui/       # GPUI views: root layout, sidebar, task rows, add dialog, theme
├── state/    # app state: task store, settings, engine-event bridge
└── engine/   # aria2-next: sidecar process manager + JSON-RPC client
```

- **engine** finds or spawns an `aria2-next` sidecar bound to a random
  localhost RPC port with a random secret, supervises it, and polls task state
  through one `system.multicall` round trip per tick. It has no UI
  dependencies. If no engine binary is found, it falls back to attaching to an
  already-running instance (e.g. `aria2-next --enable-rpc`).
- **state** owns the task list, settings, and connection status as GPUI
  entities. A bridge task drains engine events on the background executor and
  applies snapshots on the main thread, so views only ever observe entities.
- **ui** renders everything with GPUI. Task rows are plain stateless views;
  the root view observes the task list and connection entities.

## Building

```sh
cargo run -p skiff
```

Requirements:

- Rust (latest stable; edition 2024)
- `aria2-next` on `PATH`, next to the `skiff` binary, or pointed to by the
  `SKIFF_ARIA2_NEXT` environment variable. Without it Skiff still starts and
  shows the engine as unavailable.
- Linux: the usual GPUI system dependencies (Vulkan driver, fontconfig,
  Wayland/X11 libraries).

## Development notes

- `libc` is pinned to 0.2.189 in `Cargo.lock`: `xattr 0.2.3` (pulled in by
  gpui's HTTP client) does not compile against libc 0.2.190 yet. Drop the
  pin once a fixed `xattr` is released.
- Skiff's RPC polling is one `system.multicall` per tick (default 500 ms),
  so UI refresh costs a single loopback round trip regardless of task count.
- Task state lives in aria2-next's `--state-dir`, not in `.aria2` files next
  to payloads, so downloads resume after crashes without polluting download
  folders.

## Configuration

| Variable           | Purpose                                              |
| ------------------ | ---------------------------------------------------- |
| `SKIFF_ARIA2_NEXT` | Explicit path to the `aria2-next` binary             |
| `RUST_LOG`         | Log filtering (`skiff=debug,engine=debug` to start)  |

The engine's own state (resume checkpoints) lives under the platform state
directory (`~/.local/state/skiff/aria2` on Linux), never next to downloaded
files. Downloads default to the platform downloads directory.

## Status

Scaffold: workspace, engine supervision, RPC polling, and the core UI are in
place. Not yet: torrent file picking, history, tray integration, themes beyond
dark, i18n.
