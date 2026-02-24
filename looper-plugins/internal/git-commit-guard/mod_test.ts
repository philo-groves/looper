import { buildRiskSignalForTest } from "./mod.ts";

type RouteContract = {
  required?: string[];
  properties?: {
    looper_signal?: {
      const?: string;
    };
  };
};

function contractPath(): string {
  const url = new URL("../../contracts/plugin-route-v1.json", import.meta.url);
  let path = decodeURIComponent(url.pathname);
  if (Deno.build.os === "windows" && path.startsWith("/") && /^[A-Za-z]:/.test(path.slice(1))) {
    path = path.slice(1);
  }
  if (Deno.build.os === "windows") {
    return path.replaceAll("/", "\\");
  }
  return path;
}

Deno.test("risk signal matches shared contract shape", async () => {
  const contract = JSON.parse(
    await Deno.readTextFile(contractPath()),
  ) as RouteContract;

  const signal = await buildRiskSignalForTest(
    "abc1234def5678",
    "add env file",
    ["commit touches .env file"],
  );
  if (!signal) {
    throw new Error("expected test signal to be produced");
  }

  for (const field of contract.required ?? []) {
    if (!Object.prototype.hasOwnProperty.call(signal, field)) {
      throw new Error(`missing required contract field: ${field}`);
    }
  }

  const literal = contract.properties?.looper_signal?.const;
  if (typeof literal !== "string" || literal.length === 0) {
    throw new Error("contract looper_signal.const is missing");
  }
  if (signal.looper_signal !== literal) {
    throw new Error("signal literal does not match contract");
  }
});
