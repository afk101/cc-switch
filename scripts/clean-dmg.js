/**
 * 清理残留的 DMG 挂载文件和临时文件
 * 用于在构建前自动清理，防止 hdiutil 挂载冲突导致打包失败
 *
 * 问题背景：
 * - Tauri 打包 macOS DMG 时使用 hdiutil 挂载临时磁盘映像
 * - 如果上次打包中途失败，可能留下 rw.*.dmg 文件仍处于挂载状态
 * - 再次打包时 hdiutil 会因卷名冲突而失败
 */

import { execSync } from "child_process";
import { readdirSync, unlinkSync, existsSync } from "fs";
import { join } from "path";

/**
 * 执行 shell 命令并返回输出
 * @param {string} cmd - 要执行的命令
 * @returns {string} 命令输出
 */
function run(cmd) {
  try {
    return execSync(cmd, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  } catch {
    return "";
  }
}

/**
 * 卸载所有与 CC Switch 相关的已挂载卷
 */
function detachVolumes() {
  // 查找所有已挂载的 CC Switch 相关卷
  const mountOutput = run("mount | grep -i 'CC Switch' || true");
  const lines = mountOutput.trim().split("\n").filter(Boolean);

  for (const line of lines) {
    // mount 输出格式: /dev/diskXsY on /Volumes/CC Switch (hfs, ...)
    const match = line.match(/on (\/Volumes\/[^\s(]+)/);
    if (match) {
      const volPath = match[1];
      console.log(`[clean-dmg] 卸载残留卷: ${volPath}`);
      run(`hdiutil detach "${volPath}" -force 2>/dev/null || true`);
    }
  }

  // 额外尝试直接卸载常见卷名
  run('hdiutil detach "/Volumes/CC Switch" -force 2>/dev/null || true');
}

/**
 * 删除指定目录下的 rw.*.dmg 临时文件
 * @param {string} dir - 目标目录路径
 */
function cleanRwDmgFiles(dir) {
  if (!existsSync(dir)) return;

  const files = readdirSync(dir);
  for (const file of files) {
    if (file.startsWith("rw.") && file.endsWith(".dmg")) {
      const filePath = join(dir, file);
      console.log(`[clean-dmg] 删除残留文件: ${filePath}`);
      try {
        unlinkSync(filePath);
      } catch (e) {
        console.warn(`[clean-dmg] 无法删除 ${filePath}: ${e.message}`);
      }
    }
  }
}

/**
 * 主清理流程
 */
function main() {
  // 仅在 macOS 上执行
  if (process.platform !== "darwin") {
    console.log("[clean-dmg] 非 macOS 平台，跳过 DMG 清理");
    return;
  }

  console.log("[clean-dmg] 开始清理残留 DMG 文件...");

  // 1. 卸载残留卷
  detachVolumes();

  // 2. 删除临时 dmg 文件
  const bundleBase = "src-tauri/target/release/bundle";
  cleanRwDmgFiles(join(bundleBase, "dmg"));
  cleanRwDmgFiles(join(bundleBase, "macos"));

  console.log("[clean-dmg] 清理完成");
}

main();
