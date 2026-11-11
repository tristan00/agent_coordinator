# Relay UI components

The progress cards use Magic UI's Number Ticker and Animated Circular Progress Bar, installed as source components under `src/components/magicui`. Upstream: https://github.com/magicuidesign/magicui/tree/main/apps/www/registry/magicui. The upstream MIT license is retained beside the components. Local CSS adapts their styling to Relay; reduced motion is respected.

Install dependencies with `pnpm install --frozen-lockfile`, then run `pnpm build` from this directory. Commit the generated `web/metrics.js` and license notice with source changes. Rust embeds those files, so Node is only needed to rebuild frontend components, not to run Relay. The rest of the board uses the existing JavaScript interface.
