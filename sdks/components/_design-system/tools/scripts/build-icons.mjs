import { readFile, writeFile, mkdir } from "fs/promises";
import path from "path";
import { fileURLToPath } from "url";
import { optimize } from "svgo";
import { transform } from "@svgr/core";
import { globby } from "globby";
import svgoConfig from "../svgo.config.json" with { type: "json" };

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const RAW = path.resolve(__dirname, "../../packages/icons/raw");
const OUT = path.resolve(__dirname, "../../packages/icons/src");

await mkdir(OUT, { recursive: true });

// Define the first 3 icons from the chunky.svg file
// Following the ICON_EXTRACTION_GUIDE.md - using original viewBox coordinates with padding
const iconDefinitions = [
  {
    name: "clock",
    viewBox: "0 0 48 48", // First icon: centered in 48x48 viewBox (6-42 range)
    paths: [
      "M6 24C6 26.3638 6.46558 28.7044 7.37017 30.8883C8.27475 33.0722 9.60062 35.0565 11.2721 36.7279C12.9435 38.3994 14.9278 39.7252 17.1117 40.6298C19.2956 41.5344 21.6362 42 24 42C26.3638 42 28.7044 41.5344 30.8883 40.6298C33.0722 39.7252 35.0565 38.3994 36.7279 36.7279C38.3994 35.0565 39.7252 33.0722 40.6298 30.8883C41.5344 28.7044 42 26.3638 42 24C42 19.2261 40.1036 14.6477 36.7279 11.2721C33.3523 7.89642 28.7739 6 24 6C19.2261 6 14.6477 7.89642 11.2721 11.2721C7.89642 14.6477 6 19.2261 6 24Z",
      "M24 16V24",
      "M24 32H24.02",
    ],
  },
  {
    name: "clock-alert",
    viewBox: "87 0 48 48", // Second icon: centered in 48x48 viewBox (93-129 range)
    paths: [
      "M93 24C93 26.3638 93.4656 28.7044 94.3702 30.8883C95.2748 33.0722 96.6006 35.0565 98.2721 36.7279C99.9435 38.3994 101.928 39.7252 104.112 40.6298C106.296 41.5344 108.636 42 111 42C113.364 42 115.704 41.5344 117.888 40.6298C120.072 39.7252 122.056 38.3994 123.728 36.7279C125.399 35.0565 126.725 33.0722 127.63 30.8883C128.534 28.7044 129 26.3638 129 24C129 19.2261 127.104 14.6477 123.728 11.2721C120.352 7.89642 115.774 6 111 6C106.226 6 101.648 7.89642 98.2721 11.2721C94.8964 14.6477 93 19.2261 93 24Z",
      "M111 32V32.02",
      "M111 25.9995C111.9 26.0022 112.774 25.7017 113.481 25.1464C114.189 24.5911 114.689 23.8135 114.9 22.9392C115.111 22.0648 115.022 21.1448 114.646 20.3276C114.27 19.5105 113.629 18.8439 112.828 18.4355C112.032 18.0279 111.122 17.9016 110.246 18.077C109.369 18.2524 108.578 18.7192 108 19.4015",
    ],
  },
  {
    name: "clock-x",
    viewBox: "174 0 48 48", // Third icon: centered in 48x48 viewBox (180-216 range)
    paths: [
      "M180 24C180 26.3638 180.466 28.7044 181.37 30.8883C182.275 33.0722 183.601 35.0565 185.272 36.7279C186.944 38.3994 188.928 39.7252 191.112 40.6298C193.296 41.5344 195.636 42 198 42C200.364 42 202.704 41.5344 204.888 40.6298C207.072 39.7252 209.056 38.3994 210.728 36.7279C212.399 35.0565 213.725 33.0722 214.63 30.8883C215.534 28.7044 216 26.3638 216 24C216 21.6362 215.534 19.2956 214.63 17.1117C213.725 14.9278 212.399 12.9435 210.728 11.2721C209.056 9.60062 207.072 8.27475 204.888 7.37017C202.704 6.46558 200.364 6 198 6C195.636 6 193.296 6.46558 191.112 7.37017C188.928 8.27475 186.944 9.60062 185.272 11.2721C183.601 12.9435 182.275 14.9278 181.37 17.1117C180.466 19.2956 180 21.6362 180 24Z",
      "M185.4 11.4004L210.6 36.6004",
    ],
  },
];

const exportsList = [];

for (const iconDef of iconDefinitions) {
  const componentName = iconDef.name.replace(/(^[a-z])|-(\w)/g, (_, a, b) =>
    a ? a.toUpperCase() : b.toUpperCase(),
  );

  // Create SVG content from paths
  const svgContent = `<svg viewBox="${iconDef.viewBox}" fill="none" xmlns="http://www.w3.org/2000/svg">
${iconDef.paths.map((path) => `  <path d="${path}" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>`).join("\n")}
</svg>`;

  // Generate React component manually with correct viewBox
  const tsx = `import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ${componentName} = React.forwardRef<SVGSVGElement, IconProps>(
  ({ size = 24, strokeWidth = 1.5, absoluteStrokeWidth = false, ...props }, ref) => {
    // Calculate the actual stroke width based on absoluteStrokeWidth setting
    // When absoluteStrokeWidth is false, stroke width scales with size (24px = 1.5, so scale proportionally)
    // When absoluteStrokeWidth is true, stroke width remains constant
    const actualStrokeWidth = absoluteStrokeWidth 
      ? strokeWidth 
      : (Number(size) / 24) * Number(strokeWidth);
    
    return (
      <svg
        ref={ref}
        width={size}
        height={size}
        viewBox="${iconDef.viewBox}"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        ${iconDef.paths.map((path) => `<path d="${path}"/>`).join("\n        ")}
      </svg>
    );
  }
);

${componentName}.displayName = "${componentName}";
export { ${componentName} };
`;

  await writeFile(path.join(OUT, `${componentName}.tsx`), tsx, "utf8");
  exportsList.push(`export { ${componentName} } from "./${componentName}";`);
}

// Update the index.ts file
await writeFile(
  path.join(OUT, "index.ts"),
  `export * from "./IconBase";\n${exportsList.join("\n")}\n`,
);
console.log(`Generated ${exportsList.length} icons.`);
