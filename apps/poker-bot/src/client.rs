//! JSON-RPC client for interacting with a Calimero poker table.

use serde_json::{json, Value};

use crate::types::{GameView, HandResult, TableStats};

/// Client that talks to one Calimero node's JSON-RPC endpoint.
pub struct PokerClient {
    url: String,
    context_id: String,
    public_key: String,
}

impl PokerClient {
    pub fn new(node_url: &str, context_id: &str, public_key: &str) -> Self {
        let url = format!("{}/jsonrpc", node_url.trim_end_matches('/'));
        Self {
            url,
            context_id: context_id.to_string(),
            public_key: public_key.to_string(),
        }
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    // ── queries ─────────────────────────────────────────────────────

    pub fn get_game_state(&self) -> Result<GameView, String> {
        let output = self.call_method("get_game_state", json!({}))?;
        serde_json::from_value(output).map_err(|e| format!("parse game state: {e}"))
    }

    pub fn get_my_cards(&self) -> Result<Vec<String>, String> {
        let output = self.call_method("get_my_cards", json!({}))?;
        serde_json::from_value(output).map_err(|e| format!("parse cards: {e}"))
    }

    pub fn get_hand_result(&self) -> Result<HandResult, String> {
        let output = self.call_method("get_hand_result", json!({}))?;
        serde_json::from_value(output).map_err(|e| format!("parse hand result: {e}"))
    }

    #[allow(dead_code)] // Available for external consumers
    pub fn get_stats(&self) -> Result<TableStats, String> {
        let output = self.call_method("get_stats", json!({}))?;
        serde_json::from_value(output).map_err(|e| format!("parse stats: {e}"))
    }

    // ── mutations ───────────────────────────────────────────────────

    pub fn join_table(&self, buy_in: u64) -> Result<(), String> {
        self.call_method("join_table", json!({ "buy_in": buy_in }))?;
        Ok(())
    }

    pub fn start_hand(&self) -> Result<(), String> {
        self.call_method("start_hand", json!({}))?;
        Ok(())
    }

    pub fn fold(&self) -> Result<(), String> {
        self.call_method("fold", json!({}))?;
        Ok(())
    }

    pub fn check(&self) -> Result<(), String> {
        self.call_method("check", json!({}))?;
        Ok(())
    }

    pub fn call_bet(&self) -> Result<(), String> {
        self.call_method("call", json!({}))?;
        Ok(())
    }

    pub fn raise_to(&self, amount: u64) -> Result<(), String> {
        self.call_method("raise_to", json!({ "amount": amount }))?;
        Ok(())
    }

    // ── internal ────────────────────────────────────────────────────

    fn call_method(&self, method: &str, args: Value) -> Result<Value, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "execute",
            "params": {
                "contextId": self.context_id,
                "method": method,
                "argsJson": args,
                "executorPublicKey": self.public_key,
                "substitute": []
            }
        });

        let resp: Value = ureq::post(&self.url)
            .set("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|e| format!("HTTP error: {e}"))?
            .into_json()
            .map_err(|e| format!("JSON parse: {e}"))?;

        if let Some(err) = resp.get("error") {
            return Err(format!("RPC error: {err}"));
        }

        // Extract result.output
        resp.get("result")
            .and_then(|r| r.get("output"))
            .cloned()
            .ok_or_else(|| "missing result.output in response".to_string())
    }
}
