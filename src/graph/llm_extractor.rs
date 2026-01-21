use std::time::Duration;

use anyhow::{Result, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{Entity, EntityType, Relation, RelationType};
use crate::config;

/// LLM API响应格式（OpenAI 兼容格式）
#[derive(Debug, Deserialize)]
struct LLMApiResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: String,
}

/// 知识图谱提取结果（完全由LLM定义）
#[derive(Debug, Deserialize)]
struct KnowledgeGraphResponse {
    #[serde(default)]
    entities: Vec<LLMEntity>,
    #[serde(default)]
    relations: Vec<LLMRelation>,
}

#[derive(Debug, Deserialize)]
struct LLMEntity {
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LLMRelation {
    source: String,
    target: String,
    #[serde(rename = "type")]
    relation_type: String,
    #[serde(default)]
    description: Option<String>,
}

/// LLM请求格式（OpenAI 兼容格式）
#[derive(Debug, Serialize)]
struct LLMRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

/// LLM知识图谱提取器（完全由LLM生成实体和关系）
pub struct LLMGraphExtractor {
    client: Client,
    api_url: String,
    api_key: Option<String>,
    model: String,
    enabled: bool,
}

impl LLMGraphExtractor {
    /// 从配置创建
    pub fn from_env() -> Self {
        let cfg = config::get();
        let llm_config = &cfg.llm;

        let enabled = llm_config.is_enabled();
        let api_url = llm_config.api_url.clone().unwrap_or_default();
        let api_key = llm_config.api_key.clone();
        let model = llm_config.model.clone();

        #[cfg(test)]
        let client_builder = Client::builder().timeout(Duration::from_secs(120)).no_proxy();
        #[cfg(not(test))]
        let client_builder = Client::builder().timeout(Duration::from_secs(120));
        let client = client_builder.build().unwrap();

        Self { client, api_url, api_key, model, enabled }
    }

    /// 检查是否已启用
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 提取知识图谱（实体和关系一次性提取，类型完全由LLM定义）
    pub async fn extract_knowledge_graph(&self, text: &str, context: &str) -> Result<(Vec<Entity>, Vec<Relation>)> {
        if !self.enabled {
            return Err(anyhow!("LLM未启用"));
        }

        if text.trim().is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let prompt = self.build_extraction_prompt(text, context);
        let response = self.call_llm(&prompt).await?;

        // 转换实体
        let entities: Vec<Entity> = response
            .entities
            .into_iter()
            .map(|e| {
                // 实体类型完全由LLM定义，直接使用Custom类型
                let entity_type = EntityType::Custom(e.entity_type);
                let mut entity = Entity::new(e.name, entity_type);
                if let Some(desc) = e.description {
                    entity = entity.with_property("description".to_string(), desc);
                }
                entity
            })
            .collect();

        // 转换关系
        let relations: Vec<Relation> = response
            .relations
            .into_iter()
            .map(|r| {
                // 关系类型完全由LLM定义，直接使用Custom类型
                let relation_type = RelationType::Custom(r.relation_type);
                let mut relation = Relation::new(r.source, r.target, relation_type);
                if let Some(desc) = r.description {
                    relation = relation.with_property("description".to_string(), desc);
                }
                relation
            })
            .collect();

        Ok((entities, relations))
    }

