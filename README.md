# ChatDeepSeek

一款使用 **Tauri v2 + Rust + SQLite + React** 构建的桌面端 AI 对话聊天应用。以「精简、高效、高质量视觉体验」为设计目标，专为与 DeepSeek 等大语言模型的日常对话而打造。

## 功能特性

- **对话隔离**：每个对话拥有独立、隔离的上下文，互不干扰；对话按「年-月」时间顺序自动分组显示
- **DeepSeek 双模型**：通过 **Anthropic (Messages API) 协议** 调用 DeepSeek，一个官方 API Key 即可使用 `deepseek-v4-flash` 与 `deepseek-v4-pro` 两个模型；模型在**消息发送框**中随时切换
- **深度思考模式**：可开关，并按模型选择推理强度
  - `deepseek-v4-flash`：无 / 低(low) / 高(high) / 最大(max)
  - `deepseek-v4-pro`：无 / 高(high) / 最大(max)
- **联网搜索（Function Calling）**：通过 Tool Calls 让模型自主决定是否搜索、搜索什么，无需手动干预
  - **Tavily**：适合简单日常任务、事实类数据检索，快速轻量
  - **AnySearch**：适合专业垂直领域内容搜索（财经、医疗、学术、代码、法律等），支持 `tag` 子域能力
  - 搜索结果以「网页标题 + 链接」小型卡片形式展示，点击卡片直接跳转网页
  - 支持智能路由：模型可指定 `provider`，未指定时按查询内容自动选择合适引擎
- **Markdown 渲染**：AI 回复实时渲染 Markdown（代码高亮、表格、列表等），流式输出
- **精简界面**：
  - 左侧：对话列表 + 「开启新对话」按钮，底部设置按钮
  - 右侧：聊天区 + 消息输入栏（**AI 模型选择**、输入框、发送、联网搜索开关、深度思考开关与强度选择、停止生成）
  - 设置面板：左侧垂直选项卡（通用 / AI 模型 / 搜索服务），**固定面板大小**，切换选项卡时面板尺寸不变
- **本地数据规整存储**：全部数据保存在项目根目录的 `data/` 文件夹，结构清晰：
  - `data/json/` —— API Key 与应用设置（`settings.json`）
  - `data/db/` —— SQLite 数据库（会话上下文记忆）
  - `data/sessions/` —— 会话数据（每个会话一个 JSON 文件）
  - 开发模式下位于项目根目录；正式安装运行时若根目录不可写，自动回退至系统应用数据目录
- **1M 上下文管理**：模型上下文总量 1M tokens，自动估算各会话用量：
  - 用量达 **90%** 时提示「建议开启新对话」
  - 用量**已满**时输入框禁用并提示「上下文已满，请新开会话」，发送消息会被拦截并提示

## 技术栈

| 层 | 技术 |
| --- | --- |
| 桌面框架 | Tauri v2（WebView2 / WKWebView / WebKitGTK） |
| 后端 | Rust（Tokio + reqwest 流式 SSE + rusqlite） |
| 前端 | React 18 + TypeScript + Vite |
| Markdown | marked + DOMPurify + highlight.js |
| 数据库 | SQLite（rusqlite bundled，无外部依赖） |

## 项目结构

