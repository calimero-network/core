import { RootKey } from "./dataSource/DidDataSource";

export interface DidApi {
  getDidList(): Promise<RootKey[]>;
}
