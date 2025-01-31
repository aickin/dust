use crate::blocks::block::{parse_pair, replace_variables_in_string, Block, BlockType, Env};
use crate::http::request::HttpRequest;
use crate::Rule;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use js_sandbox::Script;
use pest::iterators::Pair;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Error {
    pub error: String,
}

#[derive(Clone)]
pub struct Curl {
    method: String,
    url: String,
    headers_code: String,
    body_code: String,
}

impl Curl {
    pub fn parse(block_pair: Pair<Rule>) -> Result<Self> {
        let mut method: Option<String> = None;
        let mut url: Option<String> = None;
        let mut headers_code: Option<String> = None;
        let mut body_code: Option<String> = None;

        for pair in block_pair.into_inner() {
            match pair.as_rule() {
                Rule::pair => {
                    let (key, value) = parse_pair(pair)?;
                    match key.as_str() {
                        "method" => method = Some(value),
                        "url" => url = Some(value),
                        "headers_code" => headers_code = Some(value),
                        "body_code" => body_code = Some(value),
                        _ => Err(anyhow!("Unexpected `{}` in `curl` block", key))?,
                    }
                }
                Rule::expected => Err(anyhow!("`expected` is not yet supported in `curl` block"))?,
                _ => unreachable!(),
            }
        }

        if !method.is_some() {
            Err(anyhow!("Missing required `method` in `curl` block"))?;
        }
        if !url.is_some() {
            Err(anyhow!("Missing required `url` in `curl` block"))?;
        }
        if !headers_code.is_some() {
            Err(anyhow!("Missing required `headers_code` in `curl` block"))?;
        }
        if !body_code.is_some() {
            Err(anyhow!("Missing required `body_code` in `curl` block"))?;
        }

        Ok(Curl {
            method: method.unwrap(),
            url: url.unwrap(),
            headers_code: headers_code.unwrap(),
            body_code: body_code.unwrap(),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct CurlResult {
    status: u16,
    body: Option<serde_json::Value>,
    error: Option<String>,
}

#[async_trait]
impl Block for Curl {
    fn block_type(&self) -> BlockType {
        BlockType::Curl
    }

    fn inner_hash(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update("curl".as_bytes());
        hasher.update(self.method.as_bytes());
        hasher.update(self.url.as_bytes());
        hasher.update(self.headers_code.as_bytes());
        hasher.update(self.body_code.as_bytes());
        format!("{}", hasher.finalize().to_hex())
    }

    async fn execute(&self, name: &str, env: &Env) -> Result<Value> {
        let config = env.config.config_for_block(name);

        let use_cache = match config {
            Some(v) => match v.get("use_cache") {
                Some(v) => match v {
                    Value::Bool(b) => *b,
                    _ => true,
                },
                None => true,
            },
            _ => true,
        };

        let e = env.clone();
        let headers_code = self.headers_code.clone();
        let headers_value: Value = match tokio::task::spawn_blocking(move || {
            let mut script = Script::from_string(headers_code.as_str())?
                .with_timeout(std::time::Duration::from_secs(10));
            script.call("_fun", (&e,))
        })
        .await?
        {
            Ok(v) => v,
            Err(e) => Err(anyhow!("Error in headers code: {}", e))?,
        };

        let e = env.clone();
        let body_code = self.body_code.clone();
        let body_value: Value = match tokio::task::spawn_blocking(move || {
            let mut script = Script::from_string(body_code.as_str())?
                .with_timeout(std::time::Duration::from_secs(10));
            script.call("_fun", (&e,))
        })
        .await?
        {
            Ok(v) => v,
            Err(e) => Err(anyhow!("Error in body code: {}", e))?,
        };

        let url = replace_variables_in_string(&self.url, "url", env)?;

        let request =
            HttpRequest::new(self.method.as_str(), url.as_str(), headers_value, body_value)?;

        let response = request
            .execute_with_cache(env.project.clone(), env.store.clone(), use_cache)
            .await?;

        Ok(json!(response))
    }

    fn clone_box(&self) -> Box<dyn Block + Sync + Send> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
