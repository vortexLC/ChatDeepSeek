import { useDeferredValue, useEffect, useRef, useState } from "react";
import type {
  AgentMode,
  Artifact,
  ChatDraft,
  ContextStatus,
  Conversation,
  EditTarget,
  Effort,
  Message,
  ModelOption,
} from "../types";
import { MessageItem } from "./MessageItem";
import { InputBar } from "./InputBar";
import { Markdown } from "./Markdown";
import { renderMarkdown } from "../lib/markdown";
import {
  AlertIcon,
  ChevronDownIcon,
  GlobeIcon,
  ImageIcon,
  LinkIcon,
  SearchIcon,
  SparkIcon,
  VideoIcon,
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

function effortOptionsForFamily(family: string): Effort[] {
  if (family === "pro") return ["high", "max"];
  return ["low", "high", "max"];
}

function ArtifactCards({
  artifacts,
  onOpenArtifact,
  onOpenFile,
  convId,
}: {
  artifacts: Artifact[];
  onOpenArtifact: (convId: number, artifact: Artifact) => void;
  onOpenFile: (convId: number, path: string, title: string) => void;
  convId: number;
}) {
  if (!artifacts || artifacts.length === 0) return null;
  return (
    <div className="artifact-list">
      {artifacts.map((a, i) => (
        <button
          key={`${a.path}-${i}`}
          className={`artifact-card ${a.kind}`}
          onClick={() =>
            a.kind === "file"
              ? onOpenFile(convId, a.path, a.name)
              : onOpenArtifact(convId, a)
          }
          title={a.path}
        >
          {a.kind === "image" ? (
            <ImageIcon size={15} />
          ) : a.kind === "video" ? (
            <VideoIcon size={15} />
          ) : (
            <LinkIcon size={13} />
          )}
          <span className="artifact-name">{a.name}</span>
          <span className="artifact-note">
            {a.kind === "image"
              ? "图片"
              : a.kind === "video"
                ? "视频"
                : "文件"}
            {a.size > 0 ? ` · ${(a.size / 1024).toFixed(0)} KB` : ""}
          </span>
        </button>
      ))}
    </div>
  );
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
  // 流式渲染节流：高优先级渲染走低优先级延迟值，避免每次 token 全量重渲染长 Markdown
  const deferredContent = useDeferredValue(draft.content);

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
        <ArtifactCards
          artifacts={draft.artifacts}
          onOpenArtifact={onOpenArtifact}
          onOpenFile={onOpenFile}
          convId={convId}
        />
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
        {draft.reasoning && showReasoning && (
          <div className="thinking-block open">
            <button
              className="thinking-toggle"
              onClick={() => setShowReasoning((v) => !v)}
            >
              <ChevronDownIcon size={13} />
              <span>深度思考过程</span>
            </button>
            <div className="thinking-content">{draft.reasoning}</div>
          </div>
        )}
        {deferredContent && (
          <Markdown
            className="markdown-body streaming"
            html={renderMarkdown(deferredContent) + `<span class="cursor">▍</span>`}
            onOpenLink={onOpenLink}
          />
        )}
        {draft.error && <div className="draft-error">{draft.error}</div>}
      </div>
    </div>
  );
}

interface ChatViewProps {
  conversation: Conversation | null;
  messages: Message[];
  loadingMessages: boolean;
  draft: ChatDraft | null;
  context: ContextStatus | null;
  onNewConversation: () => void;
  previewOpen: boolean;
  onTogglePreview: () => void;
  modelOptions: ModelOption[];
  onSelectModel: (option: ModelOption) => void;
  onToggleWebSearch: () => void;
  onToggleDeepThink: () => void;
  onSetEffort: (e: Effort) => void;
  onSetMode: (mode: AgentMode) => void;
  editTarget: EditTarget | null;
  onCancelEdit: () => void;
  onSend: (content: string) => void;
  onSendEdit: (content: string) => void;
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
  context,
  onNewConversation,
  previewOpen,
  onTogglePreview,
  modelOptions,
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
  const endRef = useRef<HTMLDivElement>(null);
  const [contextDismissed, setContextDismissed] = useState(false);

  useEffect(() => {
    setContextDismissed(false);
  }, [conversation?.id]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [
    messages,
    draft?.reasoning,
    draft?.content,
    draft?.status,
    draft?.searchItems,
    draft?.artifacts,
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
                  onClick={() => onSend(s)}
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
        <div ref={endRef} />
      </div>

      {context && !contextDismissed && (context.full || context.near_full) && (
        <div className={`context-bar${context.full ? " full" : ""}`}>
          <AlertIcon size={15} />
          <span className="context-bar-text">
            {context.full
              ? `当前会话上下文已满（${formatTokens(context.used_tokens)} / ${formatTokens(context.total_tokens)}），无法继续发送消息，请开启新对话`
              : `当前会话上下文已使用 ${Math.round(context.percent * 100)}%（${formatTokens(context.used_tokens)} / ${formatTokens(context.total_tokens)}），建议开启新对话`}
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

      <InputBar
        disabled={false}
        contextFull={context?.full ?? false}
        streaming={streaming}
        editTarget={editTarget}
        onCancelEdit={onCancelEdit}
        mode={conversation.mode}
        onSetMode={onSetMode}
        modelOptions={modelOptions}
        currentModel={conversation.model}
        onSelectModel={onSelectModel}
        webSearch={conversation.web_search}
        deepThink={conversation.deep_think}
        effort={conversation.effort}
        effortOptions={effortOptionsForFamily(
          conversation.model.includes("flash")
            ? "flash"
            : conversation.model.includes("pro")
              ? "pro"
              : "flash"
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
