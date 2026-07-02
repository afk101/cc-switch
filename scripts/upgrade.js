#!/usr/bin/env node

/**
 * 上游版本升级脚本
 *
 * 功能：
 *   1. 检查并配置 upstream remote（farion1231/cc-switch.git）
 *   2. 拉取 upstream 最新 tags
 *   3. 解析当前分支对应的 tag 版本
 *   4. 交互式选择要合并的目标 tag
 *   5. 基于当前分支创建新分支并执行 merge
 *   6. 自动 push 或提示处理冲突
 */

import { execSync } from "child_process";
import readline from "readline";

// ─── 常量 ─────────────────────────────────────────────────────────────────────
const UPSTREAM_URL = "https://github.com/farion1231/cc-switch.git";
const UPSTREAM_REMOTE = "upstream";
// 匹配分支名中的版本号，例如 feature/v3.16.4 → v3.16.4
const BRANCH_VERSION_RE = /v\d+\.\d+\.\d+(?:-[a-zA-Z0-9.]+)?/;

// ─── 工具函数 ──────────────────────────────────────────────────────────────────

/**
 * 执行 shell 命令并返回 stdout（去除首尾空白）
 * @param {string} cmd - 要执行的命令
 * @param {object} [opts] - 可选配置
 * @param {boolean} [opts.silent] - 是否抑制 stderr 输出
 * @returns {string}
 */
function run(cmd, opts = {}) {
  try {
    return execSync(cmd, {
      encoding: "utf8",
      stdio: opts.silent ? ["pipe", "pipe", "pipe"] : ["pipe", "pipe", "pipe"],
    }).trim();
  } catch (err) {
    if (opts.throwOnError) throw err;
    return "";
  }
}

/**
 * 打印带颜色的信息
 * @param {string} msg
 * @param {"info"|"success"|"warn"|"error"} type
 */
function log(msg, type = "info") {
  const colors = {
    info: "\x1b[36m", // 青色
    success: "\x1b[32m", // 绿色
    warn: "\x1b[33m", // 黄色
    error: "\x1b[31m", // 红色
  };
  const reset = "\x1b[0m";
  const prefix = { info: "ℹ", success: "✓", warn: "⚠", error: "✗" }[type];
  console.log(`${colors[type]}${prefix} ${msg}${reset}`);
}

/**
 * 创建 readline 接口
 * @returns {readline.Interface}
 */
function createRL() {
  return readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });
}

/**
 * 询问用户问题并返回答案
 * @param {readline.Interface} rl
 * @param {string} question
 * @returns {Promise<string>}
 */
function ask(rl, question) {
  return new Promise((resolve) => rl.question(question, resolve));
}

// ─── 核心逻辑 ──────────────────────────────────────────────────────────────────

/**
 * 检查并配置 upstream remote
 * @returns {Promise<void>}
 */
async function ensureUpstream() {
  const remotes = run("git remote");
  const hasUpstream = remotes.split("\n").includes(UPSTREAM_REMOTE);

  if (hasUpstream) {
    const existingUrl = run(`git remote get-url ${UPSTREAM_REMOTE}`);
    if (existingUrl !== UPSTREAM_URL) {
      log(
        `upstream remote 已存在但 URL 不匹配：${existingUrl}，期望：${UPSTREAM_URL}`,
        "warn",
      );
      log("正在更新 upstream URL...", "info");
      run(`git remote set-url ${UPSTREAM_REMOTE} ${UPSTREAM_URL}`);
      log("upstream URL 已更新", "success");
    } else {
      log("upstream remote 已配置", "success");
    }
  } else {
    log("未检测到 upstream remote，正在添加...", "info");
    run(`git remote add ${UPSTREAM_REMOTE} ${UPSTREAM_URL}`);
    log(`upstream remote 已添加：${UPSTREAM_URL}`, "success");
  }
}

/**
 * 从 upstream 拉取最新 tags
 * @returns {Promise<void>}
 */
