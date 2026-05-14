# Web frontend integration

**Status:** Scaffold — auth pages + tooling done, server hand-off pending.
**Date:** 2026-05-15
**Branch:** `feat/web-frontend`

The web app lives at [`web/`](../web). It is a standalone Vite+ React/TypeScript
project that consumes the same `/auth/*` endpoints already exposed by the
platform server in [`server/src/routes/auth.rs`](../server/src/routes/auth.rs).

## Local development

```bash
cd web
pnpm install
pnpm dev          # http://localhost:5173 (proxies /auth to :3000)
pnpm check        # vp check — format + lint + typecheck
pnpm test         # vp test  — vitest one-shot
pnpm build        # tsc -b && vp build → web/dist/
```

The dev server proxies `/auth/**` to `http://localhost:3000` (the axum
server). Run the server with `cargo run -p server` in another terminal.

> **Node ≥ 22.18.0** is required so Vite+ can natively load
> `vite.config.ts`. A `.nvmrc` is committed; on older Node we fall back to
> `tsx/esm` via `cross-env NODE_OPTIONS` in the `package.json` scripts.

## Production wiring (server-side, not yet implemented)

For the v1 deploy the SPA ships inside the same axum binary. The server
just needs to:

1. **Serve static files from `web/dist/`** (built once during release).
2. **Fall back to `index.html`** for any non-`/auth/*` GET so client-side
   routing keeps working on hard reloads of `/login`, `/register`, etc.

The proposed change in [`server/src/main.rs`](../server/src/main.rs) is small.
First, add to `server/Cargo.toml`:

```toml
tower-http = { version = "0.5", features = ["cors", "fs"] }
```

Then mount the static directory after the existing `/auth` routes:

```rust
use tower_http::services::{ServeDir, ServeFile};

let web_dir = std::env::var("SUPER_WEB_DIST")
    .unwrap_or_else(|_| "web/dist".into());

let index = ServeFile::new(format!("{web_dir}/index.html"));
let static_files = ServeDir::new(&web_dir).fallback(index.clone());

let app = Router::new()
    .nest("/auth", routes::auth::routes_with_state(service.clone()))
    .fallback_service(static_files)
    .layer(cors);
```

Notes:

- The `/auth/*` nest is matched first, so API requests are unaffected.
- `ServeDir::fallback(ServeFile)` gives us SPA history-mode support without
  any extra middleware: any unknown path returns `index.html` and React
  Router takes over.
- `SUPER_WEB_DIST` lets devs point at the dev server's output (or omit it in
  release builds where the bundled path is fine).

### Build & release

The CI workflow at
[`.github/workflows/web-ci.yml`](../.github/workflows/web-ci.yml) already
uploads `web/dist` as an artifact on every PR and on `main`. The release
pipeline (separate, TBD) should download that artifact and place it at
`server/web/dist/` (or wherever `SUPER_WEB_DIST` points) before running
`cargo build --release -p server`.

For now, a developer can preview the integration end-to-end with:

```bash
(cd web && pnpm build)
SUPER_WEB_DIST=$(pwd)/web/dist cargo run -p server
open http://localhost:3000/
```

## What is **not** in scope on this branch

- Modifying `server/src/main.rs` — left untouched so the change is a tiny,
  reviewable PR after the web work merges.
- Bundling `web/dist` into the Rust binary via `include_dir!` or
  `rust-embed` — only needed once we ship single-binary releases.
- A `tower-http`-side rate limiter or auth middleware in front of the SPA
  files — none of the static assets are sensitive.

## What ships on this branch

- [`web/`](../web) — Vite+ React/TS app (`vp create vite -- --template
  react-ts`).
- shadcn/ui (radix base) with the
  [`design-system/`](../../design-system/) Linear/Apple tokens mapped to
  semantic CSS variables in `web/src/index.css`.
- `/login`, `/register`, `/` routes via React Router 7. `/login` and
  `/register` call the existing `/auth/login`, `/auth/register`, and
  `/auth/authorize` endpoints. PKCE verifier/challenge generation lives
  in `web/src/lib/pkce.ts`.
- Toolchain: `vp dev`, `vp build`, `vp check` (fmt + lint + typecheck),
  `vp test` (vitest + jsdom + Testing Library).
- CI: `.github/workflows/web-ci.yml` runs the full check on PRs to
  `main` that touch `web/**`.
