import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const releaseType = process.argv[2];
const allowedTypes = new Set(["patch", "minor", "major"]);

if (!allowedTypes.has(releaseType)) {
  console.error("Usage: npm run release -- <patch|minor|major>");
  process.exit(1);
}

const run = (cmd, args, options = {}) =>
  execFileSync(cmd, args, {
    cwd: process.cwd(),
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  });

const gitStatus = run("git", ["status", "--short"]).trim();
if (gitStatus) {
  console.error("Release kræver et rent git-worktree.");
  console.error(gitStatus);
  process.exit(1);
}

run("npm", ["version", releaseType, "--no-git-tag-version"], { stdio: "inherit" });

const packageJsonPath = path.join(process.cwd(), "package.json");
const tauriConfigPath = path.join(process.cwd(), "src-tauri", "tauri.conf.json");

const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8"));
const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
const version = packageJson.version;

tauriConfig.version = version;
writeFileSync(tauriConfigPath, `${JSON.stringify(tauriConfig, null, 2)}\n`);

run("git", ["add", "package.json", "package-lock.json", "src-tauri/tauri.conf.json"]);
run("git", ["commit", "-m", `Release v${version}`], { stdio: "inherit" });
run("git", ["tag", `v${version}`], { stdio: "inherit" });

console.log(`Created release commit and tag for v${version}.`);
console.log("Push commit and tag to trigger GitHub Actions.");
