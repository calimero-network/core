Static frontends for the `45-frontend-two-releases` scenario.
Each release's `index.html` embeds its own version string, so a response shows which release served it.
`workflows/app-migration/build-wasms.sh` packs them into two bundles of one package.
