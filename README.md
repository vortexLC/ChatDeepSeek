# ChatDeepSeek

一款使用 **Tauri v2 + Rust + SQLite + React** 构建的桌面端 AI 对话应用，以「精简、高效、高质量视觉体验」为设计目标，支持**OpenAI 兼容接口**（自定义主体模型 / 图片生成模型）、**联网搜索**（Tavily + AnySearch）、**图片生成**与**隔离沙箱编程工具**。

## 功能特性

### 对话与消息

- **对话隔离**：每个对话拥有独立、隔离的上下文与目录，互不干扰；侧边栏按「年-月」自动分组，支持双击重命名、二次确认删除
- **OpenAI 兼容接口**：模型提供商统一使用 OpenAI 兼容协议（chat/completions），可自由添加任意服务商；预置 DeepSeek 官方（`deepseek-v4-flash` / `deepseek-v4-pro`，填入一个 Key 即可使用），图片生成适配硅基流动与阿里云百炼
- **模型三类分型**：文本（无视觉）/ 多模态（视觉）/ 图片生成；每个模型可独立配置上下文容量，并支持**一键测试连接**
- **消息框切换模型**：当前对话可在消息框左侧随时切换对话模型（按服务商分组显示）
- **附件上传**：支持上传图片（PNG/JPEG/GIF/WebP/BMP，需多模态模型）与文档（文本类 + PDF，后端自动提取文本），单文件 ≤ 8MB、单次最多 10 个；SVG 等携带脚本的图片直接拒绝
- **编辑重发**：编辑已发送的用户消息，删除其后的历史并以新内容重新生成
- **深度思考模式**：可开关；OpenAI 兼容协议自动读取 `reasoning_content` 流式展示为「思考过程」
- **Markdown 渲染**：AI 回复实时渲染（代码高亮、表格、任务列表等），代码块为**可折叠卡片**（自动从语言标签或首行注释识别文件名），流式输出带光标
- **流式阶段提示**：思考 →（搜索）→ 分析 → 回答 → 生成，各阶段状态实时切换
- **停止生成保留内容**：流式中途点击「停止」，已生成的内容与思考过程自动保留并持久化为消息；网络中断、流式停滞（120 秒无数据）或请求超时（60 秒）时同样保留已生成内容，不会丢失；空内容消息不会进入后续请求（避免接口 400）

### 联网搜索（Function Calling）

- 模型通过 Tool Calls 自主决定是否搜索、搜索什么，无需手动干预
- **Tavily**：适合简单日常任务、事实类数据检索，快速轻量
- **AnySearch**：适合专业垂直领域内容搜索（财经、医疗、学术、代码、法律等）
- **智能路由**：模型可指定 `provider`；未指定时按查询内容关键词自动选择引擎（专业领域 → AnySearch，日常 → Tavily），单引擎无结果时自动切换另一引擎兜底
- 搜索结果以「网页标题 + 链接」卡片实时推送到聊天界面，点击卡片在**右侧面板**内预览（也可在浏览器中打开）
- 搜索策略（模型自动 / 始终 Tavily / 始终 AnySearch）与每轮结果数（1-20）可配置

### 两种模式（发送框左上角模式菜单切换）

| 模式 | 说明 |
| --- | --- |
| **Chat** | 普通对话 + 图片生成 |
| **Agent** | 编程工具（read_file / write_file / edit_file / delete_file / list_files / glob / grep + 隔离沙箱 bash）+ 联网搜索 + 图片生成 |

### 图片生成

- 模型调用 `generate_image` 工具（OpenAI 兼容 `images/generations` 接口），支持尺寸、负面提示词，结果自动下载保存到会话 `images/` 目录并实时推送卡片
- 适配两家图片生成服务商：**硅基流动**（如 `Kwai-Kolors/Kolors`）与**阿里云百炼**（如 `wanx-v1`；`qwen-image` 系列走原生多模态接口；DashScope 自动把尺寸参数转为其要求的 `1024*1024` 星号格式）
- 产物以缩略图卡片展示，点击可在右侧面板预览

### 工具调用兼容性（文本 JSON 兜底）

部分模型/API 不支持 function calling，会把工具参数以**文本 JSON 形式**输出（如 `{"prompt": "..."}`）而非真正发起调用。应用会自动识别这类回复并**代为执行**：

