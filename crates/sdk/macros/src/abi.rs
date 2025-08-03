use crate::logic::method::PublicLogicMethod;
use serde_json::json;

pub struct Abi<'a> {
    pub methods: &'a Vec<PublicLogicMethod<'a>>,
}

impl Abi<'_> {
    pub fn to_string(&self) -> String {
        let methods = self
            .methods
            .iter()
            .map(|method| {
                let args = method
                    .args
                    .iter()
                    .map(|arg| arg.to_json())
                    .collect::<Vec<_>>();
                let result = method.ret.as_ref().map(|ret| ret.to_json());
                json!({
                    "name": method.name.to_string(),
                    "args": args,
                    "result": result,
                })
            })
            .collect::<Vec<_>>();

        json!({
            "methods": methods,
        })
        .to_string()
    }
} 