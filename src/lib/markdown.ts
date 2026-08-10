import { marked } from "marked";
import DOMPurify from "dompurify";
// 按需引入 highlight.js：core + 常用语言，避免全量语言包导致 bundle 超过 1MB
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import go from "highlight.js/lib/languages/go";
import http from "highlight.js/lib/languages/http";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import php from "highlight.js/lib/languages/php";
import plaintext from "highlight.js/lib/languages/plaintext";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

hljs.registerLanguage("bash", bash);
hljs.registerAliases(["sh", "shell", "zsh", "console", "powershell", "ps1", "bat"], { languageName: "bash" });
hljs.registerLanguage("c", c);
hljs.registerLanguage("cpp", cpp);
hljs.registerAliases(["c++", "h", "hpp"], { languageName: "cpp" });
hljs.registerLanguage("csharp", csharp);
hljs.registerAliases(["cs", "c#"], { languageName: "csharp" });
hljs.registerLanguage("css", css);
hljs.registerAliases(["scss", "less"], { languageName: "css" });
hljs.registerLanguage("diff", diff);
hljs.registerLanguage("go", go);
hljs.registerLanguage("http", http);
hljs.registerLanguage("ini", ini);
hljs.registerAliases(["toml", "conf"], { languageName: "ini" });
hljs.registerLanguage("java", java);
hljs.registerLanguage("javascript", javascript);
hljs.registerAliases(["js", "jsx", "mjs", "cjs"], { languageName: "javascript" });
hljs.registerLanguage("json", json);
hljs.registerLanguage("markdown", markdown);
hljs.registerAliases(["md"], { languageName: "markdown" });
hljs.registerLanguage("php", php);
hljs.registerLanguage("plaintext", plaintext);
hljs.registerAliases(["text", "txt"], { languageName: "plaintext" });
hljs.registerLanguage("python", python);
hljs.registerAliases(["py"], { languageName: "python" });
hljs.registerLanguage("ruby", ruby);
hljs.registerAliases(["rb"], { languageName: "ruby" });
hljs.registerLanguage("rust", rust);
hljs.registerAliases(["rs"], { languageName: "rust" });
hljs.registerLanguage("sql", sql);
hljs.registerLanguage("typescript", typescript);
hljs.registerAliases(["ts", "tsx"], { languageName: "typescript" });
hljs.registerLanguage("xml", xml);
hljs.registerAliases(["html", "svg", "vue"], { languageName: "xml" });
hljs.registerLanguage("yaml", yaml);
hljs.registerAliases(["yml"], { languageName: "yaml" });

marked.setOptions({
  gfm: true,
  breaks: true,
});

const renderer = new marked.Renderer();
const originalCode = renderer.code.bind(renderer);
renderer.code = (token) => {
  const rawLang = (token.lang || "").trim();
  const code = token.text ?? "";
  // 语言标签可携带文件名：如 "python app.py" / "python:app.py"
  let lang = rawLang;
  let fileName = "";
  const spaceIdx = rawLang.indexOf(" ");
  if (spaceIdx > 0) {
    lang = rawLang.slice(0, spaceIdx).trim();
    fileName = rawLang.slice(spaceIdx + 1).trim();
  } else {
    const colonIdx = rawLang.indexOf(":");
    if (colonIdx > 0) {
      lang = rawLang.slice(0, colonIdx).trim();
      fileName = rawLang.slice(colonIdx + 1).trim();
    }
  }
  if (!fileName) {
    // 从代码首行注释提取文件名（# xxx.py / // xxx.js / <!-- xxx.html --> 等）
    fileName = extractFileNameFromCode(code);
  }
  const title = fileName || (lang ? lang : "代码块");
  if (lang && hljs.getLanguage(lang)) {
    const highlighted = hljs.highlight(code, { language: lang }).value;
    // 折叠式代码卡片：默认收起，展开后固定窗口内滚动，标题栏显示文件名/语言
    return `<div class="code-block"><button class="code-block-toggle" type="button"><svg class="code-block-chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg><span class="code-block-title" title="${escapeHtml(title)}">${escapeHtml(title)}</span><span class="code-block-lang">${escapeHtml(lang)}</span></button><div class="code-block-body"><pre><code class="hljs language-${lang}">${highlighted}</code></pre></div></div>`;
  }
  return originalCode(token);
};

/** 从代码首行注释提取文件名（# xxx.py / // xxx.js / <!-- xxx.html --> / -- xxx.sql 等） */
function extractFileNameFromCode(code: string): string {
  const firstLine = (code.split("\n")[0] ?? "").trim();
  const m = firstLine.match(
    /^(?:#|\/\/|<!--|--|;;|%|REM)\s*([\w@./\\-]+\.[A-Za-z0-9]+)/i
  );
  return m ? m[1] : "";
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function renderMarkdown(md: string): string {
  const raw = marked.parse(md, { renderer }) as string;
  return DOMPurify.sanitize(raw, {
    // checked/disabled 用于保留 GFM 任务列表复选框的状态
    ADD_ATTR: ["target", "checked", "disabled"],
    ADD_TAGS: ["input"],
  });
}
