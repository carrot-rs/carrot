# Carrot — GitHub Setup & Migration Leitfaden

**Für:** den Dev, der das Setup macht
**Stand:** April 2026, basiert auf den aktuellen GitHub-Features (Issue Types, Sub-issues, Discussions)

---

## Ziel

Das Carrot-Projekt von verstreuten lokalen Markdown-Files in ein sauberes, modernes GitHub-Setup migrieren. Das Setup muss zwei Zielgruppen bedienen:

1. **Public** (User, potenzielle Contributors, Sponsoren): klare Roadmap, was kommt wann, wo kann ich helfen
2. **Dev** (wir): Tracking, Hierarchie, Sub-issues für Progress

Wir bauen kein separates Roadmap-Repo. Alles lebt im Haupt-Repo `nyxb/carrot`. Begründung: 2026 erlauben Issue Types + Sub-issues sauber alles in einem Repo abzubilden, und ein zweites Repo erzeugt nur Maintenance-Last.

**Wir bauen auch kein public Project Board.** Das ist Absicht, nicht Vergessen — siehe nächster Abschnitt.

---

## Warum kein Project Board

Frische, erfolgreiche Projekte 2024–2026 (Biome, Hey API, MCP, Roo Code) verzichten bewusst auf öffentliche Project Boards. Drei Gründe:

1. **Boards veralten schnell.** Ein stale Board sieht schlechter aus als gar keins.
2. **Discoverability.** Eine pinned Discussion + ein Blog-Post bekommen RSS, Search-Traffic, Tweets. Ein Board-Link wird nicht geteilt.
3. **Sub-issues haben den Bedarf entschärft.** Seit April 2025 GA können Initiative → Epic → Feature → Task als saubere Hierarchie in Issues gebaut werden, mit automatischen Progress-Bars. Kein Board nötig.

Statt Board verwenden wir:

- **Discussion `Roadmap 2026`** als Public-Roadmap (pinned)
- **Pinned Initiative-Issues** im Repo (bis zu 3, am Issues-Tab)
- **Issue-Search mit Filtern** für interne Triage und Working-Views (`is:open type:Initiative`, `no:label`, etc.)
- **Sub-issue-Hierarchie** für Progress-Tracking

Wenn Carrot in 12 Monaten so groß ist, dass ein Board echten Wert liefert, kann es in 30 Minuten nachträglich angelegt werden. Vorher: lieber nicht.

---

## Sprach-Regel (wichtig, gilt überall)

- **Lokale Plan-Files (`/plan/*.md`) sind deutsch.** Bleiben deutsch. Sind interne Working-Docs.
- **Alles auf GitHub ist englisch.** Issues, PRs, Discussions, Labels, README, ROADMAP, CONTRIBUTING, Issue-Templates, Commit-Messages. Ohne Ausnahme.
- **Übersetzung passiert beim Migrieren.** Nicht "erst rüberkopieren, dann später übersetzen" — die englische Version wird direkt im Migrations-Schritt erstellt. Der deutsche Originaltext bleibt im `plan/`-Ordner als Working-Doc.
- Begründung: Public-facing Repo, internationale Contributors, Sponsoren-Sichtbarkeit. Deutsch auf GitHub schließt 95 % der potenziellen Mitstreiter aus.

---

## Vorbereitung

Was Dennis dir gibt:

- Zugriff auf die GitHub-Org `nyxb` (Owner-Rolle nötig für Issue-Types-Setup)
- Den Ordner `/Users/nyxb/Projects/carrot/plan/` mit allen lokalen `.md`-Files (Pläne, Notizen, Architektur)
- Eine kurze Liste der 8 Phasen und 11 Feature-Kategorien (falls noch nicht in den MD-Files klar erkennbar)

Was du brauchst:

- GitHub CLI (`gh`) lokal eingerichtet
- Schreibrechte auf das `nyxb/carrot`-Repo
- Verständnis vom Code-Stand (siehe Phase 2.0 — Plan-Status-Check)

---

## Phase 0 — Org-Level Setup (einmalig, ~10 min)

Diese Sachen werden auf Org-Ebene konfiguriert (`github.com/organizations/nyxb/settings`) und gelten für alle Repos der Org.

### 0.1 Issue Types definieren

