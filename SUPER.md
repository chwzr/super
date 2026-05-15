# Design System — Super

> Category: Developer Tools & CLI
> Coding agent for engineers. Dark-native canvas, Apple blue precision, Berkeley Mono identity.

## 0. Brand Mark

The Super logo is the `◆` diamond glyph rendered in Apple Action Blue (`#0071e3`) with a 6px ambient glow (`filter: drop-shadow(0 0 6px rgba(0,113,227,0.45))`). It appears alongside the wordmark "super" set in Berkeley Mono weight 700 at 16px. The logo assembly is always displayed as a single inline-flex unit with a 9px gap between glyph and wordmark.

**ASCII Representation:** `diamond_4.txt` is the canonical ASCII art representation of the Super logo — a compressed diamond form built from Unicode box-drawing and punctuation characters. It is used for terminal contexts, splash screens, and any environment where the SVG/CSS mark cannot render.

```
        _______
      .'_/_|_\_'.
      \`\  |  /`/
       `\\ | //'
         `\|/`
           `
```

The 3D animated diamond on the landing page (a Three.js octahedron with a custom GLSL shader) dissolves into an ASCII character field on scroll, rendered using Berkeley Mono and drawn from the character ramp `' .·,:;-~+=*x×X%#%@▒▓█'`. This ASCII canvas is the animated expression of the same mark encoded statically in `diamond_4.txt`.

---

## 1. Visual Theme & Atmosphere

Super is a dark-native product — `#08090a` is not a dark theme applied to a light-first design but the native medium from which all content emerges. The visual language is built from two well-understood references and resolves them into a third thing: Linear's disciplined near-black surface hierarchy and Inter Variable typographic precision merged with Apple's action-blue accent family and capsule-geometry CTA language.

The result is a CLI marketing page that reads as engineering artifact more than sales surface. Typography is tight and compressed at scale — aggressive negative tracking (-0.04em) on display headings — while body copy breathes at relaxed line-heights (1.55–1.6) against a dark canvas that provides natural section separation without dividers or decorative elements. The sole chromatic accent is Apple Action Blue (`#0071e3` / `#2997ff`), used only for interactive and semantic roles: CTAs, eyebrows, phase indicators, and terminal prompts. Everything else is achromatic — dark backgrounds stepping through four luminance levels, white text stepping through four opacity grades.

The ambient depth layer is a pair of radial gradients — a diffuse blue corona above the fold and a softer glint at the lower-right — that give the page dimensional presence without materially changing the rendering. These gradients are `position: fixed` and `pointer-events: none`, so they behave as an atmospheric layer beneath all content.

Berkeley Mono is the brand typeface. It carries the logo wordmark, all eyebrow labels, code blocks, terminal bodies, and the install snippet. Inter Variable handles all prose hierarchy. The combination is engineer-native: the typeface you trust for code, the typeface you trust for reading, in calibrated tension.

**Key Characteristics:**
- Dark-native: `#08090a` page canvas, `#0f1011` panel, `#191a1b` elevated surface, `#28282c` secondary surface
- Apple blue accent family as the only chromatic color: `#0071e3` action, `#2997ff` interactive highlight, `#0066cc` link
- Inter Variable with `"cv01", "ss03"` OpenType features globally — same geometric optimization as Linear
- Berkeley Mono as the brand mono typeface (not a fallback): wordmark, eyebrows, terminal, code
- Signature font weight 510 for all UI text emphasis — Linear's between-weight, neither regular nor medium
- Aggressive negative letter-spacing at display scale: -0.04em on both h1 and h2
- Semi-transparent white borders throughout: `rgba(255,255,255,0.05)` → `0.08` → `0.12`
- Button geometry: `border-radius: 980px` pill for all primary and ghost CTAs
- Fixed atmospheric background: two radial blue gradients that provide ambient glow without structural weight
- Scroll-driven ASCII dissolve on the hero diamond: 3D → character field transition using Berkeley Mono

---

## 2. Color Palette & Roles

### Background Surfaces
- **Page Canvas** (`#08090a`): The native dark background of all marketing surfaces. Near-black with a barely-cool blue undertone, identical to Linear's marketing black.
- **Deepest Black** (`#010102`): The darkest reserved step — used as the terminal background (`#050608` in practice) and the deepest compositing layer.
- **Panel Dark** (`#0f1011`): Panel and sidebar layer — one luminance step above the canvas.
- **Surface Level 1** (`#191a1b`): Elevated surface areas, card backgrounds, raised containers.
- **Surface Level 2** (`#28282c`): Secondary elevated surface — the lightest dark step used for hover states and deep utility components.

