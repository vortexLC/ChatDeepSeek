import { marked } from "marked";
import DOMPurify from "dompurify";
import hljs from "highlight.js";

marked.setOptions({
  gfm: true,
  breaks: true,
});

const renderer = new marked.Renderer();
const originalCode = renderer.code.bind(renderer);
renderer.code = (token) => {
  const lang = (token.lang || "").trim();
  const code = token.text ?? "";
  if (lang && hljs.getLanguage(lang)) {
    const highlighted = hljs.highlight(code, { language: lang }).value;
    return `<div class="code-block"><div class="code-header"><span>${escapeHtml(
      lang
    )}</span></div><pre><code class="hljs language-${lang}">${highlighted}</code></pre></div>`;
  }
  return originalCode(token);
};

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function renderMarkdown(md: string): string {
  const raw = marked.parse(md, { renderer }) as string;
  return DOMPurify.sanitize(raw, {
    ADD_ATTR: ["target"],
    ADD_TAGS: ["input"],
  });
}
