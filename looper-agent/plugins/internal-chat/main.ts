type ChatPluginPerceptInput = {
  session_id: string;
  turn_id: string;
  text: string;
};

type PlannedAction = {
  plugin?: string;
  actuator: string;
  args: Record<string, unknown>;
};

type ChatPluginEffectOutput = {
  mode: "stream_chat";
  user_prompt?: string;
  system_prompt?: string;
  planned_actions?: PlannedAction[];
  task_completion?: {
    status: string;
    details: string;
  };
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

function buildEffects(input: ChatPluginPerceptInput): ChatPluginEffectOutput {
  const trimmed = input.text.trim();
  const plannedActions = planActions(trimmed);

  if (!trimmed) {
    return {
      mode: "stream_chat",
      user_prompt: "The user submitted an empty message. Ask for clarification.",
      system_prompt: "You are Looper. Keep responses concise and practical.",
      planned_actions: [],
    };
  }

  if (trimmed.toLowerCase().startsWith("task:")) {
    const requested = trimmed.slice(5).trim();
    const taskName = requested || "unspecified task";
    return {
      mode: "stream_chat",
      user_prompt: `Please complete this task and describe the outcome briefly: ${taskName}`,
      system_prompt:
        "You are Looper. Execute user tasks carefully and return concrete completion details.",
      planned_actions: plannedActions,
      task_completion: {
        status: "completed",
        details: `Task completion recorded for '${taskName}'.`,
      },
    };
  }

  return {
    mode: "stream_chat",
    user_prompt: trimmed,
    system_prompt:
      "You are Looper. Provide direct, useful answers in plain language. If planned tool actions ran, use their outputs directly instead of asking the user to run commands.",
    planned_actions: plannedActions,
  };
}

function planActions(text: string): PlannedAction[] {
  const parsed = parseFilesystemCommand(text) ?? parseFilesystemRequest(text);
  return parsed ? [parsed] : [];
}

function parseFilesystemCommand(text: string): PlannedAction | null {
  const slashCommand = text.match(/^\/(grep|glob|glop|read)\s+(.+)$/i);
  if (slashCommand) {
    return parseFilesystemCommandParts(slashCommand[1], slashCommand[2]);
  }

  const bareCommand = text.match(/^(grep|glob|glop|read)\s+(.+)$/i);
  if (bareCommand) {
    return parseFilesystemCommandParts(bareCommand[1], bareCommand[2]);
  }

  return null;
}

function parseFilesystemCommandParts(
  keyword: string,
  rest: string,
): PlannedAction | null {
  const normalized = keyword.toLowerCase();
  if (normalized === "read") {
    const filePath = cleanToken(rest);
    if (!filePath) return null;
    return {
      plugin: "filesystem-read",
      actuator: "filesystem_read",
      args: {
        file_path: filePath,
        max_lines: 250,
      },
    };
  }

  const actuator = normalized === "grep"
    ? "filesystem_grep"
    : "filesystem_glob";

  const inMatch = rest.match(/^(.+?)\s+in\s+(.+)$/i);
  if (inMatch) {
    const pattern = cleanToken(inMatch[1]);
    const path = cleanToken(inMatch[2]);
    if (!pattern) return null;
    return {
      plugin: "filesystem-read",
      actuator,
      args: {
        pattern,
        path: path || ".",
        max_results: 200,
      },
    };
  }

  const pattern = cleanToken(rest);
  if (!pattern) return null;
  return {
    plugin: "filesystem-read",
    actuator,
    args: {
      pattern,
      path: ".",
      max_results: 200,
    },
  };
}

function parseFilesystemRequest(text: string): PlannedAction | null {
  const readMatch = text.match(
    /(?:read|open|show)(?:\s+me)?(?:\s+the)?(?:\s+contents?\s+of|\s+file)?\s+([./\\][^\s,;]+|[\w.-]+\.[\w.-]+)/i,
  );
  if (readMatch) {
    return {
      plugin: "filesystem-read",
      actuator: "filesystem_read",
      args: {
        file_path: cleanToken(readMatch[1]),
        max_lines: 250,
      },
    };
  }

  const grepMatch = text.match(
    /(?:search\s+for|find|grep)\s+["'`]?([^"'`]+)["'`]?\s+in\s+([./\\][^\s,;]+|[\w./\\-]+)/i,
  );
  if (grepMatch) {
    return {
      plugin: "filesystem-read",
      actuator: "filesystem_grep",
      args: {
        pattern: cleanToken(grepMatch[1]),
        path: cleanToken(grepMatch[2]),
        max_results: 200,
      },
    };
  }

  const globMatch = text.match(
    /(?:list|show)\s+(?:all\s+)?files?\s+(?:matching\s+)?["'`]?([^"'`]+)["'`]?(?:\s+in\s+([./\\][^\s,;]+|[\w./\\-]+))?/i,
  );
  if (globMatch && globMatch[1].includes("*")) {
    return {
      plugin: "filesystem-read",
      actuator: "filesystem_glob",
      args: {
        pattern: cleanToken(globMatch[1]),
        path: cleanToken(globMatch[2] || "."),
        max_results: 200,
      },
    };
  }

  return null;
}

function cleanToken(raw: string): string {
  const trimmed = raw.trim();
  if (
    (trimmed.startsWith("\"") && trimmed.endsWith("\"")) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1).trim();
  }
  return trimmed;
}

const raw = await readInput();
const input = JSON.parse(raw) as ChatPluginPerceptInput;
const output = buildEffects(input);
console.log(JSON.stringify(output));