| 模型输出的 JSON | 识别为 |
| --- | --- |
| 含 `prompt`（Chat / Agent 模式） | `generate_image` |
| 含 `command`（Agent 模式） | `bash` |
| 含 `path` + `old_string` | `edit_file` |
| 含 `path` + `content` | `write_file` |
| 仅含 `path` | `read_file`（只读安全） |
| 含 `pattern` | `grep` |
| 含 `dir` | `list_files` |

识别严格限定"整体为合法 JSON + 特征字段 + 对应模式的工具集内"，普通文本或非对应模式不触发。系统提示词会引导模型：无法发起函数调用时输出 JSON 参数格式，应用自动执行并回喂结果继续对话。

### 编程工具与操作系统级沙箱（Agent 模式）

- **文件工具**：read_file / write_file / edit_file / delete_file / list_files / glob（支持 `*` `**`）/ grep，工作区为会话 `files/` 目录
- **bash 沙箱**：命令通过 **Windows AppContainer** 运行 —— 会话 `files/` 目录在系统层面（ACL）仅授权该会话容器，子进程**无法访问工作区之外的任何文件**（无论命令怎么写，`C:\...`、messages.db 等一律被 OS 拒绝）；沙箱初始化失败时拒绝执行（fail-closed）；非 Windows 平台无 AppContainer，退化为普通子进程执行（保留权限确认与超时）
- **越界确认**：文件工具/命令尝试访问会话目录之外（绝对路径、`..` 逃逸、盘符/UNC/环境变量路径）时，弹出权限确认对话框（90 秒超时自动拒绝）；**拒绝不终止任务**——拒绝结果作为工具结果回喂模型，模型可改用会话内路径继续
- 命令 60 秒超时终止，输出统一截断保护上下文；删除/清空会话时立即失效待确认的越界请求

### 右侧预览面板

- 点击消息中的链接、搜索卡片、文件产物或图片产物卡片，在右侧面板内预览
- **网页预览**：后端抓取并**消毒 HTML**（剔除 script/iframe/svg/form/video/audio 等危险元素与危险协议）后在 sandbox iframe 中渲染；内置 **SSRF 防护**（拒绝内网/回环地址，重定向逐跳复检，含 IPv4-mapped IPv6）
- **文件预览**：会话目录内文本文件（≤ 2MB），随系统深浅色主题自适应
- 右上角图标开关面板，预览内容可一键在系统浏览器中打开

### 上下文管理

- 上下文容量按模型配置（设置 → 服务商，千 token，默认 128K），自动估算已用（消息 + 思考链 + 工具调用 + 搜索结果 + 附件；文档按实际发送的截断上限 20000 字符估算，避免大文件撑爆）
- **自动压缩**：用量达到 **60%** 时调用模型将早期对话浓缩为摘要（保留最近 6 条消息），之后仅发送摘要 + 近期消息，长对话可持续使用；用户点击停止时跳过压缩
- 用量达 **90%**：提示条「建议开启新对话」（可一键新建或关闭提示）
- 用量**已满（100%）**：提示条升级、输入框禁用，即使尝试发送也会被后端拦截

### 系统托盘与界面

- **系统托盘**：关闭窗口最小化到托盘（不退出）；托盘左键点击显示主界面，菜单「显示主界面 / 退出」；退出时自动取消所有进行中的任务
- **界面布局**：左侧对话列表（分组/重命名/删除）+ 聊天区（消息流 + 输入栏）+ 右侧预览面板
- **输入栏**：模式菜单（Chat / Agent）、附件上传、模型选择、联网搜索开关、深度思考开关、发送/停止
- **设置面板**：左侧垂直选项卡（通用 / 服务商 / 模型选择 / 搜索服务），固定面板尺寸；主题跟随系统/浅色/深色
- **通知与提示**：右上角堆叠 Toast（错误提示等）、错误横幅、权限确认弹窗

## 技术栈

| 层 | 技术 |
| --- | --- |
| 桌面框架 | Tauri v2（WebView2 / WKWebView / WebKitGTK），含系统托盘 |
| 后端 | Rust（Tokio + reqwest 流式 SSE + rusqlite bundled） |
| 前端 | React 18 + TypeScript + Vite |
| Markdown | marked + DOMPurify + highlight.js（按需引入常用语言） |
| 数据库 | SQLite（rusqlite bundled，无外部依赖，WAL 模式） |
| 安全 | Windows AppContainer 沙箱（windows-sys）、HTML 消毒（scraper）、SSRF 防护、路径规范化 |

## 项目结构

