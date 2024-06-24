#!/bin/bash
rm -rf node_modules dist lib && pnpm install && pnpm build && cd ../../node-ui/ && rm -rf node_modules && pnpm install && pnpm build