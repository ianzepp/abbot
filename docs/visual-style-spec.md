# Visual Style Specification
## "Field Survey Terminal" — Retro-Scientific Research Interface

FIELD SURVEY TERMINAL — ASCII LAYOUT DIAGRAM
═══════════════════════════════════════════════

Full viewport (~1366 x 850)


┌─────────────────────────────────────────────────────────────────────────────────────────────────────┐
│▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ NAV BAR ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│
│▓ ☰ TAXA_INDEX  T_REX  [TRICERATOPS]  VELO  SPINO ...  ▓  ⛋ SECTORS  HELL_CRK  MORRISON  FILING ▓│
│▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│
│                                                                                                     │
│  STAT STRIP (3 columns above map)                        │  DETAIL PANEL (right ~35%)              │
│                                                          │                                         │
│  Col A               Col B              Col C            │  47.20    DINO-DRAFT SURVEY     -105.20 │
│ ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐ │                RECORD                   │
│ │GLOBAL_TAXA_  │  │SECTOR_BIO_   │  │SURVEY_LOG_STATUS │ │ ┌─────────────────────────────────────┐ │
│ │DENSITY [V-1] │  │SIGNATURES    │  │                  │ │ │ BORROWER'S NAME / SPECIES ID        │ │
│ │              │  │              │  │ ┌──────────────┐ │ │ │                                     │ │
│ │  7        ⚙⚙ │  │ EPOCH: Meso  │  │ │● SAT LINK  │ │ │ │   𝒯𝓇𝒾𝒸𝑒𝓇𝒶𝓉𝑜𝓅𝓈                    │ │
│ │  CONFIRMED   │  │ PERIOD: L.Cr │  │ │─────────── │ │ │ │                                     │ │
│ │  SITES       │  │ CONF: 0.982  │  │ │◻ RECORDS   │ │ │ │                   Project Field Lead │ │
│ │  ━━━━━━━━━━  │  │              │  │ │─────────── │ │ │ └─────────────────────────────────────┘ │
│ │              │  │              │  │ │◎ SCAN AREA │ │ │                                         │
│ │← ACTIVE DIST │  │              │  │ └──────────────┘ │ │  OVERVIEW   INVENTORY   METRICS       │
│ │  SCAN DONE   │  │              │  │    DRAFT_RELEASE │ │  ━━━━━━━━                              │
│ └──────────────┘  └──────────────┘  └──────────────────┘ │  DISCOVERY DESC / SUMMARY    ERA/YEAR  │
│ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ │ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─│
│                                                          │ ┌─────────────────────────────────────┐ │
│  MAP VIEWPORT (fills remaining left zone)                │ │                                     │ │
│ ┌──────────────────────────────────────────────────────┐ │ │                                     │ │
│ │                                                      │ │ │                                     │ │
│ │  ┌──────────────────────┐                            │ │ │         [ILLUSTRATION]              │ │
│ │  │ LOCAL_SURVEY_VIEWPORT│                            │ │ │                                     │ │
│ │  │ SAVAGE BONE BED,     │                            │ │ │        Etching/engraving            │ │
│ │  │ CANNONBALL FORMATION │                            │ │ │         style artwork               │ │
│ │  │ NORTH DAKOTA, USA    │                            │ │ │                                     │ │
│ │  │ LAT: 46.3  LNG:-103 │                            │ │ │                                     │ │
│ │  └──────────────────────┘                            │ │ └─────────────────────────────────────┘ │
│ │                                                      │ │                                         │
│ │                         ● ╌╌╌                        │ │  TRICERATOPS HORRIDUS                   │
│ │                       (marker)                       │ │  ━━━━━━━━━━━━━━━━━━━━━                   │
│ │                                                      │ │                                         │
│ │(◁)                                              (▷)  │ │  Triceratops is one of the last         │
│ │                                                      │ │  non-avian dinosaurs to evolve,         │
│ │                                                      │ │  known for its massive bony frill       │
│ │                                                      │ │  and three distinct facial horns.       │
│ │┌───┐                                                 │ │  It was a powerful herbivore that       │
│ ││ + │                                                 │ │  roamed late Cretaceous North           │
│ │├───┤                                                 │ │  America.                               │
│ ││ − │                                                 │ │                                         │
│ │└───┘          ┌──────────────┐   COORDINATE_SYSTEM:  │ │                                         │
│ │               │ SITE 3 OF 7  │   GEO_DECIMAL         │ │                                         │
│ │               └──────────────┘   FOCUS_MODE:         │ │                                         │
│ │                                  SINGLE_UNIT         │ │                                         │
│ └──────────────────────────────────────────────────────┘ │                                         │
│                                                          │                                         │
├─────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ ● ENGINE: GEMINI_FLASH_LITE    SYSTEM_STATE: OPERATIONAL         RECORDED_AT:...   BUILD_R-990-V  │
└─────────────────────────────────────────────────────────────────────────────────────────────────────┘
│◄──────────────────── ~65% ────────────────────►│◄────────── ~35% ──────────►│