### Text & Content
- **Primary Text** (`#f7f8f8`): Near-white, not pure white — prevents harshness on deep dark backgrounds. The default text color for all headings and critical copy.
- **Secondary Text** (`#d0d6e0`): Cool silver-gray. Body copy, card descriptions, nav links in default state.
- **Tertiary Text** (`#8a8f98`): Muted gray. Hero lede, section lede, phase body text, de-emphasized content.
- **Quaternary Text** (`#62666d`): Most subdued — timestamps, install prompt copy, labels on secondary controls.

### Brand & Accent (Apple Blue Family)
- **Apple Action Blue** (`#0071e3`): Primary interactive and brand signal. Used for primary button fill, eyebrow labels, phase timeline markers, card icons, checkpoint borders, and the logo glyph. This is the only chromatic fill color in the system.
- **Apple Blue High-Luminance** (`#2997ff`): Bright interactive variant for dark surface contexts. Terminal prompt color, eyebrow dot, `◆` icon glyph in smaller usage, interactive accent on dark panels.
- **Apple Blue Link** (`#0066cc`): Inline link blue — not used in this marketing surface but held in the token set for prose contexts.
- **Apple Blue Hover** (`#1d83eb`): Primary button hover state — a single step lighter than the action blue.

### Accent Tints (Context-Only)
- **Blue Tint 10%** (`rgba(0,113,227,0.10)`): Card icon background, checkpoint row tint.
- **Blue Tint 06%** (`rgba(0,113,227,0.06)`): Eyebrow pill background.
- **Blue Tint 05%** (`rgba(41,151,255,0.05)`): Ambient atmospheric radial gradient (lower-right).
- **Blue Tint 10% Ambient** (`rgba(0,113,227,0.10)`): Hero atmospheric radial gradient (top-center).
- **Blue Glow** (`rgba(0,113,227,0.45)`): Logo glyph drop-shadow.
- **Phase Glow** (`rgba(0,113,227,0.55)`): Phase timeline marker outer glow ring.

### Borders & Dividers
- **Border Subtle** (`rgba(255,255,255,0.05)`): Default section dividers, nav bottom border, footer top border, phase row separators.
- **Border Standard** (`rgba(255,255,255,0.08)`): Cards, terminal container, workflow container, ghost button default, input-style elements.
- **Border Strong** (`rgba(255,255,255,0.12)`): Ghost button hover state, elevated interactive containers.
- **Blue Border Muted** (`rgba(0,113,227,0.25)`): Eyebrow pill border, card icon border, phase marker ring.
- **Blue Checkpoint Border** (`rgba(0,113,227,0.15)`): Checkpoint row bottom border.

### Status
- **Success Green** (`#27a644`): Terminal `run_tests` pass indicator — a single semantic status color, used only for verified success states. Not a brand color.

### Gradient System
- **Atmospheric Top** (`radial-gradient(1200px 600px at 50% -200px, rgba(0,113,227,0.10), transparent 60%)`): Fixed blue corona above the fold.
- **Atmospheric Corner** (`radial-gradient(900px 500px at 90% 100%, rgba(41,151,255,0.05), transparent 60%)`): Subtle bottom-right glint.
- **Phase Timeline** (`linear-gradient(to bottom, transparent 0%, rgba(0,113,227,0.25) 12%, var(--border) 50%, rgba(0,113,227,0.25) 88%, transparent 100%)`): Vertical connector line on the phase list, fades at both ends.
- **Nav Backdrop** (`rgba(8,9,10,0.72)` + `backdrop-filter: blur(14px) saturate(140%)`): Frosted-glass nav bar — not a flat surface.

---

## 3. Typography Rules

### Font Family
- **Display & Prose**: `Inter Variable`, fallbacks: `Inter`, `-apple-system`, `BlinkMacSystemFont`, `SF Pro Display`, `Segoe UI`, `Roboto`, `Helvetica`, `Arial`, `sans-serif`
- **Monospace & Brand**: `Berkeley Mono` (variable weight, loaded via `BerkeleyMonoVariable.woff2`), fallbacks: `ui-monospace`, `SF Mono`, `Menlo`, `monospace`
- **OpenType Features**: `font-feature-settings: 'cv01', 'ss03'` applied globally to all text. These are identity-level features — cv01 (single-story alternate 'a') and ss03 (geometric letterform adjustments) transform Inter into Super's typeface.
- **Font Smoothing**: `-webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale` — required on dark backgrounds to prevent subpixel rendering artifacts.

