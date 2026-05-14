# Super — Web

The user-facing web app for the Super platform. Built with
[Vite+](https://viteplus.dev), React 19, Tailwind CSS v4 and
[shadcn/ui](https://ui.shadcn.com) (radix base). The same auth endpoints
served by the axum platform server power this app.

## Quick start

```bash
pnpm install
pnpm dev          # http://localhost:5173, proxies /auth → :3000
```

In a second terminal:

```bash
# from the repo root
cargo run -p server
```

## Scripts

| Script              | What it does                                     |
| ------------------- | ------------------------------------------------ |
| `pnpm dev`          | Vite+ dev server                                 |
| `pnpm build`        | `tsc -b && vp build` → `dist/`                   |
| `pnpm preview`      | Preview the production build                     |
| `pnpm test`         | Vitest one-shot run                              |
| `pnpm test:watch`   | Vitest watch mode                                |
| `pnpm lint`         | Oxlint via `vp lint`                             |
| `pnpm format`       | `vp fmt --check`                                 |
| `pnpm format:write` | `vp fmt --write`                                 |
| `pnpm typecheck`    | `tsc -b --noEmit`                                |
| `pnpm check`        | Format + lint + typecheck in one go (`vp check`) |

## Layout

```
web/
├── public/                       # static assets
├── src/
│   ├── components/
│   │   ├── ui/                   # shadcn-managed components (do not hand-edit)
│   │   └── RequireAuth.tsx       # route guard
│   ├── lib/
│   │   ├── api.ts                # /auth/* client + ApiError
│   │   ├── auth-context.ts       # Auth context + useAuth hook
│   │   ├── auth.tsx              # AuthProvider (token storage + me() probe)
│   │   ├── pkce.ts               # PKCE verifier/challenge helpers
│   │   └── utils.ts              # cn()
│   ├── pages/
│   │   ├── HomePage.tsx
│   │   ├── LoginPage.tsx
│   │   └── RegisterPage.tsx
│   ├── test/
│   │   └── setup.ts              # jest-dom + cleanup wiring
│   ├── App.tsx                   # router + providers
│   ├── index.css                 # Tailwind + design tokens (Linear/Apple)
│   └── main.tsx
├── components.json               # shadcn config
├── vite.config.ts                # Vite + plugins + lint + test config
└── package.json
```

## Design tokens

The theme in `src/index.css` mirrors the canonical specs at
`../design-system/` (`LINEAR.md` and `APPLE.md`). Two layers of CSS
variables:

- **Raw palette tokens** — defined under `:root` (light, Apple-inspired)
  and `.dark` (Linear-style dark surface).
- **Semantic tokens** — `--background`, `--foreground`, `--primary`,
  `--muted-foreground`, etc. Wired into Tailwind v4 via `@theme inline`
  so utilities like `bg-background` and `text-muted-foreground` resolve
  to the right values in either mode.

Add components via the CLI; do not hand-write UI primitives:

```bash
pnpm dlx shadcn@latest add <component>
```

## Adding a new shadcn component

```bash
pnpm dlx shadcn@latest add button card field
```

Per the project's `CLAUDE.md`, only the standard shadcn registry,
`assistant-ui`, and `tool-ui` are allowed component sources.

## Notes for current Node versions

`vp` (Vite+) loads `vite.config.ts` natively, which requires
**Node 22.18+**. A `.nvmrc` is checked in. On older Node we set
`NODE_OPTIONS=--import tsx/esm` via `cross-env` so the scripts still
work; CI runs on the `.nvmrc` version directly.

## Server integration

See [`../docs/web-integration.md`](../docs/web-integration.md) for the
proposed `tower-http` static-file mount that lets the axum binary serve
`web/dist/` on `/`.
