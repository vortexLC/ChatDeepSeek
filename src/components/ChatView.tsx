import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import type {
  AgentMode,
  Artifact,
  ChatDraft,
  ContextStatus,
  Conversation,
  EditTarget,
  Effort,
  Job,
  Message,
  ModelOption,
  UploadAttachment,
} from "../types";
import { MessageItem } from "./MessageItem";
import { InputBar } from "./InputBar";
import { Markdown } from "./Markdown";
import { ArtifactCards } from "./ArtifactCards";
import { JobCards } from "./JobCard";
import { renderMarkdown } from "../lib/markdown";
import {
  AlertIcon,
  ChevronDownIcon,
  GlobeIcon,
  LinkIcon,
  SearchIcon,
  SparkIcon,
  XIcon,
} from "./icons";

function formatTokens(n: number): string {
  if (n >= 10000) {
    return `${(n / 10000).toFixed(1)} 万`;
  }
  return String(n);
}

const SUGGESTIONS = [
  "帮我总结《三体》的核心思想",
  "写一份简洁的周报模板",
  "解释什么是大语言模型的注意力机制",
  "推荐 3 个适合新手的编程项目",
];

function effortOptionsForProtocol(protocol?: string): Effort[] {
  // 仅 Anthropic 协议会携带推理强度（output_config.effort）；
  // OpenAI 兼容协议不传该参数，返回空数组以隐藏强度选择
  if (protocol === "anthropic") return ["low", "high", "max"];
  return [];
}

function DraftMessage({
  draft,
  onOpenLink,
  onOpenArtifact,
  onOpenFile,
  convId,
}: {
  draft: ChatDraft;
  onOpenLink: (url: string) => void;
  onOpenArtifact: (convId: number, artifact: Artifact) => void;
  onOpenFile: (convId: number, path: string, title: string) => void;
  convId: number;
}) {
  const [showReasoning, setShowReasoning] = useState(false);
  // 流式渲染节流：高优先级渲染走低优先级延迟值，避免每次 token 全量重渲染长 Markdown；
  // 渲染结果按延迟值缓存，未变化期间不重复解析
  const deferredContent = useDeferredValue(draft.content);
  const contentHtml = useMemo(
    () => renderMarkdown(deferredContent) + `<span class="cursor">▍</span>`,
    [deferredContent]
  );

  useEffect(() => {
    if (draft.reasoning) setShowReasoning(true);
  }, [draft.reasoning]);

  const statusChip =
    draft.status === "idle" || (draft.status === "answering" && draft.content.length > 0)
      ? null
      : draft.status === "thinking" ? (
          <span className="status-chip thinking">
            <span className="dot-pulse" />
            正在思考你的问题…
          </span>
        ) : draft.status === "searching" ? (
          <span className="status-chip searching">
            <SearchIcon size={12} />
            {draft.searchProvider === "tavily"
              ? "正在使用 Tavily 搜索相关网页…"
              : "正在使用 AnySearch 搜索专业内容…"}
            {draft.searchItems.length > 0 && (
              <em className="chip-count">已获取 {draft.searchItems.length} 条结果</em>
            )}
          </span>
        ) : draft.status === "analyzing" ? (
          <span className="status-chip analyzing">
            <SparkIcon size={12} />
            正在分析
            {draft.searchItems.length > 0
              ? ` ${draft.searchItems.length} 条搜索结果`
              : "搜索结果"}
            ，提炼与问题相关的核心事实…
          </span>
        ) : draft.status === "generating" ? (
          <span className="status-chip generating">
            <SparkIcon size={12} />
            正在生成（图片/视频生成可能需要几分钟）…
          </span>
        ) : (
          <span className="status-chip answering">
            <SparkIcon size={12} />
            正在生成回答…
          </span>
        );

  return (
    <div className="msg assistant">
      <div className="msg-avatar">DS</div>
      <div className="msg-body">
        {statusChip}
        {draft.reasoning && showReasoning && (
          <div className="thinking-block open">
            <button
              className="thinking-toggle"
              onClick={() => setShowReasoning((v) => !v)}
            >
              <ChevronDownIcon size={13} />
              <span>思考过程</span>
            </button>
            <div className="thinking-content">{draft.reasoning}</div>
          </div>
        )}
        {deferredContent && (
          <Markdown
            className="markdown-body streaming"
            html={contentHtml}
            onOpenLink={onOpenLink}
          />
        )}
        {draft.searchItems.length > 0 && (
          <div className="search-cards">
            <div className="search-cards-label">
              <GlobeIcon size={12} />
              联网搜索结果
            </div>
            <div className="search-cards-row">
              {draft.searchItems.map((it, i) => (
                <button
                  key={`${it.url}-${i}`}
                  className="search-card"
                  onClick={() => onOpenLink(it.url)}
                  title={it.url}
                >
                  <span className="search-card-title">{it.title || it.url}</span>
                  <span className="search-card-url">{it.url}</span>
                </button>
              ))}
            </div>
          </div>
        )}
        <ArtifactCards
          artifacts={draft.artifacts}
          onOpenArtifact={onOpenArtifact}
          onOpenFile={onOpenFile}
          convId={convId}
        />
      </div>
    </div>
  );
}

