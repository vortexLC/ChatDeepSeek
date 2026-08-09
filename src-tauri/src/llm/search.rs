use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::commands::{AppState, emit};
use crate::llm::CancelToken;
use crate::models::SearchItem;

pub const ANYSEARCH_ENDPOINT: &str = "https://api.anysearch.com/v1/search";
pub const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";

const PROFESSIONAL_KEYWORDS: [&str; 32] = [
    "股票", "股价", "财报", "行情", "基金", "债券", "汇丰", "期货", "大宗", "期权",
    "论文", "文献", "期刊", "专利", "DOI", "citation",
    "法律", "法规", "判例", "法案", "司法解释",
    "诊断", "症状", "治疗", "临床试验", "药物", "药品", "肿瘤",
    "代码", "API", "函数库", "CVE",
];

pub fn search_tool_json() -> serde_json::Value {
    json!({
        "type": "function",
        "function": {
            "name": "web_search",
            "description": "搜索互联网获取实时、最新的信息。推荐策略：简单日常任务、事实类数据检索（如新闻、百科、常识、天气、名人资料）请使用 provider=tavily（快速轻量）；专业垂直领域内容（如财经股票、学术论文、医疗健康、法律条文、代码技术、安全漏洞）请使用 provider=anysearch（专业深度）。provider 默认 auto 由系统智能选择。",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "搜索查询关键词，应具体、包含核心实体与限定词，如「2026年5月英伟达财报 营收」"
                    },
                    "provider": {
                        "type": "string",
                        "enum": ["auto", "tavily", "anysearch"],
                        "description": "搜索引擎选择：auto 自动 / tavily 日常快速 / anysearch 专业深度，默认 auto"
                    }
                },
                "required": ["query"]
            }
        }
    })
}

pub fn search_tool_json_anthropic() -> serde_json::Value {
    let tool = search_tool_json();
    let f = &tool["function"];
    json!({
        "name": f["name"],
        "description": f["description"],
        "input_schema": f["parameters"]
    })
}

pub struct SearchOutcome {
    pub items: Vec<SearchItem>,
    pub summary: String,
}

fn resolve_provider(strategy: &str, query: &str) -> String {
    if strategy == "tavily" || strategy == "anysearch" {
        return strategy.to_string();
    }
    let q = query.to_lowercase();
    if PROFESSIONAL_KEYWORDS.iter().any(|k| q.contains(k)) {
        "anysearch".to_string()
    } else {
        "tavily".to_string()
    }
}

fn query_looks_chinese(query: &str) -> bool {
    query.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

pub async fn execute_search(
    app: &AppHandle,
    state: &AppState,
    conv_id: i64,
    arguments: &str,
    s: &crate::models::SearchSettings,
    token: &CancelToken,
) -> Result<SearchOutcome, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let query = args["query"].as_str().unwrap_or("").trim().to_string();
    if query.is_empty() {
        return Err("搜索参数缺少 query".into());
    }

    let max_results = s.max_results.clamp(1, 20);
    let requested = args["provider"].as_str().unwrap_or("auto");
    let strategy = if s.strategy == "auto" {
        requested
    } else {
        s.strategy.as_str()
    };
    let mut chosen = resolve_provider(strategy, &query);

    let tavily_ok = s.tavily_enabled && !s.tavily_key.trim().is_empty();
    let anysearch_ok = s.anysearch_enabled && !s.anysearch_key.trim().is_empty();
    if !tavily_ok && !anysearch_ok {
        return Err("未配置可用的搜索服务，请在设置中填写 Tavily 或 AnySearch 的 API Key".into());
    }
    if chosen == "tavily" && !tavily_ok {
        chosen = "anysearch".to_string();
    }
    if chosen == "anysearch" && !anysearch_ok {
        chosen = "tavily".to_string();
    }
    if chosen == "tavily" && !tavily_ok {
        return Err("Tavily 搜索服务不可用".into());
    }

    emit(app, conv_id, "status", Some("searching"));
    emit_provider(app, conv_id, &chosen);

    let mut outcome = match chosen.as_str() {
        "anysearch" => {
            anysearch_search(state, &s.anysearch_key, &query, max_results, token).await
        }
        _ => tavily_search(state, &s.tavily_key, &query, max_results, token).await,
    };

    if outcome.as_ref().map(|o| o.items.is_empty()).unwrap_or(true) {
        let fallback = if chosen == "tavily" { "anysearch" } else { "tavily" };        if (fallback == "tavily" && tavily_ok) || (fallback == "anysearch" && anysearch_ok) {
            outcome = match fallback {
                "anysearch" => {
                    anysearch_search(state, &s.anysearch_key, &query, max_results, token).await
                }
                _ => tavily_search(state, &s.tavily_key, &query, max_results, token).await,
            };
        }
    }

    let final_outcome = outcome.map_err(|e| format!("搜索失败: {e}"))?;
    for item in &final_outcome.items {
        emit_item(app, conv_id, item);
    }
    Ok(final_outcome)
}

