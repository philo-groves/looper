type PerceptInput = {
  session_id: string;
  turn_id: string;
  text: string;
};

type ActionPlan = {
  actuator: "filesystem_grep" | "filesystem_glob" | "filesystem_read";
  pattern?: string;
  path?: string;
  max_results?: number;
  file_path?: string;
  max_lines?: number;
};

type PluginPlan = {
  actions: ActionPlan[];
};

async function readInput(): Promise<string> {
  const decoder = new TextDecoder();
  const chunks: Uint8Array[] = [];
  for await (const chunk of Deno.stdin.readable) {
    chunks.push(chunk);
  }
  return decoder.decode(concatChunks(chunks));
}

function concatChunks(chunks: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const c of chunks) total += c.length;
  const out = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.length;
  }
  return out;
}

function parseAction(text: string): ActionPlan | null {
  const trimmed = text.trim();
  if (!trimmed) return null;

  const slashCommand = trimmed.match(/^\/(grep|glob|glop|read)\s+(.+)$/i);
  if (slashCommand) {
    return parseCommand(slashCommand[1], slashCommand[2]);
  }

  const bareCommand = trimmed.match(/^(grep|glob|glop|read)\s+(.+)$/i);
  if (bareCommand) {
    return parseCommand(bareCommand[1], bareCommand[2]);
  }

  return null;
}

function parseCommand(keyword: string, rest: string): ActionPlan | null {
  if (keyword.toLowerCase() === "read") {
    const filePath = cleanToken(rest);
    if (!filePath) return null;
    return { actuator: "filesystem_read", file_path: filePath, max_lines: 250 };
  }

  const actuator = keyword.toLowerCase() === "grep"
    ? "filesystem_grep"
    : "filesystem_glob";

  const inMatch = rest.match(/^(.+?)\s+in\s+(.+)$/i);
  if (inMatch) {
    const pattern = cleanToken(inMatch[1]);
    const path = cleanToken(inMatch[2]);
    if (!pattern) return null;
    return { actuator, pattern, path: path || ".", max_results: 200 };
  }

  const pattern = cleanToken(rest);
  if (!pattern) return null;
  return { actuator, pattern, path: ".", max_results: 200 };
}

function cleanToken(raw: string): string {
  const trimmed = raw.trim();
  if ((trimmed.startsWith("\"") && trimmed.endsWith("\"")) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))) {
    return trimmed.slice(1, -1).trim();
  }
  return trimmed;
}

const raw = await readInput();
const input = JSON.parse(raw) as PerceptInput;
const action = parseAction(input.text);
const output: PluginPlan = {
  actions: action ? [action] : [],
};

console.log(JSON.stringify(output));
