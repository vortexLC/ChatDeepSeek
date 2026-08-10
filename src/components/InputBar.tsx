import { useEffect, useRef, useState } from "react";
import type {
  AgentMode,
  EditTarget,
  Effort,
  ModelOption,
  UploadAttachment,
} from "../types";
import {
  BrainIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  GlobeIcon,
  GlobeOffIcon,
  LinkIcon,
  PaperclipIcon,
  PencilIcon,
  SendIcon,
  StopIcon,
  XIcon,
} from "./icons";

const EFFORT_LABELS: Record<string, string> = {
  low: "低",
  high: "高",
  max: "最大",
};

const ACCEPT_TYPES =
  "image/png,image/jpeg,image/gif,image/webp,image/bmp,.txt,.md,.markdown,.csv,.json,.xml,.yaml,.yml,.log,.ini,.conf,.toml,.sql,.py,.js,.ts,.rs,.java,.c,.cpp,.h,.hpp,.go,.rb,.php,.sh,.bat,.ps1,.html,.css,.scss,.vue,.tsx,.jsx,.pdf";

const MODE_OPTIONS: { value: AgentMode; label: string; title: string }[] = [
  { value: "chat", label: "Chat", title: "普通对话" },
  { value: "image", label: "Image", title: "Chat + 图片生成" },
  { value: "video", label: "Video", title: "Chat + 视频生成" },
  { value: "build", label: "Build", title: "编程工具（隔离沙箱）" },
  { value: "agent", label: "Agent", title: "全部工具（编程 + 图片 + 视频）" },
];

function modeLabel(mode: AgentMode): string {
  return MODE_OPTIONS.find((m) => m.value === mode)?.label ?? "Chat";
}

interface InputBarProps {
  resetKey: number;
  disabled: boolean;
  contextFull: boolean;
  streaming: boolean;
  editTarget: EditTarget | null;
  onCancelEdit: () => void;
  mode: AgentMode;
  onSetMode: (mode: AgentMode) => void;
  modelOptions: ModelOption[];
  currentModel: string;
  onSelectModel: (option: ModelOption) => void;
  webSearch: boolean;
  deepThink: boolean;
  effort: Effort;
  effortOptions: Effort[];
  onToggleWebSearch: () => void;
  onToggleDeepThink: () => void;
  onSetEffort: (e: Effort) => void;
  onSend: (content: string, attachments: UploadAttachment[]) => Promise<boolean>;
  onSendEdit: (content: string, attachments: UploadAttachment[]) => Promise<boolean>;
  onStop: () => void;
}

