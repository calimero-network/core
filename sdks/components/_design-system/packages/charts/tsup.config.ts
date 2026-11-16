import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts"],
  format: ["esm", "cjs"],
  dts: true,
  external: ["react", "react/jsx-runtime", "recharts"],
  treeshake: true,
  clean: true,
});
