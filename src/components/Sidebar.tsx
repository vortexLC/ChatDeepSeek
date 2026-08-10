import { useMemo, useRef, useState } from "react";
import type { Conversation } from "../types";
import { ChatIcon, PlusIcon, SettingsIcon, TrashIcon } from "./icons";

interface SidebarProps {
  conversations: Conversation[];
  activeId: number | null;
  /** 各会话进行中（pending）任务数量，>0 时显示"生成中"徽标 */
  pendingJobs?: Record<number, number>;
  onSelect: (id: number) => void;
  onNew: () => void;
  onDelete: (id: number) => void;
  onRename: (id: number, title: string) => void;
  onOpenSettings: () => void;
}

function groupLabel(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}年${d.getMonth() + 1}月`;
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  if (sameDay) {
    return `${String(d.getHours()).padStart(2, "0")}:${String(
      d.getMinutes()
    ).padStart(2, "0")}`;
  }
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

export function Sidebar({
  conversations,
  activeId,
  pendingJobs = {},
  onSelect,
  onNew,
  onDelete,
  onRename,
  onOpenSettings,
}: SidebarProps) {
  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [confirmId, setConfirmId] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);

  const groups = useMemo(() => {
    const map = new Map<string, Conversation[]>();
    for (const c of conversations) {
      const label = groupLabel(c.created_at);
      const arr = map.get(label) ?? [];
      arr.push(c);
      map.set(label, arr);
    }
    return Array.from(map.entries());
  }, [conversations]);

  const startRename = (c: Conversation) => {
    setRenamingId(c.id);
    setDraft(c.title);
    setTimeout(() => {
      renameInputRef.current?.focus();
      renameInputRef.current?.select();
    }, 30);
  };

  const commitRename = () => {
    if (renamingId !== null) {
      const title = draft.trim();
      if (title && title.length > 0) onRename(renamingId, title);
    }
    setRenamingId(null);
  };

  return (
    <aside className="sidebar">
      <div className="sidebar-top">
        <div className="brand">
          <span className="brand-logo">
            <ChatIcon size={18} />
          </span>
          <span className="brand-name">ChatDeepSeek</span>
        </div>
        <button className="btn-new" onClick={onNew}>
          <PlusIcon size={15} />
          <span>开启新对话</span>
        </button>
      </div>

      <div className="sidebar-list">
        {groups.length === 0 && (
          <div className="sidebar-empty">暂无对话，点击上方开启新对话</div>
        )}
        {groups.map(([label, items]) => (
          <div className="conv-group" key={label}>
            <div className="conv-group-label">{label}</div>
            {items.map((c) => {
              const active = c.id === activeId;
              const renaming = c.id === renamingId;
              const confirming = c.id === confirmId;
              return (
                <div
                  key={c.id}
                  className={`conv-item${active ? " active" : ""}`}
                  onClick={() => {
                    if (renaming) return;
                    onSelect(c.id);
                  }}
                  onDoubleClick={() => !renaming && startRename(c)}
                >
                  {renaming ? (
                    <input
                      ref={renameInputRef}
                      className="conv-rename-input"
                      value={draft}
                      onChange={(e) => setDraft(e.target.value)}
                      onBlur={commitRename}
                      onClick={(e) => e.stopPropagation()}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitRename();
                        if (e.key === "Escape") setRenamingId(null);
                      }}
                    />
                  ) : (
                    <>
                      <span className="conv-title" title={c.title}>
                        {c.title}
                      </span>
                      {(pendingJobs[c.id] ?? 0) > 0 && (
                        <span className="conv-pending" title="视频正在后台生成">
                          <span className="dot-pulse" />
                          生成中
                        </span>
                      )}
                      <span className="conv-time">{formatTime(c.updated_at)}</span>
                      {c.id === confirmId ? (
                        <button
                          className="conv-del confirm"
                          onClick={(e) => {
                            e.stopPropagation();
                            onDelete(c.id);
                          }}
                          title="确认删除"
                        >
                          确认
                        </button>
                      ) : (
                        <button
                          className="conv-del"
                          onClick={(e) => {
                            e.stopPropagation();
                            if (confirming) {
                              onDelete(c.id);
                              setConfirmId(null);
                            } else {
                              setConfirmId(c.id);
                              setTimeout(
                                () => setConfirmId((v) => (v === c.id ? null : v)),
                                2500
                              );
                            }
                          }}
                          title="删除对话"
                        >
                          <TrashIcon size={13} />
                        </button>
                      )}
                    </>
                  )}
                </div>
              );
            })}
          </div>
        ))}
      </div>

      <div className="sidebar-footer">
        <button className="btn-settings" onClick={onOpenSettings}>
          <SettingsIcon size={17} />
          <span>设置</span>
        </button>
      </div>
    </aside>
  );
}