Settings → Planning → Issue types → Create new type. Diese fünf anlegen, in dieser Reihenfolge:

| Name | Beschreibung | Wann nutzen |
|------|--------------|-------------|
| `Initiative` | Eine der 8 Phasen oder eine große strategische Richtung | Phase-Level: "Phase 1 — Core Terminal", "Phase 5 — AI Integration" |
| `Epic` | Eine der 11 Feature-Kategorien innerhalb einer Phase | "Command Blocks", "Native Editor", "Nushell Integration" |
| `Feature` | Konkretes User-facing Feature | "Block-folding", "Syntax Highlighting via Tree-sitter" |
| `Task` | Implementations-Stück, oft kein User-Wert allein | "Wire up tree-sitter dependency", "Add config for theme path" |
| `Bug` | Defekt | Standard |

Wichtig: Issue Types sind organisationsweit — keine separaten Sets pro Repo.

Das war's für Org-Level. Kein Project, kein Workflow.

---

## Phase 1 — Repo-Level Setup (einmalig, ~45 min)

Alles in `nyxb/carrot`.

### 1.1 Labels

Standard-Labels (bug, enhancement, etc.) löschen oder behalten egal — wir nutzen primär Issue Types. Diese Labels anlegen:

**Area-Labels** (für Filter, mehrfach möglich pro Issue):

```
area:terminal       — Terminal-Layer, PTY, Blocks
area:editor         — Native Editor, Buffer, Cursor
area:gpu            — Rendering, GPUI, Shaders
area:nu             — Nushell-Integration
area:ai             — AI-Features, Inline-Assist
area:config         — Settings, Themes, Keymaps
area:plugin         — Extension-System
area:platform       — OS-spezifisch (macOS/Linux/Windows)
area:docs           — Dokumentation
area:ci             — Build, Release, GitHub Actions
```

**Phase-Labels** (Phase, in der das Issue umgesetzt werden soll):

```
phase:1   — Core Terminal
phase:2   — Nushell Integration
phase:3   — Native Editor
phase:4   — (siehe Plan-Files)
phase:5   — (siehe Plan-Files)
phase:6   — (siehe Plan-Files)
phase:7   — (siehe Plan-Files)
phase:8   — (siehe Plan-Files)
phase:exploring   — noch keine Phase zugewiesen
```

**Priority-Labels** (eins pro Issue):

```
priority:p0   — now (this iteration)
priority:p1   — next (this quarter)
priority:p2   — later (this year)
priority:p3   — someday
```

**Status-Labels** (selten genutzt, primär für besondere Zustände):

```
help-wanted         — wir suchen Hilfe
good-first-issue    — neue Contributors willkommen
blocked             — wartet auf was anderes
needs-design        — braucht erst Design-Diskussion
```

Per `gh` CLI gehts schnell:

```bash
# Areas
gh label create "area:terminal"  --color "0E8A16" --description "Terminal layer, PTY, blocks"
gh label create "area:editor"    --color "1D76DB" --description "Native editor, buffer, cursor"
gh label create "area:gpu"       --color "B60205" --description "Rendering, GPUI, shaders"
gh label create "area:nu"        --color "5319E7" --description "Nushell integration"
gh label create "area:ai"        --color "FBCA04" --description "AI features, inline assist"
gh label create "area:config"    --color "C5DEF5" --description "Settings, themes, keymaps"
gh label create "area:plugin"    --color "BFD4F2" --description "Extension system"
gh label create "area:platform"  --color "D4C5F9" --description "OS-specific work"
gh label create "area:docs"      --color "0075CA" --description "Documentation"
gh label create "area:ci"        --color "CCCCCC" --description "Build, release, CI"

# Phases
gh label create "phase:1" --color "FEF2C0" --description "Phase 1 — Core Terminal"
gh label create "phase:2" --color "FEF2C0" --description "Phase 2 — Nushell Integration"
gh label create "phase:3" --color "FEF2C0" --description "Phase 3 — Native Editor"
gh label create "phase:4" --color "FEF2C0" --description "Phase 4"
gh label create "phase:5" --color "FEF2C0" --description "Phase 5"
gh label create "phase:6" --color "FEF2C0" --description "Phase 6"
gh label create "phase:7" --color "FEF2C0" --description "Phase 7"
gh label create "phase:8" --color "FEF2C0" --description "Phase 8"
gh label create "phase:exploring" --color "EDEDED" --description "No phase assigned yet"

# Priorities
gh label create "priority:p0" --color "B60205" --description "Now"
gh label create "priority:p1" --color "D93F0B" --description "Next"
gh label create "priority:p2" --color "FBCA04" --description "Later"
gh label create "priority:p3" --color "C5DEF5" --description "Someday"

# Status
gh label create "good-first-issue" --color "7057FF"
gh label create "help-wanted"      --color "008672"
gh label create "blocked"          --color "E11D21"
gh label create "needs-design"     --color "F9D0C4"
```