### Hierarchy

| Role | Family | Size | Weight | Line Height | Letter Spacing | Notes |
|------|--------|------|--------|-------------|----------------|-------|
| Hero Display | Inter Variable | clamp(38px, 6vw, 64px) | 510 | 1.02 | -0.04em | H1 — tight, compressed, fluid |
| Section Display | Inter Variable | clamp(34px, 4.8vw, 52px) | 510 | 1.05 | -0.04em | H2 — section headings, max 720px wide |
| Phase Title | Inter Variable | 22px | 590 | — | -0.022em | Feature phase titles in ordered list |
| Card Title | Inter Variable | 19px | 590 | — | -0.018em | Card h3 — strong but compact |
| Hero Lede | Inter Variable | 19px | 400 | 1.55 | -0.01em | Primary hero paragraph, max 600px wide |
| Section Lede | Inter Variable | 18px | 400 | 1.60 | -0.01em | Feature section introduction copy |
| Body / Phase | Inter Variable | 15–16px | 400 | 1.60 | -0.01em | Card body (15px), phase body (16px) |
| Nav Links | Inter Variable | 13px | 510 | — | -0.01em | Horizontal navigation text |
| Button Label | Inter Variable | 13px (std) / 14px (lg) | 510 | — | -0.01em | Pill button text |
| H1 Accent Span | Inter Variable | inherit | inherit | inherit | inherit | `.accent` class, colored `--text-3` |
| Eyebrow | Berkeley Mono | 11px | 500 | — | 0.22em | Uppercase, color `--apple-blue-hi` |
| H2 Eyebrow | Berkeley Mono | 11px | 500 | — | 0.22em | Same as eyebrow but pill-bordered |
| Phase Number | Berkeley Mono | 12px | 400 | — | 0.10em | `PHASE 01` label, blue, uppercase |
| Pin Tag | Berkeley Mono | 10px | 400 | — | 0.12em | Inline status pill on phase titles |
| Logo Wordmark | Berkeley Mono | 16px | 700 | 1 | -0.01em | "super" wordmark next to `◆` glyph |
| Terminal Body | Berkeley Mono | 13px | 400 | 1.70 | — | All terminal and workflow code |
| Install Snippet | Berkeley Mono | 13px | 400 | — | -0.01em | npm install command block |
| Workflow Steps | Berkeley Mono | 13px | 400 | 1.80 | — | Audit trail step grid |
| Footer Copy | Inter Variable | 13px | 510 | — | — | Footer navigation links |
| Footer Copyright | Berkeley Mono | 11px | 400 | — | 0.05em | "© 2026 · BUILT IN PUBLIC" |

### Principles
- **510 as the UI workhorse**: Font weight 510 is used for all UI text that needs emphasis without heaviness — nav links, buttons, eyebrows, captions. It is Super's signature weight, adopted from Linear's typographic grammar.
- **Compression at headline scale**: Both h1 and h2 use `-0.04em` letter-spacing — a single consistent tightening rule across all display text. This creates a machined density at large sizes.
- **Clamp-based fluid scaling**: Hero and section headings scale between a minimum and maximum using `clamp()`, not discrete breakpoints. The viewport range 38px→64px (h1) and 34px→52px (h2) provides smooth adaptation.
- **Dual-axis color signaling**: Heading color is `--text` (`#f7f8f8`); lede / supporting copy is `--text-3` (`#8a8f98`). This two-level contrast creates hierarchy without weight changes.
- **Berkeley Mono as identity marker**: Every label, prefix, or technical element uses Berkeley Mono, not Inter. This creates a clear semantic split: prose = Inter, system = mono.
- **Uppercase only for eyebrows and phase numbers**: Uppercase is reserved strictly for Berkeley Mono eyebrow labels and phase counters. It is never applied to Inter prose.

---

## 4. Component Stylings

### Buttons

