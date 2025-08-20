#!/usr/bin/env node

/**
 * Simple TypeScript to Rust Transpiler for Calimero
 */

class CalimeroTranspiler {
  constructor(appName) {
    this.appName = appName;
  }

  transpile(tsCode) {
    // Simple regex-based parsing for now
    const classMatch = tsCode.match(/class\s+(\w+)\s*\{([\s\S]*)\}/);
    if (!classMatch) {
      throw new Error('No class found in TypeScript code');
    }

    const className = classMatch[1];
    const classBody = classMatch[2];
    
    console.log('Class body:', classBody);
    
    // Extract properties (only class-level properties, not method parameters)
    const properties = [];
    const lines = classBody.split('\n');
    
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i].trim();
      
      // Look for property declarations (simple pattern: name: type)
      // Must be at class level (not inside constructor or methods)
      const propertyMatch = line.match(/^(\w+):\s*([^;\n]+)/);
      if (propertyMatch) {
        const propertyName = propertyMatch[1];
        const propertyType = this.cleanType(propertyMatch[2]);
        
        console.log(`Line ${i}: Found potential property: ${propertyName}: ${propertyType}`);
        
        // Check if this is a class-level property (not inside a method)
        const isClassLevel = this.isClassLevelProperty(lines, i);
        console.log(`Line ${i}: Is class level: ${isClassLevel}`);
        
        if (isClassLevel) {
          // Avoid duplicates
          if (!properties.find(p => p.name === propertyName)) {
            properties.push({
              name: propertyName,
              type: this.mapType(propertyType)
            });
            console.log(`Added property: ${propertyName}`);
          } else {
            console.log(`Skipped duplicate: ${propertyName}`);
          }
        }
      }
    }

    console.log('Final properties:', properties);

    // Extract methods
    const methods = [];
    const methodRegex = /(\w+)\s*\(([^)]*)\)(?:\s*:\s*(\w+))?\s*\{([\s\S]*?)\}/g;
    let match;
    while ((match = methodRegex.exec(classBody)) !== null) {
      const methodName = match[1];
      if (methodName !== 'constructor') {
        methods.push({
          name: methodName,
          params: match[2].split(',').map(p => p.trim()).filter(p => p),
          returnType: match[3] || 'void',
          body: match[4]
        });
      }
    }

    return this.generateRust(properties, methods);
  }

  isClassLevelProperty(lines, lineIndex) {
    // Check if we're inside a method by looking for method boundaries
    let braceCount = 0;
    let inMethod = false;
    
    for (let i = 0; i < lineIndex; i++) {
      const line = lines[i].trim();
      
      // Check for method start (including constructor)
      if (line.match(/^\w+\s*\([^)]*\)\s*\{/)) {
        inMethod = true;
        braceCount = 1;
        console.log(`Line ${i}: Entered method: ${line}`);
      }
      // Count braces
      else if (inMethod) {
        braceCount += (line.match(/\{/g) || []).length;
        braceCount -= (line.match(/\}/g) || []).length;
        
        if (braceCount === 0) {
          inMethod = false;
          console.log(`Line ${i}: Exited method`);
        }
      }
    }
    
    return !inMethod;
  }

  cleanType(type) {
    return type.trim().replace(/[;\n]/g, '');
  }

  mapType(tsType) {
    const typeMap = {
      'string': 'String',
      'number': 'i64',
      'boolean': 'bool',
      'void': '()',
      'Map<string, string>': 'UnorderedMap<String, String>',
      'Map<string, number>': 'UnorderedMap<String, i64>',
      'Array<[string, string]>': 'Vec<(String, String)>',
      'Array<string>': 'Vec<String>',
      'Array<number>': 'Vec<i64>'
    };
    
    // Try exact match first
    if (typeMap[tsType]) {
      return typeMap[tsType];
    }
    
    // Try generic type matching
    if (tsType.includes('Map<')) {
      return 'UnorderedMap<String, String>';
    }
    if (tsType.includes('Array<')) {
      return 'Vec<String>';
    }
    
    return 'String';
  }

  generateRust(properties, methods) {
    const stateFields = properties
      .map(p => `    ${p.name}: ${p.type},`)
      .join('\n');

    const methodImpls = methods
      .map(m => {
        const params = m.params.map(p => {
          const [name, type] = p.split(':').map(s => s.trim());
          return `${name}: ${this.mapType(type || 'any')}`;
        }).join(', ');
        
        return `    pub fn ${m.name}(&mut self, ${params}) -> app::Result<()> {
        app::log!("Executing ${m.name}");
        // TODO: Transpile method body
        Ok(())
    }`;
      })
      .join('\n\n');

    return `#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::UnorderedMap;

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct ${this.appName} {
${stateFields}
}

#[app::logic]
impl ${this.appName} {
    #[app::init]
    pub fn init() -> ${this.appName} {
        app::log!("Initializing ${this.appName}");
        ${this.appName} {
${properties.map(p => `            ${p.name}: ${this.getDefaultValue(p.type)},`).join('\n')}
        }
    }

${methodImpls}
}`;
  }

  getDefaultValue(rustType) {
    const defaults = {
      'String': 'String::new()',
      'i64': '0',
      'bool': 'false',
      'UnorderedMap<String, String>': 'UnorderedMap::new()',
      'Vec<String>': 'Vec::new()',
      'Vec<i64>': 'Vec::new()',
      'Vec<(String, String)>': 'Vec::new()'
    };
    return defaults[rustType] || 'String::new()';
  }
}

// CLI
const args = process.argv.slice(2);
if (args.length < 3) {
  console.log('Usage: node simple-transpiler.js <input.ts> <output.rs> <app-name>');
  process.exit(1);
}

const [inputFile, outputFile, appName] = args;

// Example TypeScript input
const exampleTS = `
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

const transpiler = new CalimeroTranspiler(appName);
const rustCode = transpiler.transpile(exampleTS);

console.log('Generated Rust code:');
console.log('='.repeat(50));
console.log(rustCode);
console.log('='.repeat(50));