    /// 构建知识图谱提取提示词
    fn build_extraction_prompt(&self, text: &str, context: &str) -> String {
        format!(
            r#"你是一个专业的知识图谱构建专家。请仔细阅读以下文本，从中提取实体和实体之间的关系，构建知识图谱。

文档信息: {}

文本内容:
{}

## 任务要求

### 1. 实体提取
请识别文本中的所有重要实体，包括但不限于：
- 人名、组织名、公司名
- 技术名词、编程语言、框架、工具
- 项目名称、产品名称
- 地点、时间
- 专业术语、概念
- 其他你认为重要的实体

对于每个实体，请自定义一个最合适的类型名称（用中文），如：
- 人物、公司、技术、框架、编程语言、数据库、工具、项目、地点、时间段、概念、技能、职位 等

### 2. 关系提取
请识别实体之间的所有有意义的关系。关系类型请自定义（用中文），可以包括但不限于：
- 任职于、工作于、属于
- 使用、掌握、熟悉、精通
- 负责、开发、设计、实现
- 包含、组成、依赖
- 位于、发生于
- 关联、相关
- 其他你认为合适的关系类型

### 3. 输出要求

请严格按照以下JSON格式输出，不要输出任何其他内容：

```json
{{
  "entities": [
    {{
      "name": "实体名称",
      "type": "实体类型（自定义中文类型）",
      "description": "简短描述（可选）"
    }}
  ],
  "relations": [
    {{
      "source": "源实体名称",
      "target": "目标实体名称",
      "type": "关系类型（自定义中文类型）",
      "description": "关系描述（可选）"
    }}
  ]
}}
```

### 4. 注意事项
- 实体名称要准确，与文本中的表述一致
- 关系的source和target必须是entities中存在的实体名称
- 尽量提取所有有意义的实体和关系，不要遗漏
- 避免提取过于笼统或无意义的实体（如"系统"、"项目"等太泛化的词）
- 关系要基于文本内容，不要臆测
- 只输出JSON，不要有任何其他文字

请开始提取："#,
            context, text
        )
    }

    /// 调用LLM API
    async fn call_llm(&self, prompt: &str) -> Result<KnowledgeGraphResponse> {
        let request = LLMRequest {
            model: self.model.clone(),
            messages: vec![Message { role: "user".to_string(), content: prompt.to_string() }],
            max_tokens: Some(4000),
            temperature: Some(0.1), // 低温度，更确定性的输出
        };

        let mut req_builder = self.client.post(&self.api_url).json(&request);

        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
        }

        let response = req_builder.send().await.map_err(|e| anyhow!("LLM API请求失败: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("LLM API返回错误 {}: {}", status, body));
        }

        let body = response.text().await.map_err(|e| anyhow!("读取响应失败: {}", e))?;

        // 解析 OpenAI 格式的响应
        let api_response: LLMApiResponse =
            serde_json::from_str(&body).map_err(|e| anyhow!("解析LLM API响应失败: {}. 响应内容: {}", e, body))?;

        // 从第一个 choice 的 message 中提取内容
        let content =
            api_response.choices.first().ok_or_else(|| anyhow!("LLM响应中没有choices"))?.message.content.clone();

        // 尝试清理可能的markdown代码块标记
        let clean_content = self.clean_json_response(&content);

        // 解析 JSON 内容
        serde_json::from_str::<KnowledgeGraphResponse>(&clean_content)
            .map_err(|e| anyhow!("解析知识图谱JSON失败: {}. 内容: {}", e, clean_content))
    }

    /// 清理LLM响应中可能的markdown代码块标记
    fn clean_json_response(&self, content: &str) -> String {
        let content = content.trim();

        // 移除可能的 ```json 和 ``` 标记
        let content = if content.starts_with("```json") {
            content.strip_prefix("```json").unwrap_or(content)
        } else if content.starts_with("```") {
            content.strip_prefix("```").unwrap_or(content)
        } else {
            content
        };

        let content = if content.ends_with("```") { content.strip_suffix("```").unwrap_or(content) } else { content };

        content.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_graph_extractor_creation() {
        let extractor = LLMGraphExtractor::from_env();
        // 如果没有设置环境变量，应该是禁用状态
        // assert!(!extractor.is_enabled());
        println!("LLM enabled: {}", extractor.is_enabled());
    }

    #[test]
    fn test_clean_json_response() {
        let extractor = LLMGraphExtractor::from_env();

        let input = r#"```json
{"entities": [], "relations": []}
```"#;
        let cleaned = extractor.clean_json_response(input);
        assert_eq!(cleaned, r#"{"entities": [], "relations": []}"#);

        let input2 = r#"{"entities": [], "relations": []}"#;
        let cleaned2 = extractor.clean_json_response(input2);
        assert_eq!(cleaned2, r#"{"entities": [], "relations": []}"#);
    }
}