```
ChatDeepSeek/
├── README.md
├── package.json              # 前端依赖与脚本
├── vite.config.ts
├── index.html
├── tsconfig.json
├── start.bat                 # 一键启动器（环境检查 / 开发 / 打包 菜单）
├── package.bat               # 一键打包器（便携版 + NSIS 安装包）
├── scripts/
│   ├── package-portable.ps1  # 便携版打包（复制 exe 到 dist-portable/）
│   └── run-release.ps1       # 构建 release 并直接运行
├── src/                      # 前端 (React + TS)
│   ├── main.tsx
│   ├── App.tsx               # 应用根组件与状态管理
│   ├── types.ts              # 共享类型定义
│   ├── api.ts                # Tauri invoke 封装 + chat_event 监听
│   ├── styles.css            # 全部样式 (CSS Variables, 浅色/深色主题)
│   ├── lib/
│   │   └── markdown.ts       # Markdown 渲染 (marked + DOMPurify + hljs + 代码卡片)
│   └── components/
│       ├── icons.tsx         # 内联 SVG 图标集
│       ├── Sidebar.tsx       # 对话列表 (年月分组 / 重命名 / 删除确认)
│       ├── ChatView.tsx      # 聊天区 (消息流 + 上下文提示条 + 搜索/产物卡片)
│       ├── MessageItem.tsx   # 单条消息 (Markdown / 思考折叠 / 复制 / 编辑)
│       ├── DraftMessage      # 流式消息 (ChatView 内: 状态提示 + 思考 + 搜索卡片 + 产物)
│       ├── InputBar.tsx      # 输入栏 (模式/模型/附件/联网/深度思考/发送/停止)
│       ├── SettingsPanel.tsx # 设置面板 (通用 / 服务商 / 模型选择 / 搜索服务)
│       ├── ArtifactCards.tsx # 产物卡片 (图片缩略图 / 文件芯片, LRU 缓存)
│       ├── Markdown.tsx      # Markdown 容器 (代码折叠 / 链接拦截预览)
│       ├── WebPreviewPanel.tsx # 右侧预览面板 (网页 / 文件 / 图片)
│       └── Toast.tsx         # 右上角堆叠通知
└── src-tauri/                # 后端 (Rust)
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── build.rs
    ├── capabilities/default.json
    ├── icons/                # 应用图标
    └── src/
        ├── main.rs
        ├── lib.rs            # Tauri 入口, 命令注册, 托盘, 数据根目录定位
        ├── commands.rs       # Tauri 命令 (会话/消息/设置/附件/网页抓取/权限应答)
        ├── db.rs             # SQLite 初始化 / CRUD / 设置存取 / 上下文估算
        ├── models.rs         # 数据结构 + 服务商/模型解析 + 遗留配置迁移
        ├── llm/
        │   ├── mod.rs        # Agent 主循环 (流式 + 工具调用 + 文本JSON兜底 + 共享系统提示词 + 上下文压缩)
        │   ├── openai.rs     # OpenAI 兼容协议客户端 (流式 + function calling + 超时/停滞保护 + 摘要)
        │   └── search.rs     # 联网搜索 (Tavily + AnySearch + 智能路由 + 兜底切换)
        └── agent/
            ├── tools.rs      # 工具定义与执行 (文件/glob/grep/bash/搜索/图片生成)
            ├── sandbox.rs    # 会话级隔离沙箱 (Windows AppContainer / 非Windows退化)
            └── generate.rs   # 图片生成 (OpenAI 兼容 images/generations + qwen-image 原生接口)
```

## 快速开始

### 环境要求

- Node.js ≥ 18
- Rust ≥ 1.77
- Windows：WebView2 运行时（Win10/11 自带）
- macOS：Xcode Command Line Tools
- Linux：WebKitGTK 4.1 等 Tauri 系统依赖

### 开发运行

```bash
npm install
npm run tauri dev
```

或双击 **`start.bat`**（一键启动器）：自动检查 Node/Rust/WebView2 环境、自动安装依赖，提供菜单：

| 选项 | 说明 |
| --- | --- |
| 1. 快速启动 | 构建 release 版并运行（推荐，可避免 Smart App Control 拦截未签名 exe） |
| 2. 开发模式 | `npm run tauri dev` 热更新调试 |
| 3. 构建安装包 | NSIS 安装包 |
| 4. 构建便携版 | 单 exe，输出到 `dist-portable/` |

### 打包发布