**Primary Pill (Default)**
- Background: `#0071e3`
- Text: `#ffffff`
- Padding: `8px 18px`
- Border radius: `980px` (full pill)
- Border: `1px solid transparent`
- Hover: Background `#1d83eb`
- Active: `transform: scale(0.98)`
- Font: Inter Variable 13px weight 510, letter-spacing -0.01em
- Use: Primary CTA — "Get early access", "Request an invite"

**Primary Pill Large**
- Same as above but padding `11px 22px`, font-size `14px`
- Use: Hero-adjacent CTAs where visual weight must match h1

**Ghost Pill (Secondary)**
- Background: `rgba(255,255,255,0.03)`
- Text: `--text-2` (`#d0d6e0`)
- Border: `1px solid rgba(255,255,255,0.08)`
- Hover text: `--text` (`#f7f8f8`)
- Hover background: `rgba(255,255,255,0.06)`
- Hover border: `rgba(255,255,255,0.12)`
- Active: `transform: scale(0.98)`
- Use: Secondary actions — "See how it works →", "Read the /explain spec →"

**Ghost Pill Large**
- Same ghost spec, padding `11px 22px`, font-size `14px`
- Use: Hero-level secondary CTA paired with primary-lg

### Logo

- Container: `inline-flex`, `align-items: center`, `gap: 9px`
- Glyph `◆`: Berkeley Mono, 17px, color `#0071e3`, `transform: translateY(-0.5px)`, `filter: drop-shadow(0 0 6px rgba(0,113,227,0.45))`
- Wordmark "super": Berkeley Mono, 16px, weight 700, letter-spacing -0.01em, color `--text`
- Footer variant: identical but font-size `13px`

### Navigation

- Position: `position: sticky; top: 0; z-index: 50`
- Background: `rgba(8,9,10,0.72)` — semi-transparent, not opaque
- Backdrop: `blur(14px) saturate(140%)` — frosted glass effect
- Border bottom: `1px solid rgba(255,255,255,0.05)`
- Inner container: max-width 1200px, padding `14px 24px`, flex space-between
- Links: Inter Variable 13px weight 510, color `--text-2`, hover `--text`, transition 0.15s
- Mobile: text links hidden below 720px; gap reduces from 28px to 12px

### Cards

**Feature Card (Standard)**
- Background: `rgba(255,255,255,0.02)`
- Border: `1px solid rgba(255,255,255,0.08)`
- Border radius: `14px`
- Padding: `28px`
- Hover background: `rgba(255,255,255,0.035)`
- Hover border: `rgba(255,255,255,0.12)`
- Transition: background/border/transform `0.2s ease`

**Card Icon**
- Size: `34px × 34px`
- Border radius: `9px`
- Background: `rgba(0,113,227,0.10)`
- Border: `1px solid rgba(0,113,227,0.25)`
- Color: `#2997ff`
- Font: Berkeley Mono, 16px, weight 700
- Margin bottom: `22px`
- Icons used: `◆`, `⟡`, `⟢`, `≡`, `↺`, `∗`, `⌘`, `⎈`, `⊕`

**Card Heading (h3)**
- 19px, weight 590, letter-spacing -0.018em, color `--text`

**Card Body (p)**
- 15px, weight 400, color `--text-3`, line-height 1.6, letter-spacing -0.01em

### Eyebrow Labels

**Hero Eyebrow**
- Font: Berkeley Mono 11px, weight 500, letter-spacing 0.22em, uppercase
- Color: `--apple-blue-hi` (`#2997ff`)
- Dot: inline-block `5px × 5px`, `border-radius: 50%`, same blue, `box-shadow: 0 0 8px rgba(41,151,255,0.7)`

**Section Eyebrow (h2-eyebrow)**
- Same font spec as hero eyebrow but color `--apple-blue` (`#0071e3`)
- Container: `display: inline-block; padding: 5px 11px 5px 9px; border-radius: 980px`
- Border: `1px solid rgba(0,113,227,0.25)`
- Background: `rgba(0,113,227,0.06)`

### Phases (Ordered Timeline)

- Container: `position: relative` — holds the vertical timeline line
- Timeline line: `position: absolute; left: 38px; top/bottom: 36px; width: 1px` — gradient stops from transparent through blue through border
- Phase row: CSS grid `80px 1fr`, gap `32px`, padding `26px 0`, border-bottom subtle
- Phase number: Berkeley Mono 12px, letter-spacing 0.1em, color `--apple-blue-hi` + `::after` pseudo-element circle marker
- Phase marker: `9px × 9px` circle, background `--bg`, border `1px solid --apple-blue`, `box-shadow: 0 0 0 4px var(--bg), 0 0 14px rgba(0,113,227,0.55)`
- Phase title: 22px, weight 590, letter-spacing -0.022em
- Pin tag inside title: Berkeley Mono 10px, letter-spacing 0.12em, uppercase, `rgba(255,255,255,0.04)` bg, border, 980px radius

