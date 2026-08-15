import { memo, useEffect, useMemo, useState } from "react";
import type { Artifact, Message, SearchItem, ToolStep } from "../types";
import { renderMarkdown } from "../lib/markdown";
import { Markdown } from "./Markdown";
import { ArtifactCards, useArtifactSrc } from "./ArtifactCards";
import {
  BrainIcon,
  CheckIcon,
  ChevronDownIcon,
  CopyIcon,
  ExternalIcon,
  FileIcon,
  ImageIcon,
  LinkIcon,
  PencilIcon,
  SearchIcon,
  SparkIcon,
  TerminalIcon,
  TrashIcon,
} from "./icons";

function formatTime(ts: number): string {
  const d = new Date(ts);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  if (sameDay) {
    return `${String(d.getHours()).padStart(2, "0")}:${String(
      d.getMinutes()
    ).padStart(2, "0")}`;
  }
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")} ${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes()
  ).padStart(2, "0")}`;
}

/**
 * 用户附件图片：临时消息（data URL）直接显示；
 * 持久化消息（相对路径 uploads/xxx）经 getArtifactAbsPath 解析为绝对路径，
 * 否则 convertFileSrc 相对路径会因数据根目录与 CWD 不一致而裂图。
 */
function AttachmentImage({
  convId,
  path,
  name,
}: {
  convId: number;
  path: string;
  name: string;
}) {
  const isData = path.startsWith("data:");
  const src = useArtifactSrc(convId, path);
  if (isData || src) {
    return (
      <img
        className="msg-attachment-img"
        src={isData ? path : (src as string)}
        alt={name}
        title={name}
      />
    );
  }
  return (
    <div className="msg-attachment-img placeholder" title={name}>
      <LinkIcon size={12} />
    </div>
  );
}

function formatDuration(ms: number): string {
  if (ms <= 0) return "";
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  return `${s < 10 ? s.toFixed(1) : Math.round(s)}s`;
}

/** 步骤图标：按类型 / 工具名映射 */
function StepIcon({ step }: { step: ToolStep }) {
  const size = 13;
  if (step.kind === "reasoning") return <BrainIcon size={size} />;
  if (step.kind === "search") return <SearchIcon size={size} />;
  if (step.kind === "image") return <ImageIcon size={size} />;
  switch (step.tool) {
    case "bash":
      return <TerminalIcon size={size} />;
    case "write_file":
    case "edit_file":
      return <PencilIcon size={size} />;
    case "delete_file":
      return <TrashIcon size={size} />;
    default:
      return <FileIcon size={size} />;
  }
}

/**
 * 执行过程时间线：深度思考与各次工具调用按发生顺序展示，
 * 思考步骤可展开完整思考内容，搜索步骤可展开来源卡片。
 * 历史消息与流式 Draft 共用同一实现。
 */
export function StepsTimeline({
  steps,
  reasoning,
  onOpenLink,
  defaultOpen = false,
  expandReasoning = false,
}: {
  steps: ToolStep[];
  reasoning: string;
  onOpenLink: (url: string) => void;
  /** 初始是否展开（流式期间默认展开以展示进度） */
  defaultOpen?: boolean;
  /** 受控：思考步骤详情是否展开（流式时正文开始前自动展开） */
  expandReasoning?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const [detail, setDetail] = useState<string | null>(null);

  useEffect(() => {
    // 流式：正文开始前展示思考详情，正文开始后自动收起
    setDetail(expandReasoning ? "reasoning" : (d) => (d === "reasoning" ? null : d));
  }, [expandReasoning]);

  if (steps.length === 0) return null;
  const totalMs = steps.reduce((a, s) => a + (s.duration_ms || 0), 0);
  const total = formatDuration(totalMs);

  return (
    <div className={`steps-timeline${open ? " open" : ""}`}>
      <button
        className="timeline-toggle"
        onClick={() => setOpen((v) => !v)}
        title="展开/收起执行过程"
      >
        <ChevronDownIcon size={13} />
        <SparkIcon size={12} />
        执行过程 · {steps.length} 步{total ? ` · ${total}` : ""}
      </button>
      {open && (
        <ol className="timeline-list">
          {steps.map((s, i) => {
            const key = s.kind === "reasoning" ? "reasoning" : String(i);
            const hasDetail =
              s.kind === "reasoning" ? !!reasoning : s.items.length > 0;
            const isDetailOpen = detail === key;
            return (
              <li key={key} className={`timeline-step kind-${s.kind}`}>
                <button
                  className={`timeline-step-head${hasDetail ? " clickable" : ""}`}
                  onClick={
                    hasDetail
                      ? () => setDetail(isDetailOpen ? null : key)
                      : undefined
                  }
                >
                  <span className="timeline-step-icon">
                    <StepIcon step={s} />
                  </span>
                  <span className="timeline-step-label">{s.label}</span>
                  {s.duration_ms > 0 && (
                    <span className="timeline-dur">
                      {formatDuration(s.duration_ms)}
                    </span>
                  )}
                  {hasDetail && (
                    <ChevronDownIcon
                      size={12}
                      className={`timeline-step-chevron${isDetailOpen ? " open" : ""}`}
                    />
                  )}
                </button>
                {isDetailOpen && s.kind === "reasoning" && (
                  <div className="timeline-detail thinking-text">{reasoning}</div>
                )}
                {isDetailOpen && s.kind === "search" && (
                  <div className="timeline-detail">
                    <div className="search-cards-row">
                      {s.items.map((it, j) => (
                        <button
                          key={`${it.url}-${j}`}
                          className="search-card"
                          onClick={() => onOpenLink(it.url)}
                          title={it.url}
                        >
                          <span className="search-card-title">
                            {it.title || it.url}
                          </span>
                          <span className="search-card-url">
                            {it.url}
                            <ExternalIcon size={11} />
                          </span>
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}

/** 旧消息兼容：无 steps 时由 reasoning / search_results 合成时间线 */
export function synthesizeSteps(
  reasoning: string,
  searchItems: SearchItem[]
): ToolStep[] {
  const steps: ToolStep[] = [];
  if (reasoning) {
    steps.push({
      kind: "reasoning",
      label: "深度思考",
      tool: "",
      duration_ms: 0,
      items: [],
    });
  }
  if (searchItems.length > 0) {
    steps.push({
      kind: "search",
      label: `联网搜索 · ${searchItems.length} 条结果`,
      tool: "web_search",
      duration_ms: 0,
      items: searchItems,
    });
  }
  return steps;
}

function MessageItemInner({
  message,
  onOpenLink,
  onOpenArtifact,
  onOpenFile,
  onEditMessage,
}: {
  message: Message;
  onOpenLink: (url: string) => void;
  onOpenArtifact: (convId: number, artifact: Artifact) => void;
  onOpenFile: (convId: number, path: string, title: string) => void;
  onEditMessage?: (message: Message) => void;
}) {
  const [copied, setCopied] = useState(false);
  const isUser = message.role === "user";

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(message.content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* ignore */
    }
  };

  if (isUser) {
    return (
      <div className="msg user">
        <div className="msg-user-bubble">
          {message.attachments.length > 0 && (
            <div className="msg-attachments">
              {message.attachments.map((a, i) => (
                <div key={`${a.path}-${i}`} className="msg-attachment">
                  {a.kind === "image" ? (
                    <AttachmentImage
                      convId={message.conversation_id}
                      path={a.path}
                      name={a.name}
                    />
                  ) : (
                    <span className="msg-attachment-doc">
                      <LinkIcon size={12} />
                      <span className="msg-attachment-doc-name" title={a.name}>
                        {a.name}
                      </span>
                    </span>
                  )}
                </div>
              ))}
            </div>
          )}
          {message.content}
        </div>
        <div className="msg-meta">
          <span>{formatTime(message.created_at)}</span>
          <button className="msg-action" onClick={copy} title="复制内容">
            {copied ? <CheckIcon size={13} /> : <CopyIcon size={13} />}
          </button>
          {onEditMessage && (
            <button
              className="msg-action"
              onClick={() => onEditMessage(message)}
              title="编辑该消息并重新发送"
            >
              <PencilIcon size={13} />
            </button>
          )}
        </div>
      </div>
    );
  }

  // 历史消息 Markdown 渲染结果缓存：流式期间消息列表重渲染时避免全量重复解析
  const contentHtml = useMemo(
    () => renderMarkdown(message.content),
    [message.content]
  );

  // 时间线步骤：优先使用持久化的 steps，旧消息（升级前）由字段合成
  const steps = useMemo(
    () =>
      message.steps.length
        ? message.steps
        : synthesizeSteps(message.reasoning, message.search_results),
    [message.steps, message.reasoning, message.search_results]
  );

  return (
    <div className="msg assistant">
      <div className="msg-avatar">DS</div>
      <div className="msg-body">
        <StepsTimeline
          steps={steps}
          reasoning={message.reasoning}
          onOpenLink={onOpenLink}
        />
        {message.content && (
          <Markdown
            className="markdown-body"
            html={contentHtml}
            onOpenLink={onOpenLink}
          />
        )}
        {!message.content && !message.artifacts.length && (
          <div className="msg-empty">（模型未返回内容）</div>
        )}
        <ArtifactCards
          artifacts={message.artifacts}
          onOpenArtifact={onOpenArtifact}
          onOpenFile={onOpenFile}
          convId={message.conversation_id}
        />
        <div className="msg-meta">
          <span>{formatTime(message.created_at)}</span>
          <button className="msg-action" onClick={copy} title="复制内容">
            {copied ? <CheckIcon size={13} /> : <CopyIcon size={13} />}
          </button>
        </div>
      </div>
    </div>
  );
}

/** memo 包裹：流式渲染期间仅重渲染变化的消息（依赖 props 引用稳定，见 App 层） */
export const MessageItem = memo(MessageItemInner);