ZONE HIERARCHY (simplified)
════════════════════════════

┌─────────────────────────────────────────────────────────────┐
│ NAV_BAR  [full width, dark bg, ~44px]                       │
├────────────────────────────────┬────────────────────────────┤
│ LEFT_ZONE  ~65%               │ RIGHT_ZONE  ~35%           │
│                                │                            │
│ ┌────────┬────────┬──────────┐ │ ┌────────────────────────┐ │
│ │STAT_A  │STAT_B  │STAT_C    │ │ │ RECORD_HEADER         │ │
│ └────────┴────────┴──────────┘ │ ├────────────────────────┤ │
│ ┌────────────────────────────┐ │ │ SPECIMEN_CARD (nested) │ │
│ │                            │ │ ├────────────────────────┤ │
│ │  MAP_VIEWPORT              │ │ │ TAB_ROW                │ │
│ │                            │ │ ├────────────────────────┤ │
│ │  ┌──────────┐              │ │ │ TABLE_HEADER           │ │
│ │  │LOCATION  │    ●         │ │ ├────────────────────────┤ │
│ │  │OVERLAY   │  marker      │ │ │ ILLUSTRATION_PANEL     │ │
│ │  └──────────┘              │ │ ├────────────────────────┤ │
│ │                            │ │ │ TITLE (underlined)     │ │
│ │(◁)                    (▷)  │ │ ├────────────────────────┤ │
│ │                            │ │ │ BODY_TEXT              │ │
│ │[+]    ┌──────────┐  meta   │ │ │                        │ │
│ │[−]    │PAGINATION│         │ │ │                        │ │
│ │       └──────────┘         │ │ └────────────────────────┘ │
│ └────────────────────────────┘ │                            │
├────────────────────────────────┴────────────────────────────┤
│ STATUS_BAR  [full width, light bg, ~32px]                   │
└─────────────────────────────────────────────────────────────┘



MAP OVERLAY ELEMENTS (z-index layers)
══════════════════════════════════════

  z-0  MAP_BACKGROUND (tan fill, landmass shapes)
  z-1  MAP_MARKER (solid dot + dashed annotation line)
  z-2  LOCATION_OVERLAY_CARD (top-left, opaque bg)
  z-2  PAGINATION_BADGE (bottom-center, opaque bg)
  z-2  COORDINATE_META (bottom-right, text only)
  z-2  ZOOM_CONTROLS (bottom-left, stacked buttons)
  z-2  NAV_ARROWS (mid-left + mid-right, circular)

---

## 1. Design Philosophy

**Core Aesthetic:** Vintage scientific field-research terminal — evoking 1970s–1980s government survey equipment, typewriter-produced field reports, and analog data instrumentation. The interface feels like a declassified research station UI rendered for the modern web.

**Key Tensions That Define the Style:**
- Analog warmth vs. digital precision
- Institutional formality vs. hand-drawn humanity
- Dense information vs. generous spatial breathing room
- Monochrome restraint vs. selective parchment warmth

---

## 2. Color Palette

### 2.1 Foundations

| Token | Hex | Role |
|---|---|---|
| `--bg-primary` | `#F5F0E8` | Main background — warm parchment/aged paper |
| `--bg-secondary` | `#EDE8DD` | Card/panel backgrounds — slightly darker parchment |
| `--bg-map` | `#E8E0D2` | Map/canvas fill — muted khaki-tan |
| `--bg-map-feature` | `#DDD4C4` | Map landmass/feature fill — warm tan |
| `--bg-nav` | `#2C2824` | Top navigation bar — near-black warm brown |
| `--bg-status-bar` | `#F0EBE2` | Bottom status bar — light parchment with subtle border |

### 2.2 Text & Foreground

| Token | Hex | Role |
|---|---|---|
| `--text-primary` | `#2C2824` | Primary body text — warm near-black |
| `--text-secondary` | `#8A8178` | Labels, metadata, secondary info — warm medium gray |
| `--text-tertiary` | `#B5AEA4` | Watermark text, ghost labels — faded parchment tone |
| `--text-nav` | `#D4CFC6` | Inactive nav items — muted warm light gray |
| `--text-nav-active` | `#F5F0E8` | Active nav item — matches parchment background |

