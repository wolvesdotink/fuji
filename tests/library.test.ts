// The library store's background thumbnail → CLIP index chain must belong to
// the load that started it: a stale load must not index (or mark ready, or
// clear loading flags for) the catalog that replaced it.
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";

(globalThis as any).window = globalThis;
(globalThis as any).addEventListener ??= () => {};
const { mockIPC, clearMocks } = await import("@tauri-apps/api/mocks");
const { createPinia, setActivePinia } = await import("pinia");
const { useLibraryStore } = await import("../src/stores/library.ts");

type Deferred<T> = { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void };
// Plain initializers instead of `let x!: T`: Node 22.6's type stripping
// does not accept definite-assignment assertions.
function deferred<T>(): Deferred<T> {
  let resolve: (v: T) => void = () => {};
  let reject: (e: unknown) => void = () => {};
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const tick = () => new Promise((r) => setTimeout(r, 5));

function image(id: string) {
  return {
    id,
    file_path: `/lib/${id}/${id}.HIF`,
    file_name: `${id}.HIF`,
    file_size: 1,
    date_created: 0,
    date_modified: 0,
    media_type: "Image",
  };
}

/** Mocked backend whose slow commands are resolved by the test. */
function backend() {
  const thumbs = new Map<string, Deferred<unknown>>();
  const models: Deferred<null>[] = [];
  const modelChannels: number[] = [];
  const listings = new Map<string, Deferred<unknown>>();
  const indexCalls: { ids: string[]; paths: string[] }[] = [];
  mockIPC((cmd, args: any) => {
    switch (cmd) {
      case "plugin:path|resolve_directory":
        return "/home";
      case "plugin:path|join":
        return args.paths.join("/");
      case "list_library_images":
        // "EMPTY" → empty folder, "FAIL" → listing error, "slow*" → resolved
        // by the test, anything else → one image named after the folder.
        if (args.dirPath === "EMPTY") return [];
        if (args.dirPath === "FAIL") throw new Error("Directory does not exist");
        if (args.dirPath.startsWith("slow")) {
          const d = deferred<unknown>();
          listings.set(args.dirPath, d);
          return d.promise;
        }
        return [image(args.dirPath)];
      case "read_file_ratings":
        return {};
      case "generate_library_thumbnails": {
        const d = deferred<unknown>();
        thumbs.set(args.imageIds[0], d);
        return d.promise;
      }
      case "ensure_clip_models": {
        const d = deferred<null>();
        models.push(d);
        modelChannels.push(args.onProgress.id);
        return d.promise;
      }
      case "index_library":
        indexCalls.push({ ids: args.imageIds, paths: args.thumbPaths });
        return null;
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
  const finishThumbs = (id: string) =>
    thumbs.get(id)!.resolve({
      items: [{ image_id: id, thumbnail_path: `/thumbs/${id}.v3.jpg` }],
      failed: [],
    });
  /** Send a model-download progress message on the n-th download's channel. */
  const modelProgress = (n: number, index: number, bytes: number) =>
    (window as any).__TAURI_INTERNALS__.runCallback(modelChannels[n], {
      index,
      message: { bytes_downloaded: bytes, bytes_total: 100, file_name: "model" },
    });
  return { thumbs, models, indexCalls, finishThumbs, listings, modelProgress };
}

/** Swallow (and count) console.error output from expected failures. */
async function quietly(fn: () => Promise<void>): Promise<unknown[]> {
  const errors: unknown[] = [];
  const original = console.error;
  console.error = (...args: unknown[]) => errors.push(args);
  try {
    await fn();
  } finally {
    console.error = original;
  }
  return errors;
}

beforeEach(() => {
  clearMocks();
  setActivePinia(createPinia());
});
afterEach(() => clearMocks());

test("a stale load's thumbnails do not index the newer catalog", async () => {
  const be = backend();
  const store = useLibraryStore();
  await store.loadLibrary("A");
  await tick();
  await store.loadLibrary("B");
  await tick();
  assert.ok(be.thumbs.has("A") && be.thumbs.has("B"));

  be.finishThumbs("A");
  await tick();
  assert.equal(be.models.length, 0, "A must not start indexing");
  assert.deepEqual(be.indexCalls, []);
  assert.equal(store.isLoadingThumbnails, true, "B is still generating");
  assert.equal(store.isIndexReady, false);

  be.finishThumbs("B");
  await tick();
  be.models[0].resolve(null);
  await tick();
  assert.deepEqual(be.indexCalls, [{ ids: ["B"], paths: ["/thumbs/B.v3.jpg"] }]);
  assert.equal(store.isIndexReady, true);
  assert.equal(store.isLoadingThumbnails, false);
  assert.equal(store.isIndexing, false);
});

test("a folder change during model preparation stops the old index build", async () => {
  const be = backend();
  const store = useLibraryStore();
  await store.loadLibrary("A");
  await tick();
  be.finishThumbs("A");
  await tick();
  assert.equal(be.models.length, 1, "A is preparing the model");

  await store.loadLibrary("B");
  await tick();
  be.models[0].resolve(null);
  await tick();
  assert.deepEqual(be.indexCalls, [], "A's catalog was replaced; nothing indexed yet");
  assert.equal(store.isIndexReady, false);

  be.finishThumbs("B");
  await tick();
  be.models[1].resolve(null);
  await tick();
  assert.deepEqual(be.indexCalls, [{ ids: ["B"], paths: ["/thumbs/B.v3.jpg"] }]);
  assert.equal(store.isIndexReady, true);
  assert.equal(store.isDownloadingModel, false);
});

test("a failed thumbnail command does not continue into indexing", async () => {
  const be = backend();
  const store = useLibraryStore();
  const errors = await quietly(async () => {
    await store.loadLibrary("A");
    await tick();
    be.thumbs.get("A")!.reject("Failed to create cache dir");
    await tick();
  });
  assert.equal(be.models.length, 0);
  assert.deepEqual(be.indexCalls, []);
  assert.equal(store.isLoadingThumbnails, false);
  assert.equal(errors.length, 1);
});

for (const [label, folder] of [
  ["an empty folder", "EMPTY"],
  ["a folder whose listing fails", "FAIL"],
] as const) {
  test(`replacing a pending load with ${label} clears the thumbnail state`, async () => {
    const be = backend();
    const store = useLibraryStore();
    await store.loadLibrary("A");
    await tick();
    assert.equal(store.isLoadingThumbnails, true);

    await quietly(() => store.loadLibrary(folder));
    await tick();
    assert.equal(store.isLoadingThumbnails, false, "the new load owns the flag");
    assert.deepEqual(store.thumbnailProgress, { completed: 0, total: 0 });

    be.finishThumbs("A");
    await tick();
    assert.equal(store.isLoadingThumbnails, false, "stale A leaves it alone");
    assert.deepEqual(store.thumbnailProgress, { completed: 0, total: 0 });
    assert.equal(store.isLoading, false);
    assert.equal(be.models.length, 0);
  });
}

test("a stale listing does not clear the newer load's loading indicator", async () => {
  const be = backend();
  const store = useLibraryStore();
  const a = store.loadLibrary("slowA");
  await tick();
  const b = store.loadLibrary("slowB");
  await tick();
  be.listings.get("slowA")!.resolve([image("slowA")]);
  await a;
  assert.equal(store.isLoading, true, "B is still listing");
  assert.equal(store.images.length, 0, "A's stale listing is dropped");
  be.listings.get("slowB")!.resolve([image("slowB")]);
  await b;
  assert.equal(store.isLoading, false);
  assert.deepEqual(
    store.images.map((i: { id: string }) => i.id),
    ["slowB"]
  );
  // Let B's background thumbnail → index chain finish before the mocks are
  // torn down; otherwise its pending IPC calls land on a cleared mock.
  await tick();
  be.finishThumbs("slowB");
  await tick();
  be.models[0].resolve(null);
  await tick();
  assert.equal(store.isIndexReady, true);
});

test("a new folder is not reported as indexed until its own index is built", async () => {
  const be = backend();
  const store = useLibraryStore();
  await store.loadLibrary("A");
  await tick();
  be.finishThumbs("A");
  await tick();
  be.models[0].resolve(null);
  await tick();
  assert.equal(store.isIndexReady, true);

  await store.loadLibrary("B");
  await tick();
  assert.equal(store.isIndexReady, false, "B's thumbnails are still pending");
  be.finishThumbs("B");
  await tick();
  be.models[1].resolve(null);
  await tick();
  assert.equal(store.isIndexReady, true);
});

test("a stale model download cannot overwrite the current download progress", async () => {
  const be = backend();
  const store = useLibraryStore();
  await store.loadLibrary("A");
  await tick();
  be.finishThumbs("A");
  await tick();
  be.modelProgress(0, 0, 10);
  assert.equal(store.modelDownloadProgress.bytes_downloaded, 10);

  await store.loadLibrary("B");
  await tick();
  be.finishThumbs("B");
  await tick();
  assert.equal(store.isDownloadingModel, true);
  assert.equal(store.modelDownloadProgress.bytes_downloaded, 0, "reset for B's download");
  be.modelProgress(1, 0, 60);
  be.modelProgress(0, 1, 20); // stale A keeps reporting
  assert.equal(store.modelDownloadProgress.bytes_downloaded, 60);

  be.models[0].resolve(null); // A's download ends first
  await tick();
  assert.equal(store.isDownloadingModel, true, "B is still downloading");
  be.models[1].resolve(null);
  await tick();
  assert.equal(store.isDownloadingModel, false);
  assert.deepEqual(be.indexCalls, [{ ids: ["B"], paths: ["/thumbs/B.v3.jpg"] }]);
});
