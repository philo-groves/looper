type Payload = {
  mode?: string;
  plugin?: string;
  sensor?: string;
  actuator?: string;
  action?: unknown;
  workspace_root?: string;
};

type PluginState = {
  lastSeenHash?: string;
  lastActuatorSeenHash?: string;
  notifiedHashes?: string[];
};

type CommitInfo = {
  hash: string;
  subject: string;
};

type RouteContract = {
  required?: string[];
  properties?: {
    looper_signal?: {
      const?: string;
    };
  };
};

const SENSOR_ID = "git_commits";
const ACTUATOR_ID = "desktop_notify_secrets";
const decoder = new TextDecoder();
let contractCache: RouteContract | null = null;

function parsePayload(): Payload {
  const index = Deno.args.indexOf("--looper-payload");
  if (index < 0 || index + 1 >= Deno.args.length) {
    return {};
  }

  try {
    return JSON.parse(Deno.args[index + 1]);
  } catch {
    return {};
  }
}

function hasOwn(input: Record<string, unknown>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(input, key);
}

async function buildRiskSignal(commit: CommitInfo, reasons: string[]): Promise<string | null> {
  const looperSignal = await routeSignalLiteral();
  const signal: Record<string, unknown> = {
    looper_signal: looperSignal,
    event: "new_risky_commit",
    plugin: "git_commit_guard",
    route_to_actuator: "git_commit_guard:desktop_notify_secrets",
    action_message: `Notify desktop about risky commit ${commit.hash.slice(0, 7)}.`,
    commit_hash: commit.hash,
    commit_subject: commit.subject,
    reasons,
  };

  if (!(await matchesRouteContract(signal))) {
    return null;
  }

  return JSON.stringify(signal);
}

export async function buildRiskSignalForTest(
  hash: string,
  subject: string,
  reasons: string[],
): Promise<Record<string, unknown> | null> {
  const encoded = await buildRiskSignal({ hash, subject }, reasons);
  if (!encoded) {
    return null;
  }
  return JSON.parse(encoded) as Record<string, unknown>;
}

function pathFromImportMeta(relative: string): string {
  const url = new URL(relative, import.meta.url);
  let path = decodeURIComponent(url.pathname);
  if (Deno.build.os === "windows" && path.startsWith("/") && /^[A-Za-z]:/.test(path.slice(1))) {
    path = path.slice(1);
  }
  if (Deno.build.os === "windows") {
    return path.replaceAll("/", "\\");
  }
  return path;
}

function contractPath(): string {
  return pathFromImportMeta("../../contracts/plugin-route-v1.json");
}

async function readRouteContract(): Promise<RouteContract> {
  if (contractCache) {
    return contractCache;
  }

  try {
    const raw = await Deno.readTextFile(contractPath());
    const parsed = JSON.parse(raw) as RouteContract;
    contractCache = parsed;
    return parsed;
  } catch {
    contractCache = {};
    return contractCache;
  }
}

async function routeSignalLiteral(): Promise<string> {
  const contract = await readRouteContract();
  const value = contract.properties?.looper_signal?.const;
  if (typeof value === "string" && value.trim().length > 0) {
    return value.trim();
  }
  return "plugin_route_v1";
}

async function matchesRouteContract(signal: Record<string, unknown>): Promise<boolean> {
  const contract = await readRouteContract();
  const required = Array.isArray(contract.required) ? contract.required : [];
  for (const field of required) {
    if (!hasOwn(signal, field)) {
      return false;
    }
  }

  const literal = await routeSignalLiteral();
  return signal.looper_signal === literal;
}

function statePath(): string {
  return pathFromImportMeta("./.state.json");
}

async function readState(): Promise<PluginState> {
  try {
    const raw = await Deno.readTextFile(statePath());
    const parsed = JSON.parse(raw) as PluginState;
    return {
      lastSeenHash: parsed.lastSeenHash,
      lastActuatorSeenHash: parsed.lastActuatorSeenHash,
      notifiedHashes: Array.isArray(parsed.notifiedHashes) ? parsed.notifiedHashes : [],
    };
  } catch {
    return { notifiedHashes: [] };
  }
}

