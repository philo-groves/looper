type ActuatorInput = {
  kind?: string;
  actuator?: string;
  args?: {
    text?: string;
  };
};

type ActuatorOutput = {
  status: "completed" | "failed" | "skipped";
  details: string;
  sensor_output?: string;
};

async function readInput(): Promise<string> {
  const decoder = new TextDecoder();
  const chunks: Uint8Array[] = [];
  for await (const chunk of Deno.stdin.readable) {
    chunks.push(chunk);
  }
  let total = 0;
  for (const chunk of chunks) total += chunk.length;
  const merged = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.length;
  }
  return decoder.decode(merged);
}

function inspectText(text: string): { details: string; sensor: string } {
  const lines = text.split(/\r?\n/);
  const words = text.trim().length === 0 ? 0 : text.trim().split(/\s+/).length;
  const chars = text.length;

  const secretPattern = /(api[_-]?key|token|secret|password|AKIA[0-9A-Z]{16})/i;
  const hasPotentialSecret = secretPattern.test(text);

  const details = `text inspection complete: ${chars} chars, ${words} words, ${lines.length} lines`;
  const sensor = [
    "sensor plugin_command_complete:",
    `actuator=text_inspect`,
    `chars=${chars}`,
    `words=${words}`,
    `lines=${lines.length}`,
    `potential_secret=${hasPotentialSecret ? "yes" : "no"}`,
  ].join(" ");

  return { details, sensor };
}

const raw = await readInput();
const payload = JSON.parse(raw) as ActuatorInput;

let output: ActuatorOutput;

if (payload.kind !== "actuator_execute") {
  output = {
    status: "skipped",
    details: "unsupported input kind",
  };
} else if (payload.actuator !== "text_inspect") {
  output = {
    status: "skipped",
    details: `unsupported actuator: ${payload.actuator ?? "unknown"}`,
  };
} else {
  const text = payload.args?.text ?? "";
  const { details, sensor } = inspectText(text);
  output = {
    status: "completed",
    details,
    sensor_output: sensor,
  };
}

console.log(JSON.stringify(output));