### 2.3 Accents & Indicators

| Token | Hex | Role |
|---|---|---|
| `--accent-progress` | `#2C6E49` | Progress bars, status "active" dot — muted forest green |
| `--accent-marker` | `#2C2824` | Map pin/marker — same as primary text |
| `--border-default` | `#D4CFC6` | Card borders, dividers — warm light gray |
| `--border-subtle` | `#E5E0D8` | Inner dividers, table row separators |

### 2.4 Usage Rules

- **No saturated colors.** Every hue is desaturated and warm-shifted. Even the green accent (`#2C6E49`) sits low on the saturation scale.
- **No pure black or pure white.** Darkest value is `#2C2824` (warm charcoal). Lightest is `#F5F0E8` (warm cream).
- **Backgrounds layer subtly.** The difference between `--bg-primary` and `--bg-secondary` is only ~5–8 luminance points — panels emerge through whisper-level contrast, not hard boxes.

---

## 3. Typography

### 3.1 Type Scale & Families

| Role | Family | Weight | Size | Transform | Tracking |
|---|---|---|---|---|---|
| **Labels / Field Names** | Monospace (e.g. `IBM Plex Mono`, `JetBrains Mono`, `Courier Prime`) | 400–500 | 11–12px | `uppercase` | `0.08em–0.12em` |
| **Data Values** | Same monospace | 500–600 | 13–14px | `uppercase` | `0.04em` |
| **Hero Metric** | Same monospace or condensed sans | 700 | 48–56px | None | `-0.02em` |
| **Handwritten / Specimen ID** | Script/calligraphic (e.g. `Caveat`, `Dancing Script`, `Dawning of a New Day`) | 400 | 36–48px | None | `0` |
| **Body / Descriptions** | Serif (e.g. `Courier Prime`, `IBM Plex Serif`, or monospace) | 400 | 14–15px | None | `0.01em` |
| **Section Headings** | Monospace | 600 | 13–14px | `uppercase` | `0.06em–0.10em` |
| **Tab Labels** | Monospace | 600 (active) / 400 (inactive) | 13–14px | `uppercase` | `0.08em` |
| **Status Bar** | Monospace | 400 | 11px | `uppercase` | `0.08em` |

### 3.2 Typography Rules

- **Underscores replace spaces in labels.** Field names read like variable names: `GLOBAL_TAXA_DENSITY`, `SECTOR_BIO_SIGNATURES`, `LOCAL_SURVEY_VIEWPORT`. This is a defining characteristic of the style.
- **All-caps with wide tracking on labels.** Every metadata label, section header, and category name uses `text-transform: uppercase` with generous letter-spacing.
- **Key/value pairs use colon-space alignment.** Label sits left, value sits right, connected by a colon. The monospace font makes these self-aligning.
- **The handwritten script font is used sparingly** — only for the primary specimen/entity name in the detail card, creating a strong contrast between the institutional labels and the "field researcher's hand."
- **Underline as emphasis.** The active nav item and some headings use underline decoration rather than bold or color change. The underline weight matches the border weight (~1–2px).

---

## 4. Spatial System & Layout

### 4.1 Grid Structure

The layout uses a **two-zone asymmetric split:**

| Zone | Width | Content |
|---|---|---|
| **Left / Main** | ~65% | Map viewport, summary stat cards, location info |
| **Right / Detail Panel** | ~35% | Record detail card, tabs, description, illustration |

The top row above the map uses a **three-column stat strip** spanning the full left zone, each column holding a distinct data module.

### 4.2 Spacing Scale

| Token | Value | Usage |
|---|---|---|
| `--space-xs` | `4px` | Inline spacing, icon-to-label gap |
| `--space-sm` | `8px` | Internal card padding tight, between list rows |
| `--space-md` | `16px` | Standard card padding, gap between cards |
| `--space-lg` | `24px` | Section separation, panel outer padding |
| `--space-xl` | `32px` | Zone margins, large section breaks |
| `--space-2xl` | `48px` | Top-level layout gutters |

### 4.3 Layout Rules

- **Cards do not have rounded corners.** All containers use `border-radius: 0`. The aesthetic is squared-off and utilitarian.
- **Borders are single-pixel, warm gray.** Cards are delineated by `1px solid var(--border-default)`, not box-shadow. No drop shadows anywhere.
- **The map is the visual anchor.** It occupies the largest single area and bleeds to the edges of its zone — no border, no padding against the left/bottom edges.
- **Overlay cards float on the map** with opaque parchment backgrounds (e.g., the location viewport card at top-left of map, and the pagination indicator at bottom-center).
- **Generous vertical whitespace inside cards.** Content within cards breathes — there's more space between sections than you'd expect from a "dense data" interface.

