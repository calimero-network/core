#!/usr/bin/env node

/**
 * Simple TypeScript to Rust Transpiler for Calimero
 */

function transpile(tsCode, appName) {
  // Extract class properties (only at class level)
  const properties = [];
  const lines = tsCode.split('\n');
  
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    
    // Look for property declarations: name: type
    const match = line.match(/^(\w+):\s*([^;\n]+)/);
    if (match) {
      const name = match[1];
      const type = match[2].trim();
      
      // Only add if it's a class property (not method parameter)
      if (type.includes('Map') || type.includes('number') || type.includes('string') || type.includes('Array')) {
        if (!properties.find(p => p.name === name)) {
          properties.push({ name, type });
        }
      }
    }
  }
  
  // Extract methods
  const methods = [];
  const methodRegex = /(\w+)\s*\(([^)]*)\)(?:\s*:\s*(\w+))?\s*\{/g;
  let methodMatch;
  while ((methodMatch = methodRegex.exec(tsCode)) !== null) {
    const methodName = methodMatch[1];
    if (methodName !== 'constructor') {
      methods.push(methodName);
    }
  }
  
  // Generate Rust code
  const stateFields = properties
    .map(p => `    ${p.name}: ${mapType(p.type)},`)
    .join('\n');
    
  const methodImpls = methods
    .map(m => `    pub fn ${m}(&mut self) -> app::Result<()> {
        app::log!("Executing ${m}");
        Ok(())
    }`)
    .join('\n\n');
  
  return `#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::UnorderedMap;

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct ${appName} {
${stateFields}
}

#[app::logic]
impl ${appName} {
    #[app::init]
    pub fn init() -> ${appName} {
        app::log!("Initializing ${appName}");
        ${appName} {
${properties.map(p => `            ${p.name}: ${getDefaultValue(mapType(p.type))},`).join('\n')}
        }
    }

${methodImpls}
}`;
}

function mapType(tsType) {
  if (tsType.includes('Map<string, string>')) return 'UnorderedMap<String, String>';
  if (tsType.includes('number')) return 'i64';
  if (tsType.includes('string')) return 'String';
  if (tsType.includes('Array')) return 'Vec<String>';
  return 'String';
}

function getDefaultValue(rustType) {
  if (rustType === 'UnorderedMap<String, String>') return 'UnorderedMap::new()';
  if (rustType === 'i64') return '0';
  if (rustType === 'String') return 'String::new()';
  if (rustType === 'Vec<String>') return 'Vec::new()';
  return 'String::new()';
}

// CLI
const args = process.argv.slice(2);
if (args.length < 3) {
  console.log('Usage: node transpiler.js <input.ts> <output.rs> <app-name>');
  process.exit(1);
}

const [inputFile, outputFile, appName] = args;

// Read input file
import fs from "fs";
let tsCode;
try {
  tsCode = fs.readFileSync(inputFile, 'utf8');
} catch (err) {
  console.log('Using example TypeScript code instead');
  tsCode = `
class KvStore {
  items: Map<string, string>;
  counter: number;
  
  constructor() {
    this.items = new Map();
    this.counter = 0;
  }
  
  set(key: string, value: string): void {
    this.items.set(key, value);
    this.counter++;
  }
  
  get(key: string): string | undefined {
    return this.items.get(key);
  }
}
`;
}

const rustCode = transpile(tsCode, appName);

console.log('Generated Rust code:');
console.log('='.repeat(50));
console.log(rustCode);
console.log('='.repeat(50));

// Write output file
try {
  fs.writeFileSync(outputFile, rustCode);
  console.log(`\nRust code written to ${outputFile}`);
} catch (err) {
  console.log('Could not write output file:', err.message);
}