### Terminal

- Container background: `#050608`
- Border: `1px solid rgba(255,255,255,0.08)`
- Border radius: `14px`
- Shadow: `rgba(0,0,0,0.4) 0 24px 60px -20px` outer + `inset 0 0 0 1px rgba(255,255,255,0.02)` inner
- **Terminal bar**: `rgba(255,255,255,0.025)` bg, padding `11px 14px`, border-bottom subtle
  - Traffic lights: `11px × 11px`, `border-radius: 50%`, default `--surface-2`, active dot `rgba(0,113,227,0.55)`
  - Label: Berkeley Mono 11px, color `--text-4`, `margin-left: auto`
- **Terminal body**: padding `24px`, Berkeley Mono 13px, line-height 1.7, color `--text-2`
- Terminal color tokens:
  - `term-prompt` (`--apple-blue-hi`): `▸` prompt character
  - `term-cmd` (`--text`): typed command
  - `term-out` (`--text-2`): standard output
  - `term-accent` (`--apple-blue-hi`): highlighted output terms
  - `term-dim` (`--text-4`): muted references
  - `term-muted` (`--text-3`): de-emphasized alternatives
  - `term-success` (`#27a644`): passed test counts
  - `term-comment` (`--text-4`, italic): `# comment` lines
- Cursor: `7px × 14px`, background `--apple-blue-hi`, `animation: blink 1s steps(2) infinite`

### Workflow (Audit Trail)

- Background: `rgba(255,255,255,0.015)`
- Border: `1px solid rgba(255,255,255,0.08)`
- Border radius: `16px`
- Padding: `28px`
- Font: Berkeley Mono 13px, color `--text-2`, line-height 1.8
- Step grid: `22px 130px 1fr 90px`, gap `16px`, padding `9px 4px`
- Step separator: `border-bottom: 1px dashed rgba(255,255,255,0.04)`
- Thinking step: `.tool` and `.what` both `--text-3`, `.what` italic
- Checkpoint step: background `rgba(0,113,227,0.05)`, border-radius `8px`, negative margin, `.tool` `--apple-blue`, `.what` `--text`

### Install Snippet

- Display: `inline-flex`, `align-items: center`, `gap: 10px`
- Background: `rgba(255,255,255,0.025)`
- Border: `1px solid rgba(255,255,255,0.08)`
- Border radius: `8px`
- Padding: `9px 14px`
- Font: Berkeley Mono 13px, color `--text-2`
- `.prompt`: color `--apple-blue-hi`, `user-select: none`
- `.copy`: color `--text-4`, 11px, uppercase, letter-spacing 0.05em, border-left `1px solid --border`

### Footer

- Border top: `1px solid rgba(255,255,255,0.05)`
- Padding: `40px 24px`
- Inner: max-width 1100px, flex space-between, wrap, gap `18px`
- Links: 13px, weight 510, color `--text-3`, hover `--text`
- Copyright: Berkeley Mono 11px, color `--text-4`, letter-spacing 0.05em

---

## 5. Layout Principles

### Spacing System
- Base unit: `8px`
- Primary rhythm: `8px`, `14px`, `18px`, `20px`, `24px`, `28px`, `32px`, `36px`, `40px`
- Section padding: `130px 24px` — generous vertical breathing room
- Hero padding: `88px 24px 64px`
- CTA section: `140px 24px 160px`
- Card internal: `28px`
- Nav inner: `14px 24px`
- Section header bottom margin: `64px`

### Grid & Container

- **Global max-width**: `1200px` (nav), `1100px` (sections, footer), `760px` (hero), `720px` (CTA, h2 text)
- **Feature card grid**: `repeat(auto-fit, minmax(280px, 1fr))`, gap `20px`
- **Explain split**: `1fr 1.05fr`, gap `72px`, `align-items: center`
- **Phase list**: single column with fixed `80px` number column, `1fr` content
- **Workflow**: 4-column micro grid within the mono container
- **All containers are centered**: `margin: 0 auto`

