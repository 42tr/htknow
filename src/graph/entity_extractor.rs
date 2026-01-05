use aho_corasick::AhoCorasick;
use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;

use super::{Entity, EntityType};

/// 规则引擎实体提取器
pub struct RuleBasedExtractor {
    // 技术词典（Aho-Corasick自动机）
    tech_dict: AhoCorasick,
    tech_terms: Vec<String>,
    // 正则表达式模式
    date_pattern: Regex,
    url_pattern: Regex,
}

// 预定义的技术术语词典
static TECH_TERMS: Lazy<Vec<&str>> = Lazy::new(|| {
    vec![
        // 编程语言
        "Rust",
        "Python",
        "Java",
        "JavaScript",
        "TypeScript",
        "Go",
        "C++",
        "C#",
        // 框架
        "React",
        "Vue",
        "Angular",
        "Django",
        "Flask",
        "Spring",
        "Axum",
        "Tokio",
        // 数据库
        "MySQL",
        "PostgreSQL",
        "MongoDB",
        "Redis",
        "SQLite",
        "Elasticsearch",
        // 云服务
        "AWS",
        "Azure",
        "GCP",
        "Docker",
        "Kubernetes",
        "K8s",
        // 工具
        "Git",
        "GitHub",
        "GitLab",
        "Jenkins",
        "CI/CD",
        // 技术概念
        "API",
        "REST",
        "GraphQL",
        "gRPC",
        "HTTP",
        "HTTPS",
        "WebSocket",
        "微服务",
        "分布式",
        "云原生",
        "容器化",
        "虚拟化",
    ]
});

impl RuleBasedExtractor {
    pub fn new() -> Result<Self> {
        let tech_terms: Vec<String> = TECH_TERMS.iter().map(|s| s.to_string()).collect();
        let tech_dict = AhoCorasick::new(&tech_terms)?;

        // 日期正则：匹配 YYYY-MM-DD, YYYY/MM/DD, YYYY年MM月DD日等
        let date_pattern = Regex::new(r"(\d{4}[-/年]\d{1,2}[-/月]\d{1,2}日?)|(\d{4}年\d{1,2}月)|(\d{1,2}月\d{1,2}日)")?;

        // URL正则
        let url_pattern = Regex::new(r"https?://[^\s]+")?;

        Ok(Self { tech_dict, tech_terms, date_pattern, url_pattern })
    }

    /// 提取技术实体（快速词典匹配）
    pub fn extract_tech_entities(&self, text: &str) -> Vec<Entity> {
        let mut entities = Vec::new();

        for mat in self.tech_dict.find_iter(text) {
            let term = &self.tech_terms[mat.pattern().as_usize()];
            entities.push(Entity::new(term.clone(), EntityType::Technology));
        }

        // 去重
        entities.sort_by(|a, b| a.name.cmp(&b.name));
        entities.dedup_by(|a, b| a.name == b.name);

        entities
    }

    /// 提取日期实体
    pub fn extract_dates(&self, text: &str) -> Vec<Entity> {
        let mut entities = Vec::new();

        for cap in self.date_pattern.captures_iter(text) {
            if let Some(matched) = cap.get(0) {
                entities.push(Entity::new(matched.as_str().to_string(), EntityType::Date));
            }
        }

        // 去重
        entities.sort_by(|a, b| a.name.cmp(&b.name));
        entities.dedup_by(|a, b| a.name == b.name);

        entities
    }

    /// 提取URL实体
    pub fn extract_urls(&self, text: &str) -> Vec<Entity> {
        let mut entities = Vec::new();

        for cap in self.url_pattern.captures_iter(text) {
            if let Some(matched) = cap.get(0) {
                entities.push(
                    Entity::new(matched.as_str().to_string(), EntityType::Custom("url".to_string()))
                        .with_property("type".to_string(), "url".to_string()),
                );
            }
        }

        // 去重
        entities.sort_by(|a, b| a.name.cmp(&b.name));
        entities.dedup_by(|a, b| a.name == b.name);

        entities
    }

    /// 提取所有规则支持的实体
    pub fn extract_all(&self, text: &str) -> Vec<Entity> {
        let mut entities = Vec::new();

        entities.extend(self.extract_tech_entities(text));
        entities.extend(self.extract_dates(text));
        entities.extend(self.extract_urls(text));

        entities
    }
}

impl Default for RuleBasedExtractor {
    fn default() -> Self {
        Self::new().expect("Failed to create RuleBasedExtractor")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tech_entities() {
        let extractor = RuleBasedExtractor::new().unwrap();
        let text = "我们使用Rust和Python开发了一个基于React的前端应用，后端使用Axum框架。";
        let entities = extractor.extract_tech_entities(text);

        assert!(entities.len() > 0);
        assert!(entities.iter().any(|e| e.name == "Rust"));
        assert!(entities.iter().any(|e| e.name == "Python"));
        assert!(entities.iter().any(|e| e.name == "React"));
        assert!(entities.iter().any(|e| e.name == "Axum"));
    }

    #[test]
    fn test_extract_dates() {
        let extractor = RuleBasedExtractor::new().unwrap();
        let text = "项目开始于2024年1月15日，计划在2024-06-30完成。";
        let entities = extractor.extract_dates(text);

        assert!(entities.len() >= 2);
    }
}
