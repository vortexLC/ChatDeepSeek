import { memo, useMemo, useState } from "react";
import type { Artifact, Job, Message, SearchItem } from "../types";
import { renderMarkdown } from "../lib/markdown";
import { Markdown } from "./Markdown";
import { ArtifactCards, useArtifactSrc } from "./ArtifactCards";
import {
  CheckIcon,
  ChevronDownIcon,
  CopyIcon,
  ExternalIcon,
  LinkIcon,
  PencilIcon,
  SearchIcon,
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

function formatDuration(ms: number): string {
  const secs = Math.max(1, Math.floor(ms / 1000));
  return secs >= 60
    ? `${Math.floor(secs / 60)} 分 ${secs % 60} 秒`
    : `${secs} 秒`;
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

function SearchCards({
  items,
  onOpenLink,
}: {
  items: SearchItem[];
  onOpenLink: (url: string) => void;
}) {
  if (items.length === 0) return null;
  return (
    <div className="search-cards">
      <div className="search-cards-label">
        <SearchIcon size={12} />
        联网搜索结果
      </div>
      <div className="search-cards-row">
        {items.map((it, i) => (
          <button
            key={`${it.url}-${i}`}
            className="search-card"
            onClick={() => onOpenLink(it.url)}
            title={it.url}
          >
            <span className="search-card-title">{it.title || it.url}</span>
            <span className="search-card-url">
              {it.url}
              <ExternalIcon size={11} />
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

function MessageItemInner({
  message,
  jobs,
  onOpenLink,
  onOpenArtifact,
  onOpenFile,
  onEditMessage,
}: {
  message: Message;
  jobs?: Job[];
  onOpenLink: (url: string) => void;
  onOpenArtifact: (convId: number, artifact: Artifact) => void;
  onOpenFile: (convId: number, path: string, title: string) => void;
  onEditMessage?: (message: Message) => void;
}) {
  const [copied, setCopied] = useState(false);
  const [showReasoning, setShowReasoning] = useState(false);
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

  // 异步任务完成消息：展示"任务完成"横条（提交时间 / 耗时），明确其归属
  const job = message.job_id != null
    ? jobs?.find((j) => j.id === message.job_id)
    : undefined;
  const jobDone = job?.status === "done";
  // 历史消息 Markdown 渲染结果缓存：流式期间消息列表重渲染时避免全量重复解析
  const contentHtml = useMemo(
    () => renderMarkdown(message.content),
    [message.content]
  );

  return (
    <div className="msg assistant">
      <div className="msg-avatar">DS</div>
      <div className="msg-body">
        {jobDone && (
          <div className="job-banner">
            <CheckIcon size={13} />
            <span>任务完成</span>
            <span className="job-banner-detail">
              提交于 {formatTime(job.submitted_at)} · 耗时{" "}
              {formatDuration(job.finished_at - job.submitted_at)}
            </span>
          </div>
        )}
        {message.reasoning && (
          <div className={`thinking-block${showReasoning ? " open" : ""}`}>
            <button
              className="thinking-toggle"
              onClick={() => setShowReasoning((v) => !v)}
            >
              <ChevronDownIcon size={13} />
              <span>思考过程</span>
            </button>
            {showReasoning && (
              <div className="thinking-content">{message.reasoning}</div>
            )}
          </div>
        )}
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
        <SearchCards items={message.search_results} onOpenLink={onOpenLink} />
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
