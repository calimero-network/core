use serde_json::{json, Value};

use super::*;

const MODEL: &str = "Qwen/Qwen3.8-27B";

/// A chat completion as NEAR AI answers one, extras included: a reasoning model's
/// `reasoning_content`, `logprobs`, `matched_stop`, a model id other than the one asked for.
fn completion(message: Value, finish_reason: &str) -> Vec<u8> {
    let mut message = message;
    message["reasoning_content"] = json!("Let me think about this.");
    message["annotations"] = json!(null);
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "created": 1_764_200_000,
        "model": "qwen/qwen3.8-27b-fp8",
        "choices": [{
            "index": 0,
            "message": message,
            "logprobs": null,
            "finish_reason": finish_reason,
            "matched_stop": 151_645
        }],
        "usage": {
            "prompt_tokens": 12,
            "completion_tokens": 30,
            "total_tokens": 42,
            "reasoning_tokens": 25,
            "prompt_tokens_details": { "cached_tokens": 0 },
            "completion_tokens_details": { "reasoning_tokens": 25 }
        }
    })
    .to_string()
    .into_bytes()
}

fn assistant(content: Value) -> Value {
    json!({ "role": "assistant", "content": content })
}

fn usage() -> Usage {
    Usage {
        prompt_tokens: 12,
        completion_tokens: 30,
        total_tokens: 42,
    }
}

fn prompt(value: Value) -> Prompt {
    serde_json::from_value(value).expect("a prompt")
}

fn ask(text: &str) -> Prompt {
    prompt(json!({
        "model": MODEL, "max_tokens": 512, "messages": [{ "role": "user", "content": text }]
    }))
}

fn body_of(request: &Request) -> Value {
    serde_json::from_slice(&request.body).expect("a JSON body")
}

fn unusable(reason: &str) -> Error {
    Error::Unusable {
        reason: reason.into(),
    }
}

fn ticket_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "create_ticket",
            "description": "Open a support ticket",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "maxLength": 80 },
                    "urgency": { "type": "number", "maximum": 0.5 }
                },
                "required": ["title"],
                "additionalProperties": false
            }
        }
    })
}

fn ticket_call() -> Value {
    json!({
        "id": "call_b5dc0567f4334ffea914c322",
        "type": "function",
        "function": { "name": "create_ticket", "arguments": "{\"title\": \"printer broken\"}" }
    })
}

#[test]
fn a_prompt_is_posted_as_json_with_its_retry_key_and_no_api_key() {
    let request = http_request(&ask("Capital of France?"), "run-7");
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, ENDPOINT);
    assert_eq!(
        request.headers,
        [
            ("content-type".to_owned(), "application/json".to_owned()),
            ("idempotency-key".to_owned(), "run-7".to_owned()),
        ]
    );
    assert_eq!(
        body_of(&request),
        json!({
            "model": MODEL,
            "max_tokens": 512,
            "messages": [{ "role": "user", "content": "Capital of France?" }]
        })
    );
}

#[test]
fn temperature_and_tools_go_out_only_when_set() {
    let mut p = ask("hi");
    p.temperature = Some(0.2);
    let body = body_of(&http_request(&p, "k"));
    assert_eq!(body["temperature"], json!(0.2));
    assert!(body.get("tools").is_none() && body.get("tool_choice").is_none());
}

#[test]
fn tools_and_a_tool_follow_up_go_out_byte_for_byte() {
    let messages = json!([
        { "role": "system", "content": "You file tickets." },
        { "role": "user", "content": "The printer is broken" },
        { "role": "assistant", "content": null, "tool_calls": [ticket_call()] },
        { "role": "tool", "tool_call_id": "call_b5dc0567f4334ffea914c322", "content": "{\"ticket\": 42}" }
    ]);
    for choice in [
        json!("auto"),
        json!("none"),
        json!("required"),
        json!({ "type": "function", "function": { "name": "create_ticket" } }),
    ] {
        let p = prompt(json!({
            "model": MODEL, "max_tokens": 512, "messages": messages,
            "tools": [ticket_tool()], "tool_choice": choice
        }));
        let request = http_request(&p, "k");
        let body = body_of(&request);
        assert_eq!(body["messages"], messages);
        assert_eq!(body["tools"], json!([ticket_tool()]));
        assert_eq!(body["tool_choice"], choice);
        // Integers stay integers in a schema.
        let text = String::from_utf8(request.body).expect("utf-8");
        assert!(text.contains("\"maxLength\":80,"), "{text}");
    }
}

