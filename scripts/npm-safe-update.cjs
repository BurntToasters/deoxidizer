#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const console = require("node:console");
const { randomUUID } = require("node:crypto");
const { closeSync, existsSync, mkdtempSync, openSync, readFileSync, renameSync, rmSync, writeFileSync } = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const process = require("node:process");

// Minimal npm invocation helper (mirrors Zinnia scripts/npm-cli.cjs, which
// this repo does not have; deoxidizer has no npm dependencies to resolve).
function npmInvocation({
  env = process.env,
  platform = process.platform,
  execPath = process.execPath,
} = {}) {
  if (platform !== "win32") {
    return { command: "npm", prefixArgs: [] };
  }

  const npmExecPath = String(env.npm_execpath || "").trim();
  if (!npmExecPath) {
    throw new Error(
      "npm_execpath is unavailable on Windows; run this command through npm.",
    );
  }
  return { command: execPath, prefixArgs: [npmExecPath] };
}

// Must stay in sync with package.json (packageManager pin + engines).
const MINIMUM_NPM_VERSION = "12.0.2";
const SUPPORTED_NODE_VERSIONS = ">=22.22.2 <27";

function parseVersion(value) {
  const match = String(value)
    .trim()
    .match(/^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/u);
  if (!match) throw new Error(`Invalid semantic version: ${value}`);
  return match.slice(1).map(Number);
}

function isVersionAtLeast(value, minimum) {
  const current = parseVersion(value);
  const required = parseVersion(minimum);
  for (let index = 0; index < 3; index += 1) {
    if (current[index] > required[index]) return true;
    if (current[index] < required[index]) return false;
  }
  return true;
}

function isSupportedNodeVersion(value) {
  const [major] = parseVersion(value);
  if (major === 22) return isVersionAtLeast(value, "22.22.2");
  return major >= 23 && major < 27;
}

// Pinned Rust toolchain comes from rust-toolchain.toml (single source of
// truth, shared with scripts/check-toolchain.cjs).
function pinnedRustChannel() {
  const toolchainPath = path.join(__dirname, "..", "rust-toolchain.toml");
  const channel = readFileSync(toolchainPath, "utf8").match(
    /^channel\s*=\s*"([^"]+)"/m,
  )?.[1];
  if (!channel) throw new Error("rust-toolchain.toml does not declare a channel");
  return channel;
}

function hasPinnedRustToolchain(output, channel) {
  return String(output)
    .split(/\r?\n/u)
    .some(
      (line) =>
        line === channel ||
        line.startsWith(`${channel}-`) ||
        line.startsWith(`${channel} `),
    );
}

function npmUpdateArguments(cachePath) {
  return [
    "update",
    "--package-lock-only",
    "--ignore-scripts",
    "--no-audit",
    "--min-release-age=3",
    `--cache=${cachePath}`,
  ];
}

// Deoxidizer has no npm dependencies and no reviewed-dev audit script:
// the production audit below is the whole audit plan. `root` is kept so
// callers share the Zinnia (root, cachePath, npm) audit-plan shape.
function npmAuditPlan(root, cachePath, npm) {
  void root;
  return [
    {
      command: npm.command,
      args: [
        ...npm.prefixArgs,
        "audit",
        "--omit=dev",
        "--audit-level=high",
        "--ignore-scripts",
        `--cache=${cachePath}`,
      ],
    },
  ];
}

function npmUpdateInvocation(options = {}) {
  try {
    return npmInvocation(options);
  } catch (error) {
    const platform = options.platform ?? process.platform;
    if (
      platform === "win32" &&
      /npm_execpath is unavailable/u.test(String(error?.message || error))
    ) {
      const env = options.env ?? process.env;
      const execPath = options.execPath ?? process.execPath;
      const prefixes = [
        String(env.npm_config_prefix || "").trim(),
        env.APPDATA ? path.join(env.APPDATA, "npm") : "",
        path.dirname(execPath),
      ].filter(Boolean);
      for (const prefix of prefixes) {
        const cliPath = path.join(
          prefix,
          "node_modules",
          "npm",
          "bin",
          "npm-cli.js",
        );
        if (existsSync(cliPath)) {
          return { command: execPath, prefixArgs: [cliPath] };
        }
      }
      return { command: "npm.cmd", prefixArgs: [] };
    }
    throw error;
  }
}

function usesWindowsCmdShell(command) {
  return process.platform === "win32" && /\.cmd$/i.test(String(command));
}

