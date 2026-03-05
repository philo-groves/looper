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

function asArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.map((item) => String(item)).filter((item) => item.trim().length > 0);
}

function runCyberTriage(args: Record<string, unknown>): ActuatorOutput {
  const target = String(args.target ?? "unknown-target");
  const observations = asArray(args.observations);
  const topSignals = observations.slice(0, 5);
  const details =
    topSignals.length === 0
      ? `cyber triage completed for ${target}; no observations supplied`
      : `cyber triage completed for ${target}; ${topSignals.length} high-priority observation(s) summarized`;

  return {
    status: "completed",
    details,
    sensor_output: [
      "sensor plugin_command_complete:",
      "actuator=cyber_triage",
      `target=${target}`,
      `signals=${topSignals.length}`,
      "confidence=initial",
      topSignals.length > 0 ? `top_signal=${topSignals[0]}` : "top_signal=none",
    ].join(" "),
  };
}

function runCyberReportOutline(args: Record<string, unknown>): ActuatorOutput {
  const title = String(args.title ?? "Security Assessment");
  const findings = asArray(args.findings);
  return {
    status: "completed",
    details: `report outline prepared for '${title}' with ${findings.length} finding input(s)`,
    sensor_output: [
      "sensor plugin_command_complete:",
      "actuator=cyber_report_outline",
      `title=${title}`,
      `finding_count=${findings.length}`,
      "sections=executive_summary,scope,findings,recommendations",
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
  case "cyber_triage":
    output = runCyberTriage(args);
    break;
  case "cyber_report_outline":
    output = runCyberReportOutline(args);
    break;
  default:
    output = {
      status: "skipped",
      details: `unsupported actuator: ${payload.actuator ?? "unknown"}`,
    };
    break;
}

console.log(JSON.stringify(output));
