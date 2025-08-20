/**
 * Example TypeScript Application for Calimero
 * 
 * This will be transpiled to Rust using the Calimero SDK
 */

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
    console.log("Set", key, "to", value);
  }
  
  get(key: string): string | undefined {
    const value = this.items.get(key);
    console.log("Getting", key, "=", value);
    return value;
  }
  
  delete(key: string): boolean {
    const deleted = this.items.delete(key);
    if (deleted) {
      this.counter--;
      console.log("Deleted", key);
    }
    return deleted;
  }
  
  clear(): void {
    this.items.clear();
    this.counter = 0;
    console.log("Cleared all items");
  }
  
  getCount(): number {
    return this.counter;
  }
  
  getAllItems(): Array<[string, string]> {
    return Array.from(this.items.entries());
  }
}