```
ChatDeepSeek/
├── README.md
├── package.json              # 前端依赖与脚本
├── vite.config.ts
├── index.html
├── src/                      # 前端 (React + TS)
│   ├── main.tsx
│   ├── App.tsx               # 应用根组件与状态管理
│   ├── types.ts              # 共享类型定义
│   ├── api.ts                # Tauri invoke 封装 + 事件监听
│   ├── styles.css            # 全部样式 (CSS Variables, 浅色/深色主题)
│   ├── lib/
│   │   └── markdown.ts       # Markdown 渲染 (marked + DOMPurify + hljs)
│   └── components/
│       ├── icons.tsx         # 内联 SVG 图标集
│       ├── Sidebar.tsx       # 对话列表 (年月分组) + 新建对话 + 设置入口
│       ├── ChatView.tsx      # 聊天区 (消息流 + 搜索卡片 + 模型选择)
│       ├── MessageItem.tsx   # 单条消息 (Markdown 渲染 / 思考折叠块 / 复制)
│       ├── InputBar.tsx      # 输入栏 (联网/深度思考开关 + 强度选择 + 发送/停止)
│       └── SettingsPanel.tsx # 设置面板 (左侧垂直选项卡, 固定尺寸)
└── src-tauri/                # 后端 (Rust)
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── capabilities/default.json
    ├── icons/                # 应用图标
    └── src/
        ├── main.rs
        ├── lib.rs            # Tauri 入口, 命令注册, 全局状态
        ├── db.rs             # SQLite 初始化 / CRUD / 设置存取
        ├── models.rs         # 数据结构
        ├── llm/
        │   ├── mod.rs        # 对话主循环 (流式 + 工具调用 + 事件发射)
        │   ├── anthropic.rs  # Anthropic Messages API 客户端 (调用 DeepSeek)
        │   └── search.rs     # 联网搜索工具 (Tavily + AnySearch + 智能路由)
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

### 打包发布

```bash
npm run bundle            # 构建安装包（NSIS，安装时可选安装路径）
npm run bundle:portable   # 构建便携版并复制到 dist-portable/
```

产物：

| 产物 | 位置 | 说明 |
| --- | --- | --- |
| 便携版 | `dist-portable/ChatDeepSeek.exe` | 单文件免安装，双击即用；运行环境（WebView2，Win10/11 自带）无需下载 |
| 安装包 | `src-tauri/target/release/bundle/nsis/ChatDeepSeek_0.1.0_x64-setup.exe` | NSIS 安装器，安装时**可选择安装路径**，支持中英文语言选择 |

- **便携版**：`data` 文件夹自动生成在 `ChatDeepSeek.exe` **同目录**
- **安装版**：`data` 文件夹默认生成在**安装目录**中；安装目录不可写时自动回退系统应用数据目录
- 应用内嵌前端资源与运行时依赖，用户无需安装 Node.js / Rust 等任何环境

## API 配置

在应用内点击左下角 **设置** 按钮，配置以下服务：

### 1. AI 模型（DeepSeek API）

| 配置项 | 说明 |
| --- | --- |
| DeepSeek API Key | 从 [DeepSeek Platform](https://platform.deepseek.com/) 获取，**一个 Key 即可使用全部模型** |

应用通过 **Anthropic (Messages API) 协议**（`https://api.deepseek.com/anthropic`）调用 DeepSeek 模型。模型 ID 固定为官方 `deepseek-v4-flash` 与 `deepseek-v4-pro`，在**消息发送框左侧**下拉切换，切换后仅对当前对话生效。

### 2. 联网搜索服务

| 服务 | 用途 | 获取地址 |
| --- | --- | --- |
| Tavily API Key | 日常/简单任务数据搜索 | https://tavily.com |
| AnySearch API Key | 专业垂直领域深度搜索 | https://www.anysearch.com/console/api-keys |

搜索策略可设置：`模型自动选择 / 始终 Tavily / 始终 AnySearch`。

## 深度思考与推理强度