fn emit_provider(app: &AppHandle, conv_id: i64, provider: &str) {
    let payload = json!({
        "kind": "status",
        "text": "searching",
        "conversation_id": conv_id,
        "search_provider": provider,
    });
    let _ = app.emit("chat_event", payload);
}

fn emit_item(app: &AppHandle, conv_id: i64, item: &SearchItem) {
    let payload = json!({
        "kind": "search_result",
        "conversation_id": conv_id,
        "item": item,
    });
    let _ = app.emit("chat_event", payload);
}

async fn tavily_search(
    state: &AppState,
    api_key: &str,
    query: &str,
    max_results: i64,
    token: &CancelToken,
) -> Result<SearchOutcome, String> {
    let body = json!({
        "api_key": api_key,
        "query": query,
        "search_depth": "basic",
        "max_results": max_results,
        "include_answer": false,
    });
    let resp = state
        .client
        .post(TAVILY_ENDPOINT)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Tavily 请求失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Tavily API 错误 ({status}): {}", text.chars().take(200).collect::<String>()));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Tavily 响应解析失败: {e}"))?;
    let results = v["results"].as_array().cloned().unwrap_or_default();
    let mut items = Vec::new();
    for r in results {
        if token.is_cancelled() {
            return Err("已停止生成".into());
        }
        let title = r["title"].as_str().unwrap_or("").to_string();
        let url = r["url"].as_str().unwrap_or("").to_string();
        let mut snippet = r["content"].as_str().unwrap_or("").to_string();
        if snippet.chars().count() > 320 {
            snippet = snippet.chars().take(320).collect();
        }
        if title.is_empty() && url.is_empty() {
            continue;
        }
        items.push(SearchItem {
            title,
            url,
            snippet,
            provider: "tavily".into(),
        });
    }
    Ok(SearchOutcome {
        summary: build_summary(&items, "Tavily"),
        items,
    })
}

async fn anysearch_search(
    state: &AppState,
    api_key: &str,
    query: &str,
    max_results: i64,
    token: &CancelToken,
) -> Result<SearchOutcome, String> {
    let mut body = json!({
        "query": query,
        "max_results": max_results,
    });
    if query_looks_chinese(query) {
        body["language"] = json!("zh-CN");
    }
    let mut req = state.client.post(ANYSEARCH_ENDPOINT);
    if !api_key.trim().is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("AnySearch 请求失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("AnySearch API 错误 ({status}): {}", text.chars().take(200).collect::<String>()));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("AnySearch 响应解析失败: {e}"))?;
    if let Some(code) = v["code"].as_i64() {
        if code != 0 {
            let msg = v["message"].as_str().unwrap_or("未知错误");
            return Err(format!("AnySearch 错误: {msg}"));
        }
    }
    let results = v["data"]["results"].as_array().cloned().unwrap_or_default();
    let mut items = Vec::new();
    for r in results {
        if token.is_cancelled() {
            return Err("已停止生成".into());
        }
        let title = r["title"].as_str().unwrap_or("").to_string();
        let url = r["url"].as_str().unwrap_or("").to_string();
        let mut snippet = r["snippet"]
            .as_str()
            .or_else(|| r["content"].as_str())
            .unwrap_or("")
            .to_string();
        if snippet.chars().count() > 320 {
            snippet = snippet.chars().take(320).collect();
        }
        if title.is_empty() && url.is_empty() {
            continue;
        }
        items.push(SearchItem {
            title,
            url,
            snippet,
            provider: "anysearch".into(),
        });
    }
    Ok(SearchOutcome {
        summary: build_summary(&items, "AnySearch"),
        items,
    })
}

fn build_summary(items: &[SearchItem], engine: &str) -> String {
    if items.is_empty() {
        return format!("({engine} 未返回任何搜索结果)");
    }
    let mut out = String::from(format!("以下是 {engine} 的搜索结果（共 {} 条）：\n", items.len()));
    for (i, it) in items.iter().enumerate() {
        let title = if it.title.is_empty() { "(无标题)" } else { it.title.as_str() };
        let snippet = if it.snippet.is_empty() {
            "(无摘要)".to_string()
        } else {
            it.snippet.clone()
        };
        out.push_str(&format!("{}. {} | 来源: {}\n摘要: {}\n", i + 1, title, it.url, snippet));
    }
    out
}