---

## 5. Component Patterns

### 5.1 Navigation Bar

```
┌──────────────────────────────────────────────────────────┐
│ ☰ TAXA_INDEX   ITEM_A   [ITEM_B]   ITEM_C   ITEM_D ... │
└──────────────────────────────────────────────────────────┘
```

- **Background:** `--bg-nav` (warm near-black, `#2C2824`)
- **Height:** ~44–48px
- **Items:** Monospace, uppercase, `0.08em` tracking, `--text-nav` color
- **Active item:** `--text-nav-active` with a bottom-aligned **underline** or **box/outline treatment** — the active item appears visually "selected" with a subtle bordered rectangle
- **Left section vs. right section:** The nav is divided — left group holds primary entity tabs, right group (separated by space) holds category/context tabs (e.g., "SECTORS", "FORMATIONS"). A small icon prefix distinguishes the right group
- **Icons:** Small, monoline, ~16px, matching text color. Placed before the first item in each group

### 5.2 Stat Card (Top Row Module)

```
┌────────────────────────────┐
│ FIELD_LABEL_NAME [V-1]     │  ← monospace uppercase label, secondary color
│                            │
│ 7                          │  ← hero metric, 48-56px, primary color
│ CONFIRMED SITES            │  ← monospace descriptor below metric
│ ━━━━━━━━━━━━━━━            │  ← accent-colored progress bar
│                            │
│ ← STATUS MESSAGE COMPLETE  │  ← small status line, tertiary
└────────────────────────────┘
```

- No visible border on the card itself — separated by vertical dividers between columns or implied by spacing
- The progress/status bar is a thin horizontal line in `--accent-progress` green
- Version tags `[V-1]` rendered as inline monospace at reduced size

### 5.3 Key/Value Data List

```
EPOCH ID:           Mesozoic
PERIOD:             Late Cretaceous
CONFIDENCE:         0.982 ALPHA
```

- Labels left-aligned, values right-aligned (or right-padded)
- Monospace throughout, allowing character-level alignment
- Label in secondary color, value in primary color
- Rows separated by `--space-sm` (8px) vertical gap
- No horizontal rules between rows — whitespace only

### 5.4 Status List (with icons)

```
┌──────────────────────────────────┐
│  ● SATELLITE LINK STABLE         │  ← green dot indicator
│  ◻ RECORDS ARCHIVED: 1289        │  ← icon + colon-separated value
│  ◎ SCAN AREA: GLOBAL MERCATOR    │
│──────────────────────────────────│  ← subtle inner dividers between rows
└──────────────────────────────────┘
```

- Light border container, `1px solid var(--border-default)`
- Each row separated by `var(--border-subtle)` horizontal rule
- Status dot: small filled circle in `--accent-progress` for "active"
- Icons: monoline, 14–16px, secondary color

### 5.5 Detail Record Card

```
┌─────────────────────────────────────────┐
│  47.20       DINO-DRAFT SURVEY RECORD   │  ← coordinates + record title
│                                 -105.20 │
│ ┌─────────────────────────────────────┐ │
│ │ BORROWER'S NAME / SPECIES ID       │ │  ← sub-label
│ │                                     │ │
│ │     𝒯𝓇𝒾𝒸𝑒𝓇𝒶𝓉𝑜𝓅𝓈                   │ │  ← handwritten script font
│ │                    Project Field Lead│ │  ← right-aligned role/attribution
│ └─────────────────────────────────────┘ │
│                                         │
│  OVERVIEW    INVENTORY    METRICS       │  ← tab row
│  ━━━━━━━━                               │  ← underline on active tab
│  DISCOVERY DESCRIPTION / SUMMARY   ERA  │  ← table header
│ ┌─────────────────────────────────────┐ │
│ │         [ILLUSTRATION]              │ │  ← hand-drawn/etched style image
│ └─────────────────────────────────────┘ │
│  SPECIMEN NAME                          │  ← bold monospace, underlined
│                                         │
│  Body description text in serif or      │  ← body copy, 14-15px
│  monospace. Period-appropriate tone.    │
└─────────────────────────────────────────┘
```

- Outer border: `1px solid var(--border-default)`
- Inner specimen card: nested border, slightly different background (`--bg-secondary`)
- The handwritten name is the single most visually distinctive element — it should feel like someone signed a field card
- Tab row uses underline indicator, not background highlight
- Illustration container has a subtle background tint and holds a line-art/etching-style image