#[test]
fn a_plain_answer_keeps_the_message_and_drops_near_ais_extras() {
    let got = answer(200, &completion(assistant(json!("\n\nParis.")), "stop"));
    assert_eq!(
        got,
        Ok(Answer {
            message: Message::Assistant {
                content: Some("\n\nParis.".into()),
                tool_calls: None,
            },
            finish_reason: "stop".into(),
            usage: usage(),
        })
    );
}

#[test]
fn a_tool_call_comes_back_as_the_next_requests_assistant_message() {
    let message = json!({ "role": "assistant", "content": null, "tool_calls": [ticket_call()] });
    let got = answer(200, &completion(message.clone(), "tool_calls")).expect("an answer");
    assert_eq!(got.finish_reason, "tool_calls");
    assert_eq!(serde_json::to_value(&got.message).expect("json"), message);
    // Straight into the follow-up's messages.
    let mut follow_up = ask("The printer is broken");
    follow_up.messages.push(got.message);
    assert_eq!(
        body_of(&http_request(&follow_up, "k"))["messages"][1],
        message
    );
}

#[test]
fn an_empty_answer_says_why() {
    for (content, finish, why) in [
        (
            json!(null),
            "length",
            "the model ran out of tokens before answering",
        ),
        (
            json!("\n\n"),
            "length",
            "the model ran out of tokens before answering",
        ),
        (
            json!(""),
            "content_filter",
            "the model answered nothing (finish_reason: content_filter)",
        ),
    ] {
        assert_eq!(
            answer(200, &completion(assistant(content), finish)),
            Err(unusable(why))
        );
    }
}

#[test]
fn an_error_status_carries_near_ais_message() {
    let body = json!({ "error": {
        "message": "temperature is not supported by this model",
        "type": "BadRequestError", "param": "temperature", "code": 400
    } });
    let e = answer(400, body.to_string().as_bytes()).expect_err("an error");
    assert_eq!(
        e,
        Error::Status {
            status: 400,
            message: Some("temperature is not supported by this model".into()),
        }
    );
    assert_eq!(
        e.to_string(),
        "NEAR AI answered 400: temperature is not supported by this model"
    );
    let e = answer(503, b"overloaded").expect_err("an error");
    assert_eq!(
        e,
        Error::Status {
            status: 503,
            message: None
        }
    );
    assert_eq!(e.to_string(), "NEAR AI answered 503");
}

#[test]
fn a_reply_that_is_no_chat_completion_is_unusable() {
    let no_choices = json!({
        "choices": [], "usage": { "prompt_tokens": 1, "completion_tokens": 0, "total_tokens": 1 }
    });
    assert_eq!(
        answer(200, no_choices.to_string().as_bytes()),
        Err(unusable("NEAR AI's answer has no choices"))
    );
    assert_eq!(
        answer(200, b"<html>bad gateway</html>"),
        Err(unusable(
            "NEAR AI's answer is not a chat completion: expected value at line 1 column 1"
        ))
    );
}

#[test]
fn an_error_reaches_the_app_as_a_typed_code() {
    let cases = [
        (
            Error::NotSent {
                reason: "host not approved".into(),
            },
            "NearAiNotSent",
            json!({ "reason": "host not approved" }),
        ),
        (
            Error::Status {
                status: 429,
                message: Some("slow down".into()),
            },
            "NearAiStatus",
            json!({ "status": 429, "message": "slow down" }),
        ),
        (
            unusable("the model ran out of tokens before answering"),
            "NearAiUnusable",
            json!({ "reason": "the model ran out of tokens before answering" }),
        ),
    ];
    for (e, code, fields) in cases {
        assert_eq!(AppError::from(e), AppError::with_fields(code, fields));
    }
}
