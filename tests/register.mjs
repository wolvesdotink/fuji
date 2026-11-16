// Resolve the app's `@/` import alias (vite/tsconfig) for `node --test`.
import { register } from "node:module";
register("./alias-hooks.mjs", import.meta.url);
