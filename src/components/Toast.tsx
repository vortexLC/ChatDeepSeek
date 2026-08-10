import { AlertIcon, CheckIcon, SparkIcon, XIcon } from "./icons";

export type ToastType = "info" | "success" | "error";

export interface ToastItem {
  id: number;
  type: ToastType;
  text: string;
}

/** 右上角堆叠通知：任务提交 / 生成完成 / 生成失败 等异步状态的即时反馈 */
export function ToastStack({
  toasts,
  onDismiss,
}: {
  toasts: ToastItem[];
  onDismiss: (id: number) => void;
}) {
  if (toasts.length === 0) return null;
  return (
    <div className="toast-stack">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.type}`}>
          <span className="toast-icon">
            {t.type === "success" ? (
              <CheckIcon size={13} />
            ) : t.type === "error" ? (
              <AlertIcon size={13} />
            ) : (
              <SparkIcon size={13} />
            )}
          </span>
          <span className="toast-text">{t.text}</span>
          <button
            className="toast-close"
            onClick={() => onDismiss(t.id)}
            title="关闭"
          >
            <XIcon size={11} />
          </button>
        </div>
      ))}
    </div>
  );
}