### Whitespace Philosophy
- **Darkness is space**: The `#08090a` canvas functions as negative space. Empty dark area between sections requires no decorative separators — the background itself provides the boundary.
- **Generous section breathing**: 130px vertical padding between sections creates a page that reads in discrete chapters rather than continuous scroll.
- **Fixed atmospheric layer**: The `position: fixed` radial gradients create persistent spatial depth without consuming layout area.
- **Section borders over dividers**: `border-top: 1px solid rgba(255,255,255,0.05)` on `.bordered` sections provides visual orientation without decorative weight.

### Border Radius Scale

| Class | Radius | Application |
|-------|--------|-------------|
| Micro | `2px` | — (not used in current surface) |
| Tight | `8px` | Install snippet, card icon |
| Standard | `9px` | Card icon only |
| Card | `14px` | Feature cards, terminal container |
| Panel | `16px` | Workflow container |
| Pill | `980px` | All buttons, section eyebrow |
| Full | `50%` | Terminal traffic-light dots, eyebrow dot |

---

## 6. Depth & Elevation

| Level | Treatment | Use |
|-------|-----------|-----|
| Atmospheric (−1) | Fixed radial blue gradients beneath all content | Page-wide ambient glow; not a surface |
| Level 0 | `#08090a` flat, no border | Page canvas, hero background |
| Level 1 | `#0f1011` or nav `rgba(8,9,10,0.72)` + blur | Panel surfaces; frosted nav bar |
| Level 2 | `rgba(255,255,255,0.02)` bg + `1px solid rgba(255,255,255,0.08)` | Cards, workflow, install snippet |
| Level 2b | `rgba(255,255,255,0.015)` bg + border | Workflow (slightly more recessed than cards) |
| Level 3 | `#050608` + `inset 0 0 0 1px rgba(255,255,255,0.02)` + outer shadow | Terminal (deepest foreground surface) |
| Focus / Hover | Border opacity increase `0.08 → 0.12` | Card hover, button hover state |
| Terminal Shadow | `rgba(0,0,0,0.4) 0 24px 60px -20px` | Terminal — only hard drop-shadow in the system |
| Interactive | `transform: scale(0.98)` | Button press — physical feedback |

**Shadow Philosophy**: Super inherits Linear's approach — on dark surfaces, traditional dark shadows disappear. Elevation is communicated through background luminance stepping (each level raises white opacity) and border opacity (subtle → standard → strong). The sole exception is the terminal, which uses a dramatic `rgba(0,0,0,0.4) 0 24px 60px -20px` shadow to anchor it as the most prominent UI surface on the page.

**Atmospheric Depth**: A pair of `position: fixed` radial gradients sit beneath all page content at `z-index: 0`. These gradients never scroll and are always translucent — they function as a persistent light-source simulation rather than a rendered surface.

---

## 7. Do's and Don'ts

### Do
- Use `#08090a` as the native canvas — this is not a dark theme, it is the default medium.
- Apply Berkeley Mono to every label, eyebrow, tag, code element, and the logo wordmark.
- Set `font-feature-settings: 'cv01', 'ss03'` globally on all Inter text — these features are identity-defining.
- Use font weight 510 as the default emphasis weight for all UI text and navigation.
- Apply negative letter-spacing to all display text: -0.04em on h1 and h2.
- Reserve Apple Action Blue (`#0071e3` / `#2997ff`) exclusively for interactive and brand semantic roles.
- Use `border-radius: 980px` on all buttons — pill geometry is the signature CTA shape.
- Express elevation through background luminance steps and border opacity, not shadow stacks.
- Include the `◆` glyph in Apple Action Blue wherever the Super logo appears; always pair with the "super" mono wordmark.
- Use uppercase only in Berkeley Mono contexts (eyebrows, phase numbers, copyright, status tags) — never on Inter prose text.

