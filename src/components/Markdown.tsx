import { useEffect, useRef } from "react";

export function Markdown({
  html,
  className,
  onOpenLink,
}: {
  html: string;
  className?: string;
  onOpenLink: (url: string) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onClick = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      // 代码块折叠/展开（点击标题栏切换）
      const toggle = target.closest?.(".code-block-toggle");
      if (toggle) {
        e.preventDefault();
        toggle.parentElement?.classList.toggle("open");
        return;
      }
      const link = target.closest?.("a");
      const href = link?.getAttribute("href");
      if (link && href) {
        // 一律拦截默认导航：http(s) 走应用内预览，其它协议（file://、mailto:、#锚点、
        // 相对链接等）忽略，防止主窗口被导航到本地文件/空白页而无法返回
        e.preventDefault();
        if (/^https?:\/\//i.test(href)) {
          onOpenLink(href);
        }
      }
    };
    el.addEventListener("click", onClick);
    return () => el.removeEventListener("click", onClick);
  }, [onOpenLink]);

  // 流式期间代码块自动展开，方便实时查看正在生成的代码内容；
  // 完成后由用户手动展开/收起（非流式 html 稳定，展开状态不会被打断）
  useEffect(() => {
    const el = ref.current;
    if (!el || !className?.includes("streaming")) return;
    el.querySelectorAll<HTMLElement>(".code-block").forEach((b) =>
      b.classList.add("open")
    );
  }, [html, className]);

  return (
    <div
      ref={ref}
      className={className}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
