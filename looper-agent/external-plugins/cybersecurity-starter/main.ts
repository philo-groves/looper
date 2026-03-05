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

type FindingSeverity = "critical" | "high" | "medium" | "low" | "info";
type FindingConfidence = "initial" | "low" | "medium" | "high";
type FindingStatus = "candidate" | "validated" | "needs_follow_up";

type EvidenceArtifact = {
  evidence_id: string;
  kind: "log" | "request" | "response" | "snippet" | "metadata";
  summary: string;
  captured_at: string;
};

type SecurityFinding = {
  finding_id: string;
  target: string;
  vector: string;
  severity: FindingSeverity;
  confidence: FindingConfidence;
  impact: string;
  status: FindingStatus;
  evidence: EvidenceArtifact[];
  repro_steps: string[];
  created_at: string;
};

type Hypothesis = {
  hypothesis_id: string;
  target: string;
  finding_ref: string;
  statement: string;
  confidence: FindingConfidence;
  created_at: string;
};

type Asset = {
  asset_id: string;
  kind: "host" | "domain" | "path" | "repository";
  value: string;
  source: string;
};

function nowIso(): string {
  return new Date().toISOString();
}

function stableId(prefix: string, seed: string): string {
  let hash = 2166136261;
  for (let i = 0; i < seed.length; i++) {
    hash ^= seed.charCodeAt(i);
    hash += (hash << 1) + (hash << 4) + (hash << 7) + (hash << 8) + (hash << 24);
  }
  const normalized = Math.abs(hash >>> 0).toString(16).padStart(8, "0");
  return `${prefix}-${normalized}`;
}

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

function asRecordArray(value: unknown): Record<string, unknown>[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry) => typeof entry === "object" && entry !== null) as Record<string, unknown>[];
}

function getString(args: Record<string, unknown>, key: string, fallback = ""): string {
  const value = args[key];
  if (typeof value === "string" && value.trim().length > 0) {
    return value.trim();
  }
  return fallback;
}

