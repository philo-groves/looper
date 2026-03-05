type ActuatorInput = {
  kind?: string;
  actuator?: string;
  args?: Record<string, unknown>;
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
  const total = chunks.reduce((n, chunk) => n + chunk.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return decoder.decode(out);
}

function runBlogOutline(args: Record<string, unknown>): ActuatorOutput {
  const topic = String(args.topic ?? "Untitled Topic");
  const audience = String(args.audience ?? "general audience");
  return {
    status: "completed",
    details: `outline scaffold prepared for '${topic}' targeting ${audience}`,
    sensor_output: [
      "sensor plugin_command_complete:",
      "actuator=blog_outline",
      `topic=${topic}`,
      `audience=${audience}`,
      "sections=intro,problem,insights,examples,conclusion",
    ].join(" "),
  };
}

function runDraftSummary(args: Record<string, unknown>): ActuatorOutput {
  const title = String(args.title ?? "Untitled Draft");
  const takeaways = Array.isArray(args.takeaways)
    ? args.takeaways.map((item) => String(item)).filter((item) => item.trim().length > 0)
    : [];

  return {
    status: "completed",
    details: `draft summary prepared for '${title}' with ${takeaways.length} key takeaway(s)`,
    sensor_output: [
      "sensor plugin_command_complete:",
      "actuator=blog_draft_summary",
      `title=${title}`,
      `takeaways=${takeaways.length}`,
    ].join(" "),
  };
}

const raw = await readInput();
const payload = JSON.parse(raw) as ActuatorInput;

if (payload.kind !== "actuator_execute") {
  console.log(JSON.stringify({ status: "skipped", details: "unsupported input kind" } satisfies ActuatorOutput));
  Deno.exit(0);
}

const args = payload.args ?? {};
let output: ActuatorOutput;
switch (payload.actuator) {
  case "blog_outline":
    output = runBlogOutline(args);
    break;
  case "blog_draft_summary":
    output = runDraftSummary(args);
    break;
  default:
    output = {
      status: "skipped",
      details: `unsupported actuator: ${payload.actuator ?? "unknown"}`,
    };
    break;
}

console.log(JSON.stringify(output));