### 5.6 Map Viewport

- **Background:** `--bg-map` — muted khaki-tan
- **Features:** Filled shapes in `--bg-map-feature` — no outlines on landmasses, just solid fill against background
- **Markers:** Solid circles, `--accent-marker`, ~12px diameter, no shadow
- **Dashed lines:** Thin dashed lines radiating from markers for annotation
- **Navigation arrows:** Left/right chevrons in circular outlines, `1px` border, positioned at vertical midpoint of map edges
- **Zoom controls:** `+` / `−` buttons stacked vertically, bottom-left, square, `1px` border, `--bg-primary` fill
- **Pagination badge:** Centered bottom, monospace text, e.g. `SITE 3 OF 7`, light background, `1px` border
- **Coordinate metadata:** Bottom-right, small monospace, `--text-tertiary`

### 5.7 Floating Map Overlay Card

```
┌──────────────────────────────────────┐
│ LOCAL_SURVEY_VIEWPORT                │  ← monospace label, secondary
│ SAVAGE BONE BED, CANNONBALL          │  ← bold monospace location name
│ FORMATION, NORTH DAKOTA, USA         │
│ LAT: 46.3071   LNG: -103.9375       │  ← coordinate pair
└──────────────────────────────────────┘
```

- Opaque `--bg-primary` background
- Light border or no border (the opacity contrast against the map suffices)
- Positioned absolutely over the map in the top-left region

### 5.8 Status Bar (Bottom)

```
┌──────────────────────────────────────────────────────────────────┐
│ ● ENGINE: GEMINI_FLASH_LITE    SYSTEM_STATE: OPERATIONAL    ... │
└──────────────────────────────────────────────────────────────────┘
```

- Full-width, ~32px height
- `--bg-status-bar` background, top border `1px solid var(--border-default)`
- All text: smallest type size (10–11px), monospace, uppercase, `--text-secondary`
- Status indicator dot: small filled circle, `--accent-progress` green
- Left-aligned: engine/system info. Right-aligned: build version/branch metadata

---

## 6. Iconography

| Attribute | Value |
|---|---|
| **Style** | Monoline / outlined stroke, not filled |
| **Stroke weight** | 1–1.5px |
| **Size** | 14–18px (inline with text) |
| **Color** | Matches adjacent text color — never a different hue |
| **Source recommendation** | Lucide, Phosphor (light weight), or custom SVG |
| **Usage** | Nav group prefixes, status list items, map controls, and action buttons only — icons are functional, not decorative |

---

## 7. Illustration Style

The detail card includes an illustration rendered in a **scientific etching / engraving style:**

- **Technique:** Fine crosshatching, stippling, and contour lines — emulating 19th-century natural history plate illustration
- **Color:** Monochrome — `--text-primary` on `--bg-secondary`
- **Border:** Contained within a bordered rectangle with a subtle background tint
- **Sizing:** Occupies the full width of the detail card content area, aspect ratio ~4:3 to ~16:9

This is a core atmospheric element. If using generated images, apply a halftone/engraving filter. If using SVG, use fine stroke work with no fills.

---

## 8. Interaction & Motion

| Pattern | Treatment |
|---|---|
| **Hover on nav items** | Subtle brightness shift — text transitions from `--text-nav` toward `--text-nav-active`. No background change. Duration: `150ms ease` |
| **Active tab underline** | 2px solid bar beneath text, same color as text. Could animate width from 0 on tab switch: `200ms ease-out` |
| **Map pan/zoom** | Smooth inertial motion. Controls use `:active` state with slight inset (1px translate or border color change) |
| **Card transitions** | If cards swap content (e.g., changing selected specimen), use a subtle `opacity` fade: `150–200ms` |
| **Progress bar** | Can animate width on load: `600ms ease-out` with slight delay |
| **General rule** | Motion is minimal and functional. No bouncing, no spring physics, no parallax. Everything feels like an instrument display updating. |

---

## 9. Borders & Dividers

| Pattern | Spec |
|---|---|
| **Card border** | `1px solid var(--border-default)` — `#D4CFC6` |
| **Inner divider** | `1px solid var(--border-subtle)` — `#E5E0D8` |
| **Nav bottom edge** | Implicit via background color contrast (dark nav against light content) |
| **Active underline** | `2px solid var(--text-primary)` — `#2C2824` |
| **Progress/accent bar** | `3–4px solid var(--accent-progress)` — `#2C6E49` |
| **Border radius** | `0` everywhere. No rounded corners. |
| **Box shadow** | None. Zero. The entire interface is flat and planar. |

---