Phase-Labels und Priorities mit Dennis abstimmen, sobald die finalen Phasen-Namen feststehen.

### 1.2 Issue Templates

Anlegen in `.github/ISSUE_TEMPLATE/`. Komplett englisch.

`bug_report.yml`

```yaml
name: Bug report
description: Report a defect in Carrot
type: Bug
labels: []
body:
  - type: textarea
    id: what-happened
    attributes:
      label: What happened?
      description: Describe the bug and what you expected instead.
    validations:
      required: true
  - type: textarea
    id: repro
    attributes:
      label: Steps to reproduce
    validations:
      required: true
  - type: input
    id: version
    attributes:
      label: Carrot version
    validations:
      required: true
  - type: dropdown
    id: os
    attributes:
      label: Operating system
      options: [macOS, Linux, Windows]
    validations:
      required: true
```

`feature_request.yml`

```yaml
name: Feature request
description: Propose a new feature or improvement
type: Feature
labels: []
body:
  - type: textarea
    id: problem
    attributes:
      label: What problem does this solve?
    validations:
      required: true
  - type: textarea
    id: proposal
    attributes:
      label: Proposed solution
  - type: textarea
    id: alternatives
    attributes:
      label: Alternatives considered
```

`config.yml`

```yaml
blank_issues_enabled: false
contact_links:
  - name: 💬 Discussions
    url: https://github.com/nyxb/carrot/discussions
    about: Open-ended ideas, questions, and roadmap input go here.
```

### 1.3 ROADMAP.md im Repo-Root

```markdown
# Roadmap

Carrot's public roadmap is the pinned discussion:
**[🗺️ Roadmap 2026](https://github.com/nyxb/carrot/discussions/<NUMMER>)**

It outlines what we're building this year and how the phases fit together.

## Tracking

- **Initiatives** (one per phase) are tracked as pinned issues — see the
  [pinned issues at the top of the Issues tab](https://github.com/nyxb/carrot/issues).
- Each Initiative contains **Epics** (feature categories) as sub-issues.
- Each Epic contains **Features** as sub-issues.
- Browse all of them with type filters:
  - [All Initiatives](https://github.com/nyxb/carrot/issues?q=is%3Aissue+type%3AInitiative)
  - [All Epics](https://github.com/nyxb/carrot/issues?q=is%3Aissue+type%3AEpic)
  - [Open Features](https://github.com/nyxb/carrot/issues?q=is%3Aissue+is%3Aopen+type%3AFeature)

## Contributing

The best entry points are issues labeled
[`good-first-issue`](https://github.com/nyxb/carrot/labels/good-first-issue) and
[`help-wanted`](https://github.com/nyxb/carrot/labels/help-wanted).

For the why behind architectural decisions, see
[`docs/architecture/`](./docs/architecture/).
```

Die Discussion-Nummer eintragen, sobald Phase 3 durch ist.

### 1.4 CONTRIBUTING.md (kurz halten)

```markdown
# Contributing to Carrot

Thanks for your interest! Before opening a PR:

1. Check whether an issue exists. If not, open one and let's discuss before you write code.
2. For larger features, comment on the relevant Initiative or Epic issue first.
3. PRs should reference an issue (`Closes #123` or `Refs #123`).
4. Keep PRs scoped — one feature or fix per PR.

