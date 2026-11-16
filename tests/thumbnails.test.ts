// Run with `pnpm test` (node --test with TypeScript type stripping).
//
// Models Tauri's channel transport: payloads above ~8 KiB of JSON are
// delivered through a separate async fetch, so a batch can land after the
// command's promise has resolved. The stores chain work (CLIP indexing) right
// after loadThumbnails, so the thumbnail map must be complete at that point.
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";

(globalThis as any).window = globalThis;
const { mockIPC, clearMocks } = await import("@tauri-apps/api/mocks");
const { generateLibraryThumbnails, generateThumbnails } = await import(
  "../src/lib/commands.ts"
);
const { fillThumbnails } = await import("../src/lib/thumbnails.ts");

type Entry = { image_id: string; thumbnail_path: string };

function entries(n: number): Entry[] {
  return Array.from({ length: n }, (_, i) => ({
    image_id: `DSCF${i}`,
    thumbnail_path: `/cache/DSCF${i}.v3.jpg`,
  }));
}

const inFlight: Promise<void>[] = [];

/** Deliver channel messages the way the fetch path does: after a delay. */
function deliverLater(channel: any, messages: unknown[], delayMs: number) {
  const run = (window as any).__TAURI_INTERNALS__.runCallback;
  inFlight.push(
    new Promise((resolve) =>
      setTimeout(() => {
        messages.forEach((message, index) => run(channel.id, { index, message }));
        resolve();
      }, delayMs)
    )
  );
}

/** Mock a thumbnail command: return the result now, deliver batches late. */
function mockCommand(
  name: string,
  items: Entry[],
  failed: string[] = [],
  delayMs = 30
) {
  mockIPC((cmd, args: any) => {
    if (cmd !== name) throw new Error(`unexpected ${cmd}`);
    const total = items.length + failed.length;
    const batches = [];
    for (let i = 0; i < items.length; i += 1000) {
      batches.push({
        items: items.slice(i, i + 1000),
        failed: [],
        completed: Math.min(i + 1000, items.length),
        total,
      });
    }
    if (failed.length) {
      batches.push({ items: [], failed, completed: total, total });
    }
    deliverLater(args.onProgress, batches, delayMs);
    return { items, failed };
  });
}

beforeEach(() => clearMocks());
afterEach(async () => {
  await Promise.all(inFlight.splice(0));
  clearMocks();
});

test("harness reproduces the race: channel batches alone are late", async () => {
  // This is what the stores did at c3624bb: fill the map from channel
  // batches only, then read it as soon as the command resolves.
  mockCommand("generate_library_thumbnails", entries(1000));
  const paths = new Map<string, string>();
  await generateLibraryThumbnails(["/a"], ["a"], "/cache", (batch) => {
    for (const item of batch.items) paths.set(item.image_id, item.thumbnail_path);
  });
  assert.equal(paths.size, 0, "batches had not arrived when the command resolved");
});

test("library: every path is present when fillThumbnails resolves", async () => {
  const items = entries(2500);
  mockCommand("generate_library_thumbnails", items);
  const paths = new Map<string, string>();
  const progress: number[] = [];
  await fillThumbnails(
    (onBatch) => generateLibraryThumbnails(["/a"], ["a"], "/cache", onBatch),
    { paths, onProgress: (completed) => progress.push(completed) }
  );
  assert.equal(paths.size, 2500);
  assert.equal(paths.get("DSCF2499"), "/cache/DSCF2499.v3.jpg");
  assert.deepEqual(progress, [2500]);

  // Late batches only repeat entries and do not move progress backwards.
  await new Promise((r) => setTimeout(r, 60));
  assert.equal(paths.size, 2500);
  assert.deepEqual(progress, [2500]);
});

test("camera: failed ids are reported when fillThumbnails resolves", async () => {
  mockCommand("generate_thumbnails", entries(3), ["BROKEN"]);
  const paths = new Map<string, string>();
  const failed = new Set<string>();
  await fillThumbnails(
    (onBatch) => generateThumbnails(["x"], ["/x.HIF"], "/cache", onBatch),
    { paths, failed }
  );
  assert.equal(paths.size, 3);
  assert.deepEqual([...failed], ["BROKEN"]);
});

test("empty input resolves", async () => {
  mockCommand("generate_library_thumbnails", []);
  const paths = new Map<string, string>();
  await fillThumbnails(
    (onBatch) => generateLibraryThumbnails([], [], "/cache", onBatch),
    { paths }
  );
  assert.equal(paths.size, 0);
});

test("a failing command rejects instead of leaving the caller pending", async () => {
  mockIPC(() => {
    throw new Error("Failed to create cache dir");
  });
  await assert.rejects(
    fillThumbnails(
      (onBatch) => generateLibraryThumbnails(["/a"], ["a"], "/cache", onBatch),
      { paths: new Map() }
    ),
    /Failed to create cache dir/
  );
});
