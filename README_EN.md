# ChatDeepSeek

[Chinese](README.md) | [English](README_EN.md) | [License](LICENSE)

A desktop AI chat application built with **Tauri v2 + Rust + SQLite + React**, designed with the goal of "simplicity, efficiency, and a polished visual experience." It supports **OpenAI-compatible APIs** (customizable main chat model / image generation model), **web search** (Tavily + AnySearch), **image generation**, and **isolated sandbox programming tools**.

## Features

### Chat & Messages

- **Conversation isolation**: Each conversation has its own independent, isolated context and directory; the sidebar auto-groups by "year-month", and supports double-click rename and confirm-before-delete
- **OpenAI-compatible API**: All model providers use the OpenAI-compatible protocol (`chat/completions`), so you can freely add any provider; DeepSeek official (`deepseek-v4-flash` / `deepseek-v4-pro`) is pre-configured (just fill in one API key), and image generation supports SiliconFlow and Alibaba Cloud Bailian
- **Three model types**: text (no vision) / multimodal (vision) / image generation; each model can independently configure context capacity and supports **one-click connection testing**
- **Switch model per conversation**: switch the current conversation's model anytime from the left side of the input bar (grouped by provider)
- **Attachments**: upload images (PNG/JPEG/GIF/WebP/BMP, requires a multimodal model) and documents (text-based + PDF, text auto-extracted on the backend), single file ≤ 8MB, up to 10 files per batch; images that carry scripts (e.g. SVG) are rejected outright
- **Edit & resend**: edit an already-sent user message, delete its subsequent history, and regenerate with the new content
- **Deep thinking mode**: toggleable; the OpenAI-compatible protocol automatically reads `reasoning_content` and streams it as "thinking process"
- **Markdown rendering**: AI replies render in real time (syntax highlighting, tables, task lists, etc.), with code blocks as **collapsible cards** (file name auto-detected from the language tag or first-line comment), streaming output with a cursor
- **Streaming stage hints**: thinking → (search) → analyzing → answering → generating, with each stage's status switching in real time
- **Keep content when stopping**: clicking "Stop" mid-stream preserves the already-generated content and thinking process and persists them as a message; content is also preserved on network disconnects, streaming stalls (no data for 120s), or request timeouts (60s), so nothing is lost; empty-content messages never enter the next request (avoiding API 400 errors)

### Web Search (Function Calling)

- The model decides via Tool Calls whether to search and what to search, without manual intervention
- **Tavily**: best for simple daily tasks and factual data retrieval; fast and lightweight
- **AnySearch**: best for professional vertical-domain searches (finance, medical, academic, code, legal, etc.)
- **Smart routing**: the model can specify a `provider`; when unspecified, the engine is auto-selected by content keywords (professional domains → AnySearch, daily → Tavily); if one engine returns nothing it automatically falls back to the other
- Search results are pushed to the chat in real time as cards ("title + link + summary"); clicking a card previews it in the **right panel** (or opens it in a browser)
- Search strategy (auto / always Tavily / always AnySearch) and results-per-round (1-20) are configurable

### Two Modes (switch in the mode menu at the top-left of the input bar)

| Mode | Description |
| --- | --- |
| **Chat** | Normal conversation + image generation |
| **Agent** | Programming tools (read_file / write_file / edit_file / delete_file / list_files / glob / grep + isolated sandbox bash) + web search + image generation |

### Image Generation

- The model calls the `generate_image` tool (OpenAI-compatible `images/generations` API), supporting size and negative prompt; results are auto-downloaded to the conversation `images/` directory and pushed as cards in real time
- Compatible with multiple image-generation providers (based on the OpenAI-compatible protocol), auto-adapting to each provider's size-format differences
- Artifacts are displayed as thumbnail cards, clickable for preview in the right panel

### Tool Call Compatibility (Text-JSON Fallback)

Some models/APIs do not support function calling; they output tool parameters as **text JSON** (e.g. `{"prompt": "..."}`) instead of making a real call. The app auto-detects such replies and **executes them on the model's behalf**:

| Model output JSON | Recognized as |
| --- | --- |
| Contains `prompt` (Chat / Agent mode) | `generate_image` |
| Contains `command` (Agent mode) | `bash` |
| Contains `path` + `old_string` | `edit_file` |
| Contains `path` + `content` | `write_file` |
| Only `path` | `read_file` (read-only, safe) |
| Contains `pattern` | `grep` |
| Contains `dir` | `list_files` |

Recognition is strictly limited to "a whole valid JSON + characteristic fields + a tool within the current mode's toolset"; plain text or mismatched modes do not trigger it. The system prompt guides the model: when it cannot make a native function call, it outputs the JSON parameter format, and the app auto-executes and feeds the result back to continue the conversation.

### Programming Tools & OS-Level Sandbox (Agent Mode)

- **File tools**: read_file / write_file / edit_file / delete_file / list_files / glob (supports `*` / `**`) / grep, working in the conversation `files/` directory
- **bash sandbox**: commands run inside a **Windows AppContainer** — the session `files/` directory is granted (via ACL) only to that session's container at the OS level, so subprocesses **cannot access any files outside the workspace** (no matter how the command is written, `C:\...`, messages.db, etc. are all rejected by the OS); sandbox initialization failure refuses execution (fail-closed); on non-Windows platforms without AppContainer, it degrades to a normal subprocess (keeping permission confirmation and timeout)
- **Over-scope confirmation**: when file tools/commands try to access paths outside the session directory (absolute paths, `..` traversal, drive letters / UNC / environment-variable paths), a permission confirmation dialog pops up (90s timeout auto-denies); **denial does not abort the task** — the denial is fed back to the model as a tool result, and the model can continue using in-session paths
- Commands time out after 60 seconds and output is truncated to protect context; deleting/clearing a conversation immediately invalidates pending out-of-scope requests

### Right Preview Panel

- Click links, search cards, file artifacts, or image artifact cards in messages to preview them in the right panel
- **Web preview**: the backend fetches and **sanitizes the HTML** (strips script/iframe/svg/form/video/audio and other dangerous elements and dangerous protocols) before rendering in a sandboxed iframe; built-in **SSRF protection** (rejects intranet/loopback addresses, re-checks on every redirect hop, including IPv4-mapped IPv6)
- **File preview**: text files within the session directory (≤ 2MB), adaptive to the system light/dark theme
- The panel is toggled by the top-right icon; preview content can be opened in the system browser in one click

### Context Management

- Context capacity is configured per model (Settings → Providers, in kilo-tokens, default 128K), with auto estimation of usage (messages + thinking chains + tool calls + search results + attachments + fixed request overhead; documents are estimated by the actual truncation limit of 20000 chars to avoid large files overrunning)
- **Auto compression**: at **60%** usage, the model condenses early conversations into a summary (the retention window is drawn backward from the latest by a token budget, not a fixed count); afterward only the summary + recent messages are sent, so long conversations can continue; compression is skipped when the user clicks Stop
- **Over-capacity fallback**: when compression fails or a single message is too large, the oldest message groups are auto-dropped to keep the request within the model's limit (tool calls and their results stay grouped so they are never split, which would break the API)
- At **90%** usage: a banner suggests "Start a new conversation" (with one-click new/close)
- At **full (100%)**: the banner upgrades and the input bar is disabled; sending is also blocked on the backend even if attempted

### System Tray & UI

- **System tray**: closing the window minimizes to the tray (does not exit); left-click shows the main interface, tray menu offers "Show / Exit"; exiting auto-cancels all running tasks
- **Layout**: left conversation list (group/rename/delete) + chat area (message stream + input bar) + right preview panel
- **Input bar**: mode menu (Chat / Agent), attachments, model selection, web-search toggle, deep-thinking toggle, send/stop
- **Settings panel**: vertical tabs on the left (General / Providers / Model Selection / Search Services), fixed panel size; theme follows system / light / dark
- **Notifications & hints**: top-right stacked Toasts (error hints, etc.), error banners, permission confirmation dialogs

## Tech Stack