function run(
  command,
  args,
  { cwd = process.cwd(), env = process.env, capture = false } = {},
) {
  const result = spawnSync(command, args, {
    cwd,
    env,
    encoding: "utf8",
    shell: usesWindowsCmdShell(command),
    windowsHide: true,
    stdio: capture ? "pipe" : "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = capture
      ? [result.stdout, result.stderr].filter(Boolean).join("\n").trim()
      : "";
    throw new Error(
      `${command} ${args.join(" ")} failed with exit code ${result.status}${detail ? `\n${detail}` : ""}`,
    );
  }
  return capture ? result.stdout.trim() : "";
}

function readSnapshot(filePath) {
  try {
    return { existed: true, bytes: readFileSync(filePath) };
  } catch (error) {
    if (error?.code === "ENOENT") return { existed: false, bytes: null };
    throw error;
  }
}

function snapshotMatches(filePath, snapshot) {
  const current = readSnapshot(filePath);
  return (
    current.existed === snapshot.existed &&
    (!current.existed || current.bytes.equals(snapshot.bytes))
  );
}

function restoreSnapshot(filePath, snapshot, expectedCurrent = null) {
  if (expectedCurrent && !snapshotMatches(filePath, expectedCurrent)) {
    throw new Error(
      "Concurrent package-lock edit detected; refusing to overwrite " +
        filePath,
    );
  }
  if (!snapshot.existed) {
    if (existsSync(filePath)) rmSync(filePath, { force: true });
    return;
  }

  const temporaryPath = `${filePath}.${process.pid}.${randomUUID()}.restore`;
  writeFileSync(temporaryPath, snapshot.bytes, { flag: "wx" });
  try {
    renameSync(temporaryPath, filePath);
  } finally {
    if (existsSync(temporaryPath)) rmSync(temporaryPath, { force: true });
  }
}

function acquireUpdateLock(root) {
  const lockPath = path.join(root, ".npm-safe-update.lock");
  const owner = `${process.pid}:${randomUUID()}\n`;
  let descriptor;
  try {
    descriptor = openSync(lockPath, "wx", 0o600);
    writeFileSync(descriptor, owner);
  } catch (error) {
    if (descriptor !== undefined) {
      closeSync(descriptor);
      if (existsSync(lockPath)) rmSync(lockPath, { force: true });
    }
    if (error?.code === "EEXIST") {
      throw new Error(`Another npm dependency update holds ${lockPath}`, {
        cause: error,
      });
    }
    throw error;
  }
  closeSync(descriptor);
  return () => {
    if (existsSync(lockPath) && readFileSync(lockPath, "utf8") === owner) {
      rmSync(lockPath, { force: true });
    }
  };
}

function assertUpdateEnvironment() {
  const nodeVersion = process.versions.node;
  if (!isSupportedNodeVersion(nodeVersion)) {
    throw new Error(
      `Node.js ${SUPPORTED_NODE_VERSIONS} required; found ${nodeVersion}`,
    );
  }

  const npm = npmUpdateInvocation();
  const npmVersion = run(npm.command, [...npm.prefixArgs, "--version"], {
    capture: true,
  });
  if (!isVersionAtLeast(npmVersion, MINIMUM_NPM_VERSION)) {
    throw new Error(
      `npm ${MINIMUM_NPM_VERSION}+ required; found ${npmVersion}`,
    );
  }

  const channel = pinnedRustChannel();
  const rustToolchains = run("rustup", ["toolchain", "list"], {
    capture: true,
  });
  if (!hasPinnedRustToolchain(rustToolchains, channel)) {
    throw new Error(
      `Rust ${channel} must already be installed before updating dependencies`,
    );
  }
}

function main() {
  assertUpdateEnvironment();
  const root = process.cwd();
  const npm = npmUpdateInvocation();
  const packageLock = path.join(root, "package-lock.json");
  const releaseUpdateLock = acquireUpdateLock(root);
  let tempRoot;

  try {
    const snapshot = readSnapshot(packageLock);
    tempRoot = mkdtempSync(path.join(os.tmpdir(), "npm-safe-update-"));
    const cachePath = path.join(tempRoot, "cache");
    const env = {
      ...process.env,
      npm_config_cache: cachePath,
      npm_config_ignore_scripts: "true",
      npm_config_min_release_age: "3",
    };
    if (
      process.platform === "win32" &&
      npm.command === process.execPath &&
      npm.prefixArgs[0]
    ) {
      env.npm_execpath = npm.prefixArgs[0];
    }

    try {
      run(npm.command, [...npm.prefixArgs, ...npmUpdateArguments(cachePath)], {
        cwd: root,
        env,
      });
    } catch (error) {
      restoreSnapshot(packageLock, snapshot);
      throw error;
    }

    const candidate = readSnapshot(packageLock);
    let auditError;
    try {
      for (const step of npmAuditPlan(root, cachePath, npm)) {
        run(step.command, step.args, { cwd: root, env });
      }
    } catch (error) {
      auditError = error;
    }

    if (!snapshotMatches(packageLock, candidate)) {
      throw new Error(
        "Concurrent package-lock edit detected after npm update; preserved " +
          packageLock,
        { cause: auditError },
      );
    }
    if (auditError) {
      restoreSnapshot(packageLock, snapshot, candidate);
      throw auditError;
    }
  } finally {
    try {
      if (tempRoot) rmSync(tempRoot, { recursive: true, force: true });
    } finally {
      releaseUpdateLock();
    }
  }
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`npm-safe-update: ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = { MINIMUM_NPM_VERSION, SUPPORTED_NODE_VERSIONS, parseVersion, isVersionAtLeast, isSupportedNodeVersion, pinnedRustChannel, hasPinnedRustToolchain, npmUpdateArguments, npmAuditPlan, npmInvocation, npmUpdateInvocation, usesWindowsCmdShell, restoreSnapshot, assertUpdateEnvironment };
