import { useState } from "react";
import type { Message, SearchItem } from "../types";
import { renderMarkdown } from "../lib/markdown";
import { Markdown } from "./Markdown";
import {
  CheckIcon,
  ChevronDownIcon,
  CopyIcon,
  ExternalIcon,
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

export function MessageItem({
  message,
  onOpenLink,
  onEditMessage,
}: {
  message: Message;
  onOpenLink: (url: string) => void;
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
        <div className="msg-user-bubble">{message.content}</div>
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

  return (
    <div className="msg assistant">
      <div className="msg-avatar">DS</div>
      <div className="msg-body">
        <SearchCards items={message.search_results} onOpenLink={onOpenLink} />
        {message.reasoning && (
          <div className={`thinking-block${showReasoning ? " open" : ""}`}>
            <button
              className="thinking-toggle"
              onClick={() => setShowReasoning((v) => !v)}
            >
              <ChevronDownIcon size={13} />
              <span>深度思考过程</span>
            </button>
            {showReasoning && (
              <div className="thinking-content">{message.reasoning}</div>
            )}
          </div>
        )}
        {message.content && (
          <Markdown
            className="markdown-body"
            html={renderMarkdown(message.content)}
            onOpenLink={onOpenLink}
          />
        )}
        {!message.content && (
          <div className="msg-empty">（模型未返回内容）</div>
        )}
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