| Layer | Technology |
| --- | --- |
| Desktop framework | Tauri v2 (WebView2 / WKWebView / WebKitGTK), with system tray |
| Backend | Rust (Tokio + reqwest streaming SSE + rusqlite bundled) |
| Frontend | React 18 + TypeScript + Vite |
| Markdown | marked + DOMPurify + highlight.js (commonly used languages loaded on demand) |
| Database | SQLite (rusqlite bundled, no external dependency, WAL mode) |
| Security | Windows AppContainer sandbox (windows-sys), HTML sanitization (scraper), SSRF protection, path normalization |

## Project Structure

```
ChatDeepSeek/
├── README.md
├── package.json              # Frontend dependencies & scripts
├── vite.config.ts
├── index.html
├── tsconfig.json
├── start.bat                 # One-click launcher (env check / dev / build menu)
├── package.bat               # One-click packager (portable + NSIS installer)
├── scripts/
│   ├── package-portable.ps1  # Portable packaging (copy exe to dist-portable/)
│   └── run-release.ps1       # Build release and run directly
├── src/                      # Frontend (React + TS)
│   ├── main.tsx
│   ├── App.tsx               # App root component & state management
│   ├── types.ts              # Shared type definitions
│   ├── api.ts                # Tauri invoke wrapper + chat_event listener
│   ├── styles.css            # All styles (CSS variables, light/dark theme)
│   ├── lib/
│   │   └── markdown.ts       # Markdown rendering (marked + DOMPurify + hljs + code cards)
│   └── components/
│       ├── icons.tsx         # Inline SVG icon set
│       ├── Sidebar.tsx       # Conversation list (year/month grouping / rename / delete confirm)
│       ├── ChatView.tsx      # Chat area (message stream + context banner + search/artifact cards)
│       ├── MessageItem.tsx   # Single message (Markdown / thinking fold / copy / edit)
│       ├── DraftMessage      # Streaming message (in ChatView: status hints + thinking + search cards + artifacts)
│       ├── InputBar.tsx      # Input bar (mode/model/attachments/web-search/deep-thinking/send/stop)
│       ├── SettingsPanel.tsx # Settings panel (general / providers / model selection / search services)
│       ├── ArtifactCards.tsx # Artifact cards (image thumbnails / file chips, LRU cache)
│       ├── Markdown.tsx      # Markdown container (code folding / link interception preview)
│       ├── WebPreviewPanel.tsx # Right preview panel (web / file / image)
│       └── Toast.tsx         # Top-right stacked notifications
└── src-tauri/                # Backend (Rust)
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── build.rs
    ├── capabilities/default.json
    ├── icons/                # App icons
    └── src/
        ├── main.rs
        ├── lib.rs            # Tauri entry, command registration, tray, data-root lookup
        ├── commands.rs       # Tauri commands (sessions/messages/settings/attachments/web fetch/permission responses)
        ├── db.rs             # SQLite init / CRUD / settings access / context estimation
        ├── models.rs         # Data structures + provider/model parsing + legacy config migration
        ├── llm/
        │   ├── mod.rs        # Agent main loop (streaming + tool calls + text-JSON fallback + shared system prompt + context compression)
        │   ├── openai.rs     # OpenAI-compatible client (streaming + function calling + timeout/stall protection + summarizer)
        │   └── search.rs     # Web search (Tavily + AnySearch + smart routing + fallback switching)
        └── agent/
            ├── tools.rs      # Tool definitions & execution (files/glob/grep/bash/search/image generation)
            ├── sandbox.rs    # Session-level isolated sandbox (Windows AppContainer / non-Windows fallback)
            └── generate.rs   # Image generation
```

## Quick Start

### Requirements

- Node.js ≥ 18
- Rust ≥ 1.77
- Windows: WebView2 runtime (built into Win10/11)
- macOS: Xcode Command Line Tools
- Linux: WebKitGTK 4.1 and other Tauri system dependencies

### Development

```bash
npm install
npm run tauri dev
```

Or double-click **`start.bat`** (one-click launcher): auto-checks Node/Rust/WebView2 environments, auto-installs dependencies, and offers a menu:

| Option | Description |
| --- | --- |
| 1. Quick start | Build a release build and run (recommended, avoids Smart App Control blocking unsigned exe) |
| 2. Dev mode | `npm run tauri dev` hot-reload debugging |
| 3. Build installer | NSIS installer |
| 4. Build portable | Single exe, output to `dist-portable/` |

### Packaging & Release

```bash
npm run bundle            # Build installer (NSIS, install path selectable)
npm run bundle:portable   # Build portable and copy to dist-portable/
npm run dev:release       # Build release and run directly (no packaging)
```

Or double-click **`package.bat`** (one-click packager): auto-builds, collects artifacts to `dist-portable/`, and cleans build intermediates.

Artifacts:

| Artifact | Location | Description |
| --- | --- | --- |
| Portable | `dist-portable/ChatDeepSeek.exe` | Single-file, no-install, double-click to run; runtime (WebView2, built into Win10/11) needs no download |
| Installer | `dist-portable/ChatDeepSeek_0.1.0_x64-setup.exe` | NSIS installer, **install path selectable**, supports Chinese & English language selection |

- **Portable**: the `data` folder is auto-generated **next to `ChatDeepSeek.exe`**
- **Installer**: the `data` folder defaults to the **install directory**; if the install directory is not writable it auto-falls back to the system app-data directory
- The app embeds frontend assets and runtime dependencies, so users do not need Node.js / Rust or any other environment

## Configuration Guide

Click the **Settings** button in the bottom-left to open the settings panel (four tabs).

### 1. General

- **Conversation defaults**: default chat model (set in "Model Selection"), new conversations default web search / deep thinking on, default mode
- **Appearance**: theme (follow system / light / dark)
- **Data**: clear all conversations (double confirm, irreversible)

### 2. Providers (Model Management)

**DeepSeek official** is pre-configured (OpenAI-compatible protocol, `https://api.deepseek.com/v1`); you can also freely add any OpenAI-compatible provider:

| Config item | Description |
| --- | --- |
| Name | Custom display name |
| API protocol | Fixed OpenAI-compatible (chat/completions) |
| API Base URL | e.g. `https://api.deepseek.com/v1`, `https://api.siliconflow.cn/v1` |
| API Key | Provider secret key |

Multiple models can be added under each provider:

- **Model type**: text (no vision) / multimodal (vision, supports image input) / image generation
- **Context capacity**: configurable for chat/multimodal models (kilo-tokens, default 128K), determines context progress and auto-compression timing
- **Test**: test connectivity by type (text message for chat, test image for image generation)

> Legacy configuration (`deepseek` / `gen` fields) API keys and models are auto-migrated to the provider system.

### 3. Model Selection

Assign concrete models (provider / model) to the following purposes:

| Slot | Purpose |
| --- | --- |
| Chat model | Used for chatting; image input requires selecting a "multimodal" model |
| Image generation model | Image generation in Chat / Agent modes (compatible with SiliconFlow and Alibaba Cloud Bailian) |

### 4. Search Services

| Service | Purpose | Where to get |
| --- | --- | --- |
| Tavily API Key | Daily/simple task data search | https://tavily.com |
| AnySearch API Key | Professional vertical-domain deep search | https://www.anysearch.com/console/api-keys |

Search strategy: `Smart auto-select / Always Tavily / Always AnySearch`; results per round: 1-20 (5 recommended).

### Deep Thinking

- **OpenAI-compatible protocol**: no reasoning-strength parameter is sent; the model's returned `reasoning_content` is automatically streamed and shown as "thinking process"
- Toggle on/off via the "Deep Thinking" button in the input bar

## Conversation Workflow

AI replies use **streaming output**, and the System Prompt guides the model through "Think → Execute → Analyze → Summarize" when processing each question:

| Stage | Description | UI hint |
| --- | --- | --- |
| Think | Break down the user's intent and decide which real-time/professional data is needed | "Thinking about your question…" |
| Execute | Call the `web_search` tool to search relevant content (multiple rounds/multiple keywords possible) | "Searching with Tavily/AnySearch… got N results" |
| Analyze | Cross-validate search data against the question and extract core facts | "Analyzing N search results to extract facts relevant to the question…" |
| Summarize | Answer in a "sum-up/points/sum-up" structure: conclusion first, then point-by-point with source links, finally key takeaways | "Generating the answer…" |