interface ChatViewProps {
  conversation: Conversation | null;
  messages: Message[];
  loadingMessages: boolean;
  draft: ChatDraft | null;
  jobs: Job[];
  context: ContextStatus | null;
  onNewConversation: () => void;
  previewOpen: boolean;
  onTogglePreview: () => void;
  modelOptions: ModelOption[];
  defaultModel: string;
  onSelectModel: (option: ModelOption) => void;
  onToggleWebSearch: () => void;
  onToggleDeepThink: () => void;
  onSetEffort: (e: Effort) => void;
  onSetMode: (mode: AgentMode) => void;
  editTarget: EditTarget | null;
  onCancelEdit: () => void;
  onSend: (content: string, attachments: UploadAttachment[]) => Promise<boolean>;
  onSendEdit: (content: string, attachments: UploadAttachment[]) => Promise<boolean>;
  onStop: () => void;
  onOpenLink: (url: string) => void;
  onOpenFile: (convId: number, path: string, title: string) => void;
  onOpenArtifact: (convId: number, artifact: Artifact) => void;
  onEditMessage: (message: Message) => void;
}

export function ChatView({
  conversation,
  messages,
  loadingMessages,
  draft,
  jobs,
  context,
  onNewConversation,
  previewOpen,
  onTogglePreview,
  modelOptions,
  defaultModel,
  onSelectModel,
  onToggleWebSearch,
  onToggleDeepThink,
  onSetEffort,
  onSetMode,
  editTarget,
  onCancelEdit,
  onSend,
  onSendEdit,
  onStop,
  onOpenLink,
  onOpenFile,
  onOpenArtifact,
  onEditMessage,
}: ChatViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [contextDismissed, setContextDismissed] = useState(false);

  useEffect(() => {
    setContextDismissed(false);
  }, [conversation?.id]);

  // 内容增长时：仅当用户处于底部附近才跟随滚动到底部；
  // 用户上翻阅读历史/思考过程时完全不干预，保证可以自由滚动查看
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    if (atBottom) {
      el.scrollTop = el.scrollHeight;
    }
  }, [
    messages,
    draft?.reasoning,
    draft?.content,
    draft?.status,
    draft?.searchItems,
    draft?.artifacts,
    jobs.length,
  ]);

  if (!conversation) {
    return (
      <div className="chat-view">
        <div className="chat-empty">
          <div className="chat-empty-logo">
            <SparkIcon size={22} />
          </div>
          <h1>ChatDeepSeek</h1>
          <p>开启新对话，与 DeepSeek 深度交流</p>
        </div>
      </div>
    );
  }

  const streaming = !!draft;

  // 有效对话模型：会话内已选且仍有效的模型优先，否则回退到默认模型，
  // 使发送框显示的模型与设置 → 模型选择 保持一致
  const effectiveModel = modelOptions.some((o) => o.model === conversation.model)
    ? conversation.model
    : defaultModel;

  return (
    <div className="chat-view">
      <header className="chat-header">
        <div className="chat-header-title" title={conversation.title}>
          {conversation.title}
        </div>
        <div className="chat-header-right">
          {streaming && (
            <span className="header-streaming">
              <span className="dot-pulse" />
            </span>
          )}
          <button
            className={`header-preview-btn${previewOpen ? " active" : ""}`}
            onClick={onTogglePreview}
            title="网页预览面板"
          >
            <LinkIcon size={15} />
          </button>
        </div>
      </header>

      <div className="messages" ref={scrollRef}>
        {loadingMessages && messages.length === 0 && (
          <div className="messages-loading">加载中…</div>
        )}
        {!loadingMessages && messages.length === 0 && !draft && (
          <div className="chat-welcome">
            <div className="chat-welcome-title">
              <SparkIcon size={15} />
              你好，我是 DeepSeek
            </div>
            <p>我可以回答你的问题、撰写内容、提供建议。试试下面这些：</p>
            <div className="suggestions">
              {SUGGESTIONS.map((s) => (
                <button
                  key={s}
                  className="suggestion-chip"
                  onClick={() => onSend(s, [])}
                  disabled={!!draft}
                >
                  {s}
                </button>
              ))}
            </div>
          </div>
        )}
        {messages.map((m) => (
          <MessageItem
            key={m.id}
            message={m}
            jobs={jobs}
            onOpenLink={onOpenLink}
            onOpenFile={onOpenFile}
            onOpenArtifact={onOpenArtifact}
            onEditMessage={onEditMessage}
          />
        ))}
        {draft && (
          <DraftMessage
            draft={draft}
            onOpenLink={onOpenLink}
            onOpenArtifact={onOpenArtifact}
            onOpenFile={onOpenFile}
            convId={conversation.id}
          />
        )}
        <JobCards jobs={jobs} />
      </div>

      {context && !contextDismissed && (context.full || context.near_full) && (
        <div className={`context-bar${context.full ? " full" : ""}`}>
          <AlertIcon size={15} />
          <span className="context-bar-text">
            {context.full
              ? `当前会话上下文已满（${formatTokens(context.used_tokens)} / ${formatTokens(context.total_tokens)}）${context.compressed ? "，已自动压缩但仍接近上限" : ""}，无法继续发送消息，请开启新对话`
              : `当前会话上下文已使用 ${Math.round(context.percent * 100)}%（${formatTokens(context.used_tokens)} / ${formatTokens(context.total_tokens)}）${context.compressed ? "，已自动压缩早期对话" : ""}，建议开启新对话`}
          </span>
          <button className="context-bar-action" onClick={onNewConversation}>
            开启新对话
          </button>
          <button
            className="context-bar-close"
            onClick={() => setContextDismissed(true)}
            title="关闭提示"
          >
            <XIcon size={12} />
          </button>
        </div>
      )}
      {context?.compressed && (
        <div className="context-compressed-chip" title="早期对话已自动摘要，不再占用完整上下文">
          <SparkIcon size={11} />
          已自动压缩早期对话
        </div>
      )}

      <InputBar
        resetKey={conversation.id}
        disabled={false}
        contextFull={context?.full ?? false}
        streaming={streaming}
        editTarget={editTarget}
        onCancelEdit={onCancelEdit}
        mode={conversation.mode}
        onSetMode={onSetMode}
        modelOptions={modelOptions}
        currentModel={effectiveModel}
        onSelectModel={onSelectModel}
        webSearch={conversation.web_search}
        deepThink={conversation.deep_think}
        effort={conversation.effort}
        effortOptions={effortOptionsForProtocol(
          modelOptions.find((o) => o.model === effectiveModel)?.protocol
        )}
        onToggleWebSearch={onToggleWebSearch}
        onToggleDeepThink={onToggleDeepThink}
        onSetEffort={onSetEffort}
        onSend={onSend}
        onSendEdit={onSendEdit}
        onStop={onStop}
      />
    </div>
  );
}