For build instructions, see [BUILDING.md](./BUILDING.md).
```

---

## Phase 2 — Migration der lokalen MD-Files (das Kernstück)

### 2.0 Plan-Status-Check (PFLICHT vor jeder Migration)

> **Diese Stufe ist nicht optional. Sie steht vor allem anderen in Phase 2.**

Die `plan/`-Files sind über Monate gewachsen. Viele sind teilweise oder vollständig umgesetzt — aber im Git-Log und Code, nicht im File selbst dokumentiert. Wer jetzt jeden Plan blind als Issue rüberkippt, erzeugt ein GitHub voller "ist eigentlich schon fertig"-Issues und verliert Vertrauen in die Roadmap.

**Workflow pro `plan/<file>.md`:**

1. **Plan lesen.** Nicht überfliegen — die nummerierten Schritte und Akzeptanzkriterien wirklich verstehen. Was ist das Outcome? Welche Dateien werden angefasst? Welche Hard Rules werden eingeführt?

2. **Code gegenchecken.** Für jeden Implementations-Punkt:
   - Existiert die Datei / das Modul / der Crate?
   - Macht der Code das, was der Plan beschreibt?
   - Sind die im Plan genannten Hard Rules in `CLAUDE.md` / `ARCHITECTURE.md` festgehalten?
   - `git log --all --oneline -- <pfad>` zeigt, ob da überhaupt schon was passiert ist.
   - Bei Zweifel: `cargo check --workspace` und `cargo test --workspace` fahren — wenn der Plan fertig wäre, wären die im Plan genannten Tests grün.

3. **Klassifizieren** (eine von drei Kategorien):

   | Status | Bedeutung | Aktion |
   |--------|-----------|--------|
   | **Done** | Alle Plan-Punkte sind im Code umgesetzt, alle Akzeptanzkriterien erfüllt | Datei nach `plan/done/` verschieben. **Kein** GitHub-Issue. |
   | **Partial** | Manche Punkte umgesetzt, andere noch offen | Nur die **offenen** Punkte als GitHub-Issue(s) anlegen (englisch). Datei bleibt vorerst in `plan/`, wird mit Verweis auf Issue-Nummer ergänzt. Nach Abschluss → `plan/done/`. |
   | **Open** | Nichts oder fast nichts umgesetzt | Plan vollständig als GitHub-Issue-Hierarchie anlegen (englisch). Datei bleibt in `plan/` als Working-Doc bis erledigt. |

4. **Wenn Issues erstellt werden: Englisch übersetzen.** Der deutsche Plan-Text bleibt lokal. Auf GitHub landet die englische Fassung — sauber strukturiert, für Außenstehende verständlich, ohne deutsch-spezifische Idiome.

5. **Verlinkung beidseitig.** Im Plan-File oben einen Header-Block einfügen:

   ```markdown
   > **Status:** Partial — see [#42](https://github.com/nyxb/carrot/issues/42)
   > **Letzter Code-Check:** 2026-04-27
   ```

   Im Issue-Body unten:

   ```markdown
   ---
   _Migrated from internal plan: `plan/19-SETTINGS-SYSTEM-MIGRATION.md` (German). Translated to English._
   ```

6. **`done-maybe/` jetzt klären.** Der Ordner `plan/done/done-maybe/` existiert, weil bei manchen Plänen unklar war, ob sie wirklich durch sind. Den Status-Check JETZT machen — entweder in `plan/done/` schieben oder als Partial-Plan zurück in `plan/` und passende Issues erstellen.

7. **`_recovered-orphans/` triagieren.** Diese Files sind aus früheren Sessions wiederhergestellt. Erst lesen, dann entscheiden:
   - Inhaltlich Duplikat eines existierenden Plans? → löschen.
   - Eigenständiger Plan, noch relevant? → in `plan/` einsortieren mit normaler Nummerierung, dann normalen Status-Check.
   - Veraltet, überholt, ohne Bezug? → in `plan/_archive/` (lokal, nicht committen).

**Reihenfolge der Files** beim Status-Check:

1. Erst `plan/done/done-maybe/*.md` — den Zustand klären, sind die wirklich done.
2. Dann `plan/_recovered-orphans/*.md` — triagieren oder löschen.
3. Dann `plan/*.md` numerisch von oben (00, 05, 06, 07, 10, 11, …) bis unten.
4. Zuletzt die unnumerierten Files (`BLOCK-SYSTEM.md`, `PROGRESS.md`, `CARROT-CMDLINE.md`, `VERTICAL-TABS.md`, etc. — Hinweis: einige tragen aktuell noch `RAIJIN-*`-Namen und werden im Zuge von `plan/RENAME-RAIJIN-TO-CARROT.md` mit umbenannt).

**Anti-Pattern (nicht machen):**

- Plan rüberkopieren ohne Code-Check — erzeugt Pseudo-Issues.
- Plan auf deutsch ins Issue kippen — Public-facing Repo bleibt englisch.
- Plan löschen, weil "Issue ist ja jetzt da" — der deutsche Original-Text bleibt als interne Working-Doc bis das Feature wirklich durch ist (dann `plan/done/`).
- "done-maybe" ungeprüft lassen — das ist genau der Schritt, den dieser Workflow erzwingt.

### 2.1 Mapping-Tabelle

Geh die Files einzeln durch (nach dem Status-Check aus 2.0) und ordne sie ein:

| Was im MD-File steht | Wird zu | Beispiel |
|----------------------|---------|----------|
| Gesamt-Vision, Pitch, "Was ist Carrot?" | `README.md` (gekürzt) + Roadmap-Discussion | Übergeordnete Story |
| 8-Phasen-Plan | 8 Initiative-Issues | "Phase 1 — Core Terminal" |
| Feature-Kategorien (die 11) | Epic-Issues, jeweils Sub-issue der passenden Initiative | "Command Blocks" unter "Phase 1" |
| Konkrete Features pro Kategorie | Feature-Issues, Sub-issues vom Epic | "Block folding" unter "Command Blocks" |
| Implementierungs-Tasks | Task-Issues, Sub-issues vom Feature — ODER inline als Markdown-Liste | "Add tree-sitter as dep" |
| Architektur-Entscheidungen, Begründungen | `docs/architecture/` (bleibt MD, **englisch übersetzt**) | DI, Module-Layout |
| Build/Setup-Anleitungen | `BUILDING.md` (englisch) | Cargo-Flags, Toolchain |
| Branding, Mascot-Notes, persönliche Brainstorms | NICHT migrieren — bleiben lokal/privat | "Nibble" Char-Sheet |
| Halbgare Ideen, ungefilterte Notizen | NICHT migrieren | TODO.md, scratch.md |

### 2.2 Schritt-für-Schritt Workflow (pro File)

Für jeden MD-File **nach erfolgreichem Status-Check (2.0):**

1. **Klassifizieren.** Welcher Reihe in der Tabelle entspricht das File?
2. **Wenn Issue (Status Open / Partial):**
   - Englische Issue-Version verfassen (nicht copy-paste aus dem deutschen Plan, sondern neu strukturiert für Außenstehende)
   - Issue erstellen mit `gh issue create`
   - Korrekten Issue Type setzen
   - Labels setzen: `area:*`, `phase:N`, `priority:*`
   - Bei Sub-issue: das Parent-Issue im Body referenzieren, dann am Parent unter "Sub-issues" → "Add existing issue" hinzufügen
   - Im deutschen Plan-File Status-Header eintragen mit Issue-Link
3. **Wenn Doku (Architektur, Build):** Datei nach `docs/` oder Root verschieben mit klarem Namen — **vorher englisch übersetzen**.
4. **Wenn Status Done:** Datei nach `plan/done/` verschieben. Fertig. Kein Issue.
5. **Originalbelassen.** Den deutschen Plan-File NICHT löschen — der bleibt als interne Working-Doc, wandert mit dem Feature mit (offen → in `plan/`, fertig → in `plan/done/`).

### 2.3 Reihenfolge der Issue-Erstellung

**Wichtig: Top-down. Zuerst Initiatives, dann Epics, dann Features.** Sonst hast du keine Parents zum Verlinken.

1. **Initiatives erstellen** — 8 Stück, eine pro Phase. Body knapp halten (englisch):

   ```markdown
   Title: Phase 3 — Native Editor

   ## Goal
   Build a Zed-level native editor inside Carrot.

   ## Why
   The terminal alone is not the differentiator. The editor-in-terminal
   is what makes Carrot a TDE rather than a terminal emulator.

   ## Scope (high level)
   - Buffer, cursor, selection
   - Tree-sitter syntax highlighting
   - LSP integration
   - Multi-cursor

   ## Out of scope (for this phase)
   - Plugin system (Phase 6)
   - AI inline assist (Phase 7)

   ## Tracking
   Sub-issues below track the Epics under this Initiative.
   ```

   Issue Type: `Initiative`. Labels: `phase:3`, `area:editor`. **Pinnen im Repo** (Issues-Tab → Pin issue).

2. **Epics erstellen** — die 11 Feature-Kategorien. Body ähnlich strukturiert. Issue Type: `Epic`. Als Sub-issue der jeweiligen Initiative hinzufügen. Labels: `phase:N`, `area:*`.

3. **Features erstellen** — alles aus den MD-Files (Status Open / Partial), was ein konkretes User-facing Feature ist. Issue Type: `Feature`. Als Sub-issue des Epics. Labels: `phase:N`, `area:*`, `priority:*`.

4. **Tasks** — nur erstellen, wenn's wirklich groß genug ist. Kleine Implementierungs-Stücke können auch als Markdown-Liste im Feature-Body bleiben (`- [ ] Add dependency`).

### 2.4 CLI-Helper für Bulk-Erstellung

Wenn die englischen Issue-Bodies fertig vorliegen, kann man Issues skripten:

```bash
# Initiative erstellen
gh issue create \
  --repo nyxb/carrot \
  --title "Phase 3 — Native Editor" \
  --body-file ./migration/phase-3-body.md \
  --label "area:editor,phase:3,priority:p1"

# Issue Type setzen geht aktuell (April 2026) am saubersten via GraphQL:
gh api graphql -f query='
  mutation {
    updateIssue(input: {id: "<ISSUE_NODE_ID>", issueTypeId: "<INITIATIVE_TYPE_ID>"}) {
      issue { id }
    }
  }'
```

Issue-Type-IDs einmalig holen:

```bash
gh api graphql -f query='
  query { organization(login: "nyxb") {
    issueTypes(first: 10) { nodes { id name } }
  }}'
```

Sub-issue-Verknüpfung via GraphQL ist umständlich — bei <100 Issues lohnt es sich nicht. Nimm das UI: am Parent-Issue unter "Sub-issues" → "Add existing issue".

### 2.5 Was NICHT migriert wird

- Persönliche Brainstorms ("Was wäre wenn…")
- Halbgare Ideen ohne klares Outcome
- Branding-Material (Mascot, Color-Schemes, Logo-Iterationen) — gehört in ein internes Brand-Repo oder bleibt lokal
- Wettbewerbs-Analysen (Warp, Wave, Ghostty notes)
- Persönliche TODOs / Scratch-Files
- Pläne, die bereits vollständig im Code umgesetzt sind (→ `plan/done/`)

Faustregel: Wenn ein Outsider den Eintrag nicht ohne Kontext versteht oder er privat ist → bleibt offline.

---

## Phase 3 — Public-facing Roadmap

### 3.1 Roadmap 2026 Discussion

Repo → Discussions → New discussion → Category: `Announcements` (vorher in Discussions-Settings als locked-to-maintainers-only anlegen, damit User dort nicht kommentieren können — Diskussion läuft in `Ideas` und `Q&A`).

Title: `🗺️ Carrot Roadmap 2026`

Body-Vorlage (englisch):

```markdown
# Carrot Roadmap 2026

Carrot is a GPU-accelerated Terminal Development Environment (TDE)
written in Rust on GPUI. This document outlines what we're working on
and what's next.

This roadmap is a living document. Things shift. Treat it as direction,
not commitment.

## What is Carrot?

A new category of tool — not a terminal emulator with editor features,
not an editor with a terminal pane. A TDE: terminal blocks (Warp-style),
native editor (Zed-level), and full Nushell support, in one app.

## 2026 Themes

### Now
- **Phase 1: Core Terminal** — solid PTY, command blocks, theme system.
  → [Track progress in #X](link)

### Next
- **Phase 2: Nushell Integration** — first-class Nu support, structured pipelines.
  → [Track progress in #X](link)

### Later
- Phase 3: Native Editor
- Phase 4: …
- Phase 5: …

### Exploratory
- AI inline assist
- Plugin system
- Mobile companion (very early thinking)

## How to engage

- 👀 **Watch** the [pinned Initiative issues](https://github.com/nyxb/carrot/issues) for live progress
- 💡 **Suggest ideas** in [Discussions → Ideas](https://github.com/nyxb/carrot/discussions/categories/ideas)
- 🐛 **Report bugs** via [Issues](https://github.com/nyxb/carrot/issues/new/choose)
- ❤️ **Support** via GitHub Sponsors (link folgt)

---

_Last updated: <Datum>_
```

Discussion **pinnen** (Discussion → ⋯ → Pin discussion).

Discussion-Categories anlegen, falls noch nicht da:
- `📣 Announcements` (maintainer-only writes)
- `💡 Ideas` (für Feature-Vorschläge)
- `🙏 Q&A` (für Nutzungsfragen)
- `🏗️ Design` (für Architektur-Diskussionen)
- `👋 General` (alles andere)

### 3.2 Repo-Pins

Im Repo-Tab "Issues" werden bis zu 3 Issues prominent angezeigt. Zu pinnen:

1. **Die aktuelle Phase-Initiative** ("Phase 1 — Core Terminal") — das ist der zentrale Tracking-Punkt mit allen Sub-issues und Progress-Bar.
2. **Ein "good-first-issues"-Sammel-Issue** oder eine wichtige aktive Feature-Anfrage — Einstiegspunkt für Contributors.
3. **(Optional) Roadmap-Meta-Issue**, das auf die Discussion verweist — falls jemand auf den Issues-Tab landet ohne die Discussion zu sehen.

### 3.3 README.md aufräumen

Komplett englisch. Oben in der README:

- Logo + One-liner ("Carrot: Terminal Development Environment")
- Status-Badge (z.B. "🚧 Pre-alpha — Phase 1 in progress")
- Demo-GIF oder Screenshot, sobald vorhanden
- Quick-Start
- Links: Roadmap (zur Discussion), Discussions, Sponsors

Dichte rauspressen — kein 10-Bildschirm-README. Details in `docs/`.

---

## Going Forward — Conventions

Damit das Setup nicht in 6 Monaten chaotisch wird:

### Wo neue Sachen hinkommen

| Was | Wohin |
|-----|-------|
| "Wäre cool wenn Carrot X könnte" | Discussion (Category: `Ideas`) |
| Bug | Issue (`bug_report.yml`) |
| Konkretes Feature mit Plan | Issue (`feature_request.yml`), als Sub-issue des passenden Epics |
| Frage zur Nutzung | Discussion (Category: `Q&A`) |
| Architektur-Diskussion | Discussion (Category: `Design`) → bei Konsens Markdown-PR an `docs/architecture/` |
| Interner Implementierungs-Plan (deutsch, working) | `plan/<NN>-<SLUG>.md` lokal |

**Regel:** Ideen starten in Discussions, werden zu Issues sobald jemand committed zu bauen.

### Plan-Workflow ab jetzt

1. Neuer Plan? → `plan/<NN>-<SLUG>.md` (deutsch, lokal).
2. Bevor Code geschrieben wird: passendes GitHub-Issue erstellen (englisch, als Sub-issue des richtigen Epics).
3. Plan-File kriegt Status-Header mit Issue-Link.
4. Während Implementation: Plan-File aktualisieren, Issue-Updates in englisch dort dokumentieren.
5. Wenn fertig + getestet + gemerged: Plan-File nach `plan/done/`, Issue close.
6. **Nie wieder einen Plan vergessen, der schon längst fertig ist.**

### Issue-Hierarchie pflegen

- Sub-issues nicht einfach als Markdown-Tasklist (`- [ ] thing`) im Body — sondern als richtige Sub-issues. Das ist der einzige Weg, dass Progress-Bars rollen und Filterung sauber funktioniert.
- Maximal 3 Hierarchie-Ebenen nutzen: Initiative → Epic → Feature. Tasks nur wenn nötig (4. Ebene), sonst inline als Checkbox-Liste im Feature-Body.

### Triage-Workflow ohne Project Board

GitHub Issues hat seit 2025 Advanced Search mit AND/OR/Parens. Damit lässt sich alles abdecken, was sonst ein Board macht:

```
# Frische Issues ohne Triage
is:issue is:open no:label

# Was ist im aktuellen Phase 1 in Arbeit
is:issue is:open label:phase:1 -label:blocked

# Alle Initiatives & Epics (Roadmap-Sicht)
is:issue type:Initiative,Epic

# Alle offenen Bugs in einem Bereich
is:issue is:open type:Bug label:area:terminal

# Was ist priorisiert für jetzt
is:issue is:open label:priority:p0
```

Diese Suchen können als **Saved Searches** im Browser gebookmarkt oder als Links in der README verlinkt werden. Reicht für Solo + kleine Teams völlig.

### PR-Conventions

- Branch-Namen: `feat/<area>-<short-desc>`, `fix/<area>-<short-desc>`
- PR-Title: gleicher Stil wie Conventional Commits, falls eingeführt — englisch
- PR-Body muss ein Issue referenzieren: `Closes #123` oder `Refs #123` — englisch
- Wenn ein PR ein Sub-Feature implementiert: das Feature-Issue wird beim Merge automatisch geschlossen, und der Progress-Bar im Parent-Issue (Epic) rollt automatisch hoch.

### Wartung

- **Wöchentlich** (5 min): Issues-Tab → Filter `no:label` → frische Issues klassifizieren (Type, Area, Phase, Priority).
- **Monatlich** (15 min): Pinned Initiative-Issues durchgehen — was steht zu lange auf "in progress"? Plan-Status-Check für betroffene `plan/`-Files wiederholen.
- **Jährlich** (1 h): Roadmap-Discussion komplett überarbeiten, alte Initiatives schließen, neue aufmachen.

### Wenn das Setup zu klein wird

Wenn Carrot mal so groß ist, dass Search-Filter nicht mehr reichen (~1000+ Issues, Team >3 Devs, Sponsoren wollen Timeline-View), kann ein public Project Board nachträglich angelegt werden. Auto-add-Workflow zieht dann alle existierenden Issues automatisch rein. Aufwand: ~30 min. Vorher: lieber nicht.

---

## Migrations-Checkliste

Damit nichts vergessen wird:

- [ ] **Phase 2.0 abgeschlossen:** alle `plan/*.md` durch den Status-Check geschickt, `done-maybe/` aufgelöst, `_recovered-orphans/` triagiert
- [ ] Org Issue Types angelegt (Initiative, Epic, Feature, Task, Bug)
- [ ] Repo-Labels angelegt (area:*, phase:*, priority:*, status-labels)
- [ ] Issue-Templates in `.github/ISSUE_TEMPLATE/` committed (englisch)
- [ ] `ROADMAP.md`, `CONTRIBUTING.md` im Repo (englisch)
- [ ] Discussion-Categories konfiguriert (Announcements, Ideas, Q&A, Design, General)
- [ ] 8 Initiative-Issues erstellt (eine pro Phase, englisch)
- [ ] 11 Epic-Issues erstellt, als Sub-issues der jeweiligen Initiative (englisch)
- [ ] Feature-Issues erstellt für alle Plan-Files mit Status Open/Partial (englisch)
- [ ] Architektur-Docs nach `docs/architecture/` migriert (englisch übersetzt)
- [ ] `Roadmap 2026` Discussion erstellt und gepinnt (englisch)
- [ ] Aktuelle Phase-Initiative + 1-2 wichtige Issues im Repo gepinnt
- [ ] README.md aufgeräumt mit Links zu Roadmap-Discussion + Issues (englisch)
- [ ] Lokale MD-Files mit Status Done in `plan/done/` verschoben
- [ ] Lokale MD-Files mit Status Partial/Open haben Status-Header mit Issue-Link
- [ ] Smoke-Test: Kann ein Outsider in 60 Sekunden verstehen, was Carrot ist und was als nächstes kommt? Wenn nein → README + Roadmap nachschärfen.
- [ ] Sprach-Audit: keinerlei deutscher Text auf GitHub (Issues, PRs, Labels, Discussions, Templates, Docs außer denen die explizit als deutsche internal-docs markiert sind und gar nicht erst gepusht werden sollten).

---

## Wenn was unklar ist

Frag Dennis. Lieber kurz nachhaken als irgendwas raten und später migrieren-müssen. Speziell bei:

- Welche Phase aktuell wirklich "in progress" ist
- Welche Feature-Kategorien zu welcher Phase gehören (nicht in jedem MD-File ist das eindeutig)
- Was als public vs. internal gilt
- Ob ein Plan wirklich Done ist oder nur "fühlt sich done an" (Code-Check ist hier nicht verhandelbar — siehe Phase 2.0)

Viel Erfolg.
