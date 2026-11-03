# prettier

xslash-managed prettiers. An example of running interpretive language packages
with xslash. **Not recommended to run prettier with xslash** - prefer a real
prebuilt formatter.

Prettier itself ships as a single self-contained script with no runtime
dependencies, so xslash can run it like any standalone executable. The
complication is plugins: a plugin is a separate package that prettier resolves
from the working directory's `node_modules`, which an xslash-extracted prettier
doesn't have. The two entries here take opposite approaches to that.

## `prettier.xslash` - pure prettier, no plugins

Downloads the prettier npm tarball
(`https://registry.npmjs.org/prettier/-/prettier-<version>.tgz`) and runs its
`package/bin/prettier.cjs` directly. Nothing else is bundled, so it works **only
without plugins**. A config that names a plugin will fail to resolve it; for
plugins you still need a project `node_modules` (e.g. `pnpm install`).

## `prettier-standalone.xslash` — self-contained, plugins included

One tarball with everything baked in: prettier, its plugins, and an embedded
`.prettierrc`. It's built from `standalone/` (a tiny pnpm project) by `pack.sh`,
which runs `pnpm deploy` and packs the result into `standalone.tar.gz`.

The wrapper script (`standalone/prettier.sh`) is what makes it feel like a
normal prettier run even though it's executing out of the xslash extract
directory:

## Not recommended to use

`prettier-standalone` works, but prefer a real prebuilt formatter:

- **It bloats checkout.** The `standalone.tar.gz` (~4.5 MB) is checked into the
  repository, so every checkout has to fetch it.
- **Reproducibility is shaky.** The tarball is byte-stable today, but a plugin
  or config change can quietly break that.
- **It fights the pnpm workspace.** You have to exclude `standalone/` from the
  root pnpm workspace and maintain its separate lockfile.
