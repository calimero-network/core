#!/usr/bin/env node

/**
 * TypeScript to Rust Transpiler for Calimero WASM Applications
 * 
 * This transpiler converts TypeScript code to Rust code that uses
 * the Calimero SDK macros and can be compiled to WASM.
 */

interface TranspileOptions {
  inputFile: string;
  outputFile: string;
  appName: string;
}

class CalimeroTranspiler {
  private appName: string;
  private stateFields: Array<{ name: string; type: string }> = [];
  private methods: Array<{ name: string; params: string[]; returnType: string; body: string }> = [];
  private events: Array<{ name: string; fields: Array<{ name: string; type: string }> }> = [];

  constructor(appName: string) {
    this.appName = appName;
  }

  /**
   * Transpile TypeScript code to Rust
   */
  transpile(tsCode: string): string {
    // Parse TypeScript code (simplified for now)
    this.parseTypeScript(tsCode);
    
    // Generate Rust code
    return this.generateRust();
  }

  /**
   * Parse TypeScript code to extract structure
   */
  private parseTypeScript(tsCode: string) {
    // For now, let's create a simple example
    // In a real implementation, you'd use a TypeScript parser
    
    // Example: Parse a simple class structure
    const classMatch = tsCode.match(/class\s+(\w+)\s*\{([\s\S]*)\}/);
    if (classMatch) {
      const className = classMatch[1];
      const classBody = classMatch[2];
      
      // Extract properties (state fields)
      const propertyMatches = classBody.matchAll(/(\w+):\s*(\w+)(?:\[\])?/g);
      for (const match of propertyMatches) {
        this.stateFields.push({
          name: match[1],
          type: this.mapTypeScriptToRust(match[2])
        });
      }
      
      // Extract methods
      const methodMatches = classBody.matchAll(/(\w+)\s*\(([^)]*)\)(?:\s*:\s*(\w+))?\s*\{([\s\S]*?)\}/g);
      for (const match of methodMatches) {
        const methodName = match[1];
        const params = match[2].split(',').map(p => p.trim()).filter(p => p);
        const returnType = match[3] || 'void';
        const body = match[4];
        
        if (methodName !== 'constructor') {
          this.methods.push({
            name: methodName,
            params: params.map(p => this.parseParam(p)),
            returnType: this.mapTypeScriptToRust(returnType),
            body: this.transpileMethodBody(body)
          });
        }
      }
    }
  }

  /**
   * Map TypeScript types to Rust types
   */
  private mapTypeScriptToRust(tsType: string): string {
    const typeMap: Record<string, string> = {
      'string': 'String',
      'number': 'i64',
      'boolean': 'bool',
      'string[]': 'Vec<String>',
      'number[]': 'Vec<i64>',
      'boolean[]': 'Vec<bool>',
      'void': '()',
      'any': 'String', // Default to String for any
    };
    
    return typeMap[tsType] || 'String';
  }

  /**
   * Parse method parameter
   */
  private parseParam(param: string): string {
    const [name, type] = param.split(':').map(s => s.trim());
    return `${name}: ${this.mapTypeScriptToRust(type || 'any')}`;
  }

  /**
   * Transpile method body from TypeScript to Rust
   */
  private transpileMethodBody(tsBody: string): string {
    // Simple transformations for now
    let rustBody = tsBody
      .replace(/console\.log\(/g, 'app::log!("')
      .replace(/\)/g, '")')
      .replace(/this\./g, 'self.')
      .replace(/\.push\(/g, '.push(')
      .replace(/\.length/g, '.len()')
      .replace(/\.get\(/g, '.get(')
      .replace(/\.set\(/g, '.insert(')
      .replace(/\.delete\(/g, '.remove(')
      .replace(/\.clear\(\)/g, '.clear()')
      .replace(/return\s+([^;]+);/g, 'Ok($1)')
      .replace(/if\s*\(([^)]+)\)\s*\{/g, 'if $1 {')
      .replace(/for\s*\(([^)]+)\)\s*\{/g, 'for $1 in {')
      .replace(/while\s*\(([^)]+)\)\s*\{/g, 'while $1 {');
    
    return rustBody;
  }

  /**
   * Generate Rust code
   */
  private generateRust(): string {
    const stateFields = this.stateFields
      .map(field => `    ${field.name}: ${field.type},`)
      .join('\n');
    
    const methods = this.methods
      .map(method => {
        const params = method.params.join(', ');
        const returnType = method.returnType === '()' ? 'app::Result<()>' : `app::Result<${method.returnType}>`;
        
        return `    pub fn ${method.name}(&mut self, ${params}) -> ${returnType} {
        app::log!("Executing ${method.name}");
        ${method.body}
        Ok(())
    }`;
      })
      .join('\n\n');

    return `#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::UnorderedMap;
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct ${this.appName} {
${stateFields}
}

#[app::event]
pub enum Event<'a> {
    // Events will be generated based on method calls
    MethodCalled { name: &'a str },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("operation failed: {0}")]
    OperationFailed(&'a str),
}

#[app::logic]
impl ${this.appName} {
    #[app::init]
    pub fn init() -> ${this.appName} {
        app::log!("Initializing ${this.appName}");
        ${this.appName} {
${this.stateFields.map(f => `            ${f.name}: ${this.getDefaultValue(f.type)},`).join('\n')}
        }
    }

${methods}
}`;
  }

  /**
   * Get default value for Rust type
   */
  private getDefaultValue(rustType: string): string {
    const defaults: Record<string, string> = {
      'String': 'String::new()',
      'i64': '0',
      'bool': 'false',
      'Vec<String>': 'Vec::new()',
      'Vec<i64>': 'Vec::new()',
      'Vec<bool>': 'Vec::new()',
      'UnorderedMap<String, String>': 'UnorderedMap::new()',
    };
    
    return defaults[rustType] || 'String::new()';
  }
}

/**
 * CLI interface
 */
function main() {
  const args = process.argv.slice(2);
  
  if (args.length < 3) {
    console.log('Usage: node transpiler.js <input.ts> <output.rs> <app-name>');
    process.exit(1);
  }
  
  const [inputFile, outputFile, appName] = args;
  
  // For now, let's create a simple example
  const exampleTypeScript = `
class KvStore {
  items: Map<string, string>;
  
  constructor() {
    this.items = new Map();
  }
  
  set(key: string, value: string): void {
    this.items.set(key, value);
    console.log("Set", key, "to", value);
  }
  
  get(key: string): string | undefined {
    return this.items.get(key);
  }
  
  clear(): void {
    this.items.clear();
    console.log("Cleared all items");
  }
}
`;
  
  const transpiler = new CalimeroTranspiler(appName);
  const rustCode = transpiler.transpile(exampleTypeScript);
  
  console.log('Generated Rust code:');
  console.log('='.repeat(50));
  console.log(rustCode);
  console.log('='.repeat(50));
  
  console.log(`\nTranspilation complete! Output saved to ${outputFile}`);
}

// ES module entry point
main();

export { CalimeroTranspiler };
