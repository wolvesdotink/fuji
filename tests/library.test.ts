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
function deferred<T>(): Deferred<T> {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => ((resolve = res), (reject = rej)));
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
  const indexCalls: { ids: string[]; paths: string[] }[] = [];
  mockIPC((cmd, args: any) => {
    switch (cmd) {
      case "plugin:path|resolve_directory":
        return "/home";
      case "plugin:path|join":
        return args.paths.join("/");
      case "list_library_images":
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
  return { thumbs, models, indexCalls, finishThumbs };
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
  const errors: unknown[] = [];
  const original = console.error;
  console.error = (...args: unknown[]) => errors.push(args);
  try {
    await store.loadLibrary("A");
    await tick();
    be.thumbs.get("A")!.reject("Failed to create cache dir");
    await tick();
  } finally {
    console.error = original;
  }
  assert.equal(be.models.length, 0);
  assert.deepEqual(be.indexCalls, []);
  assert.equal(store.isLoadingThumbnails, false);
  assert.equal(errors.length, 1);
});