```bash
npm run bundle            # 构建安装包（NSIS，安装时可选安装路径）
npm run bundle:portable   # 构建便携版并复制到 dist-portable/
npm run dev:release       # 构建 release 版并直接运行（不打包）
```

或双击 **`package.bat`**（一键打包器）：自动构建、收集产物到 `dist-portable/`、清理构建中间产物。

产物：

| 产物 | 位置 | 说明 |
| --- | --- | --- |
| 便携版 | `dist-portable/ChatDeepSeek.exe` | 单文件免安装，双击即用；运行环境（WebView2，Win10/11 自带）无需下载 |
| 安装包 | `dist-portable/ChatDeepSeek_0.1.0_x64-setup.exe` | NSIS 安装器，安装时**可选择安装路径**，支持中英文语言选择 |

- **便携版**：`data` 文件夹自动生成在 `ChatDeepSeek.exe` **同目录**
- **安装版**：`data` 文件夹默认生成在**安装目录**中；安装目录不可写时自动回退系统应用数据目录
- 应用内嵌前端资源与运行时依赖，用户无需安装 Node.js / Rust 等任何环境

## 配置指南

点击左下角 **设置** 按钮，打开设置面板（四个选项卡）。

### 1. 通用

- **对话默认**：默认对话模型（在「模型选择」页设置）、新对话默认开启联网搜索 / 深度思考、默认模式
- **外观**：主题（跟随系统 / 浅色 / 深色）
- **数据**：清空所有对话（二次确认，不可恢复）

### 2. 服务商（模型管理）

预置 **DeepSeek 官方** 服务商（OpenAI 兼容协议，`https://api.deepseek.com/v1`），也可自由添加任意 OpenAI 兼容服务商：

| 配置项 | 说明 |
| --- | --- |
| 名称 | 自定义显示名称 |
| API 协议 | 固定 OpenAI 兼容（chat/completions） |
| API Base URL | 如 `https://api.deepseek.com/v1`、`https://api.siliconflow.cn/v1` |
| API Key | 服务商密钥 |

每个服务商下可添加多个模型：

- **模型类型**：文本（无视觉）/ 多模态（视觉，支持图片输入）/ 图片生成
- **上下文容量**：对话/多模态模型可配置（千 token，默认 128K），决定上下文进度与自动压缩时机
- **测试**：按类型测试连通性（文本发消息、图片生成测试图）

> 旧版本（`deepseek` / `gen` 字段配置）的 API Key 与模型会自动迁移为服务商体系。

### 3. 模型选择

为以下用途指定具体模型（服务商 / 模型）：

| 槽位 | 用途 |
| --- | --- |
| 对话模型 | 聊天使用；图片输入需选择「多模态」模型 |
| 图片生成模型 | Chat / Agent 模式中生成图片（适配硅基流动与阿里云百炼） |

### 4. 搜索服务

| 服务 | 用途 | 获取地址 |
| --- | --- | --- |
| Tavily API Key | 日常/简单任务数据搜索 | https://tavily.com |
| AnySearch API Key | 专业垂直领域深度搜索 | https://www.anysearch.com/console/api-keys |

搜索策略：`智能自动选择 / 始终 Tavily / 始终 AnySearch`；每轮搜索结果数 1-20（建议 5）。

### 深度思考

- **OpenAI 兼容协议**：不发送推理强度参数，模型返回的 `reasoning_content` 自动流式展示为「思考过程」
- 点击消息框「深度思考」按钮即开/关

## 对话工作流程

AI 回复采用**流式输出**，并通过 System Prompt 引导模型按照「思考 → 执行 → 分析 → 总结」的流程处理每个问题：

| 阶段 | 说明 | 界面提示 |
| --- | --- | --- |
| 思考 | 拆解用户问题意图，判断需要哪些实时/专业数据 | 「正在思考你的问题…」 |
| 执行 | 调用 `web_search` 工具搜索相关内容（可多轮多关键词） | 「正在使用 Tavily/AnySearch 搜索…已获取 N 条结果」 |
| 分析 | 交叉验证搜索数据与用户问题，提炼核心事实 | 「正在分析 N 条搜索结果，提炼与问题相关的核心事实…」 |
| 总结 | 以「总-分-总」结构回答：先给结论概要，再分点展开并附来源链接，最后总结要点 | 「正在生成回答…」 |

