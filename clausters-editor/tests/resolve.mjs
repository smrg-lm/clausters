// The page's modules import their siblings without an extension, as vite
// resolves them (`./code`, `./regions`). node does not, so the tests run with
// this hook (`node --import ./tests/resolve.mjs`), which tries `.ts` where a
// relative specifier names no file: the sources run as they are, with no
// build and no runner package.
import { registerHooks } from "node:module";

registerHooks({
  resolve(specifier, context, next) {
    try {
      return next(specifier, context);
    } catch (error) {
      if (specifier.startsWith(".") && !/\.[cm]?[jt]s$/.test(specifier)) {
        return next(`${specifier}.ts`, context);
      }
      throw error;
    }
  },
});