## 10. Responsive Considerations

While the screenshot shows a wide desktop layout, adaptation rules for this style:

| Breakpoint | Behavior |
|---|---|
| **≥1280px** | Full two-zone layout as shown — map + detail panel side by side |
| **1024–1279px** | Detail panel narrows; illustration scales down; stat strip may collapse to 2 columns |
| **768–1023px** | Stack layout — map on top, detail panel below; stat strip becomes scrollable horizontal row |
| **<768px** | Single column; nav becomes hamburger menu preserving the underscore-naming convention; map gets full width with overlay cards repositioned |

---

## 11. CSS Custom Properties Summary

```css
:root {
  /* Backgrounds */
  --bg-primary:      #F5F0E8;
  --bg-secondary:    #EDE8DD;
  --bg-map:          #E8E0D2;
  --bg-map-feature:  #DDD4C4;
  --bg-nav:          #2C2824;
  --bg-status-bar:   #F0EBE2;

  /* Text */
  --text-primary:    #2C2824;
  --text-secondary:  #8A8178;
  --text-tertiary:   #B5AEA4;
  --text-nav:        #D4CFC6;
  --text-nav-active: #F5F0E8;

  /* Accents */
  --accent-progress: #2C6E49;
  --accent-marker:   #2C2824;

  /* Borders */
  --border-default:  #D4CFC6;
  --border-subtle:   #E5E0D8;

  /* Typography */
  --font-mono:       'IBM Plex Mono', 'Courier Prime', 'JetBrains Mono', monospace;
  --font-script:     'Caveat', 'Dancing Script', cursive;
  --font-body:       'IBM Plex Mono', 'Courier Prime', monospace;  /* or a serif */

  /* Spacing */
  --space-xs:  4px;
  --space-sm:  8px;
  --space-md:  16px;
  --space-lg:  24px;
  --space-xl:  32px;
  --space-2xl: 48px;

  /* Borders */
  --radius:    0px;
  --shadow:    none;
}
```

---

## 12. Mood Board Keywords

For AI image generation prompts, search queries, or communicating this style to collaborators:

> **vintage scientific survey terminal, parchment UI, field research station, analog instrument panel, 1970s government database, typewriter interface, natural history museum catalog, cartographic survey tool, warm monochrome dashboard, retro data visualization, etched illustration plate, institutional utilitarian design**

---

## 13. Do / Don't Quick Reference

| ✓ Do | ✗ Don't |
|---|---|
| Use monospace everywhere for labels | Use sans-serif display fonts |
| Replace spaces with underscores in labels | Use camelCase or Title Case for field names |
| Keep all colors warm and desaturated | Use any saturated blue, purple, or bright accent |
| Use 1px borders, no shadows | Use box-shadow, drop-shadow, or glow effects |
| Square corners on everything | Round any corners |
| Use uppercase with wide tracking for labels | Use sentence case for metadata labels |
| Include one script/handwritten element per view | Overuse the script font — it's for the primary entity name only |
| Use engraving/etching style illustrations | Use photography or modern flat illustration |
| Keep motion minimal and functional | Add playful or elastic animations |
| Design as if rendering on a CRT or thermal printer | Design as if it's a modern SaaS product |

---

## 14. Application to Abbot Kernel Monitor

This section maps the Field Survey Terminal aesthetic to Abbot's domain: a kernel frame monitor, chat interface, and administrative console.

### 14.1 Domain Translation

The survey metaphor translates directly to kernel monitoring:

| Survey Domain | Abbot Domain |
|---------------|--------------|
| Taxa / specimens | Frames (needs, tasks, tools, replies) |
| Survey sites | Scopes (main, session/xyz) |
| Map viewport | Frame timeline / trace view |
| Location marker | Selected frame |
| Specimen card | Frame inspector |
| Epoch / Period / Confidence | Op / Name / Actor / Scope |
| Illustration panel | Frame data payload (formatted JSON) |
| Site 3 of 7 | Frame 47 of 1,289 |
| Field notes | Chat messages |
| Equipment config | System configuration |
| Survey records | Audit logs |

The core insight: field research is observation and cataloging; kernel monitoring is observation and cataloging. Both involve tracking transient events, classifying specimens, and maintaining operational logs.

### 14.2 View Mapping

Abbot's TUI provides five views. Each maps to the visual spec:

| TUI View | Spec Role | Primary Zone |
|----------|-----------|--------------|
| Monitor | Map viewport + stat strip | Left (65%) |
| Chat | Field notes / scope conversation | Left or modal |
| Explorer | Archive browser | Left (replaces map) |
| Config | Equipment terminal | Left (replaces map) |
| Logs | Survey records query | Left (replaces map) |