### Don't
- Don't use `#ffffff` as primary text — `#f7f8f8` prevents visual harshness on dark backgrounds.
- Don't introduce a second chromatic accent; Apple blue is the only non-achromatic color in the system.
- Don't apply the blue family decoratively — every blue element should be interactive, semantic, or the brand mark.
- Don't use solid-color card or button backgrounds — all surfaces are semi-transparent white overlays on the dark canvas.
- Don't round all corners to a single radius; different component classes use different steps (pill for CTAs, 14px for cards, 8px for snippets).
- Don't use weight 700 for Inter prose — Berkeley Mono uses 700 for the logo wordmark only; Inter maximum is 590 in the current surface.
- Don't use warm colors anywhere in the UI chrome — the palette is cool dark with blue-violet accent only.
- Don't skip `font-feature-settings` — without `'cv01', 'ss03'`, Inter reads as generic system text, not Super's typeface.
- Don't add decorative separators between sections — `border-top: 1px solid rgba(255,255,255,0.05)` and vertical padding are sufficient structure.
- Don't add shadows except on the terminal container — all other elevation is communicated through surface opacity stepping.

---

## 8. Responsive Behavior

### Breakpoints

| Name | Width | Key Changes |
|------|-------|-------------|
| Mobile Small | ≤ 720px | Nav text links hidden, gap reduces to 12px; hero padding 40px 18px; section padding 96px 18px |
| Mobile | 640px | Phase grid collapses to `56px 1fr`, gap 16px; phase title 18px; phase body 15px |
| Tablet | 920px | Explain split collapses to single column, gap 48px |
| Desktop | 720px+ | Full layout, horizontal nav |

### Touch Targets
- All pill buttons use at minimum `8px 18px` padding — sufficient horizontal target area; `11px` vertical on large variant.
- Nav links at 13px have 28px gaps but no explicit hit-area expansion — designed for desktop-first interaction.
- Phase rows use `26px 0` padding providing comfortable touch heights on vertical scroll.

### Collapsing Strategy
- **Hero heading**: `clamp(38px, 6vw, 64px)` — fluid, no breakpoint snap.
- **Section heading**: `clamp(34px, 4.8vw, 52px)` — same fluid approach.
- **Navigation**: text links `display: none` below 720px; only logo + CTA button visible.
- **Phase list**: number column narrows from `80px` to `56px` at 640px; gap tightens.
- **Explain grid**: single-column stacked below 920px.
- **Workflow grid**: collapses to 2 columns at 720px; `.meta` column hidden.
- **Terminal body**: padding reduces from `24px` to `18px`; font-size `13px → 12px` at 720px.
- **Diamond stage**: height reduces from `100vh` to `360px` on mobile; margin-bottom `16px`.
- **Section padding**: 130px → 96px at 720px; CTA section 140px/160px → 104px/120px.

### Image & Animation Behavior
- The Three.js diamond and ASCII canvas resize to fill the stage at any viewport with `position: sticky` scroll behavior preserved.
- The ASCII canvas scales cell count (`CELL_W: 8px`, `CELL_H: 13px`) relative to the actual viewport, maintaining consistent character density.
- The scroll-driven dissolve is viewport-relative; mobile reduces the stage height but retains the full dissolve range.

---

## 9. Agent Prompt Guide

### Quick Color Reference
- Page canvas: `#08090a`
- Deepest black: `#010102`
- Panel: `#0f1011`
- Surface elevated: `#191a1b`
- Surface secondary: `#28282c`
- Primary text: `#f7f8f8`
- Secondary text: `#d0d6e0`
- Muted text: `#8a8f98`
- Subtle text: `#62666d`
- Primary action blue: `#0071e3`
- Interactive blue (dark surfaces): `#2997ff`
- Button hover blue: `#1d83eb`
- Link blue: `#0066cc`
- Success green: `#27a644`
- Border subtle: `rgba(255,255,255,0.05)`
- Border standard: `rgba(255,255,255,0.08)`
- Border strong: `rgba(255,255,255,0.12)`
- Blue border tint: `rgba(0,113,227,0.25)`

### Example Component Prompts

- "Create a Super-style hero section on `#08090a`. Eyebrow: Berkeley Mono 11px weight 500 letter-spacing 0.22em uppercase color `#2997ff` with a glowing dot. H1 at `clamp(38px, 6vw, 64px)` Inter Variable weight 510 line-height 1.02 letter-spacing -0.04em color `#f7f8f8`. Sub-phrase in accent color `#8a8f98`. Lede paragraph 19px weight 400 line-height 1.55 color `#8a8f98`. Two pill CTAs: primary `#0071e3` + ghost `rgba(255,255,255,0.03)`, both `border-radius: 980px`, `8px 18px` padding, 13px Inter Variable weight 510."

