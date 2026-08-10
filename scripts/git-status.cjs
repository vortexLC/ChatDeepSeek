// 查看仓库变更状态（等价 git status），纯 ASCII 输出
const path = require("path");
const git = require(path.join(process.env.TEMP, "cds_git", "node_modules", "isomorphic-git"));

const dir = "D:\\AI\\Code\\ChatDeepSeek";
const fs = require("fs");

(async () => {
  const status = await git.statusMatrix({ fs, dir });
  const head = await git.log({ fs, dir, depth: 3 });
  console.log("=== HEAD ===");
  for (const c of head) console.log(c.oid.slice(0, 10), c.commit.message.split("\n")[0]);
  console.log("=== CHANGES (filepath | HEAD | workdir | stage) ===");
  let changed = 0;
  for (const [file, h, w, s] of status) {
    // 0 = 不存在, 1 = 与 HEAD 相同, 2 = 有差异
    if (h !== 1 || w !== 1 || s !== 1) {
      console.log(file, "|", h, w, s);
      changed++;
    }
  }
  console.log("total changed files:", changed);
})().catch((e) => {
  console.error("ERROR:", e.message);
  process.exit(1);
});
