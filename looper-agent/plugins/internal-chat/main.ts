type ChatPluginPerceptInput = {
  session_id: string;
  turn_id: string;
  text: string;
};

type ChatPluginEffectOutput = {
  mode: "stream_chat";
  user_prompt?: string;
  system_prompt?: string;
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
  if (!trimmed) {
    return {
      mode: "stream_chat",
      user_prompt: "The user submitted an empty message. Ask for clarification.",
      system_prompt: "You are Looper. Keep responses concise and practical.",
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
      task_completion: {
        status: "completed",
        details: `Task completion recorded for '${taskName}'.`,
      },
    };
  }

  return {
    mode: "stream_chat",
    user_prompt: trimmed,
    system_prompt: "You are Looper. Provide direct, useful answers in plain language.",
  };
}

const raw = await readInput();
const input = JSON.parse(raw) as ChatPluginPerceptInput;
const output = buildEffects(input);
console.log(JSON.stringify(output));