The detail panel (right 35%) consistently shows inspection content — the selected frame, file, config section, or log entry.

### 14.3 Nav Bar Structure

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ☰ FRAME_INDEX   MAIN   [SESSION_ABC]   SESSION_XYZ  │  ⛋ MONITOR  CONFIG  LOGS │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Left group (entity tabs):** Scope tabs — each open scope gets a tab. The active scope is bracketed or underlined.

**Right group (context tabs):** View mode tabs — MONITOR, EXPLORER, CONFIG, LOGS. These control what appears in the left zone while maintaining the current scope context.

### 14.4 Stat Strip for Kernel Monitoring

Three columns above the frame timeline:

```
┌──────────────────┐  ┌──────────────────┐  ┌──────────────────────┐
│ FRAME_ACTIVITY   │  │ KERNEL_STATE     │  │ CONNECTION_STATUS    │
│ [LIVE]           │  │                  │  │                      │
│                  │  │ NEEDS:      12   │  │ ┌──────────────────┐ │
│  1,289           │  │ TASKS:      47   │  │ │● WEBSOCKET OPEN  │ │
│  FRAMES_RECV     │  │ TOOLS:       8   │  │ │──────────────────│ │
│  ━━━━━━━━━━━━━━  │  │ REPLIES:   156   │  │ │◻ FRAMES: 1,289   │ │
│                  │  │                  │  │ │──────────────────│ │
│← RATE: 23/SEC    │  │                  │  │ │◎ SCOPE: MAIN     │ │
└──────────────────┘  └──────────────────┘  └──────────────────────┘
```

**Column A (Hero Metric):** Total frame count with rate indicator and progress bar showing buffer fill.

**Column B (Key/Value):** Breakdown by frame type — needs, tasks, tools, replies.

**Column C (Status List):** Connection state, frame count, active scope.

### 14.5 Frame Timeline as Survey Log

The map viewport becomes a scrolling frame log:

```
┌────────────────────────────────────────────────────────────────────────────┐
│  ┌─────────────────────────────┐                                           │
│  │ ACTIVE_FRAME_CONTEXT        │                                           │
│  │ SCOPE: MAIN                 │                                           │
│  │ THREAD: abc-123-def         │                                           │
│  │ FRAMES: 47 IN THREAD        │                                           │
│  └─────────────────────────────┘                                           │
│                                                                            │
│  14:32:07.123    N    NEED:ENQUEUE        ok       HEAD/MAIN              │
│  14:32:07.456    T    TASK:ENQUEUE        ok       HAND/ABC-123      ●    │
│  14:32:08.789    W    TOOL:REQUEST       done      FS:READ_FILE           │
│ ─────────────────────────────────────────────────────────────────────────  │
│  14:32:09.012    -    REPLY:STREAM        ...      HEAD/MAIN              │
│  14:32:09.234    -    REPLY:BYTES         ...      HEAD/MAIN              │
│                                                                            │
│(◁)                                                                    (▷)  │
│                                                                            │
│┌───┐                                                                       │
││ ⏸ │                        ┌──────────────────┐    FILTER: ALL           │
│├───┤                        │ FRAME 47 OF 1289 │    MODE: LIVE            │
││ ▶ │                        └──────────────────┘                          │
│└───┘                                                                       │
└────────────────────────────────────────────────────────────────────────────┘
```

**Overlay card (top-left):** Current context — scope, thread, frame count in thread.

**Timeline rows:** Timestamp, marker (N/T/W/-), name, status, actor. Selected row marked with ●.

**Controls (bottom-left):** Pause/resume instead of zoom. Filter mode selector.

**Pagination (bottom-center):** Current position in frame buffer.

**Navigation arrows:** Previous/next frame or page.

### 14.6 Frame Inspector as Record Card