- With "web search" enabled, the model is explicitly required to **actively search** when real-time or professional content is needed, and to attach source links `[source](url)` when citing results
- With "web search" disabled, the model is told it cannot access the internet and will not fabricate "searching" wording
- In Chat / Agent mode the System Prompt includes mode-specific instructions guiding tool usage

## Web Search Implementation

The AI model calls the built-in `web_search` tool through **Tool Calls (Function Calling)**:

```
web_search(query, provider?)
```

- `provider`: `auto` (default) / `tavily` / `anysearch`, chosen by the model based on the nature of the question; results per round controlled by settings (1-20)
- Smart routing (`auto` mode): when the query involves professional-domain keywords (finance, medical, academic, code, legal, etc.) it routes to AnySearch; simple daily questions use Tavily (auto-degrades when either key is missing; auto-switches to the other engine when the preferred one returns nothing)
- During search, each result is pushed to the chat in real time as a card (title + link + summary)
- Multi-round tool calls: the model can loop up to **8 rounds** of "think → tool call → answer"
- The model's available toolset is determined by the session mode: Chat = web search (toggleable) + image generation; Agent additionally adds file tools and bash

## Data Storage

All data is stored in the root `data/` folder:

```
data/
├── json/          # App settings (settings.json, includes API keys & provider config)
├── logs/          # Runtime logs (app.log, 1MB rotation keeping all; includes frontend console forwarding & error reporting)
└── sessions/      # Conversation data: one project directory per conversation
    └── <SessionID>/
        ├── session.json   # Conversation metadata (title/model/mode/switches/compression summary)
        ├── messages.db    # Conversation content (messages/thinking chains/tool calls/search results/artifact index)
        ├── files/         # File artifacts (Agent mode)
        ├── images/        # Image artifacts (Chat/Agent mode)
        └── uploads/       # User-uploaded attachments
```

- **Runtime logs**: `data/logs/app.log` records key operations and errors (conversation ops, message sending, tool calls & durations, image-generation tasks, web search, API errors, frontend errors, etc.), 1MB rotation keeping full history; inspect directly for troubleshooting
- **Atomic writes**: session.json and settings.json both use "temp file + rename"; a crash does not corrupt config
- **Auto migration**: legacy single-file `<SessionID>.json` auto-migrates to the session directory structure
- Data directory location: in dev mode it is the project-root `data/`; in production (portable/installer) it is `data/` **next to the exe**, falling back to the system app-data directory when the directory is not writable

## FAQ

- **How do I use a different model?** Settings → Providers to add providers and models, then assign purposes in Model Selection; in the current conversation, switch via the dropdown at the left of the input bar (affects only the current conversation)
- **Model only replies with JSON prompts (e.g. `{"prompt": "..."}`) and doesn't generate?** It means the conversation model does not support function calling — the app auto-detects text-JSON tool parameters and **executes them** (supported in both Chat and Agent modes); models that support tool calls use the normal call path
- **Image upload failed?** Image input needs a "multimodal (vision)" model; switch the conversation model in Settings → Model Selection to a multimodal model; SVG images are rejected because they may carry scripts
- **Search returns nothing?** Confirm you set a Tavily or AnySearch API key and that the corresponding service is enabled
- **"Context is full" prompt?** Click "Start a new conversation" on the banner, or create a new conversation to continue
- **bash command denied?** Commands that might access files outside the session directory (drive letters / UNC, etc.) require user confirmation; after denial the model is notified and can continue using in-session paths; sandbox subprocesses cannot access anything outside the workspace at the OS level
- **Content disappears after stopping?** Generated content and the thinking process are automatically kept as a message; if you're still on an old exe, repackage with `package.bat`
- **Smart App Control blocks startup?** Use `start.bat` option 1 (release build), or turn off Smart App Control in Windows Security