async function fetchUpstreamTags() {
  log("正在从 upstream 拉取最新 tags...", "info");
  const result = run(`git fetch ${UPSTREAM_REMOTE} --tags`, { silent: true });
  log("tags 拉取完成", "success");
}

/**
 * 从当前分支名解析本地 tag 版本
 * @returns {string} 例如 "v3.16.4"
 */
function getLocalTag() {
  const branch = run("git branch --show-current");
  if (!branch) {
    log("无法获取当前分支名（可能处于 detached HEAD 状态）", "error");
    process.exit(1);
  }

  const match = branch.match(BRANCH_VERSION_RE);
  if (!match) {
    log(
      `无法从分支名 "${branch}" 中解析版本号，期望格式如 feature/v3.16.4`,
      "error",
    );
    process.exit(1);
  }

  return match[0];
}

/**
 * 比较两个语义化版本号
 * @param {string} a - 例如 "v3.16.4"
 * @param {string} b - 例如 "v3.17.0"
 * @returns {number} -1 | 0 | 1
 */
function compareVersions(a, b) {
  const parse = (v) => {
    // 去掉前缀 v，分离主版本号和预发布后缀
    const withoutV = v.replace(/^v/, "");
    const [core, ...preParts] = withoutV.split("-");
    const nums = core.split(".").map(Number);
    const pre = preParts.join("-");
    return { nums, pre };
  };

  const pa = parse(a);
  const pb = parse(b);

  // 比较主版本号
  for (let i = 0; i < 3; i++) {
    const na = pa.nums[i] ?? 0;
    const nb = pb.nums[i] ?? 0;
    if (na !== nb) return na < nb ? -1 : 1;
  }

  // 主版本号相同，有预发布后缀的排在后面（更大）
  if (!pa.pre && !pb.pre) return 0;
  if (!pa.pre) return 1; // a 是正式版，b 是预发布 → a 更大
  if (!pb.pre) return -1; // b 是正式版，a 是预发布 → b 更大
  return pa.pre < pb.pre ? -1 : pa.pre > pb.pre ? 1 : 0;
}

/**
 * 获取 upstream 上比本地 tag 更新的所有 tags
 * @param {string} localTag - 本地当前 tag，例如 "v3.16.4"
 * @returns {string[]} 按版本升序排列的 tag 列表
 */
function getNewerUpstreamTags(localTag) {
  // 获取 upstream 上所有 tags
  const rawTags = run(
    `git ls-remote --tags ${UPSTREAM_REMOTE} | awk -F'refs/tags/' '{print $2}' | grep -v '\\^{}'`,
  );

  if (!rawTags) {
    log("无法获取 upstream tags 列表", "error");
    process.exit(1);
  }

  const allUpstreamTags = rawTags.split("\n").filter(Boolean);

  // 过滤出比本地 tag 更新的版本，并排序
  return allUpstreamTags
    .filter((tag) => compareVersions(tag, localTag) > 0)
    .sort(compareVersions);
}

/**
 * 交互式选择目标 tag
 * @param {string[]} tags - 可选的 tag 列表（升序）
 * @param {string} localTag - 当前本地 tag
 * @returns {Promise<string>} 用户选择的 tag
 */
async function selectTargetTag(tags, localTag) {
  const rl = createRL();
  const latestTag = tags[tags.length - 1];

  console.log("\n\x1b[1m可选的 upstream 新版本：\x1b[0m");
  console.log(`  当前版本：${localTag}\n`);

  tags.forEach((tag, idx) => {
    const isLatest = tag === latestTag;
    const marker = isLatest ? " \x1b[32m← 最新\x1b[0m" : "";
    console.log(`  [${idx + 1}] ${tag}${marker}`);
  });

  console.log(`  [0] 取消退出\n`);

  while (true) {
    const answer = await ask(
      rl,
      `请选择要升级到的版本 [1-${tags.length}]（默认 ${tags.length}，最新）：`,
    );
    const trimmed = answer.trim();

    if (trimmed === "0") {
      log("已取消操作", "warn");
      rl.close();
      process.exit(0);
    }

    const idx = trimmed === "" ? tags.length : parseInt(trimmed, 10);

    if (isNaN(idx) || idx < 1 || idx > tags.length) {
      log(`请输入 1 到 ${tags.length} 之间的数字`, "warn");
      continue;
    }

    rl.close();
    return tags[idx - 1];
  }
}