```
┌─────────────────────────────────────────────────────────────────┐
│  abc-123      KERNEL_FRAME_RECORD              2024-02-04       │
│                                                   14:32:07      │
│ ┌─────────────────────────────────────────────────────────────┐ │
│ │ FRAME_IDENTIFIER / SYSCALL_NAME                             │ │
│ │                                                             │ │
│ │     𝓃𝑒𝑒𝒹:𝓁𝑒𝒶𝓈𝑒                                            │ │
│ │                                                             │ │
│ │                                          Actor: HEAD/MAIN   │ │
│ └─────────────────────────────────────────────────────────────┘ │
│                                                                 │
│  OVERVIEW    DATA    TRACE                                      │
│  ━━━━━━━━                                                       │
│  FRAME METADATA                                      OP / STATUS│
│ ┌─────────────────────────────────────────────────────────────┐ │
│ │                                                             │ │
│ │  {                                                          │ │
│ │    "goal": "Process user request",                          │ │
│ │    "scope": "main",                                         │ │
│ │    "thread_id": "abc-123-def"                               │ │
│ │  }                                                          │ │
│ │                                                             │ │
│ └─────────────────────────────────────────────────────────────┘ │
│                                                                 │
│  NEED:LEASE                                                     │
│  ━━━━━━━━━━                                                     │
│                                                                 │
│  OP:              req                                           │
│  PARENT_ID:       def-456-ghi                                   │
│  ACTOR:           head/main                                     │
│  SCOPE:           main                                          │
│  STATUS:          ok                                            │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Header:** Frame ID (left), record type label (center), timestamp (right).

**Specimen card:** The frame's `name` field rendered in script font — the "field researcher's hand" signing the observation.

**Tab row:** OVERVIEW (metadata), DATA (payload), TRACE (parent/child relationships).

**Illustration panel:** JSON payload, syntax-highlighted or formatted as monospace.

**Key/value list:** Op, parent_id, actor, scope, status.

### 14.7 Chat View as Field Notes

When viewing a scope's chat:

```
┌─────────────────────────────────────────────────────────────────┐
│ FIELD_NOTES: SCOPE_MAIN                                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  14:30:22    OBSERVER                                           │
│  ───────────────────────────────────────────────────────────    │
│  What files are in the workspace?                               │
│                                                                 │
│  14:30:24    RESPONDENT                                         │
│  ───────────────────────────────────────────────────────────    │
│  The workspace contains the following specimens:                │
│                                                                 │
│  • src/ — source artifacts (47 files)                           │
│  • docs/ — documentation archive (12 files)                     │
│  • tui/ — terminal interface (8 files)                          │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│ ┌─────────────────────────────────────────────────────────────┐ │
│ │ ENTER_OBSERVATION: _                                        │ │
│ └─────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

**Header:** FIELD_NOTES: SCOPE_NAME

**Messages:** Timestamp, role (OBSERVER for user, RESPONDENT for assistant), content.

**Input:** Bordered text area with underscore label.

### 14.8 Status Bar

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ● ENGINE: ANTHROPIC_CLAUDE    SYSTEM_STATE: OPERATIONAL    BUILD: V0.1.0-DEV │
└──────────────────────────────────────────────────────────────────────────────┘
```

- Green dot for connected, hollow for disconnected
- Provider/model identifier
- System state (OPERATIONAL, DEGRADED, OFFLINE)
- Build version (right-aligned)

### 14.9 Implementation Phases

**Phase 1: Foundation**
- CSS custom properties from spec (section 11)
- Font loading (IBM Plex Mono + Caveat)
- Base layout grid (nav, left zone, right zone, status bar)
- Color scheme application

**Phase 2: Core Monitor View**
- Stat strip with three columns
- Frame timeline component
- Frame inspector panel
- Overlay cards (context, pagination)

**Phase 3: Navigation & Tabs**
- Nav bar with left/right groups
- Scope tab management
- View mode switching
- Underline active indicators

**Phase 4: Additional Views**
- Chat view (field notes style)
- Config editor (port from TUI)
- Explorer (archive browser)
- Logs query interface

**Phase 5: Polish**
- Keyboard shortcuts
- Pause/filter controls
- Motion (minimal, functional)
- Responsive breakpoints

### 14.10 Key Transformations from Current Web UI

| Current | Becomes |
|---------|---------|
| Dark theme (#1e1e1e) | Warm parchment (#F5F0E8) |
| Blue accent (#007acc) | Forest green (#2C6E49) |
| Rounded corners | Square corners (0px radius) |
| Box shadows | 1px borders only |
| Sans-serif labels | Monospace uppercase with underscores |
| Colored op badges | Single-color text with marker prefix |
| Bottom tab bar | Top nav bar with entity/context groups |
| Vertical trace tree | Horizontal timeline rows |

### 14.11 Typography Transformation Examples

**Current:** `need:lease`, `head/main`, `Frame Details`

**Becomes:** `NEED:LEASE`, `HEAD/MAIN`, `FRAME_DETAILS`

**Script font usage:** Only the frame's `name` field in the inspector specimen card:

```
Standard label:    FRAME_IDENTIFIER / SYSCALL_NAME
Script rendering:  𝓃𝑒𝑒𝒹:𝓁𝑒𝒶𝓈𝑒
```

This creates the core tension between institutional formality and human presence.