async function writeState(state: PluginState): Promise<void> {
  await Deno.writeTextFile(statePath(), JSON.stringify(state, null, 2));
}

async function runCommand(command: string, args: string[], cwd: string): Promise<{ ok: boolean; stdout: string; stderr: string }> {
  const output = await new Deno.Command(command, {
    args,
    cwd,
    stdout: "piped",
    stderr: "piped",
  }).output();

  return {
    ok: output.success,
    stdout: decoder.decode(output.stdout).trim(),
    stderr: decoder.decode(output.stderr).trim(),
  };
}

async function ensureGitRepo(workspaceRoot: string): Promise<boolean> {
  const result = await runCommand("git", ["rev-parse", "--is-inside-work-tree"], workspaceRoot);
  return result.ok && result.stdout.toLowerCase() === "true";
}

async function recentCommits(workspaceRoot: string, limit: number): Promise<CommitInfo[]> {
  const result = await runCommand(
    "git",
    ["log", "--pretty=format:%H%x09%s", "-n", String(limit)],
    workspaceRoot,
  );
  if (!result.ok || !result.stdout) {
    return [];
  }

  return result.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => {
      const tab = line.indexOf("\t");
      if (tab < 0) {
        return { hash: line, subject: "(no subject)" };
      }
      return {
        hash: line.slice(0, tab),
        subject: line.slice(tab + 1),
      };
    });
}

function riskyEnvPath(path: string): boolean {
  const lower = path.toLowerCase();
  return (
    lower === ".env" ||
    lower.endsWith("/.env") ||
    lower.endsWith("\\.env") ||
    lower.includes("/.env.") ||
    lower.includes("\\.env.")
  );
}

