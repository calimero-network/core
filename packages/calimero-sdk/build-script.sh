#!/bin/bash
rm -rf node_modules dist lib && pnpm install && pnpm build && cd ../../examples/only-peers-simple && rm -rf node_modules && pnpm install && pnpm dev