// 提交并推送到 Gitea（isomorphic-git，无需本机 git 命令）
// 流程：fetch → 检查分歧 → add（尊重 .gitignore）→ commit → push
const path = require("path");
const fs = require("fs");
const git = require(path.join(process.env.TEMP, "cds_git", "node_modules", "isomorphic-git"));
const http = require(path.join(
  process.env.TEMP,
  "cds_git",
  "node_modules",
  "isomorphic-git",
  "http",
  "node",
  "index.cjs"
));

const dir = "D:\\AI\\Code\\ChatDeepSeek";
const token = process.env.GITEA_TOKEN;
if (!token) {
  console.error("GITEA_TOKEN 未设置");
  process.exit(1);
}
const author = { name: "wlcgitea", email: "luolearninggorum@qq.com" };
// Gitea git HTTP 认证：token 作为 basic auth 用户名，密码留空
const auth = () => ({ username: token, password: "" });

(async () => {
  // 1. fetch 远程状态
  console.log("fetch origin ...");
  await git.fetch({ fs, dir, remote: "origin", onAuth: auth, http });

  // 2. 分歧检查：本地 HEAD 与 origin/main
  const localHead = await git.resolveRef({ fs, dir, ref: "HEAD" });
  let remoteHead = null;
  try {
    remoteHead = await git.resolveRef({ fs, dir, ref: "refs/remotes/origin/main" });
  } catch {}
  console.log("local HEAD :", localHead.slice(0, 10));
  console.log("origin/main:", remoteHead ? remoteHead.slice(0, 10) : "(none)");
  if (remoteHead && remoteHead !== localHead) {
    // 列出远程领先的提交，判断是否快进
    const localLog = await git.log({ fs, dir, ref: "HEAD", depth: 50 });
    const localIds = new Set(localLog.map((c) => c.oid));
    const remoteLog = await git.log({ fs, dir, ref: "refs/remotes/origin/main", depth: 50 });
    const remoteOnly = remoteLog.filter((c) => !localIds.has(c.oid));
    if (remoteOnly.length > 0) {
      console.error("远程存在本地没有的提交，需要先合并/拉取，已停止：");
      for (const c of remoteOnly.slice(0, 5)) console.error("  ", c.oid.slice(0, 10), c.commit.message.split("\n")[0]);
      process.exit(1);
    }
    console.log("远程提交均已在本地（快进）");
  }

  // 3. 暂存全部变更（isomorphic-git add 尊重 .gitignore）
  console.log("git add . ...");
  await git.add({ fs, dir, filepath: "." });

  // 4. 提交（无 staged 变更时跳过，避免产生空提交）
  const status = await git.statusMatrix({ fs, dir });
  const hasStaged = status.some(([, , , s]) => s === 2 || s === 3);
  let oid = null;
  if (hasStaged) {
    const message =
      "feat: 异步视频任务状态追踪、对话区时间线/产物卡片、数据存储健壮性与多项修复\n\n" +
      "- 视频任务：jobs 持久化、提交/完成/失败事件、任务卡片、Toast 通知、侧边栏徽标、重启恢复轮询\n" +
      "- 产物卡片：图片缩略图、视频首帧截图、文件芯片；思考过程时间线化、任务完成横幅\n" +
      "- 修复：停止生成保留已生成内容、流式滚动自由查看、代码块折叠卡片、上下文容量仅文本模型显示\n" +
      "- 存储：session.json/settings 原子写入、保存失败错误提示、删除重试、存储闭环单元测试\n" +
      "- 启动/打包脚本优化（中文界面、参数执行、WebView2 检测、保留便携版数据目录）";
    console.log("commit ...");
    oid = await git.commit({ fs, dir, message, author });
    console.log("committed:", oid.slice(0, 10));
  } else {
    console.log("no staged changes, skip commit");
    try {
      oid = await git.resolveRef({ fs, dir, ref: "HEAD" });
    } catch {}
  }

  // 5. 推送
  console.log("push origin main ...");
  const result = await git.push({ fs, dir, remote: "origin", ref: "main", onAuth: auth, http });
  console.log("push result:", JSON.stringify(result));
  console.log("DONE");
})().catch((e) => {
  console.error("ERROR:", e.message);
  process.exit(1);
});