function extractHost(target: string): string {
  try {
    const url = target.includes("://") ? new URL(target) : new URL(`https://${target}`);
    return url.hostname;
  } catch {
    return target.replace(/^https?:\/\//i, "").split("/")[0] || target;
  }
}

function pickVector(observation: string): string {
  const lowered = observation.toLowerCase();
  if (/(sqli|sql injection|union select|database error)/.test(lowered)) return "injection";
  if (/(xss|script>|onerror=|<img)/.test(lowered)) return "xss";
  if (/(auth|idor|broken access|unauthorized|forbidden bypass)/.test(lowered)) return "access_control";
  if (/(ssrf|metadata endpoint|169\.254\.169\.254)/.test(lowered)) return "ssrf";
  if (/(secret|token|apikey|password)/.test(lowered)) return "secrets_exposure";
  return "misconfiguration";
}

function pickSeverity(observation: string): FindingSeverity {
  const lowered = observation.toLowerCase();
  if (/(rce|remote code execution|account takeover|critical)/.test(lowered)) return "critical";
  if (/(sql injection|idor|auth bypass|ssrf|high)/.test(lowered)) return "high";
  if (/(xss|csrf|medium|sensitive data)/.test(lowered)) return "medium";
  if (/(info|informational|header|banner)/.test(lowered)) return "info";
  return "low";
}

function findingFromObservation(target: string, observation: string): SecurityFinding {
  const created_at = nowIso();
  const vector = pickVector(observation);
  const severity = pickSeverity(observation);
  const evidence: EvidenceArtifact[] = [{
    evidence_id: stableId("evi", `${target}:${observation}`),
    kind: "snippet",
    summary: observation,
    captured_at: created_at,
  }];

  return {
    finding_id: stableId("finding", `${target}:${vector}:${observation}`),
    target,
    vector,
    severity,
    confidence: "initial",
    impact: `Potential ${vector} risk observed for ${target}`,
    status: "candidate",
    evidence,
    repro_steps: [
      "Collect and preserve request/response or log evidence for this observation",
      "Confirm behavior is reproducible in a controlled passive test",
    ],
    created_at,
  };
}

function findingFromRecord(record: Record<string, unknown>): SecurityFinding {
  const target = getString(record, "target", "unknown-target");
  const vector = getString(record, "vector", "misconfiguration");
  const created_at = getString(record, "created_at", nowIso());
  const finding_id = getString(record, "finding_id", stableId("finding", `${target}:${vector}:${created_at}`));
  const evidence = asRecordArray(record.evidence).map((entry, idx) => ({
    evidence_id: getString(entry, "evidence_id", `${finding_id}-e${idx + 1}`),
    kind: (getString(entry, "kind", "metadata") as EvidenceArtifact["kind"]),
    summary: getString(entry, "summary", "evidence item"),
    captured_at: getString(entry, "captured_at", created_at),
  }));
  const repro_steps = asArray(record.repro_steps);

  return {
    finding_id,
    target,
    vector,
    severity: (getString(record, "severity", "low") as FindingSeverity),
    confidence: (getString(record, "confidence", "initial") as FindingConfidence),
    impact: getString(record, "impact", `Potential ${vector} risk observed for ${target}`),
    status: (getString(record, "status", "candidate") as FindingStatus),
    evidence,
    repro_steps,
    created_at,
  };
}

function buildSensorOutput(actuator: string, payload: Record<string, unknown>): string {
  return [
    "sensor plugin_command_complete:",
    `actuator=${actuator}`,
    `payload=${JSON.stringify(payload)}`,
  ].join(" ");
}

function runCyberSurfaceMap(args: Record<string, unknown>): ActuatorOutput {
  const target = getString(args, "target", "unknown-target");
  const host = extractHost(target);
  const repoPaths = asArray(args.repo_paths);
  const timestamp = nowIso();
  const correlation_id = stableId("corr", `surface:${target}:${timestamp}`);

  const assets: Asset[] = [
    {
      asset_id: stableId("asset", `host:${host}`),
      kind: "host",
      value: host,
      source: "target_input",
    },
    {
      asset_id: stableId("asset", `domain:${host}`),
      kind: "domain",
      value: host,
      source: "target_inference",
    },
    ...repoPaths.map((path) => ({
      asset_id: stableId("asset", `repo:${path}`),
      kind: "repository" as const,
      value: path,
      source: "repo_paths",
    })),
  ];

  const payload = {
    correlation_id,
    timestamp,
    target,
    asset_count: assets.length,
    assets,
    mode: "passive",
  };

  return {
    status: "completed",
    details: `surface mapping completed for ${target}; discovered ${assets.length} asset(s) in passive mode`,
    sensor_output: buildSensorOutput("cyber_surface_map", payload),
  };
}

function runCyberEndpointInventory(args: Record<string, unknown>): ActuatorOutput {
  const target = getString(args, "target", "unknown-target");
  const hints = asArray(args.hints);
  const defaultEndpoints = ["/", "/login", "/api", "/api/v1", "/health", "/admin"];
  const inferred = hints
    .flatMap((hint) => {
      const matches = hint.match(/\/[a-zA-Z0-9_./-]*/g);
      return matches ? matches : [];
    })
    .filter((entry) => entry.length > 1);

  const endpointSet = new Set<string>([...defaultEndpoints, ...inferred]);
  const endpoints = Array.from(endpointSet).slice(0, 25);
  const payload = {
    correlation_id: stableId("corr", `inventory:${target}:${endpoints.length}`),
    timestamp: nowIso(),
    target,
    endpoint_count: endpoints.length,
    endpoints,
    mode: "passive",
  };

  return {
    status: "completed",
    details: `endpoint inventory prepared for ${target}; ${endpoints.length} candidate endpoint(s)`,
    sensor_output: buildSensorOutput("cyber_endpoint_inventory", payload),
  };
}

function runCyberTechFingerprint(args: Record<string, unknown>): ActuatorOutput {
  const target = getString(args, "target", "unknown-target");
  const stackHints = asArray(args.stack_hints).map((entry) => entry.toLowerCase());
  const families = {
    frontend: stackHints.filter((entry) => /(react|vue|angular|svelte|next|nuxt)/.test(entry)),
    backend: stackHints.filter((entry) => /(node|express|nestjs|django|flask|spring|rails|laravel)/.test(entry)),
    infra: stackHints.filter((entry) => /(nginx|apache|cloudflare|aws|gcp|azure|kubernetes)/.test(entry)),
  };

  const payload = {
    correlation_id: stableId("corr", `fingerprint:${target}`),
    timestamp: nowIso(),
    target,
    tech_families: families,
    mode: "passive",
  };

  return {
    status: "completed",
    details: `technology fingerprint prepared for ${target}; ${stackHints.length} hint(s) processed`,
    sensor_output: buildSensorOutput("cyber_tech_fingerprint", payload),
  };
}

function runCyberTriage(args: Record<string, unknown>): ActuatorOutput {
  const target = getString(args, "target", "unknown-target");
  const observations = asArray(args.observations);
  const findings = observations.slice(0, 10).map((observation) => findingFromObservation(target, observation));
  const payload = {
    correlation_id: stableId("corr", `triage:${target}:${findings.length}`),
    timestamp: nowIso(),
    target,
    finding_count: findings.length,
    findings,
    mode: "passive",
  };

  return {
    status: "completed",
    details: findings.length === 0
      ? `cyber triage completed for ${target}; no observations supplied`
      : `cyber triage completed for ${target}; ${findings.length} candidate finding(s) generated`,
    sensor_output: buildSensorOutput("cyber_triage", payload),
  };
}

function runCyberHypothesisGenerate(args: Record<string, unknown>): ActuatorOutput {
  const target = getString(args, "target", "unknown-target");
  const records = asRecordArray(args.findings);
  const findings = records.map((record) => findingFromRecord(record)).slice(0, 10);
  const hypotheses: Hypothesis[] = findings.map((finding) => ({
    hypothesis_id: stableId("hyp", `${finding.finding_id}:${finding.vector}`),
    target,
    finding_ref: finding.finding_id,
    statement: `Investigate whether ${finding.vector} behavior for ${finding.target} is reproducible with passive evidence collection`,
    confidence: finding.confidence,
    created_at: nowIso(),
  }));

  const payload = {
    correlation_id: stableId("corr", `hypothesis:${target}:${hypotheses.length}`),
    timestamp: nowIso(),
    target,
    hypothesis_count: hypotheses.length,
    hypotheses,
    mode: "passive",
  };

  return {
    status: "completed",
    details: `generated ${hypotheses.length} validation hypothesis/hypotheses for ${target}`,
    sensor_output: buildSensorOutput("cyber_hypothesis_generate", payload),
  };
}

function runCyberValidateFinding(args: Record<string, unknown>): ActuatorOutput {
  const target = getString(args, "target", "unknown-target");
  const records = asRecordArray(args.findings);
  const findings = records.length > 0
    ? records.map((record) => findingFromRecord(record))
    : asArray(args.observations).map((observation) => findingFromObservation(target, observation));

  const validated = findings.map((finding) => {
    const hasEvidence = finding.evidence.length > 0;
    const hasRepro = finding.repro_steps.length > 0;
    const status: FindingStatus = hasEvidence && hasRepro ? "validated" : "needs_follow_up";
    const confidence: FindingConfidence = hasEvidence && hasRepro ? "medium" : "low";
    return {
      ...finding,
      status,
      confidence,
    };
  });

  const validCount = validated.filter((finding) => finding.status === "validated").length;
  const payload = {
    correlation_id: stableId("corr", `validate:${target}:${validated.length}`),
    timestamp: nowIso(),
    target,
    validated_count: validCount,
    follow_up_count: validated.length - validCount,
    findings: validated,
    mode: "passive_validation_only",
  };

  return {
    status: "completed",
    details: `validation pass complete for ${target}; ${validCount}/${validated.length} finding(s) validated`,
    sensor_output: buildSensorOutput("cyber_validate_finding", payload),
  };
}

function runCyberReportOutline(args: Record<string, unknown>): ActuatorOutput {
  const title = getString(args, "title", "Security Assessment");
  const findings = asRecordArray(args.findings).map((record) => findingFromRecord(record));
  const payload = {
    correlation_id: stableId("corr", `outline:${title}:${findings.length}`),
    timestamp: nowIso(),
    title,
    finding_count: findings.length,
    sections: [
      "executive_summary",
      "scope_and_method",
      "validated_findings",
      "findings_needing_follow_up",
      "recommendations",
      "evidence_index",
    ],
  };

  return {
    status: "completed",
    details: `report outline prepared for '${title}' with ${findings.length} finding input(s)`,
    sensor_output: buildSensorOutput("cyber_report_outline", payload),
  };
}

function runCyberReportDraft(args: Record<string, unknown>): ActuatorOutput {
  const title = getString(args, "title", "Security Assessment");
  const findings = asRecordArray(args.findings).map((record) => findingFromRecord(record));
  const validated = findings.filter((finding) => finding.status === "validated");
  const followUp = findings.filter((finding) => finding.status !== "validated");
  const recommendationCount = Math.max(1, findings.length);

  const payload = {
    correlation_id: stableId("corr", `draft:${title}:${findings.length}`),
    timestamp: nowIso(),
    title,
    validated_count: validated.length,
    follow_up_count: followUp.length,
    recommendation_count: recommendationCount,
    mode: "passive",
  };

  return {
    status: "completed",
    details: `report draft metadata generated for '${title}'; validated=${validated.length}, follow_up=${followUp.length}`,
    sensor_output: buildSensorOutput("cyber_report_draft", payload),
  };
}

function runCyberReportEvidencePack(args: Record<string, unknown>): ActuatorOutput {
  const findings = asRecordArray(args.findings).map((record) => findingFromRecord(record));
  const evidence = findings.flatMap((finding) => finding.evidence.map((entry) => ({
    finding_id: finding.finding_id,
    evidence_id: entry.evidence_id,
    summary: entry.summary,
    captured_at: entry.captured_at,
  })));

  const payload = {
    correlation_id: stableId("corr", `evidence:${findings.length}:${evidence.length}`),
    timestamp: nowIso(),
    evidence_count: evidence.length,
    findings_with_evidence: new Set(evidence.map((item) => item.finding_id)).size,
    evidence,
  };

  return {
    status: "completed",
    details: `evidence pack prepared with ${evidence.length} artifact(s)`,
    sensor_output: buildSensorOutput("cyber_report_evidence_pack", payload),
  };
}

function runCyberReportExecSummary(args: Record<string, unknown>): ActuatorOutput {
  const title = getString(args, "title", "Security Assessment");
  const findings = asRecordArray(args.findings).map((record) => findingFromRecord(record));
  const criticalOrHigh = findings.filter((finding) => finding.severity === "critical" || finding.severity === "high").length;
  const validated = findings.filter((finding) => finding.status === "validated").length;

  const payload = {
    correlation_id: stableId("corr", `exec-summary:${title}:${findings.length}`),
    timestamp: nowIso(),
    title,
    total_findings: findings.length,
    critical_high_count: criticalOrHigh,
    validated_count: validated,
    summary_points: [
      `Total findings reviewed: ${findings.length}`,
      `Critical/high findings: ${criticalOrHigh}`,
      `Validated findings: ${validated}`,
      "Assessment operated in passive-first validation mode",
    ],
  };

  return {
    status: "completed",
    details: `executive summary prepared for '${title}'`,
    sensor_output: buildSensorOutput("cyber_report_exec_summary", payload),
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
  case "cyber_surface_map":
    output = runCyberSurfaceMap(args);
    break;
  case "cyber_endpoint_inventory":
    output = runCyberEndpointInventory(args);
    break;
  case "cyber_tech_fingerprint":
    output = runCyberTechFingerprint(args);
    break;
  case "cyber_triage":
    output = runCyberTriage(args);
    break;
  case "cyber_hypothesis_generate":
    output = runCyberHypothesisGenerate(args);
    break;
  case "cyber_validate_finding":
    output = runCyberValidateFinding(args);
    break;
  case "cyber_report_outline":
    output = runCyberReportOutline(args);
    break;
  case "cyber_report_draft":
    output = runCyberReportDraft(args);
    break;
  case "cyber_report_evidence_pack":
    output = runCyberReportEvidencePack(args);
    break;
  case "cyber_report_exec_summary":
    output = runCyberReportExecSummary(args);
    break;
  default:
    output = {
      status: "skipped",
      details: `unsupported actuator: ${payload.actuator ?? "unknown"}`,
    };
    break;
}

console.log(JSON.stringify(output));
