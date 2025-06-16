export interface ClientKey {
  client_id: string;
  root_key_id: string;
  name: string;
  permissions: string[];
  created_at: number;
  revoked_at?: number;
  is_valid: boolean;
}
