//! 中文分词器模块 - 提供高性能的中英文混合分词功能

use std::sync::Arc;

use jieba_rs::Jieba;
use lazy_static::lazy_static;
use log::info;
use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

// 使用全局静态 Jieba 实例，避免重复初始化
lazy_static! {
    static ref JIEBA: Arc<Jieba> = {
        let jieba = Jieba::new();
        // 可以在这里加载自定义词典
        // 注意：jieba-rs 的 load_dict 需要可变引用和 BufRead
        // 如果需要加载自定义词典，应该在初始化时处理
        Arc::new(jieba)
    };
}

/// 分词模式
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentationMode {
    /// 搜索模式：适合搜索引擎（召回率高但较慢）
    Search,
    /// 全模式：速度较慢，但能识别更多的词汇
    All,
}

impl Default for SegmentationMode {
    fn default() -> Self {
        SegmentationMode::Search
    }
}

/// 快速中文分词器 - 针对性能优化
#[derive(Clone)]
pub struct FastChineseTokenizer {
    mode: SegmentationMode,
}

impl FastChineseTokenizer {
    /// 创建新的分词器
    pub fn new(mode: SegmentationMode) -> Self {
        FastChineseTokenizer { mode }
    }
    /// 创建全模式分词器
    pub fn all() -> Self {
        Self::new(SegmentationMode::All)
    }

    /// 执行分词
    pub fn segment(&self, text: &str) -> Vec<String> {
        let words = match self.mode {
            SegmentationMode::Search => {
                // 搜索模式：使用 cut_for_search，召回率高
                JIEBA.cut_for_search(text, false).into_iter().map(|s| s.to_string()).collect()
            }
            SegmentationMode::All => JIEBA.cut_all(text).into_iter().map(|s| s.to_string()).collect(),
        };
        info!("Segmented words: {:?}", words);
        words
    }
}

impl Tokenizer for FastChineseTokenizer {
    type TokenStream<'a> = ChineseTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        let tokens = self.segment(text);
        ChineseTokenStream::new(tokens)
    }
}

/// Token stream for Chinese text
pub struct ChineseTokenStream<'a> {
    tokens: Vec<String>,
    current_index: usize,
    current_offset: usize,
    token: Token,
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a> ChineseTokenStream<'a> {
    fn new(tokens: Vec<String>) -> Self {
        ChineseTokenStream {
            tokens,
            current_index: 0,
            current_offset: 0,
            token: Token::default(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<'a> TokenStream for ChineseTokenStream<'a> {
    fn advance(&mut self) -> bool {
        while self.current_index < self.tokens.len() {
            let token_text = &self.tokens[self.current_index];
            self.current_index += 1;

            // 跳过空的token
            if token_text.trim().is_empty() {
                self.current_offset += token_text.len();
                continue;
            }

            let start = self.current_offset;
            let end = start + token_text.len();

            self.token.text.clear();
            self.token.text.push_str(token_text);
            self.token.offset_from = start;
            self.token.offset_to = end;
            self.token.position = self.token.position.wrapping_add(1);

            self.current_offset = end;
            return true;
        }
        false
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}
