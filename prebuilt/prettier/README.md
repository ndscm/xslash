# prettier

An xslash-managed prettier. Works **only without plugins.**

Prettier ships as a single self-contained script with no runtime dependencies,
so xslash can run it like any standalone executable. Plugins break that: a
plugin is a separate package that prettier resolves from the cwd's
`node_modules`, which an xslash-extracted prettier doesn't have. For plugins you
still need a project `node_modules` (e.g. `pnpm install`).

## Using ndscm to manage formatters

Don't use this entry. ndscm requires a formatter with no per-repo prepare step,
which plugins violate here. Instead use a wrapper that runs `pnpm install` if
`node_modules` is missing, then runs prettier via `npx` (or `pnpm dlx`) - no
xslash. Plugins resolve normally, and the lazy install keeps it prepare-free.