/**
 * 检查工作区是否干净（无未提交的更改）
 * @returns {boolean}
 */
function isWorkingTreeClean() {
  const status = run("git status --porcelain");
  return status === "";
}

/**
 * 执行升级合并操作
 * @param {string} targetTag - 目标 tag，例如 "v3.17.0"
 * @returns {Promise<void>}
 */
async function performUpgrade(targetTag) {
  const newBranch = `feature/${targetTag}`;

  // 检查目标分支是否已存在
  const existingBranches = run("git branch --list");
  if (existingBranches.split("\n").map((b) => b.trim().replace(/^\* /, "")).includes(newBranch)) {
    log(`分支 "${newBranch}" 已存在`, "error");
    log("请手动删除该分支或选择其他版本", "warn");
    process.exit(1);
  }

  // 检查工作区是否干净
  if (!isWorkingTreeClean()) {
    log("工作区有未提交的更改，请先提交或暂存后再执行升级", "error");
    process.exit(1);
  }

  log(`正在基于当前分支创建新分支：${newBranch}`, "info");
  run(`git checkout -b ${newBranch}`);
  log(`新分支 "${newBranch}" 已创建并切换`, "success");

  log(`正在合并 tag：${targetTag}`, "info");
  try {
    run(`git merge ${targetTag} --no-edit`);
    log(`tag "${targetTag}" 合并成功！`, "success");
  } catch (err) {
    // merge 冲突
    log(`合并 "${targetTag}" 时出现冲突，请手动解决后执行：`, "error");
    console.log("  1. 解决冲突文件");
    console.log("  2. git add <已解决的文件>");
    console.log("  3. git commit");
    console.log(`  4. git push origin ${newBranch}`);
    process.exit(1);
  }

  // 自动 push
  const rl = createRL();
  const confirm = await ask(
    rl,
    `\n是否立即推送到 origin？[Y/n]：`,
  );
  rl.close();

  const shouldPush = confirm.trim().toLowerCase() !== "n";

  if (shouldPush) {
    log(`正在推送分支 "${newBranch}" 到 origin...`, "info");
    try {
      run(`git push origin ${newBranch}`);
      log(`分支 "${newBranch}" 已成功推送到 origin！`, "success");
    } catch (err) {
      log(`推送失败，请手动执行：git push origin ${newBranch}`, "error");
      process.exit(1);
    }
  } else {
    log("已跳过推送，可稍后手动执行：", "info");
    console.log(`  git push origin ${newBranch}`);
  }
}

// ─── 主入口 ────────────────────────────────────────────────────────────────────

async function main() {
  console.log("\n\x1b[1m🚀 cc-switch 上游版本升级工具\x1b[0m\n");

  // 1. 检查并配置 upstream
  await ensureUpstream();

  // 2. 拉取 upstream tags
  await fetchUpstreamTags();

  // 3. 解析本地当前 tag
  const localTag = getLocalTag();
  log(`当前分支版本：${localTag}`, "info");

  // 4. 获取比本地更新的 upstream tags
  const newerTags = getNewerUpstreamTags(localTag);

  if (newerTags.length === 0) {
    log("当前已是最新版本，无需升级！", "success");
    process.exit(0);
  }

  log(`发现 ${newerTags.length} 个新版本可升级`, "success");

  // 5. 交互式选择目标 tag
  const targetTag = await selectTargetTag(newerTags, localTag);
  log(`已选择升级目标：${targetTag}`, "success");

  // 6. 执行升级
  await performUpgrade(targetTag);

  console.log("\n\x1b[1m✅ 升级流程完成！\x1b[0m\n");
}

main().catch((err) => {
  log(`未预期的错误：${err.message}`, "error");
  process.exit(1);
});