async function commitFiles(workspaceRoot: string, hash: string): Promise<string[]> {
  const result = await runCommand("git", ["show", "--name-only", "--pretty=format:", hash], workspaceRoot);
  if (!result.ok || !result.stdout) {
    return [];
  }
  return result.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

async function commitPatch(workspaceRoot: string, hash: string): Promise<string> {
  const result = await runCommand("git", ["show", "--pretty=format:", "--unified=0", hash], workspaceRoot);
  if (!result.ok) {
    return "";
  }
  return result.stdout;
}

async function commitRisk(workspaceRoot: string, hash: string): Promise<string[]> {
  const reasons: string[] = [];
  const files = await commitFiles(workspaceRoot, hash);
  if (files.some(riskyEnvPath)) {
    reasons.push("commit touches .env file");
  }

  const patch = await commitPatch(workspaceRoot, hash);
  if (patch.length > 0) {
    const addedLines = patch
      .split("\n")
      .filter((line) => line.startsWith("+") && !line.startsWith("+++"));

    const secretPattern = /(api[_-]?key|secret|token|password|private[_-]?key)\s*[:=]/i;
    if (addedLines.some((line) => secretPattern.test(line))) {
      reasons.push("commit adds a likely secret value");
    }
  }

  return reasons;
}

function quotePowerShell(input: string): string {
  return input.replaceAll("'", "''");
}

function quoteAppleScript(input: string): string {
  return input.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
}

async function notifyDesktop(title: string, body: string, workspaceRoot: string): Promise<boolean> {
  if (Deno.build.os === "windows") {
    const script =
      "Import-Module BurntToast -ErrorAction SilentlyContinue; " +
      `if (Get-Command New-BurntToastNotification -ErrorAction SilentlyContinue) { New-BurntToastNotification -Text '${quotePowerShell(title)}', '${quotePowerShell(body)}' } else { msg * '${quotePowerShell(`${title}: ${body}`)}' }`;
    const result = await runCommand("powershell", ["-NoProfile", "-Command", script], workspaceRoot);
    return result.ok;
  }

  if (Deno.build.os === "darwin") {
    const script = `display notification \"${quoteAppleScript(body)}\" with title \"${quoteAppleScript(title)}\"`;
    const result = await runCommand("osascript", ["-e", script], workspaceRoot);
    return result.ok;
  }

  const result = await runCommand("notify-send", [title, body], workspaceRoot);
  return result.ok;
}

async function handleSensor(payload: Payload): Promise<unknown> {
  if (payload.sensor !== SENSOR_ID) {
    return { percepts: [] };
  }

  const workspaceRoot = payload.workspace_root ?? Deno.cwd();
  if (!(await ensureGitRepo(workspaceRoot))) {
    return { percepts: [] };
  }

  const commits = await recentCommits(workspaceRoot, 25);
  if (commits.length === 0) {
    return { percepts: [] };
  }

  const state = await readState();
  const latestHash = commits[0].hash;

  if (!state.lastSeenHash) {
    state.lastSeenHash = latestHash;
    await writeState(state);
    return { percepts: [] };
  }

  let fresh = commits;
  const boundary = commits.findIndex((commit) => commit.hash === state.lastSeenHash);
  if (boundary >= 0) {
    fresh = commits.slice(0, boundary);
  }

  const percepts: string[] = [];
  for (const commit of fresh.reverse()) {
    const reasons = await commitRisk(workspaceRoot, commit.hash);
    if (reasons.length > 0) {
      const signal = await buildRiskSignal(commit, reasons);
      if (signal) {
        percepts.push(signal);
      }
    }
  }

  state.lastSeenHash = latestHash;
  await writeState(state);

  return { percepts };
}

async function handleActuator(payload: Payload): Promise<unknown> {
  if (payload.actuator !== ACTUATOR_ID) {
    return { output: "ignored: unknown actuator" };
  }

  const workspaceRoot = payload.workspace_root ?? Deno.cwd();
  if (!(await ensureGitRepo(workspaceRoot))) {
    return { output: "no git repository at workspace root" };
  }

  const commits = await recentCommits(workspaceRoot, 10);
  if (commits.length === 0) {
    return { output: "no commits found" };
  }

  const state = await readState();
  const latestHash = commits[0].hash;

  if (!state.lastActuatorSeenHash) {
    state.lastActuatorSeenHash = latestHash;
    await writeState(state);
    return { output: "baseline established; waiting for new commits" };
  }

  let fresh = commits;
  const boundary = commits.findIndex((commit) => commit.hash === state.lastActuatorSeenHash);
  if (boundary >= 0) {
    fresh = commits.slice(0, boundary);
  }

  const notified = new Set(state.notifiedHashes ?? []);
  const sent: string[] = [];

  for (const commit of fresh.reverse()) {
    if (notified.has(commit.hash)) {
      continue;
    }

    const reasons = await commitRisk(workspaceRoot, commit.hash);
    if (reasons.length === 0) {
      continue;
    }

    const shortHash = commit.hash.slice(0, 7);
    const title = "Looper: risky git commit";
    const body = `${shortHash} ${commit.subject} (${reasons.join("; ")})`;
    const ok = await notifyDesktop(title, body, workspaceRoot);
    if (ok) {
      sent.push(shortHash);
      notified.add(commit.hash);
    }
  }

  state.lastActuatorSeenHash = latestHash;
  state.notifiedHashes = Array.from(notified).slice(-200);
  await writeState(state);

  if (sent.length === 0) {
    return { output: "no new risky commits to notify" };
  }

  return {
    output: `desktop notifications sent for commits: ${sent.join(", ")}`,
    notifications_sent: sent.length,
  };
}

async function main(): Promise<void> {
  const payload = parsePayload();
  let result: unknown;

  if (payload.mode === "sensor") {
    result = await handleSensor(payload);
  } else if (payload.mode === "actuator") {
    result = await handleActuator(payload);
  } else {
    result = { output: "unsupported mode" };
  }

  console.log(JSON.stringify(result));
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(message);
    Deno.exit(1);
  }
}