- "Design a Super-style feature card. Background `rgba(255,255,255,0.02)`, border `1px solid rgba(255,255,255,0.08)`, 14px radius, 28px padding. Card icon: 34px × 34px, 9px radius, `rgba(0,113,227,0.10)` bg, `1px solid rgba(0,113,227,0.25)` border, color `#2997ff`, Berkeley Mono 16px weight 700. Card title 19px Inter Variable weight 590 letter-spacing -0.018em color `#f7f8f8`. Card body 15px weight 400 color `#8a8f98` line-height 1.6."

- "Build a Super terminal block. Outer: `#050608` bg, `1px solid rgba(255,255,255,0.08)` border, 14px radius, shadow `rgba(0,0,0,0.4) 0 24px 60px -20px`. Terminal bar: `rgba(255,255,255,0.025)` bg, three `11px` circle dots (active one `rgba(0,113,227,0.55)`), Berkeley Mono 11px label color `#62666d`. Body: 24px padding, Berkeley Mono 13px line-height 1.7. Prompt `▸` in `#2997ff`, command in `#f7f8f8`, output in `#d0d6e0`, accents in `#2997ff`, dimmed paths in `#62666d`, success counts in `#27a644`."

- "Compose a Super-style section eyebrow. Berkeley Mono 11px weight 500 letter-spacing 0.22em uppercase color `#0071e3`, pill container: `padding: 5px 11px 5px 9px`, `border-radius: 980px`, `border: 1px solid rgba(0,113,227,0.25)`, `background: rgba(0,113,227,0.06)`. Section heading below: `clamp(34px, 4.8vw, 52px)` Inter Variable weight 510 letter-spacing -0.04em line-height 1.05 color `#f7f8f8`. Section lede: 18px weight 400 line-height 1.6 letter-spacing -0.01em color `#8a8f98`."

- "Create a Super navigation bar. `position: sticky`, `background: rgba(8,9,10,0.72)`, `backdrop-filter: blur(14px) saturate(140%)`, `border-bottom: 1px solid rgba(255,255,255,0.05)`. Inner: max-width 1200px, `padding: 14px 24px`, flex space-between. Logo: Berkeley Mono 16px weight 700, `◆` glyph at 17px color `#0071e3` with `drop-shadow(0 0 6px rgba(0,113,227,0.45))`. Nav links: Inter Variable 13px weight 510 color `#d0d6e0` hover `#f7f8f8`. CTA: primary pill `#0071e3` 13px `8px 18px` pad `border-radius: 980px`."

### Iteration Guide

1. **Set the atmospheric layer first**: add the two fixed radial gradients (`rgba(0,113,227,0.10)` from top, `rgba(41,151,255,0.05)` from lower-right) before placing any content — they define the spatial depth of the entire page.
2. **Apply font-feature-settings globally**: `"cv01", "ss03"` on the root and every code block with Berkeley Mono — this is non-negotiable for the design identity.
3. **Build surfaces from black up**: start `#08090a`, layer `#0f1011` for panels, `rgba(255,255,255,0.02)` for cards — never use opaque non-black surface fills.
4. **Use borders as the depth signal**: `rgba(255,255,255,0.05)` → `0.08` → `0.12` is the entire depth vocabulary above Level 1. Add or increase border opacity before reaching for shadows.
5. **Reserve blue for action and identity only**: if a blue element is not interactive, a brand mark, or a semantic signal (success/phase/eyebrow), it should not be blue.
6. **Pill geometry exclusively for all buttons**: `border-radius: 980px` — no exceptions in the current surface. Rectangular or lightly-rounded buttons are not the Super shape language.
7. **Berkeley Mono signals "system", Inter signals "human"**: every technical label, prefix, counter, or code span belongs to Berkeley Mono; all prose, headings, and descriptions belong to Inter.
8. **Font weight ceiling is 590**: Inter caps at 590 for the strongest emphasis; Berkeley Mono uses 700 only for the "super" wordmark. Weight 700 elsewhere is out of system.

### Known Gaps / Intentional Omissions
- No semantic palette beyond `#27a644` (success green) — error/warning states are not defined in the current marketing surface.
- No light mode — the system is dark-native only and contains no light-theme tokens.
- No focus ring specification beyond the interactive hover/border states — keyboard accessibility tokens are not explicitly defined.
- Interaction states on cards use opacity transitions but no focus-visible ring — this should be added before publishing to production.
- The scroll-driven 3D diamond is a bespoke Three.js + GLSL implementation; no CSS-only fallback is provided.
