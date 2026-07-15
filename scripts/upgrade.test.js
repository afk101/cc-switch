import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

/**
 * 从升级脚本源码加载真实的命令执行函数，同时跳过交互式主入口。
 * @param {string} temporaryDirectory - 临时模块目录
 * @returns {Promise<(cmd: string, opts?: object) => string>}
 */
async function loadRunFunction(temporaryDirectory) {
  const scriptPath = new URL("./upgrade.js", import.meta.url);
  const source = await readFile(scriptPath, "utf8");
  const sourceWithoutMain = source.replace(
    /\nmain\(\)\.catch\([\s\S]*$/u,
    "\n",
  );
  assert.notEqual(sourceWithoutMain, source, "应移除升级脚本交互式主入口");
  const exportableSource = sourceWithoutMain.replace(
    /\nfunction run\(/u,
    "\nexport function run(",
  );
  const modulePath = path.join(temporaryDirectory, "upgrade-test-module.mjs");
  await writeFile(modulePath, exportableSource, "utf8");
  const module = await import(pathToFileURL(modulePath).href);
  return module.run;
}

test("Git 命令失败时必须向升级流程抛出异常", async (t) => {
  const temporaryDirectory = await mkdtemp(
    path.join(os.tmpdir(), "cc-switch-upgrade-test-"),
  );
  t.after(() => rm(temporaryDirectory, { recursive: true, force: true }));
  const run = await loadRunFunction(temporaryDirectory);

  assert.throws(
    () => run(`"${process.execPath}" -e "process.exit(7)"`, { silent: true }),
    /Command failed/u,
  );
});
