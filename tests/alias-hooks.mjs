import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

const src = new URL("../src/", import.meta.url);

export async function resolve(specifier, context, next) {
  if (specifier.startsWith("@/")) {
    const base = new URL(specifier.slice(2), src).href;
    for (const candidate of [`${base}.ts`, base, `${base}/index.ts`]) {
      if (existsSync(fileURLToPath(candidate))) {
        return { url: candidate, shortCircuit: true };
      }
    }
  }
  return next(specifier, context);
}