依据 [DeepSeek 思考模式文档](https://api-docs.deepseek.com/zh-cn/guides/thinking_mode)，通过 **Anthropic 格式** 参数控制：

- **开启**：`reasoning.effort` + `output_config.effort`（`low / high / max`）
- **关闭**：`reasoning.effort = "none"`

点击消息框「深度思考」按钮即开/关；开启后按钮旁显示推理强度选项（Flash 支持 低/高/最大，Pro 支持 高/最大），点击即切换。模型切换到 Pro 时，不支持的强度（低/无）会自动调整为「高」。

服务端 effort 映射（来自官方文档，Pro 模型的实际映射 2026 年 8 月初更新）：

| 请求传入 | flash 实际映射 | pro 实际映射 |
| --- | --- | --- |
| low | low | high |
| high | high | high |
| max | max | max |

> 注意：思考模式下 `temperature`、`top_p` 等采样参数不生效（为兼容已有软件设置参数不会报错）。

## 对话工作流程与阶段提示

AI 回复采用**流式输出**，并通过 System Prompt 引导模型按照「思考 → 执行 → 分析 → 总结」的流程处理每个问题：

| 阶段 | 说明 | 界面提示 |
| --- | --- | --- |
| 思考 | 拆解用户问题意图，判断需要哪些实时/专业数据 | 「正在思考你的问题…」 |
| 执行 | 调用 `web_search` 工具搜索相关内容（可多轮多关键词） | 「正在使用 Tavily/AnySearch 搜索…已获取 N 条结果」 |
| 分析 | 交叉验证搜索数据与用户问题，提炼核心事实 | 「正在分析 N 条搜索结果，提炼与问题相关的核心事实…」 |
| 总结 | 以「总-分-总」结构回答：先给结论概要，再分点展开并附来源链接，最后总结要点 | 「正在生成回答…」 |

- 开启「联网搜索」时，模型会被明确要求：需要实时信息或专业内容时**主动搜索**，引用搜索结果时附带来源链接 `[来源](url)`
- 未开启「联网搜索」时，同样遵循「先思考、再以总-分-总结构组织回答」的规范
- 阶段提示随流式过程实时切换，搜索到的结果同时以网页卡片形式展示，可点击跳转

## 联网搜索实现说明

AI 模型通过 **Tool Calls（Function Calling）** 调用本应用内置的 `web_search` 工具：

```
web_search(query, provider?, max_results?, search_depth?)
```

- `provider`：`auto`（默认）/ `tavily` / `anysearch`，由模型根据问题性质自主选择
- 智能路由（`auto` 模式）：查询涉及财经、医疗、学术、代码、法律等专业领域关键词时自动路由至 AnySearch；日常简单问题使用 Tavily（缺少任一 Key 时自动降级）
- AnySearch 支持 `tag` 子域能力（如 `finance.quote`、`code.doc`、`academic.search`），查询命中专业场景时自动附加
- 搜索过程中，每个结果实时以卡片形式（标题 + 链接 + 摘要）推送至聊天界面，模型最终回答前已可见
- 多轮工具调用：模型可进行最多 6 轮「思考 → 搜索 → 再回答」循环，思考内容在携带 `tools` 参数时会完整回传给 API（DeepSeek 要求）

## 数据存储

全部数据保存在根目录 `data/` 文件夹：

```
data/
├── json/          # API Key、应用设置（settings.json）
├── db/            # SQLite 数据库：会话上下文记忆（messages 表）
└── sessions/      # 会话数据：每个会话一个 <会话ID>.json
```

## 上下文管理（1M）

- 模型上下文总量 **1,000,000 tokens**，应用按会话估算已用上下文（内容 + 思考链 + 工具调用/搜索结果）
- 用量达到 **90%**：聊天区显示提示条「当前会话上下文已使用 X%（N / 100.0 万），建议开启新对话」，可一键开启新对话或手动关闭提示
- 用量**已满（100%）**：提示条升级为「上下文已满」，输入框禁用（占位符提示「上下文已满，请新开会话」），即使尝试发送也会被后端拦截并提示「上下文已满，请新开会话」

## 常见问题

- **如何使用不同模型？** 在消息发送框左侧下拉选择 DeepSeek V4 Flash / DeepSeek V4 Pro，选择仅对当前对话生效
- **搜索无结果？** 确认设置了 Tavily 或 AnySearch 的 API Key，并确认对应服务处于启用状态
- **深度思考强度选项为何不同？** `deepseek-v4-flash` 支持 低/高/最大，`deepseek-v4-pro` 支持 高/最大，与模型实际支持保持一致
- **深度思考开启后如何关闭？** 再次点击「深度思考」按钮即可关闭，推理强度仅在开启时显示
- **提示「上下文已满」怎么办？** 点击提示条上的「开启新对话」按钮，或新建对话即可继续使用

## 免责声明

- 请遵守 DeepSeek、Tavily、AnySearch 各自的服务条款与速率限制
- API Key 保存在本地 SQLite 数据库中，请妥善保管您的设备