export function InputBar({
  resetKey,
  disabled,
  contextFull,
  streaming,
  editTarget,
  onCancelEdit,
  mode,
  onSetMode,
  modelOptions,
  currentModel,
  onSelectModel,
  webSearch,
  deepThink,
  effort,
  effortOptions,
  onToggleWebSearch,
  onToggleDeepThink,
  onSetEffort,
  onSend,
  onSendEdit,
  onStop,
}: InputBarProps) {
  const [value, setValue] = useState("");
  const [modeOpen, setModeOpen] = useState(false);
  const [attachments, setAttachments] = useState<UploadAttachment[]>([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  // 切换会话时清空输入与附件
  useEffect(() => {
    setValue("");
    setAttachments([]);
    setAttachError(null);
  }, [resetKey]);

  useEffect(() => {
    if (editTarget) {
      setValue(editTarget.text);
      setTimeout(() => {
        taRef.current?.focus();
        taRef.current?.setSelectionRange(
          editTarget.text.length,
          editTarget.text.length
        );
      }, 30);
    }
  }, [editTarget]);

  useEffect(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(ta.scrollHeight, 200)}px`;
  }, [value]);

  const canSend =
    !disabled &&
    !contextFull &&
    !streaming &&
    (value.trim().length > 0 || attachments.length > 0);

  // 保证强度菜单始终包含当前值（兼容旧数据中的 low/none 等）
  const effectiveEffortOptions = effortOptions.includes(effort)
    ? effortOptions
    : [effort, ...effortOptions];

  const pickFiles = async (files: FileList | null) => {
    if (!files) return;
    const next: UploadAttachment[] = [];
    let err: string | null = null;
    for (const f of Array.from(files).slice(0, 10)) {
      if (f.size > 8 * 1024 * 1024) {
        err = `「${f.name}」超过 8MB，已跳过`;
        continue;
      }
      const kind: "image" | "document" = f.type.startsWith("image/")
        ? "image"
        : "document";
      const data_url = await new Promise<string>((resolve) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result ?? ""));
        reader.onerror = () => resolve("");
        reader.readAsDataURL(f);
      });
      if (data_url) {
        next.push({
          name: f.name,
          mime: f.type || "application/octet-stream",
          kind,
          data_url,
        });
      }
    }
    setAttachError(err);
    if (next.length > 0) {
      setAttachments((prev) => [...prev, ...next]);
    }
  };

  const submit = async () => {
    if (!canSend) return;
    const text = value.trim();
    const ok = editTarget
      ? await onSendEdit(text, attachments)
      : await onSend(text, attachments);
    if (!ok) {
      // 发送失败：保留已输入的文字与附件，避免用户重新输入
      return;
    }
    setValue("");
    setAttachments([]);
    setAttachError(null);
  };

  const cancelEdit = () => {
    setValue("");
    setAttachments([]);
    onCancelEdit();
  };

  const inputDisabled = disabled || contextFull;

  return (
    <div className="input-wrap">
      <div className="input-box">
        {editTarget && (
          <div className="edit-bar">
            <PencilIcon size={12} />
            <span>正在编辑已发送的消息</span>
            <button className="edit-bar-cancel" onClick={cancelEdit}>
              <XIcon size={11} />
              取消编辑
            </button>
          </div>
        )}
        {attachments.length > 0 && (
          <div className="input-attachments">
            {attachments.map((a, i) => (
              <div key={`${a.name}-${i}`} className="attachment-chip">
                {a.kind === "image" ? (
                  <img
                    className="attachment-thumb"
                    src={a.data_url}
                    alt={a.name}
                  />
                ) : (
                  <LinkIcon size={13} />
                )}
                <span className="attachment-name" title={a.name}>
                  {a.name}
                </span>
                <button
                  className="attachment-remove"
                  title="移除"
                  onClick={() =>
                    setAttachments((prev) => prev.filter((_, j) => j !== i))
                  }
                >
                  <XIcon size={10} />
                </button>
              </div>
            ))}
          </div>
        )}
        {attachError && <div className="attach-error">{attachError}</div>}
        <textarea
          ref={taRef}
          className="input-textarea"
          rows={1}
          placeholder={
            contextFull
              ? "上下文已满，请新开会话"
              : disabled
                ? "请先开启新对话"
                : "输入消息，Enter 发送，Shift+Enter 换行"
          }
          value={value}
          disabled={inputDisabled}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              submit();
            }
          }}
        />
        <div className="input-tools">
          <div className="input-tools-left">
            <div className="mode-select-wrap">
              <button
                className="mode-trigger"
                title="选择模式"
                onClick={() => setModeOpen((v) => !v)}
                disabled={inputDisabled || streaming}
              >
                {modeLabel(mode)}
                <ChevronUpIcon size={12} />
              </button>
              {modeOpen && (
                <>
                  <div
                    className="mode-popover-mask"
                    onClick={() => setModeOpen(false)}
                  />
                  <div className="mode-popover">
                    {MODE_OPTIONS.map((m) => (
                      <button
                        key={m.value}
                        className={`mode-option${mode === m.value ? " active" : ""}`}
                        onClick={() => {
                          onSetMode(m.value);
                          setModeOpen(false);
                        }}
                      >
                        <span className="mode-option-label">{m.label}</span>
                        <span className="mode-option-desc">{m.title}</span>
                      </button>
                    ))}
                  </div>
                </>
              )}
            </div>
            <button
              className="tool-btn"
              title="上传图片或文档（图片需多模态模型）"
              onClick={() => fileRef.current?.click()}
              disabled={inputDisabled || streaming}
            >
              <PaperclipIcon size={16} />
            </button>
            <input
              ref={fileRef}
              type="file"
              multiple
              accept={ACCEPT_TYPES}
              style={{ display: "none" }}
              onChange={(e) => {
                pickFiles(e.target.files);
                e.target.value = "";
              }}
            />
            <div className="tool-model-select">
              <select
                value={currentModel}
                disabled={inputDisabled || streaming}
                onChange={(e) => {
                  const option = modelOptions.find(
                    (o) => o.model === e.target.value
                  );
                  if (option) onSelectModel(option);
                }}
                title="选择 AI 模型"
              >
                {modelOptions.map((o) => (
                  <option key={o.model} value={o.model}>
                    {o.label}
                  </option>
                ))}
              </select>
              <ChevronDownIcon size={12} />
            </div>
            <button
              className={`tool-btn${webSearch ? " on" : ""}`}
              title={webSearch ? "联网搜索：开" : "联网搜索：关"}
              onClick={onToggleWebSearch}
              disabled={inputDisabled || streaming}
            >
              {webSearch ? <GlobeIcon size={16} /> : <GlobeOffIcon size={16} />}
              <span className="tool-label">联网</span>
            </button>
            {deepThink && effortOptions.length === 0 ? (
              <button
                className="tool-btn on"
                title="深度思考：开（当前模型协议不支持调节推理强度）"
                onClick={onToggleDeepThink}
                disabled={inputDisabled || streaming}
              >
                <BrainIcon size={16} />
                <span className="tool-label">深度思考</span>
              </button>
            ) : deepThink ? (
              <div className="effort-wrap">
                <button
                  className="tool-btn on"
                  title={`深度思考：${EFFORT_LABELS[effort] ?? effort}（点击关闭）`}
                  onClick={onToggleDeepThink}
                  disabled={inputDisabled || streaming}
                >
                  <BrainIcon size={16} />
                  <span className="tool-label">深度思考</span>
                </button>
                <div className="effort-popover">
                  <div className="effort-popover-title">
                    推理强度
                    {effectiveEffortOptions.includes(effort)
                      ? `（当前：${EFFORT_LABELS[effort] ?? effort}）`
                      : ""}
                  </div>
                  <div className="effort-options">
                    {effectiveEffortOptions.map((opt) => (
                      <button
                        key={opt}
                        className={`effort-option${effort === opt ? " active" : ""}`}
                        onClick={() => onSetEffort(opt)}
                        disabled={inputDisabled || streaming}
                      >
                        {EFFORT_LABELS[opt] ?? opt}
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            ) : (
              <button
                className="tool-btn"
                title="深度思考：关（点击开启）"
                onClick={onToggleDeepThink}
                disabled={inputDisabled || streaming}
              >
                <BrainIcon size={16} />
                <span className="tool-label">深度思考</span>
              </button>
            )}
          </div>
          <div className="input-tools-right">
            {streaming ? (
              <button className="btn-stop" onClick={onStop} title="停止生成">
                <StopIcon size={15} />
              </button>
            ) : (
              <button
                className="btn-send"
                onClick={submit}
                disabled={!canSend}
                title="发送"
              >
                <SendIcon size={16} />
              </button>
            )}
          </div>
        </div>
      </div>

      <div className="input-hint">
        AI 生成内容仅供参考，请注意甄别信息真实性
      </div>
    </div>
  );
}