- 开启「联网搜索」时，模型会被明确要求：需要实时信息或专业内容时**主动搜索**，引用搜索结果时附带来源链接 `[来源](url)`
- 未开启「联网搜索」时，模型会被告知无法访问互联网，不会虚构「正在搜索」话术
- Chat / Agent 模式下 System Prompt 附带模式说明，指导工具使用

## 联网搜索实现说明

AI 模型通过 **Tool Calls（Function Calling）** 调用本应用内置的 `web_search` 工具：

```
web_search(query, provider?)
```

- `provider`：`auto`（默认）/ `tavily` / `anysearch`，由模型根据问题性质自主选择；每轮结果数由设置（1-20）控制
- 智能路由（`auto` 模式）：查询涉及财经、医疗、学术、代码、法律等专业领域关键词时自动路由至 AnySearch；日常简单问题使用 Tavily（缺少任一 Key 时自动降级；首选引擎无结果时自动切换另一引擎）
- 搜索过程中，每个结果实时以卡片形式（标题 + 链接 + 摘要）推送至聊天界面
- 多轮工具调用：模型可进行最多 **8 轮**「思考 → 工具调用 → 再回答」循环
- 模型可调用的工具集由会话模式决定：Chat 为联网搜索（可关闭）+ 图片生成；Agent 额外增加文件工具与 bash

## 数据存储

全部数据保存在根目录 `data/` 文件夹：

```
data/
├── json/          # 应用设置（settings.json，含 API Key 与服务商配置）
├── logs/          # 运行日志（app.log，1MB 轮转保留全部；含前端 console 转发与错误上报）
└── sessions/      # 会话数据：每个会话一个项目目录
    └── <会话ID>/
        ├── session.json   # 会话元数据（标题/模型/模式/开关/压缩摘要）
        ├── messages.db    # 会话内容（消息/思考链/工具调用/搜索结果/产物索引）
        ├── files/         # 文件产物（Agent 模式）
        ├── images/        # 图片产物（Chat/Agent 模式）
        └── uploads/       # 用户上传的附件
```

- **运行日志**：`data/logs/app.log` 记录关键运行情况与错误（会话操作、消息发送、工具调用与耗时、图片生成任务、联网搜索、API 错误、前端错误等），按 1MB 轮转保留全部历史，排查问题可直接查看
- **原子写入**：session.json 与 settings.json 均采用「临时文件 + 重命名」写入，崩溃不会损坏配置
- **自动迁移**：旧版单文件 `<会话ID>.json` 自动迁移为会话目录结构
- 数据目录定位：开发模式在项目根目录 `data/`；生产模式（便携版/安装版）在 **exe 同目录** `data/`，目录不可写时自动回退系统应用数据目录

## 常见问题

- **如何使用不同模型？** 设置 → 服务商 中添加服务商与模型，模型选择 中指定用途；当前对话在消息框左侧下拉切换（仅对当前对话生效）
- **模型只回复 JSON 提示词（如 `{"prompt": "..."}`）不生成？** 说明该对话模型不支持 function calling——应用会自动识别文本 JSON 形式的工具参数并**代为执行**生成（Chat / Agent 模式均支持）；支持工具调用的模型则走正常调用路径
- **图片上传失败？** 图片输入需要「多模态（视觉）」模型，请在 设置 → 模型选择 中将对话模型切换为多模态模型；SVG 图片因可能携带脚本而被拒绝
- **搜索无结果？** 确认设置了 Tavily 或 AnySearch 的 API Key，并确认对应服务处于启用状态
- **提示「上下文已满」怎么办？** 点击提示条上的「开启新对话」按钮，或新建对话即可继续使用
- **bash 命令被拒绝？** 命令若可能访问会话目录之外的文件（盘符/UNC 等）需用户确认，拒绝后模型会收到提示并可改用会话内路径继续；沙箱内子进程在系统层面无法访问工作区外任何文件
- **停止生成后内容消失了？** 已生成的内容与思考过程会自动保留为消息；若仍在用旧版本 exe，请用 `package.bat` 重新打包
- **Smart App Control 拦截启动？** 使用 `start.bat` 的选项 1（release 构建）运行，或在 Windows 安全中心关闭 Smart App Control

## 免责声明

- 请遵守 DeepSeek、Tavily、AnySearch、硅基流动、阿里云百炼等服务商各自的服务条款与速率限制
- API Key 保存在本地 `data/json/settings.json` 中，请妥善保管您的设备与数据目录
- AI 生成内容仅供参考，请注意甄别信息真实性
