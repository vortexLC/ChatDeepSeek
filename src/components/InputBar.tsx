import { useEffect, useRef, useState } from "react";
import type { EditTarget, Effort, ModelOption } from "../types";
import {
  BrainIcon,
  ChevronDownIcon,
  GlobeIcon,
  GlobeOffIcon,
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

interface InputBarProps {
  disabled: boolean;
  contextFull: boolean;
  streaming: boolean;
  editTarget: EditTarget | null;
  onCancelEdit: () => void;
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
  onSend: (content: string) => void;
  onSendEdit: (content: string) => void;
  onStop: () => void;
}

export function InputBar({
  disabled,
  contextFull,
  streaming,
  editTarget,
  onCancelEdit,
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
  const taRef = useRef<HTMLTextAreaElement>(null);

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

  const canSend = !disabled && !contextFull && !streaming && value.trim().length > 0;

  // 保证强度菜单始终包含当前值（兼容旧数据中的 low/none 等）
  const effectiveEffortOptions = effortOptions.includes(effort)
    ? effortOptions
    : [effort, ...effortOptions];

  const submit = () => {
    if (!canSend) return;
    if (editTarget) {
      onSendEdit(value.trim());
    } else {
      onSend(value.trim());
    }
    setValue("");
  };

  const cancelEdit = () => {
    setValue("");
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
            {deepThink ? (
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
